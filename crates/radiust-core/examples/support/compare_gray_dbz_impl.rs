use radiust_core::dbz;
use radiust_core::gray::{self, GrayDecision};
use radiust_core::limits::Limits;
use radiust_core::model::{ArtifactReceipt, FrameRef, RawArtifact, RawFrame};
use radiust_core::raster::{
    QUALITY_INTERPOLATED, QUALITY_MISSING, QUALITY_OUTSIDE_COVERAGE, QUALITY_RECOVERED,
    QUALITY_UNKNOWN_COLOR, RasterResultData,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_INPUT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PIXELS: u64 = 100_000_000;

fn text<'a>(value: Option<&'a Value>, name: &str) -> Result<&'a str, String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing or invalid {name}"))
}

fn confined_file(root: &Path, value: Option<&Value>, name: &str) -> Result<PathBuf, String> {
    let relative = Path::new(text(value, name)?);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_))
        })
        || relative.to_string_lossy().contains([':', '\\'])
    {
        return Err(format!("{name} is not a confined relative path"));
    }
    let path = root.join(relative).canonicalize().map_err(|_| format!("{name} is unavailable"))?;
    if !path.starts_with(root) || !path.is_file() {
        return Err(format!("{name} escapes its fixture root or is not a file"));
    }
    Ok(path)
}

fn read_confined(root: &Path, value: Option<&Value>, name: &str) -> Result<Vec<u8>, String> {
    let path = confined_file(root, value, name)?;
    let metadata = fs::metadata(&path).map_err(|_| format!("{name} is unavailable"))?;
    if metadata.len() > MAX_INPUT_BYTES {
        return Err(format!("{name} exceeds its byte limit"));
    }
    fs::read(path).map_err(|_| format!("{name} could not be read"))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn sha_u16(values: &[u16]) -> String {
    let mut bytes = Vec::with_capacity(values.len().saturating_mul(2));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    sha256(&bytes)
}

fn count_flag(values: &[u16], flag: u16) -> usize {
    values.iter().filter(|value| **value & flag != 0).count()
}

fn frame_context(entry: &Value, fixture_only_revision: &str) -> Result<FrameRef, String> {
    let source = text(entry.get("source"), "source")?;
    let product = text(entry.get("product"), "product")?;
    let metadata = entry.get("frame").filter(|value| value.is_object());
    if metadata.is_some_and(|frame| {
        frame.get("source").and_then(Value::as_str).is_some_and(|value| value != source)
            || frame.get("product").and_then(Value::as_str).is_some_and(|value| value != product)
    }) {
        return Err("fixture frame context does not match its source path".into());
    }
    let station = metadata
        .and_then(|frame| frame.get("station"))
        .and_then(Value::as_str)
        .or_else(|| entry.get("station").and_then(Value::as_str))
        .map(str::to_owned);
    let valid_time = metadata
        .and_then(|frame| frame.get("valid_time"))
        .and_then(Value::as_str)
        .unwrap_or("2000-01-01T00:00:00Z");
    let revision = metadata
        .and_then(|frame| frame.get("revision"))
        .and_then(Value::as_str)
        .unwrap_or(fixture_only_revision);
    let frame_index = metadata.and_then(|frame| frame.get("frame_index")).cloned();
    let mut locator = json!({"fixture_context_only": true});
    if let Some(index) = frame_index.filter(Value::is_number) {
        locator["frame_index"] = index;
        locator["footer_observation_times"] = json!([valid_time]);
    }
    let mut frame = FrameRef {
        source: source.into(),
        product: product.into(),
        station,
        valid_time: valid_time.into(),
        base_time: None,
        logical_id: String::new(),
        revision: Some(revision.into()),
        locator_version: "test-fixture-v1".into(),
        locator,
    };
    frame.logical_id = radiust_core::identity::logical_id(&frame)
        .map_err(|_| "fixture-only frame context could not be identified".to_owned())?;
    Ok(frame)
}

fn raw_frame(entry: &Value, root: &Path) -> Result<(RawFrame, String), String> {
    let inputs = entry
        .get("inputs")
        .and_then(Value::as_array)
        .filter(|inputs| inputs.len() == 1)
        .ok_or("passed path requires exactly one source artifact")?;
    let input = &inputs[0];
    let bytes = read_confined(root, input.get("path"), "source artifact")?;
    let digest = sha256(&bytes);
    if text(input.get("sha256"), "source artifact SHA-256")? != digest {
        return Err("source artifact SHA-256 does not match the manifest".into());
    }
    let name = text(input.get("name"), "artifact name")?.to_owned();
    let media_type = text(input.get("media_type"), "artifact media type")?.to_owned();
    let mut file = tempfile::NamedTempFile::new()
        .map_err(|_| "temporary fixture artifact could not be created".to_owned())?;
    file.write_all(&bytes).map_err(|_| "temporary fixture artifact could not be written")?;
    let path = file.into_temp_path();
    let frame = frame_context(entry, &format!("fixture-only:{digest}"))?;
    Ok((
        RawFrame {
            frame,
            artifacts: vec![RawArtifact {
                receipt: ArtifactReceipt {
                    name,
                    media_type,
                    size_bytes: bytes.len() as u64,
                    sha256: digest.clone(),
                },
                path,
            }],
            private_locator: None,
        },
        digest,
    ))
}

fn blocked_entry(entry: &Value, reason: &str) -> Value {
    json!({
        "path_id": entry.get("path_id"),
        "source": entry.get("source"),
        "product": entry.get("product"),
        "manifest_gray_status": entry.get("status"),
        "gray_status": "blocked",
        "dbz_status": "blocked",
        "blocked_reasons": entry.get("blocked_reasons").cloned().unwrap_or_else(|| json!([reason])),
        "time_status": "unknown",
        "geometry_status": "unknown",
        "live_status": "not_tested"
    })
}

fn validate_passed_entry(entry: &Value, root: &Path, limits: &Limits) -> Value {
    let path_id = entry.get("path_id").and_then(Value::as_str).unwrap_or("unknown");
    let attempt = (|| -> Result<Value, String> {
        let (raw, input_sha256) = raw_frame(entry, root)?;
        let baseline_bytes = read_confined(root, entry.get("baseline_path"), "gray baseline")?;
        let expected_baseline_sha = text(entry.get("baseline_hash"), "gray baseline SHA-256")?;
        if sha256(&baseline_bytes) != expected_baseline_sha {
            return Err("gray baseline SHA-256 does not match the manifest".into());
        }
        let baseline = image::load_from_memory(&baseline_bytes)
            .map_err(|_| "gray baseline could not be decoded")?
            .to_rgba8();
        let gray = match gray::decode_source_frame(&raw, limits)
            .map_err(|error| format!("source gray decoder failed: {error}"))?
        {
            GrayDecision::Applied(gray) => gray,
            GrayDecision::Unavailable { reason, .. } => {
                return Err(format!("verified path returned unavailable: {reason}"));
            }
        };
        let shape_matches = baseline.dimensions() == (gray.width, gray.height);
        let gray_pixel_diff_count = if shape_matches {
            gray.rgba
                .chunks_exact(4)
                .zip(baseline.as_raw().chunks_exact(4))
                .filter(|(left, right)| left != right)
                .count()
        } else {
            usize::MAX
        };
        let gray_sha256 = sha256(&gray.rgba);
        let valid_time_status = if entry
            .get("frame")
            .and_then(|frame| frame.get("valid_time"))
            .and_then(Value::as_str)
            .is_some()
        {
            "fixture_frame_metadata"
        } else {
            "unknown"
        };
        let time_status = valid_time_status;
        let geometry_status = if gray
            .geometry
            .as_ref()
            .is_some_and(|geometry| geometry.mapping_complete && geometry.crs.is_some())
        {
            "verified_mapping"
        } else {
            "unknown"
        };
        let gray_rgba = gray.rgba.clone();
        let result = dbz::decode_verified_gray_frame(gray, limits)
            .map_err(|error| format!("verified source dBZ decoder failed: {error}"))?;
        let RasterResultData::Pixel(field) = result.data else {
            return Err("source gray conversion returned a non-pixel result".into());
        };
        let invalid_mask = QUALITY_MISSING | QUALITY_OUTSIDE_COVERAGE | QUALITY_UNKNOWN_COLOR;
        let mut valid_pixels = 0_u64;
        let mut invalid_pixels = 0_u64;
        let mut formula_mismatch_count = 0_u64;
        for ((rgba, &value), &quality) in
            gray_rgba.chunks_exact(4).zip(&field.values).zip(&field.quality)
        {
            let is_invalid = quality & invalid_mask != 0;
            if is_invalid {
                invalid_pixels += 1;
                if !value.is_nan() {
                    formula_mismatch_count += 1;
                }
                continue;
            }
            valid_pixels += 1;
            let expected = f32::from(rgba[0].min(224)) * (5.0 / 16.0);
            if !value.is_finite() || (value - expected).abs() > 0.000_001 {
                formula_mismatch_count += 1;
            }
        }
        let clipped_pixel_count = field.processing.clipped_pixel_count.unwrap_or(0);
        let valid_clipped_pixel_count = field.processing.valid_clipped_pixel_count.unwrap_or(0);
        let quality_counts = json!({
            "missing": count_flag(&field.quality, QUALITY_MISSING),
            "outside_coverage": count_flag(&field.quality, QUALITY_OUTSIDE_COVERAGE),
            "unknown_color": count_flag(&field.quality, QUALITY_UNKNOWN_COLOR),
            "recovered": count_flag(&field.quality, QUALITY_RECOVERED),
            "interpolated": count_flag(&field.quality, QUALITY_INTERPOLATED)
        });
        Ok(json!({
            "path_id": path_id,
            "source": entry.get("source"),
            "product": entry.get("product"),
            "manifest_gray_status": "passed",
            "gray_status": if shape_matches && gray_pixel_diff_count == 0 { "passed" } else { "difference_pending" },
            "gray_shape": [field.height, field.width],
            "baseline_shape": [baseline.height(), baseline.width()],
            "gray_pixel_diff_count": if shape_matches { json!(gray_pixel_diff_count) } else { Value::Null },
            "gray_sha256": gray_sha256,
            "input_sha256": input_sha256,
            "dbz_status": if formula_mismatch_count == 0 { "verified" } else { "difference_pending" },
            "units": result.mode_info.units,
            "valid_pixels": valid_pixels,
            "invalid_pixels": invalid_pixels,
            "formula_mismatch_count": formula_mismatch_count,
            "quality_sha256": sha_u16(&field.quality),
            "origin_quality_sha256": field.origin_quality.as_deref().map(sha_u16),
            "adjustment_sha256": field.encoding_adjustment.as_deref().map(sha256),
            "quality_flag_counts": quality_counts,
            "clipped_pixel_count": clipped_pixel_count,
            "valid_clipped_pixel_count": valid_clipped_pixel_count,
            "time_status": time_status,
            "geometry_status": geometry_status,
            "live_status": "not_tested",
            "fixture_context_only": true
        }))
    })();
    match attempt {
        Ok(report) => report,
        Err(reason) => {
            eprintln!("gray/dBZ path {path_id}: {reason}");
            blocked_entry(entry, &reason)
        }
    }
}

pub fn run() -> Result<i32, String> {
    let mut args = std::env::args().skip(1);
    let mut manifest = PathBuf::from("tests/fixtures/legacy-display/manifest.json");
    let mut fixture_root: Option<PathBuf> = None;
    let mut report = PathBuf::from("validation-results/gray-dbz-paths.json");
    while let Some(option) = args.next() {
        let value = args.next().ok_or_else(|| format!("{option} requires a value"))?;
        match option.as_str() {
            "--manifest" => manifest = value.into(),
            "--fixture-root" => fixture_root = Some(value.into()),
            "--report" => report = value.into(),
            _ => return Err(format!("unknown option {option}")),
        }
    }
    let manifest = if manifest.is_absolute() {
        manifest
    } else {
        std::env::current_dir().map_err(|_| "working directory is unavailable")?.join(manifest)
    };
    let manifest = manifest.canonicalize().map_err(|_| "manifest is unavailable")?;
    if fs::metadata(&manifest).map_err(|_| "manifest is unavailable")?.len() > MAX_MANIFEST_BYTES {
        return Err("manifest exceeds its byte limit".into());
    }
    let root = fixture_root.unwrap_or_else(|| manifest.parent().unwrap_or(Path::new(".")).into());
    let root = root.canonicalize().map_err(|_| "fixture root is unavailable")?;
    if !manifest.starts_with(&root) {
        return Err("manifest is outside its fixture root".into());
    }
    let input: Value =
        serde_json::from_slice(&fs::read(&manifest).map_err(|_| "manifest could not be read")?)
            .map_err(|_| "manifest JSON is invalid")?;
    if input.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("manifest schema_version must be 1".into());
    }
    let entries = input
        .get("entries")
        .and_then(Value::as_array)
        .filter(|entries| !entries.is_empty())
        .ok_or("manifest requires a nonempty entries array")?;
    let limits = Limits {
        max_artifact_bytes: MAX_INPUT_BYTES,
        max_frame_bytes: MAX_INPUT_BYTES,
        max_pixels: MAX_PIXELS,
        max_temp_bytes: 2 * MAX_PIXELS * 16,
        ..Limits::default()
    };
    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::with_capacity(entries.len());
    for entry in entries {
        let path_id = text(entry.get("path_id"), "path_id")?;
        if !seen.insert(path_id.to_owned()) {
            return Err("manifest contains duplicate path_id".into());
        }
        match entry.get("status").and_then(Value::as_str) {
            Some("passed") => paths.push(validate_passed_entry(entry, &root, &limits)),
            Some("blocked") => {
                paths.push(blocked_entry(entry, "path is blocked by its evidence manifest"))
            }
            _ => paths
                .push(blocked_entry(entry, "path status is not an enabled passed/blocked state")),
        }
    }
    let gray_passed = paths.iter().filter(|path| path["gray_status"] == "passed").count();
    let gray_pending =
        paths.iter().filter(|path| path["gray_status"] == "difference_pending").count();
    let gray_blocked = paths.iter().filter(|path| path["gray_status"] == "blocked").count();
    let dbz_verified = paths.iter().filter(|path| path["dbz_status"] == "verified").count();
    let dbz_blocked = paths.iter().filter(|path| path["dbz_status"] == "blocked").count();
    let runner_exit_code =
        if gray_pending == 0 && gray_blocked == 0 && dbz_verified == entries.len() { 0 } else { 1 };
    let report_value = json!({
        "schema_version": 1,
        "feature": "005-gray-dbz-decoding",
        "evidence_kind": "offline_fixture_comparison",
        "fixture_context_only": true,
        "source_observation_time_claimed": false,
        "geographic_truth_claimed": false,
        "live_acquisition_tested": false,
        "counts": {
            "total": paths.len(),
            "gray_passed": gray_passed,
            "gray_difference_pending": gray_pending,
            "gray_blocked": gray_blocked,
            "dbz_verified": dbz_verified,
            "dbz_blocked": dbz_blocked
        },
        "runner_exit": {
            "code": runner_exit_code,
            "reason": if runner_exit_code == 0 { "all manifest paths passed" } else { "blocked or difference-pending paths remain in the evidence manifest" }
        },
        "direct_native_dbz_evidence": {
            "status": "verified_by_separate_offline_contracts",
            "paths": ["rainviewer/composite", "tw/grid", "rdcap/reflectivity"],
            "evidence": ["crates/radiust-core/tests/dbz_native.rs", "tests/integration/test_gray_dbz_sdk.py"]
        },
        "paths": paths
    });
    let output = if report.is_absolute() {
        report
    } else {
        std::env::current_dir().map_err(|_| "working directory is unavailable")?.join(report)
    };
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|_| "report directory could not be created")?;
    }
    fs::write(
        &output,
        serde_json::to_vec_pretty(&report_value).map_err(|_| "report could not be encoded")?,
    )
    .map_err(|_| "report could not be written")?;
    println!("gray/dbz paths: {}; report={}", report_value["counts"], output.display());
    Ok(runner_exit_code)
}
