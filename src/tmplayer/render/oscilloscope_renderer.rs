//! 真 PCM 时域示波器。
//!
//! 样本来自 `audio::pcm_tap` 的共享环（宿主播放链路上的抽头），不经 cava。
//! 三个要素照搬硬件示波器，缺一则观感立刻退化：
//!
//! 1. **上升沿触发**：每帧在最新一段样本里找一个带滞回的零交叉作为窗口起点，
//!    周期信号因此在屏上静止。没有它，窗口只能随时间平移，每帧移动
//!    `(1/帧率)/窗口时长` 个屏宽，远超人眼可跟踪的范围，画面就是一片沸腾的噪点。
//! 2. **峰值（min/max）抽取**：每个子列画该段样本的极值包络带，而不是采一个点。
//!    包络对时域欠采样免疫，高频段因此是稳定的实心带而非混叠锯齿 ——
//!    这正是示波器的 peak-detect 采集模式。
//! 3. **绝对幅度映射**：不归一化、不做 AGC。安静段贴中线、高潮段撑满，
//!    响度动态如实呈现。

use crate::tmplayer::app::state::AppState;
use crate::tmplayer::audio::pcm_tap::PcmSnapshot;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// 显示窗口时长，相当于示波器的 time/div。
///
/// 40 ms 在 40 Hz 处约一个周期、在 1 kHz 处约 40 个周期：低频看得出波形，
/// 高频在峰值抽取下收敛成稳定包络。
const WINDOW_MS: f32 = 40.0;

/// 触发搜索区间，须覆盖最低可见频率的一个周期（40 Hz → 25 ms）。
const TRIGGER_SEARCH_MS: f32 = 25.0;

/// 触发滞回带，相对搜索区间内的峰值。抑制噪声在零点附近反复误触发。
const TRIGGER_HYSTERESIS: f32 = 0.05;

/// 滞回带的绝对下限：静音时不让浮点噪声触发。
const TRIGGER_FLOOR: f32 = 1.0e-4;

/// 右声道颜色向背景压暗的比例。
///
/// 不能靠色相区分左右：System 主题的 accent / accent2 / accent3 全是 `#FFFFFF`，
/// 任何色相方案在该主题下都不可见。压暗在所有主题下都成立。
const RIGHT_CHANNEL_DIM: f32 = 0.55;

/// 示波器的复用缓冲。存在 `AppState` 里，渲染路径因此不做任何分配。
#[derive(Debug, Default)]
pub struct ScopeScratch {
    snapshot: PcmSnapshot,
    /// 每单元格一个盲文点位掩码，行优先
    left: Vec<u8>,
    right: Vec<u8>,
    /// 本帧右声道是否真的画了东西
    stereo: bool,
}

pub fn render(f: &mut Frame, area: Rect, app: &mut AppState) {
    let (w_cells, h_cells) = (area.width as usize, area.height as usize);
    if w_cells == 0 || h_cells == 0 {
        return;
    }

    // 环由宿主持有：先克隆 Arc 再借 scratch，避免同时借用 app 的两个字段。
    match app.pcm_ring.clone() {
        Some(ring) => ring.snapshot(&mut app.scope.snapshot),
        None => app.scope.snapshot.clear(),
    }

    rasterize(&mut app.scope, w_cells, h_cells);
    paint(f, area, app, w_cells, h_cells);
}

fn rasterize(scope: &mut ScopeScratch, w_cells: usize, h_cells: usize) {
    let ScopeScratch {
        snapshot,
        left,
        right,
        stereo,
    } = scope;

    let cells = w_cells * h_cells;
    left.clear();
    left.resize(cells, 0);
    right.clear();
    right.resize(cells, 0);
    *stereo = false;

    let window = window_frames(snapshot);
    if window < 2 {
        return;
    }
    let start = trigger_offset(snapshot, window);

    draw_channel(
        left,
        w_cells,
        h_cells,
        &snapshot.left[start..start + window],
    );
    if snapshot.stereo {
        draw_channel(
            right,
            w_cells,
            h_cells,
            &snapshot.right[start..start + window],
        );
        *stereo = true;
    }
}

/// 显示窗口帧数：按固定时长换算，与终端宽度无关 —— 宽度只影响抽取密度。
fn window_frames(snapshot: &PcmSnapshot) -> usize {
    if snapshot.sample_rate == 0 {
        return 0;
    }
    let by_time = (snapshot.sample_rate as f32 * WINDOW_MS / 1000.0) as usize;
    by_time.min(snapshot.len)
}

/// 找窗口起点：在最新一窗之前的搜索区间内取**最后**一个带滞回的上升沿零交叉，
/// 使画面既相位稳定又尽量新。
///
/// 触发信号取左右声道之和：任一声道静音时仍能锁定，且不破坏声道间相位差。
/// 找不到（静音、纯直流）就回落到最新一窗，即示波器的自由运行模式。
fn trigger_offset(snapshot: &PcmSnapshot, window: usize) -> usize {
    let latest = snapshot.len - window;
    let search = ((snapshot.sample_rate as f32 * TRIGGER_SEARCH_MS / 1000.0) as usize).min(latest);
    let begin = latest - search;

    let mut peak = 0.0f32;
    for i in begin..latest {
        peak = peak.max(mono(snapshot, i).abs());
    }
    let hysteresis = (peak * TRIGGER_HYSTERESIS).max(TRIGGER_FLOOR);

    // 必须先跌破 -hysteresis 才算“上膛”，再向上穿过零点才触发。
    let mut armed = false;
    let mut found = None;
    for i in begin..latest {
        let v = mono(snapshot, i);
        if v <= -hysteresis {
            armed = true;
        } else if armed && v >= 0.0 {
            armed = false;
            found = Some(i);
        }
    }
    found.unwrap_or(latest)
}

fn mono(snapshot: &PcmSnapshot, index: usize) -> f32 {
    (snapshot.left[index] + snapshot.right[index]) * 0.5
}

/// 峰值抽取：每个子列取该段样本的 min/max 并填满其间像素。
///
/// 相邻子列的跨度不相接时互相延伸一格，轨迹因此连续 —— 陡沿处等价于连线，
/// 但不必单独走一遍 Bresenham。样本比子列还少时每列复用最近的样本，
/// 由同一段接合逻辑连成折线，无需第二条代码路径。
fn draw_channel(grid: &mut [u8], w_cells: usize, h_cells: usize, samples: &[f32]) {
    let w_px = w_cells * 2;
    let h_px = (h_cells * 4) as i32;
    let n = samples.len();
    let mut prev: Option<(i32, i32)> = None;

    for col in 0..w_px {
        let begin = col * n / w_px;
        let end = ((col + 1) * n / w_px).clamp(begin + 1, n);
        let (mut lo, mut hi) = (samples[begin], samples[begin]);
        for &v in &samples[begin..end] {
            lo = lo.min(v);
            hi = hi.max(v);
        }

        let (top, bottom) = (sample_row(hi, h_px), sample_row(lo, h_px));
        let (mut from, mut to) = (top, bottom);
        if let Some((prev_top, prev_bottom)) = prev {
            from = from.min(prev_bottom);
            to = to.max(prev_top);
        }
        for y in from..=to {
            set_pixel(grid, w_cells, h_cells, col as i32, y);
        }
        // 接合用本列的原始跨度，否则延伸量会逐列累积。
        prev = Some((top, bottom));
    }
}

/// 样本值 → 像素行：+1 在顶、-1 在底。EQ 提升可能让样本越过 ±1，故钳位。
fn sample_row(v: f32, h_px: i32) -> i32 {
    let span = (h_px - 1) as f32;
    ((1.0 - v) * 0.5 * span).round().clamp(0.0, span) as i32
}

/// 逐单元格直写帧缓冲：颜色按格变化（左声道 / 右声道 / 中线三种），
/// 整行一个 Span 表达不了，直写也省掉每帧的字符串分配。
fn paint(f: &mut Frame, area: Rect, app: &AppState, w_cells: usize, h_cells: usize) {
    let scope = &app.scope;
    let h_px = (h_cells * 4) as i32;

    // 中线 graticule：用盲文点位精确落在零电平那一子行上，而非单元格的几何中心。
    let zero_row = sample_row(0.0, h_px) as usize;
    let (graticule_row, graticule_sub) = (zero_row / 4, zero_row % 4);
    let graticule =
        braille_from_bits(braille_bit(0, graticule_sub) | braille_bit(1, graticule_sub));
    let graticule_fg = app.theme.color_subtext();

    let buf = f.buffer_mut();
    let clip = area.intersection(buf.area);

    for row in 0..clip.height as usize {
        let t = if h_cells <= 1 {
            1.0
        } else {
            row as f32 / (h_cells - 1) as f32
        };
        let trace_fg = vertical_gradient_color(app, t);
        let right_fg = mix(trace_fg, app.theme.color_base(), RIGHT_CHANNEL_DIM);

        for col in 0..clip.width as usize {
            let index = row * w_cells + col;
            let left = scope.left[index];
            let right = if scope.stereo { scope.right[index] } else { 0 };

            // 两声道重叠的格子归左声道：亮色压暗色，与叠加显示的惯例一致。
            let (glyph, fg) = match (left, right) {
                (0, 0) if row == graticule_row => (graticule, graticule_fg),
                (0, 0) => continue,
                (0, r) => (braille_from_bits(r), right_fg),
                (l, r) => (braille_from_bits(l | r), trace_fg),
            };

            if let Some(cell) = buf.cell_mut((clip.x + col as u16, clip.y + row as u16)) {
                cell.set_char(glyph);
                cell.set_fg(fg);
            }
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
    bits[cell_y * w_cells + cell_x] |= braille_bit(dx, dy);
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

fn vertical_gradient_color(app: &AppState, t: f32) -> Color {
    let top = app.theme.color_accent2();
    let bottom = app.theme.color_accent3();
    mix(top, bottom, t)
}

/// 非 truecolor 终端（`Ansi256` / `NoColor`）上主题色不是 `Color::Rgb`，
/// 此处退化为返回 `a`：渐变塌成单色、左右声道同色。与既有频谱渲染的降级一致。
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
