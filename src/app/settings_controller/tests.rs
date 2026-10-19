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

#[test]
fn text_edit_limit_counts_all_unicode_chars_even_after_moving_home() {
    let mut edit = SettingsTextEdit::new("歌".repeat(MPRIS_NAME_MAX_CHARS));
    edit.home();
    edit.insert('t', MPRIS_NAME_MAX_CHARS);
    assert_eq!(edit.buffer, "歌".repeat(MPRIS_NAME_MAX_CHARS));
    assert_eq!(edit.cursor, 0);

    edit.delete();
    edit.insert('曲', MPRIS_NAME_MAX_CHARS);
    assert_eq!(char_count(&edit.buffer), MPRIS_NAME_MAX_CHARS);
    assert!(edit.buffer.starts_with('曲'));
    assert_eq!(edit.cursor, 1);
}

#[test]
fn text_edit_keeps_unicode_cursor_boundaries_and_rejects_controls() {
    let mut edit = SettingsTextEdit::new("a中🎵b".to_string());
    edit.home();
    edit.backspace();
    edit.move_cursor(2);
    edit.backspace();
    assert_eq!(edit.buffer, "a🎵b");
    assert_eq!(edit.cursor, 1);
    edit.delete();
    assert_eq!(edit.buffer, "ab");
    for ch in ['\n', '\r', '\t', '\0', '\u{7f}'] {
        edit.insert(ch, MPRIS_NAME_MAX_CHARS);
    }
    assert_eq!(edit.buffer, "ab");
    edit.end();
    edit.delete();
    edit.move_cursor(i32::MAX);
    assert_eq!(edit.cursor, 2);
    edit.move_cursor(i32::MIN);
    assert_eq!(edit.cursor, 0);
}

#[test]
fn empty_text_edit_starts_raw_and_accepts_t_as_text() {
    let mut edit = SettingsTextEdit::new(String::new());
    assert_eq!(edit.buffer, "");
    assert_eq!(edit.cursor, 0);
    edit.insert('t', MPRIS_NAME_MAX_CHARS);
    assert_eq!(edit.buffer, "t");
    edit.backspace();
    assert_eq!(edit.buffer, "");
    assert_eq!(edit.cursor, 0);
}

#[test]
fn resetting_settings_discards_both_text_editors_and_general_scroll() {
    let mut settings = SettingsController {
        general_selected: 5,
        general_scroll: 4,
        mpris_name_edit: Some(SettingsTextEdit::new("discard".to_string())),
        ..SettingsController::default()
    };
    settings.begin_download_path_edit("/discard".to_string());
    assert!(settings.text_editing());
    settings.reset_navigation();
    assert!(!settings.text_editing());
    assert_eq!(settings.general_selected, 0);
    assert_eq!(settings.general_scroll, 0);
}

#[test]
fn general_viewport_keeps_offset_until_focus_crosses_a_boundary() {
    use crate::ui::settings::scroll_for_focus;
    let scroll = scroll_for_focus(0, SETTINGS_GENERAL_ITEMS, 2, 2);
    assert_eq!(scroll, 1);
    assert_eq!(scroll_for_focus(scroll, SETTINGS_GENERAL_ITEMS, 2, 1), 1);
    assert_eq!(scroll_for_focus(scroll, SETTINGS_GENERAL_ITEMS, 2, 0), 0);
    assert_eq!(scroll_for_focus(4, SETTINGS_GENERAL_ITEMS, 6, 5), 0);
}

#[test]
fn chinese_text_window_keeps_end_caret_inside_the_visible_width() {
    use crate::ui::settings::{adjust_path_window, caret_display_col, display_width, path_window};
    let text = "中文歌曲";
    let start = adjust_path_window(0, caret_display_col(text, 4), 4);
    let (visible, caret) = path_window(text, 4, 4, start);
    assert_eq!(visible, " 曲");
    assert_eq!(caret, 2);
    assert_eq!(display_width(&visible), 3);

    let (visible, caret) = path_window("ab中", 2, 3, 0);
    assert_eq!(visible, "ab");
    assert_eq!(caret, 2);
    let (visible, caret) = path_window("", 0, 4, 0);
    assert_eq!(visible, "");
    assert_eq!(caret, 0);
}
