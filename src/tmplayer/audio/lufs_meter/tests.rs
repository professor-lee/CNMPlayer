use super::*;

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
