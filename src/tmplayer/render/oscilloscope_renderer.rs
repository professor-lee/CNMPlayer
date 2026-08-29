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
use crate::tmplayer::ui::theme::Theme;
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
        Some(ring) => ring.snapshot(&mut app.scope.snapshot),
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
        // 环里还没有样本（无宿主、刚启动）。此时同样画居中直线，与落平后的
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
fn paint(buf: &mut Buffer, area: Rect, grid: &[u8], theme: &Theme, w_cells: usize, h_cells: usize) {
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
mod tests {
    use super::*;
    use crate::tmplayer::ui::theme::{ColorCapability, ThemeName, ThemePalette};

    const RATE: u32 = 48_000;

    fn sine(freq: f32, phase: f32, len: usize) -> PcmSnapshot {
        let mut snapshot = PcmSnapshot::default();
        for i in 0..len {
            let t = i as f32 / RATE as f32;
            snapshot.left[i] = (std::f32::consts::TAU * freq * t + phase).sin();
            snapshot.right[i] = snapshot.left[i];
        }
        snapshot.len = len;
        snapshot.sample_rate = RATE;
        snapshot
    }

    #[test]
    fn trigger_locks_onto_a_rising_zero_crossing_at_any_phase() {
        let window = (RATE as f32 * WINDOW_MS / 1000.0) as usize;
        for step in 0..8 {
            let phase = step as f32 * std::f32::consts::TAU / 8.0;
            let snapshot = sine(440.0, phase, window * 4);
            let start = trigger_offset(&snapshot, window);

            // 触发点必须真的落在上升沿上：本身刚过零、前一个为负、后一个为正。
            assert!(snapshot.left[start] >= 0.0 && snapshot.left[start] < 0.07);
            assert!(snapshot.left[start - 1] < 0.0);
            assert!(snapshot.left[start + 1] > 0.0);
        }
    }

    #[test]
    fn silence_falls_back_to_the_newest_window() {
        let snapshot = PcmSnapshot {
            len: 4096,
            sample_rate: RATE,
            ..Default::default()
        };

        let window = window_frames(&snapshot);
        assert_eq!(trigger_offset(&snapshot, window), snapshot.len - window);
    }

    #[test]
    fn full_scale_trace_is_continuous_and_spans_the_grid() {
        let (w_cells, h_cells) = (60usize, 8usize);
        let window = (RATE as f32 * WINDOW_MS / 1000.0) as usize;
        let snapshot = sine(440.0, 0.0, window * 4);
        let start = trigger_offset(&snapshot, window);

        let mut grid = vec![0u8; w_cells * h_cells];
        draw_channel(
            &mut grid,
            w_cells,
            h_cells,
            &snapshot.left[start..start + window],
            1.0,
        );

        // 每一列都得有点亮的格子：包络带连续，接合逻辑没漏掉断点。
        for col in 0..w_cells {
            assert!(
                (0..h_cells).any(|row| grid[row * w_cells + col] != 0),
                "column {col} is empty"
            );
        }
        // 绝对映射：满幅信号必须触到顶行与底行，不被归一化压扁。
        assert!((0..w_cells).any(|col| grid[col] != 0), "top row untouched");
        assert!(
            (0..w_cells).any(|col| grid[(h_cells - 1) * w_cells + col] != 0),
            "bottom row untouched"
        );
    }

    #[test]
    fn quiet_signal_stays_near_zero_level() {
        let (w_cells, h_cells) = (60usize, 8usize);
        let window = (RATE as f32 * WINDOW_MS / 1000.0) as usize;
        let mut snapshot = sine(440.0, 0.0, window * 4);
        for v in snapshot.left.iter_mut() {
            *v *= 0.02;
        }

        let mut grid = vec![0u8; w_cells * h_cells];
        draw_channel(&mut grid, w_cells, h_cells, &snapshot.left[0..window], 1.0);

        // 顶行与底行必须是空的：响度动态没有被 AGC 抹掉。
        assert!((0..w_cells).all(|col| grid[col] == 0));
        assert!((0..w_cells).all(|col| grid[(h_cells - 1) * w_cells + col] == 0));
    }

    #[test]
    fn fewer_samples_than_subcolumns_still_draws_every_column() {
        let (w_cells, h_cells) = (60usize, 6usize);
        let samples: Vec<f32> = (0..17).map(|i| ((i % 5) as f32 - 2.0) / 2.0).collect();

        let mut grid = vec![0u8; w_cells * h_cells];
        draw_channel(&mut grid, w_cells, h_cells, &samples, 1.0);

        for col in 0..w_cells {
            assert!(
                (0..h_cells).any(|row| grid[row * w_cells + col] != 0),
                "column {col} is empty"
            );
        }
    }

    #[test]
    fn gain_envelope_collapses_the_trace_toward_zero_level() {
        let (w_cells, h_cells) = (60usize, 8usize);
        let window = (RATE as f32 * WINDOW_MS / 1000.0) as usize;
        let snapshot = sine(440.0, 0.0, window * 4);

        let span = |gain: f32| {
            let mut grid = vec![0u8; w_cells * h_cells];
            draw_channel(&mut grid, w_cells, h_cells, &snapshot.left[0..window], gain);
            let rows: Vec<usize> = (0..h_cells)
                .filter(|row| (0..w_cells).any(|col| grid[row * w_cells + col] != 0))
                .collect();
            rows.last().map(|hi| hi - rows[0] + 1).unwrap_or(0)
        };

        // 幅度包络越小，波形占的行数越少 —— 暂停后就是这样落回零电平的。
        let full = span(1.0);
        let half = span(0.5);
        assert_eq!(full, h_cells, "满幅应撑满面板");
        assert!(half < full, "half={half} full={full}");
        assert_eq!(span(0.0), 1, "只剩零电平那一行");
    }

    /// 落平后波形退化成居中直线：由 `draw_channel` 自己画出来，不是外加的中线。
    #[test]
    fn settled_scope_rasterizes_a_centred_flat_line() {
        let (w_cells, h_cells) = (60usize, 8usize);
        let mut scope = ScopeScratch {
            snapshot: sine(440.0, 0.0, 8192),
            grid: Vec::new(),
        };

        rasterize(&mut scope, w_cells, h_cells, 0.0);

        // 恰好一行被点亮，且落在零电平所在的那一行。
        let lit: Vec<usize> = (0..h_cells)
            .filter(|row| (0..w_cells).any(|col| scope.grid[row * w_cells + col] != 0))
            .collect();
        let zero_row = sample_row(0.0, (h_cells * 4) as i32) as usize / 4;
        assert_eq!(lit, vec![zero_row], "应只点亮零电平那一行");

        // 且贯穿全宽：每一格都有点，才是一条不断的直线。
        assert!(
            (0..w_cells).all(|col| scope.grid[zero_row * w_cells + col] != 0),
            "直线有断点"
        );
    }

    /// 环里没有样本时（无宿主、刚启动）画的必须与落平后完全一致 —— 否则启动
    /// 瞬间会闪一下空白，或停止后画面与启动态不符。
    #[test]
    fn no_samples_matches_settled_frame() {
        let (w_cells, h_cells) = (60usize, 8usize);

        let mut empty = ScopeScratch::default();
        rasterize(&mut empty, w_cells, h_cells, 1.0);

        let mut settled = ScopeScratch {
            snapshot: sine(440.0, 0.0, 8192),
            grid: Vec::new(),
        };
        rasterize(&mut settled, w_cells, h_cells, 0.0);

        assert_eq!(empty.grid, settled.grid);
    }

    fn test_theme() -> Theme {
        Theme {
            name: ThemeName::Frappe,
            palette: ThemePalette {
                text: (198, 208, 245),
                subtext: (165, 173, 206),
                base: (48, 52, 70),
                surface: (65, 69, 89),
                buff: (81, 87, 109),
                accent: (140, 170, 238),
                accent2: (133, 193, 220),
                accent3: (202, 158, 230),
            },
            capability: ColorCapability::TrueColor,
        }
    }

    /// 离屏渲染一帧，返回 (点亮的单元格数, 出现过的前景色集合)。
    fn paint_frame(gain: f32) -> (usize, Vec<Color>) {
        let (w_cells, h_cells) = (60u16, 8u16);
        let area = Rect::new(0, 0, w_cells, h_cells);
        let mut scope = ScopeScratch {
            snapshot: sine(440.0, 0.0, 8192),
            grid: Vec::new(),
        };
        rasterize(&mut scope, w_cells as usize, h_cells as usize, gain);

        let mut buf = Buffer::empty(area);
        paint(
            &mut buf,
            area,
            &scope.grid,
            &test_theme(),
            w_cells as usize,
            h_cells as usize,
        );

        let mut colours = Vec::new();
        let mut lit = 0;
        for y in 0..h_cells {
            for x in 0..w_cells {
                let cell = &buf[(x, y)];
                if cell.symbol() == " " {
                    continue;
                }
                lit += 1;
                if !colours.contains(&cell.fg) {
                    colours.push(cell.fg);
                }
            }
        }
        (lit, colours)
    }

    #[test]
    fn settled_frame_is_a_single_centred_waveform_row() {
        let (w_cells, _) = (60usize, 8usize);
        let (playing, _) = paint_frame(1.0);
        let (settled, colours) = paint_frame(0.0);

        assert!(playing > w_cells, "播放中应画出波形");
        // 静止态正好一整行：波形自己收成的直线，不多不少。
        assert_eq!(settled, w_cells, "静止态应只剩居中那一行");
        // 且用波形的渐变色，不是任何专供中线的颜色 —— 没有独立的中线元素。
        assert_eq!(colours.len(), 1, "一行只有一个颜色: {colours:?}");
        assert_ne!(colours[0], test_theme().color_surface());
    }

    /// 静止那条线必须落在垂直居中位置。
    #[test]
    fn settled_line_sits_vertically_centred() {
        for h_cells in [4usize, 6, 8, 12] {
            let w_cells = 8usize;
            let area = Rect::new(0, 0, w_cells as u16, h_cells as u16);
            let mut scope = ScopeScratch::default();
            rasterize(&mut scope, w_cells, h_cells, 0.0);

            let mut buf = Buffer::empty(area);
            paint(&mut buf, area, &scope.grid, &test_theme(), w_cells, h_cells);

            // 收集点亮的子行，取其中点，与面板正中比较。
            let mut subrows = Vec::new();
            for row in 0..h_cells {
                let bits = scope.grid[row * w_cells];
                for dy in 0..4 {
                    if bits & braille_bit(0, dy) != 0 {
                        subrows.push(row * 4 + dy);
                    }
                }
            }
            assert_eq!(subrows.len(), 1, "h={h_cells} 应只有一条线");

            let centre = (h_cells * 4 - 1) as f32 / 2.0;
            let offset = (subrows[0] as f32 - centre).abs();
            assert!(offset <= 0.5, "h={h_cells} 偏离中心 {offset} 子行");
        }
    }
}
