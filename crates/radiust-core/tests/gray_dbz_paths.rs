use radiust_core::dbz;
use radiust_core::gray::{self, GrayDecision};
use radiust_core::limits::Limits;
use radiust_core::model::{ArtifactReceipt, FrameRef, RawArtifact, RawFrame};
use radiust_core::raster::{
    EncodingBasis, QUALITY_MISSING, QUALITY_OUTSIDE_COVERAGE, QUALITY_UNKNOWN_COLOR,
    RasterResultData,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn manifest() -> Value {
    serde_json::from_slice(
        &std::fs::read(repo_root().join("tests/fixtures/legacy-display/manifest.json")).unwrap(),
    )
    .unwrap()
}

fn frame_ref(
    source: &str,
    product: &str,
    station: Option<String>,
    valid_time: String,
    revision: String,
) -> FrameRef {
    let mut frame = FrameRef {
        source: source.into(),
        product: product.into(),
        station,
        valid_time,
        base_time: None,
        logical_id: String::new(),
        revision: Some(revision),
        locator_version: "1".into(),
        locator: serde_json::json!({}),
    };
    frame.logical_id = radiust_core::identity::logical_id(&frame).unwrap();
    frame
}

fn raw_from_entry(entry: &Value) -> RawFrame {
    let root = repo_root();
    let input = &entry["inputs"][0];
    let relative = input["path"].as_str().unwrap();
    let bytes = std::fs::read(root.join("tests/fixtures/legacy-display").join(relative)).unwrap();
    let digest = hex::encode(Sha256::digest(&bytes));
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&bytes).unwrap();
    let path = file.into_temp_path();
    let receipt = ArtifactReceipt {
        name: input["name"].as_str().unwrap_or("fixture-image").into(),
        media_type: input["media_type"].as_str().unwrap().into(),
        size_bytes: bytes.len() as u64,
        sha256: digest,
    };
    let source = entry["source"].as_str().unwrap();
    let product = entry["product"].as_str().unwrap();
    let station = entry["frame"]["station"].as_str().map(str::to_owned);
    let valid_time =
        entry["frame"]["valid_time"].as_str().unwrap_or("2024-01-01T00:00:00Z").to_owned();
    let revision =
        entry["frame"]["revision"].as_str().unwrap_or("offline-fixture-revision").to_owned();
    RawFrame {
        frame: frame_ref(source, product, station, valid_time, revision),
        artifacts: vec![RawArtifact { receipt, path }],
        private_locator: None,
    }
}

fn raw_without_artifacts(source: &str, product: &str, station: Option<String>) -> RawFrame {
    RawFrame {
        frame: frame_ref(
            source,
            product,
            station,
            "2024-01-01T00:00:00Z".into(),
            "blocked-fixture-revision".into(),
        ),
        artifacts: Vec::new(),
        private_locator: None,
    }
}

fn rgb_codes(rgba: &[u8]) -> impl Iterator<Item = u8> + '_ {
    rgba.chunks_exact(4).map(|pixel| pixel[0])
}

#[test]
fn all_fifteen_passed_paths_match_gray_goldens_and_decode_with_rule_provenance() {
    let root = repo_root();
    let input_manifest = manifest();
    let entries = input_manifest["entries"].as_array().unwrap();
    let passed = entries.iter().filter(|entry| entry["status"] == "passed").collect::<Vec<_>>();
    assert_eq!(passed.len(), 15);

    for entry in passed {
        let path_id = entry["path_id"].as_str().unwrap();
        let raw = raw_from_entry(entry);
        let gray = match gray::decode_source_frame(&raw, &Limits::default()).unwrap() {
            GrayDecision::Applied(gray) => gray,
            GrayDecision::Unavailable { reason, .. } => panic!("{path_id} unavailable: {reason}"),
        };
        let EncodingBasis::VerifiedSourceRule { rule, .. } = &gray.encoding_basis else {
            panic!("{path_id} did not retain its verified rule identity");
        };
        assert_eq!(rule.path_id, path_id);
        assert_eq!(rule.config_hash, entry["config_hash"].as_str().unwrap());

        let baseline = image::open(
            root.join("tests/fixtures/legacy-display")
                .join(entry["baseline_path"].as_str().unwrap()),
        )
        .unwrap()
        .to_rgba8();
        assert_eq!((gray.width, gray.height), baseline.dimensions(), "{path_id}");
        assert_eq!(gray.rgba, baseline.into_raw(), "{path_id} gray pixels changed");
        assert_eq!(
            gray.frame_index,
            if entry["inputs"][0]["format"] == "GIF" { Some(0) } else { None }
        );

        let gray_copy = gray.clone();
        let result = dbz::decode_verified_gray_frame(gray, &Limits::default()).unwrap();
        let RasterResultData::Pixel(field) = result.data else {
            panic!("{path_id} did not return pixel-space dBZ");
        };
        assert_eq!(result.mode_info.actual.as_deref(), Some("dbz"), "{path_id}");
        assert_eq!(result.mode_info.units.as_deref(), Some("dBZ"), "{path_id}");
        assert_eq!(field.quality, gray_copy.quality, "{path_id}");
        assert_eq!(
            field.origin_quality.as_deref(),
            gray_copy.origin_quality.as_deref(),
            "{path_id}"
        );
        for (index, (code, value)) in rgb_codes(&gray_copy.rgba).zip(&field.values).enumerate() {
            let invalid = field.quality[index]
                & (QUALITY_MISSING | QUALITY_OUTSIDE_COVERAGE | QUALITY_UNKNOWN_COLOR)
                != 0;
            if invalid {
                assert!(value.is_nan(), "{path_id} pixel {index} lost its invalid mask");
            } else {
                assert_eq!(
                    *value,
                    f32::from(code.min(224)) * (5.0 / 16.0),
                    "{path_id} pixel {index}"
                );
            }
        }
        let gray_hash = hex::encode({
            let mut digest = Sha256::new();
            digest.update(b"radiust-verified-source-gray-rgba-v1\0");
            digest.update(gray_copy.width.to_le_bytes());
            digest.update(gray_copy.height.to_le_bytes());
            digest.update(&gray_copy.rgba);
            digest.finalize()
        });
        assert_eq!(
            result.processing.steps[1]["sha256"].as_str(),
            Some(gray_hash.as_str()),
            "{path_id} gray RGBA provenance",
        );
    }
}

#[test]
fn nz_clips_only_the_six_verified_gray_pixels_and_local_bare_file_stays_strict() {
    let entry = manifest()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path_id"] == "nz/rain")
        .unwrap()
        .clone();
    let raw = raw_from_entry(&entry);
    let gray = match gray::decode_source_frame(&raw, &Limits::default()).unwrap() {
        GrayDecision::Applied(gray) => gray,
        GrayDecision::Unavailable { reason, .. } => panic!("NZ gray unavailable: {reason}"),
    };
    let clipped_positions = rgb_codes(&gray.rgba)
        .enumerate()
        .filter_map(|(index, code)| {
            (code > 224).then_some((index % gray.width as usize, index / gray.width as usize, code))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        clipped_positions,
        vec![
            (274, 128, 228),
            (198, 153, 226),
            (154, 182, 227),
            (174, 212, 225),
            (257, 404, 225),
            (288, 405, 229),
        ]
    );
    assert_eq!(clipped_positions.iter().map(|(_, _, code)| *code).max(), Some(229));

    let result = dbz::decode_verified_gray_frame(gray.clone(), &Limits::default()).unwrap();
    let RasterResultData::Pixel(field) = result.data else { panic!("NZ result is not pixel dBZ") };
    let adjustment = field.encoding_adjustment.as_ref().unwrap();
    assert_eq!(adjustment.iter().filter(|value| **value == 1).count(), 6);
    assert_eq!(field.processing.clipped_pixel_count, Some(6));
    let valid_clipped = clipped_positions
        .iter()
        .filter(|(x, y, _)| {
            let index = y * gray.width as usize + x;
            field.quality[index]
                & (QUALITY_MISSING | QUALITY_OUTSIDE_COVERAGE | QUALITY_UNKNOWN_COLOR)
                == 0
        })
        .count() as u64;
    assert_eq!(field.processing.valid_clipped_pixel_count, Some(valid_clipped));
    for (x, y, code) in clipped_positions {
        let index = y * gray.width as usize + x;
        assert_eq!(gray.rgba[index * 4], code);
        assert_eq!(adjustment[index], 1);
        if field.quality[index]
            & (QUALITY_MISSING | QUALITY_OUTSIDE_COVERAGE | QUALITY_UNKNOWN_COLOR)
            != 0
        {
            assert!(field.values[index].is_nan());
        } else {
            assert_eq!(field.values[index], 70.0);
        }
    }

    let local_gray = repo_root().join("tests/fixtures/legacy-display/nz/old-gray.png");
    assert!(dbz::decode_gray_file(local_gray, None).is_err());
}

#[test]
fn th_animation_frame_zero_keeps_its_own_footer_time() {
    let entry = manifest()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path_id"] == "th/composite/kkn240Loop")
        .unwrap()
        .clone();
    let mut raw = raw_from_entry(&entry);
    let first = "2023-04-22T14:45:05Z";
    let latest = "2023-04-22T16:00:05Z";
    raw.frame.valid_time = latest.into();
    raw.frame.locator = serde_json::json!({
        "footer_observation_times": [first, latest],
        "frame_index": 0
    });
    raw.frame.logical_id = radiust_core::identity::logical_id(&raw.frame).unwrap();
    let gray = match gray::decode_source_frame(&raw, &Limits::default()).unwrap() {
        GrayDecision::Applied(gray) => gray,
        GrayDecision::Unavailable { reason, .. } => panic!("TH gray unavailable: {reason}"),
    };
    assert_eq!(gray.frame_index, Some(0));
    assert_eq!(gray.valid_time.as_deref(), Some(first));
}

#[test]
fn all_eight_blocked_paths_remain_unavailable_without_needing_a_raw_fallback() {
    let catalog: Value =
        serde_json::from_slice(include_bytes!("../../../python/radiust/resources/gray/index.json"))
            .unwrap();
    let blocked = catalog["paths"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["status"] != "passed")
        .collect::<Vec<_>>();
    assert_eq!(blocked.len(), 8);
    for entry in blocked {
        let path_id = entry["path_id"].as_str().unwrap();
        let parts = path_id.split('/').collect::<Vec<_>>();
        let station = (parts.len() == 3).then(|| parts[2].to_owned());
        let raw = raw_without_artifacts(
            entry["source"].as_str().unwrap(),
            entry["product"].as_str().unwrap(),
            station,
        );
        match gray::decode_source_frame(&raw, &Limits::default()).unwrap() {
            GrayDecision::Unavailable { reason, .. } => {
                assert!(!reason.trim().is_empty(), "{path_id} has no blocked reason");
            }
            GrayDecision::Applied(_) => panic!("blocked gray path {path_id} was applied"),
        }
    }
}
