pub mod app;
pub mod audio;
pub mod data;
pub mod playback;
pub mod render;
pub mod ui;
pub mod utils;

use crate::app::player::{cleanup_cache_dir, resolve_cache_root};
use anyhow::Result;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

use crate::data::config::{
    AudioQuality as HostAudioQuality, BarChannels as HostBarChannels, BarNumber as HostBarNumber,
    Config as HostConfig, GraphicsProtocol, Language as HostLanguage,
    VisualizeMode as HostVisualizeMode,
};

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
pub enum FullscreenExit {
    BackToHost,
    BackToHostOpenSettings,
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
}

#[derive(Debug, Clone)]
pub struct HostConfigSync {
    pub theme: String,
    pub transparent_background: bool,
    pub album_border: bool,
    pub language: HostLanguage,
    pub graphics_protocol: GraphicsProtocol,
    pub page_lyrics: bool,
    pub audio_quality: HostAudioQuality,
    pub eq_bands_db: [f32; crate::tmplayer::app::state::EQ_BANDS],
    pub playback_memory: bool,
    pub vip_audio_unlocked: bool,
    pub show_hints: bool,
    pub home_more_recommend: bool,
    pub visualize: HostVisualizeMode,
    pub super_smooth_bar: bool,
    pub bars_gap: bool,
    pub bar_number: HostBarNumber,
    pub bar_channels: HostBarChannels,
    pub bar_channel_reverse: bool,
}

pub trait HostPlaybackBridge {
    async fn tick(&mut self);
    fn metadata_signature(&self) -> u64;
    fn runtime_snapshot(&self) -> HostPlaybackRuntimeSnapshot;
    fn snapshot(&mut self) -> HostPlaybackSnapshot;
    fn config_snapshot(&self) -> HostConfigSync;
    async fn apply_config_sync(&mut self, config: HostConfigSync);
    async fn toggle_play_pause(&mut self);
    async fn play_previous(&mut self);
    async fn play_next(&mut self);
    async fn play_queue_index(&mut self, index: usize);
    fn seek_to_ratio(&mut self, ratio: f32);
    fn set_volume(&mut self, volume: f32);
    fn toggle_repeat_mode(&mut self);
    async fn toggle_like_current(&mut self);
}

pub async fn run_fullscreen(
    host_config: &HostConfig,
    bootstrap: FullscreenBootstrap,
    host_bridge: Option<&mut impl HostPlaybackBridge>,
) -> Result<FullscreenExit> {
    let config = tm_config_from_host(host_config);
    let theme = data::theme_loader::ThemeLoader::load(&host_config.theme)?;

    let mut app = app::state::AppState::new(config, theme, host_config.language);
    let ncm_cover_cache_dir = resolve_cache_root(host_config).join("tmplayer_ncm_cover");
    if host_config.cache.clean_on_startup {
        let _ = cleanup_cache_dir(&ncm_cover_cache_dir, &host_config.cache);
    }
    let _ = std::fs::create_dir_all(&ncm_cover_cache_dir);
    app.ncm_cover_cache_dir = Some(ncm_cover_cache_dir);
    app.eq.bands_db = app.config.eq_bands_db;

    apply_bootstrap(&mut app, bootstrap);

    app::event_loop::run(&mut app, host_bridge).await
}

fn tm_config_from_host(host: &HostConfig) -> data::config::Config {
    data::config::Config {
        theme: host.theme.clone(),
        ui_fps: host.ui_fps,
        spectrum_hz: host.spectrum_hz,
        mpris_poll_ms: host.mpris_poll_ms,
        visualize: match host.visualize {
            HostVisualizeMode::Off => data::config::VisualizeMode::Off,
            HostVisualizeMode::Bars => data::config::VisualizeMode::Bars,
            HostVisualizeMode::Oscilloscope => data::config::VisualizeMode::Oscilloscope,
        },
        eq_bands_db: host.eq_bands_db,
        transparent_background: host.transparent_background,
        page_lyrics: host.page_lyrics,
        album_border: host.album_border,
        graphics_protocol: host.graphics_protocol,
        kitty_cover_scale_percent: host.kitty_cover_scale_percent,
        super_smooth_bar: host.super_smooth_bar,
        bars_gap: host.bars_gap,
        audio_quality: match host.audio_quality {
            HostAudioQuality::Standard => data::config::AudioQuality::Standard,
            HostAudioQuality::Higher => data::config::AudioQuality::Higher,
            HostAudioQuality::Exhigh => data::config::AudioQuality::Exhigh,
            HostAudioQuality::Lossless => data::config::AudioQuality::Lossless,
            HostAudioQuality::Hires => data::config::AudioQuality::Hires,
            HostAudioQuality::Jyeffect => data::config::AudioQuality::Jyeffect,
            HostAudioQuality::Sky => data::config::AudioQuality::Sky,
            HostAudioQuality::Dolby => data::config::AudioQuality::Dolby,
            HostAudioQuality::Jymaster => data::config::AudioQuality::Jymaster,
        },
        playback_memory: host.playback_memory,
        show_hints: host.show_hints,
        home_more_recommend: host.home_more_recommend,
        bar_number: match host.bar_number {
            HostBarNumber::Auto => data::config::BarNumber::Auto,
            HostBarNumber::N16 => data::config::BarNumber::N16,
            HostBarNumber::N32 => data::config::BarNumber::N32,
            HostBarNumber::N48 => data::config::BarNumber::N48,
            HostBarNumber::N64 => data::config::BarNumber::N64,
            HostBarNumber::N80 => data::config::BarNumber::N80,
            HostBarNumber::N96 => data::config::BarNumber::N96,
        },
        bar_channels: match host.bar_channels {
            HostBarChannels::Stereo => data::config::BarChannels::Stereo,
            HostBarChannels::Mono => data::config::BarChannels::Mono,
        },
        bar_channel_reverse: host.bar_channel_reverse,
        // Fullscreen page data comes from CNMPlayer API flow; disable TMPlayer local fetch pipeline.
        lyrics_cover_fetch: false,
        lyrics_cover_download: false,
        audio_fingerprint: false,
        acoustid_api_key: String::new(),
        resume_last_position: false,
        keybind_search_box: host.keybind_search_box.clone(),
        keybind_fullscreen: host.keybind_fullscreen.clone(),
        keybind_settings: host.keybind_settings.clone(),
        keybind_sidebar: host.keybind_sidebar.clone(),
        keybind_quit: host.keybind_quit.clone(),
        keybind_page_up: host.keybind_page_up.clone(),
        keybind_page_down: host.keybind_page_down.clone(),
        keybind_prev: host.keybind_prev.clone(),
        keybind_next: host.keybind_next.clone(),
        keybind_toggle_play_pause: host.keybind_toggle_play_pause.clone(),
        keybind_toggle_mode: host.keybind_toggle_mode.clone(),
        keybind_fullscreen_prev: host.keybind_fullscreen_prev.clone(),
        keybind_fullscreen_next: host.keybind_fullscreen_next.clone(),
        keybind_fullscreen_toggle_play_pause: host.keybind_fullscreen_toggle_play_pause.clone(),
        keybind_fullscreen_toggle_mode: host.keybind_fullscreen_toggle_mode.clone(),
        keybind_fullscreen_eq: host.keybind_fullscreen_eq.clone(),
        keybind_fullscreen_eq_reset: host.keybind_fullscreen_eq_reset.clone(),
        keybind_toggle_like_fullscreen: host.keybind_toggle_like_fullscreen.clone(),
    }
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
                path: PathBuf::from("ncm://seed/current"),
                title,
            });
            tracks.push(track_from_seed(current));
        }
    } else {
        for (idx, item) in bootstrap.playlist.iter().enumerate() {
            let id = item.id.clone().unwrap_or_else(|| format!("seed-{idx}"));
            let title = if item.title.trim().is_empty() {
                format!("Track {}", idx + 1)
            } else {
                item.title.clone()
            };
            playlist.items.push(data::playlist::PlaylistItem {
                path: PathBuf::from(format!("ncm://{id}")),
                title,
            });
            tracks.push(app::state::TrackMetadata {
                title: item.title.clone(),
                artist: item.artist.clone(),
                album: item.album.clone(),
                duration: item.duration,
                cover: None,
                cover_hash: None,
                cover_folder: None,
                lyrics: None,
            });
        }
    }

    if tracks.is_empty() {
        app.api_tracks.clear();
        app.playlist = data::playlist::Playlist::default();
        app.playlist_view = data::playlist::Playlist::default();
        app.player.mode = app::state::PlayMode::Idle;
        app.player.playback = app::state::PlaybackState::Stopped;
        app.player.position = Duration::from_secs(0);
        app.player.track = app::state::TrackMetadata {
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            duration: Duration::from_secs(0),
            cover: None,
            cover_hash: None,
            cover_folder: None,
            lyrics: None,
        };
        app.local_view_album_cover = None;
        app.local_view_album_cover_hash = None;
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

    app.player.mode = app::state::PlayMode::Idle;
    app.player.playback = app::state::PlaybackState::Playing;
    app.player.liked = current_liked;
    app.player.position = Duration::from_secs(0);
    app.player.track = app.api_tracks[active_idx].clone();

    app.local_view_album_cover = bootstrap.playlist_cover;
    app.local_view_album_cover_hash = app
        .local_view_album_cover
        .as_deref()
        .map(hash_bytes)
        .map(Some)
        .unwrap_or(None);
    app.local_folder_kind = if app.local_view_album_cover.is_some() {
        app::state::LocalFolderKind::Album
    } else {
        app::state::LocalFolderKind::Plain
    };
    app.local_view_album_folder = None;
    app.local_folder = None;
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
        cover_folder: None,
        lyrics: seed.lyrics.clone(),
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}
