use super::UiIcons;
use std::time::Duration;

#[test]
fn icons_are_stable_nerd_font_glyphs() {
    let icons = UiIcons::new();
    assert_eq!(icons.previous(), "[\u{f048}]");
    assert_eq!(icons.play_pause(false), "[\u{f04b}]");
    assert_eq!(icons.play_pause(true), "[\u{f04c}]");
    assert_eq!(icons.heart(true), "\u{f004}");
    assert_eq!(icons.heart(false), "\u{f08a}");
    assert_eq!(icons.download(), '\u{ec74}');
    assert_eq!(icons.downloaded(), '\u{f00c}');
    assert_eq!(icons.downloading(Duration::from_millis(100)), '\u{2819}');
}
