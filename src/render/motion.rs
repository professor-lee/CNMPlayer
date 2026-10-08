//! Monotonic-time UI motion, independent of event frequency and rendering FPS.
use ratatui::layout::Rect;
use std::time::{Duration, Instant};

pub(crate) const MODAL_ANIM_DURATION: Duration = Duration::from_millis(180);

/// Reveal a modal around its vertical center without changing its final width.
pub(crate) fn reveal_centered_rect(full: Rect, progress: f32) -> Rect {
    let progress = progress.clamp(0.0, 1.0);
    let height = ((f32::from(full.height) * progress).round() as u16).min(full.height);
    if height == 0 {
        return Rect::default();
    }
    Rect {
        y: full.y + (full.height - height) / 2,
        height,
        ..full
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Curve {
    Linear,
    EaseOut,
    EaseInOut,
}

impl Curve {
    fn apply(self, t: f32) -> f32 {
        match self {
            Self::Linear => t,
            Self::EaseOut => 1.0 - (1.0 - t).powi(3),
            Self::EaseInOut => t * t * (3.0 - 2.0 * t),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Transition {
    value: f32,
    from: f32,
    target: f32,
    started: Option<Instant>,
    duration: Duration,
    curve: Curve,
    changed: bool,
}

impl Transition {
    pub fn new(value: f32) -> Self {
        Self {
            value,
            from: value,
            target: value,
            started: None,
            duration: Duration::ZERO,
            curve: Curve::Linear,
            changed: false,
        }
    }

    pub fn retarget(&mut self, target: f32, now: Instant, duration: Duration, curve: Curve) {
        if self.target == target {
            return;
        }
        let current = self.sample(now);
        self.changed |= current != self.value;
        self.value = current;
        self.from = current;
        self.target = target;
        self.duration = duration;
        self.curve = curve;
        self.started = Some(now);
        if duration.is_zero() || current == target {
            self.changed |= self.value != target;
            self.value = target;
            self.started = None;
        }
    }

    pub fn sample(&self, now: Instant) -> f32 {
        let Some(started) = self.started else {
            return self.value;
        };
        let elapsed = now.saturating_duration_since(started);
        if elapsed >= self.duration {
            return self.target;
        }
        let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        self.from + (self.target - self.from) * self.curve.apply(t)
    }

    /// Returns the last endpoint change as well as intermediate movement.
    pub fn tick(&mut self, now: Instant) -> bool {
        let next = self.sample(now);
        let changed = std::mem::take(&mut self.changed) || self.value != next;
        self.value = next;
        if self
            .started
            .is_some_and(|started| now.saturating_duration_since(started) >= self.duration)
        {
            self.started = None;
        }
        changed
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn is_running(&self) -> bool {
        self.started.is_some()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Toggle(Transition);

impl Toggle {
    pub fn new(on: bool) -> Self {
        Self(Transition::new(if on { 1.0 } else { 0.0 }))
    }

    pub fn set(&mut self, on: bool, now: Instant, duration: Duration, curve: Curve) {
        self.0
            .retarget(if on { 1.0 } else { 0.0 }, now, duration, curve);
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        self.0.tick(now)
    }

    pub fn value(&self) -> f32 {
        self.0.value()
    }

    pub fn on(&self) -> bool {
        self.0.target() == 1.0
    }

    pub fn is_running(&self) -> bool {
        self.0.is_running()
    }
}

/// A delayed follower. Unchanged targets do not restart the delay or tween.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Trail {
    motion: Transition,
    waiting: Option<(f32, Instant, Duration, Curve)>,
    target: f32,
}

impl Trail {
    pub fn new(value: f32) -> Self {
        Self {
            motion: Transition::new(value),
            waiting: None,
            target: value,
        }
    }

    pub fn follow(
        &mut self,
        target: f32,
        now: Instant,
        delay: Duration,
        duration: Duration,
        curve: Curve,
    ) {
        if self.target == target {
            return;
        }
        self.tick(now);
        self.target = target;
        self.motion = Transition::new(self.motion.value());
        if delay.is_zero() {
            self.waiting = None;
            self.motion.retarget(target, now, duration, curve);
        } else {
            self.waiting = Some((target, now + delay, duration, curve));
        }
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        if let Some((target, starts, duration, curve)) = self.waiting
            && now >= starts
        {
            self.waiting = None;
            self.motion.retarget(target, starts, duration, curve);
        }
        self.motion.tick(now)
    }

    pub fn value(&self) -> f32 {
        self.motion.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversal_keeps_current_position_and_finishes_at_a_real_time_endpoint() {
        let now = Instant::now();
        let mut motion = Transition::new(0.0);
        motion.retarget(10.0, now, Duration::from_millis(200), Curve::EaseInOut);
        motion.tick(now + Duration::from_millis(100));
        assert_eq!(motion.value(), 5.0);
        motion.retarget(
            0.0,
            now + Duration::from_millis(100),
            Duration::from_millis(200),
            Curve::EaseOut,
        );
        assert_eq!(motion.value(), 5.0);
        assert!(motion.tick(now + Duration::from_millis(400)));
        assert_eq!(motion.value(), 0.0);
        assert!(!motion.is_running());
        assert!(!motion.tick(now + Duration::from_secs(2)));
    }

    #[test]
    fn toggle_logic_and_rendering_are_independent_and_zero_duration_is_dirty() {
        let now = Instant::now();
        let mut toggle = Toggle::new(false);
        toggle.set(true, now, Duration::from_millis(200), Curve::Linear);
        assert!(toggle.on());
        assert_eq!(toggle.value(), 0.0);
        toggle.tick(now + Duration::from_millis(60));
        assert!((toggle.value() - 0.3).abs() < 0.00001);
        toggle.set(
            false,
            now + Duration::from_millis(60),
            Duration::ZERO,
            Curve::Linear,
        );
        assert!(toggle.tick(now + Duration::from_millis(60)));
        assert_eq!(toggle.value(), 0.0);
        assert!(!toggle.on());
    }

    #[test]
    fn repeated_trail_target_does_not_postpone_the_delayed_reveal() {
        let now = Instant::now();
        let mut trail = Trail::new(0.0);
        for offset in [0, 20, 80] {
            trail.follow(
                1.0,
                now + Duration::from_millis(offset),
                Duration::from_millis(100),
                Duration::from_millis(200),
                Curve::Linear,
            );
        }
        assert!(!trail.tick(now + Duration::from_millis(99)));
        assert_eq!(trail.value(), 0.0);
        assert!(trail.tick(now + Duration::from_millis(200)));
        assert_eq!(trail.value(), 0.5);
        assert!(trail.tick(now + Duration::from_millis(400)));
        assert_eq!(trail.value(), 1.0);
        assert!(!trail.motion.is_running() && trail.waiting.is_none());
    }
}
