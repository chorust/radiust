use radiust_core::errors::CoreError;
use radiust_core::identity::logical_id;
use radiust_core::limits::Limits;
use radiust_core::model::{ArtifactReceipt, FrameRef, PreviewMode, RawArtifact, RawFrame};
use radiust_core::source::tiles::preview_source_tiles;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

const TILE_POSITIONS: [(&str, u32, u32); 4] = [
    ("tile-z1-x0-y0.png", 0, 0),
    ("tile-z1-x1-y0.png", 1, 0),
    ("tile-z1-x0-y1.png", 0, 1),
    ("tile-z1-x1-y1.png", 1, 1),
];

#[test]
fn rainviewer_fixture_composes_all_original_rgba_pixels_in_xyz_order() {
    assert_fixture_composition("rainviewer", 512, 1024);
}

#[test]
fn windy_fixture_composes_all_original_rgba_pixels_in_xyz_order() {
    assert_fixture_composition("windy", 256, 512);
}

#[test]
fn missing_duplicate_forged_and_unverified_tile_inputs_fail_closed() {
    let limits = Limits::default();

    let mut missing = fixture_raw("rainviewer");
    missing.artifacts.pop();
    assert_transport_error(
        preview_source_tiles(&missing, &limits),
        "raw tile preview is missing tile coordinates",
    );

    let mut duplicate = fixture_raw("rainviewer");
    let first_name = duplicate.artifacts[0].receipt.name.clone();
    duplicate.artifacts[1].receipt.name = first_name;
    assert_transport_error(
        preview_source_tiles(&duplicate, &limits),
        "raw tile preview contains duplicate tile coordinates",
    );

    let mut forged_locator = fixture_raw("rainviewer");
    forged_locator.frame.locator["tile_size"] = json!(256);
    forged_locator.frame.logical_id = logical_id(&forged_locator.frame).unwrap();
    assert_transport_error(
        preview_source_tiles(&forged_locator, &limits),
        "raw tile preview frame locator is invalid",
    );

    let mut tampered_receipt = fixture_raw("rainviewer");
    tampered_receipt.artifacts[0].receipt.sha256 = "0".repeat(64);
    assert_transport_error(
        preview_source_tiles(&tampered_receipt, &limits),
        "raw tile preview artifact digest does not match its receipt",
    );

    let mut unsupported = fixture_raw("rainviewer");
    unsupported.frame.source = "opensnow".into();
    unsupported.frame.logical_id = logical_id(&unsupported.frame).unwrap();
    assert_transport_error(
        preview_source_tiles(&unsupported, &limits),
        "raw tile preview layout is unsupported for this source",
    );
}

#[test]
fn wunderground_tile_layout_is_not_guessed_from_another_source_artifact() {
    let mut raw = fixture_raw("rainviewer");
    raw.frame.source = "wunderground".into();
    raw.frame.logical_id = logical_id(&raw.frame).unwrap();

    assert_transport_error(
        preview_source_tiles(&raw, &Limits::default()),
        "raw tile preview layout is unsupported for this source",
    );
}

#[test]
fn preview_obeys_pixel_and_temporary_memory_limits() {
    let raw = fixture_raw("rainviewer");
    let pixel_limited = Limits { max_pixels: 1_000, ..Limits::default() };
    assert_resource_error(
        preview_source_tiles(&raw, &pixel_limited),
        "raw tile preview exceeds the pixel limit",
    );

    let memory_limited = Limits { max_temp_bytes: 5 * 1024 * 1024, ..Limits::default() };
    assert_resource_error(
        preview_source_tiles(&raw, &memory_limited),
        "raw tile preview exceeds the temporary memory limit",
    );
}

#[test]
fn media_format_tile_dimensions_and_aggregate_bytes_are_checked() {
    let limits = Limits::default();

    let mut wrong_media_type = fixture_raw("rainviewer");
    wrong_media_type.artifacts[0].receipt.media_type = "image/webp".into();
    assert_transport_error(
        preview_source_tiles(&wrong_media_type, &limits),
        "raw tile preview artifacts must be PNG images",
    );

    let mut malformed_png = fixture_raw("rainviewer");
    replace_artifact_bytes(&mut malformed_png, 0, b"not a PNG image");
    assert_transport_error(
        preview_source_tiles(&malformed_png, &limits),
        "raw tile preview only accepts PNG images",
    );

    let mut wrong_dimensions = fixture_raw("rainviewer");
    let smaller_tile = std::fs::read(fixture_root("windy").join("raw/tile-z1-x0-y0.png")).unwrap();
    replace_artifact_bytes(&mut wrong_dimensions, 0, &smaller_tile);
    assert_transport_error(
        preview_source_tiles(&wrong_dimensions, &limits),
        "raw tile preview tile dimensions do not match the verified layout",
    );

    let raw = fixture_raw("rainviewer");
    let byte_limited = Limits { max_frame_bytes: 1, ..Limits::default() };
    assert_resource_error(
        preview_source_tiles(&raw, &byte_limited),
        "raw tile preview tiles exceed the byte limit",
    );
}

#[cfg(unix)]
#[test]
fn artifact_symlink_escape_is_rejected_with_a_stable_error() {
    use std::os::unix::fs::symlink;

    let mut raw = fixture_raw("rainviewer");
    let outside_dir = tempfile::tempdir().unwrap();
    let outside_path = outside_dir.path().join("outside.png");
    std::fs::write(&outside_path, b"outside the raw frame directory").unwrap();
    let artifact_path: &std::path::Path = raw.artifacts[0].path.as_ref();
    let link_path = artifact_path
        .parent()
        .unwrap()
        .join(format!("radiust-tile-preview-escape-{}.png", uuid::Uuid::new_v4()));
    symlink(outside_path, &link_path).unwrap();
    raw.artifacts[0].path = tempfile::TempPath::try_from_path(link_path).unwrap();

    assert_transport_error(
        preview_source_tiles(&raw, &Limits::default()),
        "raw tile preview artifact path is unsafe",
    );
}

fn assert_fixture_composition(source: &str, tile_size: u32, mosaic_size: u32) {
    let raw = fixture_raw(source);
    let total_bytes: u64 = raw.artifacts.iter().map(|artifact| artifact.receipt.size_bytes).sum();
    let preview = preview_source_tiles(&raw, &Limits::default()).unwrap();

    assert_eq!((preview.preview.width, preview.preview.height), (mosaic_size, mosaic_size));
    assert_eq!(preview.preview.mode, PreviewMode::Raw);
    assert_eq!(preview.preview.frame.as_ref().unwrap().logical_id, raw.frame.logical_id);
    assert_eq!(preview.format, "PNG");
    assert_eq!(preview.tile_count, 4);
    assert_eq!(preview.size_bytes, total_bytes);

    let fixture_root = fixture_root(source);
    let manifest = fixture_manifest(source);
    let artifacts = manifest["frames"][0]["artifacts"].as_array().unwrap();
    let mut expected = vec![0_u8; (mosaic_size * mosaic_size * 4) as usize];
    for (artifact, (name, x, y)) in artifacts.iter().zip(TILE_POSITIONS) {
        assert_eq!(artifact["name"].as_str(), Some(name));
        let path = fixture_root.join(artifact["path"].as_str().unwrap());
        let tile = image::open(path).unwrap().into_rgba8().into_raw();
        assert_eq!(tile.len(), (tile_size * tile_size * 4) as usize);
        let row_bytes = (tile_size * 4) as usize;
        for row in 0..tile_size as usize {
            let source_start = row * row_bytes;
            let destination_start = ((y * tile_size) as usize + row) * (mosaic_size * 4) as usize
                + (x * tile_size * 4) as usize;
            expected[destination_start..destination_start + row_bytes]
                .copy_from_slice(&tile[source_start..source_start + row_bytes]);
        }
    }
    assert_eq!(preview.preview.rgba, expected);
    assert_eq!(preview.sha256, hex::encode(Sha256::digest(&expected)));
}

fn fixture_raw(source: &str) -> RawFrame {
    let manifest = fixture_manifest(source);
    let frame_manifest = &manifest["frames"][0];
    let valid_time = frame_manifest["valid_time"].as_str().unwrap().to_owned();
    let parsed_time = chrono::DateTime::parse_from_rfc3339(&valid_time).unwrap().to_utc();
    let frame = if source == "rainviewer" {
        let discovery = &manifest["discovery"];
        FrameRef {
            source: source.into(),
            product: frame_manifest["product"].as_str().unwrap().into(),
            station: None,
            valid_time,
            base_time: None,
            logical_id: String::new(),
            revision: Some(frame_manifest["revision"].as_str().unwrap().into()),
            locator_version: "rainviewer-v2".into(),
            locator: json!({
                "api_url": discovery["api_url"],
                "host": discovery["host"],
                "path": discovery["frame_path"],
                "tile_size": 512,
                "zoom": 1,
                "color": 2,
                "options": "0_0",
            }),
        }
    } else {
        let path_time = parsed_time.format("%Y/%m/%d/%H%M").to_string();
        let max_time = parsed_time.format("%Y%m%d%H%M%S").to_string();
        let tiles = TILE_POSITIONS.map(|(name, x, y)| {
            let url = format!(
                "https://rdr.windy.com/radar2/composite/{path_time}/1/{x}/{y}/reflectivity.png?multichannel=true&maxt={max_time}"
            );
            (name, url)
        });
        let revision = format!("windy-{}", parsed_time.timestamp());
        FrameRef {
            source: source.into(),
            product: frame_manifest["product"].as_str().unwrap().into(),
            station: Some("global".into()),
            valid_time,
            base_time: None,
            logical_id: String::new(),
            revision: Some(revision.clone()),
            locator_version: "windy-legacy-v1".into(),
            locator: json!({
                "url": tiles[0].1,
                "name": tiles[0].0,
                "artifacts": tiles.iter().skip(1).map(|(name, url)| json!({
                    "url": url,
                    "name": name,
                    "role": "tile",
                    "media_type": "image/png",
                })).collect::<Vec<_>>(),
                "station": "global",
                "revision": revision,
            }),
        }
    };
    let mut frame = frame;
    frame.logical_id = logical_id(&frame).unwrap();

    let fixture_root = fixture_root(source);
    let artifacts = frame_manifest["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let payload = std::fs::read(fixture_root.join(item["path"].as_str().unwrap())).unwrap();
            assert_eq!(hex::encode(Sha256::digest(&payload)), item["sha256"].as_str().unwrap());
            let mut temporary = tempfile::NamedTempFile::new().unwrap();
            temporary.write_all(&payload).unwrap();
            RawArtifact {
                receipt: ArtifactReceipt {
                    name: item["name"].as_str().unwrap().into(),
                    media_type: item["media_type"].as_str().unwrap().into(),
                    size_bytes: payload.len() as u64,
                    sha256: item["sha256"].as_str().unwrap().into(),
                },
                path: temporary.into_temp_path(),
            }
        })
        .collect();
    RawFrame { frame, artifacts, private_locator: None }
}

fn fixture_manifest(source: &str) -> Value {
    let contents = match source {
        "rainviewer" => include_str!("../../../tests/fixtures/sources/rainviewer/fixture.json"),
        "windy" => include_str!("../../../tests/fixtures/sources/windy/fixture.json"),
        _ => panic!("test fixture source is not supported"),
    };
    serde_json::from_str(contents).unwrap()
}

fn replace_artifact_bytes(raw: &mut RawFrame, index: usize, payload: &[u8]) {
    let path: &std::path::Path = raw.artifacts[index].path.as_ref();
    std::fs::write(path, payload).unwrap();
    raw.artifacts[index].receipt.size_bytes = payload.len() as u64;
    raw.artifacts[index].receipt.sha256 = hex::encode(Sha256::digest(payload));
}

fn fixture_root(source: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/fixtures/sources/{source}"))
}

fn assert_transport_error<T>(result: Result<T, CoreError>, expected: &str) {
    match result {
        Err(CoreError::Transport(message)) => assert_eq!(message, expected),
        Err(other) => panic!("expected stable transport error, got {other}"),
        Ok(_) => panic!("expected stable transport error {expected:?}"),
    }
}

fn assert_resource_error<T>(result: Result<T, CoreError>, expected: &str) {
    match result {
        Err(CoreError::ResourceLimit(message)) => assert_eq!(message, expected),
        Err(other) => panic!("expected stable resource error, got {other}"),
        Ok(_) => panic!("expected stable resource error {expected:?}"),
    }
}
