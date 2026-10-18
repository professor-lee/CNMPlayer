use super::*;

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
