use serde_json::{Value, json};
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::process::Command;

fn gray_source_manifest(root: &Path) -> PathBuf {
    let fixture: Value =
        serde_json::from_slice(include_bytes!("../../../tests/fixtures/sources/ca/fixture.json"))
            .unwrap();
    let metadata = &fixture["frames"][0];
    let bytes = include_bytes!("../../../tests/fixtures/sources/ca/raw/CASFT.gif");
    let digest = format!("{:x}", sha2::Sha256::digest(bytes));
    let mut frame = radiust_core::model::FrameRef {
        source: "ca".into(),
        product: "rain".into(),
        station: Some("CASFT".into()),
        valid_time: metadata["valid_time"].as_str().unwrap().into(),
        base_time: None,
        logical_id: String::new(),
        revision: Some(metadata["revision"].as_str().unwrap().into()),
        locator_version: metadata["locator_version"].as_str().unwrap().into(),
        locator: metadata["locator"].clone(),
    };
    frame.logical_id = radiust_core::identity::logical_id(&frame).unwrap();

    let raw = root.join("raw");
    std::fs::create_dir_all(&raw).unwrap();
    std::fs::write(raw.join("CASFT.gif"), bytes).unwrap();
    let manifest = json!({
        "schema_version": 1,
        "raw_complete": true,
        "ref": radiust_core::identity::safe_ref(&frame).unwrap(),
        "artifacts": [{
            "name": "CASFT.gif",
            "role": "data",
            "media_type": "image/gif",
            "size_bytes": bytes.len(),
            "sha256": digest,
        }],
        "metadata": {},
    });
    let path = root.join("raw-manifest.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    path
}

#[test]
fn dbz_replay_uses_source_decoder_and_keeps_format_failures_partial() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = gray_source_manifest(temp.path());
    let config = temp.path().join("radiust.yml");
    let output_root = temp.path().join("output");
    let cache_root = temp.path().join("cache");
    let temp_root = temp.path().join("stage");
    std::fs::write(
        &config,
        format!(
            "cache:\n  enabled: false\n  dir: {}\nruntime:\n  temp_root: {}\nstorage:\n  output: {}\n",
            serde_json::to_string(cache_root.to_str().unwrap()).unwrap(),
            serde_json::to_string(temp_root.to_str().unwrap()).unwrap(),
            serde_json::to_string(output_root.to_str().unwrap()).unwrap(),
        ),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args(["--conf"])
        .arg(&config)
        .args(["--json", "replay"])
        .arg(&manifest)
        .args(["--output"])
        .arg(&output_root)
        .args(["--format", "png,netcdf,geotiff,zarr", "--dbz"])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(5),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["mode_schema_version"], 1);
    assert_eq!(report["mode_info"]["requested"], "dbz");
    assert_eq!(report["mode_info"]["actual"], "dbz");
    assert_eq!(report["mode_info"]["method"], "verified_source_gray_dbz");
    assert_eq!(report["mode_info"]["units"], "dBZ");
    assert_eq!(report["counts"]["written"], 3);
    assert_eq!(report["counts"]["failed"], 1);
    assert_eq!(report["items"].as_array().unwrap().len(), 4);
    for item in report["items"].as_array().unwrap() {
        if item["format"] == "geotiff" {
            assert_eq!(item["status"], "failed");
            assert_eq!(item["mode_info"]["actual"], Value::Null);
            assert!(item["error"].is_object());
        } else {
            assert_eq!(item["status"], "written");
            assert_eq!(item["mode_info"]["actual"], "dbz");
            let output = item["output_uri"].as_str().unwrap();
            assert!(
                Path::new(output).is_file() || Path::new(output).is_dir(),
                "{}: {output}",
                item["format"]
            );
            assert!(
                Path::new(&format!("{output}.manifest.json")).is_file(),
                "{}: {output}",
                item["format"]
            );
        }
    }
}
