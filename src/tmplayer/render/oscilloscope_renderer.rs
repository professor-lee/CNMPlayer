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
use crate::ui::theme::Theme;
use ratatui::Frame;
use ratatui::buffer::Buffer;
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

/// 示波器的复用缓冲。存在 `AppState` 里，渲染路径因此不做任何分配。
#[derive(Debug, Default)]
pub struct ScopeScratch {
    snapshot: PcmSnapshot,
    /// 每单元格一个盲文点位掩码，行优先。左右声道叠加在同一张图上：
    /// 一个盲文单元格只有一个前景色，配色按行取纵向渐变，与声道无关。
    grid: Vec<u8>,
}

pub fn render(f: &mut Frame, area: Rect, app: &mut AppState) {
    let (w_cells, h_cells) = (area.width as usize, area.height as usize);
    if w_cells == 0 || h_cells == 0 {
        return;
    }

    // 环由宿主持有：先克隆 Arc 再借 scratch，避免同时借用 app 的两个字段。
    match app.pcm_ring.clone() {
        Some(ring) => ring.snapshot_tail_ms(
            &mut app.scope.snapshot,
            (WINDOW_MS + TRIGGER_SEARCH_MS).ceil() as u32,
        ),
        None => app.scope.snapshot.clear(),
    }

    rasterize(&mut app.scope, w_cells, h_cells, app.scope_gain.value());
    paint(
        f.buffer_mut(),
        area,
        &app.scope.grid,
        &app.theme,
        w_cells,
        h_cells,
    );
}

/// `gain` 是幅度包络：播放开始后张开到 1，停止后衰减回 0。
///
/// **归零不是不画**。`gain = 0` 时每个子列的 min/max 都映到零电平那一行，
/// [`draw_channel`] 于是自然画出一条贯穿全宽的居中直线 —— 那条线就是波形本身，
/// 不存在独立的中线元素。因为包络是全局的，所有子列同乘一个系数，落平必然
/// 同时发生。
fn rasterize(scope: &mut ScopeScratch, w_cells: usize, h_cells: usize, gain: f32) {
    let ScopeScratch { snapshot, grid } = scope;

    grid.clear();
    grid.resize(w_cells * h_cells, 0);

    let window = window_frames(snapshot);
    if window < 2 {
        // 环里还没有样本（刚启动）。此时同样画居中直线，与落平后的
        // 静止态是同一幅画面，不是空白。
        draw_flat_line(grid, w_cells, h_cells);
        return;
    }
    let start = trigger_offset(snapshot, window);

    draw_channel(
        grid,
        w_cells,
        h_cells,
        &snapshot.left[start..start + window],
        gain,
    );
    // 单声道音源两声道逐位相同，再画一遍只是白烧一半光栅化开销。
    if snapshot.stereo {
        draw_channel(
            grid,
            w_cells,
            h_cells,
            &snapshot.right[start..start + window],
            gain,
        );
    }
}

/// 无样本时的静止画面：零电平那一行铺满整宽。
///
/// 与 `gain = 0` 时 [`draw_channel`] 的输出逐点一致，两条路径因此收敛到同一幅
/// 画面（`no_samples_matches_settled_frame` 守着这条等价）。
fn draw_flat_line(grid: &mut [u8], w_cells: usize, h_cells: usize) {
    let zero = sample_row(0.0, (h_cells * 4) as i32);
    for col in 0..(w_cells * 2) as i32 {
        set_pixel(grid, w_cells, h_cells, col, zero);
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
fn draw_channel(grid: &mut [u8], w_cells: usize, h_cells: usize, samples: &[f32], gain: f32) {
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

        // gain 非负，先取极值再缩放与逐样本缩放等价，但少 n-2 次乘法。
        let (top, bottom) = (sample_row(hi * gain, h_px), sample_row(lo * gain, h_px));
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

/// 逐单元格直写帧缓冲。波形配色沿用原有逻辑：整行一个颜色，按行在
/// `accent2` → `accent3` 之间做纵向渐变。直写只是省掉每帧的字符串分配。
///
/// 只画 grid 里有的东西：静止时那条居中直线是 `rasterize` 画出来的波形，
/// 这里没有任何中线的特例分支。取 `Buffer` 而非 `Frame`，因此可离屏验证。
/// 与矢量模式共用（盲文点阵直写 + 行渐变配色），故为 `pub(crate)`。
pub(crate) fn paint(
    buf: &mut Buffer,
    area: Rect,
    grid: &[u8],
    theme: &Theme,
    w_cells: usize,
    h_cells: usize,
) {
    let clip = area.intersection(buf.area);
    for row in 0..clip.height as usize {
        let t = if h_cells <= 1 {
            1.0
        } else {
            row as f32 / (h_cells - 1) as f32
        };
        let fg = vertical_gradient_color(theme, t);

        for col in 0..clip.width as usize {
            let bits = grid[row * w_cells + col];
            if bits == 0 {
                continue;
            }

            if let Some(cell) = buf.cell_mut((clip.x + col as u16, clip.y + row as u16)) {
                cell.set_char(braille_from_bits(bits));
                cell.set_fg(fg);
            }
        }
    }
}

/// 与矢量模式共用的盲文点位写入：越界点直接丢弃。
pub(crate) fn set_pixel(bits: &mut [u8], w_cells: usize, h_cells: usize, x: i32, y: i32) {
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

/// 与矢量模式共用（含测试侧的盲文点位解码）。
pub(crate) fn braille_bit(dx: usize, dy: usize) -> u8 {
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

fn vertical_gradient_color(theme: &Theme, t: f32) -> Color {
    mix(theme.color_accent2(), theme.color_accent3(), t)
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

#[cfg(test)]
mod tests;
