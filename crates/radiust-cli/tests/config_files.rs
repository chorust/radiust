use serde_json::Value;
use std::process::Command;

#[test]
fn config_show_loads_user_then_project_then_environment() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let project = root.path().join("project");
    std::fs::create_dir_all(home.join(".config/radiust")).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        home.join(".config/radiust/config.yaml"),
        "runtime:\n  frame_concurrency: 3\n  discovery_workers: 7\nsources:\n  id_sidarma:\n    api_key: user-private\n",
    )
    .unwrap();
    std::fs::write(
        project.join("config.yaml"),
        "runtime:\n  frame_concurrency: 4\nsources:\n  id_sidarma:\n    radar_ids: [JAK]\n",
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_radiust"));
    command.env_clear().env("HOME", &home).current_dir(&project).args(["config", "show", "--json"]);
    let output = command.output().unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["result"]["runtime"]["frame_concurrency"], 4);
    assert_eq!(report["result"]["runtime"]["discovery_workers"], 7);
    assert_eq!(report["result"]["sources"]["id_sidarma"]["api_key"], "[REDACTED]");
    assert_eq!(report["result"]["sources"]["id_sidarma"]["radar_ids"], serde_json::json!(["JAK"]));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("user-private"));
    command.env("RADIUST_RUNTIME__FRAME_CONCURRENCY", "5");
    let output = command.output().unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["result"]["runtime"]["frame_concurrency"], 5);

    let explicit = project.join("custom.yaml");
    std::fs::write(&explicit, "output:\n  format: png\n").unwrap();
    std::fs::write(project.join("config.yaml"), "invalid project: [").unwrap();
    command.arg("--conf").arg(explicit);
    let output = command.output().unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["result"]["runtime"]["discovery_workers"], 7);
    assert_eq!(report["result"]["runtime"]["frame_concurrency"], 5);
    assert_eq!(report["result"]["output"]["format"], "png");
}
