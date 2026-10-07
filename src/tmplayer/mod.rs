pub mod app;
pub mod audio;
pub mod data;
pub mod playback;
pub mod render;
pub mod ui;
pub mod utils;

use anyhow::Result;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use crate::data::config::Config;
use ratatui::buffer::Buffer;
#[derive(Debug, Clone)]
pub struct FullscreenPlaylistItemSeed {
    pub id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub struct FullscreenTrackSeed {
    pub playlist_index: Option<usize>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Duration,
    pub liked: bool,
    pub cover: Option<Vec<u8>>,
    pub lyrics: Option<Vec<app::state::LyricLine>>,
}

#[derive(Debug, Clone, Default)]
pub struct FullscreenBootstrap {
    pub playlist: Vec<FullscreenPlaylistItemSeed>,
    pub current_index: Option<usize>,
    pub current_track: Option<FullscreenTrackSeed>,
    pub playlist_cover: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum FullscreenExit {
    BackToHost,
    BackToHostOpenSettings,
    /// 全屏页里点了作者名：宿主退出后打开该作者页（附带显示串里的段序号）。
    BackToHostOpenAuthor(usize),
    /// 全屏页里点了专辑名：宿主退出后打开该专辑页。
    BackToHostOpenAlbum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HostPlaybackState {
    Playing,
    Paused,
    #[default]
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HostRepeatMode {
    #[default]
    Sequence,
    Shuffle,
    LoopAll,
    LoopOne,
}

#[derive(Debug, Clone, Default)]
pub struct HostPlaybackSnapshot {
    pub playlist: Vec<FullscreenPlaylistItemSeed>,
    pub current_index: Option<usize>,
    pub playlist_cover: Option<Vec<u8>>,
    pub current_track: Option<FullscreenTrackSeed>,
    pub current_liked: bool,
    pub state: HostPlaybackState,
    pub repeat_mode: HostRepeatMode,
    pub position: Duration,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HostPlaybackRuntimeSnapshot {
    pub current_index: Option<usize>,
    pub current_liked: bool,
    pub state: HostPlaybackState,
    pub repeat_mode: HostRepeatMode,
    pub position: Duration,
    pub volume: f32,
    pub seeking: bool,
    /// 信息区下载图标状态（宿主每帧同步；`Hidden` = 不显示）。
    pub download: DownloadIconState,
}

/// 信息区下载图标的状态。
///
/// `Hidden`：下载不可用（宿主没有可写目录）或没有播放中的歌曲——图标整格不画、不可点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DownloadIconState {
    #[default]
    Hidden,
    NotDownloaded,
    Downloading,
    Done,
}

pub trait HostPlaybackBridge {
    async fn tick(&mut self);
    fn wake_signal(&self) -> crate::render::wake::WakeSignal;
    fn metadata_signature(&self) -> u64;
    fn runtime_snapshot(&self) -> HostPlaybackRuntimeSnapshot;
    /// 宿主播放链路上的 PCM 抽头环，示波器由此取真实波形。
    fn pcm_ring(&self) -> Arc<crate::tmplayer::audio::pcm_tap::PcmRing>;
    fn snapshot(&mut self) -> HostPlaybackSnapshot;
    /// Monotonic host configuration revision; fullscreen only snapshots when it changes.
    fn config_signature(&self) -> u64;
    fn config_snapshot(&self) -> Config;
    /// Submit settings to the host; accepted values are read back with `config_snapshot`.
    async fn apply_config_sync(&mut self, config: Config);
    /// Runtime entitlement is intentionally not persisted in fullscreen's Config mirror.
    fn vip_audio_unlocked(&self) -> bool;
    async fn toggle_play_pause(&mut self);
    async fn play_previous(&mut self);
    async fn play_next(&mut self);
    async fn play_queue_index(&mut self, index: usize);
    fn request_queue_page(&mut self);
    fn seek_to_ratio(&mut self, ratio: f32);
    fn set_volume(&mut self, volume: f32);
    fn toggle_repeat_mode(&mut self);
    async fn toggle_like_current(&mut self);
    /// 全屏页发起/取消「下载当前播放歌曲」。
    fn download_current(&mut self);
}

pub async fn run_fullscreen(
    host_config: &Config,
    bootstrap: FullscreenBootstrap,
    host_snapshot: Buffer,
    host_bridge: &mut impl HostPlaybackBridge,
) -> Result<FullscreenExit> {
    let config = host_config.clone();
    let theme = crate::data::theme_loader::ThemeLoader::load_async(&host_config.theme)
        .await
        .unwrap_or_default();

    let mut app = app::state::AppState::new(config, theme, host_config.language);
    app.eq.bands_db = app.config.eq_bands_db;
    app.refresh_download_root();

    apply_bootstrap(&mut app, bootstrap);

    app::event_loop::run(&mut app, host_snapshot, host_bridge).await
}

fn apply_bootstrap(app: &mut app::state::AppState, bootstrap: FullscreenBootstrap) {
    let mut playlist = data::playlist::Playlist::default();
    let mut tracks: Vec<app::state::TrackMetadata> = Vec::new();

    if bootstrap.playlist.is_empty() {
        if let Some(current) = bootstrap.current_track.as_ref() {
            let title = if current.title.trim().is_empty() {
                "Unknown".to_string()
            } else {
                current.title.clone()
            };
            playlist.items.push(data::playlist::PlaylistItem {
                song_id: None,
                title,
            });
            tracks.push(track_from_seed(current));
        }
    } else {
        for (idx, item) in bootstrap.playlist.iter().enumerate() {
            let title = if item.title.trim().is_empty() {
                format!("Track {}", idx + 1)
            } else {
                item.title.clone()
            };
            playlist.items.push(data::playlist::PlaylistItem {
                song_id: item.id.clone(),
                title,
            });
            tracks.push(app::state::TrackMetadata {
                title: item.title.clone(),
                artist: item.artist.clone(),
                album: item.album.clone(),
                duration: item.duration,
                cover: None,
                cover_hash: None,
                lyrics: None,
            });
        }
    }

    if tracks.is_empty() {
        app.api_tracks.clear();
        app.playlist = data::playlist::Playlist::default();
        app.playlist_view = data::playlist::Playlist::default();
        app.player.playback = app::state::PlaybackState::Stopped;
        app.player.position = Duration::from_secs(0);
        app.player.track = app::state::TrackMetadata {
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            duration: Duration::from_secs(0),
            cover: None,
            cover_hash: None,
            lyrics: None,
        };
        return;
    }

    let mut active_idx = bootstrap
        .current_index
        .unwrap_or(0)
        .min(tracks.len().saturating_sub(1));
    let mut current_liked = false;

    if let Some(current) = bootstrap.current_track.as_ref() {
        let target_idx = current
            .playlist_index
            .unwrap_or(active_idx)
            .min(tracks.len().saturating_sub(1));
        tracks[target_idx] = track_from_seed(current);
        active_idx = target_idx;
        current_liked = current.liked;
    }

    playlist.selected = active_idx;
    playlist.current = Some(active_idx);
    playlist.clamp_selected();

    app.api_tracks = tracks;
    app.playlist = playlist.clone();
    app.playlist_view = playlist;

    app.player.playback = app::state::PlaybackState::Playing;
    app.player.liked = current_liked;
    app.player.position = Duration::from_secs(0);
    app.player.track = app.api_tracks[active_idx].clone();

    app.playlist_cover = bootstrap.playlist_cover;
    app.playlist_cover_hash = app.playlist_cover.as_deref().map(hash_bytes);
}

fn track_from_seed(seed: &FullscreenTrackSeed) -> app::state::TrackMetadata {
    app::state::TrackMetadata {
        title: seed.title.clone(),
        artist: seed.artist.clone(),
        album: seed.album.clone(),
        duration: seed.duration,
        cover_hash: seed
            .cover
            .as_deref()
            .map(hash_bytes)
            .map(Some)
            .unwrap_or(None),
        cover: seed.cover.clone(),
        lyrics: seed.lyrics.clone(),
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}
