//! A shared render cap. Dirtiness and maintenance/data deadlines are independent.
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct FrameClock {
    interval: Duration,
    deadline: Instant,
    dirty: bool,
}

impl FrameClock {
    pub fn new(fps: u32, now: Instant) -> Self {
        Self {
            interval: interval(fps),
            deadline: now,
            dirty: true,
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

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn due(&self, now: Instant) -> bool {
        self.dirty && now >= self.deadline
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.dirty.then_some(self.deadline)
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
    Duration::from_secs_f64(1.0 / f64::from(fps.clamp(10, 60)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_coalesce_without_exceeding_the_cap_or_replaying_missed_frames() {
        let start = Instant::now();
        let mut clock = FrameClock::new(30, start);
        assert!(clock.due(start));
        clock.presented(start);
        for offset in [1, 5, 20, 30] {
            clock.mark_dirty();
            assert!(!clock.due(start + Duration::from_millis(offset)));
        }
        let late = start + Duration::from_secs(2);
        assert!(clock.due(late));
        clock.presented(late);
        assert!(!clock.is_dirty());
        clock.mark_dirty();
        assert!(!clock.due(late));
        assert!(!clock.due(late + Duration::from_millis(30)));
        assert!(clock.due(late + Duration::from_millis(34)));
    }

    #[test]
    fn fps_change_keeps_pending_input_and_prevents_immediate_extra_frame() {
        let start = Instant::now();
        let mut clock = FrameClock::new(30, start);
        clock.presented(start);
        clock.mark_dirty();
        clock.set_fps(60, start + Duration::from_millis(5));
        assert!(clock.is_dirty());
        assert!(!clock.due(start + Duration::from_millis(6)));
        assert!(clock.due(start + Duration::from_millis(34)));
    }
}
