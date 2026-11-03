use super::EqParams;
use super::EqSource;
use crate::tmplayer::app::state::EqSettings;
use crate::tmplayer::audio::lufs_meter::LufsMeter;
use crate::tmplayer::audio::pcm_tap::AnalysisDemand;
use crate::tmplayer::audio::pcm_tap::PcmRing;
use crate::tmplayer::audio::pcm_tap::PcmSnapshot;
use rodio::Source;
use rodio::buffer::SamplesBuffer;
use std::num::NonZero;
use std::sync::Arc;
use std::time::Duration;

fn gains(db: f32) -> EqSettings {
    let mut eq = EqSettings::default();
    eq.bands_db[5] = db;
    eq
}

fn source(
    samples: Vec<f32>,
    channels: u16,
    eq: EqSettings,
) -> (EqSource<SamplesBuffer>, Arc<EqParams>) {
    let params = Arc::new(EqParams::new());
    params.set_from(eq);
    let decoder = SamplesBuffer::new(
        NonZero::new(channels).unwrap(),
        NonZero::new(48_000).unwrap(),
        samples,
    );
    (
        EqSource::new(
            decoder,
            params.clone(),
            Arc::new(PcmRing::new()),
            Arc::new(LufsMeter::new()),
            Arc::new(AnalysisDemand::new()),
        ),
        params,
    )
}

fn sine(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|frame| (frame as f32 * std::f32::consts::TAU * 1000.0 / 48_000.0).sin() * 0.1)
        .collect()
}

#[test]
fn flat_eq_is_bit_exact_and_preserves_stereo_order() {
    let samples = vec![-0.0, 0.125, -0.75, 0.333, 0.01, -0.2, 0.9, -0.01];
    let (source, _) = source(samples.clone(), 2, EqSettings::default());
    let actual: Vec<_> = source.map(f32::to_bits).collect();
    assert_eq!(
        actual,
        samples.into_iter().map(f32::to_bits).collect::<Vec<_>>()
    );
}

#[test]
fn flat_eq_keeps_post_eq_pcm_tap_available() {
    let samples: Vec<_> = sine(512)
        .into_iter()
        .flat_map(|sample| [sample, -sample])
        .collect();
    let ring = Arc::new(PcmRing::new());
    let demand = Arc::new(AnalysisDemand::new());
    demand.set(true, false);
    let source = EqSource::new(
        SamplesBuffer::new(
            NonZero::new(2).unwrap(),
            NonZero::new(48_000).unwrap(),
            samples.clone(),
        ),
        Arc::new(EqParams::new()),
        ring.clone(),
        Arc::new(LufsMeter::new()),
        demand,
    );
    let actual: Vec<_> = source.collect();
    assert_eq!(
        actual.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        samples.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
    );
    let mut snapshot = PcmSnapshot::default();
    ring.snapshot(&mut snapshot);
    assert_eq!(snapshot.len, 512);
    assert_eq!(
        &snapshot.left[..512],
        &samples.iter().step_by(2).copied().collect::<Vec<_>>()
    );
    assert_eq!(
        &snapshot.right[..512],
        &samples
            .iter()
            .skip(1)
            .step_by(2)
            .copied()
            .collect::<Vec<_>>()
    );
}

#[test]
fn enabled_eq_reaches_configured_gain_at_band_center() {
    let samples = sine(4800);
    let (source, _) = source(samples.clone(), 1, gains(6.0));
    let actual: Vec<_> = source.collect();
    let input_energy: f32 = samples[2400..].iter().map(|x| x * x).sum();
    let output_energy: f32 = actual[2400..].iter().map(|x| x * x).sum();
    let ratio = (output_energy / input_energy).sqrt();
    assert!((ratio - 1.995_262_3).abs() < 0.02, "gain ratio {ratio}");
}

#[test]
fn eq_change_applies_to_whole_frames_without_replacing_source() {
    let samples = vec![0.1, -0.1, 0.2, -0.2, 0.3, -0.3, 0.4, -0.4];
    let (mut running, params) = source(samples.clone(), 2, EqSettings::default());
    assert_eq!(running.next(), Some(samples[0]));
    params.set_from(gains(6.0));
    assert_eq!(running.next(), Some(samples[1]));
    let (mut fresh, _) = source(samples[2..].to_vec(), 2, gains(6.0));
    assert_eq!(running.next(), fresh.next());
    assert_eq!(running.next(), fresh.next());
    params.set_from(EqSettings::default());
    assert_eq!(running.collect::<Vec<_>>(), samples[4..]);
}

#[test]
fn reenabled_eq_does_not_resurrect_pre_bypass_filter_history() {
    let mut samples = vec![0.0; 256];
    samples[0] = 1.0;
    let (mut running, params) = source(samples, 1, gains(12.0));
    assert!(running.next().unwrap() > 1.0);
    params.set_from(EqSettings::default());
    for _ in 0..32 {
        assert_eq!(running.next(), Some(0.0));
    }
    params.set_from(gains(12.0));
    assert!(running.all(|sample| sample == 0.0));
}

#[test]
fn seek_during_bypass_preserves_source_metadata_and_sample_position() {
    let samples: Vec<_> = (0..480).map(|i| i as f32 / 1000.0).collect();
    let (mut running, _) = source(samples, 2, EqSettings::default());
    assert_eq!(running.channels().get(), 2);
    assert_eq!(running.sample_rate().get(), 48_000);
    assert_eq!(running.total_duration(), Some(Duration::from_millis(5)));
    running.try_seek(Duration::from_millis(2)).unwrap();
    assert_eq!(running.next(), Some(0.192));
    assert_eq!(running.next(), Some(0.193));
}
