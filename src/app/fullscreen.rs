use super::*;

impl App {
    pub async fn fullscreen_tick_playback(&mut self) {
        self.tick_audio().await;
        self.tick_cover_fetch();
        self.tick_lyric_fetch();
        self.apply_mpris_control_events().await;
        self.sync_mpris_exposure();
        // 全屏页不跑宿主主循环，收藏的派发/收敛要在这里推进。
        self.tick_like_sync();
        // 下载结果与图标状态同理（全屏页也能发起下载）。
        self.tick_download();
    }

    pub fn fullscreen_playback_snapshot(&self) -> FullscreenPlaybackSnapshot {
        FullscreenPlaybackSnapshot {
            queue: self.playback.playback_queue.clone(),
            current_index: self.playback.playback_index,
            now_playing: self.playback.now_playing.clone(),
            now_playing_liked: self.playback.now_playing_liked,
            playlist_cover: self
                .playback
                .fullscreen_playlist_cover()
                .map(|cover| cover.to_vec()),
            state: self.playback.playback_state,
            repeat_mode: self.playback.playback_repeat_mode,
            position: self.playback.audio_player.display_position(),
        }
    }

    pub fn fullscreen_runtime_snapshot(&self) -> FullscreenRuntimeSnapshot {
        FullscreenRuntimeSnapshot {
            current_index: self.playback.playback_index,
            now_playing_liked: self.playback.now_playing_liked,
            state: self.playback.playback_state,
            repeat_mode: self.playback.playback_repeat_mode,
            position: self.playback.audio_player.display_position(),
            volume: self.playback.audio_player.volume(),
            seeking: self.playback.audio_player.is_seeking(),
            download: self.current_download_state(),
        }
    }

    pub fn fullscreen_metadata_signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.playback.playback_queue.len().hash(&mut hasher);
        self.playback
            .playback_queue_cover
            .as_ref()
            .map(Vec::len)
            .unwrap_or(0)
            .hash(&mut hasher);

        for track in &self.playback.playback_queue {
            track.song_id.hash(&mut hasher);
            track.duration_ms.hash(&mut hasher);
        }

        if let Some(track) = self.playback.now_playing.as_ref() {
            track.song_id.hash(&mut hasher);
            track.duration_ms.hash(&mut hasher);
            track
                .cover
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0)
                .hash(&mut hasher);
            track
                .lyrics
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0)
                .hash(&mut hasher);
            track
                .lyrics
                .as_ref()
                .and_then(|v| v.last().map(|line| line.start_ms))
                .unwrap_or(0)
                .hash(&mut hasher);
        }

        hasher.finish()
    }

    pub async fn fullscreen_toggle_play_pause(&mut self) {
        self.toggle_play_pause_hotkey().await;
    }

    pub async fn fullscreen_play_previous(&mut self) {
        self.play_previous_hotkey().await;
    }

    pub async fn fullscreen_play_next(&mut self) {
        self.play_next_hotkey().await;
    }

    pub async fn fullscreen_play_queue_index(&mut self, index: usize) {
        if index < self.playback.playback_queue.len() {
            self.play_queue_index(index, false).await;
        }
    }

    pub fn fullscreen_seek_to_ratio(&mut self, ratio: f32) {
        self.seek_to_ratio(ratio);
    }

    pub fn fullscreen_set_volume(&mut self, volume: f32) {
        self.playback.audio_player.set_volume(volume);
    }

    pub fn fullscreen_toggle_repeat_mode(&mut self) {
        self.cycle_repeat_mode_hotkey();
    }

    pub async fn fullscreen_toggle_like(&mut self) {
        self.toggle_like_hotkey();
    }

    pub fn fullscreen_request_queue_page(&self) {
        if let Some(pagination) = &self.playback.pagination {
            pagination.request(self.api.clone());
        }
    }

    pub fn consume_fullscreen_launch_request(&mut self) -> bool {
        std::mem::take(&mut self.launch_fullscreen_requested)
    }

    pub fn open_settings_from_fullscreen(&mut self) {
        if self.page != Page::Login {
            self.open_settings();
        }
    }

    /// 全屏页点了作者名：立即落占位作者页 + 派发后台拉取，结果由 `App::tick_author_fetch`
    /// 搬进来（宿主循环在这期间照常重绘、照常响应输入）。
    ///
    /// `index` 是显示串（`now_playing.artist`，形如 "A / B"）里的段序号：
    /// 全屏页信息区按字符位置分段命中，点谁的名字就传谁的序号。
    /// 全屏页只有显示名（ID 要靠 `song/detail` 的 `ar` 补），
    /// 本机音频 / 无播放时解析不出来，只在状态行里说明，不换页。
    pub fn open_author_page_from_fullscreen(&mut self, index: usize) {
        let Some(song_id) = self.current_song_id() else {
            self.set_runtime_status(
                self.lang_text("当前没有正在播放的歌曲", "Nothing is playing right now"),
            );
            return;
        };

        let artist_line = self
            .playback
            .now_playing
            .as_ref()
            .map(|track| track.artist.clone())
            .unwrap_or_default();
        // 标题先显示点中的那段名字；作者 ID 解析失败时也只是把错误写进简介。
        let title = {
            let line = artist_line.trim();
            let clicked = artist_name_segments(&artist_line)
                .get(index)
                .map(|name| name.trim())
                .filter(|name| !name.is_empty())
                .unwrap_or(line);
            if clicked.is_empty() {
                self.lang_text("作者页", "Artist Page").to_string()
            } else {
                clicked.to_string()
            }
        };

        self.browse.author = AuthorState::placeholder(
            title,
            self.lang_text("正在加载作者…", "Loading artist…")
                .to_string(),
        );
        // 从全屏页进来：Esc 回首页，而不是回搜索页（那里可能不是用户来时的页面）。
        self.author_return_page = Page::Home;
        self.page = Page::Author;

        let fut = fetch_author_page(
            self.api.clone(),
            self.config.language,
            song_id,
            index,
            artist_line,
        );
        let fut: AuthorFetchTask = Box::pin(async move { Some(fut.await) });
        self.browse.author_fetch = Some(spawn_shared(fut, self.wake.clone()));
    }

    /// 全屏页点了专辑名：立即落占位专辑页 + 派发后台拉取，结果由 `App::tick_playlist_fetch`
    /// 搬进来（宿主循环在这期间照常重绘、照常响应输入）。
    ///
    /// 与作者页同理：全屏页只有专辑显示名，ID 取 `song/detail` 的 `al.id`。
    pub fn open_album_page_from_fullscreen(&mut self) {
        let Some(song_id) = self.current_song_id() else {
            self.set_runtime_status(
                self.lang_text("当前没有正在播放的歌曲", "Nothing is playing right now"),
            );
            return;
        };

        // 标题先显示正在播放的专辑名；解析失败时也只是把错误写进简介。
        let title = {
            let album = self
                .playback
                .now_playing
                .as_ref()
                .map(|track| track.album.trim().to_string())
                .unwrap_or_default();
            if album.is_empty() {
                self.lang_text("专辑页", "Album Page").to_string()
            } else {
                album
            }
        };

        self.browse.playlist.cancel_pagination();
        self.browse.playlist = PlaylistState::placeholder(
            title,
            self.lang_text("正在加载专辑…", "Loading album…")
                .to_string(),
        );
        self.playlist_section_return_snapshot = None;
        // 从全屏页进来：Esc 回首页，而不是回上一次的来源页。
        self.playlist_return_page = Page::Home;
        self.page = Page::Playlist;

        let fut = fetch_album_page_from_song(self.api.clone(), self.config.language, song_id);
        let fut: PlaylistFetchTask = Box::pin(async move { Some(fut.await) });
        self.downloads.page_kind = PlaylistPageKind::Album;
        self.browse.playlist_fetch = Some(PlaylistFetchSlot {
            kind: PlaylistPageKind::Album,
            future: spawn_shared(fut, self.wake.clone()),
        });
    }

    pub fn fullscreen_config_signature(&self) -> u64 {
        self.config_revision
    }

    pub fn fullscreen_config_snapshot(&self) -> Config {
        self.config.clone()
    }

    pub async fn fullscreen_apply_config_sync(&mut self, mut config: Config) {
        config.audio_quality = config.audio_quality.clamp_for_vip(self.vip_audio_unlocked);
        config.download_audio_quality = config
            .download_audio_quality
            .clamp_for_vip(self.vip_audio_unlocked);
        config.page_lyrics_pos_x = config.page_lyrics_pos_x.clamp(0.0, 1.0);
        config.page_lyrics_pos_y = config.page_lyrics_pos_y.clamp(0.0, 1.0);
        if config.download_path != self.config.download_path
            && let Some(raw) = config.download_path.as_deref()
            && crate::app::download::validate_download_path(raw)
                .await
                .is_err()
        {
            config.download_path = self.config.download_path.clone();
            self.set_runtime_status(self.lang_text(
                "下载目录不可写，保留原设置",
                "Download directory is not writable; keeping previous setting",
            ));
        }
        let theme_changed = self.config.theme != config.theme;
        let memory_changed = self.config.playback_memory != config.playback_memory;
        let recommendations_changed = self.config.home_more_recommend != config.home_more_recommend;
        if theme_changed {
            self.theme = ThemeLoader::load_async(&config.theme)
                .await
                .unwrap_or_default();
        }
        self.playback
            .audio_player
            .set_eq(crate::tmplayer::app::state::EqSettings {
                bands_db: config.eq_bands_db,
            })
            .unwrap_or_else(|error| log::error!("apply EQ: {error:#}"));
        self.config = config;
        self.refresh_download_root();
        self.persist_config();
        if memory_changed {
            if self.config.playback_memory {
                self.persist_playback_memory();
            } else {
                self.clear_playback_memory();
            }
        }
        if recommendations_changed
            && self.page != Page::Login
            && let Err(error) = self.load_home_recommendations().await
        {
            self.browse.home.status_line = format!(
                "{}: {error}",
                self.lang_text("推荐歌单刷新失败", "Failed to refresh home recommendations"),
            );
        }
    }

    pub fn build_fullscreen_bootstrap(&self) -> crate::tmplayer::FullscreenBootstrap {
        let mut bootstrap = crate::tmplayer::FullscreenBootstrap::default();
        let Some(now) = self.playback.now_playing.as_ref() else {
            return bootstrap;
        };

        bootstrap.playlist = self
            .playback
            .playback_queue
            .iter()
            .map(|track| crate::tmplayer::FullscreenPlaylistItemSeed {
                id: Some(track.song_id.clone()),
                title: track.title.clone(),
                artist: track.artist.clone(),
                album: track.album.clone(),
                duration: Duration::from_millis(track.duration_ms.max(0) as u64),
            })
            .collect();
        if bootstrap.playlist.is_empty() {
            bootstrap
                .playlist
                .push(crate::tmplayer::FullscreenPlaylistItemSeed {
                    id: Some(now.song_id.clone()),
                    title: now.title.clone(),
                    artist: now.artist.clone(),
                    album: now.album.clone(),
                    duration: Duration::from_millis(now.duration_ms.max(0) as u64),
                });
        }

        let active_idx = self
            .playback
            .playback_index
            .unwrap_or(0)
            .min(bootstrap.playlist.len().saturating_sub(1));
        bootstrap.current_index = Some(active_idx);
        bootstrap.playlist_cover = self
            .playback
            .fullscreen_playlist_cover()
            .map(|cover| cover.to_vec());
        bootstrap.current_track = Some(crate::tmplayer::FullscreenTrackSeed {
            playlist_index: Some(active_idx),
            title: now.title.clone(),
            artist: now.artist.clone(),
            album: now.album.clone(),
            duration: Duration::from_millis(now.duration_ms.max(0) as u64),
            liked: self.playback.now_playing_liked,
            cover: now.cover.clone(),
            lyrics: now.lyrics.clone(),
        });
        bootstrap
    }
}
