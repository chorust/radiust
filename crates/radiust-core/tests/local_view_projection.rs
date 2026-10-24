use geographiclib_rs::{Geodesic, InverseGeodesic};
use radiust_core::model::GeographicPoint;
use radiust_core::projection::{AeqdWgs84, ProjectionError, build_local_view_grid};
use serde::Deserialize;

#[derive(Deserialize)]
struct ReferenceFile {
    generator: Generator,
    cases: Vec<ReferenceCase>,
}

#[derive(Deserialize)]
struct Generator {
    independent_of_implementation: bool,
}

#[derive(Deserialize)]
struct ReferenceCase {
    id: String,
    center_lon_deg: f64,
    center_lat_deg: f64,
    distance_m: f64,
    x_m: f64,
    y_m: f64,
    expected_lon_deg: f64,
    expected_lat_deg: f64,
}

#[test]
fn local_view_projection_matches_independent_wgs84_reference_points() {
    let reference: ReferenceFile =
        serde_json::from_str(include_str!("../../../tests/fixtures/local-view/reference.json"))
            .unwrap();
    assert!(reference.generator.independent_of_implementation);
    assert_eq!(reference.cases.len(), 59);

    let geodesic = Geodesic::wgs84();
    let mut maximum_error_m = 0.0_f64;
    for case in reference.cases {
        let projection = AeqdWgs84::new(GeographicPoint {
            longitude: case.center_lon_deg,
            latitude: case.center_lat_deg,
        })
        .unwrap();
        assert!((case.x_m.hypot(case.y_m) - case.distance_m).abs() < 1e-6, "{}", case.id);
        let actual = projection.inverse(case.x_m, case.y_m).unwrap();
        let (error_m, _, _, _): (f64, f64, f64, f64) = geodesic.inverse(
            case.expected_lat_deg,
            case.expected_lon_deg,
            actual.latitude,
            actual.longitude,
        );
        assert!(error_m <= 1.0, "{} inverse error was {error_m} m", case.id);
        maximum_error_m = maximum_error_m.max(error_m);

        let expected =
            GeographicPoint { longitude: case.expected_lon_deg, latitude: case.expected_lat_deg };
        let (east_m, north_m) = projection.forward(expected).unwrap();
        let projected_error_m = (east_m - case.x_m).hypot(north_m - case.y_m);
        assert!(projected_error_m <= 1.0, "{} forward error was {projected_error_m} m", case.id);
    }
    eprintln!(
        "independent projection reference: 59 cases; max inverse error {maximum_error_m:.6} m"
    );
}

#[test]
fn local_view_projection_grid_keeps_center_direction_and_half_pixel_affine() {
    let center = GeographicPoint { longitude: 140.0, latitude: 35.0 };
    let default = build_local_view_grid(center, 50.0, 1000.0, 100_000).unwrap();
    assert_eq!(default.grid.shape, [101, 101]);
    assert_eq!(default.actual_resolution_m, 1000.0);
    assert_eq!(default.grid.x[50], 0.0);
    assert_eq!(default.grid.y[50], 0.0);
    assert!(default.grid.x[0] < 0.0 && default.grid.x[100] > 0.0);
    assert!(default.grid.y[0] > 0.0 && default.grid.y[100] < 0.0);
    assert_eq!(default.grid.affine, Some([-50_500.0, 1000.0, 0.0, 50_500.0, 0.0, -1000.0]));

    let non_divisible = build_local_view_grid(center, 10.0, 3000.0, 1000).unwrap();
    assert_eq!(non_divisible.grid.shape, [9, 9]);
    assert_eq!(non_divisible.actual_resolution_m, 2500.0);
    assert_eq!(non_divisible.grid.x[0], -10_000.0);
    assert_eq!(non_divisible.grid.x[8], 10_000.0);
    assert_eq!(non_divisible.grid.y[0], 10_000.0);
    assert_eq!(non_divisible.grid.y[8], -10_000.0);
    assert_eq!(non_divisible.grid.affine, Some([-11_250.0, 2500.0, 0.0, 11_250.0, 0.0, -2500.0]));
}

#[test]
fn local_view_projection_checks_pixel_limit_and_dimension_overflow() {
    let center = GeographicPoint { longitude: 0.0, latitude: 0.0 };
    assert!(matches!(
        build_local_view_grid(center, 50.0, 1000.0, 100),
        Err(ProjectionError::PixelLimit { .. })
    ));
    assert!(matches!(
        build_local_view_grid(center, 500.0, 1e-8, u64::MAX),
        Err(ProjectionError::GridOverflow)
    ));
}
