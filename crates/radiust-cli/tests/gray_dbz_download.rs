use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/gray-dbz/local").join(name)
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!("stdout was not a JSON report: {}", String::from_utf8_lossy(&output.stdout))
    })
}

fn config(path: &Path, output: &Path) {
    std::fs::write(path, format!("storage:\n  output: {:?}\noutput:\n  format: netcdf\n", output))
        .unwrap();
}

fn run(config_path: &Path, args: &[&str]) -> Output {
    run_file(config_path, args[0], &args[1..])
}

fn run_file(config_path: &Path, file: &str, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args(["--json", "--conf"])
        .arg(config_path)
        .args(["download", "--file"])
        .arg(fixture(file))
        .args(flags)
        .output()
        .unwrap()
}

#[test]
fn local_gray_dbz_download_commits_a_manifest_and_can_be_dry_run() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.yaml");
    let output_root = directory.path().join("outputs");
    config(&config_path, &output_root);

    let dry_run = run(&config_path, &["225-codes.png", "--dbz", "--dry-run"]);
    assert!(dry_run.status.success(), "{}", String::from_utf8_lossy(&dry_run.stderr));
    let dry_run = report(&dry_run);
    assert_eq!(dry_run["mode_info"]["requested"], "dbz");
    assert_eq!(dry_run["mode_info"]["actual"], Value::Null);
    assert_eq!(dry_run["items"][0]["status"], "planned");
    assert!(!output_root.exists());

    let written = run(&config_path, &["225-codes.png", "--dbz"]);
    assert!(written.status.success(), "{}", String::from_utf8_lossy(&written.stderr));
    let written = report(&written);
    assert_eq!(written["mode_info"]["actual"], "dbz");
    assert_eq!(written["mode_info"]["units"], "dBZ");
    assert_eq!(written["items"][0]["status"], "written");
    let output_uri = written["items"][0]["output_uri"].as_str().unwrap();
    assert!(Path::new(output_uri).is_file());
    assert!(Path::new(&format!("{output_uri}.manifest.json")).is_file());
    assert_eq!(written["items"][0]["logical_id"], Value::Null);
}

#[test]
fn local_numeric_dbz_read_write_preserves_units_and_rejects_geographic_requests() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.yaml");
    let output_root = directory.path().join("outputs");
    config(&config_path, &output_root);

    let first = run(&config_path, &["225-codes.png", "--dbz"]);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let first = report(&first);
    let numeric_file = first["items"][0]["output_uri"].as_str().unwrap();

    let second = Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args(["--json", "--conf"])
        .arg(&config_path)
        .args(["download", "--file", numeric_file, "--dbz", "--format", "png"])
        .output()
        .unwrap();
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    assert_eq!(report(&second)["mode_info"]["actual"], "dbz");

    let geographic = Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args(["--json", "--conf"])
        .arg(&config_path)
        .args(["download", "--file", numeric_file, "--dbz", "--grid", "geographic"])
        .output()
        .unwrap();
    assert_eq!(geographic.status.code(), Some(2));
    let geographic = report(&geographic);
    assert_eq!(geographic["mode_info"]["actual"], Value::Null);
    assert!(geographic["error"]["message"].as_str().unwrap().contains("local"));
}

#[test]
fn local_download_requires_dbz_and_rejects_source_selection_flags() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.yaml");
    config(&config_path, &directory.path().join("outputs"));

    for flags in [
        vec!["--dbz", "--latest"],
        vec!["--dbz", "--product", "composite"],
        vec!["--dbz", "--bbox", "0,0,1,1"],
    ] {
        let output = run_file(&config_path, "225-codes.png", &flags);
        assert!(!output.status.success());
        assert!(report(&output)["mode_info"]["actual"].is_null());
    }

    let missing_dbz = run(&config_path, &["225-codes.png"]);
    assert_eq!(missing_dbz.status.code(), Some(2));
    assert_eq!(report(&missing_dbz)["mode_info"]["actual"], Value::Null);

    let ambiguous = Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args(["--json", "--conf"])
        .arg(&config_path)
        .args(["download", "rainviewer", "--file"])
        .arg(fixture("225-codes.png"))
        .args(["--dbz"])
        .output()
        .unwrap();
    assert_eq!(ambiguous.status.code(), Some(2));
    assert_eq!(report(&ambiguous)["mode_info"]["actual"], Value::Null);
}

#[test]
fn failed_local_decode_does_not_create_a_successful_download() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.yaml");
    let output_root = directory.path().join("outputs");
    config(&config_path, &output_root);

    let failed = run(&config_path, &["visible-out-of-range.png", "--dbz"]);
    assert_eq!(failed.status.code(), Some(5));
    let failed = report(&failed);
    assert_eq!(failed["error"]["code"], "invalid_gray_encoding");
    assert_eq!(failed["mode_info"]["actual"], Value::Null);
    assert!(!output_root.exists());
}
