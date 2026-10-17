use super::*;

#[test]
fn lyrics_navigation_skips_snap_when_drag_is_disabled() {
    assert_eq!(step_lyrics_settings(1, true, false), 3);
    assert_eq!(step_lyrics_settings(3, false, false), 1);
    assert_eq!(step_lyrics_settings(4, true, false), 0);
    assert_eq!(step_lyrics_settings(0, false, false), 4);
}

#[test]
fn lyrics_navigation_keeps_snap_when_drag_is_enabled() {
    assert_eq!(step_lyrics_settings(1, true, true), 2);
    assert_eq!(step_lyrics_settings(2, false, true), 1);
    assert!(lyrics_settings_row_enabled(2, true));
    assert!(!lyrics_settings_row_enabled(2, false));
}
