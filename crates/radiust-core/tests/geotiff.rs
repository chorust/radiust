use std::fs::File;

use radiust_core::errors::CoreError;
use radiust_core::limits::Limits;
use radiust_core::model::{Grid, RadarField};
use radiust_core::output::geotiff::{read_selected_field, write_field};
use tiff::decoder::{Decoder, DecodingResult};

fn fixture_field() -> RadarField {
    RadarField {
        name: "reflectivity".into(),
        values: vec![12.5, f32::NAN, -3.0, 0.25],
        shape: vec![2, 2],
        quality: vec![0, 1, 4, 0],
        units: Some("dBZ".into()),
        valid_time: "2026-09-26T01:02:03Z".into(),
        grid: Grid {
            shape: vec![2, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![120.0, 120.5],
            y: vec![23.0, 23.5],
            affine: None,
        },
        provenance: vec!["fixture-source".into(), "decode-v1".into()],
    }
}

#[test]
fn geotiff_group_roundtrips_float_and_quality_rasters() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("field.tif");
    let paths = write_field(&fixture_field(), &output, &Limits::default()).unwrap();

    assert_eq!(paths.len(), 3);
    assert_eq!(paths[0], output);
    assert_eq!(paths[1].file_name().unwrap(), "field_quality.tif");
    assert_eq!(paths[2].file_name().unwrap(), "field_provenance.json");
    assert!(paths.iter().all(|path| path.is_file()));

    let mut data = Decoder::new(File::open(&paths[0]).unwrap()).unwrap();
    assert_eq!(data.dimensions().unwrap(), (2, 2));
    assert!(matches!(data.read_image().unwrap(), DecodingResult::F32(values)
        if values[0] == -3.0 && values[1] == 0.25 && values[2] == 12.5 && values[3].is_nan()));

    let mut quality = Decoder::new(File::open(&paths[1]).unwrap()).unwrap();
    assert_eq!(quality.dimensions().unwrap(), (2, 2));
    assert!(matches!(quality.read_image().unwrap(), DecodingResult::U16(values)
        if values == vec![4, 0, 0, 1]));
}

#[test]
fn geotiff_reader_roundtrips_values_quality_and_pixel_centers() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("field.tif");
    write_field(&fixture_field(), &output, &Limits::default()).unwrap();

    let decoded = read_selected_field(&output, &Limits::default()).unwrap();
    let original = fixture_field();
    assert_eq!(decoded.name, original.name);
    assert_eq!(decoded.units, original.units);
    assert_eq!(decoded.valid_time, original.valid_time);
    assert_eq!(decoded.shape, original.shape);
    assert_eq!(decoded.quality, original.quality);
    assert_eq!(decoded.provenance, original.provenance);
    assert_eq!(decoded.grid.crs.as_deref(), Some("EPSG:4326"));
    assert_eq!(decoded.grid.x, original.grid.x);
    assert_eq!(decoded.grid.y, original.grid.y);
    assert_eq!(decoded.values[0], 12.5);
    assert!(decoded.values[1].is_nan());
    assert_eq!(&decoded.values[2..], &original.values[2..]);
    assert_eq!(decoded.grid.affine, Some([119.75, 0.5, 0.0, 23.75, 0.0, -0.5]));
}

#[test]
fn geotiff_writer_rejects_unverified_crs_and_irregular_axes() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("bad.tif");
    let mut field = fixture_field();
    field.grid.crs = Some("EPSG:3857".into());
    assert!(write_field(&field, &output, &Limits::default()).is_err());
    assert!(!output.exists());

    field = fixture_field();
    field.shape = vec![2, 3];
    field.values.push(1.0);
    field.values.push(2.0);
    field.quality.push(0);
    field.quality.push(0);
    field.grid.shape = vec![2, 3];
    field.grid.x.push(121.0);
    field.grid.x[2] = 122.0;
    assert!(write_field(&field, &output, &Limits::default()).is_err());
    assert!(!output.exists());
}

#[test]
fn geotiff_preserves_regular_twd67_geographic_coordinates() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("twd67.tif");
    let mut field = fixture_field();
    field.grid.crs = Some("EPSG:3821".into());

    let paths = write_field(&field, &output, &Limits::default()).unwrap();
    let provenance: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&paths[2]).unwrap()).unwrap();
    assert_eq!(provenance["source_crs"], "EPSG:3821");
    assert_eq!(provenance["crs"], "EPSG:3821");
    assert_eq!(provenance["grid"], "geographic");
    assert_eq!(provenance["coordinate_operation"], "identity");

    let decoded = read_selected_field(&output, &Limits::default()).unwrap();
    assert_eq!(decoded.grid.crs.as_deref(), Some("EPSG:3821"));
    assert_eq!(decoded.grid.x, field.grid.x);
    assert_eq!(decoded.grid.y, field.grid.y);
}

#[test]
fn geotiff_projects_irregular_geographic_latitudes_to_regular_web_mercator() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("projected.tif");
    let mercator_y = [0.0_f64, 1_000_000.0, 2_000_000.0];
    let mut field = fixture_field();
    field.shape = vec![3, 2];
    field.grid.shape = vec![3, 2];
    field.grid.y = mercator_y.map(|y| (y / 6_378_137.0).sinh().atan().to_degrees()).to_vec();
    field.values = vec![12.5, f32::NAN, -3.0, 0.25, 1.5, 2.5];
    field.quality = vec![0, 1, 4, 0, 2, 3];

    let paths = write_field(&field, &output, &Limits::default()).unwrap();
    let provenance: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&paths[2]).unwrap()).unwrap();
    assert_eq!(provenance["source_crs"], "EPSG:4326");
    assert_eq!(provenance["crs"], "EPSG:3857");
    assert_eq!(provenance["coordinate_operation"], "spherical_web_mercator");
    assert!((provenance["transform"][5].as_f64().unwrap() + 1_000_000.0).abs() < 1e-5);
    assert!((provenance["transform"][3].as_f64().unwrap() - 2_500_000.0).abs() < 1e-5);

    let mut data = Decoder::new(File::open(&paths[0]).unwrap()).unwrap();
    assert!(matches!(data.read_image().unwrap(), DecodingResult::F32(values)
        if values[0] == 1.5 && values[1] == 2.5
            && values[2] == -3.0 && values[3] == 0.25
            && values[4] == 12.5 && values[5].is_nan()));
    let mut quality = Decoder::new(File::open(&paths[1]).unwrap()).unwrap();
    assert!(matches!(quality.read_image().unwrap(), DecodingResult::U16(values)
        if values == vec![2, 3, 4, 0, 0, 1]));

    let decoded = read_selected_field(&output, &Limits::default()).unwrap();
    assert_eq!(decoded.grid.crs.as_deref(), Some("EPSG:3857"));
    assert_eq!(decoded.quality, field.quality);
    for (actual, expected) in decoded.values.iter().zip(&field.values) {
        if expected.is_nan() {
            assert!(actual.is_nan());
        } else {
            assert_eq!(actual, expected);
        }
    }
    assert_eq!(decoded.grid.x.len(), 2);
    assert!((decoded.grid.x[0] - 13_358_338.895_192_828).abs() < 1e-6);
    assert!((decoded.grid.x[1] - 13_413_998.640_589_466).abs() < 1e-6);
    for (actual, expected) in decoded.grid.y.iter().zip([0.0, 1_000_000.0, 2_000_000.0]) {
        assert!((actual - expected).abs() < 1e-8);
    }
}

#[test]
fn geotiff_reader_requires_matching_sidecars_and_regular_files() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("field.tif");
    let paths = write_field(&fixture_field(), &output, &Limits::default()).unwrap();
    std::fs::remove_file(&paths[1]).unwrap();
    assert!(read_selected_field(&output, &Limits::default()).is_err());

    write_field(&fixture_field(), &output, &Limits::default()).unwrap();
    let provenance_path = &paths[2];
    let mut provenance: serde_json::Value =
        serde_json::from_slice(&std::fs::read(provenance_path).unwrap()).unwrap();
    provenance["variable"] = serde_json::json!("other_field");
    std::fs::write(provenance_path, serde_json::to_vec(&provenance).unwrap()).unwrap();
    assert!(read_selected_field(&output, &Limits::default()).is_err());

    write_field(&fixture_field(), &output, &Limits::default()).unwrap();
    let other_output = directory.path().join("other.tif");
    let mut other = fixture_field();
    other.grid.y.reverse();
    let other_paths = write_field(&other, &other_output, &Limits::default()).unwrap();
    std::fs::copy(&other_paths[1], &paths[1]).unwrap();
    assert!(read_selected_field(&output, &Limits::default()).is_err());

    assert!(read_selected_field(directory.path(), &Limits::default()).is_err());
}

#[test]
fn geotiff_reader_enforces_pixel_file_and_frame_budgets() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("field.tif");
    write_field(&fixture_field(), &output, &Limits::default()).unwrap();

    let limits = Limits { max_pixels: 3, ..Limits::default() };
    assert!(matches!(read_selected_field(&output, &limits), Err(CoreError::ResourceLimit(_))));

    let limits = Limits { max_artifact_bytes: 1, ..Limits::default() };
    assert!(matches!(read_selected_field(&output, &limits), Err(CoreError::ResourceLimit(_))));

    let limits = Limits { max_frame_bytes: 23, ..Limits::default() };
    assert!(matches!(read_selected_field(&output, &limits), Err(CoreError::ResourceLimit(_))));
}

#[test]
fn geotiff_overwrite_replaces_the_complete_three_file_group() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("field.tif");
    let first = write_field(&fixture_field(), &output, &Limits::default()).unwrap();
    let mut changed = fixture_field();
    changed.values[0] = 20.0;
    write_field(&changed, &output, &Limits::default()).unwrap();
    let mut data = Decoder::new(File::open(&first[0]).unwrap()).unwrap();
    assert!(matches!(data.read_image().unwrap(), DecodingResult::F32(values)
        if values[2] == 20.0));
    assert!(first.iter().all(|path| path.is_file()));
}
