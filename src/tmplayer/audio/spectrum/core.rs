//! Linear, autosensed Cava DSP, rewritten from cavacore.c at
//! 6d43df3b2c7882122585c02c064b20009842a6f8. See THIRD_PARTY_NOTICES.md.
//! FFTW is replaced by unnormalized f64 realfft; no other smoothing is added.

use super::{MAX_BARS, SpectrumError};
use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};
use std::{f64::consts::PI, sync::Arc};

const NOISE: f64 = 0.77;

struct FftWorkspace {
    plan: Arc<dyn RealToComplex<f64>>,
    hann: Vec<f64>,
    input: Vec<f64>,
    output: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
}

impl FftWorkspace {
    fn new(planner: &mut RealFftPlanner<f64>, size: usize) -> Self {
        let plan = planner.plan_fft_forward(size);
        Self {
            hann: (0..size)
                .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f64 / (size - 1) as f64).cos()))
                .collect(),
            input: plan.make_input_vec(),
            output: plan.make_output_vec(),
            scratch: plan.make_scratch_vec(),
            plan,
        }
    }

    fn transform(&mut self, history: &[f64]) -> Result<(), SpectrumError> {
        for ((input, &sample), &hann) in self.input.iter_mut().zip(history).zip(&self.hann) {
            *input = sample * hann;
        }
        self.plan
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .map_err(|error| SpectrumError::Fft(error.to_string()))
    }
}

pub(super) struct Core {
    pub(super) rate: u32,
    pub(super) bars: usize,
    pub(super) output: [f64; MAX_BARS * 2],
    bass: FftWorkspace,
    treble: FftWorkspace,
    // C reverses the interleaved buffer, then deinterleaves it. These windows
    // retain the equivalent newest-to-oldest order without an interleaved copy.
    history: [Vec<f64>; 2],
    nonzero_samples: usize,
    lower: Vec<usize>,
    upper: Vec<i32>,
    cutoff: Vec<f32>,
    eq: Vec<f64>,
    bass_bars: usize,
    fall: [f64; MAX_BARS * 2],
    memory: [f64; MAX_BARS * 2],
    peak: [f64; MAX_BARS * 2],
    previous: [f64; MAX_BARS * 2],
    sensitivity: f64,
    sensitivity_initial: bool,
    framerate: f64,
    frame_skip: u64,
}

impl Core {
    pub(super) fn new(bars: usize, rate: u32, low: u32, high: u32) -> Result<Self, SpectrumError> {
        if !(1..=384_000).contains(&rate) {
            return Err(SpectrumError::SampleRate(rate));
        }
        let size = match rate {
            1..=8125 => 512,
            8126..=16250 => 1024,
            16251..=32500 => 2048,
            32501..=75000 => 4096,
            75001..=150000 => 8192,
            150001..=300000 => 16384,
            _ => 32768,
        };
        if bars == 0 || bars > MAX_BARS || bars > size / 2 + 1 {
            return Err(SpectrumError::Bars(bars));
        }
        if low == 0 || low >= high || high > rate / 2 {
            return Err(SpectrumError::Cutoff { low, high, rate });
        }
        let mut planner = RealFftPlanner::<f64>::new();
        let mut core = Self {
            rate,
            bars,
            output: [0.0; MAX_BARS * 2],
            bass: FftWorkspace::new(&mut planner, size * 2),
            treble: FftWorkspace::new(&mut planner, size),
            history: [vec![0.0; size * 2], vec![0.0; size * 2]],
            nonzero_samples: 0,
            lower: vec![0; bars + 1],
            upper: vec![0; bars + 1],
            cutoff: vec![0.0; bars + 1],
            eq: vec![0.0; bars],
            bass_bars: 0,
            fall: [0.0; MAX_BARS * 2],
            memory: [0.0; MAX_BARS * 2],
            peak: [0.0; MAX_BARS * 2],
            previous: [0.0; MAX_BARS * 2],
            sensitivity: 1.0,
            sensitivity_initial: true,
            framerate: 75.0,
            frame_skip: 1,
        };
        core.map_bands(low, high)?;
        Ok(core)
    }

    fn map_bands(&mut self, low: u32, high: u32) -> Result<(), SpectrumError> {
        let normal = self.treble.input.len();
        let bass = self.bass.input.len();
        // C computes the ratio and denominator in float, then calls double log10.
        let constant = ((low as f32 / high as f32) as f64).log10()
            / (1.0_f32 / (self.bars as f32 + 1.0) - 1.0) as f64;
        // This is intentionally integer division BEFORE conversion to float.
        let min_bandwidth = (self.rate / bass as u32) as f32;
        let mut first = true;
        for n in 0..=self.bars {
            let coefficient =
                -constant + ((n as f32 + 1.0) / (self.bars as f32 + 1.0)) as f64 * constant;
            self.cutoff[n] = (high as f64 * 10.0_f64.powf(coefficient)) as f32;
            if n > 0 && self.cutoff[n - 1] >= self.cutoff[n] {
                self.cutoff[n] = self.cutoff[n - 1] + min_bandwidth;
            }
            let mut relative = self.cutoff[n] / (self.rate / 2) as f32;
            if self.cutoff[n] < 100.0 {
                self.lower[n] = (relative * (bass / 2) as f32) as usize;
                self.bass_bars += 1;
                if self.bass_bars > 1 {
                    first = false;
                }
                self.lower[n] = self.lower[n].min(bass / 2);
            } else {
                self.lower[n] = (relative * (normal / 2) as f32).ceil() as usize;
                first = n == self.bass_bars;
                if first && n > 0 {
                    // C converts the entire float expression to int, not the
                    // product first. Keep a signed intermediate for truncation.
                    self.upper[n - 1] = (relative * (bass / 2) as f32 - 1.0) as i32;
                }
                self.lower[n] = self.lower[n].min(normal / 2);
            }
            if n > 0 {
                if !first {
                    self.upper[n - 1] = self.lower[n] as i32 - 1;
                    if self.lower[n] <= self.lower[n - 1] {
                        let limit = if n < self.bass_bars {
                            bass / 2
                        } else {
                            normal / 2
                        };
                        if self.lower[n - 1] < limit {
                            self.lower[n] = self.lower[n - 1] + 1;
                            self.upper[n - 1] = self.lower[n] as i32 - 1;
                        }
                    }
                } else if self.upper[n - 1] < self.lower[n - 1] as i32 {
                    self.upper[n - 1] = self.lower[n - 1] as i32 + 1;
                }
            }
            relative = self.lower[n] as f32
                / if n < self.bass_bars {
                    bass as f32 / 2.0
                } else {
                    normal as f32 / 2.0
                };
            self.cutoff[n] = relative * (self.rate as f32 / 2.0);
        }
        for n in 0..self.bars {
            let limit = if n < self.bass_bars {
                bass / 2
            } else {
                normal / 2
            };
            if self.upper[n] < self.lower[n] as i32 || self.upper[n] as usize > limit {
                return Err(SpectrumError::BandMapping(n));
            }
            self.eq[n] = 1.0 / 2.0_f64.powi(28);
            self.eq[n] *= (self.cutoff[n + 1] as f64).powf(0.85);
            self.eq[n] /= if n < self.bass_bars {
                bass as f64
            } else {
                normal as f64
            }
            .log2();
            self.eq[n] /= (self.upper[n] as i64 - self.lower[n] as i64 + 1) as f64;
        }
        Ok(())
    }

    pub(super) fn clear_window(&mut self) {
        for history in &mut self.history {
            history.fill(0.0);
        }
        self.nonzero_samples = 0;
    }

    pub(super) fn reset(&mut self) {
        self.clear_window();
        self.output.fill(0.0);
        self.fall.fill(0.0);
        self.memory.fill(0.0);
        self.peak.fill(0.0);
        self.previous.fill(0.0);
        self.sensitivity = 1.0;
        self.sensitivity_initial = true;
        self.framerate = 75.0;
        self.frame_skip = 1;
    }

    pub(super) fn has_window(&self) -> bool {
        self.nonzero_samples != 0
    }

    pub(super) fn window_len(&self) -> usize {
        self.history[0].len()
    }

    /// None means actual zero samples, not an empty execution. As in C, an
    /// oversized batch keeps its oldest prefix, not its most recent suffix.
    pub(super) fn execute(
        &mut self,
        samples: Option<(&[f32], &[f32])>,
        frames: usize,
    ) -> Result<(), SpectrumError> {
        let frames = frames.min(self.window_len());
        let mut silence = true;
        if frames != 0 {
            self.framerate -= self.framerate / 64.0;
            self.framerate += self.rate as f64 * self.frame_skip as f64 / frames as f64 / 64.0;
            self.frame_skip = 1;
            for channel in 0..2 {
                let history = &mut self.history[channel];
                self.nonzero_samples -= history[history.len() - frames..]
                    .iter()
                    .filter(|&&value| value != 0.0)
                    .count();
                let retained = history.len() - frames;
                history.copy_within(..retained, frames);
                for n in 0..frames {
                    // Match the existing float audio input's multiply before
                    // converting to Cava's double domain.
                    let value = samples.map_or(0.0, |(left, right)| {
                        let sample = if channel == 0 { left[n] } else { right[n] };
                        (sample * 65535.0_f32) as f64
                    });
                    history[frames - n - 1] = value;
                    if value != 0.0 {
                        self.nonzero_samples += 1;
                        silence = false;
                    }
                }
            }
        } else {
            self.frame_skip = self.frame_skip.saturating_add(1);
        }
        for channel in 0..2 {
            self.bass.transform(&self.history[channel])?;
            self.treble.transform(&self.history[channel])?;
            for n in 0..self.bars {
                let fft = if n < self.bass_bars {
                    &self.bass.output
                } else {
                    &self.treble.output
                };
                let mut sum = 0.0;
                for bin in self.lower[n] as i32..=self.upper[n] {
                    let value = fft[bin as usize];
                    sum += value.re.hypot(value.im);
                }
                self.output[channel * self.bars + n] = sum * self.eq[n] * self.sensitivity;
            }
        }
        let framerate_mod = 66.0 / self.framerate;
        let gravity_mod = framerate_mod.powf(2.5) * 2.0 / NOISE;
        let integral_mod = framerate_mod.powf(0.1);
        let mut overshoot = false;
        for n in 0..self.bars * 2 {
            if self.output[n] < self.previous[n] {
                self.output[n] = self.peak[n] * (1.0 - self.fall[n] * self.fall[n] * gravity_mod);
                if self.output[n] < 0.0 {
                    self.output[n] = 0.0;
                }
                self.fall[n] += 0.028;
            } else {
                self.peak[n] = self.output[n];
                self.fall[n] = 0.0;
            }
            self.previous[n] = self.output[n];
            self.output[n] += self.memory[n] * NOISE / integral_mod;
            // C saves the unbounded integral, before its output-only clamp.
            self.memory[n] = self.output[n];
            if self.output[n] > 1.0 {
                overshoot = true;
                self.output[n] = 1.0;
            }
        }
        if overshoot {
            self.sensitivity *= 1.0 - 0.02 * framerate_mod;
            self.sensitivity_initial = false;
        } else if !silence {
            self.sensitivity *= 1.0 + 0.001 * framerate_mod;
            if self.sensitivity_initial {
                self.sensitivity *= 1.0 + 0.1 * framerate_mod;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_two_tones_match_rounded_blueprint() {
        let mut core = Core::new(10, 44100, 50, 10000).unwrap();
        let mut left = [0.0; 512];
        let mut right = [0.0; 512];
        for frame in 0..300 {
            for n in 0..512 {
                let time = (n + frame * 512) as f64 / 44100.0;
                left[n] =
                    ((2.0 * std::f64::consts::PI * 200.0 * time).sin() * 20000.0 / 65535.0) as f32;
                right[n] =
                    ((2.0 * std::f64::consts::PI * 2000.0 * time).sin() * 20000.0 / 65535.0) as f32;
            }
            core.execute(Some((&left, &right)), 512).unwrap();
        }
        let blueprint = [
            0.0, 0.0, 0.994, 0.004, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.683, 0.002, 0.0, 0.0,
        ];
        for (actual, expected) in core.output[..20].iter().zip(blueprint) {
            let rounded = (actual * 1000.0).round() / 1000.0;
            assert!((rounded - expected).abs() <= expected * 0.02 + 0.0005);
        }
    }

    #[test]
    fn incremental_window_and_empty_execution_preserve_history() {
        let mut core = Core::new(20, 44100, 50, 8000).unwrap();
        core.execute(Some((&[0.25, 0.5], &[-0.25, -0.5])), 2)
            .unwrap();
        let window = core.history.clone();
        let rate = core.framerate;
        core.execute(None, 0).unwrap();
        assert_eq!(core.history, window);
        assert_eq!(core.frame_skip, 2);
        assert_eq!(core.framerate, rate);
        core.execute(Some((&[0.75], &[-0.75])), 1).unwrap();
        assert_eq!(&core.history[0][..3], &[49151.25, 32767.5, 16383.75]);
        core.execute(None, core.window_len()).unwrap();
        assert!(!core.has_window());
    }

    #[test]
    fn oversized_input_keeps_oldest_prefix_and_reset_restores_state() {
        let mut core = Core::new(20, 44100, 50, 8000).unwrap();
        let window = core.window_len();
        let mut samples = vec![0.5; window + 5];
        samples[window..].fill(0.75);
        core.execute(Some((&samples, &samples)), samples.len())
            .unwrap();
        assert!(core.history[0].iter().all(|&sample| sample == 32767.5));
        core.reset();
        assert!(!core.has_window());
        assert!(core.output.iter().all(|&value| value == 0.0));
        assert!(core.memory.iter().all(|&value| value == 0.0));
        assert_eq!(core.sensitivity, 1.0);
        assert_eq!(core.framerate, 75.0);
        assert_eq!(core.frame_skip, 1);
    }
}
