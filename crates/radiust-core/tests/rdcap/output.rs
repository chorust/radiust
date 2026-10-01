//! RDCAP scientific output and independent-readback contract tests.

use super::support;
use radiust_core::config::CoreConfig;
use radiust_core::download::DownloadStatus;
use radiust_core::engine::Engine;
use radiust_core::limits::Limits;
use radiust_core::model::RadarField;
use radiust_core::output::{geotiff, netcdf, png, zarr};
use radiust_core::source::SourceRegistry;
use serde_json::Value;
use std::sync::Arc;

fn fixture_key(station_id: &str) -> String {
    let manifest: Value = serde_json::from_str(support::FIXTURE_MANIFEST).unwrap();
    manifest["stations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|station| station["station_id"] == station_id)
        .unwrap()["selected_key_epoch_ms"]
        .as_u64()
        .unwrap()
        .to_string()
}

fn assert_roundtrip(expected: &RadarField, actual: &RadarField, label: &str) {
    assert_eq!(expected.shape, actual.shape, "{label} shape");
    assert_eq!(expected.quality, actual.quality, "{label} quality");
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(&expected.valid_time).unwrap().timestamp_micros(),
        chrono::DateTime::parse_from_rfc3339(&actual.valid_time).unwrap().timestamp_micros(),
        "{label} time"
    );
    assert_eq!(expected.grid.crs, actual.grid.crs, "{label} CRS");
    assert_eq!(expected.values.len(), actual.values.len(), "{label} value count");
    for (expected, actual) in expected.values.iter().zip(&actual.values) {
        if expected.is_nan() {
            assert!(actual.is_nan(), "{label}: expected NaN, got {actual}");
        } else {
            assert!((expected - actual).abs() <= 1e-5, "{label}: {expected} != {actual}");
        }
    }
    for (expected, actual) in expected.grid.x.iter().zip(&actual.grid.x) {
        assert!((expected - actual).abs() <= 1e-9, "{label} x coordinate");
    }
    for (expected, actual) in expected.grid.y.iter().zip(&actual.grid.y) {
        assert!((expected - actual).abs() <= 1e-9, "{label} y coordinate");
    }
}

#[tokio::test]
async fn reconstructed_three_country_fields_roundtrip_through_all_native_outputs() {
    let engine = Engine::new(CoreConfig::default(), SourceRegistry::default()).unwrap();
    let limits = Limits::default();
    let temp = tempfile::tempdir().unwrap();
    let output_root = std::env::var_os("RADIUST_RDCAP_FORMATS_OUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| temp.path().to_path_buf());
    std::fs::create_dir_all(&output_root).unwrap();

    for station_id in ["TWN/RCHL", "JPN/ISHI", "PHL/SUBI"] {
        let station_root = output_root.join(station_id.replace('/', "-"));
        std::fs::create_dir_all(&station_root).unwrap();
        let raw = support::fixture_raw_frame(
            station_id,
            &fixture_key(station_id),
            &support::reconstructed_file_response(station_id),
            temp.path(),
        );
        let frame = raw.frame.clone();
        let expected = engine.decode_science(Arc::new(raw)).await.unwrap();
        assert!(expected.quality.contains(&65), "{station_id} annotation must survive decode");

        let preview = png::preview_field(&expected, &limits).unwrap();
        let png_path = station_root.join("reflectivity.png");
        png::write_png(&expected, &png_path, &serde_json::json!({})).unwrap();
        let encoded = image::open(&png_path).unwrap().to_rgba8().into_raw();
        assert_eq!(encoded, preview.rgba, "{station_id} preview and PNG");
        let render: Value = serde_json::from_slice(
            &std::fs::read(station_root.join("reflectivity.render.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(render["palette"]["id"], "rdcap-reflectivity-v1");
        assert_eq!(render["frame_identity"]["station"], station_id);
        assert_eq!(render["annotation_rule"]["quality"], 65);
        assert_eq!(
            render["palette"]["evidence"]["reference"],
            "docs/rdcap-single-station-analysis.md"
        );

        let netcdf_path = station_root.join("reflectivity.nc");
        netcdf::write_field(&expected, &netcdf_path, &limits).unwrap();
        let netcdf_field =
            netcdf::read_selected_field(&netcdf_path, "reflectivity", None, &limits).unwrap();
        assert_roundtrip(&expected, &netcdf_field, "NetCDF");

        let geotiff_path = station_root.join("reflectivity.tif");
        geotiff::write_field(&expected, &geotiff_path, &limits).unwrap();
        let geotiff_field = geotiff::read_selected_field(&geotiff_path, &limits).unwrap();
        assert_roundtrip(&expected, &geotiff_field, "GeoTIFF");
        let geotiff_metadata: Value = serde_json::from_slice(
            &std::fs::read(station_root.join("reflectivity_provenance.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            geotiff_metadata["quality_flag_masks"],
            serde_json::json!([1, 2, 4, 8, 16, 32, 64])
        );
        assert!(
            geotiff_metadata["quality_flag_meanings"]
                .as_str()
                .unwrap()
                .ends_with("source_annotation")
        );

        let zarr_path = station_root.join("reflectivity.zarr");
        zarr::write_field(&expected, &zarr_path, &limits).unwrap();
        let zarr_field = zarr::read_selected_field(&zarr_path, None, &limits).unwrap();
        assert_roundtrip(&expected, &zarr_field, "Zarr");
        let zarr_quality: Value =
            serde_json::from_slice(&std::fs::read(zarr_path.join("quality/.zattrs")).unwrap())
                .unwrap();
        assert_eq!(zarr_quality["flag_masks"], serde_json::json!([1, 2, 4, 8, 16, 32, 64]));
        assert!(zarr_quality["flag_meanings"].as_str().unwrap().ends_with("source_annotation"));

        let idempotent_root = station_root.join("idempotent");
        let first = engine
            .write_science_to(
                frame.clone(),
                expected.clone(),
                idempotent_root.clone(),
                "netcdf",
                false,
                serde_json::json!({}),
                None,
            )
            .await
            .unwrap();
        let second = engine
            .write_science_to(
                frame,
                expected,
                idempotent_root,
                "netcdf",
                false,
                serde_json::json!({}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(first.items[0].status, DownloadStatus::Written);
        assert_eq!(second.items[0].status, DownloadStatus::Skipped);
        let output_path = std::path::PathBuf::from(first.items[0].output_uri.as_ref().unwrap());
        let manifest: Value = serde_json::from_slice(
            &std::fs::read(output_path.with_file_name("decoded.nc.manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["processing_spec"]["decoder_version"], "rdcap-csr-v1");
        assert_eq!(
            manifest["processing_spec"]["resource_version"],
            "rdcap-reflectivity-v1+rdcap-annotation-v1"
        );
    }

    let all_missing_root = output_root.join("all-missing");
    std::fs::create_dir_all(&all_missing_root).unwrap();
    let all_missing = RadarField {
        name: "reflectivity".into(),
        values: vec![f32::NAN; 4],
        shape: vec![2, 2],
        quality: vec![1; 4],
        units: Some("dBZ".into()),
        valid_time: "2026-10-01T00:00:00Z".into(),
        grid: radiust_core::model::Grid {
            shape: vec![2, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![120.0, 121.0],
            y: vec![24.0, 23.0],
            affine: None,
        },
        provenance: vec![
            "source=rdcap".into(),
            "product=reflectivity".into(),
            "station=TWN/RCHL".into(),
            "frame_logical_id=offline-all-missing".into(),
            "raw_sha256=0000000000000000".into(),
        ],
    };
    let all_missing_preview = png::preview_field(&all_missing, &limits).unwrap();
    assert!(all_missing_preview.rgba.chunks_exact(4).all(|pixel| pixel[3] == 0));
    png::write_png(&all_missing, all_missing_root.join("all-missing.png"), &serde_json::json!({}))
        .unwrap();
    let netcdf_path = all_missing_root.join("all-missing.nc");
    netcdf::write_field(&all_missing, &netcdf_path, &limits).unwrap();
    let netcdf_read =
        netcdf::read_selected_field(&netcdf_path, "reflectivity", None, &limits).unwrap();
    assert_roundtrip(&all_missing, &netcdf_read, "all-missing NetCDF");
    let geotiff_path = all_missing_root.join("all-missing.tif");
    geotiff::write_field(&all_missing, &geotiff_path, &limits).unwrap();
    let geotiff_read = geotiff::read_selected_field(&geotiff_path, &limits).unwrap();
    assert_roundtrip(&all_missing, &geotiff_read, "all-missing GeoTIFF");
    let zarr_path = all_missing_root.join("all-missing.zarr");
    zarr::write_field(&all_missing, &zarr_path, &limits).unwrap();
    let zarr_read = zarr::read_selected_field(&zarr_path, None, &limits).unwrap();
    assert_roundtrip(&all_missing, &zarr_read, "all-missing Zarr");
}

#[test]
fn zarr_and_geotiff_readers_accept_legacy_six_flag_metadata() {
    let limits = Limits::default();
    let temp = tempfile::tempdir().unwrap();
    let field = RadarField {
        name: "reflectivity".into(),
        values: vec![f32::NAN, 0.0, 10.0, 20.0],
        shape: vec![2, 2],
        quality: vec![1, 0, 0, 0],
        units: Some("dBZ".into()),
        valid_time: "2026-10-01T00:00:00Z".into(),
        grid: radiust_core::model::Grid {
            shape: vec![2, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![120.0, 121.0],
            y: vec![24.0, 23.0],
            affine: None,
        },
        provenance: vec!["source=test".into()],
    };

    let zarr_path = temp.path().join("legacy.zarr");
    zarr::write_field(&field, &zarr_path, &limits).unwrap();
    let quality_attrs_path = zarr_path.join("quality/.zattrs");
    let mut quality_attrs: Value =
        serde_json::from_slice(&std::fs::read(&quality_attrs_path).unwrap()).unwrap();
    quality_attrs["flag_masks"] = serde_json::json!([1, 2, 4, 8, 16, 32]);
    quality_attrs["flag_meanings"] = serde_json::json!(
        "missing outside_coverage unknown_color recovered interpolated below_detection"
    );
    std::fs::write(&quality_attrs_path, serde_json::to_vec(&quality_attrs).unwrap()).unwrap();
    let consolidated_path = zarr_path.join(".zmetadata");
    let mut consolidated: Value =
        serde_json::from_slice(&std::fs::read(&consolidated_path).unwrap()).unwrap();
    consolidated["metadata"]["quality/.zattrs"] = quality_attrs;
    std::fs::write(&consolidated_path, serde_json::to_vec(&consolidated).unwrap()).unwrap();
    let zarr_read = zarr::read_selected_field(&zarr_path, None, &limits).unwrap();
    assert_roundtrip(&field, &zarr_read, "legacy Zarr");

    let geotiff_path = temp.path().join("legacy.tif");
    geotiff::write_field(&field, &geotiff_path, &limits).unwrap();
    let provenance_path = temp.path().join("legacy_provenance.json");
    let mut provenance: Value =
        serde_json::from_slice(&std::fs::read(&provenance_path).unwrap()).unwrap();
    provenance.as_object_mut().unwrap().remove("quality_flag_masks");
    provenance.as_object_mut().unwrap().remove("quality_flag_meanings");
    std::fs::write(&provenance_path, serde_json::to_vec(&provenance).unwrap()).unwrap();
    let geotiff_read = geotiff::read_selected_field(&geotiff_path, &limits).unwrap();
    assert_roundtrip(&field, &geotiff_read, "legacy GeoTIFF");
}
