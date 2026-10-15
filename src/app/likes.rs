use super::*;

impl App {
    /// 切歌后刷新收藏态：先用本地缓存（含未决意图）立即显示，服务端确认
    /// 交给 `tick_like_sync` 收敛——不再阻塞事件循环。
    pub(super) fn refresh_now_playing_like_state(&mut self) {
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
    pub(super) fn sync_like_display(&mut self, song_id: &str) {
        if self.current_song_id().as_deref() == Some(song_id) {
            self.playback.now_playing_liked = self.playback.like_machine.displayed(song_id);
        }
    }

    /// 收藏切换：立刻按期望值改界面（乐观更新），请求由 `tick_like_sync` 派发。
    pub(super) fn toggle_like_hotkey(&mut self) {
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
    pub(super) fn tick_like_sync(&mut self) {
        self.pump_like_toggle();
        self.pump_like_verify();
        self.dispatch_like_toggle();
    }

    pub(super) fn pump_like_toggle(&mut self) {
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

    pub(super) fn pump_like_verify(&mut self) {
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

    pub(super) fn dispatch_like_toggle(&mut self) {
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

    pub(super) async fn refresh_liked_song_cache(&mut self) -> Result<()> {
        let uid = self.resolve_current_user_id().await?;
        let ids = fetch_liked_song_ids(&mut self.api, &uid, self.config.language).await?;
        self.apply_liked_song_ids(ids);
        Ok(())
    }

    /// 应用「我喜欢的音乐」全量 id 集合。
    pub(super) fn apply_liked_song_ids(&mut self, ids: HashSet<String>) {
        self.playback.like_machine.replace_confirmed(ids);
        self.refresh_now_playing_like_state();
    }

    pub(super) fn is_liked_playlist(&self, playlist_id: &str, title: Option<&str>) -> bool {
        is_liked_playlist(
            self.browse.home_sidebar.liked_playlist_id.as_deref(),
            playlist_id,
            title,
        )
    }
}
