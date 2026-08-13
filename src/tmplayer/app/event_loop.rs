use crate::tmplayer::app::state::{
    AppState, CoverSnapshot, LocalFolderKind, Overlay, PlayMode, PlaybackState, RepeatMode,
};
use crate::tmplayer::audio::cava::{CavaChannels, CavaConfig, CavaRunner};
use crate::tmplayer::data::config::{AudioQuality, BarChannels, BarNumber, VisualizeMode};
use crate::tmplayer::data::theme_loader::ThemeLoader;
use crate::tmplayer::ui::theme::ThemeName;
use crate::tmplayer::ui::tui::{Tui, UiLayout};
use crate::tmplayer::utils::input::{Action, map_key, map_mouse};
use crate::tmplayer::{
    HostConfigSync, HostPlaybackBridge, HostPlaybackRuntimeSnapshot, HostPlaybackSnapshot,
    HostPlaybackState, HostRepeatMode,
};
use anyhow::Result;
use crossterm::event::{self, Event};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use std::time::{Duration, Instant};

const HELP_MODAL_ITEMS: usize = 14;

fn sync_playlists_when_viewing_playback(app: &mut AppState) {
    if app.local_view_album_folder.is_some() && app.local_folder.is_some() {
        if app.local_view_album_folder.as_ref() == app.local_folder.as_ref() {
            app.playlist = app.playlist_view.clone();
        }
    }
}

fn clear_spectrum(app: &mut AppState) {
    let bar_len = app.spectrum.bars.len().max(1);
    app.spectrum.bars = vec![0.0; bar_len];
    app.spectrum.bars_left = vec![0.0; bar_len];
    app.spectrum.bars_right = vec![0.0; bar_len];
    app.spectrum.stereo_left = [0.0; 64];
    app.spectrum.stereo_right = [0.0; 64];
}

fn has_spectrum_data(app: &AppState) -> bool {
    app.spectrum.bars.iter().any(|&v| v > 0.0)
        || app.spectrum.bars_left.iter().any(|&v| v > 0.0)
        || app.spectrum.bars_right.iter().any(|&v| v > 0.0)
        || app.spectrum.stereo_left.iter().any(|&v| v > 0.0)
        || app.spectrum.stereo_right.iter().any(|&v| v > 0.0)
}

fn map_host_state(state: HostPlaybackState) -> PlaybackState {
    match state {
        HostPlaybackState::Playing => PlaybackState::Playing,
        HostPlaybackState::Paused => PlaybackState::Paused,
        HostPlaybackState::Stopped => PlaybackState::Stopped,
    }
}

fn map_host_repeat(mode: HostRepeatMode) -> RepeatMode {
    match mode {
        HostRepeatMode::Sequence => RepeatMode::Sequence,
        HostRepeatMode::Shuffle => RepeatMode::Shuffle,
        HostRepeatMode::LoopAll => RepeatMode::LoopAll,
        HostRepeatMode::LoopOne => RepeatMode::LoopOne,
    }
}

fn host_config_sync_from_app(app: &AppState) -> HostConfigSync {
    HostConfigSync {
        theme: app.config.theme.clone(),
        transparent_background: app.config.transparent_background,
        album_border: app.config.album_border,
        language: app.language,
        graphics_protocol: app.config.graphics_protocol,
        page_lyrics: app.config.page_lyrics,
        audio_quality: match app.config.audio_quality {
            AudioQuality::Standard => crate::data::config::AudioQuality::Standard,
            AudioQuality::Higher => crate::data::config::AudioQuality::Higher,
            AudioQuality::Exhigh => crate::data::config::AudioQuality::Exhigh,
            AudioQuality::Lossless => crate::data::config::AudioQuality::Lossless,
            AudioQuality::Hires => crate::data::config::AudioQuality::Hires,
            AudioQuality::Jyeffect => crate::data::config::AudioQuality::Jyeffect,
            AudioQuality::Sky => crate::data::config::AudioQuality::Sky,
            AudioQuality::Dolby => crate::data::config::AudioQuality::Dolby,
            AudioQuality::Jymaster => crate::data::config::AudioQuality::Jymaster,
        },
        eq_bands_db: app.config.eq_bands_db,
        playback_memory: app.config.playback_memory,
        vip_audio_unlocked: app.vip_audio_unlocked,
        show_hints: app.config.show_hints,
        home_more_recommend: app.config.home_more_recommend,
        visualize: match app.config.visualize {
            VisualizeMode::Off => crate::data::config::VisualizeMode::Off,
            VisualizeMode::Bars => crate::data::config::VisualizeMode::Bars,
            VisualizeMode::Oscilloscope => crate::data::config::VisualizeMode::Oscilloscope,
        },
        super_smooth_bar: app.config.super_smooth_bar,
        bars_gap: app.config.bars_gap,
        bar_number: match app.config.bar_number {
            BarNumber::Auto => crate::data::config::BarNumber::Auto,
            BarNumber::N16 => crate::data::config::BarNumber::N16,
            BarNumber::N32 => crate::data::config::BarNumber::N32,
            BarNumber::N48 => crate::data::config::BarNumber::N48,
            BarNumber::N64 => crate::data::config::BarNumber::N64,
            BarNumber::N80 => crate::data::config::BarNumber::N80,
            BarNumber::N96 => crate::data::config::BarNumber::N96,
        },
        bar_channels: match app.config.bar_channels {
            BarChannels::Stereo => crate::data::config::BarChannels::Stereo,
            BarChannels::Mono => crate::data::config::BarChannels::Mono,
        },
        bar_channel_reverse: app.config.bar_channel_reverse,
    }
}

fn apply_host_config_sync(app: &mut AppState, config: HostConfigSync) {
    if app.config.theme != config.theme {
        if let Ok(theme) = ThemeLoader::load(&config.theme) {
            app.theme = theme;
            app.config.theme = config.theme;
        }
    }

    app.config.transparent_background = config.transparent_background;
    app.config.album_border = config.album_border;
    app.language = config.language;
    app.config.page_lyrics = config.page_lyrics;
    app.vip_audio_unlocked = config.vip_audio_unlocked;
    app.config.audio_quality = match config.audio_quality {
        crate::data::config::AudioQuality::Standard => AudioQuality::Standard,
        crate::data::config::AudioQuality::Higher => AudioQuality::Higher,
        crate::data::config::AudioQuality::Exhigh => AudioQuality::Exhigh,
        crate::data::config::AudioQuality::Lossless => AudioQuality::Lossless,
        crate::data::config::AudioQuality::Hires => AudioQuality::Hires,
        crate::data::config::AudioQuality::Jyeffect => AudioQuality::Jyeffect,
        crate::data::config::AudioQuality::Sky => AudioQuality::Sky,
        crate::data::config::AudioQuality::Dolby => AudioQuality::Dolby,
        crate::data::config::AudioQuality::Jymaster => AudioQuality::Jymaster,
    }
    .clamp_for_vip(app.vip_audio_unlocked);
    app.config.eq_bands_db = config.eq_bands_db;
    app.eq.bands_db = config.eq_bands_db;
    app.config.playback_memory = config.playback_memory;
    app.config.show_hints = config.show_hints;
    app.config.home_more_recommend = config.home_more_recommend;
    app.config.visualize = match config.visualize {
        crate::data::config::VisualizeMode::Off => VisualizeMode::Off,
        crate::data::config::VisualizeMode::Bars => VisualizeMode::Bars,
        crate::data::config::VisualizeMode::Oscilloscope => VisualizeMode::Oscilloscope,
    };
    app.config.super_smooth_bar = config.super_smooth_bar;
    app.config.bars_gap = config.bars_gap;
    app.config.bar_number = match config.bar_number {
        crate::data::config::BarNumber::Auto => BarNumber::Auto,
        crate::data::config::BarNumber::N16 => BarNumber::N16,
        crate::data::config::BarNumber::N32 => BarNumber::N32,
        crate::data::config::BarNumber::N48 => BarNumber::N48,
        crate::data::config::BarNumber::N64 => BarNumber::N64,
        crate::data::config::BarNumber::N80 => BarNumber::N80,
        crate::data::config::BarNumber::N96 => BarNumber::N96,
    };
    app.config.bar_channels = match config.bar_channels {
        crate::data::config::BarChannels::Stereo => BarChannels::Stereo,
        crate::data::config::BarChannels::Mono => BarChannels::Mono,
    };
    app.config.bar_channel_reverse = config.bar_channel_reverse;
}

async fn save_and_sync_host_config(
    app: &mut AppState,
    host_bridge: &mut Option<&mut impl HostPlaybackBridge>,
) {
    let _ = app.config.save();
    if let Some(bridge) = host_bridge.as_mut() {
        (*bridge)
            .apply_config_sync(host_config_sync_from_app(app))
            .await;
    }
}

async fn sync_eq_config(
    app: &mut AppState,
    host_bridge: &mut Option<&mut impl HostPlaybackBridge>,
) {
    app.eq = app.eq.clamp();
    app.config.eq_bands_db = app.eq.bands_db;

    save_and_sync_host_config(app, host_bridge).await;
}

fn empty_track_metadata() -> crate::tmplayer::app::state::TrackMetadata {
    crate::tmplayer::app::state::TrackMetadata {
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        duration: Duration::from_secs(0),
        cover: None,
        cover_hash: None,
        cover_folder: None,
        lyrics: None,
    }
}

fn hash_cover_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn ncm_song_id_for_index(app: &AppState, index: usize) -> Option<String> {
    let item = app.playlist.items.get(index)?;
    let raw = item.path.to_string_lossy();
    let id = raw.strip_prefix("ncm://")?.trim();
    if id.is_empty() {
        return None;
    }
    Some(id.to_string())
}

fn ncm_cover_cache_path(dir: &std::path::Path, song_id: &str) -> Option<PathBuf> {
    let id = song_id.trim();
    if id.is_empty() {
        return None;
    }
    Some(dir.join(format!("{id}.img")))
}

fn persist_ncm_cover_to_disk(dir: &std::path::Path, song_id: &str, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let Some(path) = ncm_cover_cache_path(dir, song_id) else {
        return;
    };
    let _ = fs::create_dir_all(dir);
    let _ = fs::write(path, bytes);
}

fn load_ncm_cover_from_disk(dir: &std::path::Path, song_id: &str) -> Option<(Vec<u8>, u64)> {
    let path = ncm_cover_cache_path(dir, song_id)?;
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let hash = hash_cover_bytes(&bytes);
    Some((bytes, hash))
}

fn enforce_ncm_cover_memory_policy(app: &mut AppState, current_index: usize) {
    let Some(cache_dir) = app.ncm_cover_cache_dir.clone() else {
        return;
    };
    if app.api_tracks.is_empty() {
        return;
    }

    let song_ids: Vec<Option<String>> = (0..app.api_tracks.len())
        .map(|idx| ncm_song_id_for_index(app, idx))
        .collect();

    for (idx, track) in app.api_tracks.iter_mut().enumerate() {
        let song_id = song_ids.get(idx).and_then(|v| v.as_deref());

        if idx == current_index {
            if track.cover.is_none() {
                if let Some(song_id) = song_id {
                    if let Some((bytes, hash)) = load_ncm_cover_from_disk(&cache_dir, song_id) {
                        track.cover = Some(bytes);
                        track.cover_hash = Some(hash);
                    }
                }
            }
            continue;
        }

        if let (Some(bytes), Some(song_id)) = (track.cover.as_deref(), song_id) {
            persist_ncm_cover_to_disk(&cache_dir, song_id, bytes);
        }
        track.cover = None;
        track.cover_folder = None;
    }

    if let Some(cur) = app.api_tracks.get(current_index).cloned() {
        app.player.track = cur;
    }
}

fn sync_from_host_snapshot(app: &mut AppState, snapshot: HostPlaybackSnapshot) {
    let queue_len = snapshot.playlist.len();

    if queue_len == 0 {
        app.api_tracks.clear();
        app.playlist = crate::tmplayer::data::playlist::Playlist::default();
        app.playlist_view = crate::tmplayer::data::playlist::Playlist::default();
        app.player.mode = PlayMode::Idle;
        app.player.playback = map_host_state(snapshot.state);
        app.player.repeat_mode = map_host_repeat(snapshot.repeat_mode);
        app.player.liked = false;
        app.player.position = snapshot.position;
        app.player.track = empty_track_metadata();
        app.local_view_album_cover = None;
        app.local_view_album_cover_hash = None;
        return;
    }

    let mut playlist = crate::tmplayer::data::playlist::Playlist::default();
    let mut tracks = Vec::with_capacity(queue_len);

    for (idx, item) in snapshot.playlist.iter().enumerate() {
        let id = item.id.clone().unwrap_or_else(|| format!("seed-{idx}"));
        let title = if item.title.trim().is_empty() {
            format!("Track {}", idx + 1)
        } else {
            item.title.clone()
        };

        playlist
            .items
            .push(crate::tmplayer::data::playlist::PlaylistItem {
                path: PathBuf::from(format!("ncm://{id}")),
                title,
            });

        tracks.push(crate::tmplayer::app::state::TrackMetadata {
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

    let mut current = snapshot
        .current_index
        .unwrap_or(0)
        .min(queue_len.saturating_sub(1));
    let current_track = if let Some(track) = snapshot.current_track.as_ref() {
        let idx = track
            .playlist_index
            .unwrap_or(current)
            .min(queue_len.saturating_sub(1));
        current = idx;
        let cover_hash = track.cover.as_deref().map(hash_cover_bytes);
        let mapped = crate::tmplayer::app::state::TrackMetadata {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration: track.duration,
            cover: track.cover.clone(),
            cover_hash,
            cover_folder: None,
            lyrics: track.lyrics.clone(),
        };
        tracks[idx] = mapped.clone();
        mapped
    } else {
        tracks
            .get(current)
            .cloned()
            .unwrap_or_else(empty_track_metadata)
    };

    playlist.current = Some(current);
    playlist.selected = current;
    playlist.clamp_selected();

    let keep_selected = app.overlay == Overlay::Playlist && app.playlist_view.len() == queue_len;
    let view_selected = if keep_selected {
        app.playlist_view.selected.min(queue_len.saturating_sub(1))
    } else {
        current
    };

    let mut view = playlist.clone();
    view.selected = view_selected;
    view.clamp_selected();

    app.api_tracks = tracks;
    app.playlist = playlist;
    app.playlist_view = view;

    app.player.mode = PlayMode::Idle;
    app.player.playback = map_host_state(snapshot.state);
    app.player.repeat_mode = map_host_repeat(snapshot.repeat_mode);
    app.player.liked = snapshot.current_liked;
    app.player.position = snapshot.position;
    app.player.track = current_track;

    enforce_ncm_cover_memory_policy(app, current);
}

fn apply_host_runtime_snapshot(app: &mut AppState, runtime: HostPlaybackRuntimeSnapshot) -> bool {
    let mut changed = false;

    let playback = map_host_state(runtime.state);
    if app.player.playback != playback {
        app.player.playback = playback;
        changed = true;
    }

    let repeat = map_host_repeat(runtime.repeat_mode);
    if app.player.repeat_mode != repeat {
        app.player.repeat_mode = repeat;
        changed = true;
    }

    if app.player.liked != runtime.current_liked {
        app.player.liked = runtime.current_liked;
        changed = true;
    }

    if app.player.position != runtime.position {
        app.player.position = runtime.position;
        changed = true;
    }

    let runtime_volume = runtime.volume.clamp(0.0, 1.0);
    if (app.player.volume - runtime_volume).abs() > f32::EPSILON {
        app.player.volume = runtime_volume;
        changed = true;
    }

    if let Some(index) = runtime.current_index {
        if !app.playlist.items.is_empty() {
            let idx = index.min(app.playlist.len().saturating_sub(1));
            if app.playlist.current != Some(idx) {
                app.playlist.current = Some(idx);
                app.playlist.selected = idx;
                app.playlist.clamp_selected();
                changed = true;
            }
        }
    }

    changed
}

async fn sync_from_host_bridge(
    app: &mut AppState,
    host_bridge: &mut Option<&mut impl HostPlaybackBridge>,
    last_metadata_signature: &mut Option<u64>,
    sync_config: bool,
) -> bool {
    let Some(bridge) = host_bridge.as_mut() else {
        return false;
    };

    let mut changed = false;
    (*bridge).tick().await;

    if sync_config {
        let config = (*bridge).config_snapshot();
        apply_host_config_sync(app, config);
        changed = true;
    }

    let runtime = (*bridge).runtime_snapshot();
    changed |= apply_host_runtime_snapshot(app, runtime);

    let metadata_signature = (*bridge).metadata_signature();
    if last_metadata_signature.is_none_or(|sig| sig != metadata_signature) {
        let snapshot = (*bridge).snapshot();
        sync_from_host_snapshot(app, snapshot);
        *last_metadata_signature = Some(metadata_signature);
        changed = true;
    }

    changed
}

pub async fn run(
    app: &mut AppState,
    mut host_bridge: Option<&mut impl HostPlaybackBridge>,
) -> Result<crate::tmplayer::FullscreenExit> {
    enable_raw_mode()?;
    let mut tui = Tui::new(app)?;
    tui.enter()?;

    // Prefer cava for system-wide visualization (keeps our renderer/style; cava only provides bars).
    // If cava isn't installed, we leave the spectrum empty.
    let mut cava: Option<CavaRunner> = None;
    let mut cava_cfg: Option<CavaConfig> = None;

    let mut last_spectrum = Instant::now();
    let mut last_host_config_sync = Instant::now()
        .checked_sub(Duration::from_millis(400))
        .unwrap_or_else(Instant::now);
    let mut last_host_metadata_signature: Option<u64> = None;
    let mut needs_redraw = true;
    let mut last_draw_at = Instant::now()
        .checked_sub(Duration::from_millis(250))
        .unwrap_or_else(Instant::now);

    let mut last_layout = UiLayout::default();

    // Initialize cava with the current desired config (best-effort).
    ensure_cava(
        &mut cava,
        &mut cava_cfg,
        desired_cava_config(app, &last_layout),
    );

    let _ = sync_from_host_bridge(
        app,
        &mut host_bridge,
        &mut last_host_metadata_signature,
        true,
    );

    loop {
        let frame_start = Instant::now();
        let mut state_changed = false;

        let sync_config =
            frame_start.duration_since(last_host_config_sync) >= Duration::from_millis(250);
        if sync_config {
            last_host_config_sync = frame_start;
        }

        state_changed |= sync_from_host_bridge(
            app,
            &mut host_bridge,
            &mut last_host_metadata_signature,
            sync_config,
        )
        .await;

        while event::poll(Duration::from_millis(0))? {
            match event::read()? {
                Event::Key(k) => {
                    let action = map_key(k, app.overlay, &app.config);
                    handle_action(app, &mut host_bridge, action, &last_layout).await?;
                    state_changed = true;
                }
                Event::Mouse(m) => {
                    let action = map_mouse(m);
                    handle_action(app, &mut host_bridge, action, &last_layout).await?;
                    state_changed = true;
                }
                Event::Resize(_, _) => {
                    // Kitty graphics placements may get cleared on terminal resize.
                    tui.on_resize();
                    state_changed = true;
                }
                _ => {}
            }
        }

        ensure_cava(
            &mut cava,
            &mut cava_cfg,
            desired_cava_config(app, &last_layout),
        );

        if app.config.visualize == VisualizeMode::Bars {
            let bars = desired_bar_count(app, &last_layout);
            ensure_bar_buffers(app, bars);
        }

        // spectrum update
        if app.config.visualize == VisualizeMode::Off {
            if has_spectrum_data(app) {
                clear_spectrum(app);
                state_changed = true;
            }
        } else if frame_start.duration_since(last_spectrum)
            >= Duration::from_millis((1000 / app.config.spectrum_hz.max(1)) as u64)
        {
            last_spectrum = frame_start;
            state_changed = true;

            match app.config.visualize {
                VisualizeMode::Off => clear_spectrum(app),
                VisualizeMode::Bars => {
                    if let Some(c) = cava.as_ref() {
                        let (l, r) = c.latest_stereo_bars();
                        app.spectrum.bars_left = l;
                        app.spectrum.bars_right = r;
                        let raw = c.latest_bars();
                        app.spectrum.bars = app.spectrum_bar_smoother.apply(&raw);
                    } else {
                        clear_spectrum(app);
                    }
                }
                VisualizeMode::Oscilloscope => {
                    if let Some(c) = cava.as_ref() {
                        let (l, r) = c.latest_stereo_bars();
                        fill_fixed_bars(&mut app.spectrum.stereo_left, &l);
                        fill_fixed_bars(&mut app.spectrum.stereo_right, &r);
                        app.spectrum.bars = c.latest_bars();
                    } else {
                        clear_spectrum(app);
                    }

                    let dt = 1.0 / app.config.spectrum_hz.max(1) as f32;
                    crate::tmplayer::render::oscilloscope_renderer::advance_phases(
                        &mut app.spectrum.osc_phase_left,
                        dt,
                    );
                    crate::tmplayer::render::oscilloscope_renderer::advance_phases(
                        &mut app.spectrum.osc_phase_right,
                        dt,
                    );
                }
            }
        }

        if app.player.mode == PlayMode::Idle && app.player.playback == PlaybackState::Playing {
            let dt = frame_start.saturating_duration_since(app.last_frame);
            if dt > Duration::from_millis(0) {
                let next = app.player.position.saturating_add(dt);
                app.player.position = if app.player.track.duration > Duration::from_millis(0) {
                    next.min(app.player.track.duration)
                } else {
                    next
                };
                state_changed = true;
            }
        }

        app.tick(frame_start);

        if app.should_continuous_redraw() {
            state_changed = true;
        }

        let target_fps = if app.should_continuous_redraw() {
            app.active_render_fps()
        } else {
            app.idle_render_fps()
        };
        let frame_dt = fps_to_dt(target_fps);

        if state_changed {
            needs_redraw = true;
        }

        if app.should_continuous_redraw() && last_draw_at.elapsed() >= frame_dt {
            needs_redraw = true;
        }

        if needs_redraw {
            last_layout = tui.draw(app)?;
            last_draw_at = Instant::now();
            needs_redraw = false;
        }

        // frame pacing
        let elapsed = frame_start.elapsed();
        if elapsed < frame_dt {
            std::thread::sleep(frame_dt - elapsed);
        }

        if tui.should_quit {
            break;
        }
    }

    tui.exit()?;
    disable_raw_mode()?;

    let exit = if app.request_host_settings_open {
        crate::tmplayer::FullscreenExit::BackToHostOpenSettings
    } else {
        crate::tmplayer::FullscreenExit::BackToHost
    };
    Ok(exit)
}

fn fps_to_dt(fps: u32) -> Duration {
    let fps = fps.clamp(4, 60);
    Duration::from_millis((1000 / fps) as u64)
}

fn switch_idle_track(app: &mut AppState, dir: i8) {
    if app.api_tracks.is_empty() {
        return;
    }

    let from = CoverSnapshot::from(&app.player.track);
    let next = if dir < 0 {
        match app.player.repeat_mode {
            RepeatMode::Sequence => app.playlist.next_index_no_wrap(),
            RepeatMode::LoopAll => app.playlist.next_index_sequence(),
            RepeatMode::LoopOne => app.playlist.current,
            RepeatMode::Shuffle => pick_shuffle_index(&app.playlist),
        }
    } else {
        match app.player.repeat_mode {
            RepeatMode::Sequence => app.playlist.prev_index_no_wrap(),
            RepeatMode::LoopAll => app.playlist.prev_index_sequence(),
            RepeatMode::LoopOne => app.playlist.current,
            RepeatMode::Shuffle => pick_shuffle_index(&app.playlist),
        }
    };

    let Some(i) = next else {
        return;
    };

    if let Some(track) = app.api_tracks.get(i).cloned() {
        app.playlist.current = Some(i);
        app.playlist.selected = i;
        app.playlist_view.current = Some(i);
        app.playlist_view.selected = i;

        app.player.track = track;
        app.player.position = Duration::from_secs(0);
        app.player.playback = PlaybackState::Playing;

        enforce_ncm_cover_memory_policy(app, i);

        let to = CoverSnapshot::from(&app.player.track);
        app.start_cover_anim(from, to, dir, Instant::now());
    }
}

async fn handle_action(
    app: &mut AppState,
    host_bridge: &mut Option<&mut impl HostPlaybackBridge>,
    action: Action,
    layout: &UiLayout,
) -> Result<()> {
    match action {
        Action::Quit => {
            // handled by tui flag
            app.set_toast("Bye");
        }
        Action::OpenSettingsModal => {
            app.settings_selected = app.settings_selected.min(9);
            app.overlay = Overlay::SettingsModal;
        }
        Action::OpenHelpModal => {
            app.help_keybind_selected = app
                .help_keybind_selected
                .min(HELP_MODAL_ITEMS.saturating_sub(1));
            app.overlay = Overlay::HelpModal;
        }
        Action::OpenEqModal => {
            app.overlay = Overlay::EqModal;
            app.eq_selected = 0;
        }
        Action::EqSetBandDb { band, db } => {
            if app.overlay == Overlay::EqModal {
                app.eq_selected = band.min(crate::tmplayer::app::state::EQ_BANDS.saturating_sub(1));
                let db = db.clamp(-12.0, 12.0);
                if app.eq_selected < crate::tmplayer::app::state::EQ_BANDS {
                    app.eq.bands_db[app.eq_selected] = db;
                }
                sync_eq_config(app, host_bridge).await;
            }
        }
        Action::EqResetDefault => {
            if app.overlay == Overlay::EqModal {
                app.eq = crate::tmplayer::app::state::EqSettings::default();
                app.eq_selected = 0;
                sync_eq_config(app, host_bridge).await;
            }
        }
        Action::FolderChar(c) => {
            if app.overlay == Overlay::AcoustIdModal {
                app.acoustid_input.push(c);
            }
        }
        Action::FolderBackspace => {
            if app.overlay == Overlay::AcoustIdModal {
                app.acoustid_input.pop();
            }
        }
        Action::CloseOverlay => {
            if app.overlay == Overlay::Playlist {
                // close animation will be driven by ui
                // actual state closed after fully slid out
                // here just set target
                app.playlist_slide_target_x = -(layout.left_width as i16);
                app.overlay = Overlay::None;
            } else if app.overlay == Overlay::AcoustIdModal
                || app.overlay == Overlay::BarSettingsModal
                || app.overlay == Overlay::LocalAudioSettingsModal
                || app.overlay == Overlay::AboutModal
            {
                app.overlay = Overlay::SettingsModal;
            } else {
                app.close_overlay();
            }
        }
        Action::TogglePlaylist => {
            if app.overlay == Overlay::Playlist {
                app.playlist_slide_target_x = -(layout.left_width as i16);
                app.overlay = Overlay::None;
            } else {
                // 需求：打开 playlist 时聚焦当前播放的歌曲。
                app.playlist_view = app.playlist.clone();
                if let Some(cur) = app.playlist.current {
                    app.playlist_view.selected = cur;
                    app.playlist_view.clamp_selected();
                }

                // Always reset view state to the currently playing folder when opening.
                app.local_view_album_folder = app.local_folder.clone();
                if let Some(folder) = app.local_folder.as_deref() {
                    let cover = crate::tmplayer::playback::metadata::read_cover_from_folder(folder);
                    app.local_view_album_cover = cover.as_ref().map(|(b, _)| b.clone());
                    app.local_view_album_cover_hash = cover.map(|(_, h)| Some(h)).unwrap_or(None);
                }
                app.playlist_album_anim = None;

                // Keep view album index in sync for MultiAlbum.
                if app.local_folder_kind == LocalFolderKind::MultiAlbum {
                    if let Some(vf) = app.local_view_album_folder.as_ref() {
                        if let Some(i) = app.local_album_folders.iter().position(|p| p == vf) {
                            app.local_view_album_index = i;
                        }
                    }
                }
                app.overlay = Overlay::Playlist;
                app.playlist_slide_x = -(layout.left_width as i16);
                app.playlist_slide_target_x = 0;
            }
        }
        Action::Confirm => match app.overlay {
            Overlay::Playlist => {
                if let Some(bridge) = host_bridge.as_mut() {
                    let idx = app
                        .playlist_view
                        .selected
                        .min(app.playlist_view.len().saturating_sub(1));
                    (*bridge).play_queue_index(idx).await;
                    let snapshot = (*bridge).snapshot();
                    sync_from_host_snapshot(app, snapshot);
                    return Ok(());
                }
            }
            Overlay::SettingsModal => match app.settings_selected {
                0 | 1 | 2 | 3 => {
                    apply_settings_delta(app, host_bridge, 1).await;
                }
                4 => {
                    app.bar_settings_selected = 0;
                    app.overlay = Overlay::BarSettingsModal;
                }
                5 => {
                    app.overlay = Overlay::HelpModal;
                }
                6 => {
                    apply_settings_delta(app, host_bridge, 1).await;
                }
                7 => {
                    apply_settings_delta(app, host_bridge, 1).await;
                }
                8 => {
                    app.set_toast("Logout is unavailable in fullscreen");
                }
                9 => {
                    app.overlay = Overlay::AboutModal;
                }
                _ => {}
            },
            Overlay::BarSettingsModal => match app.bar_settings_selected {
                0 => {
                    if crate::tmplayer::audio::cava::is_available() {
                        app.config.visualize = app.config.visualize.cycle(1);
                        save_and_sync_host_config(app, host_bridge).await;
                    } else if app.config.visualize
                        != crate::tmplayer::data::config::VisualizeMode::Off
                    {
                        app.config.visualize = crate::tmplayer::data::config::VisualizeMode::Off;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                }
                1 => {
                    app.config.super_smooth_bar = !app.config.super_smooth_bar;
                    save_and_sync_host_config(app, host_bridge).await;
                }
                2 => {
                    app.config.bars_gap = !app.config.bars_gap;
                    save_and_sync_host_config(app, host_bridge).await;
                }
                3 => {
                    app.config.bar_number = cycle_bar_number(app.config.bar_number, 1);
                    save_and_sync_host_config(app, host_bridge).await;
                }
                4 => {
                    app.config.bar_channels = toggle_bar_channels(app.config.bar_channels);
                    save_and_sync_host_config(app, host_bridge).await;
                }
                5 => {
                    app.config.album_border = !app.config.album_border;
                    save_and_sync_host_config(app, host_bridge).await;
                }
                6 => {
                    app.config.page_lyrics = !app.config.page_lyrics;
                    save_and_sync_host_config(app, host_bridge).await;
                }
                7 => {
                    app.config.audio_quality =
                        app.config.audio_quality.cycle(1, app.vip_audio_unlocked);
                    save_and_sync_host_config(app, host_bridge).await;
                }
                8 => {
                    app.config.playback_memory = !app.config.playback_memory;
                    save_and_sync_host_config(app, host_bridge).await;
                }
                _ => {}
            },
            Overlay::LocalAudioSettingsModal => match app.local_audio_settings_selected {
                0 => {
                    app.config.lyrics_cover_fetch = !app.config.lyrics_cover_fetch;
                    let _ = app.config.save();
                    if app.config.lyrics_cover_fetch {
                        app.reset_remote_fetch_state();
                    }
                }
                1 => {
                    app.config.lyrics_cover_download = !app.config.lyrics_cover_download;
                    let _ = app.config.save();
                }
                2 => {
                    if !app.config.acoustid_api_key.trim().is_empty() {
                        app.config.audio_fingerprint = !app.config.audio_fingerprint;
                        let _ = app.config.save();
                    }
                }
                3 => {
                    app.acoustid_input = app.config.acoustid_api_key.clone();
                    app.overlay = Overlay::AcoustIdModal;
                }
                4 => {
                    app.config.resume_last_position = !app.config.resume_last_position;
                    let _ = app.config.save();
                }
                _ => {}
            },
            Overlay::AcoustIdModal => {
                let key = app.acoustid_input.trim().to_string();
                app.config.acoustid_api_key = key.clone();
                if key.is_empty() {
                    app.config.audio_fingerprint = false;
                }
                let _ = app.config.save();
                app.overlay = Overlay::SettingsModal;
            }
            Overlay::HelpModal => {
                app.close_overlay();
            }
            Overlay::EqModal => {
                app.close_overlay();
            }
            _ => {}
        },
        Action::PlaylistUp => {
            app.playlist_view.move_up();
            app.playlist_view.clamp_selected();
            sync_playlists_when_viewing_playback(app);
        }
        Action::PlaylistDown => {
            app.playlist_view.move_down();
            app.playlist_view.clamp_selected();
            sync_playlists_when_viewing_playback(app);
        }
        Action::PlaylistMoveItemUp => (),
        Action::PlaylistMoveItemDown => (),
        Action::PrevAlbum | Action::NextAlbum => (),
        Action::ModalUp => {
            if app.overlay == Overlay::SettingsModal {
                let count = 10;
                if app.settings_selected == 0 {
                    app.settings_selected = count - 1;
                } else {
                    app.settings_selected -= 1;
                }
            } else if app.overlay == Overlay::BarSettingsModal {
                let count = 9;
                if app.bar_settings_selected == 0 {
                    app.bar_settings_selected = count - 1;
                } else {
                    app.bar_settings_selected -= 1;
                }
            } else if app.overlay == Overlay::LocalAudioSettingsModal {
                let count = 5;
                if app.local_audio_settings_selected == 0 {
                    app.local_audio_settings_selected = count - 1;
                } else {
                    app.local_audio_settings_selected -= 1;
                }
            } else if app.overlay == Overlay::EqModal {
                let step = 1.0;
                if app.eq_selected < crate::tmplayer::app::state::EQ_BANDS {
                    let v = app.eq.bands_db[app.eq_selected];
                    app.eq.bands_db[app.eq_selected] = (v + step).clamp(-12.0, 12.0);
                }
                sync_eq_config(app, host_bridge).await;
            } else if app.overlay == Overlay::HelpModal {
                if app.help_keybind_selected == 0 {
                    app.help_keybind_selected = HELP_MODAL_ITEMS - 1;
                } else {
                    app.help_keybind_selected -= 1;
                }
            }
        }
        Action::ModalDown => {
            if app.overlay == Overlay::SettingsModal {
                let count = 10;
                app.settings_selected = (app.settings_selected + 1) % count;
            } else if app.overlay == Overlay::BarSettingsModal {
                let count = 9;
                app.bar_settings_selected = (app.bar_settings_selected + 1) % count;
            } else if app.overlay == Overlay::LocalAudioSettingsModal {
                let count = 5;
                app.local_audio_settings_selected = (app.local_audio_settings_selected + 1) % count;
            } else if app.overlay == Overlay::EqModal {
                let step = 1.0;
                if app.eq_selected < crate::tmplayer::app::state::EQ_BANDS {
                    let v = app.eq.bands_db[app.eq_selected];
                    app.eq.bands_db[app.eq_selected] = (v - step).clamp(-12.0, 12.0);
                }
                sync_eq_config(app, host_bridge).await;
            } else if app.overlay == Overlay::HelpModal {
                app.help_keybind_selected = (app.help_keybind_selected + 1) % HELP_MODAL_ITEMS;
            }
        }
        Action::ModalLeft => {
            if app.overlay == Overlay::SettingsModal {
                apply_settings_delta(app, host_bridge, -1).await;
            } else if app.overlay == Overlay::BarSettingsModal {
                match app.bar_settings_selected {
                    0 => {
                        if crate::tmplayer::audio::cava::is_available() {
                            app.config.visualize = app.config.visualize.cycle(-1);
                            save_and_sync_host_config(app, host_bridge).await;
                        }
                    }
                    1 => {
                        app.config.super_smooth_bar = !app.config.super_smooth_bar;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    2 => {
                        app.config.bars_gap = !app.config.bars_gap;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    3 => {
                        app.config.bar_number = cycle_bar_number(app.config.bar_number, -1);
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    4 => {
                        app.config.bar_channels = toggle_bar_channels(app.config.bar_channels);
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    5 => {
                        app.config.album_border = !app.config.album_border;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    6 => {
                        app.config.page_lyrics = !app.config.page_lyrics;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    7 => {
                        app.config.audio_quality =
                            app.config.audio_quality.cycle(-1, app.vip_audio_unlocked);
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    8 => {
                        app.config.playback_memory = !app.config.playback_memory;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    _ => {}
                }
            } else if app.overlay == Overlay::LocalAudioSettingsModal {
                apply_local_audio_settings_delta(app, -1);
            } else if app.overlay == Overlay::EqModal {
                let count = crate::tmplayer::app::state::EQ_BANDS;
                if app.eq_selected == 0 {
                    app.eq_selected = count - 1;
                } else {
                    app.eq_selected -= 1;
                }
            }
        }
        Action::ModalRight => {
            if app.overlay == Overlay::SettingsModal {
                apply_settings_delta(app, host_bridge, 1).await;
            } else if app.overlay == Overlay::BarSettingsModal {
                match app.bar_settings_selected {
                    0 => {
                        if crate::tmplayer::audio::cava::is_available() {
                            app.config.visualize = app.config.visualize.cycle(1);
                            save_and_sync_host_config(app, host_bridge).await;
                        }
                    }
                    1 => {
                        app.config.super_smooth_bar = !app.config.super_smooth_bar;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    2 => {
                        app.config.bars_gap = !app.config.bars_gap;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    3 => {
                        app.config.bar_number = cycle_bar_number(app.config.bar_number, 1);
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    4 => {
                        app.config.bar_channels = toggle_bar_channels(app.config.bar_channels);
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    5 => {
                        app.config.album_border = !app.config.album_border;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    6 => {
                        app.config.page_lyrics = !app.config.page_lyrics;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    7 => {
                        app.config.audio_quality =
                            app.config.audio_quality.cycle(1, app.vip_audio_unlocked);
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    8 => {
                        app.config.playback_memory = !app.config.playback_memory;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    _ => {}
                }
            } else if app.overlay == Overlay::LocalAudioSettingsModal {
                apply_local_audio_settings_delta(app, 1);
            } else if app.overlay == Overlay::EqModal {
                let count = crate::tmplayer::app::state::EQ_BANDS;
                app.eq_selected = (app.eq_selected + 1) % count;
            }
        }
        Action::PlaylistSelect(idx) => {
            if idx < app.playlist_view.len() {
                app.playlist_view.selected = idx;
                app.playlist_view.clamp_selected();
                sync_playlists_when_viewing_playback(app);

                if let Some(bridge) = host_bridge.as_mut() {
                    (*bridge).play_queue_index(idx).await;
                    let snapshot = (*bridge).snapshot();
                    sync_from_host_snapshot(app, snapshot);
                    return Ok(());
                }

                // double click => play
                let now = Instant::now();
                if let Some((at, last_col, last_row)) = app.last_mouse_click {
                    if now.duration_since(at) <= Duration::from_millis(400) {
                        // same row (best-effort)
                        if last_row == (layout.playlist_list_inner.y + idx as u16) {
                            return Box::pin(handle_action(
                                app,
                                host_bridge,
                                Action::Confirm,
                                layout,
                            ))
                            .await;
                        }
                        let _ = last_col;
                    }
                }
                app.last_mouse_click = Some((now, 0, layout.playlist_list_inner.y + idx as u16));
            }
        }
        Action::TogglePlayPause => {
            if let Some(bridge) = host_bridge.as_mut() {
                (*bridge).toggle_play_pause().await;
                let snapshot = (*bridge).snapshot();
                sync_from_host_snapshot(app, snapshot);
                return Ok(());
            }

            match app.player.mode {
                PlayMode::Idle => {
                    app.player.playback = match app.player.playback {
                        PlaybackState::Playing => PlaybackState::Paused,
                        PlaybackState::Paused | PlaybackState::Stopped => PlaybackState::Playing,
                    };
                }
            }
        }
        Action::Prev => match app.player.mode {
            _ if host_bridge.is_some() => {
                if let Some(bridge) = host_bridge.as_mut() {
                    (*bridge).play_previous().await;
                    let snapshot = (*bridge).snapshot();
                    sync_from_host_snapshot(app, snapshot);
                }
            }
            PlayMode::Idle => {
                switch_idle_track(app, 1);
            }
        },
        Action::Next => match app.player.mode {
            _ if host_bridge.is_some() => {
                if let Some(bridge) = host_bridge.as_mut() {
                    (*bridge).play_next().await;
                    let snapshot = (*bridge).snapshot();
                    sync_from_host_snapshot(app, snapshot);
                }
            }
            PlayMode::Idle => {
                switch_idle_track(app, -1);
            }
        },
        Action::VolumeUp => match app.player.mode {
            PlayMode::Idle => {
                let next = (app.player.volume + 0.05).min(1.0);
                if let Some(bridge) = host_bridge.as_mut() {
                    (*bridge).set_volume(next);
                }
                app.player.volume = next;
            }
        },
        Action::VolumeDown => match app.player.mode {
            PlayMode::Idle => {
                let next = (app.player.volume - 0.05).max(0.0);
                if let Some(bridge) = host_bridge.as_mut() {
                    (*bridge).set_volume(next);
                }
                app.player.volume = next;
            }
        },
        Action::SetVolume(v) => match app.player.mode {
            PlayMode::Idle => {
                let next = v.clamp(0.0, 1.0);
                if let Some(bridge) = host_bridge.as_mut() {
                    (*bridge).set_volume(next);
                }
                app.player.volume = next;
            }
        },
        Action::ToggleRepeatMode => {
            if let Some(bridge) = host_bridge.as_mut() {
                (*bridge).toggle_repeat_mode();
                let snapshot = (*bridge).snapshot();
                sync_from_host_snapshot(app, snapshot);
                return Ok(());
            }
            if app.player.mode == PlayMode::Idle {
                app.player.repeat_mode = app.player.repeat_mode.next();
            }
        }
        Action::ToggleFavorite => {
            if let Some(bridge) = host_bridge.as_mut() {
                (*bridge).toggle_like_current().await;
                let snapshot = (*bridge).snapshot();
                sync_from_host_snapshot(app, snapshot);
                return Ok(());
            }

            app.set_toast("Like is unavailable in local mode");
        }
        Action::SeekToFraction(r) => {
            if let Some(bridge) = host_bridge.as_mut() {
                (*bridge).seek_to_ratio(r);
                let snapshot = (*bridge).snapshot();
                sync_from_host_snapshot(app, snapshot);
                return Ok(());
            }

            let dur = app.player.track.duration;
            if dur.as_millis() == 0 {
                return Ok(());
            }
            let target = Duration::from_secs_f32(dur.as_secs_f32() * r.clamp(0.0, 1.0));
            match app.player.mode {
                PlayMode::Idle => {
                    app.player.position = target;
                }
            }
        }
        Action::MouseClick { col, row } => {
            // map click to controls/progress/volume/playlist
            if let Some(a) = crate::tmplayer::ui::tui::hit_test(layout, app, col, row) {
                Box::pin(handle_action(app, host_bridge, a, layout)).await?;
            }
        }
        Action::None => {}
    }

    Ok(())
}

fn themes() -> [ThemeName; 5] {
    [
        ThemeName::System,
        ThemeName::Latte,
        ThemeName::Frappe,
        ThemeName::Macchiato,
        ThemeName::Mocha,
    ]
}

fn theme_count() -> usize {
    themes().len()
}

fn theme_index(name: ThemeName) -> usize {
    themes().iter().position(|&t| t == name).unwrap_or(0)
}

fn theme_by_index(idx: usize) -> ThemeName {
    let t = themes();
    t[idx.min(t.len().saturating_sub(1))]
}

fn theme_key(name: ThemeName) -> &'static str {
    match name {
        ThemeName::System => "system",
        ThemeName::Latte => "latte",
        ThemeName::Frappe => "frappe",
        ThemeName::Macchiato => "macchiato",
        ThemeName::Mocha => "mocha",
    }
}

async fn apply_settings_delta(
    app: &mut AppState,
    host_bridge: &mut Option<&mut impl HostPlaybackBridge>,
    delta: i32,
) {
    match app.settings_selected {
        // Theme
        0 => {
            let count = theme_count() as i32;
            if count <= 0 {
                return;
            }
            let cur = theme_index(app.theme.name) as i32;
            let next = (cur + delta).rem_euclid(count) as usize;
            let name = theme_by_index(next);
            let key = theme_key(name);
            if let Ok(theme) = ThemeLoader::load(key) {
                app.theme = theme;
                app.config.theme = key.to_string();
                save_and_sync_host_config(app, host_bridge).await;
            } else {
                app.set_toast("Theme load error");
            }
        }
        // Transparent background
        1 => {
            if delta != 0 {
                app.config.transparent_background = !app.config.transparent_background;
                save_and_sync_host_config(app, host_bridge).await;
            }
        }
        // Language
        2 => {
            if delta != 0 {
                app.language = match app.language {
                    crate::data::config::Language::Zh => crate::data::config::Language::En,
                    crate::data::config::Language::En => crate::data::config::Language::Zh,
                };
                save_and_sync_host_config(app, host_bridge).await;
            }
        }
        // Kitty graphics
        3 => {
            if delta != 0 {
                app.config.graphics_protocol = app.config.graphics_protocol.cycle(delta);
                save_and_sync_host_config(app, host_bridge).await;
            }
        }
        // Show hints
        6 => {
            if delta != 0 {
                app.config.show_hints = !app.config.show_hints;
                save_and_sync_host_config(app, host_bridge).await;
            }
        }
        // Home more recommendations
        7 => {
            if delta != 0 {
                app.config.home_more_recommend = !app.config.home_more_recommend;
                save_and_sync_host_config(app, host_bridge).await;
            }
        }
        _ => {}
    }
}

fn apply_local_audio_settings_delta(app: &mut AppState, delta: i32) {
    if delta == 0 {
        return;
    }

    match app.local_audio_settings_selected {
        0 => {
            app.config.lyrics_cover_fetch = !app.config.lyrics_cover_fetch;
            let _ = app.config.save();
            if app.config.lyrics_cover_fetch {
                app.reset_remote_fetch_state();
            }
        }
        1 => {
            app.config.lyrics_cover_download = !app.config.lyrics_cover_download;
            let _ = app.config.save();
        }
        2 => {
            if !app.config.acoustid_api_key.trim().is_empty() {
                app.config.audio_fingerprint = !app.config.audio_fingerprint;
                let _ = app.config.save();
            }
        }
        3 => {}
        4 => {
            app.config.resume_last_position = !app.config.resume_last_position;
            let _ = app.config.save();
        }
        _ => {}
    }
}

fn cycle_bar_number(cur: BarNumber, delta: i32) -> BarNumber {
    let options = [
        BarNumber::Auto,
        BarNumber::N16,
        BarNumber::N32,
        BarNumber::N48,
        BarNumber::N64,
        BarNumber::N80,
        BarNumber::N96,
    ];
    let idx = options.iter().position(|v| *v == cur).unwrap_or(0) as i32;
    let next = (idx + delta).rem_euclid(options.len() as i32) as usize;
    options[next]
}

fn toggle_bar_channels(cur: BarChannels) -> BarChannels {
    match cur {
        BarChannels::Stereo => BarChannels::Mono,
        BarChannels::Mono => BarChannels::Stereo,
    }
}

fn bar_number_value(n: BarNumber) -> usize {
    match n {
        BarNumber::Auto => 64,
        BarNumber::N16 => 16,
        BarNumber::N32 => 32,
        BarNumber::N48 => 48,
        BarNumber::N64 => 64,
        BarNumber::N80 => 80,
        BarNumber::N96 => 96,
    }
}

fn auto_bar_number(width_cells: u16, channels: BarChannels) -> usize {
    if width_cells == 0 {
        return 64;
    }
    let base = match channels {
        BarChannels::Stereo => (width_cells as usize / 2).max(1),
        BarChannels::Mono => width_cells as usize,
    };
    let options = [16usize, 32, 48, 64, 80, 96];
    let mut out = 16usize;
    for v in options {
        if base >= v {
            out = v;
        }
    }
    out
}

fn desired_bar_count(app: &AppState, layout: &UiLayout) -> usize {
    let raw = match app.config.bar_number {
        BarNumber::Auto => auto_bar_number(layout.spectrum_rect.width, app.config.bar_channels),
        v => bar_number_value(v),
    };
    let max_total = max_display_bars(layout.spectrum_rect.width, app.config.bars_gap);
    let max_per_side = match app.config.bar_channels {
        BarChannels::Stereo => (max_total / 2).max(1),
        BarChannels::Mono => max_total.max(1),
    };
    raw.min(max_per_side).max(1)
}

fn desired_cava_config(app: &AppState, layout: &UiLayout) -> Option<CavaConfig> {
    match app.config.visualize {
        VisualizeMode::Off => None,
        VisualizeMode::Bars => Some({
            let bars = desired_bar_count(app, layout);
            CavaConfig {
                framerate_hz: app.config.spectrum_hz,
                bars,
                channels: CavaChannels::Mono,
                reverse: app.config.bar_channel_reverse,
            }
        }),
        VisualizeMode::Oscilloscope => Some(CavaConfig {
            framerate_hz: app.config.spectrum_hz,
            bars: 64,
            channels: CavaChannels::Mono,
            reverse: app.config.bar_channel_reverse,
        }),
    }
}

fn ensure_cava(
    cava: &mut Option<CavaRunner>,
    cfg: &mut Option<CavaConfig>,
    desired: Option<CavaConfig>,
) {
    if cfg.as_ref() == desired.as_ref() {
        return;
    }

    // Drop old process first to avoid short-lived overlap when recreating cava.
    *cava = None;
    *cfg = None;

    let Some(desired) = desired else {
        return;
    };

    match CavaRunner::start(desired) {
        Ok(c) => {
            *cava = Some(c);
            *cfg = Some(desired);
        }
        Err(e) => {
            if cfg.is_none() {
                log::warn!("cava unavailable; leaving spectrum empty: {e}");
            }
            *cava = None;
            *cfg = None;
        }
    }
}

fn ensure_bar_buffers(app: &mut AppState, bars: usize) {
    if app.spectrum.bars.len() != bars {
        app.spectrum.bars = vec![0.0; bars];
        app.spectrum.bars_left = vec![0.0; bars];
        app.spectrum.bars_right = vec![0.0; bars];
        app.spectrum_bar_smoother = crate::tmplayer::audio::smoother::Ema::new(0.35, bars);
    }
}

fn max_display_bars(width_cells: u16, gap: bool) -> usize {
    if width_cells == 0 {
        return 1;
    }
    let w = width_cells as usize;
    if gap {
        w.div_ceil(2).max(1)
    } else {
        (w / 2).max(1)
    }
}

fn fill_fixed_bars(dst: &mut [f32; 64], src: &[f32]) {
    for i in 0..64 {
        dst[i] = src.get(i).copied().unwrap_or(0.0);
    }
}

fn pick_shuffle_index(pl: &crate::tmplayer::data::playlist::Playlist) -> Option<usize> {
    if pl.items.is_empty() {
        return None;
    }
    let len = pl.items.len();
    if len == 1 {
        return Some(0);
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut idx = (nanos as usize) % len;
    if Some(idx) == pl.current {
        idx = (idx + 1) % len;
    }
    Some(idx)
}

// fallback bars removed (leave spectrum empty when unavailable)
