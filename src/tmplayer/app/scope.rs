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
mod tests;
