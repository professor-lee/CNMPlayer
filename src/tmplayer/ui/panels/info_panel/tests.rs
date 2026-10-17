use super::{cover_content_rect, layout};
use ratatui::layout::{Constraint, Layout, Rect};

#[test]
fn cover_image_stays_square_in_full_width_and_two_panel_layouts() {
    for (width, height) in [(80, 24), (120, 24), (120, 40), (200, 50), (51, 13)] {
        let window = Rect::new(7, 5, width, height);
        let left = Layout::horizontal([Constraint::Percentage(33), Constraint::Percentage(67)])
            .split(window)[0];
        for panel in [left, window] {
            let info = layout(panel, width);
            let image = cover_content_rect(info.cover);
            if info.cover.is_empty() {
                assert!(image.is_empty());
                continue;
            }
            assert_eq!(
                image.width,
                image.height * 2,
                "image={image:?}, panel={panel:?}"
            );
            assert_eq!(info.cover.width, image.width + 2);
            assert_eq!(info.cover.height, image.height + 2);
            assert_eq!(image.x, info.cover.x + 1);
            assert_eq!(image.y, info.cover.y + 1);
            assert_eq!(info.cover.intersection(info.inner), info.cover);
            let left_gap = info.cover.left() - info.inner.left();
            let right_gap = info.inner.right() - info.cover.right();
            assert!(left_gap.abs_diff(right_gap) <= 1);
            assert!(info.meta.top() >= info.cover.bottom());
        }
    }
}

#[test]
fn small_cover_space_never_degrades_to_a_non_square_image() {
    for width in 0..24 {
        for height in 0..20 {
            let info = layout(Rect::new(7, 5, width, height), width);
            let image = cover_content_rect(info.cover);
            if info.cover.is_empty() {
                assert_eq!((info.cover.width, info.cover.height), (0, 0));
            } else {
                assert!(image.height > 0);
                assert_eq!(image.width, image.height * 2);
                assert_eq!(info.cover.intersection(info.inner), info.cover);
            }
        }
    }
}
