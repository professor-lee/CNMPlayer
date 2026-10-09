//! One wall-clock envelope for the real-PCM oscilloscope, shared by every column.
use std::time::{Duration, Instant};

use crate::render::motion::{Curve, Transition};
use crate::tmplayer::audio::pcm_tap::PcmActivity;

// Retain cava's integral-inspired attack: noise_reduction / integral_mod,
// where integral_mod = (66 / reference_fps)^0.1 and default reduction is 77%.
const CAVA_REFERENCE_HZ: f32 = 66.0;
const CAVA_NOISE_REDUCTION: f32 = 0.77;
const SCOPE_REFERENCE_HZ: f32 = 60.0;
const SCOPE_SETTLED_EPSILON: f32 = 1.0 / 255.0;
const PCM_STALE_AFTER: Duration = Duration::from_millis(120);
const RELEASE_DURATION: Duration = Duration::from_millis(330);

#[derive(Debug)]
enum Phase {
    Idle,
    Attack { started: Instant, from: f32 },
    Steady,
    Release(Transition),
}

/// Gain scales the waveform itself to a flat line; there is no separate centerline.
/// PCM freshness, not sample amplitude, determines whether the source is active.
#[derive(Debug)]
pub(crate) struct ScopeGain {
    current: f32,
    generation: Option<u64>,
    phase: Phase,
    integral_retention: f32,
}

impl Default for ScopeGain {
    fn default() -> Self {
        Self {
            current: 0.0,
            generation: None,
            phase: Phase::Idle,
            integral_retention: CAVA_NOISE_REDUCTION
                / (CAVA_REFERENCE_HZ / SCOPE_REFERENCE_HZ).powf(0.1),
        }
    }
}

impl ScopeGain {
    pub(crate) fn tick(&mut self, playing: bool, activity: Option<PcmActivity>, now: Instant) {
        if let Some(activity) = activity
            && self.generation != Some(activity.generation)
        {
            // A seek/song/sample-rate epoch must never revive the old waveform.
            self.reset();
            self.generation = Some(activity.generation);
        }

        let expires_at = activity
            .filter(|activity| activity.frames >= 2)
            .and_then(|activity| activity.updated_at)
            .map(|updated_at| updated_at + PCM_STALE_AFTER);
        let active = playing && expires_at.is_some_and(|deadline| now <= deadline);
        // A late render must not extend either the attack or the stall grace.
        let phase_at =
            if playing && !active && matches!(self.phase, Phase::Attack { .. } | Phase::Steady) {
                expires_at.map_or(now, |deadline| deadline.min(now))
            } else {
                now
            };

        // Sample the existing phase before handling edges. In particular, fresh
        // PCM arriving during release starts attack at this exact residual gain.
        match &mut self.phase {
            Phase::Attack { started, from } => {
                let frames =
                    phase_at.saturating_duration_since(*started).as_secs_f32() * SCOPE_REFERENCE_HZ;
                let remaining = (1.0 - *from) * self.integral_retention.powf(frames);
                self.current = 1.0 - remaining;
                if remaining <= SCOPE_SETTLED_EPSILON {
                    self.current = 1.0;
                    self.phase = Phase::Steady;
                }
            }
            Phase::Release(release) => {
                release.tick(now);
                self.current = release.value();
                if !release.is_running() {
                    self.phase = Phase::Idle;
                }
            }
            Phase::Idle | Phase::Steady => {}
        }

        if active {
            if matches!(self.phase, Phase::Idle | Phase::Release(_)) {
                // The edge frame preserves the sampled gain, even for the first
                // source; elapsed time on subsequent ticks drives fast attack.
                self.phase = if self.current == 1.0 {
                    Phase::Steady
                } else {
                    Phase::Attack {
                        started: now,
                        from: self.current,
                    }
                };
            }
        } else if matches!(self.phase, Phase::Attack { .. } | Phase::Steady) {
            if self.current == 0.0 {
                self.phase = Phase::Idle;
            } else {
                let mut release = Transition::new(self.current);
                release.retarget(0.0, phase_at, RELEASE_DURATION, Curve::EaseInOut);
                release.tick(now);
                self.current = release.value();
                self.phase = if release.is_running() {
                    Phase::Release(release)
                } else {
                    Phase::Idle
                };
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        self.current = 0.0;
        self.generation = None;
        self.phase = Phase::Idle;
    }

    pub(crate) fn value(&self) -> f32 {
        self.current
    }

    pub(crate) fn is_animating(&self) -> bool {
        matches!(self.phase, Phase::Attack { .. } | Phase::Release(_))
    }
}

#[cfg(test)]
mod tests {
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
}
