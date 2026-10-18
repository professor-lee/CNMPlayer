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
    assert!(!clock.due(start + Duration::from_millis(16)));
    assert!(clock.due(start + Duration::from_millis(17)));
    clock.presented(start + Duration::from_millis(17));
    clock.set_continuous(false);
}
