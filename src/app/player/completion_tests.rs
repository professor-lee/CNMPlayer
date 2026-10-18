use super::{CompletionSource, SourceCompletion};
use rodio::Source;
use rodio::source::SeekError;
use std::num::NonZero;
use std::sync::Arc;
use std::time::Duration;

struct FiniteSource {
    remaining: u64,
    total_duration: Option<Duration>,
    fail_seek: bool,
}

impl Iterator for FiniteSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        Some(0.0)
    }
}

impl Source for FiniteSource {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.remaining as usize)
    }

    fn channels(&self) -> NonZero<u16> {
        NonZero::new(1).unwrap()
    }

    fn sample_rate(&self) -> NonZero<u32> {
        NonZero::new(1_000).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        if self.fail_seek {
            return Err(SeekError::NotSupported {
                underlying_source: "FiniteSource",
            });
        }
        let total_samples = self
            .total_duration
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let target_samples = pos.as_millis() as u64;
        self.remaining = total_samples.saturating_sub(target_samples);
        Ok(())
    }
}

fn wrapped(source: FiniteSource) -> (CompletionSource<FiniteSource>, Arc<SourceCompletion>) {
    let completion = SourceCompletion::new();
    (
        CompletionSource::new(source, completion.clone()),
        completion,
    )
}

fn consume_to_end(source: &mut impl Source<Item = f32>) {
    while source.next().is_some() {}
}

#[test]
fn exact_decoder_duration_is_complete() {
    let (mut source, completion) = wrapped(FiniteSource {
        remaining: 1_000,
        total_duration: Some(Duration::from_secs(1)),
        fail_seek: false,
    });
    consume_to_end(&mut source);
    assert!(!completion.decoder_failed());
}

#[test]
fn codec_padding_within_tolerance_is_complete() {
    let (mut source, completion) = wrapped(FiniteSource {
        remaining: 800,
        total_duration: Some(Duration::from_secs(1)),
        fail_seek: false,
    });
    consume_to_end(&mut source);
    assert!(!completion.decoder_failed());
}

#[test]
fn eof_before_decoder_duration_reports_failure() {
    let (mut source, completion) = wrapped(FiniteSource {
        remaining: 600,
        total_duration: Some(Duration::from_secs(1)),
        fail_seek: false,
    });
    consume_to_end(&mut source);
    assert!(completion.decoder_failed());
}

#[test]
fn unknown_decoder_duration_does_not_report_failure() {
    let (mut source, completion) = wrapped(FiniteSource {
        remaining: 1,
        total_duration: None,
        fail_seek: false,
    });
    consume_to_end(&mut source);
    assert!(!completion.decoder_failed());
}

#[test]
fn successful_seek_updates_position_before_eof_check() {
    let (mut source, completion) = wrapped(FiniteSource {
        remaining: 1_000,
        total_duration: Some(Duration::from_secs(1)),
        fail_seek: false,
    });
    source.try_seek(Duration::from_millis(800)).unwrap();
    consume_to_end(&mut source);
    assert!(!completion.decoder_failed());
}

#[test]
fn failed_seek_does_not_update_position_before_eof_check() {
    let (mut source, completion) = wrapped(FiniteSource {
        remaining: 400,
        total_duration: Some(Duration::from_secs(1)),
        fail_seek: true,
    });
    assert!(source.try_seek(Duration::from_millis(800)).is_err());
    consume_to_end(&mut source);
    assert!(completion.decoder_failed());
}

#[test]
fn successful_seek_replaces_a_prior_terminal_decode_outcome() {
    let (mut source, completion) = wrapped(FiniteSource {
        remaining: 400,
        total_duration: Some(Duration::from_secs(1)),
        fail_seek: false,
    });
    consume_to_end(&mut source);
    assert!(completion.decoder_failed());
    source.try_seek(Duration::from_millis(800)).unwrap();
    consume_to_end(&mut source);
    assert!(!completion.decoder_failed());
}
