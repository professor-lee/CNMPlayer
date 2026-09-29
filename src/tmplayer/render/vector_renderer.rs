//! 全屏页「矢量模式」：把 PCM 波形画成李萨如图。
//!
//! 数据源与示波器相同（`audio::pcm_tap` 的共享环），映射不同：**左声道作 X、
//! 右声道作 Y**，逐样本画出立体声相位图 —— 业内通行的 goniometer /
//! vectorscope。面板中心为原点，不画坐标轴；单声道音源两声道相同，图形
//! 退化为 45° 对角线，这是如实呈现而非缺陷。
//!
//! 与示波器渲染器的两点刻意差异：
//!
//! - **无触发、无峰值抽取**：李萨如看的是相位关系而非周期计数，取最近一段
//!   短窗逐段连线即可，相位图天然稳态。
//! - **自动缩放（峰值保持）**：图形始终撑满可用区域 —— 攻击即时（当前帧
//!   峰值直接参与分母），释放缓慢（`PEAK_RELEASE_DB_S`）。渐弱时图形随实际
//!   电平相对保持峰值缓慢收缩，低于 [`SCALE_FLOOR`] 后按实际幅度继续缩小，
//!   最终消失；不会归一化出满幅噪点。
//!
//! 盲文光栅（2×4 点/格、逐点写位、行渐变配色）与示波器共用同一套
//! [`set_pixel`]/[`paint`]。

use crate::tmplayer::app::state::AppState;
use crate::tmplayer::audio::pcm_tap::PcmSnapshot;
use crate::tmplayer::render::oscilloscope_renderer::{paint, set_pixel};
use ratatui::Frame;
use ratatui::layout::Rect;
use std::time::Duration;

/// 显示窗口时长。短窗即可：20 ms 内 40 Hz 仍有大半个周期，高频则由密集
/// 轨迹自然填成实心图形（vectorscope 的常态）。
const VECTOR_WINDOW_MS: f32 = 20.0;

/// 自动缩放的幅度下限（约 −60 dBFS）。低于它后图形按实际幅度收缩，
/// 渐弱的最终因此是图形消失，而不是把底噪放大成满幅。
const SCALE_FLOOR: f32 = 1.0e-3;

/// 峰值保持的释放速率（dB/s）。渐弱在数秒尺度上发生，释放必须更慢，
/// 图形才会随之平滑缩小而非瞬间塌缩。
const PEAK_RELEASE_DB_S: f32 = -12.0;

/// 低于此电平不再画任何点：避免收缩末段在原点残留一个孤点。
const VISIBILITY_MIN: f32 = SCALE_FLOOR * 0.1;

/// 矢量模式的全部可变状态：李萨如光栅、自动缩放包络与打断动画。
///
/// 存放在 [`AppState`] 里（与示波器的 `ScopeScratch` 同位），渲染路径零分配。
#[derive(Debug, Default)]
pub struct VectorState {
    pub(crate) snapshot: PcmSnapshot,
    /// 每盲文点一个掩码，行优先 —— 与示波器同一张光栅模型，共用 [`paint`]。
    grid: Vec<u8>,
    /// 峰值保持的当前值（自动缩放的分母记忆）。
    scale_peak: f32,
    /// 渲染线程观测到的窗口原始峰值，`tick` 消费。`None` 表示环里没有样本。
    observed_level: Option<f32>,
    /// 面板尺寸（单元格数），`render` 时刷新；缩放与光栅都以此为坐标系。
    w_cells: usize,
    h_cells: usize,
}

impl VectorState {
    /// 时间推进：峰值保持的释放、打断动画的状态机（后续阶段接入）。
    pub(crate) fn tick(&mut self, enabled: bool, playing: bool, dt: Duration) {
        if !enabled {
            self.reset();
            return;
        }
        let _ = playing; // 阶段 C（打断动画）使用

        // 释放按时间取幂，掉帧时缩放轨迹不变（与 ScopeGain 同一纪律）。
        let release = 10.0_f32
            .powf(PEAK_RELEASE_DB_S / 20.0)
            .powf(dt.as_secs_f32());
        self.scale_peak = self
            .observed_level
            .unwrap_or(0.0)
            .max(self.scale_peak * release);
    }

    /// 渲染线程每帧报告窗口原始峰值与面板尺寸。
    pub(crate) fn observe(&mut self, level: Option<f32>, w_cells: usize, h_cells: usize) {
        self.observed_level = level;
        self.w_cells = w_cells;
        self.h_cells = h_cells;
    }

    /// 快动画（分散 / 回位）进行中：需要 `spectrum_hz` 高帧率推完。
    pub(crate) fn is_animating(&self) -> bool {
        false
    }

    /// 停稳后的极慢悬浮：图形静止但仍在动，需要基础帧率持续重绘。
    pub(crate) fn is_floating(&self) -> bool {
        false
    }

    fn reset(&mut self) {
        self.scale_peak = 0.0;
        self.observed_level = None;
        self.grid.clear();
    }
}

pub fn render(f: &mut Frame, area: Rect, app: &mut AppState) {
    let (w_cells, h_cells) = (area.width as usize, area.height as usize);
    if w_cells == 0 || h_cells == 0 {
        return;
    }

    // 环由宿主持有：先克隆 Arc 再借 state，避免同时借用 app 的两个字段。
    match app.pcm_ring.clone() {
        Some(ring) => ring.snapshot(&mut app.vector.snapshot),
        None => app.vector.snapshot.clear(),
    }

    let level = window_level(&app.vector.snapshot);
    app.vector.observe(level, w_cells, h_cells);
    app.vector.rasterize();
    paint(
        f.buffer_mut(),
        area,
        &app.vector.grid,
        &app.theme,
        w_cells,
        h_cells,
    );
}

impl VectorState {
    /// 依当前缩放把窗口样本画成李萨如轨迹。
    fn rasterize(&mut self) {
        let (w_cells, h_cells) = (self.w_cells, self.h_cells);
        let VectorState {
            snapshot,
            grid,
            scale_peak,
            observed_level,
            ..
        } = self;

        grid.clear();
        grid.resize(w_cells * h_cells, 0);

        let Some(level) = observed_level else { return };
        if *level <= VISIBILITY_MIN {
            return;
        }

        let w_px = w_cells * 2;
        let h_px = h_cells * 4;
        // 四周各留 1 点余量，图形不顶到边框。
        let half = (w_px.min(h_px) as f32 - 2.0) * 0.5;
        if half <= 0.0 {
            return;
        }

        // 攻击即时（当前峰值直接进分母），记忆只负责慢释放。
        let eff_peak = scale_peak.max(*level).max(SCALE_FLOOR);
        let scale = half / eff_peak;
        let (cx, cy) = ((w_px as f32 - 1.0) * 0.5, (h_px as f32 - 1.0) * 0.5);

        let n = window_frames(snapshot);
        if n < 2 {
            return;
        }
        let base = snapshot.len - n;
        let (left, right) = (
            &snapshot.left[base..base + n],
            &snapshot.right[base..base + n],
        );

        let mut prev: Option<(f32, f32)> = None;
        for i in 0..n {
            // X=左声道、Y=右声道（屏幕 y 向下，故取负）。
            let (x, y) = (cx + left[i] * scale, cy - right[i] * scale);
            if let Some((x0, y0)) = prev {
                draw_segment(grid, w_cells, h_cells, x0, y0, x, y);
            }
            prev = Some((x, y));
        }
    }
}

/// 显示窗口帧数：固定时长换算，上限为环内样本数。
fn window_frames(snapshot: &PcmSnapshot) -> usize {
    if snapshot.sample_rate == 0 {
        return 0;
    }
    let by_time = (snapshot.sample_rate as f32 * VECTOR_WINDOW_MS / 1000.0) as usize;
    by_time.min(snapshot.len).max(1)
}

/// 窗口内左右声道的原始峰值（自动缩放与静音判定的输入，未平滑）。
fn window_level(snapshot: &PcmSnapshot) -> Option<f32> {
    let n = window_frames(snapshot);
    if n < 2 {
        return None;
    }
    let base = snapshot.len - n;
    let mut peak = 0.0f32;
    for i in base..snapshot.len {
        peak = peak
            .max(snapshot.left[i].abs())
            .max(snapshot.right[i].abs());
    }
    Some(peak)
}

/// 相邻样本连线：沿较长轴步进，终点数 = 段长 + 1，保证不断点。
fn draw_segment(
    grid: &mut [u8],
    w_cells: usize,
    h_cells: usize,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
) {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as usize;
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        set_pixel(
            grid,
            w_cells,
            h_cells,
            (x0 + dx * t).round() as i32,
            (y0 + dy * t).round() as i32,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    /// 生成一个合成快照：`sample_at(i)` 给出第 i 帧的 (左, 右)。
    fn synth(len: usize, sample_at: impl Fn(usize) -> (f32, f32)) -> PcmSnapshot {
        let mut s = PcmSnapshot::default();
        for i in 0..len {
            let (l, r) = sample_at(i);
            s.left[i] = l;
            s.right[i] = r;
        }
        s.len = len;
        s.sample_rate = SR;
        s.stereo = true;
        s
    }

    fn circle(amp: f32, phase: f32) -> impl Fn(usize) -> (f32, f32) {
        move |i| {
            let t = i as f32 / SR as f32;
            let w = std::f32::consts::TAU * 110.0 * t;
            (amp * w.sin(), amp * (w + phase).sin())
        }
    }

    /// 收集光栅里全部点亮点的子像素坐标（与示波器测试同一解码方式）。
    fn lit_dots(state: &VectorState) -> Vec<(i32, i32)> {
        let mut dots = Vec::new();
        for (cell, &bits) in state.grid.iter().enumerate() {
            if bits == 0 {
                continue;
            }
            let (cx, cy) = (cell % state.w_cells, cell / state.w_cells);
            for dy in 0..4 {
                for dx in 0..2 {
                    if bits & crate::tmplayer::render::oscilloscope_renderer::braille_bit(dx, dy)
                        != 0
                    {
                        dots.push(((cx * 2 + dx) as i32, (cy * 4 + dy) as i32));
                    }
                }
            }
        }
        dots
    }

    fn bbox(dots: &[(i32, i32)]) -> (i32, i32, i32, i32) {
        let xs = dots.iter().map(|d| d.0);
        let ys = dots.iter().map(|d| d.1);
        (
            xs.clone().min().unwrap(),
            xs.max().unwrap(),
            ys.clone().min().unwrap(),
            ys.max().unwrap(),
        )
    }

    /// 110 Hz 双声道相位差 90°：以 0.8 幅度画出一个居中的大圆。
    #[test]
    fn lissajous_draws_centered_figure() {
        let snap = synth(882, circle(0.8, std::f32::consts::FRAC_PI_2));
        let mut st = VectorState {
            snapshot: snap,
            ..Default::default()
        };
        st.observe(window_level(&st.snapshot), 40, 20);
        st.tick(true, true, Duration::from_secs(1));
        st.rasterize();

        let dots = lit_dots(&st);
        assert!(
            dots.len() > 100,
            "应有可观密度的轨迹，实得 {} 点",
            dots.len()
        );

        let (min_x, max_x, min_y, max_y) = bbox(&dots);
        let (w_px, h_px) = (80.0, 80.0);
        let (cx, cy) = ((w_px - 1.0) * 0.5, (h_px - 1.0) * 0.5);
        assert!(
            (max_x - min_x) as f32 >= w_px * 0.6,
            "图形宽度应占面板多数：{min_x}..{max_x}"
        );
        assert!(
            (max_y - min_y) as f32 >= h_px * 0.6,
            "图形高度应占面板多数：{min_y}..{max_y}"
        );
        let mid_x = (min_x + max_x) as f32 / 2.0;
        let mid_y = (min_y + max_y) as f32 / 2.0;
        assert!((mid_x - cx).abs() <= 3.0, "水平居中：中点 {mid_x} vs {cx}");
        assert!((mid_y - cy).abs() <= 3.0, "垂直居中：中点 {mid_y} vs {cy}");
    }

    /// 高于下限的任何电平都自动缩放到同一尺寸；低于可见下限则图形消失。
    #[test]
    fn autoscale_fills_panel_and_fades_to_nothing() {
        for amp in [1.0f32, 0.01] {
            let snap = synth(882, circle(amp, std::f32::consts::FRAC_PI_2));
            let mut st = VectorState {
                snapshot: snap,
                ..Default::default()
            };
            st.observe(window_level(&st.snapshot), 40, 20);
            st.tick(true, true, Duration::from_secs(1));
            st.rasterize();
            let (min_x, max_x, _, _) = bbox(&lit_dots(&st));
            assert!(
                (max_x - min_x) as f32 >= 80.0 * 0.6,
                "幅度 {amp} 应自动缩放到接近满幅"
            );
        }

        let snap = synth(882, circle(1.0e-5, std::f32::consts::FRAC_PI_2));
        let mut st = VectorState {
            snapshot: snap,
            ..Default::default()
        };
        st.observe(window_level(&st.snapshot), 40, 20);
        st.tick(true, true, Duration::from_secs(1));
        st.rasterize();
        assert!(
            lit_dots(&st).is_empty(),
            "低于可见下限应完全消失（渐弱的最终）"
        );
    }

    /// 单声道两声道相同：全部点落在 45° 对角线上（如实呈现，不做特例）。
    #[test]
    fn mono_source_renders_diagonal() {
        let snap = synth(882, |i| {
            let t = i as f32 / SR as f32;
            let v = (std::f32::consts::TAU * 70.0 * t).sin() * 0.6;
            (v, v)
        });
        let mut st = VectorState {
            snapshot: snap,
            ..Default::default()
        };
        st.observe(window_level(&st.snapshot), 40, 20);
        st.tick(true, true, Duration::from_secs(1));
        st.rasterize();

        let (cx, cy) = (39.5f32, 39.5f32);
        for (x, y) in lit_dots(&st) {
            let off = (x as f32 - cx) + (y as f32 - cy);
            assert!(
                off.abs() <= 2.0,
                "点 ({x},{y}) 偏离对角线 {off:.2}（连线步进与取整允差内）"
            );
        }
    }

    /// 峰值保持：攻击即时（观测值直接进位）、释放按时间取幂、掉帧不变轨迹；
    /// 关档复位。
    #[test]
    fn peak_hold_release_is_time_based() {
        let one_sec = 10.0_f32.powf(PEAK_RELEASE_DB_S / 20.0);

        let mut st = VectorState::default();
        st.observe(Some(1.0), 40, 20);
        st.tick(true, true, Duration::from_millis(16));
        assert!((st.scale_peak - 1.0).abs() < 1e-5, "攻击即时");

        // 响度落回后，保持值沿释放曲线下行（观测值不再托住它）。
        st.observe(Some(0.0), 40, 20);
        st.tick(true, true, Duration::from_secs(1));
        assert!(
            (st.scale_peak - one_sec).abs() < 1e-4,
            "释放一档：{}",
            st.scale_peak
        );

        // 更高的观测值仍然即时进位（自动缩放的攻击端）。
        st.observe(Some(2.0), 40, 20);
        st.tick(true, true, Duration::from_millis(16));
        assert!((st.scale_peak - 2.0).abs() < 1e-5, "更高观测即时进位");

        // 帧率无关：静音后 4×250ms 与 1×1s 同一落点。
        let mut small = VectorState::default();
        small.observe(Some(1.0), 40, 20);
        small.tick(true, true, Duration::from_millis(16));
        small.observe(Some(0.0), 40, 20);
        for _ in 0..4 {
            small.tick(true, true, Duration::from_millis(250));
        }
        assert!(
            (small.scale_peak - one_sec).abs() < 1e-4,
            "小步与大步等价：{}",
            small.scale_peak
        );

        st.tick(false, true, Duration::from_secs(1));
        assert_eq!(st.scale_peak, 0.0, "关档复位");
    }
}
