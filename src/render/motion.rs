//! Monotonic-time UI motion, independent of event frequency and rendering FPS.
use std::time::{Duration, Instant};

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

    pub fn is_running(&self) -> bool {
        self.motion.is_running()
    }

    pub fn value(&self) -> f32 {
        self.motion.value()
    }
}

#[cfg(test)]
mod tests;
