use super::*;

pub(crate) const STARTUP_LOADING_MIN_VISIBLE_SECS: f32 = 0.75;

pub(crate) const STARTUP_LOADING_FILL_SECS: f32 = 0.62;

pub(crate) const STARTUP_LOADING_COMPLETE_RAMP_SECS: f32 = 0.26;

/// 加载页进度：已完成步数 + 当前步的时间缓动。
///
/// 只有后台初始化真正走完的步骤才会推进进度条，单步内的缓动只是为了在
/// 跳步之间保持呼吸感；收尾时再由 ramp 补到 1.0（超时跳过剩余步骤也走这条路）。
pub(crate) fn startup_loading_progress(
    step_done: usize,
    step_total: usize,
    step_elapsed: f32,
    complete_elapsed: Option<f32>,
    complete_requested: bool,
) -> f32 {
    // 单步内的非线性缓动（沿用原来的 cubic-bezier 曲线手感）。
    let t = (step_elapsed / STARTUP_LOADING_FILL_SECS).clamp(0.0, 1.0);
    let eased = cubic_bezier_y(t, 0.08, 0.98);
    // 单步最多推进到 1/total 的份额，留出到下一步的余量。
    let within = (eased / step_total.max(1) as f32).min(0.96);
    let base = ((step_done as f32 / step_total.max(1) as f32) + within).min(0.96);

    if !complete_requested {
        return base;
    }

    let complete_t =
        (complete_elapsed.unwrap_or(0.0) / STARTUP_LOADING_COMPLETE_RAMP_SECS).clamp(0.0, 1.0);
    let complete_eased = cubic_bezier_y(complete_t, 0.25, 1.0);
    (base + (1.0 - base) * complete_eased).clamp(0.0, 1.0)
}

impl App {
    pub(crate) fn begin_startup_loading(&mut self, target: Page) {
        self.page = Page::Loading;
        self.close_overlay();
        self.startup.begin(target);
    }

    pub(crate) fn finish_startup_loading(&mut self) {
        self.startup.finish();
    }

    pub(crate) fn tick_startup_loading(&mut self) {
        if self.page == Page::Loading
            && let Some(target) = self.startup.tick()
        {
            self.page = target;
        }
    }

    pub(crate) fn startup_loading_progress(&self) -> f32 {
        self.startup.current_progress()
    }

    pub fn startup_loading_progress_for_width(&self, _bar_width: u16) -> f32 {
        if self.page != Page::Loading {
            return 0.0;
        }

        if self.startup.started_at.is_none() {
            return 0.0;
        }

        self.startup_loading_progress()
    }
}

use super::{Page, StartupInit};
use std::time::Instant;

/// Startup task and loading-screen progress state.
pub(crate) struct StartupController {
    pub init: StartupInit,
    pub progress: f32,
    pub target: Page,
    pub started_at: Option<Instant>,
    pub complete_started_at: Option<Instant>,
    pub complete_requested: bool,
}

impl StartupController {
    pub fn detached() -> Self {
        Self {
            init: StartupInit::detached(),
            progress: 0.0,
            target: Page::Login,
            started_at: None,
            complete_started_at: None,
            complete_requested: false,
        }
    }

    pub fn begin(&mut self, target: Page) {
        self.progress = 0.0;
        self.started_at = Some(Instant::now());
        self.complete_started_at = None;
        self.complete_requested = false;
        self.target = target;
    }

    pub fn finish(&mut self) {
        self.complete_requested = true;
        self.complete_started_at.get_or_insert_with(Instant::now);
    }

    pub fn current_progress(&self) -> f32 {
        super::startup_loading_progress(
            self.init.step_done(),
            self.init.step_total(),
            self.init.step_elapsed(),
            self.complete_started_at
                .map(|time| time.elapsed().as_secs_f32()),
            self.complete_requested,
        )
    }

    pub fn tick(&mut self) -> Option<Page> {
        let Some(started_at) = self.started_at else {
            self.started_at = Some(Instant::now());
            return None;
        };
        self.progress = self.current_progress();
        let ramp_done = self.complete_started_at.is_some_and(|time| {
            time.elapsed().as_secs_f32() >= super::STARTUP_LOADING_COMPLETE_RAMP_SECS
        });
        if self.complete_requested
            && ramp_done
            && started_at.elapsed().as_secs_f32() >= super::STARTUP_LOADING_MIN_VISIBLE_SECS
        {
            self.progress = 0.0;
            self.started_at = None;
            self.complete_started_at = None;
            self.complete_requested = false;
            Some(self.target)
        } else {
            None
        }
    }
}
