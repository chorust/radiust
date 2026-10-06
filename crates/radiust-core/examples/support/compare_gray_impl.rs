use radiust_core::gray::apply_for_source;
use radiust_core::limits::Limits;
use radiust_core::preview::preview_bytes;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};

const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;
const MAX_INPUT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PIXELS: u64 = 100_000_000;

fn text<'a>(value: Option<&'a Value>, name: &str) -> Result<&'a str, String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing or invalid {name}"))
}

fn confined_path(root: &Path, value: Option<&Value>, name: &str) -> Result<PathBuf, String> {
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

fn read_confined(
    root: &Path,
    value: Option<&Value>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>, String> {
    let path = confined_path(root, value, name)?;
    let metadata = fs::metadata(&path).map_err(|_| format!("{name} is unavailable"))?;
    if metadata.len() > limit {
        return Err(format!("{name} exceeds its byte limit"));
    }
    fs::read(path).map_err(|_| format!("{name} could not be read"))
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn valid_hash<'a>(value: Option<&'a Value>, name: &str) -> Result<&'a str, String> {
    let value = text(value, name)?;
    if value.len() != 64
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(format!("{name} is not a lowercase SHA-256"));
    }
    Ok(value)
}

fn compare(
    actual: &[u8],
    actual_width: u32,
    actual_height: u32,
    baseline: &[u8],
    baseline_width: u32,
    baseline_height: u32,
) -> Map<String, Value> {
    let shape = json!([actual_height, actual_width]);
    let baseline_shape = json!([baseline_height, baseline_width]);
    let mut result = Map::new();
    result.insert("shape".into(), shape.clone());
    result.insert("baseline_shape".into(), baseline_shape.clone());
    result.insert("output_hash".into(), json!(digest(actual)));
    if shape != baseline_shape || actual.len() != baseline.len() {
        result.insert("pixel_diff_count".into(), Value::Null);
        for name in ["alpha", "background", "missing"] {
            result.insert(format!("{name}_comparison"), json!("shape differs"));
            result.insert(format!("{name}_diff_count"), Value::Null);
        }
        return result;
    }

    let mut pixel_diff_count = 0_u64;
    let mut alpha_diff_count = 0_u64;
    let mut background_diff_count = 0_u64;
    let mut missing_diff_count = 0_u64;
    for (left, right) in actual.chunks_exact(4).zip(baseline.chunks_exact(4)) {
        pixel_diff_count += u64::from(left != right);
        alpha_diff_count += u64::from(left[3] != right[3]);
        missing_diff_count += u64::from((left[3] == 0) != (right[3] == 0));
        let left_zero = left[3] != 0 && left[..3] == [0, 0, 0];
        let right_zero = right[3] != 0 && right[..3] == [0, 0, 0];
        background_diff_count += u64::from(left_zero != right_zero);
    }
    result.insert("pixel_diff_count".into(), json!(pixel_diff_count));
    for (name, count) in [
        ("alpha", alpha_diff_count),
        ("background", background_diff_count),
        ("missing", missing_diff_count),
    ] {
        result.insert(format!("{name}_diff_count"), json!(count));
        result.insert(
            format!("{name}_comparison"),
            json!(if count == 0 { "identical" } else { "different" }),
        );
    }
    result
}

fn validate_entry(entry: &Value, root: &Path, limits: &Limits) -> Value {
    let source = text(entry.get("source"), "source").unwrap_or_default();
    let product = text(entry.get("product"), "product").unwrap_or_default();
    let path_id = text(entry.get("path_id"), "path_id").unwrap_or_default();
    let mut result = json!({
        "source": source,
        "product": product,
        "path_id": path_id,
        "status": "blocked",
        "blocked_reasons": [],
        "scientific_status_unchanged": true,
        "rule_version": null,
        "config_hash": null,
        "input_hashes": [],
        "output_hash": null,
        "baseline_identity": null,
        "baseline_hash": null,
    });

    let Some(status) = entry.get("status").and_then(Value::as_str) else {
        result["blocked_reasons"] = json!(["manifest entry has no valid status"]);
        return result;
    };
    if status == "blocked" {
        let reasons = entry.get("blocked_reasons").and_then(Value::as_array);
        if let Some(reasons) = reasons.filter(|reasons| !reasons.is_empty()) {
            result["blocked_reasons"] = Value::Array(reasons.clone());
        } else {
            result["blocked_reasons"] = json!(["blocked entry must enumerate missing evidence"]);
        }
        return result;
    }

    let attempt = (|| -> Result<(Map<String, Value>, bool, String), String> {
        if status != "candidate" && status != "difference_pending" && status != "passed" {
            return Err("manifest entry status is unrecognized".into());
        }
        if path_id != format!("{source}/{product}")
            && !path_id.starts_with(&format!("{source}/{product}/"))
        {
            return Err("path_id does not belong to source/product".into());
        }
        text(entry.get("baseline_identity"), "old baseline identity")?;
        let baseline_hash = valid_hash(entry.get("baseline_hash"), "old baseline")?.to_owned();
        text(entry.get("license_basis"), "lawful sample basis")?;
        text(entry.get("sample_provenance"), "canonical raw provenance")?;

        let rule_path = read_confined(root, entry.get("rule_path"), "rule path", MAX_JSON_BYTES)?;
        let rule: Value =
            serde_json::from_slice(&rule_path).map_err(|_| "display rule JSON is invalid")?;
        let rule_version = text(entry.get("rule_version"), "rule version")?.to_owned();
        let config_hash = valid_hash(entry.get("config_hash"), "rule configuration")?.to_owned();
        if rule.get("source").and_then(Value::as_str) != Some(source)
            || rule.get("product").and_then(Value::as_str) != Some(product)
            || rule.get("path_id").and_then(Value::as_str) != Some(path_id)
            || rule.get("rule_version").and_then(Value::as_str) != Some(rule_version.as_str())
            || rule.get("config_hash").and_then(Value::as_str) != Some(config_hash.as_str())
        {
            return Err("display rule identity or fingerprint differs from manifest".into());
        }

        let baseline_bytes =
            read_confined(root, entry.get("baseline_path"), "old baseline", MAX_INPUT_BYTES)?;
        if digest(&baseline_bytes) != baseline_hash {
            return Err("old baseline SHA-256 does not match".into());
        }
        let baseline = preview_bytes(&baseline_bytes, limits)
            .map_err(|_| "old baseline could not be decoded")?;
        if baseline.format != "PNG" {
            return Err("old baseline is not PNG".into());
        }

        let inputs = entry
            .get("inputs")
            .and_then(Value::as_array)
            .filter(|inputs| !inputs.is_empty())
            .ok_or("manifest requires at least one source-matched input")?;
        if inputs.len() != 1 {
            return Err("Rust gray parity currently requires one verified image artifact".into());
        }
        let item = &inputs[0];
        let input_bytes = read_confined(root, item.get("path"), "raw input", MAX_INPUT_BYTES)?;
        let input_hash = valid_hash(item.get("sha256"), "raw input")?.to_owned();
        if digest(&input_bytes) != input_hash {
            return Err("raw input SHA-256 does not match".into());
        }
        let decoded =
            preview_bytes(&input_bytes, limits).map_err(|_| "raw input could not be decoded")?;
        let declared_format = text(item.get("format"), "input format")?;
        if decoded.format != declared_format {
            return Err("fixture declared format differs from image bytes".into());
        }
        let station = entry.get("station").and_then(Value::as_str);
        let actual = apply_for_source(
            source,
            product,
            station,
            &decoded.format,
            decoded.preview.width,
            decoded.preview.height,
            &decoded.preview.rgba,
            limits,
        )
        .map_err(|error| format!("Rust gray display transform rejected the fixture: {error}"))?;
        let comparison = compare(
            &actual.rgba,
            actual.width,
            actual.height,
            &baseline.preview.rgba,
            baseline.preview.width,
            baseline.preview.height,
        );
        let exact = comparison.get("pixel_diff_count").and_then(Value::as_u64) == Some(0)
            && comparison.get("shape") == comparison.get("baseline_shape");
        let differences = entry.get("intentional_differences").and_then(Value::as_array);
        let accepted = differences.is_some_and(|differences| {
            !differences.is_empty()
                && differences.iter().all(|difference| {
                    ["reason", "impact", "reviewer"].iter().all(|key| {
                        difference
                            .get(*key)
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.trim().is_empty())
                    }) && difference.get("accepted").and_then(Value::as_bool) == Some(true)
                })
                && entry
                    .get("review_conclusion")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.starts_with("accepted:"))
        });
        let mut details = comparison;
        details.insert("rule_version".into(), json!(rule_version));
        details.insert("config_hash".into(), json!(config_hash));
        details.insert("input_hashes".into(), json!([input_hash]));
        details.insert("baseline_identity".into(), entry["baseline_identity"].clone());
        details.insert("baseline_hash".into(), json!(baseline_hash));
        details.insert("license_basis".into(), entry["license_basis"].clone());
        details.insert("sample_provenance".into(), entry["sample_provenance"].clone());
        details.insert("crop".into(), entry.get("crop").cloned().unwrap_or(Value::Null));
        details.insert(
            "intentional_differences".into(),
            entry.get("intentional_differences").cloned().unwrap_or_else(|| json!([])),
        );
        let conclusion = if exact {
            "exact pixel agreement".to_owned()
        } else if accepted {
            text(entry.get("review_conclusion"), "review conclusion")?.to_owned()
        } else {
            "differences require review and explicit acceptance".to_owned()
        };
        details.insert("review_conclusion".into(), json!(conclusion));
        let next_status =
            if exact || accepted { "passed" } else { "difference_pending" }.to_owned();
        Ok((details, exact || accepted, next_status))
    })();

    match attempt {
        Ok((details, success, next_status)) => {
            for (key, value) in details {
                result[key] = value;
            }
            result["status"] = json!(next_status);
            if !success {
                result["blocked_reasons"] = json!([]);
            }
        }
        Err(error) => {
            eprintln!("display fixture {path_id}: {error}");
            result["status"] = json!("blocked");
            result["blocked_reasons"] =
                json!(["Rust rule, source input, or old baseline could not be verified"]);
            result["validation_issue"] = json!("NativeValidationError");
        }
    }
    result
}

pub fn run(default_report: &str, label: &str) -> Result<i32, String> {
    let mut args = std::env::args().skip(1);
    let mut manifest = PathBuf::from("tests/fixtures/legacy-display/manifest.json");
    let mut fixture_root: Option<PathBuf> = None;
    let mut report = PathBuf::from(default_report);
    while let Some(option) = args.next() {
        let value = args.next().ok_or_else(|| format!("{option} requires a value"))?;
        match option.as_str() {
            "--manifest" => manifest = value.into(),
            "--fixture-root" => fixture_root = Some(value.into()),
            "--report" => report = value.into(),
            _ => return Err(format!("unknown option {option}")),
        }
    }
    let root =
        fixture_root.unwrap_or_else(|| manifest.parent().unwrap_or(Path::new(".")).to_path_buf());
    let root = root.canonicalize().map_err(|_| "fixture root is unavailable".to_owned())?;
    let manifest = if manifest.is_absolute() {
        manifest
    } else {
        std::env::current_dir().map_err(|_| "working directory is unavailable")?.join(manifest)
    };
    let manifest = manifest.canonicalize().map_err(|_| "manifest is unavailable".to_owned())?;
    if !manifest.starts_with(&root) {
        return Err("manifest is outside the fixture root".into());
    }
    if fs::metadata(&manifest).map_err(|_| "manifest is unavailable")?.len() > MAX_JSON_BYTES {
        return Err("manifest exceeds its byte limit".into());
    }
    let document: Value =
        serde_json::from_slice(&fs::read(&manifest).map_err(|_| "manifest could not be read")?)
            .map_err(|_| "manifest JSON is invalid")?;
    if document.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("manifest schema_version must be 1".into());
    }
    let entries = document
        .get("entries")
        .and_then(Value::as_array)
        .filter(|entries| !entries.is_empty())
        .ok_or("manifest requires a nonempty entries array")?;
    let limits = Limits {
        max_artifact_bytes: MAX_INPUT_BYTES,
        max_frame_bytes: MAX_INPUT_BYTES,
        max_pixels: MAX_PIXELS,
        max_temp_bytes: 2 * MAX_PIXELS * 4,
        ..Limits::default()
    };
    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::with_capacity(entries.len());
    for entry in entries {
        let path_id = text(entry.get("path_id"), "path_id")?;
        if !seen.insert(path_id.to_owned()) {
            return Err("manifest contains duplicate path_id".into());
        }
        paths.push(validate_entry(entry, &root, &limits));
    }
    let counts = ["passed", "difference_pending", "blocked"]
        .into_iter()
        .map(|status| {
            (status.to_owned(), paths.iter().filter(|path| path["status"] == status).count())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let passed = counts["passed"];
    let total = paths.len();
    let report_value = json!({
        "schema_version": 1,
        "coverage_status": document.get("coverage_status").and_then(Value::as_str).unwrap_or("unverified"),
        "counts": {"total": total, "passed": passed,
            "difference_pending": counts["difference_pending"], "blocked": counts["blocked"]},
        "paths": paths,
        "scientific_status_unchanged": true,
    });
    if let Some(parent) = report.parent() {
        fs::create_dir_all(parent).map_err(|_| "report directory could not be created")?;
    }
    fs::write(
        &report,
        serde_json::to_vec_pretty(&report_value).map_err(|_| "report could not be encoded")?,
    )
    .map_err(|_| "report could not be written")?;
    println!("{label}: {}; report={}", report_value["counts"], report.display());
    Ok(if passed == total { 0 } else { 1 })
}
