use crate::data::config::GraphicsProtocol;
use crate::tmplayer::app::state::AppState;
use crate::tmplayer::app::state::{LocalFolderKind, Overlay};
use crate::tmplayer::render::cover_cache::CoverKey;
use crate::tmplayer::ui::borders::SOLID_BORDER;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::path::PathBuf;
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
        cover_rect: Rect {
            x: inner.x,
            y: inner.y,
            width: 0,
            height: 0,
        },
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

    let show_cover = app.local_view_album_cover.is_some();

    if !show_cover || inner.width < MIN_COVER_LAYOUT_WIDTH || inner.height < MIN_COVER_LAYOUT_HEIGHT
    {
        return list_only_layout(inner);
    }

    // Layout: cover (1/3) + 1-line separator + list (rest)
    let cover_h = ((inner.height as f32) / 3.0).round() as u16;
    let cover_h = cover_h.clamp(3, inner.height.saturating_sub(4));
    let sep_h = 1u16;
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
    let list_inner = list_area;

    PlaylistPanelLayout {
        inner,
        cover_area,
        cover_rect,
        separator_area,
        list_area,
        list_inner,
    }
}

fn cover_rect_in_area(area: Rect) -> Rect {
    // 视觉正方形：终端字符通常宽:高≈2:1
    let pad_h = 2u16;
    let avail_w = area.width.saturating_sub(pad_h.saturating_mul(2));
    let avail_h = area.height;

    let max_h_by_w = (avail_w / 2).max(1);
    let cover_h = avail_h.min(max_h_by_w).max(1);
    let cover_w = (cover_h.saturating_mul(2)).min(avail_w).max(2);

    let x = area.x + (area.width.saturating_sub(cover_w)) / 2;
    let y = area.y + (area.height.saturating_sub(cover_h)) / 2;
    Rect {
        x,
        y,
        width: cover_w,
        height: cover_h,
    }
}

fn render_album_cover(f: &mut Frame, area: Rect, app: &mut AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let cover = cover_rect_in_area(area);
    if cover.width == 0 || cover.height == 0 {
        return;
    }

    // Performance: during slide in/out, avoid expensive cover ASCII rendering.
    // Only render the real cover when the playlist overlay is fully expanded.
    let fully_expanded = app.overlay == Overlay::Playlist
        && app.playlist_slide_x == 0
        && app.playlist_slide_target_x == 0;

    let kitty_enabled = app.config.graphics_protocol != GraphicsProtocol::Off
        && app.local_view_album_cover.is_some();

    if kitty_enabled {
        // During expansion, show a solid theme color to avoid any heavy work.
        let bg = if let (Some(bytes), Some(hash)) = (
            app.local_view_album_cover.as_deref(),
            app.local_view_album_cover_hash,
        ) {
            app.cover_dominant_rgb(hash, bytes)
                .map(|(r, g, b)| Color::Rgb(r, g, b))
                .unwrap_or(app.theme.color_surface())
        } else {
            app.theme.color_surface()
        };
        let row = " ".repeat(cover.width as usize);
        let mut s = String::new();
        for _ in 0..cover.height {
            s.push_str(&row);
            s.push('\n');
        }
        f.render_widget(
            Paragraph::new(s)
                .style(Style::default().bg(bg))
                .wrap(Wrap { trim: false }),
            cover,
        );

        // Hot-switch support: pre-warm the ASCII cover cache while kitty is on.
        let bytes = app.local_view_album_cover.as_deref();
        let hash = app.local_view_album_cover_hash;
        let folder = app.local_view_album_folder.as_ref();
        warm_album_cover_ascii_cache(bytes, hash, folder, cover.width, cover.height, app, '█');

        // Keep the original prev/next hint bars in kitty mode.
        render_multi_album_hint_bars(f, area, cover, app);
        return;
    }

    if !fully_expanded {
        // Hide album cover while opening/closing; use a pure solid color placeholder.
        f.render_widget(
            Block::default().style(Style::default().bg(app.theme.color_surface())),
            cover,
        );
        render_multi_album_hint_bars(f, area, cover, app);
        return;
    }

    // Render slide animation when switching albums in MultiAlbum.
    if let Some(anim) = app.playlist_album_anim.take() {
        let now = app.last_frame;
        let p = (now.duration_since(anim.started_at).as_secs_f32() / anim.duration.as_secs_f32())
            .clamp(0.0, 1.0);
        let offset = (p * cover.width as f32).round() as i16;

        let from_ascii = album_cover_ascii(
            anim.from_cover.as_ref(),
            anim.from_hash,
            anim.from_folder.as_ref(),
            cover.width,
            cover.height,
            app,
            '█',
        );
        let to_ascii = album_cover_ascii(
            anim.to_cover.as_ref(),
            anim.to_hash,
            anim.to_folder.as_ref(),
            cover.width,
            cover.height,
            app,
            '█',
        );

        let composed = compose_slide_cover(
            cover.width,
            cover.height,
            &from_ascii,
            &to_ascii,
            anim.dir,
            offset,
        );
        f.render_widget(
            Paragraph::new(composed)
                .style(
                    Style::default()
                        .bg(app.theme.color_surface())
                        .fg(app.theme.color_text()),
                )
                .wrap(Wrap { trim: false }),
            cover,
        );

        // restore animation (lifetime managed in tick)
        app.playlist_album_anim = Some(anim);
    } else {
        let current_cover = app.local_view_album_cover.take();
        let current_hash = app.local_view_album_cover_hash;
        let current_folder = app.local_view_album_folder.clone();
        let ascii = album_cover_ascii(
            current_cover.as_ref(),
            current_hash,
            current_folder.as_ref(),
            cover.width,
            cover.height,
            app,
            '█',
        );
        app.local_view_album_cover = current_cover;
        f.render_widget(
            Paragraph::new(ascii)
                .style(
                    Style::default()
                        .bg(app.theme.color_surface())
                        .fg(app.theme.color_text()),
                )
                .wrap(Wrap { trim: false }),
            cover,
        );
    }

    render_multi_album_hint_bars(f, area, cover, app);
}

fn render_multi_album_hint_bars(f: &mut Frame, area: Rect, cover: Rect, app: &AppState) {
    // Multi-album prev/next hint bars
    if app.local_folder_kind != LocalFolderKind::MultiAlbum {
        return;
    }

    let h = cover.height;
    if h == 0 {
        return;
    }

    if app.local_view_album_index > 0 {
        // Stick to playlist border (inside)
        let left = Rect {
            x: area.x,
            y: cover.y,
            width: 1,
            height: h,
        };
        let s = (0..h).map(|_| "▒\n").collect::<String>();
        f.render_widget(
            Paragraph::new(s).style(
                Style::default()
                    .fg(app.theme.color_subtext())
                    .bg(app.theme.color_surface()),
            ),
            left,
        );
    }
    if app.local_view_album_index + 1 < app.local_album_folders.len() {
        // Stick to playlist border (inside)
        let right = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: cover.y,
            width: 1,
            height: h,
        };
        let s = (0..h).map(|_| "▒\n").collect::<String>();
        f.render_widget(
            Paragraph::new(s).style(
                Style::default()
                    .fg(app.theme.color_subtext())
                    .bg(app.theme.color_surface()),
            ),
            right,
        );
    }
}

fn render_separator(f: &mut Frame, area: Rect, app: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let line = "─".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(line).style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        ),
        area,
    );
}

fn render_playlist_list(f: &mut Frame, area: Rect, app: &AppState) {
    // Virtualized rendering to avoid lag on huge playlists.
    let footer_rows: u16 = 2;
    let list_rows = area.height.saturating_sub(footer_rows);

    let total = app.playlist_view.items.len();
    let selected = app.playlist_view.selected.min(total.saturating_sub(1));

    let visible = list_rows as usize;
    let mut start = 0usize;
    if visible > 0 && total > visible {
        // Keep selection within the visible window.
        if selected >= visible {
            start = selected + 1 - visible;
        }
        // Also clamp to tail.
        start = start.min(total - visible);
    }
    let end = if visible == 0 {
        0
    } else {
        (start + visible).min(total)
    };

    let mut lines: Vec<Line> = Vec::new();

    if total == 0 {
        lines.push(Line::styled(
            "(empty)",
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        ));
    } else {
        let max_w = area.width as usize;
        for i in start..end {
            let it = &app.playlist_view.items[i];
            let is_current = app.playlist_view.current == Some(i);
            let raw = format!("{:02}. {}", i + 1, it.title);
            let label = clip_with_ellipsis(&raw, max_w);
            let mut style = Style::default()
                .fg(app.theme.color_text())
                .bg(app.theme.color_surface());
            if i == app.playlist_view.selected {
                style = Style::default()
                    .fg(app.theme.color_base())
                    .bg(app.theme.color_accent())
                    .add_modifier(Modifier::BOLD);
            } else if is_current {
                style = style
                    .fg(app.theme.color_accent3())
                    .add_modifier(Modifier::BOLD);
            }
            lines.push(Line::styled(label, style));
        }
    }

    // No in-panel shortcut hint; see Keys modal.

    let p = Paragraph::new(lines).style(Style::default().bg(app.theme.color_surface()));
    f.render_widget(p, area);
}

fn clip_with_ellipsis(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }

    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    let budget = max_width - 3;
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push_str("...");
    out
}

fn album_cover_ascii(
    bytes: Option<&Vec<u8>>,
    hash: Option<u64>,
    folder: Option<&PathBuf>,
    width: u16,
    height: u16,
    app: &mut AppState,
    default_ch: char,
) -> String {
    if let (Some(bytes), Some(hash)) = (bytes, hash) {
        let key = CoverKey {
            hash,
            width,
            height,
        };
        let mut cache = app.cover_cache.borrow_mut();
        if let Some(s) = cache.get(key) {
            return s;
        }
        drop(cache);

        if let Some(folder) = folder {
            if let Some(s) = crate::tmplayer::playback::local_player::read_cover_ascii_cache(
                folder, hash, width, height,
            ) {
                app.cover_cache.borrow_mut().put(key, s.clone());
                return s;
            }
        }

        // Avoid heavy image resize + ASCII conversion on the UI thread.
        // Enqueue background render; show a placeholder this frame.
        // (The cache will be filled on a later tick.)
        app.queue_cover_ascii_render(key, bytes, default_ch, folder.cloned());
    }

    let row = default_ch.to_string().repeat(width as usize);
    let mut s = String::new();
    for _ in 0..height {
        s.push_str(&row);
        s.push('\n');
    }
    s
}

fn warm_album_cover_ascii_cache(
    bytes: Option<&[u8]>,
    hash: Option<u64>,
    folder: Option<&PathBuf>,
    width: u16,
    height: u16,
    app: &AppState,
    placeholder: char,
) {
    let (Some(bytes), Some(hash)) = (bytes, hash) else {
        return;
    };

    let key = CoverKey {
        hash,
        width,
        height,
    };
    if app.cover_cache.borrow().contains(key) {
        return;
    }

    if let Some(folder) = folder {
        if let Some(s) = crate::tmplayer::playback::local_player::read_cover_ascii_cache(
            folder, hash, width, height,
        ) {
            app.cover_cache.borrow_mut().put(key, s);
            return;
        }
    }

    app.queue_cover_ascii_render(key, bytes, placeholder, folder.cloned());
}

fn compose_slide_cover(
    width: u16,
    height: u16,
    from_ascii: &str,
    to_ascii: &str,
    dir: i8,
    offset: i16,
) -> String {
    let w = width as i16;
    let h = height as usize;

    let mut grid: Vec<Vec<char>> = vec![vec![' '; width as usize]; h];
    let from_lines = split_lines(from_ascii, h);
    let to_lines = split_lines(to_ascii, h);

    // Next: dir=-1, both move left. Prev: dir=+1, both move right.
    let (from_dx, to_dx) = if dir < 0 {
        (-offset, w - offset)
    } else {
        (offset, -w + offset)
    };

    blit(&mut grid, &from_lines, from_dx);
    blit(&mut grid, &to_lines, to_dx);

    let mut out = String::with_capacity((width as usize + 1) * h);
    for row in grid {
        out.extend(row);
        out.push('\n');
    }
    out
}

fn split_lines(s: &str, expected: usize) -> Vec<Vec<char>> {
    let mut out: Vec<Vec<char>> = Vec::with_capacity(expected);
    for line in s.lines() {
        out.push(line.chars().collect());
        if out.len() == expected {
            break;
        }
    }
    while out.len() < expected {
        out.push(Vec::new());
    }
    out
}

fn blit(dst: &mut [Vec<char>], src: &[Vec<char>], dx: i16) {
    let h = dst.len().min(src.len());
    if h == 0 {
        return;
    }
    let w = dst[0].len() as i16;
    for y in 0..h {
        for (x_src, ch) in src[y].iter().enumerate() {
            let x = x_src as i16 + dx;
            if x >= 0 && x < w {
                dst[y][x as usize] = *ch;
            }
        }
    }
}

pub fn render(f: &mut Frame, area: Rect, app: &mut AppState) {
    // solid background for playlist overlay
    f.render_widget(ratatui::widgets::Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(SOLID_BORDER)
        .style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        )
        .title(format!("Playlist ({} tracks)", app.playlist_view.len()));
    f.render_widget(block, area);

    let l = compute_layout(area, app);
    render_album_cover(f, l.cover_area, app);
    render_separator(f, l.separator_area, app);
    render_playlist_list(f, l.list_area, app);
}
