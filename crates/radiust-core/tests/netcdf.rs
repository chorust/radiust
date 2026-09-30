use radiust_core::limits::Limits;
use radiust_core::model::{Grid, RadarField};
use radiust_core::output::netcdf::{read_selected_field, write_field};

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
            affine: Some([119.75, 0.5, 0.0, 23.75, 0.0, -0.5]),
        },
        provenance: vec!["fixture-source".into(), "decode-v1".into()],
    }
}

fn write_multitime_file(path: &std::path::Path, use_string_data: bool) {
    let mut file = netcdf::create(path).unwrap();
    file.add_dimension("time", 2).unwrap();
    file.add_dimension("y", 2).unwrap();
    file.add_dimension("x", 2).unwrap();
    {
        let mut time = file.add_variable::<i64>("time", &["time"]).unwrap();
        time.put_attribute("units", "seconds since 1970-01-01 00:00:00 UTC").unwrap();
        time.put_values(&[1_790_380_800_i64, 1_790_388_000_i64], ..).unwrap();
    }
    if use_string_data {
        // If the reader loads data before resolving time ambiguity, numeric
        // conversion fails instead of returning the required ambiguity error.
        file.add_string_variable("reflectivity", &["time", "y", "x"]).unwrap();
    } else {
        let mut data = file.add_variable::<f32>("reflectivity", &["time", "y", "x"]).unwrap();
        data.put_attribute("units", "dBZ").unwrap();
        data.put_values(&[1.0_f32, 2.0, 3.0, 4.0, 11.0, 12.0, 13.0, 14.0], ..).unwrap();
        let mut quality = file.add_variable::<u16>("quality", &["time", "y", "x"]).unwrap();
        quality.put_values(&[0_u16, 1, 0, 0, 4, 0, 8, 0], ..).unwrap();
    }
    file.close().unwrap();
}

#[test]
fn netcdf_writer_roundtrips_field_values_quality_and_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("field.nc");
    let expected = fixture_field();

    write_field(&expected, &path, &Limits::default()).unwrap();
    let actual = read_selected_field(&path, "reflectivity", None, &Limits::default()).unwrap();

    assert_eq!(actual.name, expected.name);
    assert_eq!(actual.shape, expected.shape);
    assert_eq!(actual.quality, expected.quality);
    assert_eq!(actual.units, expected.units);
    assert_eq!(actual.valid_time, expected.valid_time);
    assert_eq!(actual.grid, expected.grid);
    assert_eq!(actual.provenance, expected.provenance);
    assert_eq!(actual.values[0], expected.values[0]);
    assert!(actual.values[1].is_nan());
    assert_eq!(&actual.values[2..], &expected.values[2..]);

    let written = netcdf::open(path).unwrap();
    let time = written.variable("time").unwrap();
    assert_eq!(time.dimensions().len(), 0);
    assert_eq!(time.get_value::<f64, _>(()).unwrap(), 1_790_384_523.0);
}

#[test]
fn multitime_netcdf_reads_only_the_requested_slice() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("series.nc");
    write_multitime_file(&path, false);

    let field = read_selected_field(
        &path,
        "reflectivity",
        Some("2026-09-26T02:00:00Z"),
        &Limits::default(),
    )
    .unwrap();

    assert_eq!(field.values, vec![11.0, 12.0, 13.0, 14.0]);
    assert_eq!(field.quality, vec![4, 0, 8, 0]);
    assert_eq!(field.valid_time, "2026-09-26T02:00:00Z");
}

#[test]
fn netcdf_reader_accepts_single_string_cf_attributes_from_xarray_style_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("xarray-style.nc");
    let mut file = netcdf::create(&path).unwrap();
    file.add_dimension("y", 2).unwrap();
    file.add_dimension("x", 2).unwrap();
    {
        let mut time = file.add_variable::<f64>("time", &[]).unwrap();
        time.put_attribute("units", vec!["seconds since 1970-01-01 00:00:00 UTC".to_owned()])
            .unwrap();
        time.put_attribute("calendar", vec!["proleptic_gregorian".to_owned()]).unwrap();
        time.put_value(1_790_384_523.0, ()).unwrap();
    }
    {
        let mut longitude = file.add_variable::<f64>("longitude", &["x"]).unwrap();
        longitude.put_values(&[120.0, 120.5], ..).unwrap();
        let mut latitude = file.add_variable::<f64>("latitude", &["y"]).unwrap();
        latitude.put_values(&[23.0, 23.5], ..).unwrap();
        let mut field = file.add_variable::<f32>("reflectivity", &["y", "x"]).unwrap();
        field.put_values(&[1.0, 2.0, 3.0, 4.0], ..).unwrap();
    }
    file.close().unwrap();

    let field = read_selected_field(&path, "reflectivity", None, &Limits::default()).unwrap();
    assert_eq!(field.valid_time, "2026-09-26T01:02:03Z");
    assert_eq!(field.values, vec![1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn multitime_ambiguity_is_reported_before_field_data_is_loaded() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ambiguous.nc");
    write_multitime_file(&path, true);

    let error = read_selected_field(&path, "reflectivity", None, &Limits::default()).unwrap_err();
    assert!(error.to_string().contains("time selection is ambiguous"));
}

#[test]
fn netcdf_time_metadata_preserves_subsecond_field_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("subsecond.nc");
    let mut field = fixture_field();
    field.valid_time = "2026-09-26T01:02:03.123456789Z".into();

    write_field(&field, &path, &Limits::default()).unwrap();
    let actual = read_selected_field(&path, "reflectivity", None, &Limits::default()).unwrap();

    assert_eq!(actual.valid_time, field.valid_time);
}
