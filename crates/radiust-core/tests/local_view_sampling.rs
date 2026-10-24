use radiust_core::local_view::project_field_nearest;
use radiust_core::model::{GeographicPoint, Grid, RadarField};
use radiust_core::projection::build_local_view_grid;
use radiust_core::source::catalog::{
    CatalogProvenance, PixelRegistration, RasterGeometryEvidence, SpatialEvidenceStatus,
};

fn source_field(x: Vec<f64>, y: Vec<f64>) -> RadarField {
    RadarField {
        name: "reflectivity".into(),
        values: vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0],
        shape: vec![2, 3],
        quality: vec![0, 1, 2, 4, 8, 16],
        units: Some("dBZ".into()),
        valid_time: "2026-10-10T00:00:00Z".into(),
        grid: Grid { shape: vec![2, 3], crs: Some("EPSG:4326".into()), x, y, affine: None },
        provenance: vec!["controlled-non-square".into()],
    }
}

fn geometry(shape: [u32; 2]) -> RasterGeometryEvidence {
    RasterGeometryEvidence {
        status: SpatialEvidenceStatus::Verified,
        crs: Some("EPSG:4326".into()),
        registration: PixelRegistration::Center,
        shape: Some(shape),
        affine: None,
        data_window: None,
        artifact_sha256: None,
        evidence: vec![CatalogProvenance {
            source: "controlled sampling fixture".into(),
            reference: Some("unit-test".into()),
            observed_at: Some("2026-10-10".into()),
        }],
    }
}

fn geometry_with(
    registration: PixelRegistration,
    shape: [u32; 2],
    affine: Option<[f64; 6]>,
) -> RasterGeometryEvidence {
    RasterGeometryEvidence { registration, affine, ..geometry(shape) }
}

/// Unique values and qualities so any orientation or index error changes the output.
fn asymmetric_field(x: Vec<f64>, y: Vec<f64>) -> RadarField {
    RadarField {
        name: "reflectivity".into(),
        values: (1..=9).map(|value| value as f32 * 5.0).collect(),
        shape: vec![3, 3],
        quality: vec![0, 1, 2, 4, 8, 16, 32, 64, 128],
        units: Some("dBZ".into()),
        valid_time: "2026-10-10T00:00:00Z".into(),
        grid: Grid { shape: vec![3, 3], crs: Some("EPSG:4326".into()), x, y, affine: None },
        provenance: vec!["controlled-asymmetric".into()],
    }
}

/// Expected 3x3 north-up output for a 10 km radius at 10 km resolution over a 3x3
/// source with 0.1 degree pixels centered on the target: in-view pixels (center and
/// the four cardinal tips) take the source pixel found by geographic position.
fn assert_asymmetric_projection(
    field: &RadarField,
    x: &[f64],
    y: &[f64],
    geometry: RasterGeometryEvidence,
) {
    let target = build_local_view_grid(
        GeographicPoint { longitude: 0.0, latitude: 0.0 },
        10.0,
        10_000.0,
        100,
    )
    .unwrap();
    let projected = project_field_nearest(field, &geometry, target).unwrap();
    assert_eq!(projected.field.shape, [3, 3]);
    let domain = projected.grid.source_domain.as_ref().unwrap();
    for row in 0..3_usize {
        for column in 0..3_usize {
            let index = row * 3 + column;
            let north = 1_i32 - row as i32;
            let east = column as i32 - 1;
            if north.abs() + east.abs() > 1 {
                assert!(!projected.grid.in_view[index], "corner {index} must be out of view");
                assert!(!domain[index]);
                assert!(projected.field.values[index].is_nan());
                assert_eq!(projected.field.quality[index], 3);
                continue;
            }
            let want_x = 0.1 * f64::from(east);
            let want_y = 0.1 * f64::from(north);
            let source_column = x.iter().position(|value| (value - want_x).abs() < 1e-9).unwrap();
            let source_row = y.iter().position(|value| (value - want_y).abs() < 1e-9).unwrap();
            let source_index = source_row * 3 + source_column;
            assert!(projected.grid.in_view[index]);
            assert!(domain[index], "pixel {index}");
            assert_eq!(projected.field.values[index], field.values[source_index], "pixel {index}");
            assert_eq!(
                projected.field.quality[index], field.quality[source_index],
                "pixel {index}"
            );
        }
    }
}

#[test]
fn local_view_nearest_straddling_pixels_matches_every_axis_direction() {
    for x in [vec![-0.1, 0.0, 0.1], vec![0.1, 0.0, -0.1]] {
        for y in [vec![-0.1, 0.0, 0.1], vec![0.1, 0.0, -0.1]] {
            let field = asymmetric_field(x.clone(), y.clone());
            assert_asymmetric_projection(
                &field,
                &x,
                &y,
                geometry_with(PixelRegistration::Center, [3, 3], None),
            );
        }
    }
}

#[test]
fn local_view_nearest_edge_registration_uses_affine_pixel_centers() {
    let mut field = asymmetric_field(vec![], vec![]);
    field.grid.x = vec![];
    field.grid.y = vec![];
    let geometry =
        geometry_with(PixelRegistration::Edge, [3, 3], Some([-0.15, 0.1, 0.0, 0.15, 0.0, -0.1]));
    assert_asymmetric_projection(&field, &[-0.1, 0.0, 0.1], &[0.1, 0.0, -0.1], geometry);
}

#[test]
fn local_view_nearest_preserves_non_square_values_quality_and_axis_directions() {
    for x in [vec![-0.02, 0.0, 0.03], vec![0.03, 0.0, -0.02]] {
        for y in [vec![-0.01, 0.02], vec![0.02, -0.01]] {
            let field = source_field(x.clone(), y.clone());
            let center = GeographicPoint { longitude: x[1], latitude: y[0] };
            let target = build_local_view_grid(center, 1.0, 1000.0, 100).unwrap();
            let projected = project_field_nearest(&field, &geometry([2, 3]), target).unwrap();
            let center_index = (projected.field.shape[0] / 2) * projected.field.shape[1]
                + projected.field.shape[1] / 2;

            assert_eq!(projected.field.shape, [3, 3]);
            assert_eq!(projected.field.values[center_index], 20.0);
            assert_eq!(projected.field.quality[center_index], 1);
            assert!(projected.grid.in_view[center_index]);
            assert!(projected.grid.source_domain.as_ref().unwrap()[center_index]);
        }
    }
}

#[test]
fn local_view_rejects_geometry_shape_and_partial_window_conflicts() {
    let field = source_field(vec![-0.02, 0.0, 0.03], vec![-0.01, 0.02]);
    let center = GeographicPoint { longitude: 0.0, latitude: -0.01 };
    let target = build_local_view_grid(center, 1.0, 1000.0, 100).unwrap();

    assert!(project_field_nearest(&field, &geometry([3, 2]), target.clone()).is_err());

    let mut partial = geometry([2, 3]);
    partial.data_window = Some([1, 0, 2, 2]);
    assert!(project_field_nearest(&field, &partial, target).is_err());
}
