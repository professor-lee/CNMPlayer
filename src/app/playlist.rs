use super::*;

impl App {
    /// 搬运歌单页 / 专辑页的在途拉取（每帧调用，结果就绪才动状态）。
    pub(super) fn tick_playlist_fetch(&mut self) {
        let Some(slot) = self.browse.playlist_fetch.as_ref() else {
            return;
        };
        let Some(result) = peek_shared(&slot.future).cloned() else {
            return;
        };
        let kind = slot.kind;
        self.browse.playlist_fetch = None;
        self.downloads.page_kind = kind;

        match result {
            Ok(fetch) => {
                let title = fetch.title.clone();
                self.apply_playlist_fetch(fetch);
                self.set_runtime_status(format!("{} {}", kind.opened(self.config.language), title));
            }
            Err(message) => {
                self.browse.playlist.description =
                    format!("{}: {message}", kind.failed(self.config.language));
                self.set_runtime_status(format!(
                    "{}: {message}",
                    kind.failed(self.config.language)
                ));
            }
        }
    }

    pub(super) fn tick_playlist_page_fetch(&mut self) {
        let browse = self.browse.playlist.pagination.clone();
        let playback = self.playback.pagination.clone();
        for pagination in browse.iter().chain(playback.iter().filter(|pager| {
            !browse
                .as_ref()
                .is_some_and(|browse| browse.same_source(pager))
        })) {
            let Some(result) = pagination.take_ready() else {
                continue;
            };
            match result {
                Ok(page) => {
                    if playlist_pagination::apply_page(
                        pagination,
                        page,
                        &mut self.browse.playlist,
                        self.playback.pagination.as_ref(),
                        &mut self.playback.playback_queue,
                    ) {
                        self.persist_playback_memory();
                    }
                }
                Err(error) => {
                    if self
                        .playback
                        .pagination
                        .as_ref()
                        .is_some_and(|pager| pager.same_source(pagination))
                        && self.playback.page_advance.take().is_some()
                    {
                        self.playback.playback_state = PlaybackRuntimeState::Stopped;
                    }
                    self.set_runtime_status(format!(
                        "{}: {error}",
                        self.lang_text("歌单分页加载失败", "Playlist page load failed")
                    ));
                }
            }
        }
    }

    pub(super) fn maybe_fetch_playlist_page(&mut self) {
        if self.page == Page::Playlist
            && self.browse.playlist_fetch.is_none()
            && self.browse.playlist.focused_idx + 1 >= self.browse.playlist.tracks.len()
            && let Some(pagination) = &self.browse.playlist.pagination
        {
            pagination.request(self.api.clone());
        }
    }

    pub(super) fn maybe_prefetch_playback_page(&self) {
        if let Some(index) = self.playback.playback_index
            && index.saturating_add(3) >= self.playback.playback_queue.len()
            && let Some(pagination) = &self.playback.pagination
        {
            pagination.prefetch(self.api.clone());
        }
    }

    pub(super) async fn open_focused_playlist_album(&mut self) {
        let (album_id, title, fallback_cover_url, track_kind) = {
            let Some(track) = self
                .browse
                .playlist
                .tracks
                .get(self.browse.playlist.focused_idx)
            else {
                return;
            };

            let Some(album_id) = track.id.clone() else {
                self.set_runtime_status(self.lang_text(
                    "当前条目缺少专辑 ID，无法打开",
                    "The current item has no album ID",
                ));
                return;
            };

            (
                album_id,
                track.title.clone(),
                track.cover_url.clone(),
                track.kind,
            )
        };

        let is_author_section_album = self.playlist_return_page == Page::Author
            && matches!(
                track_kind,
                PlaylistTrackKind::Album | PlaylistTrackKind::Ep | PlaylistTrackKind::Single
            );
        let section_snapshot = if is_author_section_album {
            Some(self.browse.playlist.clone())
        } else {
            None
        };

        match self.load_album_detail(&album_id).await {
            Ok(()) => {
                self.playlist_section_return_snapshot = section_snapshot;
                if let (None, Some(url)) = (&self.browse.playlist.cover.image, fallback_cover_url) {
                    self.browse.playlist.cover.load(self.api.clone(), url)
                }
                self.set_runtime_status(format!(
                    "{} {}",
                    self.lang_text("已打开专辑", "Opened album"),
                    title
                ));
            }
            Err(err) => {
                self.set_runtime_status(format!(
                    "{}: {}",
                    self.lang_text("打开专辑失败", "Failed to open album"),
                    err
                ));
            }
        }
    }

    pub(super) async fn handle_playlist_key(&mut self, key: KeyEvent) {
        // 数据还在路上：占位页上的焦点/条目都没有意义，只留返回键。
        if self.browse.playlist_fetch.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Left) {
                self.page = self.playlist_return_page;
                self.browse.playlist_fetch = None;
            }
            return;
        }

        match key.code {
            KeyCode::Up | KeyCode::BackTab => {
                let _ = self.browse.playlist.focus_prev();
            }
            KeyCode::Down | KeyCode::Tab => {
                let _ = self.browse.playlist.focus_next();
                self.maybe_fetch_playlist_page();
            }
            KeyCode::Enter => self.play_focused_playlist_track().await,
            KeyCode::Esc | KeyCode::Left => {
                if let Some(pagination) = &self.browse.playlist.pagination {
                    pagination.cancel();
                }
                if let Some(snapshot) = self.playlist_section_return_snapshot.take() {
                    self.browse.playlist = snapshot;
                    return;
                }
                self.page = match self.playlist_return_page {
                    Page::Author => Page::Author,
                    Page::Search => Page::Search,
                    _ => Page::Home,
                };
            }
            _ => {}
        }
    }

    pub(super) async fn load_playlist_detail(&mut self, playlist_id: &str) -> Result<()> {
        let fetch = fetch_playlist_page(
            self.api.clone(),
            self.config.language,
            playlist_id.to_string(),
            None,
            self.browse.home_sidebar.liked_playlist_id.clone(),
            self.browse.home_sidebar.user_id.clone(),
        )
        .await
        .map_err(anyhow::Error::msg)?;
        self.apply_playlist_fetch(fetch);
        Ok(())
    }

    /// 把拉到的歌单/专辑数据落到状态上（阻塞版与 `tick_playlist_fetch` 共用）。
    ///
    /// 落状态即宣告"这一页换成了新来源"：在途的那次拉取随之作废，否则它迟到时
    /// 会把刚打开的页面覆盖成被放弃的那一份（`tick_playlist_fetch` 只看句柄）。
    pub(super) fn apply_playlist_fetch(&mut self, fetch: PlaylistFetch) {
        let playing_source = self
            .playback
            .pagination
            .as_ref()
            .filter(|pagination| fetch.paginated && pagination.cursor().source_id == fetch.id)
            .cloned();
        if let Some(pagination) = &playing_source
            && self
                .browse
                .playlist
                .pagination
                .as_ref()
                .is_some_and(|current| current.same_source(pagination))
        {
            // Reopening the playback source must not cancel its shared in-flight page.
            self.browse.playlist.pagination = None;
        }
        let liked = self.browse.apply_playlist(fetch, &self.api);
        if let Some(pagination) = playing_source {
            self.browse.playlist.set_tracks(
                self.playback
                    .playback_queue
                    .iter()
                    .map(PlaybackTrack::as_playlist_track)
                    .collect(),
            );
            self.browse.playlist.total_tracks = pagination.cursor().total_tracks;
            self.browse.playlist.pagination = Some(pagination);
        }

        if let Some(liked) = liked {
            if let Some(profile) = liked.profile {
                self.apply_account_profile(profile);
            }
            self.apply_liked_song_ids(liked.ids);
        }
    }

    pub(super) async fn load_daily_recommend_playlist(&mut self) -> Result<()> {
        let response = self.api.recommend_songs().await?;
        let code = response_code(&response);
        if code != 200 {
            return Err(anyhow!(
                "请求失败({}): {}",
                code,
                response_message(&response)
            ));
        }

        let songs = home_daily_song_items(&response.body).ok_or_else(|| {
            anyhow!(self.lang_text("每日推荐数据缺失", "Daily recommendations are missing"))
        })?;

        let tracks = parse_tracks(songs);
        if tracks.is_empty() {
            return Err(anyhow!(
                self.lang_text("每日推荐为空", "Daily recommendations are empty")
            ));
        }

        let cover_url = tracks.iter().find_map(|track| track.cover_url.clone());

        // 这一页换成每日推荐：在途的占位拉取作废（同 `apply_playlist_fetch`）。
        self.browse.playlist_fetch = None;
        self.browse.playlist.id = Some(HOME_DAILY_RECOMMEND_TILE_ID.to_string());
        self.browse.playlist.title = self
            .lang_text("每日推荐", "Daily Recommendations")
            .to_string();
        self.browse.playlist.artist = self
            .lang_text("网易云音乐", "Netease Cloud Music")
            .to_string();
        self.browse.playlist.description = self
            .lang_text(
                "来自网易云每日推荐歌曲，按 Enter 播放",
                "Daily songs from Netease. Press Enter to play",
            )
            .to_string();
        self.browse.playlist.set_tracks(tracks);
        if let Some(x) = cover_url {
            self.browse.playlist.cover.load(self.api.clone(), x)
        }
        Ok(())
    }

    pub(super) async fn load_album_detail(&mut self, album_id: &str) -> Result<()> {
        let fetch = fetch_album_page(
            self.api.clone(),
            self.config.language,
            album_id.to_string(),
            None,
        )
        .await
        .map_err(anyhow::Error::msg)?;
        self.apply_playlist_fetch(fetch);
        Ok(())
    }
}
