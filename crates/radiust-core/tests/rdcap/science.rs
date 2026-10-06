//! RDCAP bounded CSR decode, quality, geometry, and palette contract tests.

use super::support;
use radiust_core::config::CoreConfig;
use radiust_core::engine::{Engine, EngineError};
use radiust_core::errors::{CoreError, ProviderError};
use radiust_core::limits::Limits;
use radiust_core::model::{RadarField, RawFrame};
use radiust_core::raster::RasterResultData;
use radiust_core::source::SourceRegistry;
use serde_json::Value;
use std::sync::Arc;

fn fixture_metadata(station_id: &str) -> Value {
    let manifest: Value = serde_json::from_str(support::FIXTURE_MANIFEST).unwrap();
    manifest["stations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|station| station["station_id"] == station_id)
        .unwrap()
        .clone()
}

fn raw_fixture(station_id: &str, root: &std::path::Path) -> RawFrame {
    let metadata = fixture_metadata(station_id);
    let key = metadata["selected_key_epoch_ms"].as_u64().unwrap().to_string();
    support::fixture_raw_frame(
        station_id,
        &key,
        &support::reconstructed_file_response(station_id),
        root,
    )
}

async fn decode(engine: &Engine, raw: RawFrame) -> RadarField {
    engine.decode_science(Arc::new(raw)).await.unwrap()
}

#[tokio::test]
async fn three_country_csr_fixtures_keep_native_geometry_and_markers_out_of_science_values() {
    let engine = Engine::new(CoreConfig::default(), SourceRegistry::default()).unwrap();
    let root = tempfile::tempdir().unwrap();
    for station_id in ["TWN/RCHL", "JPN/ISHI", "PHL/SUBI"] {
        let metadata = fixture_metadata(station_id);
        let expected_grid: Value = serde_json::from_slice(
            &std::fs::read(
                support::repository_root()
                    .join(format!("tests/fixtures/sources/rdcap/{station_id}/grid.json")),
            )
            .unwrap(),
        )
        .unwrap();
        let field = decode(&engine, raw_fixture(station_id, root.path())).await;
        let width = expected_grid["width"].as_u64().unwrap() as usize;
        let height = expected_grid["height"].as_u64().unwrap() as usize;
        let annotation_cells = expected_grid["annotation_cells"].as_u64().unwrap() as usize;

        assert_eq!(field.shape, [height, width], "{station_id}");
        assert_eq!(field.grid.crs.as_deref(), Some("EPSG:4326"));
        assert_eq!(field.units.as_deref(), Some("dBZ"));
        let key = metadata["selected_key_epoch_ms"].as_u64().unwrap() as i64;
        let expected_time = chrono::DateTime::from_timestamp_millis(key)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        assert_eq!(field.valid_time, expected_time);
        assert_eq!(
            field.quality.iter().filter(|quality| **quality == 65).count(),
            annotation_cells,
            "{station_id} annotation cells"
        );
        if station_id == "TWN/RCHL" {
            assert!(field.values.iter().any(|value| *value == -1.5));
        }
        assert!(
            field
                .values
                .iter()
                .zip(&field.quality)
                .any(|(value, quality)| value.is_finite() && *value < 5.0 && *quality == 0)
        );

        let expected_affine = expected_grid["geotransform"].as_array().unwrap();
        for (actual, expected) in field.grid.affine.unwrap().iter().zip(expected_affine) {
            assert!((actual - expected.as_f64().unwrap()).abs() < 1e-10, "{station_id}");
        }
        let bounds = expected_grid["bounds"].as_array().unwrap();
        let [west, dx, _, north, _, dy] = field.grid.affine.unwrap();
        assert!((field.grid.x[0] - (west + dx / 2.0)).abs() < 1e-10);
        assert!((field.grid.x[width - 1] - (west + (width as f64 - 0.5) * dx)).abs() < 1e-10);
        assert!((field.grid.y[0] - (north + dy / 2.0)).abs() < 1e-10);
        assert!((field.grid.y[height - 1] - (north + (height as f64 - 0.5) * dy)).abs() < 1e-10);
        assert!((field.grid.x[0] - bounds[0].as_f64().unwrap() - dx / 2.0).abs() < 1e-10);
        assert!(field.grid.y[0] > field.grid.y[height - 1]);
        assert!(field.provenance.iter().any(|entry| entry.contains("confidence=inferred")));
        assert!(field.provenance.iter().any(|entry| entry.contains("raw_sha256=")));

        let result =
            engine.decode_dbz(Arc::new(raw_fixture(station_id, root.path()))).await.unwrap();
        let RasterResultData::Native(native) = result.data else {
            panic!("RDCAP dBZ was routed through gray conversion");
        };
        assert_eq!(native.shape, field.shape);
        assert_eq!(native.quality, field.quality);
        assert_eq!(native.grid, field.grid);
        assert_eq!(native.units, field.units);
        assert_eq!(native.valid_time, field.valid_time);
        assert!(native.values.iter().zip(&field.values).all(|(actual, expected)| {
            (actual.is_nan() && expected.is_nan()) || actual == expected
        }));
        assert!(result.processing.encoding_basis.is_none());
        assert_eq!(result.mode_info.actual.as_deref(), Some("dbz"));
    }
}

#[tokio::test]
async fn rchl_eight_reference_points_match_and_missing_or_annotation_values_stay_masked() {
    let engine = Engine::new(CoreConfig::default(), SourceRegistry::default()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let field = decode(&engine, raw_fixture("TWN/RCHL", root.path())).await;
    let reference: Value = serde_json::from_slice(
        &std::fs::read(
            support::repository_root()
                .join("tests/fixtures/sources/rdcap/TWN/RCHL/comparison.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let width = field.shape[1];
    for point in reference["points"].as_array().unwrap() {
        let longitude = point["lng"].as_f64().unwrap();
        let latitude = point["lat"].as_f64().unwrap();
        let column = field
            .grid
            .x
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                (*left - longitude).abs().total_cmp(&(*right - longitude).abs())
            })
            .unwrap()
            .0;
        let row = field
            .grid
            .y
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                (*left - latitude).abs().total_cmp(&(*right - latitude).abs())
            })
            .unwrap()
            .0;
        let index = row * width + column;
        if let Some(expected) = point["expected"].as_f64() {
            assert_eq!(field.quality[index], 0);
            assert!((f64::from(field.values[index]) - expected).abs() <= 0.01);
        } else {
            assert!(field.values[index].is_nan());
            assert_ne!(field.quality[index], 0);
        }
    }
    assert!(field.quality.contains(&65));
}

#[tokio::test]
async fn unknown_transform_corrupt_csr_and_pixel_budget_fail_with_typed_errors() {
    let root = tempfile::tempdir().unwrap();
    let response = support::reconstructed_file_response("TWN/RCHL");
    let mut content: String = serde_json::from_slice(&response).unwrap();
    let unsupported = content.replacen("EPSG:4326", "EPSG:3857", 1);
    let unsupported_raw = support::fixture_raw_frame(
        "TWN/RCHL",
        "1790834708000",
        &serde_json::to_vec(&unsupported).unwrap(),
        root.path(),
    );
    let engine = Engine::new(CoreConfig::default(), SourceRegistry::default()).unwrap();
    assert!(matches!(
        engine.decode_science(Arc::new(unsupported_raw)).await,
        Err(EngineError::Core(CoreError::Provider(ProviderError::DecodeUnverified)))
    ));

    let lines = content.lines().collect::<Vec<_>>();
    let mut corrupt_lines = lines.iter().map(|line| (*line).to_owned()).collect::<Vec<_>>();
    let row_ptr = corrupt_lines[5].strip_prefix("rowPtr:").unwrap();
    corrupt_lines[5] = format!("rowPtr:{}", row_ptr.rsplit_once(',').unwrap().0);
    content = corrupt_lines.join("\n");
    let corrupt_raw = support::fixture_raw_frame(
        "TWN/RCHL",
        "1790834708000",
        &serde_json::to_vec(&content).unwrap(),
        root.path(),
    );
    assert!(matches!(
        engine.decode_science(Arc::new(corrupt_raw)).await,
        Err(EngineError::Core(CoreError::Provider(ProviderError::InvalidGrid)))
    ));

    let limited_raw = raw_fixture("TWN/RCHL", root.path());
    let mut limited_config = CoreConfig::default();
    limited_config.runtime.max_pixels = 16;
    let limited_engine = Engine::new(limited_config, SourceRegistry::default()).unwrap();
    assert!(matches!(
        limited_engine.decode_science(Arc::new(limited_raw)).await,
        Err(EngineError::Core(CoreError::ResourceLimit(_)))
    ));
}

#[tokio::test]
async fn csr_rejects_unverified_header_legend_and_invalid_sparse_layouts() {
    let root = tempfile::tempdir().unwrap();
    let response = support::reconstructed_file_response("TWN/RCHL");
    let content: String = serde_json::from_slice(&response).unwrap();
    let mut variants = Vec::new();

    variants.push(content.replacen(",T,", ",M,", 1));
    let mut unknown_legend = content.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut legend = unknown_legend[2].split(',').map(str::to_owned).collect::<Vec<_>>();
    legend[0] = "1".into();
    unknown_legend[2] = legend.join(",");
    variants.push(unknown_legend.join("\n"));

    let lines = content.lines().map(str::to_owned).collect::<Vec<_>>();
    let row_ptr = lines[5]
        .strip_prefix("rowPtr:")
        .unwrap()
        .split(',')
        .map(|value| value.parse::<usize>().unwrap())
        .collect::<Vec<_>>();
    let columns = lines[4]
        .strip_prefix("colIdx:")
        .unwrap()
        .split(',')
        .map(|value| value.parse::<usize>().unwrap())
        .collect::<Vec<_>>();
    let mut unordered = lines.clone();
    let row = row_ptr.windows(2).position(|pair| pair[1] - pair[0] >= 2).unwrap();
    let first = row_ptr[row];
    let second = first + 1;
    let mut changed_columns = columns;
    changed_columns.swap(first, second);
    unordered[4] = format!(
        "colIdx:{}",
        changed_columns.iter().map(usize::to_string).collect::<Vec<_>>().join(",")
    );
    variants.push(unordered.join("\n"));

    let mut out_of_range = lines.clone();
    out_of_range[6] = out_of_range[6].replacen("vals:", "vals:40000,", 1);
    variants.push(out_of_range.join("\n"));

    let mut overflow = lines;
    overflow[0] = overflow[0].replacen("901,901,", &format!("{},901,", usize::MAX), 1);
    variants.push(overflow.join("\n"));

    let engine = Engine::new(CoreConfig::default(), SourceRegistry::default()).unwrap();
    for variant in variants {
        let raw = support::fixture_raw_frame(
            "TWN/RCHL",
            "1790834708000",
            &serde_json::to_vec(&variant).unwrap(),
            root.path(),
        );
        let error = engine.decode_science(Arc::new(raw)).await.unwrap_err();
        assert!(matches!(
            error,
            EngineError::Core(CoreError::Provider(
                ProviderError::DecodeUnverified | ProviderError::InvalidGrid
            ))
        ));
    }
}

#[tokio::test]
async fn valid_zero_nnz_and_annotation_only_csr_are_science_fields() {
    let root = tempfile::tempdir().unwrap();
    let response = support::reconstructed_file_response("TWN/RCHL");
    let original: String = serde_json::from_slice(&response).unwrap();
    let legend = original.lines().nth(2).unwrap();
    let engine = Engine::new(CoreConfig::default(), SourceRegistry::default()).unwrap();

    let all_missing = format!(
        "2,2,T,140,40,141,39,int16,-999,-999,EPSG:4326\nlinearTransform(0.1,0)\n{legend}\nndv:0\ncolIdx:\nrowPtr:0,0,0\nvals:"
    );
    let missing_raw = support::fixture_raw_frame(
        "TWN/RCHL",
        "1790834708000",
        &serde_json::to_vec(&all_missing).unwrap(),
        root.path(),
    );
    let missing = engine.decode_science(Arc::new(missing_raw)).await.unwrap();
    assert!(missing.values.iter().all(|value| value.is_nan()));
    assert_eq!(missing.quality, [1, 1, 1, 1]);

    let annotation_only = format!(
        "2,2,T,140,40,141,39,int16,-999,-999,EPSG:4326\nlinearTransform(0.1,0)\n{legend}\nndv:4\ncolIdx:0,1,0,1\nrowPtr:0,2,4\nvals:9999,9999,9999,9999"
    );
    let annotation_raw = support::fixture_raw_frame(
        "TWN/RCHL",
        "1790834708000",
        &serde_json::to_vec(&annotation_only).unwrap(),
        root.path(),
    );
    let annotation = engine.decode_science(Arc::new(annotation_raw)).await.unwrap();
    assert!(annotation.values.iter().all(|value| value.is_nan()));
    assert_eq!(annotation.quality, [65, 65, 65, 65]);
    assert!(annotation.provenance.iter().any(|entry| entry.contains("cells=4")));
}

#[test]
fn sparse_buffers_are_included_in_the_decode_byte_budget() {
    let root = tempfile::tempdir().unwrap();
    let response = support::reconstructed_file_response("TWN/RCHL");
    let original: String = serde_json::from_slice(&response).unwrap();
    let legend = original.lines().nth(2).unwrap();
    for (ndv, columns, row_ptr, values) in
        [(0, "", "0,0,0", ""), (4, "0,1,0,1", "0,2,4", "10,20,30,40")]
    {
        let content = format!(
            "2,2,T,140,40,141,39,int16,-999,-999,EPSG:4326\nlinearTransform(0.1,0)\n{legend}\nndv:{ndv}\ncolIdx:{columns}\nrowPtr:{row_ptr}\nvals:{values}"
        );
        let raw = support::fixture_raw_frame(
            "TWN/RCHL",
            "1790834708000",
            &serde_json::to_vec(&content).unwrap(),
            root.path(),
        );
        let required_bytes = 4
            * (std::mem::size_of::<i16>()
                + std::mem::size_of::<f32>()
                + std::mem::size_of::<u16>())
            + ndv * (std::mem::size_of::<usize>() + std::mem::size_of::<i16>())
            + 3 * std::mem::size_of::<usize>();
        let mut limits = Limits { max_temp_bytes: required_bytes as u64 - 1, ..Limits::default() };
        assert!(matches!(
            radiust_core::science::decode_rdcap(&raw, &limits),
            Err(CoreError::ResourceLimit(_))
        ));
        limits.max_temp_bytes += 1;
        assert!(radiust_core::science::decode_rdcap(&raw, &limits).is_ok());
    }
}
