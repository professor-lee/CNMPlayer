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
