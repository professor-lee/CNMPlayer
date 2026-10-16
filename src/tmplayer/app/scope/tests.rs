use super::ScopeGain;
use crate::tmplayer::audio::pcm_tap::PcmActivity;
use std::time::{Duration, Instant};

fn at(start: Instant, milliseconds: u64) -> Instant {
    start + Duration::from_millis(milliseconds)
}

// Activity metadata deliberately has no amplitude field: these fixtures
// describe silent all-zero PCM just as well as any other successful write.
fn fresh(now: Instant, generation: u64) -> Option<PcmActivity> {
    Some(PcmActivity {
        generation,
        frames: 512,
        updated_at: Some(now),
    })
}

fn settled(start: Instant) -> ScopeGain {
    let mut gain = ScopeGain::default();
    gain.tick(true, fresh(start, 1), start);
    assert_eq!(gain.value(), 0.0);
    gain.tick(true, fresh(at(start, 500), 1), at(start, 500));
    assert_eq!(gain.value(), 1.0);
    assert!(!gain.is_animating());
    gain
}

#[test]
fn short_pcm_gap_keeps_the_wave_active_until_grace_expires() {
    let start = Instant::now();
    let mut gain = settled(start);
    let last_write = at(start, 500);
    let activity = fresh(last_write, 1);
    for elapsed in [60, 100, 120] {
        gain.tick(true, activity, at(last_write, elapsed));
        assert_eq!(gain.value(), 1.0);
        assert!(!gain.is_animating());
    }
}

#[test]
fn playing_with_stale_pcm_releases_once_and_reaches_exact_zero() {
    let start = Instant::now();
    let mut gain = settled(start);
    let activity = fresh(at(start, 500), 1);
    let silence_deadline = at(start, 620);
    gain.tick(true, activity, at(silence_deadline, 1));
    assert!(gain.value() < 1.0 && gain.value() > 0.99);
    assert!(gain.is_animating());
    gain.tick(true, activity, at(silence_deadline, 165));
    assert!((gain.value() - 0.5).abs() < 1e-6);
    gain.tick(true, activity, at(silence_deadline, 329));
    assert!(gain.value() > 0.0);
    gain.tick(true, activity, at(silence_deadline, 330));
    assert_eq!(gain.value(), 0.0);
    assert!(!gain.is_animating());
    gain.tick(true, activity, at(silence_deadline, 700));
    assert_eq!(gain.value(), 0.0);
    assert!(!gain.is_animating());
}

#[test]
fn late_pcm_stall_detection_samples_the_existing_silence_deadline() {
    let start = Instant::now();
    let mut gain = settled(start);
    let last_write = at(start, 500);
    gain.tick(true, fresh(last_write, 1), at(start, 1_000));
    assert_eq!(
        gain.value(),
        0.0,
        "a late UI frame must not start another full release delay"
    );
    assert!(!gain.is_animating());
}

#[test]
fn fresh_zero_pcm_is_active_but_missing_or_empty_pcm_never_rises() {
    let start = Instant::now();
    let mut gain = ScopeGain::default();
    gain.tick(true, None, start);
    gain.tick(true, None, at(start, 500));
    assert_eq!(gain.value(), 0.0);
    assert!(!gain.is_animating());
    let available = at(start, 500);
    for (frames, updated_at) in [(0, Some(available)), (1, Some(available)), (512, None)] {
        gain.tick(
            true,
            Some(PcmActivity {
                generation: 1,
                frames,
                updated_at,
            }),
            available,
        );
        assert_eq!(gain.value(), 0.0);
        assert!(!gain.is_animating());
    }
    gain.tick(true, fresh(available, 1), available);
    assert_eq!(gain.value(), 0.0);
    gain.tick(true, fresh(at(available, 100), 1), at(available, 100));
    assert!(gain.value() > 0.8);
    gain.tick(true, fresh(at(available, 500), 1), at(available, 500));
    assert_eq!(gain.value(), 1.0);
    assert!(!gain.is_animating());
}

#[test]
fn resumed_pcm_samples_release_before_continuous_fast_attack() {
    let start = Instant::now();
    let mut resumed = settled(start);
    let mut interrupted = settled(start);
    let release_start = at(start, 500);
    resumed.tick(false, fresh(release_start, 1), release_start);
    interrupted.tick(false, fresh(release_start, 1), release_start);
    let resume = at(release_start, 165);
    interrupted.tick(false, fresh(resume, 1), resume);
    resumed.tick(true, fresh(resume, 1), resume);
    assert_eq!(resumed.value(), interrupted.value());
    assert!((resumed.value() - 0.5).abs() < 1e-6);
    resumed.tick(true, fresh(at(resume, 100), 1), at(resume, 100));
    assert!(resumed.value() > 0.9);
    assert!(resumed.value() <= 1.0);
}

#[test]
fn pause_and_stop_bypass_grace_and_slow_down_near_the_release_endpoint() {
    let start = Instant::now();
    for stopped in [false, true] {
        let mut gain = settled(start);
        let release_start = at(start, 500);
        let activity = if stopped {
            None
        } else {
            fresh(release_start, 1)
        };
        gain.tick(false, activity, release_start);
        assert_eq!(gain.value(), 1.0);
        assert!(gain.is_animating());
        let mut values = [0.0; 5];
        for (index, elapsed) in [132, 165, 297, 329, 330].into_iter().enumerate() {
            gain.tick(false, activity, at(release_start, elapsed));
            values[index] = gain.value();
        }
        let middle_step = values[0] - values[1];
        let final_step = values[2] - values[4];
        assert!(middle_step > 0.0);
        assert!(final_step < middle_step / 3.0);
        assert!(values[3] > 0.0);
        assert_eq!(values[4], 0.0);
        assert!(!gain.is_animating());
    }
}

#[test]
fn new_generation_discards_old_gain_and_reset_stays_idle() {
    let start = Instant::now();
    let mut gain = settled(start);
    let changed = at(start, 550);
    gain.tick(false, fresh(at(start, 500), 1), at(start, 500));
    gain.tick(true, fresh(changed, 2), changed);
    assert_eq!(gain.value(), 0.0);
    gain.tick(true, fresh(at(changed, 100), 2), at(changed, 100));
    assert!(gain.value() > 0.8);
    gain.tick(
        true,
        Some(PcmActivity {
            generation: 3,
            frames: 0,
            updated_at: None,
        }),
        at(changed, 110),
    );
    assert_eq!(gain.value(), 0.0);
    assert!(!gain.is_animating());
    gain.reset();
    gain.tick(true, None, at(changed, 500));
    assert_eq!(gain.value(), 0.0);
    assert!(!gain.is_animating());
}

#[test]
fn missed_deadline_samples_the_release_endpoint_without_extra_ticks() {
    let start = Instant::now();
    let mut gain = settled(start);
    let interrupted = at(start, 500);
    gain.tick(false, fresh(interrupted, 1), interrupted);
    gain.tick(false, fresh(interrupted, 1), at(interrupted, 1_000));
    assert_eq!(gain.value(), 0.0);
    assert!(!gain.is_animating());
}

#[test]
fn different_tick_rates_sample_the_same_wall_clock_attack_and_release() {
    let start = Instant::now();
    let mut samples = [(0.0, 0.0); 3];
    for (index, fps) in [30, 60, 144].into_iter().enumerate() {
        let mut gain = ScopeGain::default();
        gain.tick(true, fresh(start, 1), start);
        for frame in 1..fps {
            let now = start + Duration::from_secs_f64(f64::from(frame) / f64::from(fps));
            if now >= at(start, 100) {
                break;
            }
            gain.tick(true, fresh(now, 1), now);
        }
        gain.tick(true, fresh(at(start, 100), 1), at(start, 100));
        let attack = gain.value();
        let interrupted = at(start, 100);
        gain.tick(false, fresh(interrupted, 1), interrupted);
        for frame in 1..fps {
            let now = interrupted + Duration::from_secs_f64(f64::from(frame) / f64::from(fps));
            if now >= at(interrupted, 200) {
                break;
            }
            gain.tick(false, None, now);
        }
        gain.tick(false, None, at(interrupted, 200));
        let release = gain.value();
        gain.tick(false, None, at(interrupted, 330));
        assert_eq!(gain.value(), 0.0);
        assert!(!gain.is_animating());
        samples[index] = (attack, release);
    }
    for sample in &samples[1..] {
        assert_eq!(*sample, samples[0]);
    }
}
