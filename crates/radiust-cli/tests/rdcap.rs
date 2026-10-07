use serde_json::Value;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

struct NativeCli {
    _root: TempDir,
    config: PathBuf,
    output_root: PathBuf,
}

impl NativeCli {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create isolated CLI workspace");
        let config = root.path().join("radiust.yml");
        let output_root = root.path().join("output");
        let cache_root = root.path().join("cache");
        let config_text = format!(
            "cache:\n  dir: {}\nstorage:\n  output: {}\n",
            yaml_string(&cache_root),
            yaml_string(&output_root),
        );
        std::fs::write(&config, config_text).expect("write isolated CLI config");
        Self { _root: root, config, output_root }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_radiust"));
        command
            .env_clear()
            .current_dir(self._root.path())
            .arg("--conf")
            .arg(&self.config)
            .args(args)
            .arg("--json");
        command.output().expect("run native radiust CLI")
    }
}

fn yaml_string(path: &Path) -> String {
    serde_json::to_string(path.to_str().expect("temporary path is UTF-8"))
        .expect("quote temporary path as a YAML string")
}

fn json_report(output: &Output, expected_exit: i32) -> Value {
    assert_eq!(output.status.code(), Some(expected_exit));
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("native CLI emits one JSON report")
}

fn write_reconstructed_rdcap_manifest(root: &Path) -> PathBuf {
    let station = "TWRCHL";
    let country = "TWN";
    let station_code = "RCHL";
    let key = "1790834708000";
    let valid_time = chrono::DateTime::from_timestamp_millis(key.parse().unwrap())
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    let payload = include_bytes!(
        "../../../tests/fixtures/sources/rdcap/TWN/RCHL/file-response.reconstructed.json"
    );
    let digest = format!("{:x}", Sha256::digest(payload));
    let mut frame = radiust_core::model::FrameRef {
        source: "rdcap".into(),
        product: "reflectivity".into(),
        station: Some(station.into()),
        valid_time: valid_time.clone(),
        base_time: None,
        logical_id: String::new(),
        revision: Some(digest.clone()),
        locator_version: "rdcap-csr-v1".into(),
        locator: json!({"country":country, "station_code":station_code, "key":key}),
    };
    frame.logical_id = radiust_core::identity::logical_id(&frame).unwrap();

    let raw_root = root.join("raw");
    std::fs::create_dir_all(&raw_root).unwrap();
    std::fs::write(raw_root.join("file-response.json"), payload).unwrap();
    let binding = json!({
        "schema_version": 1,
        "source": "rdcap",
        "product": "reflectivity",
        "station": station,
        "country": country,
        "station_code": station_code,
        "key": key,
        "valid_time": valid_time,
        "logical_id": frame.logical_id,
        "content_sha256": digest,
        "content_size_bytes": payload.len(),
    });
    let binding_bytes = serde_json::to_vec_pretty(&binding).unwrap();
    std::fs::write(raw_root.join("binding.json"), &binding_bytes).unwrap();
    let manifest = json!({
        "schema_version": 1,
        "raw_complete": true,
        "ref": radiust_core::identity::safe_ref(&frame).unwrap(),
        "artifacts": [
            {
                "name": "file-response.json",
                "media_type": "application/json",
                "size_bytes": payload.len(),
                "sha256": digest,
            },
            {
                "name": "binding.json",
                "media_type": "application/json",
                "size_bytes": binding_bytes.len(),
                "sha256": format!("{:x}", Sha256::digest(&binding_bytes)),
            }
        ]
    });
    let manifest_path = root.join("raw-manifest.json");
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    manifest_path
}

#[test]
fn decoded_download_gate_accepts_rdcap_reflectivity_for_native_formats_offline() {
    let cli = NativeCli::new();

    // The CLI has no fixture injection point for live discovery. With the
    // default network-off config, this checks format/source capability
    // validation and verifies that no output is written before acquisition.
    for format in ["png", "netcdf", "geotiff", "zarr"] {
        let output =
            cli.run(&["download", "rdcap", "--station", "TWRCHL", "--latest", "--format", format]);
        let report = json_report(&output, 2);
        assert_ne!(
            report["error"]["code"], "unsupported",
            "RDCAP reflectivity should pass the decoded-format capability gate for {format}: {report}"
        );
        assert!(
            !report["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("Native decoded downloads"),
            "RDCAP request was rejected by the decoded capability gate for {format}: {report}"
        );
        assert!(!cli.output_root.exists(), "offline request must not write {format} output");
    }

    let unsupported = cli.run(&["download", "au", "--latest", "--format", "netcdf"]);
    let unsupported = json_report(&unsupported, 2);
    assert_eq!(unsupported["error"]["code"], "unsupported");
    assert!(unsupported["error"]["message"].as_str().unwrap().contains("Native decoded downloads"));
}

#[test]
fn replay_command_decodes_one_offline_manifest_to_all_native_formats_idempotently() {
    let cli = NativeCli::new();
    let manifest_dir = cli._root.path().join("input");
    let manifest = write_reconstructed_rdcap_manifest(&manifest_dir);
    let output_root = cli._root.path().join("replayed");
    let args = [
        "replay",
        manifest.to_str().unwrap(),
        "--output",
        output_root.to_str().unwrap(),
        "--format",
        "png,netcdf,geotiff,zarr",
    ];

    let first = json_report(&cli.run(&args), 0);
    assert_eq!(first["command"], "replay");
    assert_eq!(first["mode_schema_version"], 1);
    assert_eq!(first["mode_info"]["requested"], "scientific");
    assert_eq!(first["mode_info"]["actual"], "scientific");
    assert_eq!(first["mode_info"]["variable"], "reflectivity");
    assert_eq!(first["mode_info"]["units"], "dBZ");
    assert_eq!(first["query"]["station"], "TWRCHL");
    assert_eq!(first["counts"]["written"], 4);
    assert_eq!(first["items"].as_array().unwrap().len(), 4);
    for item in first["items"].as_array().unwrap() {
        assert_eq!(item["status"], "written");
        assert_eq!(item["mode_info"]["actual"], "scientific");
        assert!(item["output_uri"].is_string());
    }

    let second = json_report(&cli.run(&args), 0);
    assert_eq!(second["counts"]["skipped"], 4);
    assert!(second["items"].as_array().unwrap().iter().all(|item| {
        item["status"] == "skipped" && item["mode_info"]["actual"] == "scientific"
    }));

    let dbz_output = cli._root.path().join("replayed-dbz");
    let dbz_args = [
        "replay",
        manifest.to_str().unwrap(),
        "--output",
        dbz_output.to_str().unwrap(),
        "--format",
        "png,netcdf",
        "--dbz",
    ];
    let dbz = json_report(&cli.run(&dbz_args), 0);
    assert_eq!(dbz["mode_info"]["requested"], "dbz");
    assert_eq!(dbz["mode_info"]["actual"], "dbz");
    assert_eq!(dbz["mode_info"]["units"], "dBZ");
    assert!(dbz["items"].as_array().unwrap().iter().all(|item| {
        item["mode_info"]["requested"] == "dbz" && item["mode_info"]["actual"] == "dbz"
    }));
}
