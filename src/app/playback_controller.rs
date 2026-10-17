use super::App;
use super::home::HOME_PRIVATE_ROAM_TILE_ID;
use super::likes::LikeMachine;
use super::player::AudioPlayer;
use super::player::AudioPlayerState;
use super::player::is_nonempty_file;
use super::playlist_pagination::PlaylistPagination;
use super::roam::bound_private_roam_queue;
use super::roam::private_roam_continuation_start;
use super::roam::private_roam_restored_index;
use super::roam::private_roam_window;
use super::search_controller::SearchItem;
use super::streaming::StreamingReader;
use super::tracks::PlaylistTrack;
use super::tracks::PlaylistTrackKind;
use super::tracks::format_duration;
use crate::data::persistence::PersistenceKey;
use crate::data::playback_session;
use crate::tmplayer::app::state::LyricLine;
use crate::tmplayer::audio::pcm_tap::PcmRing;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaybackRepeatMode {
    Sequence,
    Shuffle,
    LoopAll,
    LoopOne,
}

impl PlaybackRepeatMode {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Sequence => Self::Shuffle,
            Self::Shuffle => Self::LoopAll,
            Self::LoopAll => Self::LoopOne,
            Self::LoopOne => Self::Sequence,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaybackRuntimeState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone)]
pub(crate) struct PlaybackTrack {
    pub(crate) song_id: String,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: String,
    pub(crate) duration_ms: i64,
    pub(crate) cover_url: Option<String>,
    pub(crate) cover: Option<Vec<u8>>,
    pub(crate) lyrics: Option<Vec<LyricLine>>,
}

impl PlaybackTrack {
    pub(super) fn as_playlist_track(&self) -> PlaylistTrack {
        PlaylistTrack {
            kind: PlaylistTrackKind::Song,
            id: Some(self.song_id.clone()),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            cover_url: self.cover_url.clone(),
            duration_ms: self.duration_ms,
            duration: format_duration(self.duration_ms),
        }
    }

    pub(super) fn from_playlist_track(track: &PlaylistTrack) -> Option<Self> {
        if track.kind != PlaylistTrackKind::Song {
            return None;
        }

        let song_id = track.id.as_ref()?.trim().to_string();
        if song_id.is_empty() {
            return None;
        }

        Some(Self {
            song_id,
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration_ms: track.duration_ms,
            cover_url: track.cover_url.clone(),
            cover: None,
            lyrics: None,
        })
    }

    fn from_search_item(item: &SearchItem) -> Option<Self> {
        let song_id = item.song_id.as_ref()?.trim().to_string();
        if song_id.is_empty() {
            return None;
        }

        Some(Self {
            song_id,
            title: item
                .title
                .clone()
                .unwrap_or_else(|| item.left_label.clone()),
            artist: item
                .artist
                .clone()
                .unwrap_or_else(|| "Unknown Artist".to_string()),
            album: item
                .album
                .clone()
                .unwrap_or_else(|| "Unknown Album".to_string()),
            duration_ms: item.duration_ms.unwrap_or_default(),
            cover_url: item.cover_url.clone(),
            cover: None,
            lyrics: None,
        })
    }
}

fn playback_repeat_mode_key(mode: PlaybackRepeatMode) -> &'static str {
    match mode {
        PlaybackRepeatMode::Sequence => "sequence",
        PlaybackRepeatMode::Shuffle => "shuffle",
        PlaybackRepeatMode::LoopAll => "loop_all",
        PlaybackRepeatMode::LoopOne => "loop_one",
    }
}

fn playback_repeat_mode_from_key(value: &str) -> Option<PlaybackRepeatMode> {
    match value {
        "sequence" => Some(PlaybackRepeatMode::Sequence),
        "shuffle" => Some(PlaybackRepeatMode::Shuffle),
        "loop_all" => Some(PlaybackRepeatMode::LoopAll),
        "loop_one" => Some(PlaybackRepeatMode::LoopOne),
        _ => None,
    }
}

pub(super) fn map_audio_state(state: AudioPlayerState) -> PlaybackRuntimeState {
    match state {
        AudioPlayerState::Playing => PlaybackRuntimeState::Playing,
        AudioPlayerState::Paused => PlaybackRuntimeState::Paused,
        AudioPlayerState::Stopped => PlaybackRuntimeState::Stopped,
    }
}

fn pick_shuffle_index(len: usize, current: usize) -> usize {
    if len <= 1 {
        return 0;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as usize;
    let mut index = now % len;
    if index == current {
        index = (index + 1) % len;
    }
    index
}

impl App {
    pub(crate) fn playback_position(&self) -> Duration {
        self.playback.audio_player.display_position()
    }

    /// 是否正在后台加载跳转目标（进度条据此显示脉冲加载动画）。
    pub(crate) fn is_seeking(&self) -> bool {
        self.playback.audio_player.is_seeking()
    }

    pub(crate) fn playback_duration(&self) -> Duration {
        if let Some(duration) = self.playback.audio_player.duration() {
            return duration;
        }

        if let Some(track) = self.playback.now_playing.as_ref() {
            return Duration::from_millis(track.duration_ms.max(0) as u64);
        }

        Duration::from_secs(0)
    }

    /// Returns (downloaded_bytes, total_bytes) for streaming buffer progress.
    /// Returns None if not streaming or if total is unknown.
    pub(crate) fn buffer_progress(&mut self) -> Option<(u64, u64)> {
        self.playback.audio_player.recv_progress()
    }

    pub(crate) fn now_playing_artist_text(&self) -> String {
        self.playback
            .now_playing
            .as_ref()
            .map(|track| track.artist.clone())
            .unwrap_or_default()
    }

    /// 播放链路上的 PCM 抽头环句柄，经 `HostPlaybackBridge` 交给全屏页示波器。
    pub(crate) fn pcm_ring(&self) -> Arc<PcmRing> {
        self.playback.audio_player.pcm_ring()
    }

    pub(crate) fn seek_to_ratio(&mut self, ratio: f32) {
        if self.playback.now_playing.is_none() {
            return;
        }

        let fallback_total = self
            .playback
            .now_playing
            .as_ref()
            .map(|track| Duration::from_millis(track.duration_ms.max(0) as u64));
        let _ = self
            .playback
            .audio_player
            .seek_to_ratio(ratio, fallback_total);
        self.playback.playback_state = map_audio_state(self.playback.audio_player.state());
    }

    pub(super) async fn toggle_play_pause_hotkey(&mut self) {
        self.playback.page_advance = None;
        if self.playback.now_playing.is_none() {
            self.set_runtime_status(
                self.lang_text("当前没有可控制的播放", "No controllable playback right now"),
            );
            return;
        }

        if self.playback.playback_state == PlaybackRuntimeState::Stopped
            && let Some(index) = self.playback.playback_index
        {
            self.play_queue_index(index, false).await;
            return;
        }

        self.playback.audio_player.toggle_play_pause();
        self.playback.playback_state = map_audio_state(self.playback.audio_player.state());
    }

    pub(super) async fn play_previous_hotkey(&mut self) {
        if self.playback.playback_queue.is_empty() {
            self.set_runtime_status(self.lang_text("当前播放队列为空", "Playback queue is empty"));
            return;
        }

        let current = self
            .playback
            .playback_index
            .unwrap_or(0)
            .min(self.playback.playback_queue.len() - 1);
        let target = match self.playback.playback_repeat_mode {
            PlaybackRepeatMode::Sequence => current.checked_sub(1),
            PlaybackRepeatMode::LoopAll => Some(
                (current + self.playback.playback_queue.len() - 1)
                    % self.playback.playback_queue.len(),
            ),
            PlaybackRepeatMode::LoopOne => Some(current),
            PlaybackRepeatMode::Shuffle => Some(pick_shuffle_index(
                self.playback.playback_queue.len(),
                current,
            )),
        };

        if let Some(index) = target {
            self.play_queue_index(index, true).await;
        }
    }

    pub(super) async fn play_next_hotkey(&mut self) {
        if self.playback.playback_queue.is_empty() {
            self.set_runtime_status(self.lang_text("当前播放队列为空", "Playback queue is empty"));
            return;
        }

        let current = self
            .playback
            .playback_index
            .unwrap_or(0)
            .min(self.playback.playback_queue.len() - 1);
        if self.defer_playlist_advance(current, true) {
            return;
        }
        let target = match self.playback.playback_repeat_mode {
            PlaybackRepeatMode::Sequence => {
                if current + 1 < self.playback.playback_queue.len() {
                    Some(current + 1)
                } else {
                    None
                }
            }
            PlaybackRepeatMode::LoopAll => Some((current + 1) % self.playback.playback_queue.len()),
            PlaybackRepeatMode::LoopOne => Some(current),
            PlaybackRepeatMode::Shuffle => Some(pick_shuffle_index(
                self.playback.playback_queue.len(),
                current,
            )),
        };

        if let Some(index) = target {
            self.play_queue_index(index, true).await;
        }
    }

    pub(super) fn cycle_repeat_mode_hotkey(&mut self) {
        self.playback.cycle_repeat_mode();
        self.set_runtime_status(format!(
            "{}: {}",
            self.lang_text("播放模式", "Play Mode"),
            match self.playback.playback_repeat_mode {
                PlaybackRepeatMode::Sequence => self.lang_text("顺序播放", "Sequence"),
                PlaybackRepeatMode::Shuffle => self.lang_text("随机播放", "Shuffle"),
                PlaybackRepeatMode::LoopAll => self.lang_text("列表循环", "Loop All"),
                PlaybackRepeatMode::LoopOne => self.lang_text("单曲循环", "Loop One"),
            }
        ));
        self.persist_playback_memory();
    }

    /// 当前曲目 id。
    pub(super) fn current_song_id(&self) -> Option<String> {
        self.playback
            .now_playing
            .as_ref()
            .map(|track| track.song_id.clone())
    }

    pub(super) async fn tick_audio(&mut self) {
        self.tick_playlist_page_fetch();
        self.maybe_prefetch_playback_page();
        if let Some(announce) = self.playback.page_advance {
            let current = self.playback.playback_index.unwrap_or(0);
            if current + 1 < self.playback.playback_queue.len() {
                self.playback.page_advance = None;
                self.play_queue_index(current + 1, announce).await;
            } else if !self
                .playback
                .pagination
                .as_ref()
                .is_some_and(PlaylistPagination::is_pending)
            {
                self.playback.page_advance = None;
                self.play_next_after_finish().await;
            }
            return;
        }
        let runtime = map_audio_state(self.playback.audio_player.state());

        if self.playback.playback_state == PlaybackRuntimeState::Playing
            && runtime == PlaybackRuntimeState::Stopped
        {
            self.play_next_after_finish().await;
            return;
        }

        self.playback.playback_state = runtime;
    }

    async fn play_next_after_finish(&mut self) {
        if self.playback.playback_queue.is_empty() {
            self.playback.playback_state = PlaybackRuntimeState::Stopped;
            return;
        }

        let current = self
            .playback
            .playback_index
            .unwrap_or(0)
            .min(self.playback.playback_queue.len() - 1);
        if self.defer_playlist_advance(current, false) {
            return;
        }

        // 私人漫游：队列（快照）播完后，从权威列表中当前歌曲之后继续顺序播放。
        // 队列与历史窗口可以同为 256 首但起点不同，不能用长度比较判断是否还有后续。
        if self.playback.playback_repeat_mode == PlaybackRepeatMode::Sequence
            && self.playback_queue_is_roam()
            && current + 1 >= self.playback.playback_queue.len()
            && let Some(start) = private_roam_continuation_start(
                &self.browse.private_roam.tracks,
                &self.playback.playback_queue[current],
                self.browse.private_roam.last_played_index,
            )
        {
            let queue: Vec<PlaybackTrack> = self.browse.private_roam.tracks[start..]
                .iter()
                .filter_map(PlaybackTrack::from_playlist_track)
                .collect();
            // 来源仍是漫游本身，封面沿用漫游当前封面（跟随播放歌曲）。
            let source_cover = self.browse.private_roam.cover_url.clone();
            self.replace_queue_and_play(
                queue,
                0,
                source_cover,
                Some(HOME_PRIVATE_ROAM_TILE_ID.to_string()),
                None,
            )
            .await;
            return;
        }

        let target = match self.playback.playback_repeat_mode {
            PlaybackRepeatMode::Sequence => {
                if current + 1 < self.playback.playback_queue.len() {
                    Some(current + 1)
                } else {
                    None
                }
            }
            PlaybackRepeatMode::LoopAll => Some((current + 1) % self.playback.playback_queue.len()),
            PlaybackRepeatMode::LoopOne => Some(current),
            PlaybackRepeatMode::Shuffle => Some(pick_shuffle_index(
                self.playback.playback_queue.len(),
                current,
            )),
        };

        if let Some(index) = target {
            self.play_queue_index(index, false).await;
        } else {
            self.playback.playback_state = PlaybackRuntimeState::Stopped;
            self.set_runtime_status(self.lang_text("播放结束", "Playback finished"));
        }
    }

    fn defer_playlist_advance(&mut self, current: usize, announce: bool) -> bool {
        if matches!(
            self.playback.playback_repeat_mode,
            PlaybackRepeatMode::Sequence | PlaybackRepeatMode::LoopAll
        ) && current + 1 >= self.playback.playback_queue.len()
            && let Some(pagination) = &self.playback.pagination
            && pagination.has_more()
        {
            pagination.request(self.api.clone());
            self.playback.page_advance = Some(announce);
            return true;
        }
        false
    }

    pub(crate) async fn play_queue_index(&mut self, index: usize, announce: bool) {
        let Some(track) = self.playback.playback_queue.get(index).cloned() else {
            return;
        };
        self.playback.page_advance = None;

        // 记录私人漫游播放位置/封面；播放到列表末尾时追加新歌
        // Appending roam songs may evict a played prefix, including before this target.
        self.playback.playback_index = Some(index);
        self.track_private_roam_playback(&track).await;
        let index = self.playback.playback_index.unwrap_or(index);

        let mut enriched = track.clone();
        // Switch UI state immediately and avoid blocking network fetches here.
        self.enrich_track_metadata(&mut enriched, false).await;
        if let Some(slot) = self.playback.playback_queue.get_mut(index) {
            slot.cover = enriched.cover.clone();
        }
        self.trim_non_current_cover_memory(index);
        self.playback.now_playing = Some(enriched.clone());
        self.refresh_now_playing_like_state();
        self.playback.playback_index = Some(index);
        self.maybe_prefetch_playback_page();
        self.cover_fetch_inflight_url = None;
        self.cover_fetch_last_attempt_at = None;
        self.maybe_schedule_now_playing_cover_fetch();
        self.lyric_fetch_inflight_song_id = None;
        self.lyric_fetch_last_attempt_at = None;
        self.maybe_schedule_now_playing_lyric_fetch();
        self.persist_playback_memory();

        let quality = self.config.audio_quality.as_api_level();
        let fail = |err, app: &mut Self| {
            app.playback.now_playing_liked = false;
            app.playback.playback_state = PlaybackRuntimeState::Stopped;
            app.set_runtime_status(format!(
                "{}: {err}",
                app.lang_text("播放失败", "Playback failed"),
            ));
        };
        let ok = |app: &mut Self| {
            app.playback.playback_state = PlaybackRuntimeState::Playing;
            if announce {
                app.set_runtime_status(format!(
                    "{}: {} - {}",
                    app.lang_text("正在播放", "Now Playing"),
                    enriched.title,
                    enriched.artist
                ));
            };
        };

        let id = &track.song_id;
        let path = self.playback.audio_player.cached_song_path(id, quality);

        if is_nonempty_file(&path).await {
            // Startup cache cleanup runs in the background. The existence check and
            // decoder open are intentionally not one atomic operation; if cleanup
            // wins that race, treat the cache as stale and use the normal stream path.
            if let Ok(()) = self.playback.audio_player.play_from_file(&path).await {
                return ok(self);
            }
        }

        // Song not cached - start streaming playback while prefetching in background.
        self.playback.audio_player.stop();
        self.set_runtime_status(format!(
            "{}: {} - {}",
            self.lang_text("正在缓冲", "Buffering"),
            enriched.title,
            enriched.artist
        ));

        match self.api.song_stream_url_with_quality(id, quality).await {
            Ok(url) => {
                let (progress_tx, progress_rx) = see::sync::channel((0, 0));
                match StreamingReader::new(
                    self.api.http_client(),
                    &url,
                    path.clone(),
                    self.api.session_cookie(),
                    progress_tx,
                )
                .await
                {
                    Ok(reader) => match self
                        .playback
                        .audio_player
                        .play_streaming(reader, progress_rx)
                        .await
                    {
                        Ok(()) => ok(self),
                        Err(err) => fail(err, self),
                    },
                    Err(err) => fail(err, self),
                }
            }
            Err(err) => fail(err, self),
        }
    }

    async fn replace_queue_and_play(
        &mut self,
        queue: Vec<PlaybackTrack>,
        index: usize,
        source_cover_url: Option<String>,
        source_id: Option<String>,
        pagination: Option<PlaylistPagination>,
    ) {
        if queue.is_empty() {
            self.set_runtime_status(
                self.lang_text("当前页面没有可播放歌曲", "No playable songs on this page"),
            );
            return;
        }

        let mut queue = queue;
        let mut target = Some(index.min(queue.len() - 1));
        bound_private_roam_queue(
            &mut queue,
            &mut target,
            source_id.as_deref(),
            &mut self.playback.page_advance,
        );
        self.playback
            .replace_queue(queue, None, source_cover_url, source_id, pagination);
        let target = target.unwrap_or(0);
        self.play_queue_index(target, true).await;
    }

    fn build_queue_from_playlist(&self) -> (Vec<PlaybackTrack>, usize) {
        let focused = self.browse.playlist.focused_idx;
        let mut queue = Vec::new();
        let mut mapped_focus = None;

        for (idx, track) in self.browse.playlist.tracks.iter().enumerate() {
            if let Some(item) = PlaybackTrack::from_playlist_track(track) {
                if idx == focused {
                    mapped_focus = Some(queue.len());
                }
                queue.push(item);
            }
        }

        let target = mapped_focus.unwrap_or(0);
        (queue, target)
    }

    fn build_queue_from_search(&self) -> (Vec<PlaybackTrack>, usize) {
        let focused = self.search.focused_idx;
        let mut queue = Vec::new();
        let mut mapped_focus = None;

        for (idx, item) in self.search.results().iter().enumerate() {
            if let Some(track) = PlaybackTrack::from_search_item(item) {
                if idx == focused {
                    mapped_focus = Some(queue.len());
                }
                queue.push(track);
            }
        }

        let target = mapped_focus.unwrap_or(0);
        (queue, target)
    }

    pub(super) async fn play_focused_playlist_track(&mut self) {
        let Some(track) = self
            .browse
            .playlist
            .tracks
            .get(self.browse.playlist.focused_idx)
        else {
            return;
        };

        match track.kind {
            PlaylistTrackKind::Song => {
                let (queue, target) = self.build_queue_from_playlist();
                let source_cover = self.browse.playlist.cover.url.clone();
                self.replace_queue_and_play(
                    queue,
                    target,
                    source_cover,
                    self.browse.playlist.id.clone(),
                    self.browse.playlist.pagination.clone(),
                )
                .await;
            }
            PlaylistTrackKind::Album | PlaylistTrackKind::Ep | PlaylistTrackKind::Single => {
                self.open_focused_playlist_album();
            }
        }
    }

    pub(super) async fn play_focused_search_track(&mut self) {
        let (queue, target) = self.build_queue_from_search();
        // 搜索结果没有"所属列表"，交给首歌封面兜底。
        self.replace_queue_and_play(queue, target, None, None, None)
            .await;
    }

    pub(crate) fn is_now_playing_song(&self, song_id: Option<&str>) -> bool {
        match (self.playback.now_playing.as_ref(), song_id) {
            (Some(now), Some(song_id)) => now.song_id == song_id,
            _ => false,
        }
    }

    pub(crate) fn persist_playback_memory_on_exit(&self) {
        self.persist_playback_memory();
    }

    pub(super) fn clear_playback_memory(&self) {
        let _ = self
            .persistence
            .enqueue_latest(PersistenceKey::Playback, playback_session::clear);
    }

    pub(super) fn persist_playback_memory(&self) {
        if self.playback.restoring_memory
            || !self.config.playback_memory
            || self.playback.playback_queue.is_empty()
        {
            return;
        }

        let (window, current_index) = if self.playback_queue_is_roam() {
            private_roam_window(
                self.playback.playback_queue.len(),
                self.playback.playback_index,
            )
        } else {
            (
                0..self.playback.playback_queue.len(),
                self.playback.playback_index,
            )
        };
        let queue = self.playback.playback_queue[window]
            .iter()
            .map(|track| playback_session::PlaybackSessionTrack {
                song_id: track.song_id.clone(),
                title: track.title.clone(),
                artist: track.artist.clone(),
                album: track.album.clone(),
                duration_ms: track.duration_ms,
                cover_url: track.cover_url.clone(),
            })
            .collect::<Vec<_>>();

        let record = playback_session::PlaybackSessionRecord {
            queue,
            current_index,
            repeat_mode: Some(
                playback_repeat_mode_key(self.playback.playback_repeat_mode).to_string(),
            ),
            source_playlist_id: self.playback.playback_queue_source_id.clone(),
            source_cover_url: self.playback.playback_queue_cover_url.clone(),
            source_cursor: self
                .playback
                .pagination
                .as_ref()
                .map(PlaylistPagination::cursor),
            updated_at: 0,
        };

        let _ = self
            .persistence
            .enqueue_latest(PersistenceKey::Playback, move || {
                playback_session::save(&record)
            });
    }

    pub(super) async fn try_restore_playback_memory(&mut self) {
        if !self.config.playback_memory {
            return;
        }

        let result = compio::runtime::spawn_blocking(playback_session::load).await;
        let record = match result {
            Ok(Ok(Some(record))) => record,
            Ok(Ok(None)) => return,
            other => {
                log::error!("playback memory could not be read: {other:?}");
                self.set_runtime_status(self.lang_text(
                    "播放记忆读取失败，保留原文件",
                    "Playback memory could not be read; original preserved",
                ));
                return;
            }
        };

        let is_roam = record.source_playlist_id.as_deref() == Some(HOME_PRIVATE_ROAM_TILE_ID);
        let window = if is_roam {
            private_roam_window(record.queue.len(), record.current_index).0
        } else {
            0..record.queue.len()
        };
        let restored_index = if is_roam {
            private_roam_restored_index(
                &record.queue,
                record.current_index.or(Some(0)),
                &window,
                |track| !track.song_id.trim().is_empty(),
            )
        } else {
            None
        };
        let queue = record
            .queue
            .into_iter()
            .skip(window.start)
            .take(window.len())
            .filter_map(|track| {
                let song_id = track.song_id.trim().to_string();
                if song_id.is_empty() {
                    return None;
                }
                Some(PlaybackTrack {
                    song_id,
                    title: track.title,
                    artist: track.artist,
                    album: track.album,
                    duration_ms: track.duration_ms,
                    cover_url: track.cover_url,
                    cover: None,
                    lyrics: None,
                })
            })
            .collect::<Vec<_>>();
        let mut queue = queue;
        let mut target = if is_roam {
            restored_index.or_else(|| (!queue.is_empty()).then_some(0))
        } else {
            Some(
                record
                    .current_index
                    .unwrap_or(0)
                    .min(queue.len().saturating_sub(1)),
            )
        };
        bound_private_roam_queue(
            &mut queue,
            &mut target,
            record.source_playlist_id.as_deref(),
            &mut self.playback.page_advance,
        );

        if queue.is_empty() {
            return;
        }

        if let Some(mode) = record
            .repeat_mode
            .as_deref()
            .and_then(playback_repeat_mode_from_key)
        {
            self.playback.playback_repeat_mode = mode;
        }

        let pagination = record
            .source_cursor
            .filter(|_| !is_roam)
            .filter(|cursor| {
                Some(cursor.source_id.as_str()) == record.source_playlist_id.as_deref()
            })
            .map(PlaylistPagination::new);
        self.playback.restoring_memory = true;
        self.playback.replace_queue(
            queue,
            None,
            record.source_cover_url,
            record.source_playlist_id,
            pagination,
        );
        let target = target.unwrap_or(0);
        self.play_queue_index(target, false).await;
        self.playback.restoring_memory = false;
        if is_roam {
            self.persist_playback_memory();
        }
        self.set_runtime_status(self.lang_text("已恢复播放记忆", "Playback memory restored"));
    }
}

pub(crate) struct PlaybackController {
    pub(super) audio_player: AudioPlayer,
    pub(crate) now_playing: Option<PlaybackTrack>,
    pub(crate) now_playing_liked: bool,
    pub(super) like_machine: LikeMachine,
    pub(super) playback_queue: Vec<PlaybackTrack>,
    pub(super) playback_queue_cover_url: Option<String>,
    pub(super) playback_queue_cover: Option<Vec<u8>>,
    pub(super) playback_queue_source_id: Option<String>,
    pub(super) playback_index: Option<usize>,
    pub(crate) playback_repeat_mode: PlaybackRepeatMode,
    pub(crate) playback_state: PlaybackRuntimeState,
    restoring_memory: bool,
    pub(super) pagination: Option<PlaylistPagination>,
    /// End-of-page advance waits for the shared page instead of stopping or wrapping early.
    pub(super) page_advance: Option<bool>,
}

impl PlaybackController {
    pub(super) fn new(audio_player: AudioPlayer) -> Self {
        Self {
            audio_player,
            now_playing: None,
            now_playing_liked: false,
            like_machine: LikeMachine::default(),
            playback_queue: Vec::new(),
            playback_queue_cover_url: None,
            playback_queue_cover: None,
            playback_queue_source_id: None,
            playback_index: None,
            playback_repeat_mode: PlaybackRepeatMode::Sequence,
            playback_state: PlaybackRuntimeState::Stopped,
            restoring_memory: false,
            pagination: None,
            page_advance: None,
        }
    }

    /// 全屏入口与后续快照共用：来源封面未就绪时使用歌曲封面，不保留旧队列的图。
    pub(super) fn fullscreen_playlist_cover(&self) -> Option<&[u8]> {
        self.playback_queue_cover
            .as_deref()
            .or_else(|| {
                self.playback_queue
                    .first()
                    .and_then(|track| track.cover.as_deref())
            })
            .or_else(|| {
                self.now_playing
                    .as_ref()
                    .and_then(|track| track.cover.as_deref())
            })
    }

    fn cycle_repeat_mode(&mut self) {
        self.playback_repeat_mode = self.playback_repeat_mode.next();
    }

    pub(super) fn clear_like_state(&mut self) {
        self.now_playing_liked = false;
        self.like_machine.clear();
    }

    pub(super) fn clear_queue(&mut self) {
        self.playback_queue.clear();
        self.playback_index = None;
        self.playback_state = PlaybackRuntimeState::Stopped;
        self.playback_repeat_mode = PlaybackRepeatMode::Sequence;
        self.playback_queue_cover_url = None;
        self.playback_queue_cover = None;
        self.playback_queue_source_id = None;
        self.restoring_memory = false;
        self.pagination = None;
        self.page_advance = None;
    }

    fn replace_queue(
        &mut self,
        queue: Vec<PlaybackTrack>,
        index: Option<usize>,
        cover_url: Option<String>,
        source_id: Option<String>,
        pagination: Option<PlaylistPagination>,
    ) {
        self.playback_queue = queue;
        self.playback_index = index;
        self.playback_queue_cover_url = cover_url;
        self.playback_queue_cover = None;
        self.playback_queue_source_id = source_id;
        self.pagination = pagination;
        self.page_advance = None;
    }
}
