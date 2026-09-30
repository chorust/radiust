use radiust_core::limits::Limits;
use radiust_core::model::{Grid, RadarDataset, RadarField};
use radiust_core::output::geotiff::write_field as write_geotiff_field;
use radiust_core::output::zarr::write_dataset as write_zarr_dataset;
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

const LOCAL_PNG: &str = "../../tests/fixtures/sources/au/raw/IDR021.T.202609180511.png";

fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn run_cat(args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_radiust"));
    command.env_clear().args(args);
    command.output().expect("native radiust process starts")
}

#[cfg(unix)]
fn run_cat_on_pty(args: &[&str], no_color: bool) -> (std::process::ExitStatus, Vec<u8>) {
    use std::os::fd::FromRawFd;

    let mut master_fd = -1;
    let mut slave_fd = -1;
    let mut window = libc::winsize { ws_row: 24, ws_col: 80, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: openpty writes two newly-owned file descriptors into the outputs.
    let opened = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut window,
        )
    };
    assert_eq!(opened, 0, "openpty succeeds");
    // SAFETY: both descriptors were created by openpty and each is wrapped once.
    let mut master = unsafe { File::from_raw_fd(master_fd) };
    // SAFETY: see above; stdout, stderr, and stdin receive independent duplicates.
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    let mut command = Command::new(env!("CARGO_BIN_EXE_radiust"));
    command
        .env_clear()
        .env("COLUMNS", "80")
        .args(args)
        .stdin(Stdio::from(slave.try_clone().expect("clone pty slave for stdin")))
        .stdout(Stdio::from(slave.try_clone().expect("clone pty slave for stdout")))
        .stderr(Stdio::from(slave));
    if no_color {
        command.env("NO_COLOR", "1");
    }
    let mut child = command.spawn().expect("native radiust process starts on pty");
    drop(command);
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        master.read_to_end(&mut output).expect("read pty output");
        output
    });
    let status = child.wait().expect("native radiust process exits");
    (status, reader.join().expect("pty reader joins"))
}

fn expected_text() -> String {
    let source_fixture: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/sources/au/fixture.json"))
            .unwrap();
    let sha256 = source_fixture["frames"][0]["artifacts"][0]["sha256"].as_str().unwrap();
    format!(
        "image path=IDR021.T.202609180511.png raw source=unknown product=unknown station=unknown time=unknown units=unknown format=PNG size=512x512 sha256={sha256} display=original rule=unknown; original source pixels preserved"
    )
}

fn write_multitime_netcdf(path: &Path) {
    let mut file = netcdf::create(path).expect("create local NetCDF fixture");
    file.add_dimension("time", 2).expect("add time dimension");
    file.add_dimension("y", 2).expect("add y dimension");
    file.add_dimension("x", 2).expect("add x dimension");
    {
        let mut time = file.add_variable::<i64>("time", &["time"]).expect("add time variable");
        time.put_attribute("units", "seconds since 1970-01-01 00:00:00 UTC")
            .expect("set time units");
        time.put_values(&[1_790_380_800_i64, 1_790_388_000_i64], ..).expect("write time values");
    }
    {
        let mut field = file
            .add_variable::<f32>("reflectivity", &["time", "y", "x"])
            .expect("add reflectivity variable");
        field.put_attribute("units", "dBZ").expect("set reflectivity units");
        field
            .put_values(&[1.0_f32, 2.0, 3.0, 4.0, 11.0, 12.0, 13.0, 14.0], ..)
            .expect("write reflectivity values");
    }
    file.close().expect("close local NetCDF fixture");
}

fn local_field(name: &str, offset: f32) -> RadarField {
    RadarField {
        name: name.into(),
        values: vec![offset, offset + 1.0, offset + 2.0, offset + 3.0],
        shape: vec![2, 2],
        quality: vec![0, 1, 0, 4],
        units: Some("dBZ".into()),
        valid_time: "2026-09-26T01:02:03Z".into(),
        grid: Grid {
            shape: vec![2, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![120.0, 120.5],
            y: vec![23.0, 23.5],
            affine: None,
        },
        provenance: vec!["cat-fixture".into()],
    }
}

fn write_multivariable_zarr(path: &Path) {
    let first = local_field("reflectivity", 1.0);
    let second = local_field("rain_rate", 5.0);
    let dataset = RadarDataset {
        fields: vec![first, second],
        valid_time: "2026-09-26T01:02:03Z".into(),
        source: "fixture".into(),
    };
    write_zarr_dataset(&dataset, path, &Limits::default()).expect("write local Zarr fixture");
}

#[test]
fn cat_local_raw_image_keeps_non_tty_text_and_original_bytes() {
    let path = fixture_path(LOCAL_PNG);
    let path_text = path.to_str().expect("fixture path is valid UTF-8");
    let output = run_cat(&["cat", "--file", path_text, "--renderer", "text"]);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout, format!("{}\n", expected_text()));
    assert!(!stdout.contains('\u{1b}'), "piped text output contains no terminal escapes");
}

#[test]
fn explicit_ansi_renderer_requires_a_tty() {
    let path = fixture_path(LOCAL_PNG);
    let path_text = path.to_str().expect("fixture path is valid UTF-8");
    let output = run_cat(&["cat", "--file", path_text, "--renderer", "ansi"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr).unwrap().contains("renderer ansi requires a TTY"));
}

#[test]
fn decoded_mode_is_parsed_and_keeps_the_native_source_capability_gate() {
    let output = run_cat(&["cat", "my", "--decoded", "--renderer", "text"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("native --decoded preview supports only"), "{stderr}");
    assert!(stderr.contains("selected my"), "{stderr}");
}

#[test]
fn legacy_display_requires_a_source_identity() {
    let path = fixture_path(LOCAL_PNG);
    let path_text = path.to_str().expect("fixture path is valid UTF-8");
    let output = run_cat(&["cat", "--file", path_text, "--legacy-display", "--renderer", "text"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr).unwrap().contains("--legacy-display requires SOURCE"));
}

#[cfg(unix)]
#[test]
fn cat_tty_preview_restores_sgr_and_never_switches_terminal_modes() {
    let path = fixture_path(LOCAL_PNG);
    let path_text = path.to_str().expect("fixture path is valid UTF-8");
    let (status, output) =
        run_cat_on_pty(&["cat", "--file", path_text, "--renderer", "auto"], false);

    assert_eq!(status.code(), Some(0));
    let output = String::from_utf8(output).expect("terminal renderer emits UTF-8");
    assert!(output.contains("\u{1b}["), "TTY preview should use SGR colors");
    assert!(output.contains("\u{1b}[0m"), "each rendered row restores SGR state");
    assert!(!output.contains("?1049h"), "renderer must not enter an alternate screen");
    assert!(!output.contains("?1049l"), "renderer must not leave an alternate screen");
    assert!(!output.contains("?25l"), "renderer must not hide the cursor");
    assert!(!output.contains("?2004h"), "renderer must not change bracketed-paste mode");
}

#[cfg(unix)]
#[test]
fn cat_tty_no_color_preview_writes_no_terminal_escape_sequences() {
    let path = fixture_path(LOCAL_PNG);
    let path_text = path.to_str().expect("fixture path is valid UTF-8");
    let (status, output) =
        run_cat_on_pty(&["cat", "--file", path_text, "--renderer", "auto"], true);

    assert_eq!(status.code(), Some(0));
    let output = String::from_utf8(output).expect("terminal renderer emits UTF-8");
    assert!(!output.contains('\u{1b}'));
    assert!(output.contains("display=original"));
}

#[test]
fn quiet_keeps_cat_preview_output_visible() {
    let path = fixture_path(LOCAL_PNG);
    let path_text = path.to_str().expect("fixture path is valid UTF-8");
    let output = run_cat(&["--quiet", "cat", "--file", path_text, "--renderer", "text"]);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), format!("{}\n", expected_text()));
}

#[test]
fn cat_local_raw_image_json_reports_the_same_original_image_summary() {
    let path = fixture_path(LOCAL_PNG);
    let path_text = path.to_str().expect("fixture path is valid UTF-8");
    let output = run_cat(&["cat", "--file", path_text, "--json"]);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).expect("CLI emits JSON");
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], "cat");
    assert_eq!(report["items"], serde_json::json!([]));
    assert_eq!(report["result"], expected_text());
    assert_eq!(report["error"], Value::Null);
    assert_eq!(report["interrupted"], false);
}

#[test]
fn cat_local_multitime_netcdf_requires_selection_and_shows_the_selected_field() {
    let directory = tempfile::tempdir().expect("create temporary fixture directory");
    let path = directory.path().join("series.nc");
    write_multitime_netcdf(&path);
    let path_text = path.to_str().expect("fixture path is valid UTF-8");
    let ambiguous = run_cat(&["cat", "--file", path_text, "--variable", "reflectivity"]);

    assert_eq!(ambiguous.status.code(), Some(2));
    assert!(ambiguous.stdout.is_empty());
    let stderr = String::from_utf8(ambiguous.stderr).unwrap();
    assert!(stderr.contains("time selection is ambiguous"), "{stderr}");
    assert!(!stderr.contains('\u{1b}'), "piped error output contains no terminal escapes");

    let missing_variable =
        run_cat(&["cat", "--file", path_text, "--variable", "rain_rate", "--renderer", "text"]);
    assert_eq!(missing_variable.status.code(), Some(2));
    assert!(missing_variable.stdout.is_empty());
    assert!(
        String::from_utf8(missing_variable.stderr).unwrap().contains("rain_rate"),
        "missing-variable errors name the requested field"
    );

    let invalid_time = run_cat(&[
        "cat",
        "--file",
        path_text,
        "--variable",
        "reflectivity",
        "--at",
        "not-a-time",
        "--renderer",
        "text",
    ]);
    assert_eq!(invalid_time.status.code(), Some(2));
    assert!(
        String::from_utf8(invalid_time.stderr).unwrap().contains("time must be ISO-8601"),
        "invalid-time errors explain the accepted format"
    );

    let absent_time = run_cat(&[
        "cat",
        "--file",
        path_text,
        "--variable",
        "reflectivity",
        "--at",
        "2026-09-26T03:00:00Z",
        "--renderer",
        "text",
    ]);
    assert_eq!(absent_time.status.code(), Some(2));
    assert!(
        String::from_utf8(absent_time.stderr).unwrap().contains("has no frame"),
        "absent-time errors explain that no matching frame exists"
    );

    let selected = run_cat(&[
        "cat",
        "--file",
        path_text,
        "--variable",
        "reflectivity",
        "--at",
        "2026-09-26T02:00:00Z",
        "--renderer",
        "text",
    ]);

    assert_eq!(selected.status.code(), Some(0));
    assert!(selected.stderr.is_empty());
    assert_eq!(
        String::from_utf8(selected.stdout).unwrap(),
        "field path=series.nc format=NetCDF4 variable=reflectivity time=2026-09-26T02:00:00Z units=dBZ size=2x2 crs=unknown display=decoded\n"
    );
}

#[test]
fn cat_local_geotiff_reads_the_complete_field_group_and_validates_selectors() {
    let directory = tempfile::tempdir().expect("create temporary fixture directory");
    let path = directory.path().join("field.tif");
    let paths = write_geotiff_field(&local_field("reflectivity", 1.0), &path, &Limits::default())
        .expect("write local GeoTIFF fixture");
    let path_text = path.to_str().expect("fixture path is valid UTF-8");
    let selected = run_cat(&["cat", "--file", path_text, "--renderer", "text"]);

    assert_eq!(selected.status.code(), Some(0));
    assert!(selected.stderr.is_empty());
    let summary = String::from_utf8(selected.stdout).unwrap();
    assert!(summary.contains("format=GeoTIFF variable=reflectivity"), "{summary}");
    assert!(summary.contains("time=2026-09-26T01:02:03Z"), "{summary}");
    assert!(summary.contains("units=dBZ size=2x2 crs=EPSG:4326 display=decoded"), "{summary}");

    let wrong_variable =
        run_cat(&["cat", "--file", path_text, "--variable", "rain_rate", "--renderer", "text"]);
    assert_eq!(wrong_variable.status.code(), Some(2));
    assert!(String::from_utf8(wrong_variable.stderr).unwrap().contains("does not match requested"));

    let wrong_time = run_cat(&[
        "cat",
        "--file",
        path_text,
        "--at",
        "2026-09-26T02:00:00+01:00",
        "--renderer",
        "text",
    ]);
    assert_eq!(wrong_time.status.code(), Some(2));
    assert!(String::from_utf8(wrong_time.stderr).unwrap().contains("has no frame"));

    std::fs::remove_file(&paths[1]).expect("remove the quality sidecar");
    let missing_sidecar = run_cat(&["cat", "--file", path_text, "--renderer", "text"]);
    assert_eq!(
        missing_sidecar.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&missing_sidecar.stderr)
    );
}

#[test]
fn cat_local_multivariable_zarr_requires_a_variable_and_preserves_selected_field_identity() {
    let directory = tempfile::tempdir().expect("create temporary fixture directory");
    let path = directory.path().join("fields.zarr");
    write_multivariable_zarr(&path);
    let path_text = path.to_str().expect("fixture path is valid UTF-8");

    let ambiguous = run_cat(&["cat", "--file", path_text, "--renderer", "text"]);
    assert_eq!(ambiguous.status.code(), Some(2));
    assert!(String::from_utf8(ambiguous.stderr).unwrap().contains("selection is ambiguous"));

    let selected =
        run_cat(&["cat", "--file", path_text, "--variable", "rain_rate", "--renderer", "text"]);
    assert_eq!(selected.status.code(), Some(0), "{}", String::from_utf8_lossy(&selected.stderr));
    assert!(selected.stderr.is_empty());
    let summary = String::from_utf8(selected.stdout).unwrap();
    assert!(summary.contains("format=Zarr v2 variable=rain_rate"), "{summary}");
    assert!(summary.contains("time=2026-09-26T01:02:03Z"), "{summary}");
    assert!(summary.contains("units=dBZ size=2x2 crs=EPSG:4326 display=decoded"), "{summary}");

    let wrong_variable =
        run_cat(&["cat", "--file", path_text, "--variable", "missing_field", "--renderer", "text"]);
    assert_eq!(wrong_variable.status.code(), Some(2));
    assert!(String::from_utf8(wrong_variable.stderr).unwrap().contains("variable was not found"));

    let wrong_time = run_cat(&[
        "cat",
        "--file",
        path_text,
        "--variable",
        "rain_rate",
        "--at",
        "2026-09-26T01:03:03Z",
        "--renderer",
        "text",
    ]);
    assert_eq!(wrong_time.status.code(), Some(2));
    assert!(String::from_utf8(wrong_time.stderr).unwrap().contains("has no frame"));
}
