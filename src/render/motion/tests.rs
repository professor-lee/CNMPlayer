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
