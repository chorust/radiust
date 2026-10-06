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

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!("stdout was not a JSON report: {}", String::from_utf8_lossy(&output.stdout))
    })
}

#[test]
fn gray_and_dbz_are_distinct_modes_and_keep_existing_v1_result_fields() {
    let gray = run(&["225-codes.png", "--gray", "--renderer", "text"]);
    assert!(gray.status.success(), "{}", String::from_utf8_lossy(&gray.stderr));
    let gray = report(&gray);
    assert_eq!(gray["schema_version"], 1);
    assert_eq!(gray["mode_schema_version"], 1);
    assert_eq!(gray["mode_info"]["requested"], "gray");
    assert_eq!(gray["mode_info"]["actual"], "gray");
    assert_eq!(gray["mode_info"]["units"], "gray_code");
    assert!(gray["result"].is_string());

    let dbz = run(&["225-codes.png", "--dbz", "--renderer", "text"]);
    assert!(dbz.status.success(), "{}", String::from_utf8_lossy(&dbz.stderr));
    let dbz = report(&dbz);
    assert_eq!(dbz["mode_info"]["requested"], "dbz");
    assert_eq!(dbz["mode_info"]["actual"], "dbz");
    assert_eq!(dbz["mode_info"]["units"], "dBZ");
    assert_eq!(dbz["result"].as_str().unwrap().contains("variable=reflectivity"), true);

    let legacy = run(&["225-codes.png", "--legacy-display", "--renderer", "text"]);
    assert!(!legacy.status.success());
    assert!(report(&legacy)["error"]["message"].as_str().unwrap().contains("requires SOURCE"));
}

#[test]
fn mode_conflicts_and_source_file_or_local_selection_ambiguity_fail_before_decode() {
    for args in [vec!["225-codes.png", "--gray", "--dbz"], vec!["225-codes.png", "--raw", "--gray"]]
    {
        let output = run(&args);
        assert!(!output.status.success());
    }

    let file_and_source = Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args(["--json", "cat", "fr", "--file"])
        .arg(fixture("225-codes.png"))
        .output()
        .unwrap();
    assert!(!file_and_source.status.success());
    assert!(
        String::from_utf8_lossy(&file_and_source.stdout)
            .contains("exactly one of SOURCE or --file")
    );

    let source_frame_index = Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args(["--json", "cat", "fr", "--frame-index", "0"])
        .output()
        .unwrap();
    assert!(!source_frame_index.status.success());
    assert!(
        String::from_utf8_lossy(&source_frame_index.stdout).contains("--frame-index applies only")
    );
}

#[test]
fn raw_download_means_attach_and_raw_only_rejects_decoded_selection() {
    let help =
        Command::new(env!("CARGO_BIN_EXE_radiust")).args(["download", "--help"]).output().unwrap();
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("Include the verified source artifacts alongside decoded output"));
    assert!(help.contains("--raw-only"));
    assert!(help.contains("--dbz"));

    let replay_help =
        Command::new(env!("CARGO_BIN_EXE_radiust")).args(["replay", "--help"]).output().unwrap();
    assert!(String::from_utf8_lossy(&replay_help.stdout).contains("--dbz"));

    let cat_help =
        Command::new(env!("CARGO_BIN_EXE_radiust")).args(["cat", "--help"]).output().unwrap();
    let cat_help = String::from_utf8_lossy(&cat_help.stdout);
    assert!(cat_help.contains("--gray"));
    assert!(cat_help.contains("--legacy-display"));

    let raw_only_conflict = Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args(["--json", "download", "rainviewer", "--dry-run", "--raw-only", "--raw"])
        .output()
        .unwrap();
    assert_eq!(raw_only_conflict.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&raw_only_conflict.stdout).contains("--raw-only"));

    let planned = Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args(["--json", "download", "rainviewer", "--dry-run", "--dbz"])
        .output()
        .unwrap();
    let planned_report = report(&planned);
    assert_eq!(planned_report["mode_schema_version"], 1);
    assert_eq!(planned_report["mode_info"]["requested"], "dbz");
    assert_eq!(planned_report["mode_info"]["actual"], Value::Null);

    let conflict = Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args([
            "--json",
            "download",
            "rainviewer",
            "--dry-run",
            "--raw-only",
            "--variable",
            "reflectivity",
        ])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stdout).contains("--raw-only cannot be combined"));
}

#[test]
fn source_gray_dbz_dry_run_is_not_limited_to_direct_native_sources() {
    let output = Command::new(env!("CARGO_BIN_EXE_radiust"))
        .args([
            "--json",
            "download",
            "au",
            "--product",
            "composite",
            "--latest",
            "--dbz",
            "--dry-run",
        ])
        .output()
        .unwrap();
    let value = report(&output);
    assert_eq!(value["mode_schema_version"], 1);
    assert_eq!(value["mode_info"]["requested"], "dbz");
    assert_eq!(value["mode_info"]["actual"], Value::Null);
    assert_ne!(value["error"]["code"], "unit_mismatch");
}
