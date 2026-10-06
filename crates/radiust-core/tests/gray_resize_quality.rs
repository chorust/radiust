use radiust_core::gray::propagate_quality_for_resize;
use radiust_core::raster::{
    QUALITY_INTERPOLATED, QUALITY_MISSING, QUALITY_RECOVERED, QUALITY_UNKNOWN_COLOR,
};

#[test]
fn nearest_resize_copies_quality_without_marking_interpolation() {
    let input = [QUALITY_MISSING, QUALITY_RECOVERED, QUALITY_UNKNOWN_COLOR, 0];
    let output = propagate_quality_for_resize(2, 2, 2, 2, &input, "nearest").unwrap();
    assert_eq!(output, input);
    assert!(output.iter().all(|quality| quality & QUALITY_INTERPOLATED == 0));
}

#[test]
fn bicubic_resize_propagates_signed_nonzero_support_and_combines_flags() {
    // For Pillow's cubic kernel, source x=2 contributes to output x=1 at a
    // negative but nonzero coefficient when resizing width 4 to width 8.
    let input = [0, 0, QUALITY_UNKNOWN_COLOR, QUALITY_RECOVERED];
    let output = propagate_quality_for_resize(4, 1, 8, 1, &input, "pillow_bicubic").unwrap();
    assert_ne!(output[1] & QUALITY_UNKNOWN_COLOR, 0);
    assert_ne!(output[1] & QUALITY_INTERPOLATED, 0);
    assert_ne!(output[5] & QUALITY_RECOVERED, 0);
    assert_ne!(output[5] & QUALITY_INTERPOLATED, 0);
}

#[test]
fn any_invalid_nonzero_bicubic_contribution_stays_marked() {
    let input = [0, 0, QUALITY_MISSING, 0];
    let output = propagate_quality_for_resize(4, 1, 8, 1, &input, "pillow_bicubic").unwrap();
    assert!(output.iter().any(|quality| quality & QUALITY_MISSING != 0));
    assert!(output.iter().any(|quality| quality & QUALITY_MISSING == 0));
}
