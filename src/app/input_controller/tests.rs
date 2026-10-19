use super::HitRect;
use super::player_bar_progress_ratio;

#[test]
fn progress_drag_clamps_both_endpoints() {
    let rect = HitRect {
        x: 10,
        y: 4,
        width: 9,
        height: 1,
    };
    assert_eq!(player_bar_progress_ratio(rect, 0), 0.0);
    assert_eq!(player_bar_progress_ratio(rect, 10), 0.0);
    assert_eq!(player_bar_progress_ratio(rect, 14), 0.5);
    assert_eq!(player_bar_progress_ratio(rect, 18), 1.0);
    assert_eq!(player_bar_progress_ratio(rect, u16::MAX), 1.0);
}

#[test]
fn single_cell_progress_keeps_click_mapping_during_drag() {
    let rect = HitRect {
        x: 10,
        y: 4,
        width: 1,
        height: 1,
    };
    assert_eq!(player_bar_progress_ratio(rect, 0), 0.0);
    assert_eq!(player_bar_progress_ratio(rect, 10), 0.0);
    assert_eq!(player_bar_progress_ratio(rect, u16::MAX), 0.0);
}

#[test]
fn sidebar_hits_translate_and_clip_without_catching_hidden_rows_or_cells() {
    use ratatui::layout::Rect;
    let clip = Rect::new(4, 3, 120, 36);
    let panel = HitRect::from(Rect::new(4, 3, 40, 36));
    let playlist = HitRect::from(Rect::new(6, 7, 36, 1));
    for dx in [-40, -39, -20, -1, 0, -1, -20, -39, -40] {
        let visible_panel = panel.translated_clip(dx, clip);
        let visible_row = playlist.translated_clip(dx, clip);
        if dx == -40 {
            assert!(visible_panel.is_none());
        } else {
            let panel = visible_panel.unwrap();
            assert_eq!(panel.width as i32, 40 + dx);
            assert!(!panel.contains(panel.x + panel.width, panel.y));
        }
        if dx <= -38 {
            assert!(
                visible_row.is_none(),
                "a hidden list must not intercept clicks"
            );
        } else {
            let row = visible_row.unwrap();
            assert_eq!(i32::from(row.x), (6 + dx).max(4));
            assert!(visible_panel.unwrap().contains(row.x, row.y));
            assert!(!row.contains(row.x, row.y + 1));
        }
    }
}
