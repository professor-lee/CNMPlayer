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
        0.0, 0.0, 0.994, 0.004, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.683,
        0.002, 0.0, 0.0,
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

// Retained pre-cache execution path: deliberately independent of Core::execute
// so sensitivity placement, history invalidation and temporal order can regress.
fn execute_uncached(
    core: &mut Core,
    samples: Option<(&[f32], &[f32])>,
    frames: usize,
) -> Result<(), SpectrumError> {
    let frames = frames.min(core.window_len());
    let mut silence = true;
    if frames != 0 {
        core.framerate -= core.framerate / 64.0;
        core.framerate += core.rate as f64 * core.frame_skip as f64 / frames as f64 / 64.0;
        core.frame_skip = 1;
        for channel in 0..2 {
            let history = &mut core.history[channel];
            core.nonzero_samples -= history[history.len() - frames..]
                .iter()
                .filter(|&&value| value != 0.0)
                .count();
            let retained = history.len() - frames;
            history.copy_within(..retained, frames);
            for n in 0..frames {
                let value = samples.map_or(0.0, |(left, right)| {
                    let sample = if channel == 0 { left[n] } else { right[n] };
                    (sample * 65535.0_f32) as f64
                });
                history[frames - n - 1] = value;
                if value != 0.0 {
                    core.nonzero_samples += 1;
                    silence = false;
                }
            }
        }
    } else {
        core.frame_skip = core.frame_skip.saturating_add(1);
    }
    for channel in 0..2 {
        core.bass.transform(&core.history[channel])?;
        core.treble.transform(&core.history[channel])?;
        for n in 0..core.bars {
            let fft = if n < core.bass_bars {
                &core.bass.output
            } else {
                &core.treble.output
            };
            let mut sum = 0.0;
            for bin in core.lower[n] as i32..=core.upper[n] {
                let value = fft[bin as usize];
                sum += value.re.hypot(value.im);
            }
            core.output[channel * core.bars + n] = sum * core.eq[n] * core.sensitivity;
        }
    }
    let framerate_mod = 66.0 / core.framerate;
    let gravity_mod = framerate_mod.powf(2.5) * 2.0 / NOISE;
    let integral_mod = framerate_mod.powf(0.1);
    let mut overshoot = false;
    for n in 0..core.bars * 2 {
        if core.output[n] < core.previous[n] {
            core.output[n] = core.peak[n] * (1.0 - core.fall[n] * core.fall[n] * gravity_mod);
            if core.output[n] < 0.0 {
                core.output[n] = 0.0;
            }
            core.fall[n] += 0.028;
        } else {
            core.peak[n] = core.output[n];
            core.fall[n] = 0.0;
        }
        core.previous[n] = core.output[n];
        core.output[n] += core.memory[n] * NOISE / integral_mod;
        core.memory[n] = core.output[n];
        if core.output[n] > 1.0 {
            overshoot = true;
            core.output[n] = 1.0;
        }
    }
    if overshoot {
        core.sensitivity *= 1.0 - 0.02 * framerate_mod;
        core.sensitivity_initial = false;
    } else if !silence {
        core.sensitivity *= 1.0 + 0.001 * framerate_mod;
        if core.sensitivity_initial {
            core.sensitivity *= 1.0 + 0.1 * framerate_mod;
        }
    }
    Ok(())
}

fn assert_core_state_eq(cached: &Core, reference: &Core) {
    assert_eq!(cached.output, reference.output);
    assert_eq!(cached.history, reference.history);
    assert_eq!(cached.nonzero_samples, reference.nonzero_samples);
    assert_eq!(cached.fall, reference.fall);
    assert_eq!(cached.peak, reference.peak);
    assert_eq!(cached.previous, reference.previous);
    assert_eq!(cached.memory, reference.memory);
    assert_eq!(cached.sensitivity, reference.sensitivity);
    assert_eq!(cached.sensitivity_initial, reference.sensitivity_initial);
    assert_eq!(cached.framerate, reference.framerate);
    assert_eq!(cached.frame_skip, reference.frame_skip);
}

#[test]
fn cached_raw_bands_match_independent_old_execution_sequence() {
    for amplitude in [0.125, 100.0] {
        let mut cached = Core::new(8, 1000, 50, 400).unwrap();
        let mut reference = Core::new(8, 1000, 50, 400).unwrap();
        let left: [f32; 128] =
            std::array::from_fn(|n| (2.0 * PI * 70.0 * n as f64 / 1000.0).sin() as f32 * amplitude);
        let right: [f32; 128] = std::array::from_fn(|n| {
            (2.0 * PI * 240.0 * n as f64 / 1000.0).cos() as f32 * amplitude
        });
        for step in 0..24 {
            // Missing PCM, partial inserted silence, newly submitted PCM and
            // fully drained silence all still execute the temporal DSP path.
            let (samples, frames) = match step {
                1 | 3 | 5 | 9 | 11 | 15 | 17 | 19 | 21 | 23 => (None, 0),
                4 | 8 | 14 | 18 => (None, 32),
                10 => (None, cached.window_len()),
                _ => (Some((&left[..], &right[..])), left.len()),
            };
            if step == 16 {
                cached.clear_window();
                reference.clear_window();
            } else if step == 20 {
                cached.reset();
                reference.reset();
            }
            cached.execute(samples, frames).unwrap();
            execute_uncached(&mut reference, samples, frames).unwrap();
            assert_core_state_eq(&cached, &reference);
            super::super::apply_monstercat(&mut cached);
            super::super::apply_monstercat(&mut reference);
            assert_core_state_eq(&cached, &reference);
        }
    }
}

#[test]
fn raw_cache_skips_unchanged_fft_but_refreshes_inserted_zero_frames() {
    let mut core = Core::new(8, 1000, 50, 400).unwrap();
    let left = [0.25; 128];
    let right = [-0.5; 128];
    core.execute(Some((&left, &right)), left.len()).unwrap();
    let raw = core.raw;
    let memory = core.memory;
    let framerate = core.framerate;
    // Poison scratch results, not history: an empty update must use raw bands,
    // never rerun a transform or read the old FFT bins for aggregation.
    core.bass.input.fill(f64::NAN);
    core.treble.input.fill(f64::NAN);
    core.bass.output.fill(Complex::new(f64::NAN, f64::NAN));
    core.treble.output.fill(Complex::new(f64::NAN, f64::NAN));
    core.execute(None, 0).unwrap();
    assert_eq!(core.raw, raw);
    assert!(core.bass.input.iter().all(|value| value.is_nan()));
    assert!(core.treble.input.iter().all(|value| value.is_nan()));
    assert!(core.output.iter().all(|value| value.is_finite()));
    assert_ne!(core.memory, memory);
    assert_eq!(core.framerate, framerate);
    assert_eq!(core.frame_skip, 2);

    core.execute(None, 32).unwrap();
    assert_ne!(core.raw, raw);
    assert!(core.raw.iter().all(|value| value.is_finite()));
    assert_eq!(core.frame_skip, 1);
    assert_ne!(core.framerate, framerate);
}

#[test]
fn raw_cache_invalidates_first_window_clear_reset_and_new_mapping() {
    let mut core = Core::new(8, 1000, 50, 400).unwrap();
    assert!(!core.raw_valid);
    core.raw.fill(42.0);
    core.execute(None, 0).unwrap();
    assert!(core.raw_valid);
    assert!(core.raw.iter().all(|&value| value == 0.0 || value == 42.0));
    assert!(core.raw[..core.bars * 2].iter().all(|&value| value == 0.0));

    let samples = [0.5; 128];
    core.execute(Some((&samples, &samples)), samples.len())
        .unwrap();
    assert!(core.raw[..core.bars * 2].iter().any(|&value| value > 0.0));
    // Discontinuities call clear_window even when there is no replacement PCM.
    core.clear_window();
    assert!(!core.raw_valid);
    core.execute(None, 0).unwrap();
    assert!(core.raw[..core.bars * 2].iter().all(|&value| value == 0.0));

    core.execute(Some((&samples, &samples)), samples.len())
        .unwrap();
    core.reset();
    assert!(!core.raw_valid);
    core.execute(None, 0).unwrap();
    assert!(core.raw[..core.bars * 2].iter().all(|&value| value == 0.0));
    assert!(core.output.iter().all(|&value| value == 0.0));

    // A fresh rate/bar mapping has no reusable raw bands from its predecessor.
    core = Core::new(4, 2000, 60, 800).unwrap();
    assert!(!core.raw_valid);
    core.execute(None, 0).unwrap();
    assert!(core.raw[..core.bars * 2].iter().all(|&value| value == 0.0));
}
