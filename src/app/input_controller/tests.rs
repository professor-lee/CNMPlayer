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
