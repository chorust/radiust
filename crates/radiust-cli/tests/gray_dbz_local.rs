use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/gray-dbz/local").join(name)
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args(["--json", "cat", "--file"])
        .arg(fixture(args[0]))
        .args(&args[1..])
        .output()
        .expect("radiust binary must run")
}

fn run_with_config(args: &[&str], config: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args(["--json", "--conf"])
        .arg(config)
        .args(["cat", "--file"])
        .arg(fixture(args[0]))
        .args(&args[1..])
        .output()
        .expect("radiust binary must run")
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!("stdout was not a JSON report: {}", String::from_utf8_lossy(&output.stdout))
    })
}

#[test]
fn local_raw_gray_and_dbz_are_explicit_and_keep_the_v1_envelope() {
    let raw = run(&["225-codes.png", "--raw", "--renderer", "text"]);
    assert!(raw.status.success(), "{}", String::from_utf8_lossy(&raw.stderr));
    let raw = report(&raw);
    assert_eq!(raw["schema_version"], 1);
    assert_eq!(raw["mode_schema_version"], 1);
    assert_eq!(raw["mode_info"]["actual"], "raw");
    assert!(raw["result"].is_string());

    let gray = run(&["225-codes.png", "--gray", "--renderer", "text"]);
    assert!(gray.status.success(), "{}", String::from_utf8_lossy(&gray.stderr));
    let gray = report(&gray);
    assert_eq!(gray["mode_info"]["actual"], "gray");
    assert_eq!(gray["mode_info"]["units"], "gray_code");
    assert!(gray["result"].is_string());

    let dbz = run(&["225-codes.png", "--dbz", "--renderer", "text"]);
    assert!(dbz.status.success(), "{}", String::from_utf8_lossy(&dbz.stderr));
    let dbz = report(&dbz);
    assert_eq!(dbz["mode_info"]["actual"], "dbz");
    assert_eq!(dbz["mode_info"]["units"], "dBZ");
    assert_eq!(dbz["mode_info"]["range_policy"], "strict-v1");
    assert_eq!(dbz["mode_info"]["time_status"], "unknown");
    assert_eq!(dbz["mode_info"]["geolocation"], "unknown");
}

#[test]
fn local_mode_conflicts_and_visible_range_errors_fail_without_success_reports() {
    let conflict = run(&["225-codes.png", "--gray", "--dbz"]);
    assert!(!conflict.status.success());
    let conflict = report(&conflict);
    assert_eq!(conflict["mode_schema_version"], 1);
    assert_eq!(conflict["error"]["code"], "mode_conflict");
    assert_eq!(conflict["error"]["stage"], "validate");
    assert_eq!(conflict["mode_info"]["actual"], Value::Null);

    let invalid = run(&["visible-out-of-range.png", "--dbz"]);
    assert!(!invalid.status.success());
    let report = report(&invalid);
    assert_eq!(report["error"]["code"], "invalid_gray_encoding");
    assert_eq!(report["mode_info"]["actual"], Value::Null);
}

#[test]
fn raw_is_the_default_and_multiframe_requires_a_frame_index_for_dbz() {
    let raw = run(&["225-codes.png", "--renderer", "text"]);
    assert!(raw.status.success(), "{}", String::from_utf8_lossy(&raw.stderr));
    assert_eq!(report(&raw)["mode_info"]["actual"], "raw");

    let ambiguous = run(&["two-frame.gif", "--dbz"]);
    assert!(!ambiguous.status.success());
    let selected = run(&["two-frame.gif", "--dbz", "--frame-index", "1"]);
    assert!(selected.status.success(), "{}", String::from_utf8_lossy(&selected.stderr));
}

#[test]
fn local_dbz_rejects_corrupt_or_colored_images_and_obeys_pixel_limits() {
    for name in ["corrupt.png", "not-gray-rgb.png"] {
        let output = run(&[name, "--dbz"]);
        assert!(!output.status.success());
        let value = report(&output);
        assert_eq!(value["error"]["code"], "invalid_gray_encoding");
        assert_eq!(value["mode_info"]["actual"], Value::Null);
    }

    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("limits.yaml");
    std::fs::write(&config, "runtime:\n  max_pixels: 1\n").unwrap();
    let output = run_with_config(&["225-codes.png", "--dbz"], &config);
    assert!(!output.status.success());
    let value = report(&output);
    assert_eq!(value["error"]["code"], "resource_limit");
    assert_eq!(value["mode_info"]["actual"], Value::Null);
}
