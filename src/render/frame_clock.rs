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

    pub fn is_dirty(&self) -> bool {
        self.dirty || self.continuous
    }

    pub fn due(&self, now: Instant) -> bool {
        (self.dirty || self.continuous) && now >= self.deadline
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        (self.dirty || self.continuous).then_some(self.deadline)
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

    #[test]
    fn two_fps_uses_half_second_intervals_and_coalesces_events() {
        let start = Instant::now();
        let mut clock = FrameClock::new(2, start);
        assert_eq!(clock.interval, Duration::from_millis(500));
        clock.presented(start);
        for offset in [1, 100, 250, 499] {
            clock.mark_dirty();
            assert!(!clock.due(start + Duration::from_millis(offset)));
        }
        assert!(clock.due(start + Duration::from_millis(500)));
        clock.presented(start + Duration::from_millis(500));
        clock.mark_dirty();
        assert!(!clock.due(start + Duration::from_millis(999)));
        assert!(clock.due(start + Duration::from_secs(1)));
    }

    #[test]
    fn one_hundred_forty_four_fps_is_not_capped_at_sixty() {
        let start = Instant::now();
        let mut clock = FrameClock::new(144, start);
        let period = Duration::from_secs_f64(1.0 / 144.0);
        assert_eq!(clock.interval, period);
        clock.presented(start);
        for offset in [1, 1_000, 1_000_000, 6_000_000] {
            clock.mark_dirty();
            assert!(!clock.due(start + Duration::from_nanos(offset)));
        }
        assert!(!clock.due(start + period - Duration::from_nanos(1)));
        assert!(clock.due(start + period));
        clock.presented(start + period);
        clock.mark_dirty();
        assert!(!clock.due(start + period));
        assert!(clock.due(start + period + period));
    }

    #[test]
    fn maximum_u32_fps_still_has_a_nonzero_representable_period() {
        let start = Instant::now();
        let mut clock = FrameClock::new(u32::MAX, start);
        assert_eq!(clock.interval, Duration::from_nanos(1));
        clock.presented(start);
        clock.mark_dirty();
        assert!(!clock.due(start));
        assert!(clock.due(start + Duration::from_nanos(1)));
    }

    #[test]
    fn continuous_frames_keep_the_next_deadline_after_presenting() {
        let start = Instant::now();
        let mut clock = FrameClock::new(60, start);
        clock.set_continuous(true);
        assert!(clock.due(start));
        clock.presented(start);
        assert!(clock.is_dirty());
        assert!(!clock.due(start + Duration::from_millis(16)));
        assert!(clock.due(start + Duration::from_millis(17)));
        clock.presented(start + Duration::from_millis(17));
        clock.set_continuous(false);
        assert!(!clock.is_dirty());
        assert!(clock.next_deadline().is_none());
    }
}
