use radiust_core::dbz::{decode_gray_file, decode_gray_values};
use radiust_core::errors::CoreError;
use radiust_core::raster::{AlphaPlane, RasterInput, RasterResultData};
use sha2::{Digest, Sha256};
use std::path::Path;

fn pixel(result: &radiust_core::raster::RasterResult) -> &radiust_core::raster::PixelDbzField {
    match &result.data {
        RasterResultData::Pixel(value) => value,
        _ => panic!("local gray decoding must produce a pixel-space result"),
    }
}

#[test]
fn all_225_integer_codes_map_to_the_declared_dbz_values() {
    let codes = (0..=224).map(f64::from).collect::<Vec<_>>();
    let result = decode_gray_values(225, 1, &codes, None, "gray-dbz-v1").unwrap();
    let field = pixel(&result);
    assert_eq!((field.width, field.height), (225, 1));
    assert_eq!(field.variable, "reflectivity");
    assert_eq!(field.units, "dBZ");
    for (gray, value) in field.values.iter().enumerate() {
        assert!((*value - gray as f32 * 5.0 / 16.0).abs() <= 1e-6);
    }
    assert_eq!(field.values[0], 0.0);
    assert_eq!(field.values[16], 5.0);
    assert_eq!(field.values[32], 10.0);
    assert_eq!(field.values[160], 50.0);
    assert_eq!(field.values[224], 70.0);
    assert!(field.quality.iter().all(|value| *value == 0));
}

#[test]
fn alpha_is_checked_at_its_original_depth_without_scaling_dbz() {
    let gray = [16.0; 5];
    let alpha = AlphaPlane::U16(vec![0, 1, 255, 256, 65_535]);
    let result = decode_gray_values(5, 1, &gray, Some(alpha), "gray-dbz-v1").unwrap();
    let field = pixel(&result);
    assert!(field.values[0].is_nan());
    assert_eq!(field.quality[0] & 1, 1);
    assert!(field.values[1..].iter().all(|value| *value == 5.0));
    assert_eq!(field.alpha.as_ref().unwrap().bit_depth(), 16);
    assert_eq!(field.alpha.as_ref().unwrap().preview_u8(), vec![0, 1, 1, 1, 255]);
}

#[test]
fn invalid_array_values_report_a_safe_pixel_location() {
    let invalid_cases: &[&[f64]] = &[&[225.0], &[-1.0], &[0.5], &[f64::NAN], &[f64::INFINITY]];
    for values in invalid_cases {
        let error = decode_gray_values(1, 1, values, None, "gray-dbz-v1").unwrap_err();
        assert!(matches!(
            error,
            CoreError::InvalidGrayEncoding { row: Some(0), column: Some(0), .. }
        ));
    }
    let error = decode_gray_values(1, 1, &[0.0], None, "wrong-encoding").unwrap_err();
    assert!(matches!(error, CoreError::InvalidGrayEncoding { .. }));

    let transparent_hidden = decode_gray_values(
        2,
        1,
        &[f64::NAN, 225.0],
        Some(AlphaPlane::U8(vec![0, 255])),
        "gray-dbz-v1",
    );
    assert!(matches!(
        transparent_hidden,
        Err(CoreError::InvalidGrayEncoding { column: Some(1), .. })
    ));
    let transparent_only =
        decode_gray_values(1, 1, &[f64::NAN], Some(AlphaPlane::U8(vec![0])), "gray-dbz-v1")
            .unwrap();
    assert!(pixel(&transparent_only).values[0].is_nan());
}

#[test]
fn local_png_reader_preserves_16_bit_gray_and_alpha_and_rejects_visible_overflow() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/gray-dbz/local");
    let path = root.join("gray-alpha-16.png");
    let result = decode_gray_file(&path, None).unwrap();
    let field = pixel(&result);
    assert_eq!((field.width, field.height), (5, 1));
    assert_eq!(field.alpha.as_ref().unwrap().bit_depth(), 16);
    assert_eq!(field.alpha.as_ref().unwrap().preview_u8(), vec![0, 1, 1, 1, 255]);
    assert!(field.values[0].is_nan());
    assert_eq!(field.values[1], 5.0);
    assert_eq!(field.values[4], 70.0);

    let error = decode_gray_file(root.join("visible-out-of-range.png"), None).unwrap_err();
    assert!(matches!(error, CoreError::InvalidGrayEncoding { .. }));
}

#[test]
fn local_receipts_keep_content_identity_and_unknown_geospatial_metadata() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/gray-dbz/local");
    let path = root.join("valid-black.png");
    let bytes = std::fs::read(&path).unwrap();
    let result = decode_gray_file(&path, None).unwrap();
    let field = pixel(&result);
    let RasterInput::Local { identity, read_receipt } = &result.input else {
        panic!("local image must have a local identity")
    };
    assert_eq!(identity.encoding_declared, "gray-dbz-v1");
    assert_eq!(identity.content_sha256, hex::encode(Sha256::digest(&bytes)));
    assert_eq!(read_receipt.source_bit_depth, 8);
    assert_eq!(read_receipt.source_dtype, "u8");
    assert_eq!(read_receipt.media_type, "image/png");
    assert_eq!(read_receipt.frame_index, None);
    assert_eq!(field.values, [0.0]);
    assert_eq!(field.valid_time, None);
    assert_eq!(field.geometry, None);
    assert_eq!(field.encoding_adjustment.as_deref(), Some(&[0][..]));
    assert_eq!(result.mode_info.time_status.as_deref(), Some("unknown"));
    assert_eq!(result.mode_info.geolocation.as_deref(), Some("unknown"));
}

#[test]
fn local_reader_rejects_color_corruption_and_array_shape_mismatch() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/gray-dbz/local");
    for name in ["not-gray-rgb.png", "corrupt.png"] {
        let error = decode_gray_file(root.join(name), None).unwrap_err();
        assert!(matches!(error, CoreError::InvalidGrayEncoding { .. }));
    }
    assert!(matches!(
        decode_gray_values(2, 2, &[0.0, 16.0], None, "gray-dbz-v1"),
        Err(CoreError::InvalidGrayEncoding { .. })
    ));
    assert!(matches!(
        decode_gray_values(2, 1, &[0.0, 16.0], Some(AlphaPlane::U8(vec![255])), "gray-dbz-v1"),
        Err(CoreError::InvalidGrayEncoding { .. })
    ));
}

#[test]
fn animated_image_requires_an_explicit_frame_and_uses_the_selected_frame() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/gray-dbz/local");
    let path = root.join("two-frame.gif");
    let missing = decode_gray_file(&path, None).unwrap_err();
    assert!(matches!(missing, CoreError::InvalidGrayEncoding { .. }));
    let first = decode_gray_file(&path, Some(0)).unwrap();
    let second = decode_gray_file(&path, Some(1)).unwrap();
    assert_eq!(pixel(&first).values, vec![5.0]);
    assert_eq!(pixel(&second).values, vec![10.0]);
}
