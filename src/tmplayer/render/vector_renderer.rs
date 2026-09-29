//! 全屏页「矢量模式」：把 PCM 波形画成李萨如图。
//!
//! 数据源与示波器相同（`audio::pcm_tap` 的共享环）。屏幕正交轴、向上恒为
//! y 正方向：**横向 x = 左声道 L（向右为正），纵向 y = 右声道 R（向上为
//! 正）**。面板中心为原点，不画坐标轴；单声道（L=R）呈右上 45° 对角线、
//! 纯 L 水平、纯 R 垂直，都是如实呈现而非缺陷。
//!
//! - **无触发、无峰值抽取**：李萨如看的是相位关系而非周期计数，取最近一段
//!   短窗逐段连线即可，相位图天然稳态。
//! - **缩放以歌曲最大图为基准**：分母 = 本曲开播以来观测到的最大峰值
//!   （只增不减），最响的段落恰好撑满面板；出现更响段落时基准一次性上调
//!   —— 单调、无 AGC 抽动，仅切歌（PCM 环重置）时重新开始。渐弱时图形
//!   随实际电平相对基准缩小，最终消失。
//!
//! **打断动画**（相位机 [`Phase`]）：
//!
//! - 暂停或**突断**静音（差值判据，渐弱不触发）→ 图形炸开：每个盲文点
//!   飞向可视化区域内的一个**随机落点**（恒定大减速度，0.25~0.55 s 先后
//!   停稳），停稳后不保留原图形的剪影。粒子数有上限 [`MAX_PARTICLES`]，
//!   超出的点原地错峰淡出后退役（不参与轮换，也不瞬间消失）。
//! - 停稳后 → **自刷新（连续呼吸）**：各点从全亮错峰进入「3 s 淡出 →
//!   随机换位置 → 3 s 淡入 → 立即再淡出」的循环。淡变的「透明度」由
//!   颜色向面板底色的平滑插值模拟（终端没有真透明度）。
//! - 恢复播放（或声音回来）→ **聚集回归**：粒子先有一个小小的点火延迟
//!   （0~0.12 s，读作陆续启程），再就近锚定当前图形指数逼近
//!   （τ = [`HOMING_TAU`]，全程约 0.5 s，肉眼可见的汇聚流），贴上即吸收。
//!
//! 盲文光栅（2×4 点/格、逐点写位、行渐变配色）与示波器共用同一套
//! [`paint`]。

use crate::tmplayer::app::state::AppState;
use crate::tmplayer::audio::pcm_tap::PcmSnapshot;
use crate::tmplayer::render::oscilloscope_renderer::{braille_bit, set_pixel};
use crate::tmplayer::ui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
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

/// 低于此电平不再画任何点：避免收缩末段在原点残留一个孤点。
const VISIBILITY_MIN: f32 = SCALE_FLOOR * 0.1;

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

/// 分散粒子数上限：高密度图形（200 列终端可达数千点）全部参与轮换既
/// 看不清也浪费；按步长抽样保留这么多粒子，其余以 Dying 相位原地
/// 错峰淡出后退役（不参与后续轮换）。
const MAX_PARTICLES: usize = 1200;

/// 多余点的原地淡出时长（秒）：错峰铺开，剪影随飞行同步消解。
const SURPLUS_FADE_MIN: f32 = 0.5;
const SURPLUS_FADE_MAX: f32 = 2.5;

/// 动画时钟的单 tick 最大推进量：帧间隔偶发到秒级（单线程运行时被
/// 宿主桥/下载任务阻塞）时，相位机仍按 ≤100 ms 步进，淡变不会被
/// 整段跳过。正常 30 fps 下永不触及。
const ANIM_MAX_STEP: Duration = Duration::from_millis(100);

/// 分散落点距可视化区域边缘的最小距离（点）：尘埃不贴边框。
const SCATTER_MARGIN: f32 = 2.0;

/// 粒子飞行时长（秒）：各点先后停稳，读作「炸开后停留」。
const BURST_MIN_TIME: f32 = 0.25;
const BURST_MAX_TIME: f32 = 0.55;

/// 淡入/淡出时长（秒）：粒子颜色（模拟透明度）在波形色与面板底色之间
/// 平滑插值 —— 单点是二值的，透明感完全由这段颜色渐变承担。
const TWINKLE_FADE_S: f32 = 3.0;

/// 聚集回归的点火延迟上限（秒）：粒子陆续启程，汇聚流更可读。
const GATHER_IGNITION_S: f32 = 0.12;

/// 聚集回归的时间常数（秒）：约 0.5 s 走完 95%，肉眼可见的快速汇聚；
/// 再快（如 0.04）就退化成三帧内的瞬吸，看不出「聚集」。
const HOMING_TAU: f32 = 0.15;

/// 聚集锚定搜索的起始半径与扩张速率（点 / 点每秒）：随机散布的粒子离
/// 图形可远可近，扩张保证全部粒子都能锚定到图形、参与回归。
const HOMING_START_RADIUS: i32 = 16;
const HOMING_EXPAND_DPS: f32 = 500.0;

/// 距锚点近到这一步即视为归位（吸收）。
const ABSORB_DIST: f32 = 1.0;

/// 聚集期始终找不到锚点（图形不存在）的粒子最多滞留此时长。
const GATHER_TIMEOUT: Duration = Duration::from_millis(800);

/// 打断动画的相位机。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Phase {
    /// 跟随波形正常绘制。
    #[default]
    Active,
    /// 粒子飞向随机落点，减速中。
    Dispersing,
    /// 全部停稳，尘埃自刷新（错峰淡出 / 随机位置淡入）。
    Floating,
    /// 聚集回归：粒子锚定当前图形，指数逼近归位。
    Recovering,
}

/// 停稳尘埃的自刷新相位。`left` 为该相位剩余秒数。
#[derive(Debug, Clone, Copy)]
enum Twinkle {
    Visible {
        left: f32,
    },
    FadingOut {
        left: f32,
    },
    FadingIn {
        left: f32,
    },
    /// 超出 [`MAX_PARTICLES`] 的多余点：原地错峰淡出，到点即退役
    ///（不再参与轮换）。`total` 记初始时长用于算透明度。
    Dying {
        left: f32,
        total: f32,
    },
}

impl Twinkle {
    /// 当前透明度（1 = 波形色，0 = 与底色同）。
    fn alpha(&self) -> f32 {
        match *self {
            Twinkle::Visible { .. } => 1.0,
            Twinkle::FadingOut { left } => (left / TWINKLE_FADE_S).clamp(0.0, 1.0),
            Twinkle::FadingIn { left } => (1.0 - left / TWINKLE_FADE_S).clamp(0.0, 1.0),
            Twinkle::Dying { left, total } => (left / total.max(1.0e-3)).clamp(0.0, 1.0),
        }
    }

    /// 剩余秒数（供推进）。
    fn left(&self) -> f32 {
        match *self {
            Twinkle::Visible { left }
            | Twinkle::FadingOut { left }
            | Twinkle::FadingIn { left }
            | Twinkle::Dying { left, .. } => left,
        }
    }

    fn left_mut(&mut self) -> &mut f32 {
        match self {
            Twinkle::Visible { left }
            | Twinkle::FadingOut { left }
            | Twinkle::FadingIn { left }
            | Twinkle::Dying { left, .. } => left,
        }
    }
}

/// 一个被打断的盲文点。位置与速度都在子像素（点阵）坐标系，单位为点。
#[derive(Debug, Clone, Copy)]
struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    /// 恒定减速度（点/s²）：`v0 / 飞行时长`，速度线性衰减到 0。
    decel: f32,
    /// 自刷新相位（可见 / 淡出 / 淡入），只在 Floating 相位生效。
    twinkle: Twinkle,
    /// 聚集点火剩余延迟：归零前原地不动，读作「陆续启程」。
    ignition: f32,
}

/// 矢量模式的全部可变状态：李萨如光栅、缩放基准与打断动画。
///
/// 存放在 [`AppState`] 里（与示波器的 `ScopeScratch` 同位），渲染路径零分配。
#[derive(Debug, Default)]
pub struct VectorState {
    pub(crate) snapshot: PcmSnapshot,
    /// 复合光栅（轨迹 + 粒子）：`paint` 直写帧缓冲的就是它。
    grid: Vec<u8>,
    /// 每单元格的粒子透明度（同格多粒子取最大）：淡入淡出的颜色插值依据。
    cell_alpha: Vec<f32>,
    /// 仅轨迹的光栅：聚集锚定的搜索目标，避免粒子互相吸附。
    trace_grid: Vec<u8>,
    /// 最后一幅**画出来了的**轨迹：打断往往发生在图形已收缩消失之后
    /// （静音先于 80 ms 持续阈值把轨迹画没），孵化粒子必须用它。
    last_trace_grid: Vec<u8>,
    particles: Vec<Particle>,
    phase: Phase,
    /// 缩放分母 = 本曲开播以来观测到的最大电平（只增不减，切歌重置）。
    scale_peak: f32,
    /// 观测到「无样本」（PCM 环重置）后，下次样本出现即重置缩放基准。
    need_recalib: bool,
    /// 渲染线程观测到的窗口原始峰值，`tick` 消费。`None` 表示环里没有样本。
    observed_level: Option<f32>,
    /// 面板尺寸（单元格数），`render` 时刷新；缩放与光栅都以此为坐标系。
    w_cells: usize,
    h_cells: usize,
    /// Active 期连续低于静音电平的时长。
    silent_for: Duration,
    /// 本次分散是否由「播放中的静音」触发（决定恢复的条件是出声还是恢复播放）。
    dispersed_by_silence: bool,
    /// 分散代数：给确定性抖动换种子，两次打断的散开形态不同。
    disperse_seed: u32,
    /// 自刷新与聚集各自已经过的时间。
    float_elapsed: Duration,
    gather_elapsed: Duration,
    /// 突断判定用的近期电平（快衰减峰值保持）。
    recent_level: f32,
    /// 图形连续不可见的时长。
    invisible_for: Duration,
    /// 最近一次光栅是否真的画出了轨迹（电平高于可见下限）。
    last_drawn: bool,
}

impl VectorState {
    /// 时间推进：缩放基准、突断判定与打断动画相位机。
    pub(crate) fn tick(&mut self, enabled: bool, playing: bool, dt: Duration) {
        if !enabled {
            self.reset();
            return;
        }

        let level = self.observed_level.unwrap_or(0.0);
        self.update_scale_reference(level);
        self.update_silence_context(level, dt);

        // 动画时钟钳步：本应用跑在单线程运行时上，宿主桥与后台下载
        // 可能让帧间隔偶发到秒级；若直接用大 dt 推进相位机，3 s 的淡出
        // 会被一帧整段跳过（实测症状「1→瞬0→淡入→突0」）。每 tick 最多
        // 推进 100 ms，任何淡变相位都必然经历完整的渲染帧序列；
        // 静音判定与缩放基准仍用真实 dt（时间语义）。
        let anim_dt = dt.min(ANIM_MAX_STEP);
        match self.phase {
            Phase::Active => self.tick_active(playing, level, dt),
            Phase::Dispersing => self.tick_dispersing(playing, level, anim_dt),
            Phase::Floating => self.tick_floating(playing, level, anim_dt),
            Phase::Recovering => self.tick_recovering(playing, anim_dt),
        }
    }

    /// 渲染线程每帧报告窗口原始峰值与面板尺寸。
    pub(crate) fn observe(&mut self, level: Option<f32>, w_cells: usize, h_cells: usize) {
        self.observed_level = level;
        self.w_cells = w_cells;
        self.h_cells = h_cells;
    }

    /// 快动画（分散 / 聚集回归）进行中：需要 `spectrum_hz` 高帧率推完。
    pub(crate) fn is_animating(&self) -> bool {
        matches!(self.phase, Phase::Dispersing | Phase::Recovering)
    }

    /// 停稳后的尘埃自刷新仍在动，需要基础帧率持续重绘。
    pub(crate) fn is_floating(&self) -> bool {
        self.phase == Phase::Floating
    }

    fn reset(&mut self) {
        self.scale_peak = 0.0;
        self.observed_level = None;
        self.grid.clear();
        self.cell_alpha.clear();
        self.trace_grid.clear();
        self.last_trace_grid.clear();
        self.particles.clear();
        self.need_recalib = true;
        self.phase = Phase::Active;
        self.silent_for = Duration::ZERO;
        self.float_elapsed = Duration::ZERO;
        self.gather_elapsed = Duration::ZERO;
        self.recent_level = 0.0;
        self.invisible_for = Duration::ZERO;
        self.last_drawn = false;
    }

    // ---- 慢变化上下文：缩放基准 / 近期电平 / 幽灵退役 ----

    /// 缩放基准 = 本曲开播以来的最大电平（只增不减），切歌重置。
    fn update_scale_reference(&mut self, level: f32) {
        if self.observed_level.is_none() {
            self.need_recalib = true;
        }
        if self.need_recalib && self.observed_level.is_some() {
            self.need_recalib = false;
            self.scale_peak = 0.0;
        }
        self.scale_peak = self.scale_peak.max(level);
    }

    /// 突断判定用的近期电平（快衰减峰值保持）与幽灵轨迹退役计时。
    fn update_silence_context(&mut self, level: f32, dt: Duration) {
        let decay = 10.0_f32
            .powf(RECENT_DECAY_DB_S / 20.0)
            .powf(dt.as_secs_f32());
        self.recent_level = level.max(self.recent_level * decay);

        if self.last_drawn {
            self.invisible_for = Duration::ZERO;
        } else {
            self.invisible_for += dt;
            if self.invisible_for >= GHOST_GRACE {
                self.last_trace_grid.clear();
            }
        }
    }

    // ---- 相位机 ----

    fn tick_active(&mut self, playing: bool, level: f32, dt: Duration) {
        if !playing {
            self.scatter(false);
            return;
        }
        if level < SILENCE_FLOOR {
            self.silent_for += dt;
        } else {
            self.silent_for = Duration::ZERO;
        }
        if self.silent_for >= SILENCE_SUSTAIN {
            // 只认「突断」：断前电平可闻，且当前电平相对它跌掉
            // SUDDEN_DROP_RATIO 以上。渐弱到达静音时近期电平已随之衰减，
            // 两个条件都不满足 —— 图形按幅度消失即可。
            let sudden = self.recent_level >= SUDDEN_MIN_LEVEL
                && self.recent_level * SUDDEN_DROP_RATIO > level;
            if sudden {
                self.scatter(true);
            }
        }
    }

    fn tick_dispersing(&mut self, playing: bool, level: f32, dt: Duration) {
        if self.resume_signal(playing, level) {
            self.begin_gather();
            return;
        }
        let mut all_stopped = true;
        let (w_px, h_px) = (self.w_cells as f32 * 2.0, self.h_cells as f32 * 4.0);
        let dt_s = dt.as_secs_f32();
        for p in &mut self.particles {
            let speed = p.vx.hypot(p.vy);
            if speed <= 0.0 {
                continue;
            }
            all_stopped = false;
            let new_speed = (speed - p.decel * dt_s).max(0.0);
            let k = new_speed / speed;
            p.vx *= k;
            p.vy *= k;
            p.x += p.vx * dt_s;
            p.y += p.vy * dt_s;
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
        // 多余点在飞行期间就开始原地淡出（剪影随之消解，而不是等停稳）；
        // 到期的在此移除，未到期的进入 Floating 后继续淡。
        self.advance_dying(dt);
        if all_stopped && !self.particles.is_empty() {
            // 停稳：进入连续呼吸。各点先全亮停留一段错峰（铺满整个循环
            // 周期），然后独立循环「3 s 淡出 → 随机换位置 → 3 s 淡入 →
            // 立即再淡出」；Dying 粒子保持退役进程，不重新加入。
            let seed = self.disperse_seed;
            for i in 0..self.particles.len() {
                if matches!(self.particles[i].twinkle, Twinkle::Dying { .. }) {
                    continue;
                }
                let s = dot_seed(
                    (
                        self.particles[i].x.max(0.0) as usize,
                        self.particles[i].y.max(0.0) as usize,
                    ),
                    seed,
                );
                self.particles[i].twinkle = Twinkle::Visible {
                    left: stagger_span(s),
                };
            }
            self.phase = Phase::Floating;
            self.float_elapsed = Duration::ZERO;
        }
    }

    /// 推进 Dying 粒子并移除到期的。返回是否有粒子仍在淡出（未全部停稳）。
    fn advance_dying(&mut self, dt: Duration) -> bool {
        let dt_s = dt.as_secs_f32();
        let mut i = 0;
        let mut any_alive = false;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            if !matches!(p.twinkle, Twinkle::Dying { .. }) {
                i += 1;
                continue;
            }
            if p.twinkle.left() > dt_s {
                *p.twinkle.left_mut() -= dt_s;
                any_alive = true;
                i += 1;
            } else {
                // 淡出完成：退役（移除）。
                self.particles.swap_remove(i);
            }
        }
        any_alive
    }

    fn tick_floating(&mut self, playing: bool, level: f32, dt: Duration) {
        if self.resume_signal(playing, level) {
            self.begin_gather();
            return;
        }
        self.float_elapsed += dt;

        // 自刷新状态机（连续呼吸）：淡出(3s) → 随机换位置 → 淡入(3s) →
        // 立即再淡出 → …… 各点错峰，整体读作「渐隐后在随机位置渐入」。
        let seed = self.disperse_seed;
        let (w_px, h_px) = (self.w_cells as f32 * 2.0, self.h_cells as f32 * 4.0);
        let dt_s = dt.as_secs_f32();
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            if matches!(p.twinkle, Twinkle::Dying { .. }) {
                // Dying 的推进与退役统一走 advance_dying（本函数末尾）。
                i += 1;
                continue;
            }
            if p.twinkle.left() > dt_s {
                *p.twinkle.left_mut() -= dt_s;
                i += 1;
                continue;
            }
            let s = dot_seed((p.x.max(0.0) as usize, p.y.max(0.0) as usize), seed);
            p.twinkle = match p.twinkle {
                Twinkle::Visible { .. } => Twinkle::FadingOut {
                    left: TWINKLE_FADE_S,
                },
                Twinkle::FadingOut { .. } => {
                    // 淡出完成：随机换位置，立即开始淡入。
                    let (tx, ty) = random_spot(s.rotate_left(29) ^ 11, w_px, h_px);
                    p.x = tx;
                    p.y = ty;
                    Twinkle::FadingIn {
                        left: TWINKLE_FADE_S,
                    }
                }
                Twinkle::FadingIn { .. } => {
                    // 淡入到 1：立即开始下一轮淡出（无全亮驻留期）。
                    Twinkle::FadingOut {
                        left: TWINKLE_FADE_S,
                    }
                }
                Twinkle::Dying { .. } => unreachable!("上面已跳过"),
            };
            i += 1;
        }
        self.advance_dying(dt);
    }

    fn tick_recovering(&mut self, playing: bool, dt: Duration) {
        if !playing {
            // 聚集中再次暂停：从当下位置重新散开（换一批随机落点）。
            self.re_scatter();
            return;
        }
        self.gather_elapsed += dt;
        let k = 1.0 - (-dt.as_secs_f32() / HOMING_TAU).exp();
        let radius = (HOMING_START_RADIUS as f32
            + self.gather_elapsed.as_secs_f32() * HOMING_EXPAND_DPS) as i32;
        let (w, h) = (self.w_cells, self.h_cells);

        let VectorState {
            particles,
            trace_grid,
            gather_elapsed,
            ..
        } = self;

        let mut i = 0;
        while i < particles.len() {
            let p = &mut particles[i];
            if p.ignition > 0.0 {
                // 点火延迟：陆续启程，汇聚流更可读。
                p.ignition = (p.ignition - dt.as_secs_f32()).max(0.0);
                i += 1;
                continue;
            }
            if let Some((tx, ty)) = nearest_lit_dot(
                trace_grid,
                w,
                h,
                p.x.round() as i32,
                p.y.round() as i32,
                radius,
            ) {
                let (dx, dy) = (tx - p.x, ty - p.y);
                let d = dx.hypot(dy);
                if d <= ABSORB_DIST {
                    particles.swap_remove(i);
                    continue;
                }
                p.x += dx * k;
                p.y += dy * k;
            } else if *gather_elapsed >= GATHER_TIMEOUT {
                // 图形不存在（恢复进静音段）：无处可归，超时释放。
                particles.swap_remove(i);
                continue;
            }
            i += 1;
        }

        if particles.is_empty() {
            self.phase = Phase::Active;
        }
    }

    // ---- 分散 / 聚集的进入点 ----

    /// 分散的恢复条件：静音打断要等声音回来；暂停打断恢复播放即回归
    ///（恢复到静音段则锚定不到图形，粒子按超时释放，图形自然不存在）。
    fn resume_signal(&self, playing: bool, level: f32) -> bool {
        playing && (level > SILENCE_FLOOR || !self.dispersed_by_silence)
    }

    /// 从最后一幅可见轨迹孵化粒子：每个点亮点飞向区域内的一个**随机落点**
    ///（坐标哈希确定性采样），恒定大减速度、先后停稳；随机落点保证停稳后
    /// 不保留原图形的剪影。超过 [`MAX_PARTICLES`] 的点按步长抽样直接丢弃。
    fn scatter(&mut self, by_silence: bool) {
        self.dispersed_by_silence = by_silence;
        self.disperse_seed = self.disperse_seed.wrapping_add(1);
        self.spawn_from_last_trace();
        if self.particles.is_empty() {
            return; // 图形本就不可见（渐弱末段），没有可散开的东西
        }
        self.phase = Phase::Dispersing;
        self.silent_for = Duration::ZERO;
        self.float_elapsed = Duration::ZERO;
    }

    fn spawn_from_last_trace(&mut self) {
        let w = self.w_cells;
        let seed = self.disperse_seed;
        let (w_px, h_px) = (self.w_cells as f32 * 2.0, self.h_cells as f32 * 4.0);

        let VectorState {
            last_trace_grid: grid,
            particles,
            ..
        } = self;
        particles.clear();

        // 先收集点亮点，超上限时按步长均匀抽样（空间覆盖均匀、确定性）。
        let mut lit = Vec::new();
        for (cell, &bits) in grid.iter().enumerate() {
            if bits == 0 {
                continue;
            }
            let (cell_x, cell_y) = (cell % w, cell / w);
            for dy in 0..4 {
                for dx in 0..2 {
                    if bits & braille_bit(dx, dy) != 0 {
                        lit.push((cell_x * 2 + dx, cell_y * 4 + dy));
                    }
                }
            }
        }
        // 超上限时按步长抽样保留（空间覆盖均匀、确定性）；其余点也化作
        // 粒子，但以 Dying 相位原地错峰淡出后退役 —— 不允许任何点瞬间消失。
        let step = if lit.len() <= MAX_PARTICLES {
            1
        } else {
            lit.len().div_ceil(MAX_PARTICLES)
        };

        for (idx, &dot) in lit.iter().enumerate() {
            if step == 1 || idx % step == 0 {
                particles.push(new_scatter_particle(dot, seed, w_px, h_px));
            } else {
                let s = dot_seed(dot, seed);
                let total = SURPLUS_FADE_MIN
                    + (SURPLUS_FADE_MAX - SURPLUS_FADE_MIN) * jitter(s.rotate_left(27) ^ 13);
                particles.push(Particle {
                    x: dot.0 as f32,
                    y: dot.1 as f32,
                    vx: 0.0,
                    vy: 0.0,
                    decel: 0.0,
                    twinkle: Twinkle::Dying { left: total, total },
                    ignition: 0.0,
                });
            }
        }
    }

    /// 聚集中再次打断：粒子已在空中，从当下位置重新指派随机落点。
    fn re_scatter(&mut self) {
        let seed = self.disperse_seed.wrapping_add(1);
        self.disperse_seed = seed;
        self.dispersed_by_silence = false;
        let (w_px, h_px) = (self.w_cells as f32 * 2.0, self.h_cells as f32 * 4.0);
        for i in 0..self.particles.len() {
            let p = self.particles[i];
            self.particles[i] = retarget_scatter_particle(p, seed, w_px, h_px);
        }
        self.phase = Phase::Dispersing;
        self.float_elapsed = Duration::ZERO;
    }

    /// 进入聚集：点火延迟按粒子坐标哈希散布在 0~`GATHER_IGNITION_S`。
    fn begin_gather(&mut self) {
        let seed = self.disperse_seed;
        for i in 0..self.particles.len() {
            let p = &mut self.particles[i];
            let s = dot_seed((p.x.max(0.0) as usize, p.y.max(0.0) as usize), seed);
            p.ignition = jitter(s.rotate_left(9) ^ 8) * GATHER_IGNITION_S;
            p.vx = 0.0;
            p.vy = 0.0;
            // 自刷新淡变中的粒子一并回到全可见，全体参与回归。
            p.twinkle = Twinkle::Visible { left: f32::MAX };
        }
        self.phase = Phase::Recovering;
        self.gather_elapsed = Duration::ZERO;
    }
}

/// 孵化一个粒子：再指派随机落点与初速。
fn new_scatter_particle(dot: (usize, usize), seed: u32, w_px: f32, h_px: f32) -> Particle {
    let p = Particle {
        x: dot.0 as f32,
        y: dot.1 as f32,
        vx: 0.0,
        vy: 0.0,
        decel: 0.0,
        twinkle: Twinkle::Visible { left: f32::MAX },
        ignition: 0.0,
    };
    retarget_scatter_particle(p, seed, w_px, h_px)
}

/// 进入呼吸循环前的全亮错峰（秒）：均匀铺满一个完整循环
///（两段淡变），让各点相位均匀分布。
fn stagger_span(s: u32) -> f32 {
    jitter(s.rotate_left(23) ^ 9) * TWINKLE_FADE_S * 2.0
}

/// 区域内均匀随机落点（留 [`SCATTER_MARGIN`] 边距）。
fn random_spot(seed: u32, w_px: f32, h_px: f32) -> (f32, f32) {
    (
        SCATTER_MARGIN + jitter(seed.rotate_left(3) ^ 7) * (w_px - 2.0 * SCATTER_MARGIN),
        SCATTER_MARGIN + jitter(seed.rotate_left(11) ^ 6) * (h_px - 2.0 * SCATTER_MARGIN),
    )
}

/// 为粒子指派一个随机落点并按飞行时长解出初速与减速度：
/// 朝落点直线飞行，恒定减速度恰好在到达时把速度减到零。
fn retarget_scatter_particle(mut p: Particle, seed: u32, w_px: f32, h_px: f32) -> Particle {
    let s = dot_seed((p.x.max(0.0) as usize, p.y.max(0.0) as usize), seed);
    let (tx, ty) = random_spot(s, w_px, h_px);
    let burst_time =
        BURST_MIN_TIME + (BURST_MAX_TIME - BURST_MIN_TIME) * jitter(s.rotate_left(13) ^ 1);
    let (dx, dy) = (tx - p.x, ty - p.y);
    let dist = dx.hypot(dy).max(1.0);
    let v0 = 2.0 * dist / burst_time;
    p.vx = dx / dist * v0;
    p.vy = dy / dist * v0;
    p.decel = v0 / burst_time;
    p
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
    paint_with_alpha(
        f.buffer_mut(),
        area,
        &app.vector.grid,
        &app.vector.cell_alpha,
        &app.theme,
        w_cells,
        h_cells,
    );
}

/// [`paint`] 的带透明度版本：单元格前景 = 波形行渐变色向面板底色按
/// `cell_alpha` 插值，透明度 1 时与示波器同色。一个格子只有一个前景色，
/// 同格多个淡变中的粒子取最大透明度。
fn paint_with_alpha(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    grid: &[u8],
    cell_alpha: &[f32],
    theme: &Theme,
    w_cells: usize,
    h_cells: usize,
) {
    let clip = area.intersection(buf.area);
    let bg = theme.color_base();
    for row in 0..clip.height as usize {
        let t = if h_cells <= 1 {
            1.0
        } else {
            row as f32 / (h_cells - 1) as f32
        };
        let wave_fg = vertical_gradient_color(theme, t);

        for col in 0..clip.width as usize {
            let bits = grid[row * w_cells + col];
            if bits == 0 {
                continue;
            }
            let a = cell_alpha.get(row * w_cells + col).copied().unwrap_or(1.0);
            let fg = if a >= 1.0 {
                wave_fg
            } else {
                mix_colors(wave_fg, bg, a)
            };
            if let Some(cell) = buf.cell_mut((clip.x + col as u16, clip.y + row as u16)) {
                cell.set_char(char::from_u32(0x2800 + bits as u32).unwrap_or(' '));
                cell.set_fg(fg);
            }
        }
    }
}

/// 行渐变波形色（与示波器同一配色逻辑）。
fn vertical_gradient_color(theme: &Theme, t: f32) -> Color {
    mix_colors(theme.color_accent2(), theme.color_accent3(), t)
}

/// 颜色插值：`t = 0` 取 `a`（波形色），`t = 1` 取 `b`（底色）。
/// 非 truecolor 终端退化为返回 `a`，与示波器的降级一致。
fn mix_colors(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            let r = (ar as f32 + (br as f32 - ar as f32) * t) as u8;
            let g = (ag as f32 + (bg as f32 - ag as f32) * t) as u8;
            let bl = (ab as f32 + (bb as f32 - ab as f32) * t) as u8;
            Color::Rgb(r, g, bl)
        }
        _ => a,
    }
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
                // 轨迹恒为全可见；清空后 paint 按“缺失即 1”取全透明度。
                self.cell_alpha.clear();
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

    /// 依锁定缩放把窗口样本画成李萨如轨迹。
    fn rasterize_trace(&mut self) {
        let (w_cells, h_cells) = (self.w_cells, self.h_cells);
        let VectorState {
            snapshot,
            trace_grid,
            last_trace_grid,
            last_drawn,
            scale_peak,
            observed_level,
            ..
        } = self;

        trace_grid.clear();
        trace_grid.resize(w_cells * h_cells, 0);

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

        // 分母 = 本曲最大电平（下限防疯狂比例）。基准单调只增，
        // 响度动态直接反映为图形大小；超出部分由 set_pixel 钳在面板内。
        let scale = half / scale_peak.max(SCALE_FLOOR);
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
            // 横向 x = 左声道 L（向右为正）、纵向 y = 右声道 R（向上为正）。
            let (x, y) = (cx + left[i] * scale, cy - right[i] * scale);
            if let Some((x0, y0)) = prev {
                draw_segment(trace_grid, w_cells, h_cells, (x0, y0), (x, y));
            }
            prev = Some((x, y));
        }

        *last_drawn = true;
        // 画出来了：留存「最后一幅可见轨迹」。打断往往发生在图形已因静音
        // 收缩消失之后（80 ms 持续阈值慢于可见下限），孵化粒子必须用它。
        last_trace_grid.clear();
        last_trace_grid.resize(trace_grid.len(), 0);
        last_trace_grid.copy_from_slice(trace_grid);
    }

    /// 把粒子盖进复合光栅，并记录每格的最大透明度（供颜色插值）。
    /// 淡入淡出中的点半亮（颜色向底色靠）。
    fn stamp_particles(&mut self) {
        let (w, h, phase) = (self.w_cells, self.h_cells, self.phase);
        let VectorState {
            grid,
            cell_alpha,
            particles,
            ..
        } = self;
        cell_alpha.clear();
        cell_alpha.resize(w * h, 0.0);
        let (w_px, h_px) = ((w * 2) as i32, (h * 4) as i32);
        for p in particles {
            let a = if phase == Phase::Floating {
                p.twinkle.alpha()
            } else {
                1.0
            };
            if a <= 0.02 {
                continue;
            }
            let (x, y) = (p.x.round() as i32, p.y.round() as i32);
            set_pixel(grid, w, h, x, y);
            if x >= 0 && y >= 0 && x < w_px && y < h_px {
                let idx = (y as usize / 4) * w + x as usize / 2;
                cell_alpha[idx] = cell_alpha[idx].max(a);
            }
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

/// 窗口内左右声道的原始峰值（缩放基准与静音判定的输入，未平滑）。
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

/// 相邻样本连线：沿较长轴步进保证不断点。
fn draw_segment(grid: &mut [u8], w_cells: usize, h_cells: usize, from: (f32, f32), to: (f32, f32)) {
    let ((x0, y0), (x1, y1)) = (from, to);
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

/// 就近锚定：从 (x0, y0) 由内向外逐环找最近的轨迹点（Chebyshev 环序）。
fn nearest_lit_dot(
    grid: &[u8],
    w_cells: usize,
    h_cells: usize,
    x0: i32,
    y0: i32,
    max_radius: i32,
) -> Option<(f32, f32)> {
    for r in 0..=max_radius {
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

/// 粒子坐标 → 抖动种子：同一粒子在一代分散里种子稳定，换代则变。
fn dot_seed(dot: (usize, usize), seed: u32) -> u32 {
    (dot.0 as u32).wrapping_mul(0x9E37_79B1) ^ (dot.1 as u32).wrapping_mul(0x85EB_CA6B) ^ seed
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

    fn circle(amp: f32, freq_hz: f32, phase: f32) -> impl Fn(usize) -> (f32, f32) {
        move |i| {
            let t = i as f32 / SR as f32;
            let w = std::f32::consts::TAU * freq_hz * t;
            (amp * w.sin(), amp * (w + phase).sin())
        }
    }

    /// 确定性伪随机噪声：相邻样本大幅跳变，轨迹布满面板，用于粒子上限测试。
    fn noise() -> impl Fn(usize) -> (f32, f32) {
        move |i| {
            let s = i as u32;
            (
                jitter(s) * 2.0 - 1.0,
                jitter(s.rotate_left(13) ^ 0x5BD1) * 2.0 - 1.0,
            )
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

    /// 走一帧完整循环（与事件循环的 observe→tick→draw 顺序一致到一帧以内）。
    fn circle_state(amp: f32) -> VectorState {
        let mut st = VectorState {
            snapshot: synth(882, circle(amp, 110.0, std::f32::consts::FRAC_PI_2)),
            ..Default::default()
        };
        st.observe(window_level(&st.snapshot), 40, 20);
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

    /// 高于下限的任何电平都缩放到同一尺寸；低于可见下限则图形消失。
    #[test]
    fn autoscale_fills_panel_and_fades_to_nothing() {
        for amp in [1.0f32, 0.01] {
            let st = circle_state(amp);
            let (min_x, max_x, _, _) = bbox(&lit_dots(&st));
            assert!(
                (max_x - min_x) as f32 >= 80.0 * 0.6,
                "幅度 {amp} 应缩放到接近满幅"
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

    /// 单声道两声道相同：全部点落在右上 45° 对角线上（x−cx = −(y−cy)）。
    #[test]
    fn mono_source_renders_up_right_diagonal() {
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
                "点 ({x},{y}) 偏离右上对角线 {off:.2}（连线步进与取整允差内）"
            );
        }
    }

    /// 缩放基准 = 本曲开播以来的最大峰值：只增不减；更响的段落把基准
    /// 上调、更安静的段落不拉低；环重置→样本重现（切歌）后从零重新累积。
    #[test]
    fn scale_tracks_song_maximum_and_resets_on_track_change() {
        let mut st = VectorState::default();

        st.observe(Some(0.5), 40, 20);
        st.tick(true, true, Duration::from_millis(16));
        assert!((st.scale_peak - 0.5).abs() < 1e-6, "首帧即以当前电平为基准");

        // 更安静的段落不拉低基准（最响段仍恰好撑满面板）。
        st.observe(Some(0.2), 40, 20);
        st.tick(true, true, Duration::from_secs(1));
        assert_eq!(st.scale_peak, 0.5, "基准只增不减");

        // 更响的段落一次性上调基准。
        st.observe(Some(0.9), 40, 20);
        st.tick(true, true, Duration::from_millis(16));
        assert!((st.scale_peak - 0.9).abs() < 1e-6, "更响段落上调基准");

        // 静音不重置（环未重置，非切歌）。
        st.observe(Some(0.0), 40, 20);
        st.tick(true, true, Duration::from_secs(1));
        assert_eq!(st.scale_peak, 0.9);

        // 切歌：环重置（None）→ 样本重现后从零重新累积。
        st.observe(None, 40, 20);
        st.tick(true, true, Duration::from_millis(16));
        st.observe(Some(0.3), 40, 20);
        st.tick(true, true, Duration::from_millis(16));
        assert!((st.scale_peak - 0.3).abs() < 1e-6, "切歌后重新累积");

        st.tick(false, true, Duration::from_secs(1));
        assert_eq!(st.scale_peak, 0.0, "关档复位");
    }

    /// 暂停打断：点亮点化为粒子（有上限，超出按步长抽样直接消失），
    /// 飞向随机落点并在限时内停稳；停稳后自刷新（错峰淡出、随机位置淡入）。
    #[test]
    fn pause_disperses_settles_and_floats() {
        let mut st = circle_state(0.8);
        let dots_before = lit_dots(&st).len();
        assert!(dots_before > 100);

        // 暂停 → 分散。
        st.tick(true, false, FRAME);
        assert_eq!(st.phase, Phase::Dispersing, "暂停立即分散");
        assert!(
            !st.particles.is_empty() && st.particles.len() <= MAX_PARTICLES,
            "粒子数受上限约束：{}",
            st.particles.len()
        );

        let starts: Vec<(f32, f32)> = st.particles.iter().map(|p| (p.x, p.y)).collect();
        for _ in 0..40 {
            st.tick(true, false, FRAME);
        }
        assert_eq!(st.phase, Phase::Floating, "全部停稳进入悬浮");
        for (i, p) in st.particles.iter().enumerate() {
            assert_eq!(p.vx, 0.0, "粒子 {i} 应停稳");
            assert_eq!(p.vy, 0.0);
            // 落点在可视化区域内（随机落点均匀采样，飞不出去）。
            assert!((0.0..=79.0).contains(&p.x), "粒子 {i} 越界 x={}", p.x);
            assert!((0.0..=79.0).contains(&p.y), "粒子 {i} 越界 y={}", p.y);
        }
        // 随机落点：停稳后不应仍聚在原图形附近（散布为随机尘埃）。
        let moved_avg = st
            .particles
            .iter()
            .zip(&starts)
            .map(|(p, s)| (p.x - s.0).hypot(p.y - s.1))
            .sum::<f32>()
            / st.particles.len() as f32;
        assert!(
            moved_avg >= 15.0,
            "平均位移 {moved_avg:.1} 过小：仍能看出原图形"
        );

        // 停稳后：自刷新 —— 各点错峰淡出，并在随机位置重新出现。
        let settled_count = st.particles.len();
        let settled_dots: std::collections::HashSet<(i32, i32)> =
            lit_dots(&st).into_iter().collect();
        let mut fade_bottom_seen = false;
        let mut mid_fade_seen = false;
        for _ in 0..240 {
            st.tick(true, false, Duration::from_millis(50)); // 共 12 s
            st.rasterize();
            fade_bottom_seen |= st.particles.iter().any(|p| p.twinkle.alpha() == 0.0);
            // 3 s 的淡入淡出：50 ms 步进必然采到中间透明度。
            mid_fade_seen |= st
                .particles
                .iter()
                .any(|p| (0.05..0.95).contains(&p.twinkle.alpha()));
            for p in &st.particles {
                // 位置（含换过的随机落点）都在面板内。
                assert!((0.0..=79.0).contains(&p.x), "粒子越界 x={}", p.x);
                assert!((0.0..=79.0).contains(&p.y), "粒子越界 y={}", p.y);
                assert!(
                    (0.0..=1.0).contains(&p.twinkle.alpha()),
                    "透明度越界：{}",
                    p.twinkle.alpha()
                );
            }
        }
        assert_eq!(st.particles.len(), settled_count, "自刷新不增减粒子");
        assert!(fade_bottom_seen, "12 s 内应观测到淡出完成（透明度到 0）");
        assert!(mid_fade_seen, "应观测到 3 s 淡变中的中间透明度");
        let later: std::collections::HashSet<(i32, i32)> = lit_dots(&st).into_iter().collect();
        let relocated = settled_dots.symmetric_difference(&later).count();
        assert!(
            relocated > 0,
            "自刷新应在随机位置重新出现（点集应发生变化）"
        );
    }

    /// 粒子上限：高密度图形孵化时按步长抽样，粒子数不超过上限。
    #[test]
    fn scatter_caps_particle_count() {
        // 伪随机噪声轨迹布满 80×80 点面板（远超粒子上限）。
        let mut st = VectorState {
            snapshot: synth(882, noise()),
            ..Default::default()
        };
        st.observe(window_level(&st.snapshot), 40, 20);
        st.rasterize();
        st.tick(true, true, Duration::from_secs(1));
        st.rasterize();
        let dots = lit_dots(&st).len();
        st.tick(true, false, FRAME);
        assert!(
            dots > MAX_PARTICLES,
            "前置条件：密集图形点数 {dots} 应超过上限"
        );
        // 全部点都化作粒子：没有一个点瞬间消失（多余的以 Dying 淡出）。
        assert_eq!(
            st.particles.len(),
            dots,
            "每个点亮点都应有粒子（多余者走淡出退役）"
        );
        let kept = st
            .particles
            .iter()
            .filter(|p| !matches!(p.twinkle, Twinkle::Dying { .. }))
            .count();
        assert!(
            kept <= MAX_PARTICLES && kept >= MAX_PARTICLES * 9 / 10,
            "抽样保留 {kept} 应接近且不超过上限"
        );
        assert_eq!(
            st.particles.len() - kept,
            dots - kept,
            "多余点应处于 Dying 相位"
        );
        // 淡出全部完成后，参与轮换的粒子数收敛到上限以内。
        for _ in 0..80 {
            st.tick(true, false, FRAME);
        }
        assert!(
            st.particles.len() <= MAX_PARTICLES,
            "退役后粒子数 {} 超过上限 {}",
            st.particles.len(),
            MAX_PARTICLES
        );
        assert!(
            st.particles
                .iter()
                .all(|p| !matches!(p.twinkle, Twinkle::Dying { .. })),
            "不应再有 Dying 粒子"
        );
    }

    /// 帧间隔尖峰（单线程运行时被宿主桥/下载阻塞数秒）不得跳过淡出：
    /// 动画时钟钳步后，dt=10 s 的 tick 也只推进 100 ms，相位保持 FadingOut。
    #[test]
    fn dt_spike_does_not_skip_fadeout() {
        let mut st = VectorState {
            phase: Phase::Floating,
            w_cells: 40,
            h_cells: 20,
            ..Default::default()
        };
        st.particles = vec![Particle {
            x: 20.0,
            y: 40.0,
            twinkle: Twinkle::FadingOut {
                left: TWINKLE_FADE_S,
            },
            ..new_scatter_particle((10, 10), 1, 80.0, 80.0)
        }];
        st.observe(Some(1.0e-6), 40, 20);

        // 一个 10 s 的尖峰 tick：淡出只推进 100 ms，而不是整段跳过。
        st.tick(true, false, Duration::from_secs(10));
        assert!(
            matches!(st.particles[0].twinkle, Twinkle::Dying { .. }) == false
                && matches!(st.particles[0].twinkle, Twinkle::FadingOut { .. }),
            "尖峰 dt 不应跳过淡出相位"
        );
        let a = st.particles[0].twinkle.alpha();
        assert!(
            (0.9..=1.0).contains(&a),
            "尖峰后透明度应只下降约 100 ms 的量：{a}"
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
        st.tick(true, true, Duration::from_millis(60));
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

    /// 恢复播放：粒子**聚集回归** —— 带点火延迟与可见的指数逼近，
    /// 0.8 s 内全部归位吸收，图形恢复。
    #[test]
    fn resume_gathers_all_particles_visibly() {
        let mut st = circle_state(0.8);
        st.tick(true, false, FRAME); // 暂停分散
        for _ in 0..40 {
            st.tick(true, false, FRAME);
        }
        assert_eq!(st.phase, Phase::Floating);
        let initial = st.particles.len();
        assert!(initial > 100);

        // 恢复播放且有声：进入聚集（轨迹与粒子同画）。
        st.observe(Some(0.8), 40, 20);
        st.rasterize();
        st.tick(true, true, FRAME);
        assert_eq!(st.phase, Phase::Recovering, "聚集是可见动画而非瞬切");

        // 点火期内（≤0.12 s）粒子原地待命，随后陆续被吸收：
        // 中途应观测到「部分归位、部分仍在途」的中间态。
        let mut seen_partial = false;
        for _ in 0..25 {
            st.rasterize();
            st.tick(true, true, FRAME);
            let left = st.particles.len();
            if left > 0 && left < initial {
                seen_partial = true;
            }
        }
        assert!(seen_partial, "聚集应有可见的渐进过程（观测到中间态）");
        assert!(st.particles.is_empty(), "粒子应全部归位吸收");
        assert_eq!(st.phase, Phase::Active);
        st.rasterize();
        assert!(!lit_dots(&st).is_empty(), "聚集完成后图形恢复");
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

    /// 淡入淡出的颜色（透明度）渐变：半透明粒子的前景色恰为波形色与
    /// 面板底色的中点插值，全可见粒子与示波器同色。
    #[test]
    fn fading_particles_blend_colors_toward_background() {
        use crate::tmplayer::ui::theme::{ColorCapability, Theme, ThemeName, ThemePalette};
        use ratatui::buffer::Buffer;

        let theme = Theme {
            name: ThemeName::System,
            capability: ColorCapability::TrueColor,
            palette: ThemePalette {
                text: (200, 200, 200),
                subtext: (170, 170, 170),
                base: (36, 36, 48),
                surface: (48, 48, 64),
                buff: (60, 60, 80),
                accent: (140, 170, 220),
                accent2: (160, 200, 240),
                accent3: (130, 220, 200),
            },
        };

        let mut st = VectorState {
            phase: Phase::Floating,
            w_cells: 40,
            h_cells: 20,
            ..Default::default()
        };
        // 两颗隔离的粒子：一颗半透明（淡出到一半），一颗全可见。
        st.particles = vec![
            Particle {
                x: 20.0,
                y: 40.0,
                twinkle: Twinkle::FadingOut {
                    left: TWINKLE_FADE_S * 0.5,
                },
                ..new_scatter_particle((10, 10), 1, 80.0, 80.0)
            },
            Particle {
                x: 30.0,
                y: 40.0,
                twinkle: Twinkle::Visible { left: 9.0 },
                ..new_scatter_particle((11, 10), 1, 80.0, 80.0)
            },
        ];
        st.rasterize();

        let area = Rect::new(0, 0, 40, 20);
        let mut buf = Buffer::empty(area);
        paint_with_alpha(&mut buf, area, &st.grid, &st.cell_alpha, &theme, 40, 20);

        // 半透明粒子：y=40 子像素 → 第 10 行；期望色 = 行渐变色与底色按 0.5 插值。
        let row = 10usize;
        let t = row as f32 / 19.0;
        let wave = vertical_gradient_color(&theme, t);
        let expect_half = mix_colors(wave, theme.color_base(), 0.5);
        assert_eq!(buf.cell((10, 10)).unwrap().fg, expect_half);

        // 全可见粒子：与示波器同色（纯行渐变）。
        assert_eq!(buf.cell((15, 10)).unwrap().fg, wave);
    }
}
