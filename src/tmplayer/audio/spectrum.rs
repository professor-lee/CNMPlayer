//! CNMPlayer's incremental, internal Cava spectrum consumer.
//!
//! Cava state stays in f64; only the presentation layer clamps and averages L/R.
//! Plans and FFT workspaces are rebuilt only for bar-count/sample-rate changes.

mod core;

use self::core::Core;
use super::pcm_tap::{PcmBatch, PcmReader, PcmRing};
use std::{
    fmt,
    time::{Duration, Instant},
};

pub(crate) const MAX_BARS: usize = 96;
pub(crate) const MINI_BARS: usize = 20;
// Cava's integral can grow at extremely high FPS. A drained window plus a real
// second of silence ends DISPLAY tails regardless of FPS, without modifying DSP.
const TAIL_TIME: Duration = Duration::from_secs(1);
// Cava's horizontal Monstercat filter is enabled by the project default;
// `waves` remains disabled. It runs after the core's per-bar DSP state, as in
// cava.c, so it never feeds back into Cava's peak/integral history.
const MONSTERCAT_FACTOR: f64 = 1.0;
const DISPLAY_FLOOR: f64 = 1.0 / 65536.0;

#[derive(Debug)]
pub(crate) enum SpectrumError {
    SampleRate(u32),
    Bars(usize),
    Cutoff { low: u32, high: u32, rate: u32 },
    BandMapping(usize),
    Fft(String),
}

impl fmt::Display for SpectrumError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SampleRate(rate) => write!(formatter, "unsupported Cava sample rate: {rate}"),
            Self::Bars(bars) => write!(formatter, "unsupported Cava bars per channel: {bars}"),
            Self::Cutoff { low, high, rate } => write!(
                formatter,
                "invalid Cava cutoff {low}..{high} Hz at {rate} Hz"
            ),
            Self::BandMapping(bar) => {
                write!(formatter, "Cava frequency band {bar} has no valid FFT bins")
            }
            Self::Fft(error) => write!(formatter, "Cava FFT failed: {error}"),
        }
    }
}

impl std::error::Error for SpectrumError {}

pub(crate) struct Spectrum {
    bars: usize,
    core: Option<Core>,
    reader: PcmReader,
    batch: PcmBatch,
    last_update: Option<Instant>,
    playing: Option<bool>,
    quiet_since: Option<Instant>,
    zero_fraction: f64,
    display_tail: bool,
    discard_next: bool,
}

impl fmt::Debug for Spectrum {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Spectrum")
            .field("bars", &self.bars)
            .field("has_core", &self.core.is_some())
            .field("display_tail", &self.display_tail)
            .finish()
    }
}

impl Spectrum {
    pub(crate) fn new(bars: usize) -> Self {
        Self {
            bars,
            core: None,
            reader: PcmReader::default(),
            batch: PcmBatch::default(),
            last_update: None,
            playing: None,
            quiet_since: None,
            zero_fraction: 0.0,
            display_tail: false,
            discard_next: false,
        }
    }

    pub(crate) fn set_bars(&mut self, bars: usize) {
        if bars != self.bars {
            self.bars = bars;
            // Reinitialize Cava band/filter state, never resample old output.
            self.core = None;
            self.display_tail = false;
            self.quiet_since = None;
            self.zero_fraction = 0.0;
        }
    }

    pub(crate) fn clear(&mut self) {
        if let Some(core) = &mut self.core {
            core.reset();
        }
        // Keep this reader's generation/cursor. A fresh cursor would replay the
        // shared ring's historical PCM; clear must not reset that shared ring.
        self.discard_next = true;
        self.last_update = None;
        self.playing = None;
        self.quiet_since = None;
        self.zero_fraction = 0.0;
        self.display_tail = false;
    }

    pub(crate) fn update(
        &mut self,
        ring: &PcmRing,
        playing: bool,
        now: Instant,
    ) -> Result<(), SpectrumError> {
        // read_since releases its ring lock before returning: no FFT under lock.
        ring.read_since(&mut self.reader, &mut self.batch);
        let elapsed = self
            .last_update
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        let resumed = playing && self.playing == Some(false);
        self.last_update = Some(now);
        self.playing = Some(playing);
        if self.batch.reset {
            if let Some(core) = &mut self.core {
                core.reset();
            }
            self.display_tail = false;
            self.quiet_since = None;
            self.zero_fraction = 0.0;
        } else if self.batch.discontinuity
            && let Some(core) = &mut self.core
        {
            core.clear_window();
        }
        if self.batch.sample_rate == 0 {
            return Ok(());
        }
        if self
            .core
            .as_ref()
            .is_none_or(|core| core.rate != self.batch.sample_rate)
        {
            let high = 8000.min(self.batch.sample_rate / 2);
            let low = 50.min(high.saturating_sub(1));
            self.core = Some(Core::new(self.bars, self.batch.sample_rate, low, high)?);
            self.display_tail = false;
            self.quiet_since = None;
            self.zero_fraction = 0.0;
        }
        let core = self.core.as_mut().expect("core initialized above");
        let discard = resumed || self.discard_next;
        self.discard_next = false;
        let frames = if playing {
            self.zero_fraction = 0.0;
            if discard { 0 } else { self.batch.len }
        } else {
            // Paused read batches are intentionally consumed/discarded so a
            // resume cannot feed old ring samples back into the history window.
            let exact = elapsed.as_secs_f64() * core.rate as f64 + self.zero_fraction;
            self.zero_fraction = exact.fract();
            (exact as usize).min(core.window_len())
        };
        let signal = playing
            && frames != 0
            && (self.batch.left[..frames.min(core.window_len())]
                .iter()
                .any(|&value| value != 0.0)
                || self.batch.right[..frames.min(core.window_len())]
                    .iter()
                    .any(|&value| value != 0.0));
        if signal {
            self.quiet_since = None;
            self.display_tail = true;
        } else if !playing || frames != 0 || !core.has_window() {
            self.quiet_since.get_or_insert(now);
        }
        core.execute(
            if playing {
                Some((&self.batch.left, &self.batch.right))
            } else {
                None
            },
            frames,
        )?;
        apply_monstercat(core);
        if !core.has_window() {
            let below_floor = core.output[..core.bars * 2]
                .iter()
                .all(|&value| value.abs() < DISPLAY_FLOOR);
            let timed_out = self
                .quiet_since
                .is_some_and(|start| now.saturating_duration_since(start) >= TAIL_TIME);
            if below_floor || timed_out {
                self.display_tail = false;
            }
        }
        Ok(())
    }

    pub(crate) fn copy_bars(&self, mono: &mut [f32], left: &mut [f32], right: &mut [f32]) {
        mono.fill(0.0);
        left.fill(0.0);
        right.fill(0.0);
        if !self.display_tail {
            return;
        }
        let Some(core) = &self.core else { return };
        for n in 0..core.bars {
            let l = core.output[n].clamp(0.0, 1.0) as f32;
            let r = core.output[n + core.bars].clamp(0.0, 1.0) as f32;
            if let Some(value) = left.get_mut(n) {
                *value = l;
            }
            if let Some(value) = right.get_mut(n) {
                *value = r;
            }
            if let Some(value) = mono.get_mut(n) {
                *value = (l + r) / 2.0;
            }
        }
    }

    pub(crate) fn mini_bars(&self) -> [f32; MINI_BARS] {
        let mut result = [0.0; MINI_BARS];
        if !self.display_tail {
            return result;
        }
        let Some(core) = &self.core else {
            return result;
        };
        for (n, value) in result.iter_mut().enumerate() {
            let start = n * core.bars / MINI_BARS;
            let end = ((n + 1) * core.bars / MINI_BARS)
                .max(start + 1)
                .min(core.bars);
            let mut sum = 0.0;
            for bar in start..end {
                sum += (core.output[bar].clamp(0.0, 1.0)
                    + core.output[bar + core.bars].clamp(0.0, 1.0))
                    / 2.0;
            }
            *value = (sum / (end - start) as f64) as f32;
        }
        result
    }

    pub(crate) fn has_tail(&self) -> bool {
        self.display_tail
    }
}

fn apply_monstercat(core: &mut Core) {
    let decay = MONSTERCAT_FACTOR * 1.5;
    for channel in 0..2 {
        let start = channel * core.bars;
        for source_bar in 0..core.bars {
            let source = core.output[start + source_bar];
            for bar in (0..source_bar).rev() {
                let distance = (source_bar - bar) as i32;
                let candidate = source / decay.powi(distance);
                core.output[start + bar] = core.output[start + bar].max(candidate);
            }
            for bar in source_bar + 1..core.bars {
                let distance = (bar - source_bar) as i32;
                let candidate = source / decay.powi(distance);
                core.output[start + bar] = core.output[start + bar].max(candidate);
            }
        }
    }
}
