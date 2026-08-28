use crate::tmplayer::app::state::AppState;
use crate::tmplayer::data::config::BarChannels;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub fn render(f: &mut Frame, area: Rect, app: &mut AppState) {
    let h = area.height as usize;
    let w = area.width as usize;
    if h == 0 || w == 0 {
        return;
    }

    // Bottom hint line (leave at least 1 row above for bars when possible).
    let bars_h = h.saturating_sub(1);
    if bars_h == 0 {
        return;
    }

    let bars = &app.spectrum.bars;
    let mono_count = bars.len().max(1);
    if app.spectrum_render_grid.len() != bars_h {
        app.spectrum_render_grid.resize_with(bars_h, Vec::new);
    }
    for row in &mut app.spectrum_render_grid {
        if row.len() != w {
            row.resize(w, ' ');
        } else {
            row.fill(' ');
        }
    }

    let channels = app.effective_bar_channels();
    let (bar_widths, gap_width, draw_total, x_offset) =
        compute_bar_layout(w, app.config.bars_gap, mono_count, channels);
    if draw_total == 0 || bar_widths.is_empty() {
        return;
    }

    let draw_vals = build_display_vals(bars, draw_total, channels, app.config.bar_channel_reverse);
    let mut x_cursor = x_offset.min(w);
    for (i, &val) in draw_vals.iter().enumerate() {
        if x_cursor >= w {
            break;
        }
        let bar_width = bar_widths.get(i).copied().unwrap_or(1);
        let val = apply_height_curve(val);
        if app.config.super_smooth_bar {
            let fill = val * bars_h as f32;
            let full = fill.floor().clamp(0.0, bars_h as f32) as usize;
            let frac = (fill - full as f32).clamp(0.0, 1.0);

            for y in 0..bars_h {
                let row = bars_h - 1 - y;
                let ch = if y < full {
                    '█'
                } else if y == full {
                    smooth_char(frac)
                } else {
                    ' '
                };
                if ch != ' ' {
                    for x in x_cursor..(x_cursor + bar_width).min(w) {
                        app.spectrum_render_grid[row][x] = ch;
                    }
                }
            }
        } else {
            let bar_h = (val * bars_h as f32).round() as usize;
            for y in 0..bar_h.min(bars_h) {
                let row = bars_h - 1 - y;
                let ch = density_char(y, bar_h.max(1));
                for x in x_cursor..(x_cursor + bar_width).min(w) {
                    app.spectrum_render_grid[row][x] = ch;
                }
            }
        }

        x_cursor = x_cursor.saturating_add(bar_width);
        if i + 1 < draw_total {
            x_cursor = x_cursor.saturating_add(gap_width);
        }
    }

    // Render per-line vertical gradient using theme colors.
    let mut lines: Vec<Line> = Vec::with_capacity(bars_h + 1);
    for (row_idx, row) in app.spectrum_render_grid.iter().enumerate() {
        let t = if bars_h <= 1 {
            1.0
        } else {
            row_idx as f32 / (bars_h - 1) as f32
        };
        let fg = vertical_gradient_color(app, t);
        let s = row.iter().collect::<String>();
        lines.push(Line::from(Span::styled(s, Style::default().fg(fg))));
    }

    // bottom hint bar (same color as bars)
    let hint = "─".repeat(w);
    let fg = vertical_gradient_color(app, 1.0);
    lines.push(Line::from(Span::styled(hint, Style::default().fg(fg))));

    f.render_widget(Paragraph::new(lines), area);
}

fn compute_bar_layout(
    width: usize,
    gap: bool,
    data_len: usize,
    mode: BarChannels,
) -> (Vec<usize>, usize, usize, usize) {
    if width == 0 {
        return (Vec::new(), 0, 0, 0);
    }

    let mut desired_total = match mode {
        BarChannels::Mono => data_len,
        BarChannels::Stereo => data_len.saturating_mul(2),
    };

    // Enforce minimum width per bar when no gap.
    let max_total = if gap {
        width.div_ceil(2).max(1)
    } else {
        (width / 2).max(1)
    };
    if desired_total > max_total {
        desired_total = max_total;
    }
    if mode == BarChannels::Stereo && desired_total % 2 == 1 {
        desired_total = desired_total.saturating_sub(1).max(2);
    }

    let mut bars = desired_total.max(1);
    loop {
        if !gap {
            let bar_w = width / bars;
            if bar_w >= 2 {
                let used = bars * bar_w;
                let mut widths = vec![bar_w; bars];
                let mut remainder = width.saturating_sub(used);
                for w in &mut widths {
                    if remainder == 0 {
                        break;
                    }
                    *w += 1;
                    remainder -= 1;
                }
                let used = widths.iter().sum::<usize>();
                let offset = width.saturating_sub(used) / 2;
                return (widths, 0, bars, offset);
            }
        } else {
            let mut bar_w = width / bars;
            while bar_w >= 1 {
                let gap_w = bar_w.div_ceil(2);
                let needed = bars * bar_w + (bars.saturating_sub(1)) * gap_w;
                if needed <= width {
                    let mut widths = vec![bar_w; bars];
                    let mut remainder = width.saturating_sub(needed);
                    for w in &mut widths {
                        if remainder == 0 {
                            break;
                        }
                        *w += 1;
                        remainder -= 1;
                    }
                    let used = widths.iter().sum::<usize>() + (bars.saturating_sub(1)) * gap_w;
                    let offset = width.saturating_sub(used) / 2;
                    return (widths, gap_w, bars, offset);
                }
                if bar_w == 1 {
                    break;
                }
                bar_w -= 1;
            }
        }

        if bars <= 1 {
            let used = width.max(1);
            let offset = width.saturating_sub(used) / 2;
            return (vec![used], 0, 1, offset);
        }
        bars -= 1;
    }
}

fn build_display_vals(
    data: &[f32],
    draw_total: usize,
    mode: BarChannels,
    reverse: bool,
) -> Vec<f32> {
    let data_len = data.len().max(1);
    if draw_total == 0 {
        return Vec::new();
    }

    match mode {
        BarChannels::Mono => (0..draw_total)
            .map(|i| {
                if reverse {
                    sample_val(data, data_len, draw_total, draw_total - 1 - i)
                } else {
                    sample_val(data, data_len, draw_total, i)
                }
            })
            .collect(),
        BarChannels::Stereo => {
            let per_side = (draw_total / 2).max(1);
            let mut right: Vec<f32> = (0..per_side)
                .map(|i| {
                    if reverse {
                        sample_val(data, data_len, per_side, per_side - 1 - i)
                    } else {
                        sample_val(data, data_len, per_side, i)
                    }
                })
                .collect();
            let mut left = right.clone();
            left.reverse();
            left.append(&mut right);
            left
        }
    }
}

fn sample_val(data: &[f32], data_len: usize, draw_len: usize, i: usize) -> f32 {
    let idx =
        ((i as u32) * (data_len as u32) / (draw_len as u32)).min((data_len - 1) as u32) as usize;
    data.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0)
}

fn apply_height_curve(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    v.powf(0.72)
}

fn density_char(level: usize, height: usize) -> char {
    // bottom dense, top light
    if height == 0 {
        return ' ';
    }
    if height == 1 {
        return '░';
    }
    let ratio = level as f32 / height as f32;
    if ratio < 0.25 {
        '█'
    } else if ratio < 0.50 {
        '▓'
    } else if ratio < 0.75 {
        '▒'
    } else {
        '░'
    }
}

fn smooth_char(frac: f32) -> char {
    // Order: " ▂▃▄▅▆▇█" (low to high)
    if frac <= 0.0 {
        ' '
    } else if frac < 1.0 / 7.0 {
        '▂'
    } else if frac < 2.0 / 7.0 {
        '▃'
    } else if frac < 3.0 / 7.0 {
        '▄'
    } else if frac < 4.0 / 7.0 {
        '▅'
    } else if frac < 5.0 / 7.0 {
        '▆'
    } else if frac < 6.0 / 7.0 {
        '▇'
    } else {
        '█'
    }
}

fn vertical_gradient_color(app: &AppState, t: f32) -> Color {
    // Prefer the user's cava config [color] section (multi-stop gradient,
    // inherited exactly like the [input] section). Fall back to the theme's
    // accent range when cava defines no colors.
    if let Some(scheme) = crate::tmplayer::audio::cava::user_cava_color_scheme() {
        if let Some((r, g, b)) = scheme.color_at(t) {
            return Color::Rgb(r, g, b);
        }
    }
    let top = app.theme.color_accent2();
    let bottom = app.theme.color_accent3();
    mix(top, bottom, t)
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            let r = (ar as f32 + (br as f32 - ar as f32) * t) as u8;
            let g = (ag as f32 + (bg as f32 - ag as f32) * t) as u8;
            let b = (ab as f32 + (bb as f32 - ab as f32) * t) as u8;
            Color::Rgb(r, g, b)
        }
        _ => a,
    }
}
