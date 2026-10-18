use super::App;
use super::Overlay;
use super::Page;
#[cfg(feature = "easter-egg")]
use super::about::EasterEggPhase;
use super::playback_controller::PlaybackRuntimeState;
use super::search_controller::SEARCH_BOX_ANIM_DURATION;
use crate::data::config::VisualizeMode;
use crate::render::motion::Curve;
use crate::render::motion::Toggle;
use crate::render::motion::Trail;
use crate::render::motion::Transition;
use anyhow::Result;
use std::time::Duration;
use std::time::Instant;

/// 主程序内容页小窗口模式的统一触发阈值，与主页既有判定一致。
pub(crate) const SMALL_WINDOW_MIN_WIDTH: u16 = 32;

pub(crate) const SMALL_WINDOW_MIN_HEIGHT: u16 = 12;

/// 扁窗视口高度下限，即折叠播放栏区域高度。
const FLAT_SMALL_HEIGHT: u16 = 5;

/// 扁窗播放栏/歌词栏切换动画时长。
const FLAT_SWITCH_ANIM_DURATION: Duration = Duration::from_millis(220);

/// 窄窗 LUFS 表显示范围：-60..0 LUFS。
pub(super) const VU_LUFS_FLOOR: f32 = -120.0;

const VU_LUFS_MIN: f32 = -60.0;

const VU_LUFS_MAX: f32 = 0.0;

const VU_ATTACK_SECS: f32 = 0.08;

const VU_RELEASE_SECS: f32 = 0.35;

const VU_SETTLED_EPSILON: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SmallWindowMode {
    Flat,
    Narrow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlatPanel {
    Player,
    Lyrics,
}

pub(crate) fn mean_square_to_lufs(mean_square: f32) -> f32 {
    if mean_square <= 1.0e-12 {
        VU_LUFS_FLOOR
    } else {
        (-0.691 + 10.0 * mean_square.log10()).max(VU_LUFS_FLOOR)
    }
}

pub(crate) fn lufs_to_mean_square(lufs: f32) -> f32 {
    if lufs <= VU_LUFS_FLOOR + 0.5 {
        0.0
    } else {
        10.0_f32.powf((lufs + 0.691) / 10.0)
    }
}

pub(crate) fn lufs_to_bar_level(lufs: f32) -> f32 {
    ((lufs - VU_LUFS_MIN) / (VU_LUFS_MAX - VU_LUFS_MIN)).clamp(0.0, 1.0)
}

fn smooth_lufs_level(current: f32, target: f32, dt: f32) -> (f32, bool) {
    let target = target.max(VU_LUFS_FLOOR);
    let mut current = current.max(VU_LUFS_FLOOR);
    if (current - target).abs() <= VU_SETTLED_EPSILON {
        return (target, false);
    }

    let tau = if target > current {
        VU_ATTACK_SECS
    } else {
        VU_RELEASE_SECS
    };
    let amount = if dt <= 0.0 {
        0.0
    } else {
        1.0 - (-dt / tau).exp()
    };
    current += (target - current) * amount;
    if (current - target).abs() <= VU_SETTLED_EPSILON {
        current = target;
    }
    (current, (current - target).abs() > VU_SETTLED_EPSILON)
}

pub(super) fn cubic_bezier_y(t: f32, p1y: f32, p2y: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    let a = 3.0 * inv * inv * t * p1y;
    let b = 3.0 * inv * t * t * p2y;
    let c = t * t * t;
    (a + b + c).clamp(0.0, 1.0)
}

fn braille_from_two_bars(left: u8, right: u8) -> char {
    const LEFT_BITS: [u8; 4] = [6, 2, 1, 0];
    const RIGHT_BITS: [u8; 4] = [7, 5, 4, 3];

    let mut dots = 0u8;
    for idx in 0..left.min(4) {
        dots |= 1 << LEFT_BITS[idx as usize];
    }
    for idx in 0..right.min(4) {
        dots |= 1 << RIGHT_BITS[idx as usize];
    }

    if dots == 0 {
        ' '
    } else {
        char::from_u32(0x2800 + dots as u32).unwrap_or(' ')
    }
}

impl App {
    /// 是否有进行中的动画需要高频重绘（进度条脉冲、搜索框滑出、侧边栏
    /// 滑出、启动加载）。主事件循环据此在动画期间从 1s 空闲节流切换
    /// 到 ~30fps 重绘。
    pub(crate) fn should_continuous_redraw(&self) -> bool {
        if self.page_transition_active() {
            return true;
        }
        let playing = self.playback.now_playing.is_some();
        if self.mini_spectrum_enabled() && playing {
            return true;
        }
        if self.is_seeking() || self.page_lyrics_grab.is_some() {
            return true;
        }
        if self.input.search_motion.is_running()
            || self.browse.home_sidebar.motion.is_running()
            || self.browse.home_sidebar.trail.value() > 0.0
        {
            return true;
        }
        if self.downloads.manager.is_active()
            || self.page == Page::Loading
            || self
                .flat_switch_anim
                .as_ref()
                .is_some_and(Transition::is_running)
        {
            return true;
        }
        #[cfg(feature = "easter-egg")]
        if self.about_egg.phase != EasterEggPhase::Idle {
            return true;
        }
        false
    }

    /// 是否处于“小窗口相关”上下文：设置开启、已登录内容页、且终端低于统一阈值。
    /// 该上下文包含扁窗、窄窗，以及两个方向都过小而只显示提示的情况。
    pub(crate) fn is_small_window_context(&self) -> bool {
        if !self.config.small_window_display {
            return false;
        }
        if matches!(self.page, Page::Login | Page::Loading) {
            return false;
        }
        self.term_width < SMALL_WINDOW_MIN_WIDTH || self.term_height < SMALL_WINDOW_MIN_HEIGHT
    }

    pub(crate) fn set_terminal_size(&mut self, width: u16, height: u16) {
        // 上一帧未再登记进度条（或被弹窗遮挡）时，不能把抓取带回后续帧。
        self.cancel_invalid_player_bar_progress_drag();
        if (self.term_width, self.term_height) != (width, height) {
            // Geometry changed: settle on the logical destination, never stretch
            // or leave a stale snapshot over the new viewport.
            self.cancel_page_transition();
            self.input.player_bar_progress_drag = None;
            self.clear_player_bar_hits();
        }
        let was_small_context = self.is_small_window_context();
        self.term_width = width;
        self.term_height = height;
        if !was_small_context && self.is_small_window_context() {
            // 普通尺寸进入小窗口上下文（含“双轴过小”）：立即关闭弹窗与侧边栏。
            self.close_panels_for_small_window();
        }
        self.recompute_small_window_mode();
    }

    pub(super) fn sync_terminal_size(&mut self) {
        if let Ok((width, height)) = crossterm::terminal::size() {
            self.set_terminal_size(width, height);
        }
    }

    fn compute_small_window_mode(&self) -> Option<SmallWindowMode> {
        if !self.is_small_window_context() {
            return None;
        }

        let width = self.term_width;
        let height = self.term_height;
        if height >= SMALL_WINDOW_MIN_HEIGHT && width < SMALL_WINDOW_MIN_WIDTH {
            Some(SmallWindowMode::Narrow)
        } else if width >= SMALL_WINDOW_MIN_WIDTH
            && (FLAT_SMALL_HEIGHT..SMALL_WINDOW_MIN_HEIGHT).contains(&height)
        {
            Some(SmallWindowMode::Flat)
        } else {
            // 高度不足扁窗下限，或宽高同时过小：仍显示“终端窗口过小”。
            None
        }
    }

    fn recompute_small_window_mode(&mut self) {
        let mode = self.compute_small_window_mode();
        if mode == self.small_window_mode {
            if mode == Some(SmallWindowMode::Flat) {
                if self.flat_switch_anim.is_some() {
                    let now = Instant::now();
                    let width = self.term_width.max(1) as f32;
                    let current = self.flat_switch_offset().min(width);
                    let target = match self.flat_panel {
                        FlatPanel::Player => 0.0,
                        FlatPanel::Lyrics => width,
                    };
                    let mut motion = Transition::new(current);
                    motion.retarget(target, now, FLAT_SWITCH_ANIM_DURATION, Curve::EaseOut);
                    self.flat_switch_anim = Some(motion);
                }
                let exact = self.term_height == FLAT_SMALL_HEIGHT;
                if exact && !self.flat_exact_height {
                    // 从“上方有歌词”的扁窗缩到只剩播放栏高度时，重新以播放栏为默认面板。
                    self.flat_panel = FlatPanel::Player;
                    self.flat_switch_anim = None;
                }
                self.flat_exact_height = exact;
            }
            return;
        }

        self.small_window_mode = mode;
        match mode {
            Some(SmallWindowMode::Flat) => {
                // 每次进入扁窗都从缩略播放栏开始。
                self.flat_panel = FlatPanel::Player;
                self.flat_switch_anim = None;
                self.flat_exact_height = self.term_height == FLAT_SMALL_HEIGHT;
                self.close_panels_for_small_window();
            }
            Some(SmallWindowMode::Narrow) => {
                self.flat_exact_height = false;
                self.flat_switch_anim = None;
                self.vu_left_lufs = VU_LUFS_FLOOR;
                self.vu_right_lufs = VU_LUFS_FLOOR;
                self.vu_animating = false;
                self.vu_last_tick_at = None;
                self.close_panels_for_small_window();
            }
            None => {
                // 离开小窗口回到普通页面：清掉小窗口阶段可能残留的命中区域。
                self.flat_exact_height = false;
                self.flat_switch_anim = None;
                self.clear_content_hits();
                self.clear_player_bar_hits();
            }
        }

        // 宽高同时过小、只显示“终端窗口过小”时没有独立模式，
        // 但同样属于小窗口上下文：关闭已打开的弹窗与侧边栏。
        if mode.is_none() && self.is_small_window_context() {
            self.close_panels_for_small_window();
        }
    }

    /// 进入小窗口时关闭设置/搜索等弹窗与侧边栏，并清理隐藏页面的命中区域。
    pub(super) fn close_panels_for_small_window(&mut self) {
        if self.overlay.is_some() {
            self.close_overlay();
        }
        self.browse.home_sidebar.expanded = false;
        self.browse.home_sidebar.motion = Toggle::new(false);
        self.browse.home_sidebar.trail = Trail::new(0.0);
        self.clear_content_hits();
        self.clear_player_bar_hits();
    }

    /// 扁窗 5 行视口下两个面板的水平偏移（0=播放栏，width=歌词栏）。
    pub(crate) fn flat_switch_offset(&self) -> f32 {
        let width = self.term_width.max(1) as f32;
        self.flat_switch_anim
            .as_ref()
            .map(|anim| {
                let value = anim.sample(Instant::now());
                if anim.is_running() {
                    value
                } else {
                    anim.target()
                }
            })
            .unwrap_or(match self.flat_panel {
                FlatPanel::Player => 0.0,
                FlatPanel::Lyrics => width,
            })
    }

    pub(crate) fn flat_switch_animating(&self) -> bool {
        self.flat_switch_anim
            .as_ref()
            .is_some_and(Transition::is_running)
    }

    pub(super) fn toggle_flat_panel(&mut self) {
        if self.small_window_mode != Some(SmallWindowMode::Flat)
            || self.term_height != FLAT_SMALL_HEIGHT
        {
            return;
        }
        self.input.player_bar_progress_drag = None;
        self.clear_player_bar_hits();
        let now = Instant::now();
        let width = self.term_width.max(1) as f32;
        let current = self.flat_switch_offset().clamp(0.0, width);
        let visually_lyrics = current >= width * 0.5;
        self.flat_panel = if visually_lyrics {
            FlatPanel::Player
        } else {
            FlatPanel::Lyrics
        };
        let target = if self.flat_panel == FlatPanel::Player {
            0.0
        } else {
            width
        };
        let mut anim = Transition::new(current);
        anim.retarget(target, now, FLAT_SWITCH_ANIM_DURATION, Curve::EaseOut);
        self.flat_switch_anim = Some(anim);
    }

    pub(super) fn tick_flat_switch(&mut self, now: Instant) {
        if let Some(anim) = &mut self.flat_switch_anim {
            anim.tick(now);
            if !anim.is_running() {
                self.flat_switch_anim = None;
            }
        }
    }

    pub(super) fn tick_search_box_animation(&mut self, now: Instant) {
        let open = matches!(self.overlay, Some(Overlay::SearchBox));
        self.input
            .search_motion
            .set(open, now, SEARCH_BOX_ANIM_DURATION, Curve::EaseOut);
        self.input.search_motion.tick(now);
    }

    pub(super) fn tick_vu_meter(&mut self, now: Instant) {
        if self.small_window_mode != Some(SmallWindowMode::Narrow) {
            self.vu_animating = false;
            self.vu_last_tick_at = None;
            return;
        }

        let reading = self.playback.audio_player.lufs_meter().latest();
        let dt = self
            .vu_last_tick_at
            .map(|at| now.saturating_duration_since(at).as_secs_f32().min(0.25))
            .unwrap_or(0.0);
        self.vu_last_tick_at = Some(now);

        let stale = self.vu_last_meter_generation == reading.generation
            && self.playback.playback_state != PlaybackRuntimeState::Playing;
        self.vu_last_meter_generation = reading.generation;

        let target_left = if stale || self.playback.now_playing.is_none() {
            VU_LUFS_FLOOR
        } else {
            mean_square_to_lufs(reading.left_mean_square)
        };
        let target_right = if stale || self.playback.now_playing.is_none() {
            VU_LUFS_FLOOR
        } else {
            mean_square_to_lufs(reading.right_mean_square)
        };

        let (left, left_moving) = smooth_lufs_level(self.vu_left_lufs, target_left, dt);
        let (right, right_moving) = smooth_lufs_level(self.vu_right_lufs, target_right, dt);
        self.vu_left_lufs = left;
        self.vu_right_lufs = right;
        self.vu_animating = left_moving || right_moving;
    }

    pub(crate) fn main_spectrum_braille(&self) -> String {
        let bars = self.mini_spectrum.mini_bars();
        let mut out = String::with_capacity(30);
        for pair in bars.as_chunks::<2>().0 {
            let left_h = (pair[0].clamp(0.0, 1.0) * 4.0).round() as u8;
            let right_h = (pair[1].clamp(0.0, 1.0) * 4.0).round() as u8;
            out.push(braille_from_two_bars(left_h.min(4), right_h.min(4)));
        }
        out
    }

    pub(crate) fn sync_on_change(&mut self) {
        self.sync_terminal_size();
    }

    fn mini_spectrum_enabled(&self) -> bool {
        !matches!(
            self.config.visualize,
            VisualizeMode::Lyrics | VisualizeMode::Hidden
        )
    }

    /// Called only immediately before a Host frame submission, never during audio callbacks.
    pub(crate) fn update_main_spectrum(&mut self, now: Instant) -> Result<()> {
        if !self.mini_spectrum_enabled() || self.playback.now_playing.is_none() {
            self.mini_spectrum.clear();
            return Ok(());
        }
        self.mini_spectrum.update(
            &self.playback.audio_player.pcm_ring(),
            self.playback.playback_state == PlaybackRuntimeState::Playing,
            now,
        )?;
        Ok(())
    }

    /// The Host is not rendered while Fullscreen owns the terminal.
    pub(crate) fn reset_main_spectrum(&mut self) {
        self.mini_spectrum.clear();
    }
}
