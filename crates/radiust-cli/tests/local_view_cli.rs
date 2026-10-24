use serde_json::Value;
use std::process::{Command, Output};

fn run(config: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_radiust"))
        .env_clear()
        .env("HOME", config.parent().unwrap())
        .current_dir(config.parent().unwrap())
        .args(["--json", "--conf"])
        .arg(config)
        .args(args)
        .output()
        .expect("native radiust process starts")
}

fn config(location: bool) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let body = if location {
        "runtime:\n  allow_network: false\nlocation:\n  longitude: 121.6\n  latitude: 24.0\n"
    } else {
        "runtime:\n  allow_network: false\n"
    };
    std::fs::write(root.path().join("config.yaml"), body).unwrap();
    root
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!("stdout was not JSON: {}", String::from_utf8_lossy(&output.stdout))
    })
}

#[test]
fn bare_cat_uses_configured_location_and_never_needs_network_for_unknown_admission() {
    let root = config(true);
    let output = run(&root.path().join("config.yaml"), &["cat"]);

    assert_eq!(output.status.code(), Some(2), "{}", String::from_utf8_lossy(&output.stdout));
    let report = report(&output);
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["local_view_schema_version"], 1);
    assert_eq!(report["status"], "no_candidate");
    assert_eq!(report["result"]["center"]["longitude"], 121.6);
    assert_eq!(report["result"]["radius_m"], 50_000.0);
    assert!(report["result"]["selected"].is_null());
    assert_eq!(report["error"]["code"], "network_disabled");
}

#[test]
fn missing_location_and_invalid_local_combinations_are_local_usage_reports() {
    let no_location = config(false);
    let output = run(&no_location.path().join("config.yaml"), &["cat"]);
    assert_eq!(output.status.code(), Some(2));
    let payload = report(&output);
    assert_eq!(payload["local_view_schema_version"], 1);
    assert_eq!(payload["error"]["code"], "location_missing");
    assert!(payload["result"]["selected"].is_null());

    let configured = config(true);
    for args in [
        vec!["cat", "--radius-km", "0"],
        vec!["cat", "--at", "2026-10-01T00:00:00Z"],
        vec!["cat", "--station", "ABC"],
        vec!["cat", "--product", "composite"],
        vec!["cat", "--station", "ABC", "--station", "DEF"],
        vec!["cat", "--file", "missing.png", "--local"],
        vec!["cat", "--local", "--gray"],
        vec!["cat", "--local", "--legacy-display"],
    ] {
        let output = run(&configured.path().join("config.yaml"), &args);
        assert_eq!(output.status.code(), Some(2), "args={args:?}");
        let report = report(&output);
        assert_eq!(report["local_view_schema_version"], 1, "args={args:?}");
        assert!(report["result"]["selected"].is_null(), "args={args:?}");
    }

    let raw_local = run(&configured.path().join("config.yaml"), &["cat", "--local", "--raw"]);
    assert_eq!(raw_local.status.code(), Some(2));
    let raw_report = report(&raw_local);
    assert_eq!(raw_report["local_view_schema_version"], 1);
    assert_eq!(raw_report["result"]["requested_scope"]["mode"], "raw");
}

#[test]
fn explicit_source_keeps_its_old_route_unless_local_is_requested() {
    let root = config(true);
    let old_route = run(&root.path().join("config.yaml"), &["cat", "rainviewer"]);
    assert_eq!(old_route.status.code(), Some(2));
    let old_report = report(&old_route);
    assert!(old_report.get("local_view_schema_version").is_none());

    let local_route =
        run(&root.path().join("config.yaml"), &["cat", "rainviewer", "--radius-km", "25"]);
    assert_eq!(local_route.status.code(), Some(2));
    let local_report = report(&local_route);
    assert_eq!(local_report["local_view_schema_version"], 1);
    assert_eq!(local_report["result"]["radius_m"], 25_000.0);
    assert_eq!(local_report["result"]["requested_scope"]["source"], "rainviewer");
}
