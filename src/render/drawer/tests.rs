use super::*;
use ratatui::style::{Color, Style};

fn row(buffer: &Buffer, y: u16) -> String {
    (buffer.area.left()..buffer.area.right())
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

#[test]
fn paint_clips_both_edges_of_wide_characters() {
    let area = Rect::new(4, 7, 6, 1);
    let mut source = Buffer::empty(area);
    source.set_string(4, 7, "中AB文", Style::default());
    let mut target = Buffer::empty(area);
    target.set_string(4, 7, "......", Style::default());

    paint(&mut target, &source, -1, Rect::new(4, 7, 3, 1));
    assert_eq!(row(&target, 7), " AB...");
    paint(&mut target, &source, 0, Rect::new(4, 7, 1, 1));
    assert_eq!(target[(4, 7)].symbol(), " ");
}

#[test]
fn reveal_keeps_background_coordinates_and_translates_each_foreground_row() {
    let area = Rect::new(4, 7, 6, 2);
    let back_style = Style::default().fg(Color::Blue).bg(Color::Black);
    let front_style = Style::default().fg(Color::Red).bg(Color::White);
    let mut background = Buffer::empty(area);
    background.set_string(4, 7, "abcdef", back_style);
    background.set_string(4, 8, "ghijkl", back_style);
    let mut target = Buffer::empty(area);
    target.set_string(4, 7, "ABCDEF", front_style);
    target.set_string(4, 8, "GHIJKL", front_style);
    let mut expected = Buffer::empty(area);
    expected.set_string(4, 7, "ab", back_style);
    expected.set_string(6, 7, "ABCD", front_style);
    expected.set_string(4, 8, "gh", back_style);
    expected.set_string(6, 8, "GHIJ", front_style);

    reveal_from_right(&mut target, &background, 2, area);

    assert_eq!(target, expected);
}

#[test]
fn reveal_blanks_a_foreground_wide_character_cut_by_the_right_edge() {
    let area = Rect::new(0, 0, 6, 1);
    let mut background = Buffer::empty(area);
    background.set_string(0, 0, "abcdef", Style::default());
    let front_style = Style::default().fg(Color::Red);
    let mut target = Buffer::empty(area);
    target.set_string(0, 0, "A中BCD", front_style);

    reveal_from_right(&mut target, &background, 4, area);

    assert_eq!(row(&target, 0), "abcdA ");
    assert_eq!(target[(5, 0)].fg, Color::Red);
}

#[test]
fn reveal_preserves_a_complete_foreground_wide_character() {
    let area = Rect::new(0, 0, 6, 1);
    let mut background = Buffer::empty(area);
    background.set_string(0, 0, "abcdef", Style::default());
    let mut target = Buffer::empty(area);
    target.set_string(0, 0, "A中BCD", Style::default());

    reveal_from_right(&mut target, &background, 3, area);

    assert_eq!(row(&target, 0), "abcA中 ");
}

#[test]
fn reveal_blanks_a_background_wide_character_at_the_drawer_seam() {
    let area = Rect::new(0, 0, 6, 1);
    let mut background = Buffer::empty(area);
    background.set_string(0, 0, "A中BCD", Style::default());
    let mut target = Buffer::empty(area);
    target.set_string(0, 0, "123456", Style::default());

    reveal_from_right(&mut target, &background, 2, area);

    assert_eq!(row(&target, 0), "A 1234");
}

#[test]
fn reveal_uses_the_valid_background_intersection() {
    let mut background = Buffer::empty(Rect::new(4, 7, 8, 1));
    background.set_string(4, 7, "中abcdef", Style::default());
    let mut target = Buffer::empty(Rect::new(5, 7, 6, 1));
    target.set_string(5, 7, "ABCDEF", Style::default());

    let area = target.area;
    reveal_from_right(&mut target, &background, 2, area);

    assert_eq!(row(&target, 7), " aABCD");
}

#[test]
fn reveal_endpoints_leave_the_page_or_show_only_the_background() {
    let area = Rect::new(4, 7, 6, 1);
    let mut background = Buffer::empty(area);
    background.set_string(4, 7, "abcdef", Style::default());
    let mut foreground = Buffer::empty(area);
    foreground.set_string(4, 7, "ABCDEF", Style::default());
    let mut target = foreground.clone();

    reveal_from_right(&mut target, &background, 0, area);
    assert_eq!(target, foreground);
    for offset in [6, 9, u16::MAX] {
        target.clone_from(&foreground);
        reveal_from_right(&mut target, &background, offset, area);
        assert_eq!(target, background);
    }
}

#[test]
fn reveal_leaves_fixed_regions_untouched() {
    let area = Rect::new(4, 7, 6, 3);
    let mut background = Buffer::empty(area);
    background.set_string(4, 7, "abcdef", Style::default());
    background.set_string(4, 8, "ghijkl", Style::default());
    background.set_string(4, 9, "xxxxxx", Style::default());
    let mut target = Buffer::empty(area);
    target.set_string(4, 7, "HEADER", Style::default());
    target.set_string(4, 8, "ABCDEF", Style::default());
    target.set_string(4, 9, "PLAYER", Style::default());

    reveal_from_right(&mut target, &background, 2, Rect::new(4, 8, 6, 1));

    assert_eq!(row(&target, 7), "HEADER");
    assert_eq!(row(&target, 8), "ghABCD");
    assert_eq!(row(&target, 9), "PLAYER");
}

// Apply actual diff writes as a terminal does: a wide glyph paints both columns,
// even though its trailing Buffer cell may have a different background.
fn submit_backgrounds(previous: &Buffer, next: &Buffer, visible: &mut [Color]) {
    for (x, _, cell) in previous.diff(next) {
        let start = usize::from(x);
        let end = (start + cell.symbol().width()).min(visible.len());
        visible[start..end].fill(cell.bg);
    }
}

#[test]
fn drawer_transitions_clear_wide_row_highlights_before_focus_moves() {
    use ratatui::text::Span;
    use ratatui::widgets::{Paragraph, Widget};

    let area = Rect::new(0, 0, 12, 1);
    for base in [Color::Reset, Color::Black] {
        for text in ["album 2 首", "album 2 tr"] {
            for opening in [true, false] {
                let mut background = Buffer::empty(area);
                background.set_style(area, Style::default().bg(base));
                let mut page = background.clone();
                Paragraph::new(Span::styled(text, Style::default().bg(Color::Blue)))
                    .render(Rect::new(1, 0, 10, 1), &mut page);
                let mut previous = Buffer::empty(area);
                let mut visible = vec![Color::Reset; 12];
                let offsets: &[u16] = if opening { &[2, 1, 0] } else { &[0, 1, 2, 12] };
                for &offset in offsets {
                    let mut next = page.clone();
                    if opening {
                        reveal_from_right(&mut next, &background, offset, area);
                    } else {
                        paint(&mut next, &background, i32::from(offset), area);
                    }
                    submit_backgrounds(&previous, &next, &mut visible);
                    previous = next;
                }
                assert_eq!(visible[11], base, "{base:?}, {text:?}, opening={opening}");

                let mut unfocused = background.clone();
                Paragraph::new(Span::styled(text, Style::default().bg(base)))
                    .render(Rect::new(1, 0, 10, 1), &mut unfocused);
                submit_backgrounds(&previous, &unfocused, &mut visible);
                assert_eq!(visible, vec![base; 12], "focus moved after {text:?}");
            }
        }
    }
}
