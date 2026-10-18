//! A shared render cap. Dirtiness and maintenance/data deadlines are independent.
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct FrameClock {
    interval: Duration,
    deadline: Instant,
    dirty: bool,
    continuous: bool,
}

impl FrameClock {
    pub fn new(fps: u32, now: Instant) -> Self {
        Self {
            interval: interval(fps),
            deadline: now,
            dirty: true,
            continuous: false,
        }
    }

    pub fn set_fps(&mut self, fps: u32, now: Instant) {
        let next = interval(fps);
        if next != self.interval {
            self.interval = next;
            self.deadline = self.deadline.max(now + next);
        }
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn set_continuous(&mut self, continuous: bool) {
        self.continuous = continuous;
    }

    pub fn due(&self, now: Instant) -> bool {
        self.dirty || (self.continuous && now >= self.deadline)
    }

    pub fn next_deadline(&self) -> Duration {
        let instant = if self.dirty {
            Instant::now()
        } else if self.continuous {
            self.deadline
        } else {
            Instant::now() + Duration::from_secs(1)
        };
        instant.saturating_duration_since(Instant::now())
    }

    /// Skip expired slots instead of submitting catch-up frames. Input wakeups
    /// cannot bypass the cap; a late submission starts a new minimum interval.
    pub fn presented(&mut self, now: Instant) {
        self.dirty = false;
        let elapsed = now.saturating_duration_since(self.deadline);
        let periods = elapsed.as_nanos() / self.interval.as_nanos() + 1;
        let advance = self.interval.mul_f64(periods as f64);
        self.deadline = (self.deadline + advance).max(now + self.interval);
    }
}

fn interval(fps: u32) -> Duration {
    assert!(fps != 0, "ui_fps must be greater than zero");
    Duration::from_secs_f64(1.0 / f64::from(fps)).max(Duration::from_nanos(1))
}

#[cfg(test)]
mod tests;
