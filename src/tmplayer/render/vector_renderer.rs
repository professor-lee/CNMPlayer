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
//! **打断动画**：暂停（或播放中突然静音）时，图形的每个盲文点化作粒子，
//! 沿被打断瞬间的轨迹运动方向飞出，受恒定大减速度减速，先后停稳；停稳后
//! 以极慢的正弦漂移悬浮在终点周围的 3×3 点区域内。恢复播放（或声音回来）
//! 则就近锚定当前图形，指数逼近迅速归位，贴上即吸收。
//!
//! 盲文光栅（2×4 点/格、逐点写位、行渐变配色）与示波器共用同一套
//! [`paint`]。

use crate::tmplayer::app::state::AppState;
use crate::tmplayer::audio::pcm_tap::PcmSnapshot;
use crate::tmplayer::render::oscilloscope_renderer::{braille_bit, paint, set_pixel};
use ratatui::Frame;
use ratatui::layout::Rect;
use std::f32::consts::TAU;
use std::time::Duration;

/// 显示窗口时长。短窗即可：20 ms 内 40 Hz 仍有大半个周期，高频则由密集
/// 轨迹自然填成实心图形（vectorscope 的常态）。
const VECTOR_WINDOW_MS: f32 = 20.0;

/// 自动缩放的幅度下限（约 −60 dBFS）。低于它后图形按实际幅度收缩，
/// 渐弱的最终因此是图形消失，而不是把底噪放大成满幅。
const SCALE_FLOOR: f32 = 1.0e-3;

/// 打断（分散）判定的静音电平，与缩放下限同源。
const SILENCE_FLOOR: f32 = SCALE_FLOOR;

/// 静音需持续到此时长才判打断：一个 20 ms 窗全空只是正常乐句间隙。
const SILENCE_SUSTAIN: Duration = Duration::from_millis(80);

/// 峰值保持的释放速率（dB/s）。渐弱在数秒尺度上发生，释放必须更慢，
/// 图形才会随之平滑缩小而非瞬间塌缩。
const PEAK_RELEASE_DB_S: f32 = -12.0;

/// 低于此电平不再画任何点：避免收缩末段在原点残留一个孤点。
const VISIBILITY_MIN: f32 = SCALE_FLOOR * 0.1;

/// 粒子爆发停距（点）：飞行 2~10 点后停稳，构成可见的散开。
const BURST_MIN_DIST: f32 = 2.0;
const BURST_MAX_DIST: f32 = 10.0;

/// 粒子爆发时长（秒）：各点先后停稳，读作「浮动后停留」。
const BURST_MIN_TIME: f32 = 0.22;
const BURST_MAX_TIME: f32 = 0.5;

/// 悬浮幅度（点）：0.9 保证取整后落在终点周围 3×3 区域内。
const FLOAT_AMPLITUDE: f32 = 0.9;

/// 悬浮周期（秒）：极慢 —— 半个周期也要数秒才滑过 1 点。
const FLOAT_PERIOD_S: f32 = 12.0;

/// 回位逼近的时间常数（秒）：一帧走掉大半残差，约 0.1 s 收敛。
const HOMING_TAU: f32 = 0.04;

/// 就近锚定的搜索半径（点）。
const HOMING_RADIUS: i32 = 8;

/// 距目标近到这一步即视为归位（吸收）。
const ABSORB_DIST: f32 = 0.6;

/// 回位期找不到锚点的粒子最多滞留此时长，随后淡出（图形里已无它的位置）。
const RECOVER_TIMEOUT: Duration = Duration::from_millis(400);

/// 突断判定的近期电平衰减速率（dB/s）：比任何音乐渐弱都快，突断后它
/// 还停留在断前电平约百毫秒，差值因此可分辨。
const RECENT_DECAY_DB_S: f32 = -50.0;

/// 突断差值阈值：当前电平相对近期电平跌掉 1/32（≈ −30 dB）才算「突然的无声」。
const SUDDEN_DROP_RATIO: f32 = 1.0 / 32.0;

/// 判突断还要求断前电平可闻（≈ −40 dBFS）：安静段的停止不散开，
/// 图形本就按幅度缩到没了。
const SUDDEN_MIN_LEVEL: f32 = 1.0e-2;

/// 图形连续不可见超过此时长即退役最后一幅可见轨迹：渐弱消失许久后的
/// 暂停不得凭空散出「幽灵」粒子。
const GHOST_GRACE: Duration = Duration::from_secs(1);

/// 打断动画的相位机。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Phase {
    /// 跟随波形正常绘制。
    #[default]
    Active,
    /// 粒子飞行减速中。
    Dispersing,
    /// 全部停稳，极慢悬浮。
    Floating,
    /// 就近锚定当前图形，指数逼近归位。
    Recovering,
}

/// 一个被打断的盲文点。位置与速度都在子像素（点阵）坐标系，单位为点。
#[derive(Debug, Clone, Copy)]
struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    /// 恒定减速度（点/s²）：`v0 / 爆发时长`，速度线性衰减到 0。
    decel: f32,
    /// 悬浮中心（停稳位置）。
    anchor_x: f32,
    anchor_y: f32,
    /// 悬浮漂移相位（随机，两轴独立）。
    drift_x: f32,
    drift_y: f32,
    /// 回位期的运动方向（无锚点时为 0）。
    hx: f32,
    hy: f32,
    /// 回位期连续无锚点的时长。
    lost_for: Duration,
}

/// 矢量模式的全部可变状态：李萨如光栅、自动缩放包络与打断动画。
///
/// 存放在 [`AppState`] 里（与示波器的 `ScopeScratch` 同位），渲染路径零分配。
#[derive(Debug, Default)]
pub struct VectorState {
    pub(crate) snapshot: PcmSnapshot,
    /// 复合光栅（轨迹 + 粒子）：paint 直写帧缓冲的就是它。
    grid: Vec<u8>,
    /// 仅轨迹的光栅：回位锚定的搜索目标，避免粒子互相吸附。
    trace_grid: Vec<u8>,
    /// 最后一幅**画出来了的**轨迹与其速度场：打断往往发生在图形已收缩消失
    /// 之后（静音先于 80 ms 持续阈值把轨迹画没），孵化粒子必须用它。
    last_trace_grid: Vec<u8>,
    last_vel_field: Vec<[f32; 2]>,
    /// 每点一份的单位速度方向（打断瞬间的轨迹运动方向），随轨迹一起写入。
    vel_field: Vec<[f32; 2]>,
    particles: Vec<Particle>,
    phase: Phase,
    /// 峰值保持的当前值（自动缩放的分母记忆）。
    scale_peak: f32,
    /// 渲染线程观测到的窗口原始峰值，`tick` 消费。`None` 表示环里没有样本。
    observed_level: Option<f32>,
    /// 面板尺寸（单元格数），`render` 时刷新；缩放与光栅都以此为坐标系。
    w_cells: usize,
    h_cells: usize,
    /// Active 期连续低于静音电平的时长。
    silent_for: Duration,
    /// 本次分散是否由「播放中的静音」触发（决定恢复的条件是出声还是恢复播放）。
    dispersed_by_silence: bool,
    /// 分散代数：给确定性抖动换种子，两次暂停的散开形态不同。
    disperse_seed: u32,
    /// 悬浮已经过的时间。
    float_elapsed: Duration,
    /// 突断判定用的近期电平（快衰减峰值保持）。
    recent_level: f32,
    /// 图形连续不可见的时长。
    invisible_for: Duration,
    /// 最近一次光栅是否真的画出了轨迹（电平高于可见下限）。
    last_drawn: bool,
}

impl VectorState {
    /// 时间推进：峰值保持的释放与打断动画状态机。
    pub(crate) fn tick(&mut self, enabled: bool, playing: bool, dt: Duration) {
        if !enabled {
            self.reset();
            return;
        }

        // 释放按时间取幂，掉帧时缩放轨迹不变（与 ScopeGain 同一纪律）。
        let release = 10.0_f32
            .powf(PEAK_RELEASE_DB_S / 20.0)
            .powf(dt.as_secs_f32());
        self.scale_peak = self
            .observed_level
            .unwrap_or(0.0)
            .max(self.scale_peak * release);

        let level = self.observed_level.unwrap_or(0.0);
        // 突断判定的近期电平（快衰减峰值保持）：渐弱时紧贴当前电平，
        // 突断时停在断前电平约百毫秒，两者的差值即「突断」与「渐弱」的判据。
        let recent_decay = 10.0_f32
            .powf(RECENT_DECAY_DB_S / 20.0)
            .powf(dt.as_secs_f32());
        self.recent_level = level.max(self.recent_level * recent_decay);

        // 图形不可见的时长：超过宽限期就退役最后一幅可见轨迹，免得渐弱
        // 消失许久后的暂停凭空散出「幽灵」粒子。
        if self.last_drawn {
            self.invisible_for = Duration::ZERO;
        } else {
            self.invisible_for += dt;
            if self.invisible_for >= GHOST_GRACE {
                self.last_trace_grid.clear();
                self.last_vel_field.clear();
            }
        }

        match self.phase {
            Phase::Active => {
                if !playing {
                    self.disperse(false);
                } else {
                    if level < SILENCE_FLOOR {
                        self.silent_for += dt;
                    } else {
                        self.silent_for = Duration::ZERO;
                    }
                    if self.silent_for >= SILENCE_SUSTAIN {
                        // 只认「突断」：断前电平可闻，且当前电平相对它跌掉
                        // SUDDEN_DROP_RATIO 以上。渐弱到达静音时近期电平已
                        // 随之衰减，两个条件都不满足 —— 图形按幅度消失即可。
                        let sudden = self.recent_level >= SUDDEN_MIN_LEVEL
                            && self.recent_level * SUDDEN_DROP_RATIO > level;
                        if sudden {
                            self.disperse(true);
                        }
                    }
                }
            }
            Phase::Dispersing => {
                if self.resume_signal(playing, level) {
                    self.phase = Phase::Recovering;
                } else {
                    self.integrate_disperse(dt);
                }
            }
            Phase::Floating => {
                if self.resume_signal(playing, level) {
                    self.phase = Phase::Recovering;
                } else {
                    self.float_elapsed += dt;
                }
            }
            Phase::Recovering => {
                if !playing {
                    // 回位途中再次暂停：以当下运动方向重新散开。
                    self.reburst();
                } else {
                    self.integrate_homing(dt);
                    if self.particles.is_empty() {
                        self.phase = Phase::Active;
                    }
                }
            }
        }
    }

    /// 渲染线程每帧报告窗口原始峰值与面板尺寸。
    pub(crate) fn observe(&mut self, level: Option<f32>, w_cells: usize, h_cells: usize) {
        self.observed_level = level;
        self.w_cells = w_cells;
        self.h_cells = h_cells;
    }

    /// 快动画（分散 / 回位）进行中：需要 `spectrum_hz` 高帧率推完。
    pub(crate) fn is_animating(&self) -> bool {
        matches!(self.phase, Phase::Dispersing | Phase::Recovering)
    }

    /// 停稳后的极慢悬浮：图形静止但仍在动，需要基础帧率持续重绘。
    pub(crate) fn is_floating(&self) -> bool {
        self.phase == Phase::Floating
    }

    fn reset(&mut self) {
        self.scale_peak = 0.0;
        self.observed_level = None;
        self.grid.clear();
        self.trace_grid.clear();
        self.last_trace_grid.clear();
        self.last_vel_field.clear();
        self.vel_field.clear();
        self.particles.clear();
        self.phase = Phase::Active;
        self.silent_for = Duration::ZERO;
        self.float_elapsed = Duration::ZERO;
        self.recent_level = 0.0;
        self.invisible_for = Duration::ZERO;
        self.last_drawn = false;
    }

    /// 分散的恢复条件：静音打断要等声音回来；暂停打断恢复播放即回位
    /// （恢复到静音段则就近找不到锚点，粒子按超时淡出，图形自然消失）。
    fn resume_signal(&self, playing: bool, level: f32) -> bool {
        playing && (level > SILENCE_FLOOR || !self.dispersed_by_silence)
    }

    /// 从当前轨迹光栅孵化粒子：位置 = 点亮点，方向 = 该点记录的轨迹运动方向，
    /// 幅度策展（原始速度每毫秒横扫整个面板，不可直接用）为 2~10 点停距。
    fn disperse(&mut self, by_silence: bool) {
        self.dispersed_by_silence = by_silence;
        self.disperse_seed = self.disperse_seed.wrapping_add(1);
        self.spawn_particles();
        if self.particles.is_empty() {
            return; // 图形本就不可见（渐弱末段），没有可散开的东西
        }
        self.phase = Phase::Dispersing;
        self.silent_for = Duration::ZERO;
        self.float_elapsed = Duration::ZERO;
    }

    fn spawn_particles(&mut self) {
        let (w, h) = (self.w_cells, self.h_cells);
        let (w_px, h_px) = (w * 2, h * 4);
        let (cx, cy) = ((w_px as f32 - 1.0) * 0.5, (h_px as f32 - 1.0) * 0.5);
        let seed = self.disperse_seed;

        let VectorState {
            last_trace_grid: grid,
            last_vel_field: vel_field,
            particles,
            ..
        } = self;
        particles.clear();

        for (cell, &bits) in grid.iter().enumerate() {
            if bits == 0 {
                continue;
            }
            let (cell_x, cell_y) = (cell % w, cell / w);
            for dy in 0..4 {
                for dx in 0..2 {
                    if bits & braille_bit(dx, dy) == 0 {
                        continue;
                    }
                    let (x, y) = (cell_x * 2 + dx, cell_y * 4 + dy);

                    // 方向：该点记录的轨迹运动方向；没有记录（理论不发生）
                    // 则取从中心向外的方向。
                    let v = vel_field.get(y * w_px + x).copied().unwrap_or([0.0, 0.0]);
                    let len = v[0].hypot(v[1]);
                    let (mut dir_x, mut dir_y) = if len > 1.0e-6 {
                        (v[0] / len, v[1] / len)
                    } else {
                        let (ox, oy) = (x as f32 - cx, y as f32 - cy);
                        let ol = ox.hypot(oy).max(1.0);
                        (ox / ol, oy / ol)
                    };

                    // 抖动：停距、时长取自坐标哈希（确定性，无需随机数依赖）。
                    let jx = (x as u32).wrapping_mul(0x9E37_79B1);
                    let jy = (y as u32).wrapping_mul(0x85EB_CA6B);
                    let s = jx ^ jy ^ seed;
                    let stop_dist = BURST_MIN_DIST + (BURST_MAX_DIST - BURST_MIN_DIST) * jitter(s);
                    let burst_time = BURST_MIN_TIME
                        + (BURST_MAX_TIME - BURST_MIN_TIME) * jitter(s.rotate_left(13) ^ 1);
                    // 方向微扰 ±0.4 rad，散开更有机。
                    let a = (jitter(s.rotate_left(7) ^ 2) - 0.5) * 0.8;
                    let (sa, ca) = a.sin_cos();
                    let (ndx, ndy) = (dir_x * ca - dir_y * sa, dir_x * sa + dir_y * ca);
                    dir_x = ndx;
                    dir_y = ndy;

                    let v0 = 2.0 * stop_dist / burst_time;
                    particles.push(Particle {
                        x: x as f32,
                        y: y as f32,
                        vx: dir_x * v0,
                        vy: dir_y * v0,
                        decel: v0 / burst_time,
                        anchor_x: x as f32,
                        anchor_y: y as f32,
                        drift_x: jitter(s.rotate_left(5) ^ 3) * TAU,
                        drift_y: jitter(s.rotate_left(17) ^ 4) * TAU,
                        hx: 0.0,
                        hy: 0.0,
                        lost_for: Duration::ZERO,
                    });
                }
            }
        }
    }

    /// 分散积分：恒定减速度沿 −v̂，速度线性衰减；贴住面板边界即停。
    fn integrate_disperse(&mut self, dt: Duration) {
        let (w_px, h_px) = (self.w_cells as f32 * 2.0, self.h_cells as f32 * 4.0);
        let dt = dt.as_secs_f32();
        let VectorState { particles, .. } = self;

        let mut all_stopped = true;
        for p in particles.iter_mut() {
            let speed = p.vx.hypot(p.vy);
            if speed <= 0.0 {
                continue;
            }
            all_stopped = false;
            let new_speed = (speed - p.decel * dt).max(0.0);
            let k = new_speed / speed;
            p.vx *= k;
            p.vy *= k;
            p.x += p.vx * dt;
            p.y += p.vy * dt;

            // 贴边停住：飞出面板的点留在边缘可见处，而不是消失在虚空。
            if p.x < 0.0 {
                p.x = 0.0;
                p.vx = 0.0;
            }
            if p.x > w_px - 1.0 {
                p.x = w_px - 1.0;
                p.vx = 0.0;
            }
            if p.y < 0.0 {
                p.y = 0.0;
                p.vy = 0.0;
            }
            if p.y > h_px - 1.0 {
                p.y = h_px - 1.0;
                p.vy = 0.0;
            }
        }

        if all_stopped && !particles.is_empty() {
            for p in particles.iter_mut() {
                p.anchor_x = p.x;
                p.anchor_y = p.y;
            }
            self.phase = Phase::Floating;
            self.float_elapsed = Duration::ZERO;
        }
    }

    /// 回位积分：就近锚定轨迹点，指数逼近；贴上吸收，久找不到目标淡出。
    fn integrate_homing(&mut self, dt: Duration) {
        let k = 1.0 - (-dt.as_secs_f32() / HOMING_TAU).exp();
        let (w, h) = (self.w_cells, self.h_cells);

        let VectorState {
            particles,
            trace_grid,
            ..
        } = self;

        let mut i = 0;
        while i < particles.len() {
            let p = &mut particles[i];
            if let Some((tx, ty)) =
                nearest_lit_dot(trace_grid, w, h, p.x.round() as i32, p.y.round() as i32)
            {
                let (dx, dy) = (tx - p.x, ty - p.y);
                let d = dx.hypot(dy);
                if d <= ABSORB_DIST {
                    particles.swap_remove(i);
                    continue;
                }
                p.hx = dx / d;
                p.hy = dy / d;
                p.x += dx * k;
                p.y += dy * k;
                p.lost_for = Duration::ZERO;
            } else {
                p.lost_for += dt;
                if p.lost_for >= RECOVER_TIMEOUT {
                    particles.swap_remove(i);
                    continue;
                }
            }
            i += 1;
        }
    }

    /// 回位途中再次打断：以当下运动方向重新散开（粒子已在空中，不重新孵化）。
    fn reburst(&mut self) {
        let seed = self.disperse_seed.wrapping_add(1);
        self.disperse_seed = seed;
        self.dispersed_by_silence = false;

        for p in self.particles.iter_mut() {
            let mut dir_x = p.hx;
            let mut dir_y = p.hy;
            if dir_x.hypot(dir_y) < 1.0e-6 {
                dir_x = 1.0;
                dir_y = 0.0;
            }
            let s = (p.x as u32).wrapping_mul(0x9E37_79B1) ^ seed;
            let stop_dist = BURST_MIN_DIST + (BURST_MAX_DIST - BURST_MIN_DIST) * jitter(s);
            let burst_time = BURST_MIN_TIME + (BURST_MAX_TIME - BURST_MIN_TIME) * jitter(s ^ 5);
            let v0 = 2.0 * stop_dist / burst_time;
            p.vx = dir_x * v0;
            p.vy = dir_y * v0;
            p.decel = v0 / burst_time;
        }
        self.phase = Phase::Dispersing;
        self.float_elapsed = Duration::ZERO;
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
    /// 依相位机产出本帧复合光栅：轨迹、粒子或两者叠加。
    fn rasterize(&mut self) {
        match self.phase {
            Phase::Active | Phase::Recovering => {
                self.rasterize_trace();
                self.grid.clear();
                self.grid.resize(self.trace_grid.len(), 0);
                self.grid.copy_from_slice(&self.trace_grid);
                if self.phase == Phase::Recovering {
                    self.stamp_particles();
                }
            }
            Phase::Dispersing | Phase::Floating => {
                self.grid.clear();
                self.grid.resize(self.w_cells * self.h_cells, 0);
                self.stamp_particles();
            }
        }
    }

    /// 依当前缩放把窗口样本画成李萨如轨迹（同时维护速度场）。
    fn rasterize_trace(&mut self) {
        let (w_cells, h_cells) = (self.w_cells, self.h_cells);
        let VectorState {
            snapshot,
            trace_grid,
            vel_field,
            last_trace_grid,
            last_vel_field,
            last_drawn,
            scale_peak,
            observed_level,
            ..
        } = self;

        trace_grid.clear();
        trace_grid.resize(w_cells * h_cells, 0);
        vel_field.clear();
        vel_field.resize(w_cells * 2 * h_cells * 4, [0.0, 0.0]);

        *last_drawn = false;
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
        let mut last_dir = [1.0f32, 0.0f32];
        for i in 0..n {
            // X=左声道、Y=右声道（屏幕 y 向下，故取负）。
            let (x, y) = (cx + left[i] * scale, cy - right[i] * scale);
            if let Some((x0, y0)) = prev {
                let (dx, dy) = (x - x0, y - y0);
                let len = dx.hypot(dy);
                if len > 1.0e-6 {
                    last_dir = [dx / len, dy / len];
                }
                draw_segment(
                    trace_grid,
                    vel_field,
                    w_cells,
                    h_cells,
                    (x0, y0),
                    (x, y),
                    last_dir,
                );
            }
            prev = Some((x, y));
        }

        *last_drawn = true;
        // 画出来了：留存「最后一幅可见轨迹」。打断往往发生在图形已因静音
        // 收缩消失之后（80 ms 持续阈值慢于可见下限），孵化粒子必须用它。
        // 拷贝量在百 KB 量级、每帧一次，相对光栅化为噪声。
        last_trace_grid.clear();
        last_trace_grid.resize(trace_grid.len(), 0);
        last_trace_grid.copy_from_slice(trace_grid);
        last_vel_field.clear();
        last_vel_field.resize(vel_field.len(), [0.0, 0.0]);
        last_vel_field.copy_from_slice(vel_field);
    }

    /// 把粒子盖进复合光栅。悬浮期位置 = 停稳点 + 极慢正弦漂移。
    fn stamp_particles(&mut self) {
        let (w, h, phase, t) = (
            self.w_cells,
            self.h_cells,
            self.phase,
            self.float_elapsed.as_secs_f32(),
        );
        let grid = &mut self.grid;
        for p in &self.particles {
            let (x, y) = particle_pos(phase, t, p);
            set_pixel(grid, w, h, x.round() as i32, y.round() as i32);
        }
    }
}

/// 粒子当前显示位置：悬浮期在停稳点周围 3×3 区域内极慢漂移，其余相位
/// 即积分位置。
fn particle_pos(phase: Phase, float_elapsed_s: f32, p: &Particle) -> (f32, f32) {
    if phase == Phase::Floating {
        let ox = FLOAT_AMPLITUDE * (TAU * float_elapsed_s / FLOAT_PERIOD_S + p.drift_x).sin();
        let oy =
            FLOAT_AMPLITUDE * (TAU * float_elapsed_s / (FLOAT_PERIOD_S * 1.13) + p.drift_y).sin();
        (p.anchor_x + ox, p.anchor_y + oy)
    } else {
        (p.x, p.y)
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

/// 相邻样本连线：沿较长轴步进保证不断点；每点亮起时记录轨迹运动方向，
/// 打断瞬间它就是粒子的「瞬时运动方向」（后写覆盖 = 最新一段的方向）。
fn draw_segment(
    grid: &mut [u8],
    vel_field: &mut [[f32; 2]],
    w_cells: usize,
    h_cells: usize,
    from: (f32, f32),
    to: (f32, f32),
    dir: [f32; 2],
) {
    let ((x0, y0), (x1, y1)) = (from, to);
    let (dx, dy) = (x1 - x0, y1 - y0);
    let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as usize;
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let x = (x0 + dx * t).round() as i32;
        let y = (y0 + dy * t).round() as i32;
        set_pixel(grid, w_cells, h_cells, x, y);
        if let Some(v) = vel_field.get_mut(y.max(0) as usize * w_cells * 2 + x.max(0) as usize) {
            *v = dir;
        }
    }
}

/// 就近锚定：从 (x0, y0) 由内向外逐环找最近的轨迹点（Chebyshev 环序）。
fn nearest_lit_dot(
    grid: &[u8],
    w_cells: usize,
    h_cells: usize,
    x0: i32,
    y0: i32,
) -> Option<(f32, f32)> {
    for r in 0..=HOMING_RADIUS {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dy.abs() != r {
                    continue; // 只走第 r 环
                }
                let (x, y) = (x0 + dx, y0 + dy);
                if x < 0 || y < 0 || x >= w_cells as i32 * 2 || y >= h_cells as i32 * 4 {
                    continue;
                }
                let (ux, uy) = (x as usize, y as usize);
                let bits = grid[(uy / 4) * w_cells + ux / 2];
                if bits & braille_bit(ux % 2, uy % 4) != 0 {
                    return Some((x as f32, y as f32));
                }
            }
        }
    }
    None
}

/// 坐标哈希 → [0,1)：确定性抖动，替代随机数依赖。
fn jitter(seed: u32) -> f32 {
    let mut z = seed.wrapping_mul(0x9E37_79B1).wrapping_add(0x7F4A_7C15);
    z ^= z >> 15;
    z = z.wrapping_mul(0x85EB_CA6B);
    z ^= z >> 13;
    z = z.wrapping_mul(0xC2B2_AE35);
    z ^= z >> 16;
    (z >> 8) as f32 / 16_777_216.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;
    const FRAME: Duration = Duration::from_millis(33);

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
                    if bits & braille_bit(dx, dy) != 0 {
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

    /// 走一帧完整循环：observe → tick → rasterize（模拟事件循环顺序）。
    fn frame(st: &mut VectorState, playing: bool) {
        st.rasterize();
        let level = st.observed_level;
        let _ = level;
        st.tick(true, playing, FRAME);
        st.rasterize();
    }

    fn circle_state(amp: f32) -> VectorState {
        let mut st = VectorState {
            snapshot: synth(882, circle(amp, std::f32::consts::FRAC_PI_2)),
            ..Default::default()
        };
        st.observe(window_level(&st.snapshot), 40, 20);
        // 完整走一帧（先画后 tick）：ghost 计时器要看到「画出来了」才复位，
        // 与事件循环每帧 observe→tick→draw 的顺序一致。
        st.rasterize();
        st.tick(true, true, Duration::from_secs(1));
        st.rasterize();
        st
    }

    /// 110 Hz 双声道相位差 90°：以 0.8 幅度画出一个居中的大圆。
    #[test]
    fn lissajous_draws_centered_figure() {
        let st = circle_state(0.8);
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
            let st = circle_state(amp);
            let (min_x, max_x, _, _) = bbox(&lit_dots(&st));
            assert!(
                (max_x - min_x) as f32 >= 80.0 * 0.6,
                "幅度 {amp} 应自动缩放到接近满幅"
            );
        }

        let mut st = circle_state(1.0e-5);
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

    /// 暂停打断：轨迹点全部化为粒子，恒定减速散开并在限时内停稳；
    /// 停稳后悬浮在终点周围 3×3 区域内极慢漂移。
    #[test]
    fn pause_disperses_settles_and_floats() {
        let mut st = circle_state(0.8);
        let dots_before = lit_dots(&st).len();
        assert!(dots_before > 100);

        // 暂停 → 分散。
        st.tick(true, false, FRAME);
        assert_eq!(st.phase, Phase::Dispersing, "暂停立即分散");
        assert_eq!(st.particles.len(), dots_before, "每个点亮点一个粒子");

        // 记下起点，推完爆发（上限 BURST_MAX_TIME）。
        let starts: Vec<(f32, f32, f32)> = st
            .particles
            .iter()
            .map(|p| (p.x, p.y, p.vx.hypot(p.vy)))
            .collect();
        for _ in 0..40 {
            st.tick(true, false, FRAME);
        }
        assert_eq!(st.phase, Phase::Floating, "全部停稳进入悬浮");
        for (i, p) in st.particles.iter().enumerate() {
            assert_eq!(p.vx, 0.0, "粒子 {i} 应停稳");
            assert_eq!(p.vy, 0.0);
            let moved = (p.x - starts[i].0).hypot(p.y - starts[i].1);
            assert!(
                moved <= BURST_MAX_DIST + 1.0,
                "粒子 {i} 位移 {moved} 超出停距上界"
            );
            assert!(moved > 0.0 || starts[i].2 == 0.0, "粒子 {i} 应有位移");
        }

        // 悬浮：任意时刻都在停稳点 3×3 邻域内，且不同时刻位置确在极慢变化。
        let mut seen: Vec<(i32, i32)> = Vec::new();
        for _ in 0..30 {
            st.tick(true, false, Duration::from_millis(500));
            st.rasterize();
            for p in &st.particles {
                let (x, y) = particle_pos(st.phase, st.float_elapsed.as_secs_f32(), p);
                let (ox, oy) = (x - p.anchor_x, y - p.anchor_y);
                assert!(
                    ox.abs() <= 1.0 && oy.abs() <= 1.0,
                    "悬浮越出 3×3 区域：({ox:.2},{oy:.2})"
                );
            }
            if seen.is_empty() {
                seen = st
                    .particles
                    .iter()
                    .map(|p| {
                        let (x, y) = particle_pos(st.phase, st.float_elapsed.as_secs_f32(), p);
                        (x as i32, y as i32)
                    })
                    .collect();
            }
        }
        let now: Vec<(i32, i32)> = st
            .particles
            .iter()
            .map(|p| {
                let (x, y) = particle_pos(st.phase, st.float_elapsed.as_secs_f32(), p);
                (x as i32, y as i32)
            })
            .collect();
        // 15 s 的极慢漂移（周期 12 s）必然至少移动一个粒子一格。
        assert!(
            now.iter().zip(&seen).any(|(a, b)| a != b),
            "悬浮应随时间极慢改变取整位置"
        );
    }

    /// 播放中突然静音：持续超过 SILENCE_SUSTAIN 判打断；短于则不打断。
    #[test]
    fn sudden_silence_disperses_after_sustain() {
        let mut st = circle_state(0.8);
        st.observe(Some(1.0e-5), 40, 20);
        st.rasterize(); // 静音帧的轨迹已不可见
        st.tick(true, true, Duration::from_millis(40));
        assert_eq!(st.phase, Phase::Active, "40 ms 低于持续阈值不打断");
        eprintln!(
            "DBG after40 recent={} lastlit={}",
            st.recent_level,
            st.last_trace_grid.iter().filter(|&&b| b != 0).count()
        );
        st.tick(true, true, Duration::from_millis(60));
        eprintln!(
            "DBG after60 recent={} silent={:?} lastlit={}",
            st.recent_level,
            st.silent_for,
            st.last_trace_grid.iter().filter(|&&b| b != 0).count()
        );
        assert_eq!(st.phase, Phase::Dispersing, "80 ms 起判突然静音");
    }

    /// 渐弱不触发分散：−20 dB/s 的滑落（快于多数真实淡出）紧贴近期电平，
    /// 差值判据永不满足；图形按幅度缩到消失。
    #[test]
    fn gradual_fade_disappears_without_dispersing() {
        let mut st = circle_state(0.8);
        // 3 s、共 −60 dB 的指数渐弱，尾接静音并远超持续阈值。
        for f in 0..90 {
            let level = (0.8 * 10.0_f32.powf(-3.0 * f as f32 / 90.0)).max(1.0e-5);
            st.observe(Some(level), 40, 20);
            st.rasterize();
            st.tick(true, true, Duration::from_millis(33));
            assert_eq!(st.phase, Phase::Active, "渐弱帧 {f} 不得分散");
        }
        for _ in 0..10 {
            st.observe(Some(1.0e-5), 40, 20);
            st.rasterize();
            st.tick(true, true, Duration::from_millis(33));
        }
        assert_eq!(st.phase, Phase::Active, "静音持续后仍不分散（差值判据）");
        assert!(st.particles.is_empty());
        assert!(lit_dots(&st).is_empty(), "渐弱的最终是图形消失");
    }

    /// 渐弱消失超过宽限期后的暂停：最后一幅可见轨迹已退役，不得散出幽灵。
    #[test]
    fn pause_long_after_fade_scatters_nothing() {
        let mut st = circle_state(0.8);
        // 3 s 渐弱到不可见（差值判据不触发），随后 2 s 完全静音。
        for f in 0..90 {
            let level = (0.8 * 10.0_f32.powf(-3.0 * f as f32 / 90.0)).max(1.0e-6);
            st.observe(Some(level), 40, 20);
            st.rasterize();
            st.tick(true, true, Duration::from_millis(33));
            assert_eq!(st.phase, Phase::Active, "渐弱帧 {f} 不得分散");
        }
        for _ in 0..60 {
            st.observe(Some(1.0e-6), 40, 20);
            st.rasterize();
            st.tick(true, true, Duration::from_millis(33));
        }
        assert_eq!(st.phase, Phase::Active);
        assert!(st.last_trace_grid.is_empty(), "宽限期后幽灵轨迹应退役");

        st.tick(true, false, Duration::from_millis(33));
        assert_eq!(st.phase, Phase::Active, "无可散开的图形");
        assert!(st.particles.is_empty());
    }

    /// 恢复播放：粒子就近锚定轨迹点，指数逼近迅速归位并吸收。
    #[test]
    fn resume_homes_particles_to_pattern() {
        let mut st = circle_state(0.8);
        st.tick(true, false, FRAME); // 暂停分散
        for _ in 0..40 {
            st.tick(true, false, FRAME);
        }
        assert_eq!(st.phase, Phase::Floating);

        // 恢复播放且有声：进入回位，轨迹与粒子同画。
        st.observe(Some(0.8), 40, 20);
        st.tick(true, true, FRAME);
        assert_eq!(st.phase, Phase::Recovering);

        // 模拟帧循环（rasterize 重建轨迹供锚定），0.5 s 内全部归位。
        for _ in 0..15 {
            st.rasterize();
            st.tick(true, true, FRAME);
        }
        assert!(st.particles.is_empty(), "粒子应全部吸收");
        assert_eq!(st.phase, Phase::Active);
        st.rasterize();
        assert!(!lit_dots(&st).is_empty(), "回位后应恢复完整轨迹图形");
    }

    /// 图形本已消失（渐弱末段 / 无样本）时打断：无可散开，维持 Active。
    #[test]
    fn disperse_with_nothing_lit_stays_active() {
        let mut st = VectorState::default();
        st.observe(None, 40, 20);
        st.rasterize();
        st.tick(true, false, FRAME);
        assert_eq!(st.phase, Phase::Active);
        assert!(st.particles.is_empty());
    }
}
