use crate::tmplayer::app::state::AppState;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

const BINS: usize = 64;
const F_MIN_HZ: f32 = 40.0;
const F_MAX_HZ: f32 = 8000.0;
const GAMMA: f32 = 1.7;
const DISPLAY_WINDOW_SEC: f32 = 0.030;
const GAIN: f32 = 0.90;

pub fn render(f: &mut Frame, area: Rect, app: &AppState) {
    let w_cells = area.width as usize;
    let h_cells = area.height as usize;
    if w_cells == 0 || h_cells == 0 {
        return;
    }

    // Braille resolution: 2x4 pixels per terminal cell.
    let w_px = w_cells * 2;
    let h_px = h_cells * 4;
    if w_px == 0 || h_px == 0 {
        return;
    }

    let mid_y = ((h_px as i32) - 1) / 2;
    let (y_left, y_right) = synthesize_waveforms(app, w_px, mid_y);

    let cell_bits = rasterize_braille(w_cells, h_cells, &y_left, &y_right);

    // Render per-line vertical gradient using theme colors.
    let mut lines: Vec<Line> = Vec::with_capacity(h_cells);
    for row in 0..h_cells {
        let t = if h_cells <= 1 {
            1.0
        } else {
            row as f32 / (h_cells - 1) as f32
        };
        let fg = vertical_gradient_color(app, t);
        let mut s = String::with_capacity(w_cells);
        let base = row * w_cells;
        for col in 0..w_cells {
            let bits = cell_bits[base + col];
            s.push(braille_from_bits(bits));
        }
        lines.push(Line::from(Span::styled(s, Style::default().fg(fg))));
    }

    f.render_widget(Paragraph::new(lines), area);
}

pub fn advance_phases(phases: &mut [f32; BINS], dt_sec: f32) {
    if dt_sec <= 0.0 {
        return;
    }
    let dt_sec = dt_sec.clamp(1.0 / 240.0, 1.0 / 5.0);
    for k in 0..BINS {
        let f = freq_for_bin(k);
        phases[k] = wrap_tau(phases[k] + std::f32::consts::TAU * f * dt_sec);
    }
}

fn synthesize_waveforms(app: &AppState, w_px: usize, mid_y: i32) -> (Vec<i32>, Vec<i32>) {
    // Use a subset of bins for performance while keeping the look.
    let bin_step = 2usize;

    let mut out_left: Vec<i32> = Vec::with_capacity(w_px);
    let mut out_right: Vec<i32> = Vec::with_capacity(w_px);

    let denom_left = amplitude_denominator(&app.spectrum.stereo_left, bin_step);
    let denom_right = amplitude_denominator(&app.spectrum.stereo_right, bin_step);

    for x in 0..w_px {
        let t = (x as f32 / (w_px.saturating_sub(1).max(1) as f32)) * DISPLAY_WINDOW_SEC;

        let y_l = synth_channel(
            &app.spectrum.stereo_left,
            &app.spectrum.osc_phase_left,
            denom_left,
            t,
            bin_step,
        );
        let y_r = synth_channel(
            &app.spectrum.stereo_right,
            &app.spectrum.osc_phase_right,
            denom_right,
            t,
            bin_step,
        );

        out_left.push(map_to_pixel_row(y_l, mid_y));
        out_right.push(map_to_pixel_row(y_r, mid_y));
    }

    (out_left, out_right)
}

fn amplitude_denominator(vals: &[f32; BINS], bin_step: usize) -> f32 {
    let mut sum = 0.0f32;
    for k in (0..BINS).step_by(bin_step) {
        sum += vals[k].clamp(0.0, 1.0).powf(GAMMA);
    }
    sum.max(1e-3)
}

fn synth_channel(
    vals: &[f32; BINS],
    phases: &[f32; BINS],
    denom: f32,
    t: f32,
    bin_step: usize,
) -> f32 {
    let mut acc = 0.0f32;
    for k in (0..BINS).step_by(bin_step) {
        let a = vals[k].clamp(0.0, 1.0).powf(GAMMA);
        if a <= 0.0 {
            continue;
        }
        let f = freq_for_bin(k);
        let theta = std::f32::consts::TAU * f * t + phases[k];
        acc += a * theta.sin();
    }

    // Normalize to roughly [-1, 1].
    (acc / denom).clamp(-1.0, 1.0)
}

fn map_to_pixel_row(y: f32, mid_y: i32) -> i32 {
    let span = (mid_y as f32).max(1.0);
    let yy = (mid_y as f32) - (y * GAIN * span);
    yy.round() as i32
}

fn rasterize_braille(w_cells: usize, h_cells: usize, y_left: &[i32], y_right: &[i32]) -> Vec<u8> {
    let w_px = (w_cells * 2) as i32;
    let h_px = (h_cells * 4) as i32;

    let mut bits: Vec<u8> = vec![0u8; w_cells * h_cells];

    // Draw both channels as continuous lines (overlayed).
    draw_polyline(&mut bits, w_cells, h_cells, y_left, w_px, h_px);
    draw_polyline(&mut bits, w_cells, h_cells, y_right, w_px, h_px);

    bits
}

fn draw_polyline(
    bits: &mut [u8],
    w_cells: usize,
    h_cells: usize,
    ys: &[i32],
    w_px: i32,
    h_px: i32,
) {
    if ys.is_empty() {
        return;
    }

    let mut prev_x = 0i32;
    let mut prev_y = ys[0].clamp(0, h_px - 1);

    for (xi, &raw_y) in ys.iter().enumerate().skip(1) {
        let x = (xi as i32).clamp(0, w_px - 1);
        let y = raw_y.clamp(0, h_px - 1);
        draw_line_px(bits, w_cells, h_cells, prev_x, prev_y, x, y);
        prev_x = x;
        prev_y = y;
    }
}

fn draw_line_px(
    bits: &mut [u8],
    w_cells: usize,
    h_cells: usize,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) {
    // Bresenham line in pixel space.
    let mut x0 = x0;
    let mut y0 = y0;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        set_pixel(bits, w_cells, h_cells, x0, y0);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn set_pixel(bits: &mut [u8], w_cells: usize, h_cells: usize, x: i32, y: i32) {
    if x < 0 || y < 0 {
        return;
    }
    let w_px = (w_cells * 2) as i32;
    let h_px = (h_cells * 4) as i32;
    if x >= w_px || y >= h_px {
        return;
    }

    let cell_x = (x / 2) as usize;
    let cell_y = (y / 4) as usize;
    if cell_x >= w_cells || cell_y >= h_cells {
        return;
    }

    let dx = (x % 2) as usize;
    let dy = (y % 4) as usize;
    let bit = braille_bit(dx, dy);
    let idx = cell_y * w_cells + cell_x;
    bits[idx] |= bit;
}

fn braille_bit(dx: usize, dy: usize) -> u8 {
    // Braille dot mapping (dx: 0 left, 1 right; dy: 0..3 top..bottom)
    // (0,0)->1, (0,1)->2, (0,2)->3, (0,3)->7
    // (1,0)->4, (1,1)->5, (1,2)->6, (1,3)->8
    match (dx, dy) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (0, 3) => 0x40,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        (1, 3) => 0x80,
        _ => 0,
    }
}

fn braille_from_bits(bits: u8) -> char {
    // Unicode braille patterns start at 0x2800.
    char::from_u32(0x2800 + bits as u32).unwrap_or(' ')
}

fn freq_for_bin(k: usize) -> f32 {
    if BINS <= 1 {
        return F_MIN_HZ;
    }
    let t = (k as f32 / (BINS - 1) as f32).clamp(0.0, 1.0);
    let ratio = F_MAX_HZ / F_MIN_HZ;
    F_MIN_HZ * ratio.powf(t)
}

fn wrap_tau(mut x: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    if !x.is_finite() {
        return 0.0;
    }
    // Keep in [0, TAU).
    x %= tau;
    if x < 0.0 {
        x += tau;
    }
    x
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
