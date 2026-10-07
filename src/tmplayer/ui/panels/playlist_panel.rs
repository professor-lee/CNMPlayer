use crate::tmplayer::app::state::AppState;
use crate::tmplayer::render::cover_cache::CoverKey;
use crate::tmplayer::ui::borders::SOLID_BORDER;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MIN_COVER_LAYOUT_WIDTH: u16 = 2;
const MIN_COVER_LAYOUT_HEIGHT: u16 = 7;

#[derive(Debug, Clone, Copy)]
pub struct PlaylistPanelLayout {
    pub inner: Rect,
    pub cover_area: Rect,
    pub cover_rect: Rect,
    pub separator_area: Rect,
    pub list_area: Rect,
    pub list_inner: Rect,
}

fn list_only_layout(inner: Rect) -> PlaylistPanelLayout {
    PlaylistPanelLayout {
        inner,
        cover_area: Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 0,
        },
        cover_rect: Rect::default(),
        separator_area: Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 0,
        },
        list_area: inner,
        list_inner: inner,
    }
}

pub fn compute_layout(area: Rect, app: &AppState) -> PlaylistPanelLayout {
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    if app.playlist_cover.is_none()
        || inner.width < MIN_COVER_LAYOUT_WIDTH
        || inner.height < MIN_COVER_LAYOUT_HEIGHT
    {
        return list_only_layout(inner);
    }

    let cover_h = ((inner.height as f32) / 3.0)
        .round()
        .clamp(3.0, inner.height.saturating_sub(4) as f32) as u16;
    let sep_h = 1;
    let list_h = inner.height.saturating_sub(cover_h).saturating_sub(sep_h);
    let cover_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: cover_h,
    };
    let cover_rect = cover_rect_in_area(cover_area);
    let separator_area = Rect {
        x: inner.x,
        y: inner.y + cover_h,
        width: inner.width,
        height: sep_h,
    };
    let list_area = Rect {
        x: inner.x,
        y: inner.y + cover_h + sep_h,
        width: inner.width,
        height: list_h,
    };
    PlaylistPanelLayout {
        inner,
        cover_area,
        cover_rect,
        separator_area,
        list_area,
        list_inner: list_area,
    }
}

fn cover_rect_in_area(area: Rect) -> Rect {
    let avail_w = area.width.saturating_sub(4);
    let cover_h = area.height.min((avail_w / 2).max(1)).max(1);
    let cover_w = cover_h.saturating_mul(2).min(avail_w).max(2);
    Rect {
        x: area.x + area.width.saturating_sub(cover_w) / 2,
        y: area.y + area.height.saturating_sub(cover_h) / 2,
        width: cover_w,
        height: cover_h,
    }
}

fn placeholder(width: u16, height: u16) -> String {
    let row = "█".repeat(width as usize);
    std::iter::repeat_n(row, height as usize)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_album_cover(buf: &mut Buffer, area: Rect, app: &mut AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cover = cover_rect_in_area(area);
    let Some(bytes) = app.playlist_cover.as_deref() else {
        return;
    };
    let hash = app.playlist_cover_hash.unwrap_or_else(|| hash_bytes(bytes));
    let key = CoverKey {
        hash,
        width: cover.width,
        height: cover.height,
    };
    let cached = app.cover_cache.borrow_mut().get(key);
    let ascii = cached.unwrap_or_else(|| {
        app.queue_cover_ascii_render(key, bytes, '█');
        placeholder(cover.width, cover.height)
    });
    Paragraph::new(ascii)
        .style(
            Style::default()
                .fg(app.theme.color_text())
                .bg(app.theme.color_surface()),
        )
        .wrap(Wrap { trim: false })
        .render(cover, buf);
}

fn render_separator(buf: &mut Buffer, area: Rect, app: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let line_area = Rect {
        x: area.x.saturating_sub(1),
        y: area.y,
        width: area.width.saturating_add(2),
        height: area.height,
    };
    let dashes = usize::from(line_area.width).saturating_sub(2);
    Paragraph::new(format!("├{}┤", "─".repeat(dashes)))
        .style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        )
        .render(line_area, buf);
}

fn render_playlist_list(buf: &mut Buffer, area: Rect, app: &mut AppState) {
    let footer_rows = 2;
    let list_rows = area.height.saturating_sub(footer_rows);
    let total = app.playlist_view.items.len();
    let selected = app.playlist_view.selected.min(total.saturating_sub(1));
    let visible = list_rows as usize;
    let start =
        crate::ui::settings::scroll_for_focus(app.playlist_list_scroll, total, visible, selected);
    let end = if visible == 0 {
        0
    } else {
        (start + visible).min(total)
    };
    app.playlist_list_scroll = start;
    app.playlist_list_rows = visible;

    let mut lines = Vec::new();
    if total == 0 {
        lines.push(Line::styled(
            "(empty)",
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        ));
    } else {
        for i in start..end {
            let item = &app.playlist_view.items[i];
            let raw = format!("{:02}. {}", i + 1, item.title);
            let mut style = Style::default()
                .fg(app.theme.color_text())
                .bg(app.theme.color_surface());
            if i == app.playlist_view.selected {
                style = Style::default()
                    .fg(app.theme.color_base())
                    .bg(app.theme.color_accent())
                    .add_modifier(Modifier::BOLD);
            } else if app.playlist_view.current == Some(i) {
                style = style
                    .fg(app.theme.color_accent3())
                    .add_modifier(Modifier::BOLD);
            }
            lines.push(Line::styled(
                clip_with_ellipsis(&raw, area.width as usize),
                style,
            ));
        }
    }
    Paragraph::new(lines)
        .style(Style::default().bg(app.theme.color_surface()))
        .render(area, buf);
}

fn clip_with_ellipsis(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let width = ch.width().unwrap_or(0);
        if used + width > max_width - 3 {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.push_str("...");
    out
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

pub fn render(buf: &mut Buffer, area: Rect, app: &mut AppState, ascii_cover: bool) {
    Block::default()
        .borders(Borders::ALL)
        .border_set(SOLID_BORDER)
        .style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        )
        .title(format!("Playlist ({} tracks)", app.playlist_view.len()))
        .render(area, buf);
    let layout = compute_layout(area, app);
    if ascii_cover {
        render_album_cover(buf, layout.cover_area, app);
    }
    render_separator(buf, layout.separator_area, app);
    render_playlist_list(buf, layout.list_area, app);
}
