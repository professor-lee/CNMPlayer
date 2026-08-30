use crate::app::{App, FlatPanel};
use crate::data::config::Language;
use crate::tmplayer::data::config::BarChannels;
use crate::tmplayer::render::spectrum_renderer::{compute_bar_layout, density_char, smooth_char};
use crate::ui::{page_lyrics, player_bar};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

pub fn draw_flat(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.clear_content_hits();
    app.clear_player_bar_hits();

    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(base_bg_style(app)), area);

    let player_h = player_bar::PLAYER_BAR_HEIGHT;
    if area.height > player_h {
        let upper = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height - player_h,
        };
        draw_lyrics_area(frame, app, upper);

        let player_area = Rect {
            x: area.x,
            y: area.y + upper.height,
            width: area.width,
            height: player_h,
        };
        player_bar::draw_collapsed_player_bar(frame, app, player_area);
        return;
    }

    // area.height == player_h：整个视口一次只显示一个面板，Alt+X 横向滑动切换。
    let offset = app.flat_switch_offset().round() as i32;
    let player_x = area.x as i32 - offset;
    let lyrics_x = area.x as i32 + area.width as i32 - offset;
    let player_rect = clipped_rect_at(area, player_x);
    let lyrics_rect = clipped_rect_at(area, lyrics_x);

    if player_rect.width > 0 {
        player_bar::draw_collapsed_player_bar(frame, app, player_rect);
    }
    if lyrics_rect.width > 0 {
        draw_switch_lyrics(frame, app, lyrics_rect);
    }

    if app.flat_switch_animating() || app.flat_panel != FlatPanel::Player {
        app.clear_player_bar_hits();
    }
}

pub fn draw_narrow(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.clear_content_hits();
    app.clear_player_bar_hits();

    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(base_bg_style(app)), area);

    let width = area.width as usize;
    let height = area.height as usize;
    if width == 0 || height == 0 {
        return;
    }

    let (bar_widths, gap, draw_total, x_offset) =
        compute_bar_layout(width, true, 1, BarChannels::Stereo);
    if draw_total == 0 {
        return;
    }

    let (left_level, right_level) = if draw_total >= 2 {
        (
            crate::app::lufs_to_bar_level(app.vu_left_lufs),
            crate::app::lufs_to_bar_level(app.vu_right_lufs),
        )
    } else {
        let left_ms = crate::app::lufs_to_mean_square(app.vu_left_lufs);
        let right_ms = crate::app::lufs_to_mean_square(app.vu_right_lufs);
        let mono = crate::app::mean_square_to_lufs((left_ms + right_ms) * 0.5);
        let level = crate::app::lufs_to_bar_level(mono);
        (level, level)
    };

    let mut grid = vec![vec![' '; width]; height];
    let mut x_cursor = x_offset.min(width);
    for (bar_index, &level) in [left_level, right_level][..draw_total].iter().enumerate() {
        if x_cursor >= width {
            break;
        }
        let bar_width = bar_widths.get(bar_index).copied().unwrap_or(1);
        fill_vertical_bar(
            &mut grid,
            height,
            x_cursor,
            bar_width,
            level,
            app.config.super_smooth_bar,
        );
        x_cursor = x_cursor.saturating_add(bar_width);
        if bar_index + 1 < draw_total {
            x_cursor = x_cursor.saturating_add(gap);
        }
    }

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for (row_index, row) in grid.iter().enumerate() {
        let t = if height <= 1 {
            1.0
        } else {
            row_index as f32 / (height - 1) as f32
        };
        let fg = mix(app.theme.color_accent2(), app.theme.color_accent3(), t);
        lines.push(Line::from(Span::styled(
            row.iter().collect::<String>(),
            Style::default().fg(fg),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_lyrics_area(frame: &mut Frame, app: &App, area: Rect) {
    if area.width >= 8 && area.height >= page_lyrics::PAGE_LYRICS_PANEL_HEIGHT {
        let panel_h = page_lyrics::PAGE_LYRICS_PANEL_HEIGHT.min(area.height);
        let panel = Rect {
            x: area.x,
            y: area.y + area.height - panel_h,
            width: area.width,
            height: panel_h,
        };
        page_lyrics::draw_page_lyrics_panel(frame, app, panel);
    } else {
        draw_compact_lyrics(frame, app, area);
    }
}

fn draw_switch_lyrics(frame: &mut Frame, app: &App, area: Rect) {
    if area.width >= 8 && area.height > page_lyrics::PAGE_LYRICS_PANEL_HEIGHT {
        let panel_h = page_lyrics::PAGE_LYRICS_PANEL_HEIGHT.min(area.height);
        let panel = Rect {
            x: area.x,
            y: area.y + area.height - panel_h,
            width: area.width,
            height: panel_h,
        };
        page_lyrics::draw_page_lyrics_panel(frame, app, panel);
    } else if area.width >= 8 && area.height == page_lyrics::PAGE_LYRICS_PANEL_HEIGHT {
        page_lyrics::draw_page_lyrics_panel(frame, app, area);
    } else {
        draw_compact_lyrics(frame, app, area);
    }
}

fn draw_compact_lyrics(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let (current, next) = app.current_page_lyric_lines();
    let (line1, line2) = if current.trim().is_empty() {
        match app.config.language {
            Language::Zh => ("暂无歌词".to_string(), String::new()),
            Language::En => ("No lyrics".to_string(), String::new()),
        }
    } else {
        (current, next)
    };

    let mut lines: Vec<Line> = Vec::new();
    if area.height >= 1 {
        lines.push(Line::from(Span::styled(
            line1,
            Style::default()
                .fg(app.theme.color_accent2())
                .add_modifier(Modifier::BOLD),
        )));
    }
    if area.height >= 2 && !line2.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            line2,
            Style::default().fg(app.theme.color_subtext()),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn clipped_rect_at(area: Rect, x: i32) -> Rect {
    let left = x.max(area.x as i32);
    let right = (x + area.width as i32).min(area.x as i32 + area.width as i32);
    if left >= right {
        return Rect::default();
    }
    Rect {
        x: left as u16,
        y: area.y,
        width: (right - left) as u16,
        height: area.height,
    }
}

fn fill_vertical_bar(
    grid: &mut [Vec<char>],
    height: usize,
    x: usize,
    bar_width: usize,
    level: f32,
    super_smooth: bool,
) {
    let level = level.clamp(0.0, 1.0);
    if level <= 0.0 {
        return;
    }

    for col in x..(x + bar_width).min(grid[0].len()) {
        if super_smooth {
            let fill = level * height as f32;
            let full = fill.floor().clamp(0.0, height as f32) as usize;
            let frac = fill - full as f32;
            for y in 0..height {
                let ch = if y < full {
                    '█'
                } else if y == full {
                    smooth_char(frac)
                } else {
                    ' '
                };
                let row = height - 1 - y;
                if ch != ' ' {
                    grid[row][col] = ch;
                }
            }
        } else {
            let bar_h = (level * height as f32).round() as usize;
            for y in 0..bar_h.min(height) {
                let row = height - 1 - y;
                grid[row][col] = density_char(y, bar_h.max(1));
            }
        }
    }
}

fn mix(start: Color, end: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (start, end) {
        (Color::Rgb(sr, sg, sb), Color::Rgb(er, eg, eb)) => {
            let r = (sr as f32 + (er as f32 - sr as f32) * t) as u8;
            let g = (sg as f32 + (eg as f32 - sg as f32) * t) as u8;
            let b = (sb as f32 + (eb as f32 - sb as f32) * t) as u8;
            Color::Rgb(r, g, b)
        }
        _ => end,
    }
}

fn base_bg_style(app: &App) -> Style {
    if app.config.transparent_background {
        Style::default()
    } else {
        Style::default().bg(app.theme.color_base())
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn density_bar_fills_from_bottom() {
        let mut grid = vec![vec![' '; 1]; 4];
        fill_vertical_bar(&mut grid, 4, 0, 1, 0.5, false);
        assert_eq!(grid[3][0], '█');
        assert_eq!(grid[2][0], '▒');
        assert_eq!(grid[1][0], ' ');
        assert_eq!(grid[0][0], ' ');
    }

    #[test]
    fn smooth_bar_uses_full_and_partial_chars() {
        let mut grid = vec![vec![' '; 1]; 4];
        fill_vertical_bar(&mut grid, 4, 0, 1, 0.5, true);
        assert_eq!(grid[3][0], '█');
        assert_eq!(grid[2][0], '█');
        assert_eq!(grid[1][0], ' ');
        assert_eq!(grid[0][0], ' ');
    }
}
