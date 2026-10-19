use super::*;

#[test]
fn paused_theme_transition_keeps_redrawing_until_the_exact_target_frame() {
    use crate::ui::theme::{ColorCapability, Theme};
    let config = Config {
        visualize: VisualizeMode::Hidden,
        ..Config::default()
    };
    let mut theme = Theme::default();
    theme.capability = ColorCapability::TrueColor;
    let mut app = AppState::new(config, theme.clone(), crate::data::config::Language::En);
    assert!(!app.should_continuous_redraw());
    let mut next = theme.clone();
    next.palette.base = (200, 120, 60);
    app.apply_theme(next.clone());
    let now = Instant::now();
    assert!(app.should_continuous_redraw());
    app.tick(now + Duration::from_millis(100));
    assert_ne!(app.theme.palette.base, theme.palette.base);
    assert_ne!(app.theme.palette.base, next.palette.base);
    assert!(app.should_continuous_redraw());
    app.tick(now + Duration::from_millis(250));
    assert_eq!(app.theme.palette, next.palette);
    app.tick(now + Duration::from_millis(266));
    assert!(!app.should_continuous_redraw());

    next.capability = ColorCapability::NoColor;
    next.palette.base = (12, 34, 56);
    app.apply_theme(next.clone());
    assert_eq!(app.theme.palette, next.palette);
    assert!(!app.should_continuous_redraw());
}

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
    let ring = std::sync::Arc::new(crate::tmplayer::audio::pcm_tap::PcmRing::new());
    ring.push(&[0.5; 512], &[0.5; 512], 48_000);
    app.pcm_ring = Some(ring);
    let start = Instant::now();
    app.last_frame = start;
    app.player.playback = PlaybackState::Playing;
    tick_visual_state(&mut app, start);
    tick_visual_state(&mut app, start + Duration::from_millis(100));
    assert!(app.scope_gain.value() > 0.7);
    app.player.playback = PlaybackState::Paused;
    let mut settled = None;
    for frame in 1..=120 {
        let before = app.scope_gain.value();
        let now = start + Duration::from_millis(100 + frame * 16);
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

#[test]
fn missing_pcm_releases_scope_without_changing_playback_state() {
    let config = Config {
        visualize: VisualizeMode::Oscilloscope,
        ..Config::default()
    };
    let mut app = AppState::new(
        config,
        crate::ui::theme::Theme::default(),
        crate::data::config::Language::En,
    );
    let ring = std::sync::Arc::new(crate::tmplayer::audio::pcm_tap::PcmRing::new());
    ring.push(&[0.5; 512], &[-0.5; 512], 48_000);
    app.pcm_ring = Some(ring);
    app.player.playback = PlaybackState::Playing;
    let start = Instant::now();
    app.last_frame = start;
    tick_visual_state(&mut app, start);
    tick_visual_state(&mut app, start + Duration::from_millis(100));
    assert!(app.scope_gain.value() > 0.7, "fresh PCM opens the scope");
    tick_visual_state(&mut app, start + Duration::from_millis(160));
    let dirty = tick_visual_state(&mut app, start + Duration::from_millis(500));
    assert_eq!(app.player.playback, PlaybackState::Playing);
    assert_eq!(
        app.scope_gain.value(),
        0.0,
        "stalled PCM must not hold the old waveform open"
    );
    assert!(dirty, "the settled scope frame must be submitted");
}

/// 挂在设置弹窗下面的子页必须全部登记进 `settings_parent`：
/// 漏一个，那个页面按 Esc 就会直接退出全屏页（按键提示弹窗就这么漏过）。
#[test]
fn every_settings_child_returns_to_the_settings_modal() {
    for child in [
        Overlay::GeneralSettingsModal,
        Overlay::DownloadSettingsModal,
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
    assert_eq!(
        settings_parent(Overlay::MprisNameEditModal),
        Some(Overlay::GeneralSettingsModal)
    );
    assert_eq!(
        settings_parent(Overlay::DownloadPathEditModal),
        Some(Overlay::DownloadSettingsModal)
    );

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

struct TransitionHost {
    wake: crate::render::wake::WakeSignal,
}

impl HostPlaybackBridge for TransitionHost {
    async fn tick(&mut self) {}

    fn wake_signal(&self) -> crate::render::wake::WakeSignal {
        self.wake.clone()
    }

    fn metadata_signature(&self) -> u64 {
        0
    }

    fn runtime_snapshot(&self) -> HostPlaybackRuntimeSnapshot {
        HostPlaybackRuntimeSnapshot::default()
    }

    fn pcm_ring(&self) -> std::sync::Arc<crate::tmplayer::audio::pcm_tap::PcmRing> {
        std::sync::Arc::new(crate::tmplayer::audio::pcm_tap::PcmRing::new())
    }

    fn snapshot(&mut self) -> HostPlaybackSnapshot {
        HostPlaybackSnapshot::default()
    }

    fn host_snapshot<B: Backend>(
        &mut self,
        _terminal: &mut Terminal<B>,
    ) -> std::result::Result<ratatui::buffer::Buffer, B::Error> {
        Ok(ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(
            0, 0, 80, 24,
        )))
    }

    fn config_signature(&self) -> u64 {
        0
    }

    fn config_snapshot(&self) -> Config {
        Config::default()
    }

    async fn apply_config_sync(&mut self, _config: Config) {}

    fn vip_audio_unlocked(&self) -> bool {
        false
    }

    async fn toggle_play_pause(&mut self) {}
    async fn play_previous(&mut self) {}
    async fn play_next(&mut self) {}
    async fn play_queue_index(&mut self, _index: usize) {}
    fn request_queue_page(&mut self) {}
    fn seek_to_ratio(&mut self, _ratio: f32) {}
    fn set_volume(&mut self, _volume: f32) {}
    fn toggle_repeat_mode(&mut self) {}
    async fn toggle_like_current(&mut self) {}
    fn download_current(&mut self) {}
}

#[compio::test]
async fn fullscreen_page_transition_keeps_animation_frame_pacing() {
    let mut app = AppState::new(
        Config::default(),
        crate::ui::theme::Theme::default(),
        crate::data::config::Language::En,
    );
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    let wake = crate::render::wake::WakeSignal::default();
    let mut tui = Tui::new(&mut terminal, wake.clone());
    let mut host = TransitionHost { wake };
    let snapshot = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 80, 24));
    let started = Instant::now();
    let layout = play_page_transition(&mut tui, &mut app, &mut host, true, snapshot)
        .await
        .expect("page transition");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(900),
        "page transition must not fall back to the one-second idle heartbeat: {elapsed:?}"
    );
    assert_eq!(layout.full.width, 80);
}

#[test]
fn fullscreen_lyrics_do_not_force_continuous_redraw() {
    let config = Config {
        visualize: VisualizeMode::Lyrics,
        word_lyrics: true,
        ..Config::default()
    };
    let mut app = AppState::new(
        config,
        crate::ui::theme::Theme::default(),
        crate::data::config::Language::En,
    );
    app.player.playback = PlaybackState::Playing;
    app.player.track.lyrics = Some(vec![crate::lyrics::LyricLine {
        start_ms: 0,
        end_ms: Some(2_000),
        text: "line".to_string(),
        words: vec![],
        translation: None,
    }]);
    assert!(
        !app.should_continuous_redraw(),
        "lyrics must be driven by host position dirty frames, not a 60Hz clock"
    );
    app.config.visualize = VisualizeMode::Bars;
    assert!(
        app.should_continuous_redraw(),
        "the spectrum still animates continuously while playing"
    );
}

#[test]
fn continuous_animation_alone_does_not_mark_state_dirty() {
    let config = Config {
        visualize: VisualizeMode::Bars,
        ..Config::default()
    };
    let mut app = AppState::new(
        config,
        crate::ui::theme::Theme::default(),
        crate::data::config::Language::En,
    );
    app.player.playback = PlaybackState::Playing;
    assert!(
        app.should_continuous_redraw(),
        "bars animation is continuous while playing"
    );
    let now = Instant::now();
    app.last_frame = now;
    assert!(
        !tick_visual_state(&mut app, now),
        "continuous animation must be paced by the clock, not marked dirty every frame"
    );
}

#[test]
fn continuous_animation_end_marks_one_final_frame() {
    let start = Instant::now();
    let mut clock = FrameClock::new(60, start);
    let mut was_continuous = false;

    let continuous = true;
    mark_animation_end_frame(&mut clock, was_continuous, continuous);
    clock.set_continuous(continuous);
    was_continuous = continuous;
    clock.presented(start);

    let continuous = false;
    mark_animation_end_frame(&mut clock, was_continuous, continuous);
    clock.set_continuous(continuous);
    was_continuous = continuous;
    assert!(
        clock.due(start + Duration::from_millis(1)),
        "the animation's final frame must be submitted immediately"
    );
    clock.presented(start + Duration::from_millis(1));

    mark_animation_end_frame(&mut clock, was_continuous, continuous);
    clock.set_continuous(continuous);
    assert!(
        !clock.due(start + Duration::from_millis(2)),
        "staying idle must not keep the clock dirty"
    );
}

#[test]
fn runtime_snapshot_resets_position_anchor_when_position_is_unchanged() {
    let mut app = AppState::new(
        Config::default(),
        crate::ui::theme::Theme::default(),
        crate::data::config::Language::Zh,
    );
    app.player.playback = PlaybackState::Playing;
    app.player.position = Duration::from_secs(10);
    app.position_anchor_at = Instant::now() - Duration::from_secs(5);
    let before = app.position_anchor_at;

    apply_host_runtime_snapshot(
        &mut app,
        HostPlaybackRuntimeSnapshot {
            state: HostPlaybackState::Playing,
            position: Duration::from_secs(10),
            volume: 1.0,
            ..Default::default()
        },
    );

    assert!(
        app.position_anchor_at > before + Duration::from_secs(4),
        "an unchanged position snapshot must still refresh the anchor"
    );
}

#[compio::test]
async fn settings_preserve_sidebar_visibility_focus_and_modal_input() {
    use crate::data::config::GraphicsProtocol;
    use crate::tmplayer::utils::input::map_key;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;

    let snapshot = |count| HostPlaybackSnapshot {
        playlist: (0..count)
            .map(|i| crate::tmplayer::FullscreenPlaylistItemSeed {
                id: Some(i.to_string()),
                title: format!("Track {i}"),
                artist: String::new(),
                album: String::new(),
                duration: Duration::from_secs(60),
            })
            .collect(),
        current_index: Some(0),
        ..Default::default()
    };

    for sidebar_open in [false, true] {
        let config = Config {
            small_window_display: false,
            show_hints: false,
            graphics_protocol: GraphicsProtocol::Off,
            visualize: VisualizeMode::Lyrics,
            ..Config::default()
        };
        let mut app = AppState::new(
            config,
            crate::ui::theme::Theme::default(),
            crate::data::config::Language::En,
        );
        let mut host = TransitionHost {
            wake: crate::render::wake::WakeSignal::default(),
        };
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut tui = Tui::new(&mut terminal, host.wake.clone());
        sync_from_host_snapshot(&mut app, snapshot(5));
        let layout = tui.draw(&mut app).unwrap();
        if sidebar_open {
            handle_action(&mut app, &mut host, Action::TogglePlaylist, &layout)
                .await
                .unwrap();
            app.tick(Instant::now() + Duration::from_millis(250));
            app.playlist_view.selected = 2;
        }
        let layout = tui.draw(&mut app).unwrap();
        let sidebar_title = tui.terminal_mut().backend().buffer()[(1, 0)].clone();
        let action = map_key(
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
            app.overlay,
            &app.config,
        );
        handle_action(&mut app, &mut host, action, &layout)
            .await
            .unwrap();
        let modal_layout = tui.draw(&mut app).unwrap();
        assert_eq!(app.overlay, Overlay::SettingsModal);
        assert_eq!(modal_layout.playlist_rect, layout.playlist_rect);
        assert_eq!(
            tui.terminal_mut().backend().buffer()[(1, 0)],
            sidebar_title,
            "opening settings must not remove the sidebar underneath"
        );
        assert!(!modal_layout.modal_rows.get(0).unwrap().is_empty());
        assert!(!app.should_continuous_redraw());

        sync_from_host_snapshot(&mut app, snapshot(6));
        let selected = if sidebar_open { 2 } else { 0 };
        assert_eq!(app.playlist_view.selected, selected);
        handle_action(
            &mut app,
            &mut host,
            Action::MouseClick { col: 2, row: 2 },
            &modal_layout,
        )
        .await
        .unwrap();
        handle_action(
            &mut app,
            &mut host,
            Action::MouseScroll {
                col: 2,
                row: 2,
                forward: true,
            },
            &modal_layout,
        )
        .await
        .unwrap();
        assert_eq!(app.playlist_view.selected, selected);
        assert_eq!(app.settings_selected, 1, "scroll belongs to the modal");

        // Escape traverses the settings hierarchy before restoring the sidebar.
        app.settings_selected = 2;
        handle_action(&mut app, &mut host, Action::Confirm, &modal_layout)
            .await
            .unwrap();
        assert_eq!(app.overlay, Overlay::GeneralSettingsModal);
        handle_action(&mut app, &mut host, Action::CloseOverlay, &modal_layout)
            .await
            .unwrap();
        assert_eq!(app.overlay, Overlay::SettingsModal);
        handle_action(&mut app, &mut host, Action::CloseOverlay, &modal_layout)
            .await
            .unwrap();
        let restored = if sidebar_open {
            Overlay::Playlist
        } else {
            Overlay::None
        };
        assert_eq!(app.overlay, restored);
        assert_eq!(
            tui.draw(&mut app).unwrap().playlist_rect,
            layout.playlist_rect
        );

        // The settings shortcut also closes directly from a nested editor.
        handle_action(&mut app, &mut host, Action::OpenSettingsModal, &layout)
            .await
            .unwrap();
        app.settings_selected = 2;
        handle_action(&mut app, &mut host, Action::Confirm, &modal_layout)
            .await
            .unwrap();
        app.general_settings_selected = 5;
        handle_action(&mut app, &mut host, Action::Confirm, &modal_layout)
            .await
            .unwrap();
        assert!(app.mpris_name_edit.is_some());
        handle_action(&mut app, &mut host, Action::CloseSettings, &modal_layout)
            .await
            .unwrap();
        assert_eq!(app.overlay, restored);
        assert!(app.mpris_name_edit.is_none());
        assert_eq!(app.playlist_view.selected, selected);
    }
}

#[compio::test]
async fn name_edit_cancels_to_general_then_root_without_saving() {
    let mut app = AppState::new(
        Config::default(),
        crate::ui::theme::Theme::default(),
        crate::data::config::Language::En,
    );
    let mut host = TransitionHost {
        wake: crate::render::wake::WakeSignal::default(),
    };
    let layout = UiLayout::default();
    app.overlay = Overlay::SettingsModal;
    app.settings_selected = 2;
    handle_action(&mut app, &mut host, Action::Confirm, &layout)
        .await
        .unwrap();
    assert_eq!(app.overlay, Overlay::GeneralSettingsModal);
    handle_action(&mut app, &mut host, Action::ModalUp, &layout)
        .await
        .unwrap();
    assert_eq!(app.general_settings_selected, 5);
    handle_action(&mut app, &mut host, Action::Confirm, &layout)
        .await
        .unwrap();
    assert_eq!(app.overlay, Overlay::MprisNameEditModal);
    assert_eq!(app.mpris_name_edit.as_ref().unwrap().buffer, "");
    handle_action(&mut app, &mut host, Action::TextChar('t'), &layout)
        .await
        .unwrap();
    handle_action(
        &mut app,
        &mut host,
        Action::MouseScroll {
            col: 0,
            row: 0,
            forward: true,
        },
        &layout,
    )
    .await
    .unwrap();
    assert_eq!(app.general_settings_selected, 5);
    handle_action(&mut app, &mut host, Action::CloseOverlay, &layout)
        .await
        .unwrap();
    assert_eq!(app.overlay, Overlay::GeneralSettingsModal);
    assert!(app.mpris_name_edit.is_none());
    assert!(app.config.mpris_name.is_empty());
    handle_action(&mut app, &mut host, Action::CloseOverlay, &layout)
        .await
        .unwrap();
    assert_eq!(app.overlay, Overlay::SettingsModal);
    handle_action(&mut app, &mut host, Action::CloseOverlay, &layout)
        .await
        .unwrap();
    assert_eq!(app.overlay, Overlay::None);
}

#[compio::test]
async fn name_editor_unicode_limit_applies_at_home_and_allows_replacement() {
    let mut app = AppState::new(
        Config::default(),
        crate::ui::theme::Theme::default(),
        crate::data::config::Language::En,
    );
    app.config.mpris_name = "曲".repeat(256);
    app.overlay = Overlay::GeneralSettingsModal;
    app.general_settings_selected = 5;
    let mut host = TransitionHost {
        wake: crate::render::wake::WakeSignal::default(),
    };
    let layout = UiLayout::default();
    handle_action(&mut app, &mut host, Action::ModalLeft, &layout)
        .await
        .unwrap();
    assert_eq!(app.overlay, Overlay::GeneralSettingsModal);
    handle_action(&mut app, &mut host, Action::ModalRight, &layout)
        .await
        .unwrap();
    assert_eq!(app.overlay, Overlay::MprisNameEditModal);
    handle_action(&mut app, &mut host, Action::TextHome, &layout)
        .await
        .unwrap();
    handle_action(&mut app, &mut host, Action::TextChar('新'), &layout)
        .await
        .unwrap();
    assert_eq!(
        app.mpris_name_edit.as_ref().unwrap().buffer,
        "曲".repeat(256)
    );
    handle_action(&mut app, &mut host, Action::TextDelete, &layout)
        .await
        .unwrap();
    handle_action(&mut app, &mut host, Action::TextChar('新'), &layout)
        .await
        .unwrap();
    assert!(
        app.mpris_name_edit
            .as_ref()
            .unwrap()
            .buffer
            .starts_with("新曲")
    );
    assert_eq!(
        app.mpris_name_edit.as_ref().unwrap().buffer.chars().count(),
        256
    );
    handle_action(&mut app, &mut host, Action::CloseOverlay, &layout)
        .await
        .unwrap();
    assert_eq!(app.config.mpris_name, "曲".repeat(256));
    handle_action(&mut app, &mut host, Action::CloseSettings, &layout)
        .await
        .unwrap();
    assert_eq!(app.overlay, Overlay::None);
}

#[compio::test]
async fn root_settings_navigation_wraps_across_nine_rows() {
    let mut app = AppState::new(
        Config::default(),
        crate::ui::theme::Theme::default(),
        crate::data::config::Language::En,
    );
    let mut host = TransitionHost {
        wake: crate::render::wake::WakeSignal::default(),
    };
    let layout = UiLayout::default();
    handle_action(&mut app, &mut host, Action::OpenSettingsModal, &layout)
        .await
        .unwrap();
    handle_action(&mut app, &mut host, Action::ModalUp, &layout)
        .await
        .unwrap();
    assert_eq!(app.settings_selected, 8);
    handle_action(&mut app, &mut host, Action::Confirm, &layout)
        .await
        .unwrap();
    assert_eq!(app.overlay, Overlay::AboutModal);
    handle_action(&mut app, &mut host, Action::CloseOverlay, &layout)
        .await
        .unwrap();
    handle_action(&mut app, &mut host, Action::ModalDown, &layout)
        .await
        .unwrap();
    assert_eq!(app.settings_selected, 0);
}
