use radiust_core::identity::logical_id;
use radiust_core::limits::Limits;
use radiust_core::model::{ArtifactReceipt, FrameRef, RawArtifact, RawFrame};
use radiust_core::output;
use radiust_core::raster::{
    ModeInfo, PixelDbzField, ProcessingRecord, RasterInput, RasterResult, RasterResultData,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn source_frame() -> FrameRef {
    let mut frame = FrameRef {
        source: "ca".into(),
        product: "rain".into(),
        station: Some("CASFT".into()),
        valid_time: "2026-10-03T01:02:03Z".into(),
        base_time: None,
        logical_id: String::new(),
        revision: Some("archive-revision".into()),
        locator_version: "ca-fixture-v1".into(),
        locator: json!({"path":"fixture/raw.gif"}),
    };
    frame.logical_id = logical_id(&frame).unwrap();
    frame
}

#[test]
fn source_dbz_and_exact_raw_receipt_commit_together() {
    let temp = tempfile::tempdir().unwrap();
    let raw_bytes = b"verified source image";
    let raw_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(raw_file.path(), raw_bytes).unwrap();
    let receipt = ArtifactReceipt {
        name: "original.gif".into(),
        media_type: "image/gif".into(),
        size_bytes: raw_bytes.len() as u64,
        sha256: hex::encode(Sha256::digest(raw_bytes)),
    };
    let frame = source_frame();
    let raw = RawFrame {
        frame: frame.clone(),
        artifacts: vec![RawArtifact { receipt, path: raw_file.into_temp_path() }],
        private_locator: None,
    };
    let input = RasterInput::Source {
        frame: frame.clone(),
        resolved_revision: "a".repeat(64),
        acquisition_receipt: raw.public_receipt(),
    };
    let result = RasterResult {
        input,
        data: RasterResultData::Pixel(Arc::new(PixelDbzField {
            variable: "reflectivity".into(),
            units: "dBZ".into(),
            width: 2,
            height: 1,
            values: vec![0.0, 70.0],
            quality: vec![0, 0],
            origin_quality: None,
            encoding_adjustment: None,
            alpha: None,
            valid_time: Some(frame.valid_time.clone()),
            geometry: None,
            processing: ProcessingRecord {
                schema_version: 1,
                method: "source_gray_dbz".into(),
                input_identity: json!({"kind":"source"}),
                encoding_basis: None,
                range_policy: Some("strict-v1".into()),
                decoder_version: Some("1".into()),
                quality_policy_version: Some("1".into()),
                formula: Some("min(gray,224)*5/16".into()),
                quantization_step: Some(0.3125),
                alpha_bit_depth: None,
                steps: vec![],
                limitations: vec!["geolocation unknown".into()],
                clipped_pixel_count: Some(0),
                valid_clipped_pixel_count: Some(0),
                upstream: None,
            },
        })),
        processing: ProcessingRecord {
            schema_version: 1,
            method: "source_gray_dbz".into(),
            input_identity: json!({"kind":"source"}),
            encoding_basis: None,
            range_policy: Some("strict-v1".into()),
            decoder_version: Some("1".into()),
            quality_policy_version: Some("1".into()),
            formula: Some("min(gray,224)*5/16".into()),
            quantization_step: Some(0.3125),
            alpha_bit_depth: None,
            steps: vec![],
            limitations: vec!["geolocation unknown".into()],
            clipped_pixel_count: Some(0),
            valid_clipped_pixel_count: Some(0),
            upstream: None,
        },
        mode_info: ModeInfo {
            requested: Some("dbz".into()),
            actual: Some("dbz".into()),
            variable: Some("reflectivity".into()),
            units: Some("dBZ".into()),
            method: Some("local_gray_dbz".into()),
            encoding: Some("gray-dbz-v1".into()),
            rule_version: None,
            range_policy: Some("strict-v1".into()),
            clipped_pixel_count: Some(0),
            valid_clipped_pixel_count: Some(0),
            time_status: Some("known".into()),
            geolocation: Some("unknown".into()),
            limitations: vec!["geolocation unknown".into()],
        },
    };

    let committed = output::write_raster_result_with_raw_cancellable(
        &result,
        &raw,
        temp.path().join("output"),
        "frames/source/reflectivity.nc",
        "netcdf",
        &json!({}),
        false,
        &Limits::default(),
        &CancellationToken::new(),
    )
    .unwrap();

    let root = temp.path().join("output/frames/source");
    assert!(committed.manifest.raw_complete);
    assert_eq!(std::fs::read(root.join("raw/original.gif")).unwrap(), raw_bytes);
    assert!(root.join("raw-manifest.json").is_file());
    assert!(root.join("reflectivity.nc.manifest.json").is_file());
}
