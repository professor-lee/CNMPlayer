use super::*;
use ratatui::layout::Rect;

#[test]
fn clipped_rect_at_clips_both_sides() {
    let area = Rect {
        x: 10,
        y: 2,
        width: 20,
        height: 5,
    };
    let left = clipped_rect_at(area, 5);
    assert_eq!(
        left,
        Rect {
            x: 10,
            y: 2,
            width: 15,
            height: 5,
        }
    );
    let right = clipped_rect_at(area, 15);
    assert_eq!(
        right,
        Rect {
            x: 15,
            y: 2,
            width: 15,
            height: 5,
        }
    );
    assert_eq!(clipped_rect_at(area, 40), Rect::default());
    assert_eq!(clipped_rect_at(area, -30), Rect::default());
}
