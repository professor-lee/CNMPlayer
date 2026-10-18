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
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
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
