use crate::data::config::{BarChannels, BarNumber, Config, VisualizeMode};
use crate::data::theme_loader::ThemeLoader;
use crate::tmplayer::app::state::{AppState, Overlay, PlaybackState, RepeatMode};
use crate::tmplayer::audio::cava::{CavaChannels, CavaConfig, CavaService};
use crate::tmplayer::ui::tui::{Tui, UiLayout};
use crate::tmplayer::utils::input::{Action, map_key, map_mouse};
use crate::tmplayer::{
    HostPlaybackBridge, HostPlaybackRuntimeSnapshot, HostPlaybackSnapshot, HostPlaybackState,
    HostRepeatMode,
};
use anyhow::Result;
use crossterm::event::{self, Event};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};
use crate::render::frame_clock::FrameClock;

/// 子页的上一级：挂在设置弹窗下面的这些弹窗，Esc 应该回到设置弹窗，
/// 而不是直接关掉整个弹窗。`None` 表示没有上一级。
fn settings_parent(overlay: Overlay) -> Option<Overlay> {
    matches!(
        overlay,
        Overlay::BarSettingsModal
            | Overlay::LyricsSettingsModal
            | Overlay::DownloadSettingsModal
            | Overlay::DownloadPathEditModal
            | Overlay::HelpModal
            | Overlay::AboutModal
    )
    .then_some(Overlay::SettingsModal)
}

fn clear_spectrum(app: &mut AppState) {
    app.spectrum.bars.fill(0.0);
    app.spectrum.bars_left.fill(0.0);
    app.spectrum.bars_right.fill(0.0);
    app.spectrum_bar_smoother.reset();
    app.spectrum_left_smoother.reset();
    app.spectrum_right_smoother.reset();
}

fn has_spectrum_data(app: &AppState) -> bool {
    app.spectrum.bars.iter().any(|&v| v > 0.0)
        || app.spectrum.bars_left.iter().any(|&v| v > 0.0)
        || app.spectrum.bars_right.iter().any(|&v| v > 0.0)
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

async fn apply_host_config_sync(app: &mut AppState, config: Config, vip_audio_unlocked: bool) {
    let theme_changed = app.config.theme != config.theme;
    app.config = config;
    app.language = app.config.language;
    app.vip_audio_unlocked = vip_audio_unlocked;
    app.config.audio_quality = app.config.audio_quality.clamp_for_vip(vip_audio_unlocked);
    app.config.download_audio_quality = app
        .config
        .download_audio_quality
        .clamp_for_vip(vip_audio_unlocked);
    app.eq.bands_db = app.config.eq_bands_db;
    if theme_changed {
        app.theme = ThemeLoader::load_async(&app.config.theme)
            .await
            .unwrap_or_default();
    }
    app.refresh_download_root();
}

async fn save_and_sync_host_config(app: &mut AppState, host_bridge: &mut impl HostPlaybackBridge) {
    host_bridge.apply_config_sync(app.config.clone()).await;
    let accepted = host_bridge.config_snapshot();
    apply_host_config_sync(app, accepted, host_bridge.vip_audio_unlocked()).await;
}

async fn sync_eq_config(app: &mut AppState, host_bridge: &mut impl HostPlaybackBridge) {
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
        lyrics: None,
    }
}
fn hash_cover_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn sync_from_host_snapshot(app: &mut AppState, snapshot: HostPlaybackSnapshot) {
    let previous_index = app.playlist.current;
    let queue_len = snapshot.playlist.len();

    if queue_len == 0 {
        app.playlist_cover = None;
        app.playlist_cover_hash = None;
        clear_spectrum(app);
        app.cover_anim = None;
        app.pending_system_cover_anim = None;
        app.playlist = crate::tmplayer::data::playlist::Playlist::default();
        app.playlist_view = crate::tmplayer::data::playlist::Playlist::default();
        app.player.playback = map_host_state(snapshot.state);
        app.player.repeat_mode = map_host_repeat(snapshot.repeat_mode);
        app.player.liked = false;
        app.player.position = snapshot.position;
        app.player.track = empty_track_metadata();
        return;
    }

    let mut playlist = crate::tmplayer::data::playlist::Playlist::default();
    let mut tracks = Vec::with_capacity(queue_len);

    for (idx, item) in snapshot.playlist.iter().enumerate() {
        let title = if item.title.trim().is_empty() {
            format!("Track {}", idx + 1)
        } else {
            item.title.clone()
        };

        playlist
            .items
            .push(crate::tmplayer::data::playlist::PlaylistItem {
                song_id: item.id.clone(),
                title,
            });

        tracks.push(crate::tmplayer::app::state::TrackMetadata {
            title: item.title.clone(),
            artist: item.artist.clone(),
            album: item.album.clone(),
            duration: item.duration,
            cover: None,
            cover_hash: None,
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

    let keep_selected = app.overlay == Overlay::Playlist
        && app.playlist_view.len() <= queue_len
        && app
            .playlist_view
            .items
            .iter()
            .zip(&playlist.items)
            .all(|(old, new)| old.song_id == new.song_id);
    let view_selected = if keep_selected {
        app.playlist_view.selected.min(queue_len.saturating_sub(1))
    } else {
        current
    };

    let mut view = playlist.clone();
    view.selected = view_selected;
    view.clamp_selected();
    let identity_changed = previous_index
        .and_then(|index| app.playlist.items.get(index))
        .and_then(|item| item.song_id.as_deref())
        != playlist
            .items
            .get(current)
            .and_then(|item| item.song_id.as_deref());

    app.playlist_cover = snapshot.playlist_cover.clone();
    app.playlist_cover_hash = app.playlist_cover.as_deref().map(hash_cover_bytes);
    app.api_tracks = tracks;
    app.playlist = playlist;
    app.playlist_view = view;

    app.player.playback = map_host_state(snapshot.state);
    app.player.repeat_mode = map_host_repeat(snapshot.repeat_mode);
    app.player.liked = snapshot.current_liked;
    app.player.position = snapshot.position;
    let track_changed = identity_changed
        || previous_index != Some(current)
        || app.player.track.title != current_track.title
        || app.player.track.artist != current_track.artist
        || app.player.track.album != current_track.album;
    if track_changed {
        clear_spectrum(app);
        if previous_index.is_some() {
            let (from, dir, _) = app.pending_system_cover_anim.take().unwrap_or_else(|| {
                (
                    crate::tmplayer::app::state::CoverSnapshot::from(&app.player.track),
                    -1,
                    Instant::now(),
                )
            });
            app.start_cover_anim(
                from,
                crate::tmplayer::app::state::CoverSnapshot::from(&current_track),
                dir,
                Instant::now(),
            );
        }
    }
    if let Some(anim) = app.cover_anim.as_mut()
        && anim.to.cover_hash != current_track.cover_hash
    {
        anim.to.cover.clone_from(&current_track.cover);
        anim.to.cover_hash = current_track.cover_hash;
    }
    app.player.track = current_track;
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

    if app.player.seeking != runtime.seeking {
        app.player.seeking = runtime.seeking;
        changed = true;
    }

    if app.download_state != runtime.download {
        app.download_state = runtime.download;
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

    if let Some(index) = runtime.current_index
        && !app.playlist.items.is_empty()
    {
        let idx = index.min(app.playlist.len().saturating_sub(1));
        if app.playlist.current != Some(idx) {
            app.playlist.current = Some(idx);
            app.playlist.selected = idx;
            app.playlist.clamp_selected();
            changed = true;
        }
    }

    changed
}

async fn sync_from_host_bridge(
    app: &mut AppState,
    host_bridge: &mut impl HostPlaybackBridge,
    last_metadata_signature: &mut Option<u64>,
    last_config_signature: &mut Option<u64>,
) -> bool {
    let mut changed = false;
    host_bridge.tick().await;

    let config_signature = host_bridge.config_signature();
    if last_config_signature.is_none_or(|sig| sig != config_signature) {
        let config = host_bridge.config_snapshot();
        apply_host_config_sync(app, config, host_bridge.vip_audio_unlocked()).await;
        *last_config_signature = Some(config_signature);
        changed = true;
    }

    let runtime = host_bridge.runtime_snapshot();
    changed |= apply_host_runtime_snapshot(app, runtime);

    let metadata_signature = host_bridge.metadata_signature();
    if last_metadata_signature.is_none_or(|sig| sig != metadata_signature) {
        let snapshot = host_bridge.snapshot();
        sync_from_host_snapshot(app, snapshot);
        *last_metadata_signature = Some(metadata_signature);
        changed = true;
    }

    changed
}

fn tick_visual_state(app: &mut AppState, now: Instant) -> bool {
    let scope_before = app.scope_gain.value();
    app.tick(now);
    scope_before != app.scope_gain.value() || app.should_continuous_redraw()
}

pub async fn run(
    app: &mut AppState,
    host_bridge: &mut impl HostPlaybackBridge,
) -> Result<crate::tmplayer::FullscreenExit> {
    enable_raw_mode()?;
    let mut tui = Tui::new(host_bridge.wake_signal())?;
    tui.enter()?;

    // Prefer cava for system-wide visualization (keeps our renderer/style; cava only provides bars).
    // If cava isn't installed, we leave the spectrum empty.
    let cava = CavaService::new();

    let mut last_spectrum = Instant::now();
    let mut last_host_metadata_signature: Option<u64> = None;
    let mut last_host_config_signature: Option<u64> = None;
    let mut last_host_sync = Instant::now() - Duration::from_millis(50);
    let wake = host_bridge.wake_signal();
    let mut clock = FrameClock::new(app.idle_render_fps(), Instant::now());

    let mut last_layout = UiLayout::default();

    // 示波器始终读取宿主播放链路上的 PCM 抽头环。
    app.pcm_ring = Some(host_bridge.pcm_ring());

    let desired = desired_cava_config(app, &last_layout);
    cava.set_desired(desired);
    let mut cava_cfg = desired;
    let mut last_cava_failure: Option<String> = None;

    let _ = sync_from_host_bridge(
        app,
        host_bridge,
        &mut last_host_metadata_signature,
        &mut last_host_config_signature,
    )
    .await;

    let loop_result: Result<()> = async {
        loop {
            let frame_start = Instant::now();
            let mut state_changed = false;
            if wake.take() || last_host_sync.elapsed() >= Duration::from_millis(50) {
                state_changed |= sync_from_host_bridge(app, host_bridge, &mut last_host_metadata_signature, &mut last_host_config_signature).await;
                last_host_sync = frame_start;
            }
            while event::poll(Duration::ZERO)? {
                match event::read()? {
                    Event::Key(k) => {
                        let action = map_key(k, app.overlay, &app.config);
                        handle_action(app, host_bridge, action, &last_layout).await?;
                        state_changed = true;
                    }
                    Event::Mouse(m) => {
                        let action = map_mouse(m);
                        handle_action(app, host_bridge, action, &last_layout).await?;
                        state_changed = true;
                    }
                    Event::Resize(_, _) => state_changed = true,
                    _ => {}
                }
            }
            let desired = desired_cava_config(app, &last_layout);
            if cava_cfg != desired {
                cava.set_desired(desired);
                cava_cfg = desired;
                clear_spectrum(app);
                last_cava_failure = None;
                state_changed = true;
            }
            {
                let failure = cava.failure();
                if *failure != last_cava_failure {
                    if let Some(error) = failure.as_ref() {
                        log::warn!("Fullscreen visualization unavailable: {error}");
                        app.set_toast(format!("Visualization unavailable: {error}"));
                        state_changed = true;
                    }
                    last_cava_failure.clone_from(&failure);
                }
            }
            if app.config.visualize == VisualizeMode::Bars {
                ensure_bar_buffers(app, desired_bar_count(app, &last_layout));
            }
            if app.config.visualize.needs_cava() {
                let period = Duration::from_millis((1000 / app.config.spectrum_hz.max(1)) as u64);
                if frame_start.duration_since(last_spectrum) >= period {
                    last_spectrum = frame_start;
                    state_changed = true;
                    let snapshot = cava.latest();
                    let bars = desired_bar_count(app, &last_layout);
                    ensure_bar_buffers(app, bars);
                    let mut left = [0.0; crate::tmplayer::audio::cava::MAX_BARS];
                    let mut right = [0.0; crate::tmplayer::audio::cava::MAX_BARS];
                    let mut mono = [0.0; crate::tmplayer::audio::cava::MAX_BARS];
                    if app.config.bar_channels == BarChannels::Stereo {
                        let _ = snapshot.copy_stereo_into(&mut left, &mut right);
                        app.spectrum_left_smoother.apply_in_place(&left[..bars], &mut app.spectrum.bars_left);
                        app.spectrum_right_smoother.apply_in_place(&right[..bars], &mut app.spectrum.bars_right);
                    } else {
                        app.spectrum.bars_left.fill(0.0);
                        app.spectrum.bars_right.fill(0.0);
                    }
                    let _ = snapshot.mono_into(&mut mono);
                    app.spectrum_bar_smoother.apply_in_place(&mono[..bars], &mut app.spectrum.bars);
                }
            } else if has_spectrum_data(app) {
                clear_spectrum(app);
                state_changed = true;
            }
            state_changed |= tick_visual_state(app, frame_start);
            state_changed |= tui.poll_cover_frames();
            if state_changed || app.should_continuous_redraw() {
                clock.mark_dirty();
            }
            let target_fps = if app.should_continuous_redraw() { app.active_render_fps() } else { app.idle_render_fps() };
            clock.set_fps(target_fps, frame_start);
            if clock.due(frame_start) {
                last_layout = tui.draw(app)?;
                clock.presented(Instant::now());
            }
            let maintenance_wait = Duration::from_millis(50).saturating_sub(last_host_sync.elapsed());
            let frame_wait = clock.next_deadline().map(|deadline| deadline.saturating_duration_since(Instant::now())).unwrap_or(Duration::from_secs(1));
            let wait = maintenance_wait.min(frame_wait);
            if !wait.is_zero() {
                compio::time::sleep(wait).await;
            }
            if tui.should_quit { break; }
        }
        Ok(())
    }
    .await;

    let exit_result = tui.exit();
    let raw_result = disable_raw_mode();
    let cava_result = compio::runtime::spawn_blocking(move || cava.shutdown_blocking())
        .await
        .map_err(|_| anyhow::anyhow!("cava shutdown task panicked"));
    loop_result?;
    exit_result?;
    raw_result?;
    cava_result?;

    let exit = match app.exit_request {
        Some(exit) => exit,
        None if app.request_host_settings_open => {
            crate::tmplayer::FullscreenExit::BackToHostOpenSettings
        }
        None => crate::tmplayer::FullscreenExit::BackToHost,
    };
    Ok(exit)
}

async fn handle_action(
    app: &mut AppState,
    host_bridge: &mut impl HostPlaybackBridge,
    action: Action,
    layout: &UiLayout,
) -> Result<()> {
    match action {
        Action::Quit => {
            // handled by tui flag
            app.set_toast("Bye");
        }
        Action::OpenSettingsModal => {
            app.settings_selected = app.settings_selected.min(12);
            app.overlay = Overlay::SettingsModal;
        }
        Action::OpenHelpModal => {
            app.help_keybind_selected = app
                .help_keybind_selected
                .min(crate::tmplayer::ui::tui::help_item_count(app).saturating_sub(1));
            app.help_keybind_scroll = 0;
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
        Action::PathChar(c) => {
            if app.overlay == Overlay::DownloadPathEditModal {
                download_path_edit_insert(app, c);
            }
        }
        Action::PathBackspace => {
            if app.overlay == Overlay::DownloadPathEditModal {
                download_path_edit_backspace(app);
            }
        }
        Action::CloseOverlay => {
            if app.overlay == Overlay::Playlist {
                // 面板状态立即关闭，滑出动画由 tick 推进到位。
                app.start_playlist_slide(-(layout.left_width as i16));
                app.overlay = Overlay::None;
            } else if app.overlay == Overlay::DownloadPathEditModal {
                // 路径编辑中的 Esc = 取消编辑（丢弃输入），回下载设置页。
                app.download_path_edit = None;
                app.overlay = Overlay::DownloadSettingsModal;
            } else if let Some(parent) = settings_parent(app.overlay) {
                // 设置类子页：回上一级，而不是直接退出全屏页。
                app.overlay = parent;
            } else {
                app.close_overlay();
            }
        }
        Action::TogglePlaylist => {
            if app.overlay == Overlay::Playlist {
                app.start_playlist_slide(-(layout.left_width as i16));
                app.overlay = Overlay::None;
            } else {
                // 需求：打开 playlist 时聚焦当前播放的歌曲。
                app.playlist_view = app.playlist.clone();
                if let Some(cur) = app.playlist.current {
                    app.playlist_view.selected = cur;
                    app.playlist_view.clamp_selected();
                }

                app.overlay = Overlay::Playlist;
                if app.playlist_slide_x == app.playlist_slide_target_x {
                    app.playlist_slide_x = -(layout.left_width as i16);
                }
                app.start_playlist_slide(0);
            }
        }
        Action::Confirm => match app.overlay {
            Overlay::Playlist => {
                let idx = app
                    .playlist_view
                    .selected
                    .min(app.playlist_view.len().saturating_sub(1));
                host_bridge.play_queue_index(idx).await;
                let snapshot = host_bridge.snapshot();
                sync_from_host_snapshot(app, snapshot);
                return Ok(());
            }
            Overlay::SettingsModal => match app.settings_selected {
                0..=3 => {
                    apply_settings_delta(app, host_bridge, 1).await;
                }
                4 => {
                    app.bar_settings_selected = 0;
                    app.overlay = Overlay::BarSettingsModal;
                }
                5 => {
                    app.help_keybind_scroll = 0;
                    app.overlay = Overlay::HelpModal;
                }
                6 => {
                    app.lyrics_settings_selected = app.lyrics_settings_selected.min(2);
                    app.overlay = Overlay::LyricsSettingsModal;
                }
                7 => {
                    apply_settings_delta(app, host_bridge, 1).await;
                }
                8 => {
                    apply_settings_delta(app, host_bridge, 1).await;
                }
                9 => {
                    apply_settings_delta(app, host_bridge, 1).await;
                }
                10 => {
                    app.download_settings_selected =
                        download_selectable_rows(app).first().copied().unwrap_or(1);
                    app.download_reset_armed = false;
                    app.download_path_edit = None;
                    app.overlay = Overlay::DownloadSettingsModal;
                }
                11 => {
                    app.set_toast("Logout is unavailable in fullscreen");
                }
                12 => {
                    app.overlay = Overlay::AboutModal;
                }
                _ => {}
            },
            Overlay::DownloadSettingsModal => {
                activate_download_settings_item(app, host_bridge).await;
            }
            Overlay::DownloadPathEditModal => {
                commit_download_path_edit(app, host_bridge).await;
            }
            Overlay::BarSettingsModal => match app.bar_settings_selected {
                0 => {
                    app.config.visualize = app.config.visualize.cycle(1);
                    save_and_sync_host_config(app, host_bridge).await;
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
                    app.config.audio_quality =
                        app.config.audio_quality.cycle(1, app.vip_audio_unlocked);
                    save_and_sync_host_config(app, host_bridge).await;
                }
                7 => {
                    app.config.playback_memory = !app.config.playback_memory;
                    save_and_sync_host_config(app, host_bridge).await;
                }
                _ => {}
            },
            Overlay::LyricsSettingsModal => match app.lyrics_settings_selected {
                0 => {
                    app.config.page_lyrics = !app.config.page_lyrics;
                    save_and_sync_host_config(app, host_bridge).await;
                }
                1 => {
                    app.config.page_lyrics_drag = !app.config.page_lyrics_drag;
                    save_and_sync_host_config(app, host_bridge).await;
                }
                2
                    // 拖动关闭时吸附无意义：灰置且不可改。
                    if app.config.page_lyrics_drag => {
                        app.config.page_lyrics_snap = !app.config.page_lyrics_snap;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                _ => {}
            },
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
        }
        Action::PlaylistDown => {
            app.playlist_view.move_down();
            app.playlist_view.clamp_selected();
            if app.playlist_view.selected + 1 >= app.playlist_view.len() {
                host_bridge.request_queue_page();
            }
        }
        Action::ModalUp => {
            if app.overlay == Overlay::SettingsModal {
                let count = 13;
                if app.settings_selected == 0 {
                    app.settings_selected = count - 1;
                } else {
                    app.settings_selected -= 1;
                }
            } else if app.overlay == Overlay::DownloadSettingsModal {
                move_download_selection(app, -1);
            } else if app.overlay == Overlay::BarSettingsModal {
                let count = 8;
                if app.bar_settings_selected == 0 {
                    app.bar_settings_selected = count - 1;
                } else {
                    app.bar_settings_selected -= 1;
                }
            } else if app.overlay == Overlay::LyricsSettingsModal {
                let count = 3;
                if app.lyrics_settings_selected == 0 {
                    app.lyrics_settings_selected = count - 1;
                } else {
                    app.lyrics_settings_selected -= 1;
                }
            } else if app.overlay == Overlay::EqModal {
                let step = 1.0;
                if app.eq_selected < crate::tmplayer::app::state::EQ_BANDS {
                    let v = app.eq.bands_db[app.eq_selected];
                    app.eq.bands_db[app.eq_selected] = (v + step).clamp(-12.0, 12.0);
                }
                sync_eq_config(app, host_bridge).await;
            } else if app.overlay == Overlay::HelpModal {
                let count = crate::tmplayer::ui::tui::help_item_count(app);
                if count == 0 {
                    return Ok(());
                }
                if app.help_keybind_selected == 0 {
                    app.help_keybind_selected = count - 1;
                } else {
                    app.help_keybind_selected -= 1;
                }
            }
        }
        Action::ModalDown => {
            if app.overlay == Overlay::SettingsModal {
                let count = 13;
                app.settings_selected = (app.settings_selected + 1) % count;
            } else if app.overlay == Overlay::DownloadSettingsModal {
                move_download_selection(app, 1);
            } else if app.overlay == Overlay::BarSettingsModal {
                let count = 8;
                app.bar_settings_selected = (app.bar_settings_selected + 1) % count;
            } else if app.overlay == Overlay::LyricsSettingsModal {
                let count = 3;
                app.lyrics_settings_selected = (app.lyrics_settings_selected + 1) % count;
            } else if app.overlay == Overlay::EqModal {
                let step = 1.0;
                if app.eq_selected < crate::tmplayer::app::state::EQ_BANDS {
                    let v = app.eq.bands_db[app.eq_selected];
                    app.eq.bands_db[app.eq_selected] = (v - step).clamp(-12.0, 12.0);
                }
                sync_eq_config(app, host_bridge).await;
            } else if app.overlay == Overlay::HelpModal {
                let count = crate::tmplayer::ui::tui::help_item_count(app);
                if count > 0 {
                    app.help_keybind_selected = (app.help_keybind_selected + 1) % count;
                }
            }
        }
        Action::ModalLeft => {
            if app.overlay == Overlay::SettingsModal {
                apply_settings_delta(app, host_bridge, -1).await;
            } else if app.overlay == Overlay::DownloadSettingsModal {
                apply_download_settings_delta(app, host_bridge, -1).await;
            } else if app.overlay == Overlay::DownloadPathEditModal {
                download_path_edit_move(app, -1);
            } else if app.overlay == Overlay::BarSettingsModal {
                match app.bar_settings_selected {
                    0 => {
                        app.config.visualize = app.config.visualize.cycle(-1);
                        save_and_sync_host_config(app, host_bridge).await;
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
                        app.config.audio_quality =
                            app.config.audio_quality.cycle(-1, app.vip_audio_unlocked);
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    7 => {
                        app.config.playback_memory = !app.config.playback_memory;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    _ => {}
                }
            } else if app.overlay == Overlay::LyricsSettingsModal {
                apply_lyrics_settings_delta(app, host_bridge, -1).await;
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
            } else if app.overlay == Overlay::DownloadSettingsModal {
                apply_download_settings_delta(app, host_bridge, 1).await;
            } else if app.overlay == Overlay::DownloadPathEditModal {
                download_path_edit_move(app, 1);
            } else if app.overlay == Overlay::BarSettingsModal {
                match app.bar_settings_selected {
                    0 => {
                        app.config.visualize = app.config.visualize.cycle(1);
                        save_and_sync_host_config(app, host_bridge).await;
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
                        app.config.audio_quality =
                            app.config.audio_quality.cycle(1, app.vip_audio_unlocked);
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    7 => {
                        app.config.playback_memory = !app.config.playback_memory;
                        save_and_sync_host_config(app, host_bridge).await;
                    }
                    _ => {}
                }
            } else if app.overlay == Overlay::LyricsSettingsModal {
                apply_lyrics_settings_delta(app, host_bridge, 1).await;
            } else if app.overlay == Overlay::EqModal {
                let count = crate::tmplayer::app::state::EQ_BANDS;
                app.eq_selected = (app.eq_selected + 1) % count;
            }
        }
        Action::PlaylistSelect(idx) => {
            if idx < app.playlist_view.len() {
                app.playlist_view.selected = idx;
                app.playlist_view.clamp_selected();
                if idx + 1 >= app.playlist_view.len() {
                    host_bridge.request_queue_page();
                }
                let now = Instant::now();
                let is_double = app.last_playlist_click.is_some_and(|(at, last)| {
                    now.duration_since(at) <= Duration::from_millis(400) && last == idx
                });
                app.last_playlist_click = Some((now, idx));
                if is_double {
                    host_bridge.play_queue_index(idx).await;
                    sync_from_host_snapshot(app, host_bridge.snapshot());
                }
            }
        }
        Action::TogglePlayPause => {
            host_bridge.toggle_play_pause().await;
            sync_from_host_snapshot(app, host_bridge.snapshot());
        }
        Action::Prev => {
            app.pending_system_cover_anim = Some((
                crate::tmplayer::app::state::CoverSnapshot::from(&app.player.track),
                1,
                Instant::now(),
            ));
            host_bridge.play_previous().await;
            sync_from_host_snapshot(app, host_bridge.snapshot());
        }
        Action::Next => {
            app.pending_system_cover_anim = Some((
                crate::tmplayer::app::state::CoverSnapshot::from(&app.player.track),
                -1,
                Instant::now(),
            ));
            host_bridge.play_next().await;
            sync_from_host_snapshot(app, host_bridge.snapshot());
        }
        Action::VolumeUp => {
            let next = (app.player.volume + 0.05).min(1.0);
            host_bridge.set_volume(next);
            app.player.volume = next;
        }
        Action::VolumeDown => {
            let next = (app.player.volume - 0.05).max(0.0);
            host_bridge.set_volume(next);
            app.player.volume = next;
        }
        Action::SetVolume(v) => {
            app.volume_drag = true;
            let next = v.clamp(0.0, 1.0);
            host_bridge.set_volume(next);
            app.player.volume = next;
        }
        Action::ToggleRepeatMode => {
            host_bridge.toggle_repeat_mode();
            sync_from_host_snapshot(app, host_bridge.snapshot());
        }
        Action::ToggleFavorite => {
            host_bridge.toggle_like_current().await;
            sync_from_host_snapshot(app, host_bridge.snapshot());
        }
        Action::ToggleDownload => {
            host_bridge.download_current();
            apply_host_runtime_snapshot(app, host_bridge.runtime_snapshot());
        }
        Action::OpenAuthorPage(index) => {
            app.exit_request = Some(crate::tmplayer::FullscreenExit::BackToHostOpenAuthor(index));
        }
        Action::OpenAlbumPage => {
            app.exit_request = Some(crate::tmplayer::FullscreenExit::BackToHostOpenAlbum);
        }
        Action::SeekToFraction(r) => {
            host_bridge.seek_to_ratio(r);
            sync_from_host_snapshot(app, host_bridge.snapshot());
        }
        Action::MouseClick { col, row } => {
            // map click to controls/progress/volume/playlist
            if let Some(a) = crate::tmplayer::ui::tui::hit_test(layout, app, col, row) {
                Box::pin(handle_action(app, host_bridge, a, layout)).await?;
            }
        }
        Action::MouseScroll { col, row, forward } => {
            // 弹窗（设置/播放设置/歌词浮窗/本地音频/按键提示/EQ）打开时：
            // 滚轮切换聚焦行，与 Up/Down 同效。
            if app.overlay != Overlay::None && app.overlay != Overlay::Playlist {
                let action = if forward {
                    Action::ModalDown
                } else {
                    Action::ModalUp
                };
                Box::pin(handle_action(app, host_bridge, action, layout)).await?;
                return Ok(());
            }

            // 侧边栏（播放列表面板）：滚轮滚动聚焦。
            if crate::tmplayer::ui::tui::wheel_over_playlist(layout, app, col, row) {
                let action = if forward {
                    Action::PlaylistDown
                } else {
                    Action::PlaylistUp
                };
                Box::pin(handle_action(app, host_bridge, action, layout)).await?;
            }
        }
        Action::MouseDrag { col, .. } => {
            if app.volume_drag
                && let Some(volume) = crate::tmplayer::ui::tui::volume_for_drag(layout, col)
            {
                app.player.volume = volume;
                host_bridge.set_volume(volume);
            }
        }
        Action::MouseUp => {
            app.volume_drag = false;
        }
        Action::ModalSelect(idx) => {
            let Some(rect) = layout.modal_rows.get(idx) else {
                return Ok(());
            };

            match app.overlay {
                Overlay::SettingsModal => app.settings_selected = idx,
                Overlay::BarSettingsModal => app.bar_settings_selected = idx,
                Overlay::LyricsSettingsModal => {
                    // 开关行：左键直接改值（与宿主一致），不走双击。
                    app.lyrics_settings_selected = idx.min(2);
                    apply_lyrics_settings_delta(app, host_bridge, 1).await;
                    return Ok(());
                }
                Overlay::DownloadSettingsModal => {
                    // 与歌词浮窗同构：单击即执行（音质改值 / 路径进编辑 / 恢复默认两段式）。
                    app.download_settings_selected = idx.min(2);
                    activate_download_settings_item(app, host_bridge).await;
                    return Ok(());
                }
                Overlay::HelpModal => app.help_keybind_selected = idx,
                _ => return Ok(()),
            }

            // 同一行 400ms 内再点一次 = Enter（与播放列表双击同款判定）。
            // 列归一化到行首，行内任意位置都算同一个目标。
            let now = Instant::now();
            let is_double = app.last_mouse_click.is_some_and(|(at, col, row)| {
                now.duration_since(at) <= Duration::from_millis(400)
                    && (col, row) == (rect.x, rect.y)
            });
            app.last_mouse_click = Some((now, rect.x, rect.y));
            if is_double {
                return Box::pin(handle_action(app, host_bridge, Action::Confirm, layout)).await;
            }
        }
        Action::None => {}
    }

    Ok(())
}

async fn apply_settings_delta(
    app: &mut AppState,
    host_bridge: &mut impl HostPlaybackBridge,
    delta: i32,
) {
    match app.settings_selected {
        // Theme
        0 => {
            let themes = ThemeLoader::list_themes_async().await;
            let cur = themes
                .iter()
                .position(|key| key.eq_ignore_ascii_case(app.config.theme.as_str()))
                .unwrap_or(0) as i32;
            let next = (cur + delta).rem_euclid(themes.len() as i32) as usize;
            let key = &themes[next];
            if let Ok(theme) = ThemeLoader::load_async(key).await {
                app.theme = theme;
                app.config.theme = key.clone();
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
        // Color halfblocks / ASCII cover display
        3 => {
            if delta != 0 {
                app.config.graphics_protocol = app.config.graphics_protocol.cycle(delta);
                save_and_sync_host_config(app, host_bridge).await;
            }
        }
        // “歌词浮窗...”是可进入项：左右键不改变配置。
        6 => {}
        // Show hints
        7 => {
            if delta != 0 {
                app.config.show_hints = !app.config.show_hints;
                save_and_sync_host_config(app, host_bridge).await;
            }
        }
        // Small window display
        8 => {
            if delta != 0 {
                app.config.small_window_display = !app.config.small_window_display;
                save_and_sync_host_config(app, host_bridge).await;
            }
        }
        // Home more recommendations
        9 if delta != 0 => {
            app.config.home_more_recommend = !app.config.home_more_recommend;
            save_and_sync_host_config(app, host_bridge).await;
        }
        _ => {}
    }
}

/// 下载设置页的可选中行：下载不可用时只灰置「音质」——路径行是自救入口，
/// 「恢复默认」是把显式 `Null` / 无家目录状态拉回来的出口。
fn download_selectable_rows(app: &AppState) -> Vec<usize> {
    (0..3)
        .filter(|row| app.download_enabled() || *row != 0)
        .collect()
}

fn move_download_selection(app: &mut AppState, delta: i32) {
    let rows = download_selectable_rows(app);
    if rows.is_empty() || delta == 0 {
        return;
    }
    let current = rows
        .iter()
        .position(|row| *row == app.download_settings_selected)
        .unwrap_or(0) as i32;
    let next = (current + delta).rem_euclid(rows.len() as i32) as usize;
    app.download_settings_selected = rows[next];
    // 换行即撤下待确认态：恢复默认必须连着选两次同一个地方。
    app.download_reset_armed = false;
}

/// 下载设置页的「执行」：Enter / 左右键 / 双击共用。
async fn activate_download_settings_item(
    app: &mut AppState,
    host_bridge: &mut impl HostPlaybackBridge,
) {
    match app.download_settings_selected {
        0 => apply_download_settings_delta(app, host_bridge, 1).await,
        1 => begin_download_path_edit(app),
        2 => activate_download_reset(app, host_bridge).await,
        _ => {}
    }
}

/// 音质行：与播放设置同一套可选值（按会员放开）。
async fn apply_download_settings_delta(
    app: &mut AppState,
    host_bridge: &mut impl HostPlaybackBridge,
    delta: i32,
) {
    if delta == 0 || app.download_settings_selected != 0 || !app.download_enabled() {
        return;
    }
    let next = app
        .config
        .download_audio_quality
        .cycle(delta, app.vip_audio_unlocked);
    if next != app.config.download_audio_quality {
        app.config.download_audio_quality = next;
        save_and_sync_host_config(app, host_bridge).await;
    }
}

/// 「恢复默认」两段式：首次进入待确认态，再选一次才写回默认值。
///
/// 下载不可用（显式 `Null` / 宿主没有可写位置）时也允许：它就是那个出口。
async fn activate_download_reset(app: &mut AppState, host_bridge: &mut impl HostPlaybackBridge) {
    if !app.download_reset_armed {
        // 待确认态由行内文字（「确认恢复」+ 警戒色）表达，不再弹提示。
        app.download_reset_armed = true;
        return;
    }

    app.download_reset_armed = false;
    app.config.download_audio_quality = crate::data::config::default_download_audio_quality();
    app.config.download_path = None;
    save_and_sync_host_config(app, host_bridge).await;
    app.refresh_download_root();
}

/// 进入路径行的行内编辑（独立的 overlay，字符按键因此直接进输入框）。
fn begin_download_path_edit(app: &mut AppState) {
    let current = app
        .download_root
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| app.download_display_path());
    app.download_path_edit = Some(crate::app::DownloadPathEdit {
        cursor: current.chars().count(),
        buffer: current,
        window_col: 0,
    });
    app.overlay = Overlay::DownloadPathEditModal;
}

fn char_to_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

fn download_path_edit_insert(app: &mut AppState, ch: char) {
    let Some(edit) = app.download_path_edit.as_mut() else {
        return;
    };
    if edit.cursor >= 4096 {
        return;
    }
    let index = char_to_byte_index(&edit.buffer, edit.cursor);
    edit.buffer.insert(index, ch);
    edit.cursor += 1;
}

fn download_path_edit_backspace(app: &mut AppState) {
    let Some(edit) = app.download_path_edit.as_mut() else {
        return;
    };
    if edit.cursor == 0 {
        return;
    }
    let index = char_to_byte_index(&edit.buffer, edit.cursor - 1);
    edit.buffer.remove(index);
    edit.cursor -= 1;
}

fn download_path_edit_move(app: &mut AppState, delta: i32) {
    let Some(edit) = app.download_path_edit.as_mut() else {
        return;
    };
    let last = edit.buffer.chars().count() as i32;
    edit.cursor = (edit.cursor as i32 + delta).clamp(0, last) as usize;
}

/// 回车确认：非法（空 / 非绝对 / 不可写）就保留修改前的值，只弹一次 toast。
async fn commit_download_path_edit(app: &mut AppState, host_bridge: &mut impl HostPlaybackBridge) {
    let Some(edit) = app.download_path_edit.take() else {
        return;
    };
    let raw = edit.buffer.trim().to_string();
    app.overlay = Overlay::DownloadSettingsModal;

    match crate::app::download::validate_download_path(&raw).await {
        Ok(crate::app::download::DownloadPathChoice::Disabled) => {
            app.config.download_path = Some(crate::app::download::DOWNLOAD_PATH_NULL.to_string());
            save_and_sync_host_config(app, host_bridge).await;
            app.refresh_download_root();
        }
        Ok(crate::app::download::DownloadPathChoice::Dir(path)) => {
            app.config.download_path = Some(path.display().to_string());
            save_and_sync_host_config(app, host_bridge).await;
            app.refresh_download_root();
        }
        Err(_) => {}
    }
}

/// “歌词浮窗”子页三行开关；吸附行只在拖动开启时可改。
async fn apply_lyrics_settings_delta(
    app: &mut AppState,
    host_bridge: &mut impl HostPlaybackBridge,
    delta: i32,
) {
    if delta == 0 {
        return;
    }

    match app.lyrics_settings_selected {
        0 => {
            app.config.page_lyrics = !app.config.page_lyrics;
            save_and_sync_host_config(app, host_bridge).await;
        }
        1 => {
            app.config.page_lyrics_drag = !app.config.page_lyrics_drag;
            save_and_sync_host_config(app, host_bridge).await;
        }
        2 if app.config.page_lyrics_drag => {
            app.config.page_lyrics_snap = !app.config.page_lyrics_snap;
            save_and_sync_host_config(app, host_bridge).await;
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
    if !app.config.visualize.needs_cava() {
        return None;
    }
    Some(CavaConfig {
        framerate_hz: app.config.spectrum_hz,
        bars: desired_bar_count(app, layout),
        channels: match app.config.bar_channels {
            BarChannels::Mono => CavaChannels::Mono,
            BarChannels::Stereo => CavaChannels::Stereo,
        },
        reverse: false,
    })
}

fn ensure_bar_buffers(app: &mut AppState, bars: usize) {
    if app.spectrum.bars.len() != bars {
        app.spectrum.bars.resize(bars, 0.0);
        app.spectrum.bars_left.resize(bars, 0.0);
        app.spectrum.bars_right.resize(bars, 0.0);
        clear_spectrum(app);
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_oscilloscope_schedules_the_exact_flat_frame_without_input() {
        let config = Config {
            visualize: VisualizeMode::Oscilloscope,
            ..Config::default()
        };
        let mut app = AppState::new(
            config,
            crate::ui::theme::Theme::default(),
            crate::data::config::Language::Zh,
        );
        let start = Instant::now();
        app.last_frame = start;
        app.player.playback = PlaybackState::Playing;
        tick_visual_state(&mut app, start + Duration::from_secs(1));
        assert_eq!(app.scope_gain.value(), 1.0);
        app.player.playback = PlaybackState::Paused;
        let mut settled = None;
        for frame in 1..=120 {
            let before = app.scope_gain.value();
            let now = start + Duration::from_secs(1) + Duration::from_millis(frame * 16);
            let dirty = tick_visual_state(&mut app, now);
            if before > 0.0 && app.scope_gain.value() == 0.0 {
                assert!(
                    dirty,
                    "the final flat frame must be painted even when animation stops"
                );
                assert!(
                    !app.should_continuous_redraw(),
                    "the final frame does not require permanent animation"
                );
                settled = Some(now);
                break;
            }
        }
        let settled = settled.expect("pause reaches exact zero without an input event");
        assert!(!tick_visual_state(
            &mut app,
            settled + Duration::from_millis(16)
        ));
    }

    /// 挂在设置弹窗下面的子页必须全部登记进 `settings_parent`：
    /// 漏一个，那个页面按 Esc 就会直接退出全屏页（按键提示弹窗就这么漏过）。
    #[test]
    fn every_settings_child_returns_to_the_settings_modal() {
        for child in [
            Overlay::BarSettingsModal,
            Overlay::LyricsSettingsModal,
            Overlay::HelpModal,
            Overlay::AboutModal,
        ] {
            assert_eq!(
                settings_parent(child),
                Some(Overlay::SettingsModal),
                "{child:?} 应该回设置弹窗"
            );
        }

        // 设置弹窗本身与 EQ/播放列表没有上一级。
        assert_eq!(settings_parent(Overlay::SettingsModal), None);
        assert_eq!(settings_parent(Overlay::EqModal), None);
        assert_eq!(settings_parent(Overlay::Playlist), None);
        assert_eq!(settings_parent(Overlay::None), None);
    }

    #[test]
    fn host_song_identity_starts_cover_slide_and_resets_smoothing() {
        let mut app = AppState::new(
            Config::default(),
            crate::ui::theme::Theme::default(),
            crate::data::config::Language::Zh,
        );
        let snapshot = |id: &str| HostPlaybackSnapshot {
            playlist: vec![crate::tmplayer::FullscreenPlaylistItemSeed {
                id: Some(id.to_string()),
                title: "Same title".to_string(),
                artist: "Same artist".to_string(),
                album: "Same album".to_string(),
                duration: Duration::from_secs(60),
            }],
            current_index: Some(0),
            ..HostPlaybackSnapshot::default()
        };
        sync_from_host_snapshot(&mut app, snapshot("first"));
        assert!(
            app.cover_anim.is_none(),
            "initial sync is not a track transition"
        );
        app.spectrum.bars.fill(1.0);
        let mut output = [0.0];
        app.spectrum_bar_smoother
            .apply_in_place(&[1.0], &mut output);
        let now = Instant::now();
        app.pending_system_cover_anim = Some((
            crate::tmplayer::app::state::CoverSnapshot::from(&app.player.track),
            1,
            now,
        ));
        sync_from_host_snapshot(&mut app, snapshot("second"));
        assert_eq!(app.playlist.items[0].song_id.as_deref(), Some("second"));
        assert_eq!(app.cover_anim.as_ref().unwrap().dir, 1);
        assert!(app.pending_system_cover_anim.is_none());
        assert!(app.spectrum.bars.iter().all(|value| *value == 0.0));
        app.spectrum_bar_smoother
            .apply_in_place(&[1.0], &mut output);
        assert_eq!(output, [0.35]);
        sync_from_host_snapshot(&mut app, HostPlaybackSnapshot::default());
        assert!(app.cover_anim.is_none());
        assert!(app.playlist.items.is_empty());
    }
    #[test]
    fn appended_host_page_keeps_playlist_focus_on_the_boundary_row() {
        let mut app = AppState::new(
            Config::default(),
            crate::ui::theme::Theme::default(),
            crate::data::config::Language::Zh,
        );
        let snapshot = |ids: &[&str]| HostPlaybackSnapshot {
            playlist: ids
                .iter()
                .map(|id| crate::tmplayer::FullscreenPlaylistItemSeed {
                    id: Some(id.to_string()),
                    title: id.to_string(),
                    artist: String::new(),
                    album: String::new(),
                    duration: Duration::from_secs(60),
                })
                .collect(),
            current_index: Some(0),
            ..Default::default()
        };
        sync_from_host_snapshot(&mut app, snapshot(&["99", "100"]));
        app.overlay = Overlay::Playlist;
        app.playlist_view.selected = 1;
        sync_from_host_snapshot(&mut app, snapshot(&["99", "100", "101"]));
        assert_eq!(app.playlist_view.selected, 1);
        assert_eq!(app.playlist_view.items[2].song_id.as_deref(), Some("101"));
        sync_from_host_snapshot(&mut app, snapshot(&["new-a", "new-b", "new-c"]));
        assert_eq!(app.playlist_view.selected, 0, "替换来源时聚焦当前播放项");
    }
}
