use super::super::layout::centered_rect;
use crate::tmplayer::app::state::AppState;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

pub(in crate::tmplayer::ui::tui) fn render_about_modal(f: &mut ratatui::Frame, size: Rect, app: &mut AppState) {
    let area = centered_rect(size, 70, 22);
    f.render_widget(ratatui::widgets::Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(crate::tmplayer::ui::borders::SOLID_BORDER)
        .title(" about ")
        .style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        );
    f.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(inner);

    render_about_braille(f, chunks[0], app);
    render_about_text(f, chunks[1], app);

    let info = crate::tmplayer::data::about::about_info();
    let version = format!("v{}", info.version);
    let y = area.y + area.height.saturating_sub(1);
    let version_area = Rect {
        x: area.x.saturating_add(1),
        y,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    f.render_widget(
        Paragraph::new(version).alignment(Alignment::Center).style(
            Style::default()
                .fg(app.theme.color_subtext())
                .bg(app.theme.color_surface()),
        ),
        version_area,
    );
}

fn render_about_braille(f: &mut ratatui::Frame, area: Rect, app: &mut AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let lines = about_braille_lines(area.width as usize, area.height as usize);
    let p = Paragraph::new(lines).style(
        Style::default()
            .fg(app.theme.color_text())
            .bg(app.theme.color_surface()),
    );
    f.render_widget(p, area);
}

fn render_about_text(f: &mut ratatui::Frame, area: Rect, app: &mut AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let info = crate::tmplayer::data::about::about_info();

    let max_width = area.width as usize;
    if max_width == 0 {
        return;
    }

    let mut rendered: Vec<String> = Vec::new();
    for (k, v) in &info.links {
        let line = if k.eq_ignore_ascii_case("github_url") {
            v.to_string()
        } else {
            format!("{}: {}", k, v)
        };
        rendered.extend(wrap_text(&line, max_width));
    }

    let desc_lines: Vec<String> = wrap_text(&info.description, max_width);
    if !desc_lines.is_empty() {
        rendered.push(String::new());
        rendered.extend(desc_lines);
    }

    let max_line_width = rendered
        .iter()
        .map(|l| unicode_width::UnicodeWidthStr::width(l.as_str()))
        .max()
        .unwrap_or(0)
        .min(max_width);
    let block_h = rendered.len() as u16;
    let block_w = max_line_width.max(1) as u16;
    let offset_x = (area.width.saturating_sub(block_w)) / 2;
    let offset_y = if block_h <= area.height {
        (area.height.saturating_sub(block_h)) / 2
    } else {
        0
    };

    let lines: Vec<Line> = rendered
        .into_iter()
        .map(|l| {
            Line::styled(
                l,
                Style::default()
                    .fg(app.theme.color_text())
                    .bg(app.theme.color_surface()),
            )
        })
        .collect();
    let p = Paragraph::new(lines)
        .style(Style::default().bg(app.theme.color_surface()))
        .wrap(Wrap { trim: false });
    let text_h = area.height.saturating_sub(offset_y).min(block_h.max(1));
    let text_area = Rect {
        x: area.x + offset_x,
        y: area.y + offset_y,
        width: block_w,
        height: text_h,
    };
    f.render_widget(p, text_area);
}

fn wrap_text(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    if s.is_empty() {
        return vec![String::new()];
    }

    // Wrap on display columns, not char count: CJK text is twice as wide as it
    // is long, and counting chars would clip it.
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut buf_width = 0usize;
    for ch in s.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if buf_width + ch_width > width && !buf.is_empty() {
            out.push(std::mem::take(&mut buf));
            buf_width = 0;
        }
        buf.push(ch);
        buf_width += ch_width;
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

fn about_braille_lines(width: usize, height: usize) -> Vec<Line<'static>> {
    let blank = " ".repeat(width);
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let info = crate::tmplayer::data::about::about_info();
    let Some(selected) = select_about_braille_art(width, height, &info.braille_images) else {
        return (0..height).map(|_| Line::from(blank.clone())).collect();
    };

    let mut rows: Vec<String> = selected
        .art
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect();

    let mut start = 0usize;
    let mut end = rows.len();
    while start < end && rows[start].trim().is_empty() {
        start += 1;
    }
    while end > start && rows[end - 1].trim().is_empty() {
        end -= 1;
    }
    rows = rows[start..end].to_vec();

    let rows_w = rows
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let canvas_w = selected.width.max(rows_w);
    let canvas_h = selected.height.max(rows.len());

    let offset_x = width.saturating_sub(canvas_w) / 2;
    let offset_y = height.saturating_sub(canvas_h) / 2;
    let mut grid: Vec<Vec<char>> = vec![vec![' '; width]; height];

    for (row_idx, row) in rows.iter().enumerate() {
        let gy = offset_y + row_idx;
        if gy >= height {
            break;
        }
        for (col_idx, ch) in row.chars().enumerate() {
            let gx = offset_x + col_idx;
            if gx >= width {
                break;
            }
            grid[gy][gx] = ch;
        }
    }

    grid.into_iter()
        .map(|row| Line::from(row.into_iter().collect::<String>()))
        .collect()
}

fn select_about_braille_art(
    width: usize,
    height: usize,
    arts: &[crate::tmplayer::data::about::BrailleImage],
) -> Option<&crate::tmplayer::data::about::BrailleImage> {
    let mut best_fit: Option<(&crate::tmplayer::data::about::BrailleImage, u128)> = None;
    for art in arts {
        if art.width == 0 || art.height == 0 {
            continue;
        }
        if art.width <= width && art.height <= height {
            let score = (art.width as u128) * (art.height as u128);
            let should_replace = best_fit
                .as_ref()
                .map(|(_, best_score)| score > *best_score)
                .unwrap_or(true);
            if should_replace {
                best_fit = Some((art, score));
            }
        }
    }

    if let Some((art, _)) = best_fit {
        return Some(art);
    }

    arts.iter()
        .filter(|art| art.width > 0 && art.height > 0)
        .min_by_key(|art| {
            let dw = art.width.saturating_sub(width) as u128;
            let dh = art.height.saturating_sub(height) as u128;
            let overflow = dw.saturating_mul(dh).saturating_add(dw).saturating_add(dh);
            let area = (art.width as u128).saturating_mul(art.height as u128);
            (overflow, area)
        })
}
