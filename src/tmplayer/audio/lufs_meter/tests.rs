use super::{LufsMeter, SHELF_PROTO, k_weighting_filters, prototype_to_biquad};

#[test]
fn shelf_filter_matches_48k_prototype() {
    let filter = prototype_to_biquad(&SHELF_PROTO, 48_000.0);
    assert!((filter.b0 - SHELF_PROTO.b0).abs() < 1.0e-9);
    assert!((filter.b1 - SHELF_PROTO.b1).abs() < 1.0e-9);
    assert!((filter.b2 - SHELF_PROTO.b2).abs() < 1.0e-9);
    assert!((filter.a1 - SHELF_PROTO.a1).abs() < 1.0e-9);
    assert!((filter.a2 - SHELF_PROTO.a2).abs() < 1.0e-9);
}

#[test]
fn shelf_filter_matches_known_44_1k_values() {
    let filter = prototype_to_biquad(&SHELF_PROTO, 44_100.0);
    assert!((filter.b0 - 1.530_880_324_135_5).abs() < 1.0e-6);
    assert!((filter.b1 - -2.651_351_292_867_2).abs() < 1.0e-6);
    assert!((filter.b2 - 1.169_344_026_329_0).abs() < 1.0e-6);
    assert!((filter.a1 - -1.663_901_800_496_3).abs() < 1.0e-6);
    assert!((filter.a2 - 0.712_774_858_093_6).abs() < 1.0e-6);
}

#[test]
fn high_pass_filter_keeps_standard_b_coefficients() {
    let filters = k_weighting_filters(44_100.0);
    assert!((filters[1].b0 - 1.0).abs() < 1.0e-12);
    assert!((filters[1].b1 + 2.0).abs() < 1.0e-12);
    assert!((filters[1].b2 - 1.0).abs() < 1.0e-12);
}

#[test]
fn silence_publishes_zero_mean_square() {
    let meter = LufsMeter::new();
    meter.configure_and_reset(2, 48_000);
    let samples = vec![0.0f32; 4800];
    meter.push_batch(&samples, &samples);
    let reading = meter.latest();
    assert_eq!(reading.left_mean_square, 0.0);
    assert_eq!(reading.right_mean_square, 0.0);
    assert!(reading.generation > 0);
}

#[test]
fn reset_while_writer_is_contended_clears_reading_and_filter_history() {
    let meter = LufsMeter::new();
    meter.configure_and_reset(2, 48_000);
    let loud: Vec<f32> = (0..4800)
        .map(|frame| if frame % 2 == 0 { 0.75 } else { -0.75 })
        .collect();
    meter.push_batch(&loud, &loud);
    let before = meter.latest();
    assert!(before.left_mean_square > 0.1);
    {
        let _guard = meter.inner.lock().unwrap();
        meter.reset();
        meter.push_batch(&loud, &loud);
    }
    let cleared = meter.latest();
    assert_eq!(cleared.left_mean_square, 0.0);
    assert_eq!(cleared.right_mean_square, 0.0);
    assert!(cleared.generation > before.generation);

    let silence = [0.0; 2400];
    meter.push_batch(&silence, &silence);
    let resumed = meter.latest();
    assert_eq!(resumed.left_mean_square, 0.0);
    assert_eq!(resumed.right_mean_square, 0.0);
    assert!(resumed.generation > cleared.generation);
}

#[test]
fn reset_before_a_new_write_discards_partial_subblocks_and_old_window() {
    let meter = LufsMeter::new();
    meter.configure_and_reset(2, 48_000);
    meter.push_batch(&[0.9; 3601], &[-0.6; 3601]);
    {
        let _guard = meter.inner.lock().unwrap();
        meter.reset();
    }
    let fresh = LufsMeter::new();
    fresh.configure_and_reset(2, 48_000);
    let left: Vec<f32> = (0..2400)
        .map(|frame| if frame % 2 == 0 { 0.2 } else { -0.2 })
        .collect();
    let right: Vec<f32> = left.iter().map(|sample| sample * 0.5).collect();
    meter.push_batch(&left, &right);
    fresh.push_batch(&left, &right);
    let resumed = meter.latest();
    let expected = fresh.latest();
    assert!(resumed.left_mean_square > 0.01);
    assert_eq!(resumed.left_mean_square, expected.left_mean_square);
    assert_eq!(resumed.right_mean_square, expected.right_mean_square);
}
