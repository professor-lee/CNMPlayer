use super::FlatPanel;
use super::SmallWindowMode;
use super::analysis_demand;
use super::mini_spectrum_needs_redraw;
use super::small_window_needs_redraw;
use crate::app::Page;
use crate::data::config::Config;
use crate::data::config::VisualizeMode;

#[test]
fn mini_spectrum_redraw_stops_after_pause_tail_settles() {
    assert!(mini_spectrum_needs_redraw(true, true, true, false));
    assert!(mini_spectrum_needs_redraw(true, true, false, true));
    assert!(!mini_spectrum_needs_redraw(true, true, false, false));
    assert!(!mini_spectrum_needs_redraw(false, true, true, true));
    assert!(!mini_spectrum_needs_redraw(true, false, true, true));
}

#[test]
fn small_window_redraw_covers_playback_and_vu_animation() {
    assert!(small_window_needs_redraw(
        Some(SmallWindowMode::Flat),
        true,
        false
    ));
    assert!(!small_window_needs_redraw(
        Some(SmallWindowMode::Flat),
        false,
        true
    ));
    assert!(small_window_needs_redraw(
        Some(SmallWindowMode::Narrow),
        true,
        false
    ));
    assert!(small_window_needs_redraw(
        Some(SmallWindowMode::Narrow),
        false,
        true
    ));
    assert!(!small_window_needs_redraw(
        Some(SmallWindowMode::Narrow),
        false,
        false
    ));
    assert!(!small_window_needs_redraw(None, true, true));
}

#[test]
fn host_player_visualizations_select_only_pcm() {
    let mut config = Config {
        small_window_display: true,
        ..Config::default()
    };
    for visualize in [
        VisualizeMode::Bars,
        VisualizeMode::Oscilloscope,
        VisualizeMode::Vector,
        VisualizeMode::Lyrics,
        VisualizeMode::Hidden,
    ] {
        config.visualize = visualize;
        let expected = match visualize {
            VisualizeMode::Bars | VisualizeMode::Oscilloscope | VisualizeMode::Vector => {
                (true, false)
            }
            VisualizeMode::Lyrics | VisualizeMode::Hidden => (false, false),
        };
        for size in [(120, 40), (80, 8), (32, 5)] {
            assert_eq!(
                analysis_demand(&config, Page::Home, size, FlatPanel::Player, false, false),
                expected,
                "{visualize:?} at {size:?}"
            );
        }
    }
}

#[test]
fn narrow_windows_select_lufs_independently_of_visualization() {
    let mut config = Config {
        small_window_display: true,
        ..Config::default()
    };
    for visualize in [
        VisualizeMode::Bars,
        VisualizeMode::Oscilloscope,
        VisualizeMode::Vector,
        VisualizeMode::Lyrics,
        VisualizeMode::Hidden,
    ] {
        config.visualize = visualize;
        for size in [(1, 12), (31, 40)] {
            assert_eq!(
                analysis_demand(&config, Page::Home, size, FlatPanel::Player, false, false),
                (false, true),
                "{visualize:?} at {size:?}"
            );
        }
    }
}

#[test]
fn flat_lyrics_stop_pcm_only_after_the_player_is_offscreen() {
    let config = Config {
        visualize: VisualizeMode::Bars,
        small_window_display: true,
        ..Config::default()
    };
    for (height, panel, animating, expected) in [
        (5, FlatPanel::Player, false, (true, false)),
        (5, FlatPanel::Lyrics, true, (true, false)),
        (5, FlatPanel::Lyrics, false, (false, false)),
        (5, FlatPanel::Player, true, (true, false)),
        (6, FlatPanel::Lyrics, false, (true, false)),
        (11, FlatPanel::Lyrics, false, (true, false)),
    ] {
        assert_eq!(
            analysis_demand(&config, Page::Home, (80, height), panel, animating, false),
            expected,
            "height={height}, panel={panel:?}, animating={animating}"
        );
    }
}

#[test]
fn invisible_host_layouts_disable_both_consumers() {
    let config = Config {
        visualize: VisualizeMode::Bars,
        small_window_display: true,
        ..Config::default()
    };
    for page in [Page::Login, Page::Loading] {
        for size in [(120, 40), (31, 12), (80, 5)] {
            assert_eq!(
                analysis_demand(&config, page, size, FlatPanel::Player, false, false),
                (false, false)
            );
        }
    }
    for size in [(0, 40), (80, 0), (31, 11), (80, 4), (0, 12)] {
        assert_eq!(
            analysis_demand(&config, Page::Home, size, FlatPanel::Player, false, false),
            (false, false),
            "{size:?}"
        );
    }
}

#[test]
fn ordinary_player_visibility_respects_each_page_minimum() {
    let mut config = Config {
        visualize: VisualizeMode::Bars,
        small_window_display: false,
        ..Config::default()
    };
    for (page, size, expected) in [
        (Page::Home, (31, 12), (false, false)),
        (Page::Home, (32, 11), (false, false)),
        (Page::Home, (32, 12), (true, false)),
        (Page::Playlist, (39, 14), (false, false)),
        (Page::Playlist, (40, 13), (false, false)),
        (Page::Playlist, (40, 14), (true, false)),
        (Page::Author, (39, 14), (false, false)),
        (Page::Author, (40, 13), (false, false)),
        (Page::Author, (40, 14), (true, false)),
        (Page::Search, (41, 14), (false, false)),
        (Page::Search, (42, 13), (false, false)),
        (Page::Search, (42, 14), (true, false)),
    ] {
        assert_eq!(
            analysis_demand(&config, page, size, FlatPanel::Player, false, false),
            expected,
            "{page:?} at {size:?}"
        );
    }
    config.small_window_display = true;
    for page in [Page::Home, Page::Playlist, Page::Author, Page::Search] {
        assert_eq!(
            analysis_demand(&config, page, (32, 12), FlatPanel::Player, false, false),
            (true, false)
        );
    }
}

#[test]
fn fullscreen_visualizations_ignore_host_layout_and_never_select_lufs() {
    let mut config = Config {
        small_window_display: true,
        ..Config::default()
    };
    for visualize in [
        VisualizeMode::Bars,
        VisualizeMode::Oscilloscope,
        VisualizeMode::Vector,
        VisualizeMode::Lyrics,
        VisualizeMode::Hidden,
    ] {
        config.visualize = visualize;
        let expected = match visualize {
            VisualizeMode::Bars | VisualizeMode::Oscilloscope | VisualizeMode::Vector => {
                (true, false)
            }
            VisualizeMode::Lyrics | VisualizeMode::Hidden => (false, false),
        };
        for size in [(120, 40), (31, 12), (80, 5), (20, 4)] {
            assert_eq!(
                analysis_demand(&config, Page::Home, size, FlatPanel::Lyrics, false, true),
                expected,
                "{visualize:?} at {size:?}"
            );
        }
    }
}

#[test]
fn fullscreen_exit_uses_current_config_and_host_geometry() {
    let mut config = Config {
        visualize: VisualizeMode::Bars,
        small_window_display: true,
        ..Config::default()
    };
    assert_eq!(
        analysis_demand(
            &config,
            Page::Home,
            (30, 20),
            FlatPanel::Player,
            false,
            false
        ),
        (false, true)
    );
    assert_eq!(
        analysis_demand(
            &config,
            Page::Home,
            (30, 20),
            FlatPanel::Player,
            false,
            true
        ),
        (true, false)
    );
    config.visualize = VisualizeMode::Hidden;
    assert_eq!(
        analysis_demand(
            &config,
            Page::Home,
            (30, 20),
            FlatPanel::Player,
            false,
            true
        ),
        (false, false)
    );
    config.visualize = VisualizeMode::Vector;
    config.small_window_display = false;
    assert_eq!(
        analysis_demand(
            &config,
            Page::Home,
            (30, 20),
            FlatPanel::Player,
            false,
            true
        ),
        (true, false)
    );
    assert_eq!(
        analysis_demand(
            &config,
            Page::Home,
            (30, 20),
            FlatPanel::Player,
            false,
            false
        ),
        (false, false)
    );
    assert_eq!(
        analysis_demand(
            &config,
            Page::Home,
            (120, 40),
            FlatPanel::Player,
            false,
            false
        ),
        (true, false)
    );
    config.small_window_display = true;
    assert_eq!(
        analysis_demand(
            &config,
            Page::Home,
            (30, 20),
            FlatPanel::Player,
            false,
            false
        ),
        (false, true)
    );
}
