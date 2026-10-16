use super::*;
use crate::app::settings_controller::SETTINGS_KEYBIND_ITEMS;
/// 编辑态横向窗口：光标在窗口内不动；撞到边界时**一次只挪一个字符**。
#[test]
fn path_window_only_scrolls_at_the_edges() {
    // 窗口内：不动（文本不跟着每一次光标移动一起滑）。
    assert_eq!(adjust_path_window(10, 15, 20), 10);
    assert_eq!(adjust_path_window(0, 19, 20), 0);

    // 撞右边界：只挪一格，光标贴回右端。
    assert_eq!(adjust_path_window(0, 20, 20), 1);
    assert_eq!(adjust_path_window(5, 25, 20), 6);
    assert!(agent_in_window(20, adjust_path_window(0, 20, 20), 20));

    // 撞左边界：只挪一格。
    assert_eq!(adjust_path_window(10, 9, 20), 9);
    assert!(agent_in_window(9, adjust_path_window(10, 9, 20), 20));

    // 一次性跳很远（Home / End / 点击）：对齐到光标那一端。
    assert_eq!(adjust_path_window(100, 5, 20), 5);
    assert_eq!(adjust_path_window(0, 200, 20), 181);
    assert!(agent_in_window(200, adjust_path_window(0, 200, 20), 20));

    // 宽度 0 时不滚动。
    assert_eq!(adjust_path_window(7, 0, 0), 0);
}

fn agent_in_window(caret_col: usize, window: usize, width: usize) -> bool {
    caret_col >= window && caret_col < window + width
}

#[test]
fn caret_column_counts_wide_chars() {
    // "/tmp/" 5 列；再接 "音乐" 各占两列。
    assert_eq!(caret_display_col("/tmp/音乐", 5), 5);
    assert_eq!(caret_display_col("/tmp/音乐", 6), 7);
    assert_eq!(caret_display_col("/tmp/音乐", 7), 9);
    assert_eq!(caret_display_col("/tmp/音乐", 0), 0);
}

/// 窗口串：从 `start_col` 起截断，光标下标按窗口内字符位置给出。
#[test]
fn path_window_slices_from_the_start_column() {
    let (visible, caret) = path_window("abcdef", 6, 3, 2);
    assert_eq!(visible, "cde");
    assert_eq!(caret, visible.chars().count(), "光标在串尾时跟在最后");

    let (visible, caret) = path_window("abcdef", 3, 3, 2);
    assert_eq!(visible, "cde");
    assert_eq!(caret, 1);
}

/// 焦点条目必须始终落在可视窗口内——否则键盘选中的那一行点不到，
/// 鼠标点的行也不是看到的那一行。
#[test]
fn focused_keybind_row_is_always_inside_the_window() {
    let total = crate::app::settings_controller::SETTINGS_KEYBIND_ITEMS + 3;

    for visible_rows in 1..=total {
        for start_scroll in 0..=total {
            for focus in 0..crate::app::settings_controller::SETTINGS_KEYBIND_ITEMS {
                let scroll = scroll_for_focus(start_scroll, total, visible_rows, focus);
                let end = (scroll + visible_rows).min(total);

                assert!(
                    scroll <= focus && focus < end,
                    "visible_rows={visible_rows} start_scroll={start_scroll} \
                     focus={focus} scroll={scroll}"
                );
            }
        }
    }
}

/// 应用内滚动语义：焦点还在窗口内时视口不动，越过边界才滚，
/// 且只滚到刚好把焦点行露出来（不持续把焦点钉在窗口底边）。
#[test]
fn keybind_scroll_only_moves_on_boundary_crossing() {
    let total = crate::app::settings_controller::SETTINGS_KEYBIND_ITEMS + 3;

    // 焦点在窗口 [4, 12) 内：视口不动。
    assert_eq!(scroll_for_focus(4, total, 8, 6), 4);
    assert_eq!(scroll_for_focus(4, total, 8, 11), 4);
    // 越过下边界：只滚一格，焦点贴底。
    assert_eq!(scroll_for_focus(4, total, 8, 12), 5);
    // 越过上边界：只滚一格，焦点贴顶。
    assert_eq!(scroll_for_focus(6, total, 8, 5), 5);
    // 视口增长到装下全部内容：钳制回 0。
    assert_eq!(scroll_for_focus(6, total, 30, 10), 0);
}

#[test]
fn scroll_is_zero_when_everything_fits() {
    assert_eq!(scroll_for_focus(7, 23, 23, 19), 0);
    assert_eq!(scroll_for_focus(7, 23, 40, 19), 0);
}

/// 命中区行号必须与渲染行号一致：条目 i 画在 `i - scroll` 行。
#[test]
fn scrolled_rows_match_the_rendered_offsets() {
    for scroll in [0usize, 1, 7, 12] {
        let rows = scrolled_rows(
            crate::app::settings_controller::SETTINGS_KEYBIND_ITEMS,
            8,
            scroll,
        );
        for (idx, offset) in &rows {
            assert_eq!(*idx as isize - *offset as isize, scroll as isize);
            assert!(*offset < 8);
        }
        // 窗口内的条目一个不漏
        let expected: Vec<usize> = (scroll
            ..crate::app::settings_controller::SETTINGS_KEYBIND_ITEMS)
            .take(8)
            .collect();
        let got: Vec<usize> = rows.iter().map(|(idx, _)| *idx).collect();
        assert_eq!(got, expected);
    }
}

#[test]
fn scrolled_rows_is_empty_without_room() {
    assert!(scrolled_rows(SETTINGS_KEYBIND_ITEMS, 0, 3).is_empty());
}
