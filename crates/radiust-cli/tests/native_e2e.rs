use radiust_core::cache::Cache;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

struct NativeCli {
    _root: TempDir,
    config: PathBuf,
    output_root: PathBuf,
    cache_root: PathBuf,
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
        Self { _root: root, config, output_root, cache_root }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_radiust"));
        command.env_clear().current_dir(self._root.path()).arg("--conf").arg(&self.config);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command()
            .args(args)
            .arg("--json")
            .output()
            .expect("start the native radiust executable")
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

fn assert_envelope(report: &Value, command: Option<&str>) {
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], command.map_or(Value::Null, |name| Value::String(name.into())));
    for field in ["run_id", "query", "counts", "items", "error", "interrupted"] {
        assert!(report.get(field).is_some(), "JSON envelope is missing {field}: {report}");
    }
    assert!(report["items"].is_array());
}

#[test]
fn list_emits_the_native_json_envelope() {
    let cli = NativeCli::new();
    let output = cli.run(&["list", "sources"]);
    let report = json_report(&output, 0);

    assert_envelope(&report, Some("list"));
    assert!(report["items"].as_array().is_some_and(|items| !items.is_empty()));
    assert_eq!(report["error"], Value::Null);
}

#[test]
fn source_details_and_read_only_config_cache_commands_run_without_python() {
    let cli = NativeCli::new();

    let source = json_report(&cli.run(&["list", "fr"]), 0);
    assert_envelope(&source, Some("list"));
    assert_eq!(source["result"]["id"], "fr");
    assert_eq!(source["result"]["products"], serde_json::json!(["composite"]));

    let products = json_report(&cli.run(&["list", "products", "fr"]), 0);
    assert_eq!(products["items"].as_array().unwrap().len(), 1);
    assert_eq!(products["items"][0]["id"], "composite");

    let stations = json_report(&cli.run(&["list", "stations", "my"]), 0);
    assert_eq!(stations["items"].as_array().unwrap().len(), 2);

    let config = json_report(&cli.run(&["config"]), 0);
    assert_eq!(config["result"]["runtime"]["allow_network"], false);

    for operation in ["gc", "clear"] {
        let preview = json_report(&cli.run(&["cache", operation, "--dry-run"]), 0);
        assert_eq!(preview["operation"], operation);
        assert_eq!(preview["removed"], serde_json::json!([]));
        assert!(
            !cli.cache_root.exists(),
            "cache {operation} preview must not create the configured root"
        );
    }
}

#[test]
fn rdcap_listing_exposes_offline_directory_provenance_and_country_capabilities() {
    let cli = NativeCli::new();
    let source = json_report(&cli.run(&["list", "rdcap"]), 0);
    assert_eq!(
        source["result"]["metadata"]["country_capabilities"]["TWN"]["discovery"],
        "unverified"
    );

    let stations = json_report(&cli.run(&["list", "stations", "rdcap"]), 0);
    assert_eq!(stations["items"].as_array().unwrap().len(), 48);
    assert_eq!(stations["items"][0]["catalog_status"], "offline_snapshot");
    assert_eq!(stations["items"][0]["snapshot_date"], "2026-10-01");
    assert!(
        stations["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|station| { station["id"] == "JPMAKI" && station["country"] == "JPN" })
    );
    assert!(
        stations["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|station| { !station["id"].as_str().unwrap().contains('/') })
    );
    assert!(stations["items"].as_array().unwrap().iter().any(|station| {
        station["id"] == "PHBALE"
            && station["country"] == "PHL"
            && station["directory_statuses"] == serde_json::json!(["Inactive", "Active"])
            && station["directory_conflicts"].as_array().is_some_and(|conflicts| {
                conflicts.iter().any(|conflict| conflict["field"] == "Status")
            })
            && station["capabilities"]["raw_acquisition"] == "unverified"
    }));

    let discovery = json_report(&cli.run(&["discover", "all"]), 5);
    assert_eq!(discovery["counts"]["total"], 74);
    assert_eq!(discovery["counts"]["network_restricted"], 70);
    let rdcap_items = discovery["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["source"] == "rdcap")
        .collect::<Vec<_>>();
    assert_eq!(rdcap_items.len(), 48);
    assert!(rdcap_items.iter().all(|item| item["status"] == "network_restricted"));

    let output = cli
        .command()
        .args(["list", "stations", "rdcap"])
        .output()
        .expect("run human RDCAP listing");
    assert_eq!(output.status.code(), Some(0));
    let human = String::from_utf8(output.stdout).unwrap();
    for label in ["country", "status", "conflict", "snapshot_date", "capability"] {
        assert!(human.contains(label), "missing {label} from human output:\n{human}");
    }
}

#[test]
fn discover_defaults_to_network_off_and_reports_restricted_sources() {
    let cli = NativeCli::new();
    let output = cli.run(&["discover", "au", "vn"]);
    let report = json_report(&output, 5);

    assert_envelope(&report, Some("discover"));
    assert_eq!(report["query"]["sources"], serde_json::json!(["au", "vn"]));
    assert_eq!(report["counts"]["total"], 2);
    assert_eq!(report["counts"]["network_restricted"], 2);
    assert!(report["items"].as_array().unwrap().iter().all(|item| {
        item["status"] == "network_restricted" && item["error"]["code"] == "network_restricted"
    }));
}

#[test]
fn download_dry_run_returns_a_json_error_without_network_access() {
    let cli = NativeCli::new();
    let output = cli.run(&["download", "au", "--latest", "--dry-run"]);
    let report = json_report(&output, 2);

    assert_envelope(&report, None);
    assert_eq!(report["error"]["stage"], "validate");
    assert_eq!(report["error"]["retryable"], false);
    assert!(!cli.output_root.exists(), "offline dry-run must not create output files");
}

#[test]
fn cat_previews_a_local_image_without_network_access() {
    let cli = NativeCli::new();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/sources/au/raw/IDR021.T.202609180511.png");
    let local_image = cli._root.path().join("local.png");
    std::fs::copy(fixture, &local_image).expect("copy local image fixture into temp workspace");
    let output = cli
        .command()
        .args(["cat", "--file"])
        .arg(&local_image)
        .args(["--renderer", "text", "--json"])
        .output()
        .expect("start the native radiust executable");
    let report = json_report(&output, 0);

    assert_envelope(&report, Some("cat"));
    assert_eq!(report["items"], serde_json::json!([]));
    assert!(report["result"].as_str().is_some_and(|text| text.contains("local.png")));
    assert_eq!(report["error"], Value::Null);
}

#[test]
fn doctor_json_marks_network_probe_as_not_requested_by_default() {
    let cli = NativeCli::new();
    let output = cli.run(&["doctor"]);
    let report = json_report(&output, 0);

    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], "doctor");
    assert_eq!(report["checks"]["native_runtime"], true);
    assert_eq!(report["checks"]["source_catalog"], true);
    assert_eq!(report["checks"]["network_probe"], "not_requested");
    assert_eq!(report["checks"]["ok"], true);
}

#[test]
fn cache_clear_requires_confirmation_and_removes_indexed_entries() {
    let cli = NativeCli::new();
    let cache = Cache::open(&cli.cache_root).expect("open test cache");
    cache.put_bytes("fixture-clear-key", b"cached-object", "object", None).unwrap();
    drop(cache);

    let missing_confirmation = cli.run(&["cache", "clear"]);
    json_report(&missing_confirmation, 2);
    let cache = Cache::open(&cli.cache_root).expect("reopen cache after refused clear");
    assert_eq!(cache.get_bytes("fixture-clear-key").unwrap(), Some(b"cached-object".to_vec()));
    drop(cache);

    let output = cli.run(&["cache", "clear", "--yes"]);
    let report = json_report(&output, 0);
    assert_eq!(report["operation"], "clear");
    assert_eq!(report["bytes_before"], 13);
    assert_eq!(report["bytes_after"], 0);
    assert_eq!(report["removed"], serde_json::json!(["fixture-clear-key"]));
    let cache = Cache::open(&cli.cache_root).expect("reopen cache after clear");
    assert_eq!(cache.get_bytes("fixture-clear-key").unwrap(), None);
}

#[test]
fn config_show_exposes_isolated_roots_and_the_network_off_default() {
    let cli = NativeCli::new();
    let output = cli.run(&["config", "show"]);
    let report = json_report(&output, 0);

    assert_envelope(&report, Some("config"));
    assert_eq!(report["result"]["runtime"]["allow_network"], false);
    assert_eq!(report["result"]["storage"]["output"], cli.output_root.to_string_lossy().as_ref());
    assert_eq!(report["result"]["cache"]["dir"], cli.cache_root.to_string_lossy().as_ref());
}

#[test]
fn cache_status_uses_the_temporary_configured_cache_root() {
    let cli = NativeCli::new();
    let output = cli.run(&["cache", "status"]);
    let report = json_report(&output, 0);

    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["root"], cli.cache_root.canonicalize().unwrap().to_string_lossy().as_ref());
    assert_eq!(report["entries"], 0);
    assert_eq!(report["bytes"], 0);
    assert_eq!(report["enabled"], true);
}
