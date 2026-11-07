use super::App;
use super::Page;
use super::api::ApiState;
use super::likes::LikeToggleTask;
use super::likes::ToggleOutcome;
use super::owned_task::peek_shared;
use super::owned_task::spawn_shared;

/// 歌手 / 歌单 / 专辑收藏键：`artist:ID`、`playlist:ID`、`album:ID`。
pub(super) fn artist_key(id: &str) -> String {
    format!("artist:{id}")
}

pub(super) fn playlist_key(id: &str) -> String {
    format!("playlist:{id}")
}

pub(super) fn album_key(id: &str) -> String {
    format!("album:{id}")
}

/// 收藏写入请求。成功 `Ok(())`；失败带可展示的原因（接口错误码或传输错误）。
async fn subscribe_request(mut api: ApiState, key: String, target: bool) -> Result<(), String> {
    let Some((kind, id)) = key.split_once(':') else {
        return Err(format!("bad subscription key: {key}"));
    };
    let result = match kind {
        "artist" => api.artist_sub(id, target).await,
        "playlist" => api.playlist_subscribe(id, target).await,
        "album" => api.album_sub(id, target).await,
        _ => return Err(format!("bad subscription key: {key}")),
    };
    match result {
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

impl App {
    /// 当前页面可收藏对象的键；不可收藏（自建歌单、推荐页、其余页面）为 `None`。
    pub(super) fn current_subscription_key(&self) -> Option<String> {
        match self.page {
            Page::Playlist => self.browse.playlist.subscribe_key.clone(),
            Page::Author => self.browse.author.subscribe_key.clone(),
            _ => None,
        }
    }

    /// 该键当前应显示的收藏态：未决意图优先，其次服务端已确认值。
    pub(crate) fn subscription_displayed(&self, key: &str) -> bool {
        self.browse.subscriptions.displayed(key)
    }

    /// 收藏 / 取消收藏当前页的歌手 / 歌单 / 专辑：立刻按期望值改界面（乐观更新），
    /// 请求由 `tick_subscription_sync` 派发，输入路径不 `await` 网络。
    pub(super) fn toggle_subscription_hotkey(&mut self) {
        let Some(key) = self.current_subscription_key() else {
            self.set_runtime_status(self.lang_text(
                "当前页面没有可收藏的歌手或歌单",
                "Nothing to follow or collect on this page",
            ));
            return;
        };
        let target = !self.browse.subscriptions.displayed(&key);
        self.browse.subscriptions.set_intent(key, target);
    }

    /// 每帧收敛收藏状态：先搬在途结果，再按需补发请求。
    pub(super) fn tick_subscription_sync(&mut self) {
        self.pump_subscription_toggle();
        self.dispatch_subscription_toggle();
    }

    fn subscription_label(&self, key: &str) -> &'static str {
        match key.split_once(':').map(|(kind, _)| kind) {
            Some("artist") => self.lang_text("歌手", "artist"),
            Some("album") => self.lang_text("专辑", "album"),
            _ => self.lang_text("歌单", "playlist"),
        }
    }

    fn pump_subscription_toggle(&mut self) {
        let Some(pending) = self.browse.subscriptions.toggle.as_ref() else {
            return;
        };
        let Some(result) = peek_shared(&pending.fut).cloned() else {
            return;
        };
        let key = pending.song_id.clone();
        let target = pending.target;
        self.browse.subscriptions.toggle = None;

        let label = self.subscription_label(&key);
        match self
            .browse
            .subscriptions
            .on_toggle_result(&key, target, result)
        {
            ToggleOutcome::Settled { liked } => {
                self.set_runtime_status(format!(
                    "{} {}",
                    if liked {
                        self.lang_text("已收藏", "Followed")
                    } else {
                        self.lang_text("已取消收藏", "Unfollowed")
                    },
                    label
                ));
            }
            ToggleOutcome::Superseded | ToggleOutcome::StaleFailure => {}
            ToggleOutcome::Failed { message } => {
                self.set_runtime_status(format!(
                    "{}: {}",
                    self.lang_text("收藏操作失败", "Follow operation failed"),
                    message
                ));
            }
        }
    }

    fn dispatch_subscription_toggle(&mut self) {
        let _ = self.browse.subscriptions.drop_satisfied_intent();
        let Some((key, target)) = self.browse.subscriptions.pending_dispatch() else {
            return;
        };
        let fut = subscribe_request(self.api.clone(), key.clone(), target);
        let fut: LikeToggleTask = Box::pin(async move { Some(fut.await) });
        self.browse
            .subscriptions
            .begin_toggle(key, target, spawn_shared(fut, self.wake.clone()));
    }
}
