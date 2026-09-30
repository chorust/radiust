use serde_json::Value;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn native_command() -> Command {
    static ROOT: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let root = ROOT.get_or_init(|| {
        let root = tempfile::tempdir().expect("create isolated CLI home and workspace");
        std::fs::create_dir(root.path().join(".cache")).unwrap();
        root
    });
    let mut command = Command::new(env!("CARGO_BIN_EXE_radiust"));
    // Contract tests use a configuration-free directory and environment rather
    // than loading a developer's user or project YAML.
    command.env_clear().env("HOME", root.path()).current_dir(root.path());
    command
}

fn unique_temp_path(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("radiust-cli-{label}-{}-{nonce}", std::process::id()))
}

fn assert_snapshot(args: &[&str], expected: &str) {
    let output = native_command().args(args).output().expect("native radiust process starts");
    let expected: Value = serde_json::from_str(expected).unwrap();
    assert_eq!(output.status.code(), expected["exit_code"].as_i64().map(|code| code as i32));
    assert_eq!(String::from_utf8(output.stderr).unwrap(), expected["stderr"].as_str().unwrap());
    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout, expected["stdout"]);
}

#[cfg(unix)]
fn run_with_stderr_tty(args: &[&str]) -> (std::process::ExitStatus, Vec<u8>, Vec<u8>) {
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::FromRawFd;
    use std::process::Stdio;
    use std::thread;

    let mut master_fd = -1;
    let mut slave_fd = -1;
    // SAFETY: openpty writes two newly-owned file descriptors into the outputs.
    let opened = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(opened, 0, "openpty succeeds");
    // SAFETY: each descriptor returned by openpty is wrapped exactly once.
    let mut master = unsafe { File::from_raw_fd(master_fd) };
    // SAFETY: see above; the child receives only stderr on this terminal.
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    let mut command = native_command();
    command
        .env_clear()
        .env("TERM", "xterm-256color")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(slave));
    let mut child = command.spawn().expect("native radiust process starts");
    drop(command);
    let mut stdout = child.stdout.take().expect("child stdout is piped");
    let stdout_reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).expect("read child stdout");
        output
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        master.read_to_end(&mut output).expect("read terminal stderr");
        output
    });
    let status = child.wait().expect("native radiust process exits");
    (
        status,
        stdout_reader.join().expect("stdout reader joins"),
        stderr_reader.join().expect("stderr reader joins"),
    )
}

#[test]
fn list_sources_matches_the_v1_contract_without_python() {
    assert_snapshot(
        &["list", "sources", "--json"],
        include_str!("../../../tests/fixtures/rust-migration/cli/list-sources.json"),
    );
}

#[test]
fn list_products_exposes_the_catalog_metadata_used_by_clients() {
    let output = native_command()
        .args(["list", "products", "au", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(output.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let product = &report["items"][0];
    for field in ["variables", "units", "historical", "default", "mutable"] {
        assert!(product.get(field).is_some(), "missing catalog product field {field}");
    }
}

#[test]
fn list_source_exposes_source_details_and_its_station_catalog() {
    assert_snapshot(
        &["list", "fr", "--json"],
        include_str!(
            "../../../tests/fixtures/rust-migration/cli/python-baseline/list-source-fr.json"
        ),
    );

    let output = native_command()
        .args(["list", "fr", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(output.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let source = &report["result"];
    assert_eq!(source["id"], "fr");
    assert!(source["description"].is_string());
    assert!(source["availability"].is_string());
    assert!(source["products"].is_array());
    assert!(source["stations"].is_array());
}

#[test]
fn list_source_human_output_keeps_labeled_fields_in_order() {
    let output =
        native_command().args(["list", "fr"]).output().expect("native radiust process starts");
    assert_eq!(output.status.code(), Some(0));
    let human = String::from_utf8(output.stdout).unwrap();
    assert!(human.starts_with("LIST\n"));
    assert!(human.contains("France Meteo-France radar"));
    assert!(human.contains("needs_configuration"));
    let positions = ["id:", "description:", "availability:", "products:", "stations:"]
        .map(|label| human.find(label).expect("source details contain the labeled field"));
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(!human.contains('\x1b'));
}

#[test]
fn list_unknown_source_fails_before_any_network_access() {
    let output = native_command()
        .args(["list", "not-a-source", "--json"])
        .env("RADIUST_RUNTIME__ALLOW_NETWORK", "true")
        .output()
        .expect("native radiust process starts");
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["error"]["stage"], "validate");
    assert_eq!(report["error"]["message"], "unknown source: not-a-source");
    assert_eq!(report["items"], serde_json::json!([]));
}

#[test]
fn quiet_suppresses_success_reports_but_json_takes_precedence() {
    let quiet = native_command()
        .args(["--quiet", "list", "sources"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(quiet.status.code(), Some(0));
    assert!(quiet.stdout.is_empty());
    assert!(quiet.stderr.is_empty());

    let json = native_command()
        .args(["--quiet", "list", "sources", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(json.status.code(), Some(0));
    assert!(json.stderr.is_empty());
    let report: Value = serde_json::from_slice(&json.stdout).expect("quiet preserves JSON output");
    assert_eq!(report["command"], "list");
}

#[test]
fn native_human_reports_are_labeled_and_verbose_does_not_change_json() {
    let plain =
        native_command().args(["list", "sources"]).output().expect("native radiust process starts");
    assert_eq!(plain.status.code(), Some(0));
    let plain_text = String::from_utf8(plain.stdout).unwrap();
    assert!(plain_text.starts_with("LIST\n"));
    assert!(plain_text.contains("Total:"));

    let verbose = native_command()
        .args(["list", "sources", "--verbose"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(verbose.status.code(), Some(0));
    let verbose_text = String::from_utf8(verbose.stdout).unwrap();
    assert!(verbose_text.starts_with("LIST\n"));
    assert!(verbose_text.contains("diagnostics:\n  runtime: rust"));

    let json = |args: &[&str]| {
        let output = native_command().args(args).output().expect("native radiust process starts");
        assert_eq!(output.status.code(), Some(0));
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    assert_eq!(
        json(&["config", "show", "--json"]),
        json(&["config", "show", "--json", "--verbose"])
    );

    let conflict = native_command()
        .args(["--quiet", "list", "sources", "--verbose"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(conflict.status.code(), Some(2));
    assert!(String::from_utf8(conflict.stderr).unwrap().contains("mutually exclusive"));
}

#[test]
fn aggregate_discover_matches_v1_offline_statuses_and_exit_code() {
    assert_snapshot(
        &["discover", "all", "--json"],
        include_str!("../../../tests/fixtures/rust-migration/cli/discover-all.json"),
    );
}

#[cfg(unix)]
#[test]
fn aggregate_discover_reports_safe_progress_on_tty_stderr_and_keeps_json_on_stdout() {
    let (status, stdout, stderr) = run_with_stderr_tty(&["discover", "all", "--json"]);
    assert!(status.code().is_some());
    let report: Value = serde_json::from_slice(&stdout).expect("stdout contains one JSON report");
    assert_eq!(report["command"], "discover");
    assert!(report["counts"]["total"].as_u64().unwrap() > 0);
    assert_eq!(stdout.iter().filter(|byte| **byte == b'\n').count(), 1);

    let stderr = String::from_utf8(stderr).unwrap();
    assert!(stderr.contains("radiust: discover"), "{stderr}");
    assert!(stderr.contains("radiust: discover complete"), "{stderr}");
    assert!(stderr.contains("[00:00:"), "{stderr}");
    assert!(!stderr.contains("Bearer "), "progress must not expose credentials");
    assert!(!stderr.contains("signed_url"), "progress must not expose locators");
}

#[cfg(unix)]
#[test]
fn source_cat_reports_engine_progress_on_tty_stderr_without_polluting_json_stdout() {
    let (status, stdout, stderr) = run_with_stderr_tty(&["cat", "au", "--latest", "--json"]);
    assert_eq!(status.code(), Some(2));
    let report: Value = serde_json::from_slice(&stdout).expect("stdout contains one JSON report");
    assert_eq!(report["error"]["stage"], "validate");

    let stderr = String::from_utf8(stderr).unwrap();
    assert!(stderr.contains("radiust: discover"), "{stderr}");
    assert!(stderr.contains("radiust: discover complete"), "{stderr}");
    assert!(stderr.contains("[00:00:"), "{stderr}");
    assert!(!stderr.contains("Bearer "), "progress must not expose credentials");
    assert!(!stderr.contains("signed_url"), "progress must not expose locators");
}

#[test]
fn download_dry_run_preserves_the_legacy_offline_error_boundary() {
    assert_snapshot(
        &["download", "au", "--latest", "--dry-run", "--json"],
        include_str!("../../../tests/fixtures/rust-migration/cli/download-dry-run.json"),
    );
}

#[test]
fn download_dry_run_reports_planned_frames_without_acquiring_them() {
    let config = unique_temp_path("download-dry-run.yml");
    std::fs::write(&config, "runtime:\n  allow_network: true\n").unwrap();
    let output = native_command()
        .args(["download", "fr", "--latest", "--dry-run", "--conf"])
        .arg(&config)
        .arg("--json")
        .env("RADIUST_RUNTIME__ALLOW_NETWORK", "true")
        .output()
        .expect("native radiust process starts");
    std::fs::remove_file(config).unwrap();

    assert_eq!(output.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], "download");
    assert_eq!(report["run_id"], "dry-run");
    assert_eq!(report["counts"]["planned"], report["items"].as_array().unwrap().len());
    assert!(report["items"].as_array().unwrap().iter().all(|item| {
        item["status"] == "planned" && item["output_uri"].is_null() && item["error"].is_null()
    }));
    assert_eq!(report["error"], Value::Null);
}

#[test]
fn download_accepts_legacy_raw_and_object_storage_options_without_exposing_credentials() {
    let config = unique_temp_path("download-storage-options.yml");
    std::fs::write(&config, "runtime:\n  allow_network: false\n").unwrap();
    let output = native_command()
        .args([
            "download",
            "tw",
            "--dry-run",
            "--raw",
            "--access-key",
            "private-access-option",
            "--secret-key",
            "private-secret-option",
            "--endpoint",
            "https://objects.invalid",
            "--conf",
        ])
        .arg(&config)
        .arg("--json")
        .output()
        .expect("native radiust process starts");
    std::fs::remove_file(config).unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(serde_json::from_str::<Value>(&stdout).is_ok(), "{stdout}");
    assert!(!stdout.contains("private-access-option"));
    assert!(!stdout.contains("private-secret-option"));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("private-access-option"));
    assert!(!stderr.contains("private-secret-option"));
}

#[test]
fn cache_clear_requires_confirmation_and_removes_only_cache_entries() {
    let root = unique_temp_path("cache-clear");
    let cache = radiust_core::cache::Cache::open(&root).unwrap();
    cache.put_bytes("fixture-key", b"cache-owned", "object", None).unwrap();
    drop(cache);

    let refused = native_command()
        .args(["cache", "clear", "--cache-dir"])
        .arg(&root)
        .output()
        .expect("native radiust process starts");
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8(refused.stderr).unwrap().contains("requires --yes"));

    let confirmed = native_command()
        .args(["cache", "clear", "--cache-dir"])
        .arg(&root)
        .args(["--yes", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(confirmed.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&confirmed.stdout).unwrap();
    assert_eq!(report["operation"], "clear");
    assert_eq!(report["removed"], serde_json::json!(["fixture-key"]));

    let cache = radiust_core::cache::Cache::open(&root).unwrap();
    assert!(cache.index.entries().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cache_gc_and_clear_dry_run_preserve_entries_until_confirmed() {
    let root = unique_temp_path("cache-dry-run");
    let cache = radiust_core::cache::Cache::open(&root).unwrap();
    cache.put_bytes("expired-key", b"cache-owned", "object", Some(0)).unwrap();
    drop(cache);

    let gc = native_command()
        .args(["cache", "gc", "--cache-dir"])
        .arg(&root)
        .args(["--dry-run", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(gc.status.code(), Some(0));
    let gc_report: Value = serde_json::from_slice(&gc.stdout).unwrap();
    assert_eq!(gc_report["operation"], "gc");
    assert_eq!(gc_report["dry_run"], true);
    assert_eq!(gc_report["removed"], serde_json::json!(["expired-key"]));

    let clear = native_command()
        .args(["cache", "clear", "--cache-dir"])
        .arg(&root)
        .args(["--dry-run", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(clear.status.code(), Some(0));
    let clear_report: Value = serde_json::from_slice(&clear.stdout).unwrap();
    assert_eq!(clear_report["operation"], "clear");
    assert_eq!(clear_report["dry_run"], true);
    assert_eq!(clear_report["removed"], serde_json::json!(["expired-key"]));

    let cache = radiust_core::cache::Cache::open(&root).unwrap();
    assert_eq!(cache.index.entries().unwrap().len(), 1);
    drop(cache);

    let confirmed = native_command()
        .args(["cache", "clear", "--cache-dir"])
        .arg(&root)
        .args(["--yes", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(confirmed.status.code(), Some(0));
    assert!(radiust_core::cache::Cache::open(&root).unwrap().index.entries().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cache_dry_run_does_not_create_a_missing_cache_root() {
    let root = unique_temp_path("cache-dry-run-missing");
    for operation in ["gc", "clear"] {
        let output = native_command()
            .args(["cache", operation, "--cache-dir"])
            .arg(&root)
            .args(["--dry-run", "--json"])
            .output()
            .expect("native radiust process starts");
        assert_eq!(output.status.code(), Some(0));
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["operation"], operation);
        assert_eq!(report["removed"], serde_json::json!([]));
        assert!(!root.exists(), "{operation} preview created the cache root");
    }
}

#[test]
fn config_without_an_action_defaults_to_show() {
    let config = unique_temp_path("config-default-show.yml");
    std::fs::write(&config, "runtime:\n  allow_network: false\n").unwrap();
    let run = |args: &[&str]| {
        native_command()
            .args(args)
            .arg("--conf")
            .arg(&config)
            .arg("--json")
            .output()
            .expect("native radiust process starts")
    };
    let default = run(&["config"]);
    let explicit = run(&["config", "show"]);
    std::fs::remove_file(config).unwrap();

    assert_eq!(default.status.code(), Some(0));
    assert_eq!(explicit.status.code(), Some(0));
    let default_report: Value = serde_json::from_slice(&default.stdout).unwrap();
    let explicit_report: Value = serde_json::from_slice(&explicit.stdout).unwrap();
    assert_eq!(default_report["result"], explicit_report["result"]);
    assert_eq!(default_report["command"], "config");
}

#[test]
fn raw_only_download_keeps_network_opt_in_and_accepts_output_options() {
    let output_root = unique_temp_path("raw-output");
    let output = native_command()
        .args([
            "download",
            "au",
            "--latest",
            "--raw-only",
            "--format",
            "png",
            "--on-error",
            "stop",
            "--output",
        ])
        .arg(&output_root)
        .arg("--json")
        .output()
        .expect("native radiust process starts");

    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["error"]["stage"], "validate");
    assert_eq!(report["error"]["message"], "Unexpected operation failure");
    assert!(!output_root.exists());
}

#[test]
fn decoded_download_rejects_unverified_formats_but_accepts_geotiff_dispatch() {
    let output = native_command()
        .args(["download", "au", "--latest", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["error"]["code"], "unsupported");
    assert_eq!(report["error"]["stage"], "validate");
    assert!(report["error"]["message"].as_str().unwrap().contains("--format png"));

    // Zarr is a valid output format, but unverified sources must fail closed
    // with the same structured validation response instead of reaching a panic.
    let zarr_error = native_command()
        .args(["download", "au", "--latest", "--format", "zarr", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(zarr_error.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&zarr_error.stdout).unwrap();
    assert_eq!(report["error"]["code"], "unsupported");
    assert_eq!(report["error"]["stage"], "validate");
    assert!(report["error"]["message"].as_str().unwrap().contains("Native Zarr downloads"));

    let quiet_error = native_command()
        .args(["--quiet", "download", "rainviewer", "--latest", "--format", "geotiff", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(quiet_error.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&quiet_error.stdout).unwrap();
    assert_eq!(report["error"]["code"], "error");
    assert_eq!(report["error"]["message"], "Unexpected operation failure");
}

#[test]
fn download_format_uses_config_default_unless_cli_flag_overrides() {
    let config = unique_temp_path("download-format-config.yml");
    std::fs::write(&config, "runtime:\n  allow_network: false\noutput:\n  format: png\n").unwrap();

    let configured = native_command()
        .args(["--conf"])
        .arg(&config)
        .args(["download", "au", "--latest", "--json"])
        .output()
        .expect("native radiust process starts");
    let explicit = native_command()
        .args(["--conf"])
        .arg(&config)
        .args(["download", "au", "--latest", "--format", "netcdf", "--json"])
        .output()
        .expect("native radiust process starts");
    std::fs::remove_file(config).unwrap();

    assert_eq!(configured.status.code(), Some(2));
    assert_eq!(explicit.status.code(), Some(2));
    let configured: Value = serde_json::from_slice(&configured.stdout).unwrap();
    let explicit: Value = serde_json::from_slice(&explicit.stdout).unwrap();
    assert_eq!(configured["error"]["code"], "unsupported");
    assert_eq!(explicit["error"]["code"], "unsupported");
    assert!(configured["error"]["message"].as_str().unwrap().contains("Native PNG download"));
    assert!(explicit["error"]["message"].as_str().unwrap().contains("--format png, netcdf"));
}

#[test]
fn geographic_download_options_preserve_the_cli_offline_failure_contract() {
    let config = unique_temp_path("geographic-download-config.yml");
    let output_root = unique_temp_path("geographic-download-output");
    std::fs::write(
        &config,
        format!(
            "runtime:\n  allow_network: false\nstorage:\n  output: {}\n",
            serde_json::to_string(output_root.to_str().unwrap()).unwrap()
        ),
    )
    .unwrap();

    let output = native_command()
        .args(["--conf"])
        .arg(&config)
        .args([
            "download",
            "rainviewer",
            "--latest",
            "--format",
            "netcdf",
            "--variable",
            "reflectivity",
            "--grid",
            "geographic",
            "--bbox=-1,-1,1,1",
            "--resolution",
            "1",
            "--resampling",
            "bilinear",
            "--json",
        ])
        .output()
        .expect("native radiust process starts");
    let baseline = native_command()
        .args(["--conf"])
        .arg(&config)
        .args(["download", "rainviewer", "--latest", "--json"])
        .output()
        .expect("native radiust process starts");
    std::fs::remove_file(config).unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(baseline.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let baseline_report: Value = serde_json::from_slice(&baseline.stdout).unwrap();
    assert_eq!(report["error"]["message"], "Unexpected operation failure");
    assert_eq!(report, baseline_report, "processing options must not change the offline failure");
    assert!(!output_root.exists(), "offline rejected download must not create output files");
}

#[test]
fn single_source_discover_preserves_the_legacy_offline_error_boundary() {
    assert_snapshot(
        &["discover", "au", "--latest", "--json"],
        include_str!("../../../tests/fixtures/rust-migration/cli/discover-single.json"),
    );
}

#[test]
fn cat_local_image_auto_renderer_preserves_raw_preview_text_and_identity_digest() {
    let image_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sources/au/raw/IDR021.T.202609180511.png"
    );
    let output = native_command()
        .args(["cat", "--file", image_path])
        .output()
        .expect("native radiust process starts");
    let expected: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/rust-migration/cli/cat-file-text.json"
    ))
    .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stderr).unwrap(), expected["stderr"].as_str().unwrap());
    let expected_text = expected["stdout"].as_str().unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim_end(), expected_text);
}

#[test]
fn cat_rejects_scientific_render_options_for_raw_source_previews_before_network() {
    let output = native_command()
        .args(["cat", "rainviewer", "--palette", "default", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["error"]["message"]
            .as_str()
            .unwrap()
            .contains("raw image preview does not accept scientific decoding options")
    );

    let image = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sources/au/raw/IDR021.T.202609180511.png"
    );
    let local = native_command()
        .args(["cat", "--file", image, "--palette", "default", "--renderer", "text", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(local.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&local.stdout).unwrap();
    assert!(
        report["error"]["message"]
            .as_str()
            .unwrap()
            .contains("raw image preview does not accept scientific decoding options")
    );
}

#[test]
fn cat_local_image_json_keeps_the_v1_envelope() {
    let image_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sources/au/raw/IDR021.T.202609180511.png"
    );
    let output = native_command()
        .args(["cat", "--file", image_path, "--json"])
        .output()
        .expect("native radiust process starts");

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], "cat");
    assert_eq!(report["run_id"], Value::Null);
    assert_eq!(report["query"], Value::Null);
    assert_eq!(report["counts"]["items"], 0);
    assert_eq!(report["items"], serde_json::json!([]));
    assert_eq!(report["error"], Value::Null);
    assert_eq!(report["interrupted"], false);
    assert!(report["result"].as_str().unwrap().contains("display=original"));
}

#[test]
fn source_cat_uses_the_native_discovery_path_and_keeps_network_disabled_by_default() {
    let output = native_command()
        .args(["cat", "my", "--product", "composite", "--station", "peninsular", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["error"]["stage"], "validate");
    assert_eq!(report["error"]["message"], "public network access is disabled");
}

#[test]
fn multi_source_discover_aggregates_targets_with_network_off_by_default() {
    let output = native_command()
        .args(["discover", "au", "vn", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["query"]["sources"], serde_json::json!(["au", "vn"]));
    assert_eq!(report["counts"]["total"], 2);
    assert_eq!(report["counts"]["network_restricted"], 2);
}

#[test]
fn invalid_multi_source_filters_fail_before_any_network_access() {
    let output = native_command()
        .args(["discover", "all", "--product", "composite", "--json"])
        .output()
        .expect("native radiust process starts");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["error"]["stage"], "validate");
    assert_eq!(report["error"]["code"], "error");
}

#[test]
fn config_show_redacts_source_and_storage_credentials() {
    let config = unique_temp_path("config.yml");
    std::fs::write(
        &config,
        "sources:\n  au:\n    token: private-source-token\n    auth:\n      nested:\n        bearer_token: private-deep-token\n    headers:\n      - Authorization: Bearer-header-private\n      - nested:\n          cookie: private-cookie\n    label: \"control\\u001b-sequence\"\nstorage:\n  access_key: private-access\n  secret_key: private-secret\n",
    )
    .unwrap();
    let output = native_command()
        .args(["config", "show", "--conf"])
        .arg(&config)
        .arg("--json")
        .output()
        .expect("native radiust process starts");
    std::fs::remove_file(config).unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("private-source-token"));
    assert!(!stdout.contains("private-deep-token"));
    assert!(!stdout.contains("Bearer-header-private"));
    assert!(!stdout.contains("private-cookie"));
    assert!(!stdout.contains("private-access"));
    assert!(!stdout.contains("private-secret"));
    assert!(!stdout.contains('\u{1b}'));
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["result"]["sources"]["au"]["token"], "[REDACTED]");
    assert_eq!(report["result"]["sources"]["au"]["headers"][0]["Authorization"], "[REDACTED]");
    assert_eq!(report["result"]["sources"]["au"]["headers"][1]["nested"]["cookie"], "[REDACTED]");
}

#[test]
fn offline_discovery_does_not_require_optional_browser_or_ocr_binaries() {
    let empty_path = unique_temp_path("empty-path");
    std::fs::create_dir_all(&empty_path).unwrap();
    let output = native_command()
        .args(["discover", "all", "--json"])
        .env("PATH", &empty_path)
        .output()
        .expect("native radiust process starts");
    std::fs::remove_dir_all(empty_path).unwrap();
    assert!(output.status.code().is_some());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["command"], "discover");
    assert!(!stdout.to_ascii_lowercase().contains("chromium"));
    assert!(!stdout.to_ascii_lowercase().contains("tesseract"));
}

#[test]
fn doctor_checks_native_catalog_and_reports_network_probes_as_opt_in() {
    let output = native_command()
        .args(["doctor", "--source", "au", "--json"])
        .output()
        .expect("native radiust process starts");

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["command"], "doctor");
    assert_eq!(report["checks"]["native_runtime"], true);
    assert_eq!(report["checks"]["source_catalog"], true);
    assert_eq!(report["checks"]["source"], "au");
    assert_eq!(report["checks"]["network_probe"], "not_requested");
    assert_eq!(report["checks"]["ok"], true);
}

#[test]
fn doctor_network_request_reports_source_availability_without_claiming_a_probe() {
    let output = native_command()
        .args(["doctor", "--source", "uk", "--network", "--json"])
        .output()
        .expect("native radiust process starts");

    assert_eq!(output.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["checks"]["source"], "uk");
    assert_eq!(report["checks"]["source_availability"], "retired");
    assert_eq!(report["checks"]["network_probe"], "requested_but_source_probe_is_adapter_specific");
    assert_eq!(report["checks"]["ok"], true);
}

#[test]
fn doctor_reports_source_optional_capabilities_without_probe_or_secret_exposure() {
    let empty_path = unique_temp_path("doctor-empty-path");
    std::fs::create_dir_all(&empty_path).unwrap();
    let th = native_command()
        .args(["doctor", "--source", "th", "--json"])
        .env("PATH", &empty_path)
        .output()
        .expect("native radiust process starts");
    assert_eq!(th.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&th.stdout).unwrap();
    assert_eq!(report["checks"]["network_probe"], "not_requested");
    assert_eq!(report["checks"]["source_capabilities"]["tesseract"]["status"], "missing");
    assert_eq!(report["checks"]["ok"], true);

    let ph_config = unique_temp_path("doctor-ph.yml");
    std::fs::write(&ph_config, "sources:\n  ph:\n    timeline_token: private-doctor-token\n")
        .unwrap();
    let ph = native_command()
        .args(["doctor", "--source", "ph", "--json", "--conf"])
        .arg(&ph_config)
        .env("PATH", &empty_path)
        .output()
        .expect("native radiust process starts");
    std::fs::remove_file(ph_config).unwrap();
    assert_eq!(ph.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&ph.stdout).unwrap();
    assert_eq!(report["checks"]["source_capabilities"]["timeline_token"]["status"], "automatic_session");
    assert_eq!(report["checks"]["source_capabilities"]["timeline_token"]["value_exposed"], false);
    assert!(matches!(
        report["checks"]["source_capabilities"]["browser_fallback"]["status"].as_str(),
        Some("chromium_detected" | "chromium_missing")
    ));
    assert!(!String::from_utf8_lossy(&ph.stdout).contains("private-doctor-token"));

    let windy = native_command()
        .args(["doctor", "--source", "windy", "--json"])
        .env("PATH", &empty_path)
        .output()
        .expect("native radiust process starts");
    assert_eq!(windy.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&windy.stdout).unwrap();
    assert_eq!(report["checks"]["source_capabilities"]["http_acquisition"], "supported");
    assert!(matches!(
        report["checks"]["source_capabilities"]["playwright_acquisition"]["status"].as_str(),
        Some("chromium_detected" | "chromium_missing")
    ));

    std::fs::remove_dir_all(empty_path).unwrap();
}

#[test]
fn cache_status_uses_an_explicit_native_cache_root() {
    let root = unique_temp_path("cache");
    let cache = root.join("cache");
    let output_root = root.join("output");
    let output = native_command()
        .args(["cache", "status", "--cache-dir"])
        .arg(&cache)
        .arg("--output-root")
        .arg(&output_root)
        .arg("--json")
        .output()
        .expect("native radiust process starts");
    assert_eq!(output.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["entries"], 0);
    assert_eq!(report["bytes"], 0);
    assert!(report["root"].as_str().unwrap().ends_with("/cache"));
    std::fs::remove_dir_all(root).unwrap();
}
