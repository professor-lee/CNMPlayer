use crate::data::config::BarChannels;
use crate::tmplayer::app::state::AppState;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;

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
    let bars_left = &app.spectrum.bars_left;
    let bars_right = &app.spectrum.bars_right;
    let mono_count = bars.len().max(1);
    let mut bar_widths = [0; 192];
    let (gap_width, draw_total, x_offset) = compute_bar_layout(
        w,
        app.config.bars_gap,
        mono_count,
        app.config.bar_channels,
        &mut bar_widths,
    );
    if draw_total == 0 {
        return;
    }

    let draw_vals = build_display_vals(
        bars,
        bars_left,
        bars_right,
        draw_total,
        app.config.bar_channels,
        app.config.bar_channel_reverse,
    );
    let buf = f.buffer_mut();
    // Paint directly into the reusable terminal buffer, without row strings.
    for row in 0..h {
        let t = if bars_h <= 1 {
            1.0
        } else {
            row.min(bars_h - 1) as f32 / (bars_h - 1) as f32
        };
        let fg = vertical_gradient_color(app, t);
        for x in 0..w {
            if let Some(cell) = buf.cell_mut((area.x + x as u16, area.y + row as u16)) {
                cell.set_char(if row == bars_h { '─' } else { ' ' });
                cell.set_fg(fg);
            }
        }
    }

    let mut x_cursor = x_offset.min(w);
    for (i, &val) in draw_vals[..draw_total.min(192)].iter().enumerate() {
        if x_cursor >= w {
            break;
        }
        let bar_width = bar_widths.get(i).copied().unwrap_or(1);
        let units = (val.clamp(0.0, 1.0) * bars_h as f32 * 8.0).floor() as usize;
        for y in 0..bars_h {
            let ch = spectrum_char(units, y);
            if ch == ' ' {
                break;
            }
            let row = bars_h - 1 - y;
            for x in x_cursor..(x_cursor + bar_width).min(w) {
                if let Some(cell) = buf.cell_mut((area.x + x as u16, area.y + row as u16)) {
                    cell.set_char(ch);
                }
            }
        }
        x_cursor = x_cursor.saturating_add(bar_width);
        if i + 1 < draw_total {
            x_cursor = x_cursor.saturating_add(gap_width);
        }
    }
}

pub(crate) fn compute_bar_layout(
    width: usize,
    gap: bool,
    data_len: usize,
    mode: BarChannels,
    widths: &mut [usize; 192],
) -> (usize, usize, usize) {
    if width == 0 {
        return (0, 0, 0);
    }
    let max_total = if gap {
        width.div_ceil(2).max(1)
    } else {
        (width / 2).max(1)
    };
    let mut bars = match mode {
        BarChannels::Mono => data_len,
        BarChannels::Stereo => data_len.saturating_mul(2),
    }
    .min(max_total)
    .min(widths.len())
    .max(1);
    if mode == BarChannels::Stereo && bars % 2 == 1 {
        bars = bars.saturating_sub(1).max(2);
    }
    loop {
        if !gap {
            let bar_w = width / bars;
            if bar_w >= 2 {
                widths[..bars].fill(bar_w);
                let mut remainder = width.saturating_sub(bars * bar_w);
                for item in &mut widths[..bars] {
                    if remainder == 0 {
                        break;
                    }
                    *item += 1;
                    remainder -= 1;
                }
                let used = widths[..bars].iter().sum::<usize>();
                return (0, bars, width.saturating_sub(used) / 2);
            }
        } else {
            let mut bar_w = width / bars;
            while bar_w >= 1 {
                let gap_w = bar_w.div_ceil(2);
                let needed = bars * bar_w + (bars.saturating_sub(1)) * gap_w;
                if needed <= width {
                    widths[..bars].fill(bar_w);
                    let mut remainder = width.saturating_sub(needed);
                    for item in &mut widths[..bars] {
                        if remainder == 0 {
                            break;
                        }
                        *item += 1;
                        remainder -= 1;
                    }
                    let used =
                        widths[..bars].iter().sum::<usize>() + (bars.saturating_sub(1)) * gap_w;
                    return (gap_w, bars, width.saturating_sub(used) / 2);
                }
                if bar_w == 1 {
                    break;
                }
                bar_w -= 1;
            }
        }
        if bars <= 1 {
            widths[0] = width.max(1);
            return (0, 1, 0);
        }
        bars -= 1;
    }
}

fn build_display_vals(
    mono: &[f32],
    left: &[f32],
    right: &[f32],
    draw_total: usize,
    mode: BarChannels,
    reverse: bool,
) -> [f32; 192] {
    let mut values = [0.0; 192];
    let draw_total = draw_total.min(values.len());
    if draw_total == 0 {
        return values;
    }
    match mode {
        BarChannels::Mono => {
            let data_len = mono.len().max(1);
            for (i, value) in values[..draw_total].iter_mut().enumerate() {
                let idx = if reverse { draw_total - 1 - i } else { i };
                *value = sample_val(mono, data_len, draw_total, idx);
            }
        }
        BarChannels::Stereo => {
            let per_side = (draw_total / 2).max(1);
            let left_len = left.len().max(1);
            let right_len = right.len().max(1);
            for i in 0..per_side {
                let idx = if reverse { per_side - 1 - i } else { i };
                values[i] = sample_val(left, left_len, per_side, per_side - 1 - idx);
                values[per_side + i] = sample_val(right, right_len, per_side, idx);
            }
        }
    }
    values
}

fn sample_val(data: &[f32], data_len: usize, draw_len: usize, i: usize) -> f32 {
    let idx =
        ((i as u32) * (data_len as u32) / (draw_len as u32)).min((data_len - 1) as u32) as usize;
    data.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0)
}

/// Cava uses eight subcell levels; the VU meter uses its fractional-cell glyph helper.
fn spectrum_char(units: usize, y: usize) -> char {
    const GLYPHS: [char; 8] = ['█', '▁', '▂', '▃', '▄', '▅', '▆', '▇'];
    let full = units / 8;
    let partial = units % 8;
    if y < full {
        GLYPHS[0]
    } else if y == full && partial != 0 {
        GLYPHS[partial]
    } else {
        ' '
    }
}

pub(crate) fn smooth_char(frac: f32) -> char {
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
    // Top -> bottom
    // Use the theme's accent range for a clear vertical gradient.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_display_uses_the_entire_area_without_mirroring() {
        let mono = [0.1, 0.2, 0.4, 0.9];
        let values = build_display_vals(&mono, &[1.0; 4], &[0.0; 4], 4, BarChannels::Mono, false);
        assert_eq!(values[..4], mono);
        let reverse = build_display_vals(&mono, &[], &[], 4, BarChannels::Mono, true);
        assert_eq!(reverse[..4], [0.9, 0.4, 0.2, 0.1]);
    }

    #[test]
    fn stereo_display_values_preserve_distinct_channels() {
        let mono = [0.0; 3];
        let left = [1.0, 0.0, 0.0];
        let right = [0.0, 0.0, 0.0];
        let values = build_display_vals(&mono, &left, &right, 6, BarChannels::Stereo, false);
        assert!(values[..3].iter().any(|value| *value > 0.0));
        assert!(values[3..6].iter().all(|value| *value == 0.0));
    }
}
