use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

pub(crate) fn translated_clip(area: Rect, dx: i32, clip: Rect) -> Rect {
    let x = i32::from(area.x).saturating_add(dx);
    let start = x.max(i32::from(clip.left()));
    let end = x
        .saturating_add(i32::from(area.width))
        .min(i32::from(clip.right()));
    let top = area.top().max(clip.top());
    let bottom = area.bottom().min(clip.bottom());
    if start >= end || top >= bottom {
        return Rect::default();
    }
    Rect::new(start as u16, top, (end - start) as u16, bottom - top)
}

pub(crate) fn paint(target: &mut Buffer, source: &Buffer, dx: i32, clip: Rect) {
    let clip = translated_clip(source.area, dx, clip.intersection(target.area));
    for y in clip.top()..clip.bottom() {
        for x in clip.left()..clip.right() {
            let source_x = (i32::from(x) - dx) as u16;
            let cell = &source[(source_x, y)];
            let dest = &mut target[(x, y)];
            dest.clone_from(cell);
            // Never expose half a wide title character at either clipping boundary.
            let cut_left = x == clip.left()
                && source_x > source.area.left()
                && source[(source_x - 1, y)].symbol().width() > 1;
            if cut_left || usize::from(x) + cell.symbol().width() > usize::from(clip.right()) {
                dest.set_char(' ');
            }
        }
    }
}

/// Shift an already-rendered page right, revealing the unshifted background.
pub(crate) fn reveal_from_right(target: &mut Buffer, background: &Buffer, offset: u16, clip: Rect) {
    if offset == 0 || target.area.is_empty() {
        return;
    }
    let area = target.area;
    let clip = area.intersection(clip);
    let offset = offset.min(area.width);
    let width = usize::from(area.width);
    let shift = usize::from(offset);
    let start_row = usize::from(clip.top().saturating_sub(area.top()));
    let left = usize::from(clip.left().saturating_sub(area.left()));
    let right = usize::from(clip.right().saturating_sub(area.left()));
    for row in target
        .content
        .chunks_exact_mut(width)
        .skip(start_row)
        .take(usize::from(clip.height))
    {
        // Descending destinations keep every source cell intact until it is copied.
        for destination in (shift.max(left)..right).rev() {
            let (source, destination_cells) = row.split_at_mut(destination);
            let cell = &source[destination - shift];
            let dest = &mut destination_cells[0];
            dest.clone_from(cell);
            if destination + cell.symbol().width() > right {
                dest.set_char(' ');
            }
        }
    }
    paint(
        target,
        background,
        0,
        Rect::new(area.x, area.y, offset, area.height).intersection(clip),
    );
}

#[cfg(test)]
mod tests;
