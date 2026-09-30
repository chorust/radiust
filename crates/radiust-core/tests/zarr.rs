use radiust_core::errors::CoreError;
use radiust_core::limits::Limits;
use radiust_core::model::{Grid, RadarDataset, RadarField};
use radiust_core::output::zarr::{read_selected_field, write_dataset, write_field};
use serde_json::Value;

fn fixture_field() -> RadarField {
    RadarField {
        name: "reflectivity".into(),
        values: vec![12.5, f32::NAN, -3.0, 0.25],
        shape: vec![2, 2],
        quality: vec![0, 1, 4, 0],
        units: Some("dBZ".into()),
        valid_time: "2026-09-26T01:02:03.123456789Z".into(),
        grid: Grid {
            shape: vec![2, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![120.0, 120.5],
            y: vec![23.0, 23.5],
            affine: Some([119.75, 0.5, 0.0, 23.75, 0.0, -0.5]),
        },
        provenance: vec!["source=fixture".into()],
    }
}

fn consolidated(path: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(path.join(".zmetadata")).unwrap()).unwrap()
}

#[test]
fn zarr_v2_writer_preserves_metadata_dtype_chunks_and_codec() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("field.zarr");
    write_field(&fixture_field(), &path, &Limits::default()).unwrap();

    let consolidated = consolidated(&path);
    assert_eq!(consolidated["zarr_consolidated_format"], 1);
    let metadata = &consolidated["metadata"];
    assert_eq!(metadata[".zgroup"]["zarr_format"], 2);
    assert_eq!(metadata["reflectivity/.zarray"]["dtype"], "<f4");
    assert_eq!(metadata["reflectivity/.zarray"]["chunks"], serde_json::json!([2, 2]));
    assert!(metadata["reflectivity/.zarray"].get("node_type").is_none());
    assert!(metadata["reflectivity/.zarray"].get("dimension_separator").is_none());
    assert_eq!(metadata["quality/.zarray"]["dtype"], "<u2");
    assert_eq!(metadata["quality/.zarray"]["chunks"], serde_json::json!([2, 2]));
    for name in ["reflectivity", "quality", "longitude", "latitude"] {
        assert_eq!(
            metadata[format!("{name}/.zarray")]["compressor"],
            serde_json::json!({
                "id": "blosc",
                "cname": "lz4",
                "clevel": 5,
                "shuffle": 1,
                "blocksize": 0
            })
        );
    }
    assert_eq!(metadata["time/.zarray"]["compressor"], Value::Null);
    assert_eq!(metadata["crs/.zarray"]["compressor"], Value::Null);
    assert_eq!(
        metadata["reflectivity/.zattrs"]["_ARRAY_DIMENSIONS"],
        serde_json::json!(["latitude", "longitude"])
    );
    assert_eq!(metadata["reflectivity/.zattrs"]["coordinates"], "crs quality time");
    assert_eq!(metadata["time/.zattrs"]["calendar"], "proleptic_gregorian");
    assert_eq!(metadata["crs/.zattrs"]["inverse_flattening"], 298.257_223_563);
    assert_eq!(metadata[".zattrs"]["radiust_provenance"], serde_json::json!(["source=fixture"]));
}

#[test]
fn zarr_writer_caps_chunks_at_512_and_replaces_a_complete_store() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("field.zarr");
    let mut field = fixture_field();
    field.shape = vec![513, 2];
    field.grid.shape = field.shape.clone();
    field.grid.y = (0..513).map(f64::from).collect();
    field.values = (0..1026).map(|value| value as f32).collect();
    field.quality = vec![0; 1026];

    write_field(&field, &path, &Limits::default()).unwrap();
    let metadata = consolidated(&path)["metadata"].clone();
    assert_eq!(metadata["reflectivity/.zarray"]["chunks"], serde_json::json!([512, 2]));
    assert_eq!(metadata["latitude/.zarray"]["chunks"], serde_json::json!([513]));
    assert!(path.join("reflectivity/0.0").exists());
    assert!(path.join("reflectivity/1.0").exists());
    assert!(path.join("quality/0.0").exists());
    assert!(path.join("quality/1.0").exists());

    field.values[0] = 777.0;
    write_field(&field, &path, &Limits::default()).unwrap();
    let metadata = consolidated(&path);
    assert_eq!(metadata["metadata"]["reflectivity/.zarray"]["shape"], serde_json::json!([513, 2]));
    assert!(
        !std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains("radiust-backup"))
    );
}

#[test]
fn zarr_single_field_writer_reader_round_trip_preserves_field_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("field.zarr");
    let original = fixture_field();
    write_field(&original, &path, &Limits::default()).unwrap();
    assert!(!path.join("crs/0").exists());
    assert!(!path.join("time/0").exists());

    let restored = read_selected_field(&path, None, &Limits::default()).unwrap();
    assert_eq!(restored.name, original.name);
    assert_eq!(restored.values[0], original.values[0]);
    assert!(restored.values[1].is_nan());
    assert_eq!(restored.values[2..], original.values[2..]);
    assert_eq!(restored.shape, original.shape);
    assert_eq!(restored.quality, original.quality);
    assert_eq!(restored.units, original.units);
    assert_eq!(restored.valid_time, original.valid_time);
    assert_eq!(restored.grid, original.grid);
    assert_eq!(restored.provenance, original.provenance);
}

#[test]
fn zarr_multi_variable_reader_requires_and_honors_variable_selection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dataset.zarr");
    let reflectivity = fixture_field();
    let mut rain_rate = fixture_field();
    rain_rate.name = "rain_rate".into();
    rain_rate.values = vec![1.0, 2.0, 3.0, 4.0];
    rain_rate.quality = vec![0, 0, 2, 0];
    rain_rate.units = Some("mm h-1".into());
    rain_rate.provenance = vec!["source=rain-rate-fixture".into()];
    let dataset = RadarDataset {
        fields: vec![reflectivity, rain_rate.clone()],
        valid_time: rain_rate.valid_time.clone(),
        source: "fixture".into(),
    };
    write_dataset(&dataset, &path, &Limits::default()).unwrap();

    let ambiguous = read_selected_field(&path, None, &Limits::default()).unwrap_err();
    assert!(matches!(
        ambiguous,
        CoreError::Storage(message) if message.contains("ambiguous")
    ));
    let restored = read_selected_field(&path, Some("rain_rate"), &Limits::default()).unwrap();
    assert_eq!(restored.name, rain_rate.name);
    assert_eq!(restored.values, rain_rate.values);
    assert_eq!(restored.quality, rain_rate.quality);
    assert_eq!(restored.units, rain_rate.units);
    assert_eq!(restored.valid_time, rain_rate.valid_time);
    assert_eq!(restored.grid, rain_rate.grid);
    assert_eq!(restored.provenance, rain_rate.provenance);
}

#[test]
fn zarr_reader_rejects_missing_chunks_and_inconsistent_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let missing_data = directory.path().join("missing-data.zarr");
    write_field(&fixture_field(), &missing_data, &Limits::default()).unwrap();
    std::fs::remove_file(missing_data.join("reflectivity/0.0")).unwrap();
    assert!(read_selected_field(&missing_data, None, &Limits::default()).is_err());

    let missing_quality = directory.path().join("missing-quality.zarr");
    write_field(&fixture_field(), &missing_quality, &Limits::default()).unwrap();
    std::fs::remove_file(missing_quality.join("quality/0.0")).unwrap();
    assert!(read_selected_field(&missing_quality, None, &Limits::default()).is_err());

    let invalid_metadata = directory.path().join("invalid.zarr");
    write_field(&fixture_field(), &invalid_metadata, &Limits::default()).unwrap();
    let mut metadata = consolidated(&invalid_metadata);
    metadata["metadata"]["reflectivity/.zarray"]["dtype"] = Value::String(">f4".into());
    std::fs::write(invalid_metadata.join(".zmetadata"), serde_json::to_vec(&metadata).unwrap())
        .unwrap();
    assert!(read_selected_field(&invalid_metadata, None, &Limits::default()).is_err());
}

#[test]
fn zarr_writer_persists_all_fill_data_and_quality_chunks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fill-values.zarr");
    let mut field = fixture_field();
    field.values = vec![f32::NAN; 4];
    field.quality = vec![1; 4];
    write_field(&field, &path, &Limits::default()).unwrap();

    assert!(path.join("reflectivity/0.0").is_file());
    assert!(path.join("quality/0.0").is_file());
    let restored = read_selected_field(&path, None, &Limits::default()).unwrap();
    assert!(restored.values.iter().all(|value| value.is_nan()));
    assert_eq!(restored.quality, field.quality);
}

#[test]
fn zarr_reader_enforces_store_byte_and_pixel_limits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("field.zarr");
    write_field(&fixture_field(), &path, &Limits::default()).unwrap();

    let byte_limited = Limits { max_artifact_bytes: 1, ..Limits::default() };
    assert!(matches!(
        read_selected_field(&path, None, &byte_limited),
        Err(CoreError::ResourceLimit(_))
    ));
    let temp_limited = Limits { max_temp_bytes: 1, ..Limits::default() };
    assert!(matches!(
        read_selected_field(&path, None, &temp_limited),
        Err(CoreError::ResourceLimit(_))
    ));
    let frame_limited = Limits { max_frame_bytes: 1, ..Limits::default() };
    assert!(matches!(
        read_selected_field(&path, None, &frame_limited),
        Err(CoreError::ResourceLimit(_))
    ));
    let pixel_limited = Limits { max_pixels: 3, ..Limits::default() };
    assert!(matches!(
        read_selected_field(&path, None, &pixel_limited),
        Err(CoreError::ResourceLimit(_))
    ));
}

#[cfg(unix)]
#[test]
fn zarr_reader_rejects_symlinks_inside_the_store() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("field.zarr");
    write_field(&fixture_field(), &path, &Limits::default()).unwrap();
    let outside = directory.path().join("outside.chunk");
    std::fs::write(&outside, b"external data").unwrap();
    symlink(&outside, path.join("reflectivity/escape")).unwrap();

    assert!(read_selected_field(&path, None, &Limits::default()).is_err());
}

#[test]
fn rejected_zarr_write_leaves_the_previous_store_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("field.zarr");
    let mut field = fixture_field();
    write_field(&field, &path, &Limits::default()).unwrap();
    let previous = std::fs::read(path.join(".zmetadata")).unwrap();

    field.name = "../escape".into();
    assert!(write_field(&field, &path, &Limits::default()).is_err());
    assert_eq!(std::fs::read(path.join(".zmetadata")).unwrap(), previous);
}
