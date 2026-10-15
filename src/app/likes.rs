use super::*;

/// 「我喜欢的音乐」全量 id 的刷新结果（打开该歌单时顺带做一次）。
#[derive(Clone)]
pub(crate) struct LikedRefresh {
    pub(crate) ids: HashSet<String>,
    /// 只有当场取过账号档案时才有值（否则沿用已有 uid，不动档案）。
    pub(crate) profile: Option<AccountProfile>,
}

/// 「我喜欢的音乐」全量 id：uid 已知就直接用，否则顺带取一次账号档案。
///
/// 任一步失败都返回 `None`（旧行为是忽略这里的错误，不影响歌单页打开）。
pub(crate) async fn fetch_liked_refresh(
    api: &mut ApiState,
    language: Language,
    uid_hint: Option<String>,
) -> Option<LikedRefresh> {
    let (uid, profile) = match uid_hint {
        Some(uid) => (uid, None),
        None => {
            let profile = fetch_account_profile(api, language).await.ok()?;
            (profile.uid.clone(), Some(profile))
        }
    };

    let ids = fetch_liked_song_ids(api, &uid, language).await.ok()?;
    Some(LikedRefresh { ids, profile })
}

/// 收藏写入请求。成功 `Ok(())`；失败带可展示的原因（接口错误码或传输错误）。
///
/// 返回 `String` 而非 `anyhow::Error`：结果要跨 future 边界搬运，需 `Clone`。
pub(crate) async fn like_song_request(mut api: ApiState, song_id: String, target: bool) -> Result<(), String> {
    match api.like_song(&song_id, target).await {
        Ok(response) => {
            let code = response
                .body
                .get("code")
                .and_then(|value| value.as_i64())
                .unwrap_or(response.status);
            if code == 200 {
                Ok(())
            } else {
                Err(code.to_string())
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

/// 服务端收藏态确认请求（切歌后核对账号里的真实状态）。
pub(crate) async fn song_like_check_request(mut api: ApiState, song_id: String) -> Result<bool, ()> {
    let Ok(song_id_num) = song_id.parse::<u64>() else {
        return Err(());
    };

    let ids_json = format!("[{song_id_num}]");
    let Ok(response) = api.song_like_check(&ids_json).await else {
        return Err(());
    };
    if response_code(&response) != 200 {
        return Err(());
    }

    parse_song_like_check_result(&response.body, &song_id).ok_or(())
}

pub(crate) type LikeToggleFuture = SharedTask<Result<(), String>>;

pub(crate) type LikeVerifyFuture = SharedTask<Result<bool, ()>>;

/// 装箱后的任务体（`spawn_shared` 的入参类型）。
pub(crate) type LikeToggleTask = Pin<Box<dyn Future<Output = Option<Result<(), String>>>>>;

pub(crate) type LikeVerifyTask = Pin<Box<dyn Future<Output = Option<Result<bool, ()>>>>>;

/// 在途的收藏写入。
pub(crate) struct PendingLikeToggle {
    pub(crate) song_id: String,
    pub(crate) target: bool,
    pub(crate) fut: LikeToggleFuture,
}

/// 在途的收藏确认。
pub(crate) struct PendingLikeVerify {
    pub(crate) song_id: String,
    pub(crate) fut: LikeVerifyFuture,
}

/// 一次收藏写入回包的收敛结果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ToggleOutcome {
    /// 仍是当前意图：确认值已写入，界面据此提示。
    Settled { liked: bool },
    /// 已被更新的意图取代：确认值已写入，不提示，由 reconcile 补发新意图。
    Superseded,
    /// 失败且仍是当前意图：意图已放弃，显示应回滚到已确认值。
    Failed { message: String },
    /// 失败但已被取代：忽略。
    StaleFailure,
}

/// 收藏的「期望 / 已确认」双轨状态机。
///
/// 点击只写 `desired` 并把界面切到期望值（乐观更新），真实请求由
/// `App::tick_like_sync` 在每帧派发与收敛——输入路径上不再 `await` 网络，
/// 收藏不会冻结单线程事件循环的动画。
///
/// - 连点：只保留最后一次期望；在途请求串行化，其回包不会覆盖更新的意图。
/// - 失败：回滚显示到已确认集合，并写状态行。
///
/// 本结构不依赖 `App` 与网络：回包由调用方（tick 或单测）喂进 `on_*_result`。
#[derive(Default)]
pub(crate) struct LikeMachine {
    /// 用户最后一次意图（song_id, target）。
    pub(crate) desired: Option<(String, bool)>,
    pub(crate) toggle: Option<PendingLikeToggle>,
    pub(crate) verify: Option<PendingLikeVerify>,
    /// 已确认的收藏集合（服务端口径）。
    pub(crate) confirmed: HashSet<String>,
}

impl LikeMachine {
    /// 服务端已确认该曲目被收藏。
    pub(crate) fn is_confirmed(&self, song_id: &str) -> bool {
        self.confirmed.contains(song_id)
    }

    /// 该曲目当前应显示的状态：未决意图优先，其次已确认值。
    pub(crate) fn displayed(&self, song_id: &str) -> bool {
        match self.desired.as_ref() {
            Some((id, target)) if id == song_id => *target,
            _ => self.is_confirmed(song_id),
        }
    }

    /// 用整份「喜欢的歌曲」列表覆盖已确认集合。
    pub(crate) fn replace_confirmed(&mut self, confirmed: HashSet<String>) {
        self.confirmed = confirmed;
    }

    /// 记录一次点击。新意图优先于任何在途的服务端确认。
    pub(crate) fn set_intent(&mut self, song_id: String, target: bool) {
        self.desired = Some((song_id, target));
        self.verify = None;
    }

    /// 现在该补发的写入请求；`None` 表示无需发（无意图、已满足、或有在途）。
    pub(crate) fn pending_dispatch(&self) -> Option<(String, bool)> {
        if self.toggle.is_some() {
            return None;
        }
        let (song_id, target) = self.desired.as_ref()?;
        if self.is_confirmed(song_id) == *target {
            return None;
        }
        Some((song_id.clone(), *target))
    }

    /// 期望已被满足（例如连点两次回到原状态）时清掉意图，返回该曲目 id。
    pub(crate) fn drop_satisfied_intent(&mut self) -> Option<String> {
        if self.toggle.is_some() {
            return None;
        }
        let (song_id, target) = self.desired.as_ref()?;
        if self.is_confirmed(song_id) != *target {
            return None;
        }
        let song_id = song_id.clone();
        self.desired = None;
        Some(song_id)
    }

    pub(crate) fn begin_toggle(&mut self, song_id: String, target: bool, fut: LikeToggleFuture) {
        self.toggle = Some(PendingLikeToggle {
            song_id,
            target,
            fut,
        });
    }

    pub(crate) fn begin_verify(&mut self, song_id: String, fut: LikeVerifyFuture) {
        self.verify = Some(PendingLikeVerify { song_id, fut });
    }

    /// 收敛一次写入回包。
    pub(crate) fn on_toggle_result(
        &mut self,
        song_id: &str,
        target: bool,
        result: Result<(), String>,
    ) -> ToggleOutcome {
        let still_wanted = matches!(
            self.desired.as_ref(),
            Some((id, value)) if id == song_id && *value == target
        );
        if still_wanted {
            self.desired = None;
        }

        match result {
            Ok(()) => {
                self.set_confirmed(song_id, target);
                if still_wanted {
                    ToggleOutcome::Settled { liked: target }
                } else {
                    ToggleOutcome::Superseded
                }
            }
            Err(message) => {
                if still_wanted {
                    ToggleOutcome::Failed { message }
                } else {
                    ToggleOutcome::StaleFailure
                }
            }
        }
    }

    /// 收敛一次确认回包；返回是否真的写入了确认值。
    pub(crate) fn on_verify_result(&mut self, song_id: &str, result: Result<bool, ()>) -> bool {
        let Ok(liked) = result else {
            return false;
        };
        // 更新的意图 / 在途写入优先，别被旧确认覆盖。
        let superseded = matches!(self.desired.as_ref(), Some((id, _)) if id == song_id)
            || self
                .toggle
                .as_ref()
                .is_some_and(|pending| pending.song_id == song_id);
        if superseded {
            return false;
        }

        self.set_confirmed(song_id, liked);
        true
    }

    pub(crate) fn set_confirmed(&mut self, song_id: &str, liked: bool) {
        if liked {
            self.confirmed.insert(song_id.to_string());
        } else {
            self.confirmed.remove(song_id);
        }
    }

    /// 登出等场景：连已确认集合一起丢弃。
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

///「我喜欢的音乐」全量 id 集合。
pub(crate) async fn fetch_liked_song_ids(
    api: &mut ApiState,
    uid: &str,
    lang: Language,
) -> Result<HashSet<String>> {
    let response = api.likelist(uid).await?;
    let code = response_code(&response);
    if code != 200 {
        return Err(anyhow!(
            "{}({}): {}",
            lang_text(lang, "喜爱列表请求失败", "Failed to fetch liked songs"),
            code,
            response_message(&response)
        ));
    }

    Ok(parse_likelist_song_ids(&response.body))
}

pub(crate) fn parse_song_like_check_result(body: &Value, song_id: &str) -> Option<bool> {
    if let Some(value) = body.pointer(&format!("/data/{song_id}"))
        && let Some(liked) = parse_song_like_check_flag(value)
    {
        return Some(liked);
    }

    for pointer in [
        "/data/0/liked",
        "/songs/0/liked",
        "/data/songs/0/liked",
        "/liked",
    ] {
        if let Some(value) = body.pointer(pointer)
            && let Some(liked) = parse_song_like_check_flag(value)
        {
            return Some(liked);
        }
    }

    if let Some(obj) = body.get("data").and_then(|value| value.as_object()) {
        for value in obj.values() {
            if let Some(liked) = parse_song_like_check_flag(value) {
                return Some(liked);
            }
        }
    }

    None
}

pub(crate) fn parse_song_like_check_flag(value: &Value) -> Option<bool> {
    if let Some(flag) = value.as_bool() {
        return Some(flag);
    }

    if let Some(flag) = value.as_i64() {
        return match flag {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }

    if let Some(flag) = value.as_u64() {
        return match flag {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }

    if let Some(flag) = value.as_str() {
        return match flag.trim().to_ascii_lowercase().as_str() {
            "0" | "false" => Some(false),
            "1" | "true" => Some(true),
            _ => None,
        };
    }

    None
}

pub(crate) fn parse_likelist_song_ids(body: &Value) -> HashSet<String> {
    let mut out = HashSet::new();
    let arrays = [
        body.pointer("/ids").and_then(|value| value.as_array()),
        body.pointer("/data/ids").and_then(|value| value.as_array()),
        body.pointer("/data").and_then(|value| value.as_array()),
    ];

    for maybe_arr in arrays {
        let Some(arr) = maybe_arr else {
            continue;
        };

        for item in arr {
            if let Some(value) = item
                .as_i64()
                .map(|value| value.to_string())
                .or_else(|| item.as_u64().map(|value| value.to_string()))
                .or_else(|| item.as_str().map(|value| value.trim().to_string()))
                && !value.is_empty()
            {
                out.insert(value);
            }
        }
    }

    out
}

impl App {
    /// 切歌后刷新收藏态：先用本地缓存（含未决意图）立即显示，服务端确认
    /// 交给 `tick_like_sync` 收敛——不再阻塞事件循环。
    pub(crate) fn refresh_now_playing_like_state(&mut self) {
        let Some(song_id) = self.current_song_id() else {
            self.playback.now_playing_liked = false;
            return;
        };

        self.sync_like_display(&song_id);

        let fut = song_like_check_request(self.api.clone(), song_id.clone());
        let fut: LikeVerifyTask = Box::pin(async move { Some(fut.await) });
        self.playback
            .like_machine
            .begin_verify(song_id, spawn_shared(fut, self.wake.clone()));
    }

    /// 把某首歌的显示值同步到状态机（未决意图优先，其次已确认值）。
    pub(crate) fn sync_like_display(&mut self, song_id: &str) {
        if self.current_song_id().as_deref() == Some(song_id) {
            self.playback.now_playing_liked = self.playback.like_machine.displayed(song_id);
        }
    }

    /// 收藏切换：立刻按期望值改界面（乐观更新），请求由 `tick_like_sync` 派发。
    pub(crate) fn toggle_like_hotkey(&mut self) {
        let Some(song_id) = self.current_song_id() else {
            self.set_runtime_status(self.lang_text(
                "当前没有可收藏的歌曲",
                "No song is available for like/unlike",
            ));
            return;
        };

        let target = !self.playback.now_playing_liked;
        self.playback.like_machine.set_intent(song_id, target);
        self.playback.now_playing_liked = target;
    }

    /// 每帧收敛收藏状态：先搬在途结果，再按需补发请求。
    pub(crate) fn tick_like_sync(&mut self) {
        self.pump_like_toggle();
        self.pump_like_verify();
        self.dispatch_like_toggle();
    }

    pub(crate) fn pump_like_toggle(&mut self) {
        let Some(pending) = self.playback.like_machine.toggle.as_ref() else {
            return;
        };
        let Some(result) = peek_shared(&pending.fut).cloned() else {
            return;
        };

        let song_id = pending.song_id.clone();
        let target = pending.target;
        self.playback.like_machine.toggle = None;

        match self
            .playback
            .like_machine
            .on_toggle_result(&song_id, target, result)
        {
            ToggleOutcome::Settled { liked } => {
                self.sync_like_display(&song_id);
                self.set_runtime_status(if liked {
                    self.lang_text("已收藏当前歌曲", "Liked current song")
                        .to_string()
                } else {
                    self.lang_text("已取消收藏当前歌曲", "Unliked current song")
                        .to_string()
                });
            }
            ToggleOutcome::Superseded => self.sync_like_display(&song_id),
            ToggleOutcome::Failed { message } => {
                // 意图已被状态机放弃，显示回滚到已确认值。
                self.sync_like_display(&song_id);
                self.set_runtime_status(format!(
                    "{}: {}",
                    self.lang_text("收藏操作失败", "Like operation failed"),
                    message
                ));
            }
            ToggleOutcome::StaleFailure => {}
        }
    }

    pub(crate) fn pump_like_verify(&mut self) {
        let Some(pending) = self.playback.like_machine.verify.as_ref() else {
            return;
        };
        let Some(result) = peek_shared(&pending.fut).cloned() else {
            return;
        };

        let song_id = pending.song_id.clone();
        self.playback.like_machine.verify = None;

        if self
            .playback
            .like_machine
            .on_verify_result(&song_id, result)
        {
            self.sync_like_display(&song_id);
        }
    }

    pub(crate) fn dispatch_like_toggle(&mut self) {
        if let Some(song_id) = self.playback.like_machine.drop_satisfied_intent() {
            self.sync_like_display(&song_id);
        }

        let Some((song_id, target)) = self.playback.like_machine.pending_dispatch() else {
            return;
        };

        let fut = like_song_request(self.api.clone(), song_id.clone(), target);
        let fut: LikeToggleTask = Box::pin(async move { Some(fut.await) });
        self.playback.like_machine.begin_toggle(
            song_id,
            target,
            spawn_shared(fut, self.wake.clone()),
        );
    }

    pub(crate) async fn refresh_liked_song_cache(&mut self) -> Result<()> {
        let uid = self.resolve_current_user_id().await?;
        let ids = fetch_liked_song_ids(&mut self.api, &uid, self.config.language).await?;
        self.apply_liked_song_ids(ids);
        Ok(())
    }

    /// 应用「我喜欢的音乐」全量 id 集合。
    pub(crate) fn apply_liked_song_ids(&mut self, ids: HashSet<String>) {
        self.playback.like_machine.replace_confirmed(ids);
        self.refresh_now_playing_like_state();
    }

    pub(crate) fn is_liked_playlist(&self, playlist_id: &str, title: Option<&str>) -> bool {
        is_liked_playlist(
            self.browse.home_sidebar.liked_playlist_id.as_deref(),
            playlist_id,
            title,
        )
    }
}
