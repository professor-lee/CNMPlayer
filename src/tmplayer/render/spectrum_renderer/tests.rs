use super::*;

#[test]
fn mono_display_uses_the_entire_area_without_mirroring() {
    let mono = [0.1, 0.2, 0.4, 0.9];
    let values = build_display_vals(&mono, &[1.0; 4], &[0.0; 4], 4, BarChannels::Mono, false);
    assert_eq!(values[..4], mono);
    let reverse = build_display_vals(&mono, &[], &[], 4, BarChannels::Mono, true);
    assert_eq!(reverse[..4], [0.9, 0.4, 0.2, 0.1]);
}

#[test]
fn stereo_display_values_preserve_distinct_channels() {
    let mono = [0.0; 3];
    let left = [1.0, 0.0, 0.0];
    let right = [0.0, 0.0, 0.0];
    let values = build_display_vals(&mono, &left, &right, 6, BarChannels::Stereo, false);
    assert!(values[..3].iter().any(|value| *value > 0.0));
    assert!(values[3..6].iter().all(|value| *value == 0.0));
}
