use radiust_core::dbz::wrap_native_reflectivity;
use radiust_core::errors::CoreError;
use radiust_core::model::{FrameRef, Grid, RadarField, RawFrame};
use radiust_core::raster::{QUALITY_MISSING, RasterInput, RasterResultData};

fn raw_frame() -> RawFrame {
    let mut frame = FrameRef {
        source: "fixture".into(),
        product: "reflectivity".into(),
        station: Some("site".into()),
        valid_time: "2026-09-18T03:50:00Z".into(),
        base_time: None,
        logical_id: String::new(),
        revision: Some("acquisition-revision".into()),
        locator_version: "1".into(),
        locator: serde_json::json!({"url":"https://example.invalid/raw"}),
    };
    frame.logical_id = radiust_core::identity::logical_id(&frame).unwrap();
    RawFrame { frame, artifacts: Vec::new(), private_locator: None }
}

fn field() -> RadarField {
    RadarField {
        name: "reflectivity".into(),
        values: vec![-5.0, 70.25, 122.5, f32::NAN],
        shape: vec![2, 2],
        quality: vec![0, 0, 0, QUALITY_MISSING],
        units: Some("dBZ".into()),
        valid_time: "2026-09-18T03:50:00Z".into(),
        grid: Grid {
            shape: vec![2, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![100.0, 101.0],
            y: vec![20.0, 19.0],
            affine: Some([99.5, 1.0, 0.0, 20.5, 0.0, -1.0]),
        },
        provenance: vec!["decoder=fixture-native-v1".into(), "row_order=north_to_south".into()],
    }
}

#[test]
fn native_dbz_wrap_keeps_values_quality_coordinates_and_acquisition_identity() {
    let raw = raw_frame();
    let expected = field();
    let result = wrap_native_reflectivity(&raw, expected.clone()).unwrap();
    let RasterResultData::Native(shared) = result.data else {
        panic!("native direct result was copied into a pixel decoder");
    };
    assert_eq!(shared.name, expected.name);
    assert_eq!(shared.shape, expected.shape);
    assert_eq!(shared.quality, expected.quality);
    assert_eq!(shared.units, expected.units);
    assert_eq!(shared.valid_time, expected.valid_time);
    assert_eq!(shared.grid, expected.grid);
    assert_eq!(shared.provenance, expected.provenance);
    assert!(
        shared
            .values
            .iter()
            .zip(&expected.values)
            .all(|(actual, wanted)| { (actual.is_nan() && wanted.is_nan()) || actual == wanted })
    );
    assert_eq!(std::sync::Arc::strong_count(&shared), 1);
    assert_eq!(result.processing.method, "native_dbz");
    assert!(result.processing.encoding_basis.is_none());
    assert!(result.processing.clipped_pixel_count.is_none());
    assert_eq!(result.mode_info.actual.as_deref(), Some("dbz"));
    assert_eq!(result.mode_info.range_policy.as_deref(), Some("native"));
    let expected_revision =
        radiust_core::identity::resolved_revision(&[], Some("acquisition-revision")).unwrap();
    assert!(matches!(
        result.input,
        RasterInput::Source { ref resolved_revision, .. }
            if resolved_revision == &expected_revision
    ));
}

#[test]
fn explicit_dbz_rejects_non_reflectivity_units_without_reinterpreting_values() {
    let mut precipitation = field();
    precipitation.name = "precipitation_rate".into();
    precipitation.units = Some("mm/h".into());
    assert!(matches!(
        wrap_native_reflectivity(&raw_frame(), precipitation),
        Err(CoreError::UnitMismatch { variable, units: Some(units) })
            if variable == "precipitation_rate" && units == "mm/h"
    ));
}
