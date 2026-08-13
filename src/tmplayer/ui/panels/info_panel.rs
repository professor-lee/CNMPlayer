use crate::data::config::GraphicsProtocol;
use crate::tmplayer::app::state::{AppState, CoverSnapshot, Overlay, PlayMode};
use crate::tmplayer::render::cover_cache::CoverKey;
use crate::tmplayer::ui::borders::SOLID_BORDER;
use crate::tmplayer::ui::components::{control_buttons, progress_bar, volume_bar};
use crate::tmplayer::utils::timefmt;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Default, Clone, Copy)]
pub struct InfoPanelLayout {
    pub inner: Rect,
    pub cover: Rect,
    pub meta: Rect,
    pub progress: Rect,
    pub volume: Rect,
    pub controls: Rect,
    pub volume_label: Rect,
    pub time_line: Rect,
}

pub fn layout(area: Rect) -> InfoPanelLayout {
    // Keep borders outside and reserve an inner content area.
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 2,
    });

    // Required rows in priority order (must survive resize as long as possible):
    // 1) metadata (3 lines) 2) progress 3) volume 4) controls
    const META_H: u16 = 3;
    const PROGRESS_H: u16 = 1;
    const VOLUME_H: u16 = 1;
    const CONTROLS_H: u16 = 1;
    const CORE_H: u16 = META_H + PROGRESS_H + VOLUME_H + CONTROLS_H;

    // Secondary rows can be dropped before affecting core rows.
    let show_time_line = inner.height >= CORE_H.saturating_add(1);
    let show_volume_label = inner.height >= CORE_H.saturating_add(2);
    let time_h = if show_time_line { 1 } else { 0 };
    let volume_label_h = if show_volume_label { 1 } else { 0 };

    // Remaining height is for cover + an optional gap below cover.
    let used_without_cover = CORE_H.saturating_add(time_h).saturating_add(volume_label_h);
    let mut cover_h = inner.height.saturating_sub(used_without_cover);
    let use_cover_gap = cover_h > 1;
    if use_cover_gap {
        cover_h = cover_h.saturating_sub(1);
    }

    // Cover should shrink first. Clamp by panel width and keep as low as 0 for tiny windows.
    let max_cover_h_by_width = inner.width / 2;
    cover_h = cover_h.min(max_cover_h_by_width);
    let cover_w = if cover_h == 0 {
        0
    } else {
        (cover_h.saturating_mul(2)).min(inner.width)
    };

    let stack_h = cover_h
        .saturating_add(if use_cover_gap && cover_h > 0 { 1 } else { 0 })
        .saturating_add(used_without_cover);
    let top_pad = inner.height.saturating_sub(stack_h) / 2;
    let mut y = inner.y.saturating_add(top_pad);

    let cover = Rect {
        x: if cover_w > 0 {
            inner.x + (inner.width.saturating_sub(cover_w)) / 2
        } else {
            inner.x
        },
        y,
        width: cover_w,
        height: cover_h,
    };

    y = y.saturating_add(cover_h);
    if use_cover_gap && cover_h > 0 {
        y = y.saturating_add(1);
    }

    let meta = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: META_H.min(inner.height.saturating_sub(y.saturating_sub(inner.y))),
    };
    y = y.saturating_add(meta.height);

    let time_line = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: time_h,
    };
    y = y.saturating_add(time_h);

    let progress = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: PROGRESS_H,
    };
    y = y.saturating_add(PROGRESS_H);

    let volume = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: VOLUME_H,
    };
    y = y.saturating_add(VOLUME_H);

    let volume_label = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: volume_label_h,
    };
    y = y.saturating_add(volume_label_h);

    let controls = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: CONTROLS_H,
    };

    InfoPanelLayout {
        inner,
        cover,
        meta,
        progress,
        volume,
        controls,
        volume_label,
        time_line,
    }
}

pub fn render(f: &mut Frame, area: Rect, app: &mut AppState) {
    let b = Block::default()
        .borders(Borders::ALL)
        .border_set(SOLID_BORDER)
        .title(" ")
        .style(Style::default().fg(app.theme.color_subtext()));
    f.render_widget(b, area);

    let l = layout(area);

    // cover (animated as a whole: content + border)
    if l.cover.width > 0 && l.cover.height > 0 {
        let show_border = app.config.album_border;

        let kitty_enabled = app.config.graphics_protocol != GraphicsProtocol::Off
            && app.player.track.cover.is_some();

        let dominant_bg = if let (Some(bytes), Some(hash)) = (
            app.player.track.cover.as_deref(),
            app.player.track.cover_hash,
        ) {
            app.cover_dominant_rgb(hash, bytes)
                .map(|(r, g, b)| Color::Rgb(r, g, b))
                .unwrap_or(app.theme.color_surface())
        } else {
            app.theme.color_surface()
        };

        // Playlist overlay (including slide animation) should hide the song cover only in
        // kitty mode (otherwise the overlay will naturally cover the ASCII render).
        let playlist_overlay_visible =
            app.overlay == Overlay::Playlist || app.playlist_slide_x != app.playlist_slide_target_x;

        if kitty_enabled {
            if playlist_overlay_visible {
                // Pure color placeholder (keep border option).
                let bg = dominant_bg;
                if show_border {
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_set(SOLID_BORDER)
                        .style(Style::default().fg(app.theme.color_subtext()));
                    f.render_widget(block, l.cover);
                    let inner = l.cover.inner(ratatui::layout::Margin {
                        horizontal: 1,
                        vertical: 1,
                    });
                    if inner.width > 0 && inner.height > 0 {
                        f.render_widget(Block::default().style(Style::default().bg(bg)), inner);
                    }
                } else {
                    let inner = l.cover.inner(ratatui::layout::Margin {
                        horizontal: 1,
                        vertical: 1,
                    });
                    if inner.width > 0 && inner.height > 0 {
                        f.render_widget(Block::default().style(Style::default().bg(bg)), inner);
                    }
                }

                // Pre-warm the ASCII cover cache while hidden so closing playlist is instant.
                let snap = CoverSnapshot::from(&app.player.track);
                let (inner_w, inner_h) = if l.cover.width >= 3 && l.cover.height >= 3 {
                    (
                        l.cover.width.saturating_sub(2),
                        l.cover.height.saturating_sub(2),
                    )
                } else {
                    (l.cover.width, l.cover.height)
                };
                let _ = cover_ascii_for_snapshot(&snap, inner_w, inner_h, app);
            } else {
                // Draw border (optional) and keep the inside blank; the real image is painted
                // after ratatui draw via kitty graphics protocol.
                if show_border {
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_set(SOLID_BORDER)
                        .style(Style::default().fg(app.theme.color_subtext()));
                    f.render_widget(block, l.cover);
                    let inner = l.cover.inner(ratatui::layout::Margin {
                        horizontal: 1,
                        vertical: 1,
                    });
                    if inner.width > 0 && inner.height > 0 {
                        f.render_widget(
                            Paragraph::new(" ").style(Style::default().bg(dominant_bg)),
                            inner,
                        );
                    }
                } else {
                    let inner = l.cover.inner(ratatui::layout::Margin {
                        horizontal: 1,
                        vertical: 1,
                    });
                    if inner.width > 0 && inner.height > 0 {
                        f.render_widget(
                            Paragraph::new(" ").style(Style::default().bg(dominant_bg)),
                            inner,
                        );
                    }
                }

                // Hot-switch support: while kitty is on, pre-warm the ASCII cover in the background
                // (or load it from .order.toml) so turning kitty off in Settings is instant.
                let snap = CoverSnapshot::from(&app.player.track);
                let (inner_w, inner_h) = if l.cover.width >= 3 && l.cover.height >= 3 {
                    (
                        l.cover.width.saturating_sub(2),
                        l.cover.height.saturating_sub(2),
                    )
                } else {
                    (l.cover.width, l.cover.height)
                };
                let _ = cover_ascii_for_snapshot(&snap, inner_w, inner_h, app);
            }
        } else {
            // ASCII mode: do not actively hide the song cover when playlist opens.
            // The playlist overlay is rendered later and naturally covers it.
            if let Some(anim) = app.cover_anim.take() {
                let p = (app.last_frame.duration_since(anim.started_at).as_secs_f32()
                    / anim.duration.as_secs_f32())
                .clamp(0.0, 1.0);
                let offset = (p * l.cover.width as f32).round() as i16;

                let (from_box, from_fg) = cover_box_ascii_for_snapshot(
                    &anim.from,
                    l.cover.width,
                    l.cover.height,
                    show_border,
                    app,
                );
                let (to_box, to_fg) = cover_box_ascii_for_snapshot(
                    &anim.to,
                    l.cover.width,
                    l.cover.height,
                    show_border,
                    app,
                );

                let composed = compose_slide_cover(
                    l.cover.width,
                    l.cover.height,
                    &from_box,
                    &to_box,
                    anim.dir,
                    offset,
                );
                let fg = if to_fg == app.theme.color_text() {
                    to_fg
                } else {
                    from_fg
                };
                f.render_widget(
                    Paragraph::new(composed).style(Style::default().fg(fg)),
                    l.cover,
                );

                // restore animation (lifetime managed in tick)
                app.cover_anim = Some(anim);
            } else {
                let snap = CoverSnapshot::from(&app.player.track);
                let (box_ascii, fg) = cover_box_ascii_for_snapshot(
                    &snap,
                    l.cover.width,
                    l.cover.height,
                    show_border,
                    app,
                );
                f.render_widget(
                    Paragraph::new(box_ascii).style(Style::default().fg(fg)),
                    l.cover,
                );
            }
        }
    }

    // metadata + controls/progress/volume: prioritized content for small windows.
    if l.meta.height >= 1
        && l.progress.height >= 1
        && l.volume.height >= 1
        && l.controls.height >= 1
    {
        let title = app.player.track.title.as_str();
        let artist = app.player.track.artist.as_str();
        let album = app.player.track.album.as_str();
        let heart = if app.player.liked { "" } else { "" };

        let text_style = Style::default().fg(app.theme.color_text());
        let sub_style = Style::default().fg(app.theme.color_subtext());

        let meta_rect = Rect {
            x: l.meta.x,
            y: l.meta.y,
            width: l.meta.width,
            height: 1,
        };

        let title_line = compose_left_right_line(title, heart, meta_rect.width as usize);
        let t = Paragraph::new(Line::from(vec![Span::styled(title_line, text_style)]))
            .alignment(Alignment::Left);
        f.render_widget(t, meta_rect);

        let a = Paragraph::new(clip_to_display_width(artist, meta_rect.width as usize))
            .style(sub_style)
            .alignment(Alignment::Left);
        if l.meta.height >= 2 {
            f.render_widget(
                a,
                Rect {
                    x: meta_rect.x,
                    y: l.meta.y + 1,
                    width: meta_rect.width,
                    height: 1,
                },
            );
        }
        let al = Paragraph::new(clip_to_display_width(album, meta_rect.width as usize))
            .style(sub_style)
            .alignment(Alignment::Left);
        if l.meta.height >= 3 {
            f.render_widget(
                al,
                Rect {
                    x: meta_rect.x,
                    y: l.meta.y + 2,
                    width: meta_rect.width,
                    height: 1,
                },
            );
        }

        // time + progress
        let pos = app.player.position;
        let dur = app.player.track.duration;
        let left = timefmt::mmss(pos);
        let right = timefmt::mmss(dur);
        if l.time_line.height > 0 {
            let time_line = format!(
                "{}{:>width$}",
                left,
                right,
                width = (l.inner.width as usize).saturating_sub(left.len())
            );
            f.render_widget(
                Paragraph::new(Line::from(time_line))
                    .style(sub_style)
                    .alignment(Alignment::Center),
                l.time_line,
            );
        }

        progress_bar::render(f, l.progress, app, pos, dur);
        volume_bar::render(f, l.volume, app, app.player.volume);

        if l.volume_label.height > 0 {
            let v_label = format!("Vol {}%", (app.player.volume * 100.0).round() as i32);
            f.render_widget(
                Paragraph::new(v_label)
                    .style(sub_style)
                    .alignment(Alignment::Left),
                l.volume_label,
            );
        }

        control_buttons::render(f, l.controls, app);

        // (Removed S/R hint)
    }

    // header right status (theme + mode)
    let mode_key = match app.language {
        crate::data::config::Language::Zh => "模式",
        crate::data::config::Language::En => "Mode",
    };
    let header = format!(
        "[{}]  [{}: {}]",
        app.theme.name.as_label(),
        mode_key,
        mode_label(app.player.mode, app.language)
    );
    let header_area = Rect {
        x: area.x + 2,
        y: area.y,
        width: area.width.saturating_sub(4),
        height: 1,
    };
    f.render_widget(
        Paragraph::new(header)
            .style(Style::default().fg(app.theme.color_subtext()))
            .alignment(Alignment::Right)
            .wrap(Wrap { trim: true }),
        header_area,
    );
}

fn cover_box_ascii_for_snapshot(
    snap: &CoverSnapshot,
    width: u16,
    height: u16,
    show_border: bool,
    app: &mut AppState,
) -> (String, ratatui::style::Color) {
    if width == 0 || height == 0 {
        return (String::new(), app.theme.color_subtext());
    }

    let mut grid: Vec<Vec<char>> = vec![vec![' '; width as usize]; height as usize];

    let (inner_x, inner_y, inner_w, inner_h) = if width >= 3 && height >= 3 {
        if show_border {
            // Border
            let tl = SOLID_BORDER.top_left.chars().next().unwrap_or(' ');
            let tr = SOLID_BORDER.top_right.chars().next().unwrap_or(' ');
            let bl = SOLID_BORDER.bottom_left.chars().next().unwrap_or(' ');
            let br = SOLID_BORDER.bottom_right.chars().next().unwrap_or(' ');
            let hch = SOLID_BORDER.horizontal_top.chars().next().unwrap_or(' ');
            let vl = SOLID_BORDER.vertical_left.chars().next().unwrap_or(' ');
            let vr = SOLID_BORDER.vertical_right.chars().next().unwrap_or(' ');

            grid[0][0] = tl;
            grid[0][(width - 1) as usize] = tr;
            grid[(height - 1) as usize][0] = bl;
            grid[(height - 1) as usize][(width - 1) as usize] = br;

            for x in 1..(width - 1) {
                grid[0][x as usize] = hch;
                grid[(height - 1) as usize][x as usize] = hch;
            }
            for y in 1..(height - 1) {
                grid[y as usize][0] = vl;
                grid[y as usize][(width - 1) as usize] = vr;
            }
        }

        // Always reserve the same inner content area, even when border is hidden.
        (1usize, 1usize, (width - 2) as usize, (height - 2) as usize)
    } else {
        // Too small to reserve padding; render full area.
        (0usize, 0usize, width as usize, height as usize)
    };

    let (inner_ascii, fg) = cover_ascii_for_snapshot(snap, inner_w as u16, inner_h as u16, app);
    let inner_lines = split_lines(&inner_ascii, inner_h);
    blit_xy(&mut grid, &inner_lines, inner_x as i16, inner_y as i16);

    let mut out = String::with_capacity((width as usize + 1) * height as usize);
    for row in grid {
        out.extend(row);
        out.push('\n');
    }
    (out, fg)
}

fn hash_snapshot_seed(s: &CoverSnapshot) -> u64 {
    let mut h = DefaultHasher::new();
    s.title.hash(&mut h);
    s.artist.hash(&mut h);
    s.album.hash(&mut h);
    h.finish()
}

fn cover_ascii_for_snapshot(
    snap: &CoverSnapshot,
    width: u16,
    height: u16,
    app: &mut AppState,
) -> (String, ratatui::style::Color) {
    if let (Some(bytes), Some(hash)) = (snap.cover.as_deref(), snap.cover_hash) {
        let key = CoverKey {
            hash,
            width,
            height,
        };
        let cached = { app.cover_cache.borrow_mut().get(key) };
        let ascii = match cached {
            Some(s) => s,
            None => {
                if let Some(folder) = snap.cover_folder.as_deref() {
                    if let Some(s) = crate::tmplayer::playback::local_player::read_cover_ascii_cache(
                        folder, hash, width, height,
                    ) {
                        app.cover_cache.borrow_mut().put(key, s.clone());
                        return (s, app.theme.color_text());
                    }
                }
                // Avoid heavy render on UI thread; enqueue background render and
                // return a cheap placeholder for this frame.
                app.queue_cover_ascii_render(key, bytes, '░', snap.cover_folder.clone());
                fill_ascii(width, height, '░')
            }
        };
        (ascii, app.theme.color_text())
    } else {
        let seed = hash_snapshot_seed(snap);
        let key = CoverKey {
            hash: seed,
            width,
            height,
        };
        let cached = { app.cover_cache.borrow_mut().get(key) };
        let ascii = match cached {
            Some(s) => s,
            None => {
                let s = generate_random_cover_ascii(width, height, seed);
                {
                    let mut cache = app.cover_cache.borrow_mut();
                    cache.put(key, s);
                    cache.get(key).unwrap_or_default()
                }
            }
        };
        (ascii, app.theme.color_subtext())
    }
}

fn fill_ascii(width: u16, height: u16, ch: char) -> String {
    let row = ch.to_string().repeat(width as usize);
    let mut s = String::new();
    for _ in 0..height {
        s.push_str(&row);
        s.push('\n');
    }
    s
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

fn blit_xy(dst: &mut [Vec<char>], src: &[Vec<char>], dx: i16, dy: i16) {
    let dst_h = dst.len() as i16;
    if dst_h == 0 {
        return;
    }
    let dst_w = dst[0].len() as i16;
    if dst_w == 0 {
        return;
    }

    for (y_src, row) in src.iter().enumerate() {
        let y = y_src as i16 + dy;
        if y < 0 || y >= dst_h {
            continue;
        }
        for (x_src, ch) in row.iter().enumerate() {
            let x = x_src as i16 + dx;
            if x >= 0 && x < dst_w {
                dst[y as usize][x as usize] = *ch;
            }
        }
    }
}

fn generate_random_cover_ascii(width: u16, height: u16, seed: u64) -> String {
    // Requirement: when the app has no response / no album cover available,
    // use a consistent solid fill instead of random characters.
    let _ = seed;
    let w = width as usize;
    let h = height as usize;
    let mut out = String::with_capacity((w + 1) * h);
    for _y in 0..h {
        for _x in 0..w {
            out.push('░');
        }
        out.push('\n');
    }
    out
}

fn clip_to_display_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > max_width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

fn compose_left_right_line(left: &str, right: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let right_w = right.width().min(width);
    let left_max = width.saturating_sub(right_w + 1);
    let left_text = clip_to_display_width(left, left_max);
    let used = left_text.width() + right_w;
    let pad = width.saturating_sub(used);

    format!("{left_text}{}{right}", " ".repeat(pad))
}

fn mode_label(m: PlayMode, lang: crate::data::config::Language) -> &'static str {
    match (m, lang) {
        (PlayMode::Idle, crate::data::config::Language::Zh) => "网络",
        (PlayMode::Idle, crate::data::config::Language::En) => "Network",
    }
}
