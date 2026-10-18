use super::*;

#[test]
fn dirty_frames_are_due_immediately_and_bypass_the_frame_cap() {
    let start = Instant::now();
    let mut clock = FrameClock::new(30, start);
    assert!(clock.due(start), "the first frame is pending");
    clock.presented(start);
    assert!(!clock.due(start), "a presented clock has nothing pending");

    clock.mark_dirty();
    assert!(
        clock.due(start),
        "dirty work must not wait for the next frame deadline"
    );
    assert_eq!(clock.next_deadline(), Duration::ZERO);
    clock.presented(start);

    assert!(!clock.due(start));
    let idle = clock.next_deadline();
    assert!(
        idle >= Duration::from_millis(900) && idle <= Duration::from_millis(1_100),
        "idle must wait for the one-second heartbeat, got {idle:?}"
    );
}

#[test]
fn continuous_frames_wait_for_the_ui_deadline() {
    let start = Instant::now();
    let mut clock = FrameClock::new(60, start);
    clock.set_continuous(true);
    assert!(clock.due(start));
    clock.presented(start);

    assert!(!clock.due(start + Duration::from_millis(16)));
    assert!(clock.due(start + Duration::from_millis(17)));
    clock.presented(start + Duration::from_millis(17));
    assert!(
        clock.next_deadline() > Duration::ZERO,
        "continuous frames keep pacing after a submission"
    );

    // Dirty work bypasses the continuous deadline, but presenting it advances
    // the continuous schedule instead of leaving a zero wait behind.
    clock.mark_dirty();
    assert!(clock.due(start + Duration::from_millis(18)));
    clock.presented(start + Duration::from_millis(18));
    assert!(!clock.due(start + Duration::from_millis(19)));
    assert!(clock.next_deadline() > Duration::ZERO);
}

#[test]
fn two_fps_continuous_uses_half_second_intervals() {
    let start = Instant::now();
    let mut clock = FrameClock::new(2, start);
    assert_eq!(clock.interval, Duration::from_millis(500));
    clock.set_continuous(true);
    assert!(clock.due(start));
    clock.presented(start);

    assert!(!clock.due(start + Duration::from_millis(499)));
    assert!(clock.due(start + Duration::from_millis(500)));
}

#[test]
fn fps_change_does_not_delay_a_dirty_frame() {
    let start = Instant::now();
    let mut clock = FrameClock::new(30, start);
    clock.presented(start);
    clock.mark_dirty();
    clock.set_fps(60, start + Duration::from_millis(5));

    assert!(
        clock.due(start + Duration::from_millis(6)),
        "a dirty frame must not wait for the new FPS deadline"
    );
    assert_eq!(clock.next_deadline(), Duration::ZERO);
}

#[test]
fn one_hundred_forty_four_fps_is_not_capped_at_sixty() {
    let start = Instant::now();
    let period = Duration::from_secs_f64(1.0 / 144.0);
    let mut clock = FrameClock::new(144, start);
    assert_eq!(clock.interval, period);
    clock.presented(start);
    clock.set_continuous(true);

    assert!(!clock.due(start + period - Duration::from_nanos(1)));
    assert!(clock.due(start + period));
}

#[test]
fn maximum_u32_fps_still_has_a_nonzero_representable_period() {
    let start = Instant::now();
    let mut clock = FrameClock::new(u32::MAX, start);
    assert_eq!(clock.interval, Duration::from_nanos(1));
    clock.presented(start);
    clock.set_continuous(true);

    assert!(!clock.due(start));
    assert!(clock.due(start + Duration::from_nanos(1)));
}
