//! Gray-code display transforms and evidence-gated compatibility rules.
//!
//! Gray presentation preserves the historical pixel algorithm. It does not
//! turn display gray values into scientific dBZ values; that conversion lives
//! in [`crate::dbz`]. Evidence-gated transforms must match the embedded rule
//! catalog and its separately packaged evidence.

use crate::errors::{CoreError, CoreResult};
use crate::limits::{Limits, RasterBufferLease, RasterMemoryBudget};
use crate::model::RawFrame;
use crate::preview::preview_artifact;
pub use crate::raster::{
    EncodingBasis, GrayDecision, GrayFrame, GrayRuleIdentity, QUALITY_BELOW_DETECTION,
    QUALITY_INTERPOLATED, QUALITY_MISSING, QUALITY_OUTSIDE_COVERAGE, QUALITY_RECOVERED,
    QUALITY_SOURCE_ANNOTATION, QUALITY_UNKNOWN_COLOR, RasterInput,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::sync::OnceLock;
use tokio_util::sync::CancellationToken;

const ENCODING_VERSION: &str = "旧项目-gray-dbz-v1";
const PARALLEL_RESIZE_MIN_PIXELS: usize = 512 * 512;
const MAX_RESOURCE_BYTES: usize = 4 * 1024 * 1024;
const STEP_OPERATIONS: &[&str] = &[
    "color_preprocess",
    "palette",
    "zero_palette",
    "threshold",
    "background_mask",
    "range_mask",
    "disk_mask",
    "crop",
    "legacy_crop",
    "legacy_palette",
    "legacy_luminance_alpha",
    "gap_repair",
    "legacy_channel_decode",
    "resize",
    "gray_encode",
];
const FINGERPRINT_FIELDS: &[&str] = &[
    "source",
    "product",
    "path_id",
    "rule_version",
    "encoding_version",
    "legacy_reference",
    "ordered_steps",
    "input_constraints",
];

// Compile-time resources keep both native binaries and wheels independent of
// Python's importlib.resources and the installed package layout.
const EMBEDDED_RESOURCES: &[(&str, &str)] = &[
    ("index.json", include_str!("../resources/builtin/gray/index.json")),
    ("au.json", include_str!("../resources/builtin/gray/au.json")),
    ("bmkg.json", include_str!("../resources/builtin/gray/bmkg.json")),
    ("ca.json", include_str!("../resources/builtin/gray/ca.json")),
    ("es.json", include_str!("../resources/builtin/gray/es.json")),
    ("fr.json", include_str!("../resources/builtin/gray/fr.json")),
    ("id.json", include_str!("../resources/builtin/gray/id.json")),
    ("kr.json", include_str!("../resources/builtin/gray/kr.json")),
    ("my-east.json", include_str!("../resources/builtin/gray/my-east.json")),
    ("my.json", include_str!("../resources/builtin/gray/my.json")),
    ("nz.json", include_str!("../resources/builtin/gray/nz.json")),
    ("ph.json", include_str!("../resources/builtin/gray/ph.json")),
    ("pt.json", include_str!("../resources/builtin/gray/pt.json")),
    ("rainviewer.json", include_str!("../resources/builtin/gray/rainviewer.json")),
    ("sg.json", include_str!("../resources/builtin/gray/sg.json")),
    ("th.json", include_str!("../resources/builtin/gray/th.json")),
    ("th_royalrain.json", include_str!("../resources/builtin/gray/th_royalrain.json")),
    ("tw.json", include_str!("../resources/builtin/gray/tw.json")),
    ("vn.json", include_str!("../resources/builtin/gray/vn.json")),
    ("windy.json", include_str!("../resources/builtin/gray/windy.json")),
    (
        "evidence/au__composite.json",
        include_str!("../resources/builtin/gray/evidence/au__composite.json"),
    ),
    ("evidence/ca__rain.json", include_str!("../resources/builtin/gray/evidence/ca__rain.json")),
    (
        "evidence/es__composite.json",
        include_str!("../resources/builtin/gray/evidence/es__composite.json"),
    ),
    (
        "evidence/fr__composite.json",
        include_str!("../resources/builtin/gray/evidence/fr__composite.json"),
    ),
    (
        "evidence/id__composite.json",
        include_str!("../resources/builtin/gray/evidence/id__composite.json"),
    ),
    (
        "evidence/kr__composite.json",
        include_str!("../resources/builtin/gray/evidence/kr__composite.json"),
    ),
    (
        "evidence/my__composite__east.json",
        include_str!("../resources/builtin/gray/evidence/my__composite__east.json"),
    ),
    (
        "evidence/my__composite__peninsular.json",
        include_str!("../resources/builtin/gray/evidence/my__composite__peninsular.json"),
    ),
    ("evidence/nz__rain.json", include_str!("../resources/builtin/gray/evidence/nz__rain.json")),
    (
        "evidence/pt__composite.json",
        include_str!("../resources/builtin/gray/evidence/pt__composite.json"),
    ),
    (
        "evidence/sg__composite.json",
        include_str!("../resources/builtin/gray/evidence/sg__composite.json"),
    ),
    (
        "evidence/th__composite__kkn240Loop.json",
        include_str!("../resources/builtin/gray/evidence/th__composite__kkn240Loop.json"),
    ),
    (
        "evidence/th_royalrain__cappi.json",
        include_str!("../resources/builtin/gray/evidence/th_royalrain__cappi.json"),
    ),
    (
        "evidence/tw__observation.json",
        include_str!("../resources/builtin/gray/evidence/tw__observation.json"),
    ),
    ("evidence/vn__cmax.json", include_str!("../resources/builtin/gray/evidence/vn__cmax.json")),
];

/// A display preview and the catalog decision that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrayPreview {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub applied: bool,
    pub rule_version: Option<String>,
    pub reason: Option<String>,
}

/// Gray output plus source-recoverable pixel quality captured before display
/// transforms can force alpha opaque or resize the image.
#[derive(Clone, Debug, PartialEq)]
pub struct GrayQualityPreview {
    pub preview: GrayPreview,
    pub quality: Vec<u16>,
    pub origin_quality: Vec<u16>,
    pub original_alpha: Vec<u8>,
    pub rule_identity: GrayRuleIdentity,
}

/// Apply the source's reviewed legacy display transform, if one is enabled.
///
/// A blocked or unmatched catalog entry returns a byte-for-byte copy of the
/// supplied RGBA image. A passed entry with stale evidence, incompatible input,
/// or an unported exact operator returns an error instead.
pub fn apply_for_source(
    source: &str,
    product: &str,
    station: Option<&str>,
    format: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
    limits: &Limits,
) -> CoreResult<GrayPreview> {
    apply_for_source_inner(
        source, product, station, format, width, height, rgba, limits, false, None,
    )
    .map(|(preview, _, _)| preview)
}

/// Apply a verified gray rule while retaining mask and repair quality.
///
/// The legacy display API remains a projection of the same transform and
/// returns the historical pixels without allocating quality sidecars.
pub fn apply_for_source_with_quality(
    source: &str,
    product: &str,
    station: Option<&str>,
    format: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
    limits: &Limits,
) -> CoreResult<GrayQualityPreview> {
    let (preview, quality, rule_identity) = apply_for_source_inner(
        source, product, station, format, width, height, rgba, limits, true, None,
    )?;
    let (quality, origin_quality, original_alpha) = quality.ok_or_else(|| {
        display_error("gray quality sidecar was not produced for a verified transform")
    })?;
    let rule_identity = rule_identity
        .ok_or_else(|| display_error("verified gray transform did not retain its rule identity"))?;
    Ok(GrayQualityPreview { preview, quality, origin_quality, original_alpha, rule_identity })
}

pub(crate) fn apply_for_source_with_quality_and_cancel(
    source: &str,
    product: &str,
    station: Option<&str>,
    format: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
    limits: &Limits,
    cancellation: &CancellationToken,
) -> CoreResult<GrayQualityPreview> {
    let (preview, quality, rule_identity) = apply_for_source_inner(
        source,
        product,
        station,
        format,
        width,
        height,
        rgba,
        limits,
        true,
        Some(cancellation),
    )?;
    let (quality, origin_quality, original_alpha) = quality.ok_or_else(|| {
        display_error("gray quality sidecar was not produced for a verified transform")
    })?;
    let rule_identity = rule_identity
        .ok_or_else(|| display_error("verified gray transform did not retain its rule identity"))?;
    Ok(GrayQualityPreview { preview, quality, origin_quality, original_alpha, rule_identity })
}

fn apply_for_source_inner(
    source: &str,
    product: &str,
    station: Option<&str>,
    format: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
    limits: &Limits,
    with_quality: bool,
    cancellation: Option<&CancellationToken>,
) -> CoreResult<(GrayPreview, Option<(Vec<u16>, Vec<u16>, Vec<u8>)>, Option<GrayRuleIdentity>)> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Err(CoreError::Cancelled);
    }
    let pixel_count = checked_pixel_count(width, height)?;
    let rgba_len = pixel_count
        .checked_mul(4)
        .ok_or_else(|| CoreError::ResourceLimit("legacy display RGBA size overflow".into()))?;
    if rgba.len() != rgba_len as usize {
        return Err(display_error("legacy display requires width × height RGBA pixels"));
    }
    limits.validate_pixels(pixel_count)?;
    if with_quality {
        check_memory(pixel_count, limits.max_pixels, limits.max_temp_bytes)?;
    }

    let catalog = load_catalog().map_err(display_error)?;
    let selected = match select_entry(&catalog, source, product, station) {
        Some(entry) => Some(entry),
        None if station.is_none_or(str::is_empty) => {
            select_entry_by_input_constraints(&catalog, source, product, format, width, height)
                .map_err(display_error)?
        }
        None => None,
    };
    let Some(entry) = selected else {
        let copy = copy_bytes(rgba, limits.max_temp_bytes)?;
        let preview = GrayPreview {
            width,
            height,
            rgba: copy,
            applied: false,
            rule_version: None,
            reason: Some("no validated gray rule for this source and product".into()),
        };
        let quality = with_quality.then(|| preserved_quality(rgba, pixel_count as usize));
        return Ok((preview, quality, None));
    };

    if entry.status != "passed" {
        let copy = copy_bytes(rgba, limits.max_temp_bytes)?;
        let details = entry
            .blocked_reasons
            .as_ref()
            .and_then(|reasons| (!reasons.is_empty()).then(|| reasons.join("; ")));
        let reason = details.unwrap_or_else(|| match entry.status.as_str() {
            "difference_pending" => {
                "legacy display rule blocked: comparison differences are pending".into()
            }
            _ => "legacy display rule blocked: no source-matched reviewed baseline".into(),
        });
        let preview = GrayPreview {
            width,
            height,
            rgba: copy,
            applied: false,
            rule_version: None,
            reason: Some(reason),
        };
        let quality = with_quality.then(|| preserved_quality(rgba, pixel_count as usize));
        return Ok((preview, quality, None));
    }

    check_memory(pixel_count, limits.max_pixels, limits.max_temp_bytes)?;
    let rule_name = entry
        .rule_file
        .as_deref()
        .ok_or_else(|| display_error("matched passed legacy display entry has no rule file"))?;
    let evidence_name = entry
        .evidence_file
        .as_deref()
        .ok_or_else(|| display_error("matched passed legacy display entry has no evidence file"))?;
    let rule_json = embedded_json(rule_name).map_err(display_error)?;
    let evidence_json = embedded_json(evidence_name).map_err(display_error)?;
    let rule = validate_rule(&rule_json).map_err(display_error)?;
    let evidence = validate_evidence(&evidence_json, &rule).map_err(display_error)?;
    if !rule.matches(source, product, &entry.path_id, entry.rule_version.as_deref())
        || rule.config_hash != entry.config_hash.as_deref().unwrap_or_default()
        || rule.validation_status != "passed"
        || !evidence
    {
        return Err(display_error(
            "matched legacy display evidence is stale or refers to another source",
        ));
    }
    if let Some(expected_station) = rule.input_constraints.get("station") {
        if !expected_station.is_null() && expected_station.as_str() != station {
            return Err(display_error(
                "matched legacy display rule station differs from selected frame",
            ));
        }
    }

    let formats = rule
        .input_constraints
        .get("formats")
        .and_then(Value::as_array)
        .ok_or_else(|| display_error("legacy display formats are missing or malformed"))?;
    if !formats.iter().any(|value| value.as_str() == Some(format))
        || !["PNG", "GIF", "WEBP"].contains(&format)
    {
        return Err(display_error("legacy display input format is not explicitly supported"));
    }
    if let Some(decoded_size) = rule.input_constraints.get("decoded_size") {
        let decoded = decoded_size
            .as_array()
            .filter(|pair| pair.len() == 2)
            .and_then(|pair| Some((pair[0].as_u64()?, pair[1].as_u64()?)))
            .filter(|(w, h)| *w > 0 && *h > 0)
            .ok_or_else(|| {
                display_error("rule decoded_size must contain positive width and height")
            })?;
        if (u64::from(width), u64::from(height)) != decoded {
            return Err(display_error(
                "legacy display input dimensions differ from the verified source sample",
            ));
        }
    }
    let rule_pixel_limit =
        positive_integer(rule.input_constraints.get("max_pixels"), "rule max_pixels")?;
    let pixel_limit = rule_pixel_limit.min(limits.max_pixels);
    check_memory(pixel_count, pixel_limit, limits.max_temp_bytes)?;

    let output = transform(
        width,
        height,
        rgba,
        &rule.ordered_steps,
        pixel_limit,
        limits.max_temp_bytes,
        with_quality,
        &limits.raster_memory_budget(),
        cancellation,
    )
    .map_err(|failure| match failure {
        TransformFailure::Limit(message) => CoreError::ResourceLimit(message),
        TransformFailure::Invalid(message) => display_error(message),
        TransformFailure::Cancelled => CoreError::Cancelled,
    })?;
    let rule_identity = GrayRuleIdentity {
        source: rule.source.clone(),
        product: rule.product.clone(),
        path_id: rule.path_id.clone(),
        station: station.map(str::to_owned),
        rule_version: rule.rule_version.clone(),
        config_hash: rule.config_hash.clone(),
        evidence_ref: evidence_name.to_owned(),
        encoding_wire_version: ENCODING_VERSION.to_owned(),
        input_constraints: rule.input_constraints.clone(),
        ordered_steps: rule.ordered_steps.clone(),
    };
    let preview = GrayPreview {
        width: output.width,
        height: output.height,
        rgba: output.rgba,
        applied: true,
        rule_version: Some(rule.rule_version),
        reason: None,
    };
    let quality = output.quality.map(|quality| {
        (
            quality,
            output.origin_quality.expect("quality has matching origin sidecar"),
            output.original_alpha.expect("quality transform retains input alpha"),
        )
    });
    Ok((preview, quality, Some(rule_identity)))
}

/// Apply only a source/product path whose gray pixels are backed by the
/// embedded passed-rule catalog and matching evidence. The acquired artifact
/// receipt is checked before its image is decoded.
pub fn decode_source_frame(raw: &RawFrame, limits: &Limits) -> CoreResult<GrayDecision> {
    decode_source_frame_inner(raw, limits, None)
}

pub(crate) fn decode_source_frame_with_cancel(
    raw: &RawFrame,
    limits: &Limits,
    cancellation: &CancellationToken,
) -> CoreResult<GrayDecision> {
    decode_source_frame_inner(raw, limits, Some(cancellation))
}

fn decode_source_frame_inner(
    raw: &RawFrame,
    limits: &Limits,
    cancellation: Option<&CancellationToken>,
) -> CoreResult<GrayDecision> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Err(CoreError::Cancelled);
    }
    raw.frame.validate_identity().map_err(|_| display_error("source frame identity is invalid"))?;
    let source = raw.frame.source.as_str();
    let product = raw.frame.product.as_str();
    let station = raw.frame.station.as_deref();
    let catalog = load_catalog().map_err(display_error)?;
    let selected_by_identity = select_entry(&catalog, source, product, station);
    if let Some(entry) = selected_by_identity.filter(|entry| entry.status != "passed") {
        let reason = entry
            .blocked_reasons
            .as_ref()
            .filter(|reasons| !reasons.is_empty())
            .map(|reasons| reasons.join("; "))
            .unwrap_or_else(|| format!("gray path is {}", entry.status));
        return Ok(GrayDecision::Unavailable { reason, original_preview: None });
    }
    if raw.artifacts.len() != 1 {
        return Ok(GrayDecision::Unavailable {
            reason: "source gray rules require exactly one verified image artifact".into(),
            original_preview: None,
        });
    }
    if selected_by_identity.is_none()
        && !catalog.iter().any(|entry| entry.source == source && entry.product == product)
    {
        return Ok(GrayDecision::Unavailable {
            reason: "no gray rule is cataloged for this source and product".into(),
            original_preview: None,
        });
    }
    let artifact = &raw.artifacts[0];
    let decoded = preview_artifact(artifact, limits)?;
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Err(CoreError::Cancelled);
    }
    let selected = match selected_by_identity {
        Some(entry) => Some(entry),
        None if station.is_none_or(str::is_empty) => select_entry_by_input_constraints(
            &catalog,
            source,
            product,
            &decoded.format,
            decoded.preview.width,
            decoded.preview.height,
        )
        .map_err(display_error)?,
        None => None,
    };
    if selected.is_none() {
        return Ok(GrayDecision::Unavailable {
            reason:
                "no validated gray rule matches this source, product, station, format, and shape"
                    .into(),
            original_preview: Some(decoded.preview.rgba),
        });
    }

    let transformed = match cancellation {
        Some(cancellation) => apply_for_source_with_quality_and_cancel(
            source,
            product,
            station,
            &decoded.format,
            decoded.preview.width,
            decoded.preview.height,
            &decoded.preview.rgba,
            limits,
            cancellation,
        )?,
        None => apply_for_source_with_quality(
            source,
            product,
            station,
            &decoded.format,
            decoded.preview.width,
            decoded.preview.height,
            &decoded.preview.rgba,
            limits,
        )?,
    };
    if !transformed.preview.applied {
        return Ok(GrayDecision::Unavailable {
            reason: transformed
                .preview
                .reason
                .unwrap_or_else(|| "verified gray rule was not applied".into()),
            original_preview: Some(decoded.preview.rgba),
        });
    }
    let frame_index = if decoded.format == "GIF" {
        let requested = match raw.frame.locator.get("frame_index") {
            Some(value) => {
                value.as_u64().ok_or_else(|| display_error("source GIF frame index is invalid"))?
            }
            None => 0,
        };
        if requested != 0 {
            return Ok(GrayDecision::Unavailable {
                reason: "the verified gray decoder selects GIF frame 0 only".into(),
                original_preview: Some(decoded.preview.rgba),
            });
        }
        Some(0)
    } else {
        None
    };
    let valid_time = if let Some(index) = frame_index {
        match raw.frame.locator.get("footer_observation_times").and_then(Value::as_array) {
            Some(times) => {
                let selected =
                    times.get(index as usize).and_then(Value::as_str).ok_or_else(|| {
                        display_error("selected GIF frame has no bound observation time")
                    })?;
                crate::model::parse_utc_time(selected)
                    .map_err(|_| display_error("selected GIF frame time is invalid"))?;
                selected.to_owned()
            }
            None => raw.frame.valid_time.clone(),
        }
    } else {
        raw.frame.valid_time.clone()
    };
    let resolved_revision = crate::identity::resolved_revision(&[], raw.frame.revision.as_deref())
        .map_err(|_| display_error("source acquisition revision is unavailable"))?;
    let output_pixels = (transformed.preview.width as usize)
        .checked_mul(transformed.preview.height as usize)
        .ok_or_else(|| CoreError::ResourceLimit("gray output shape overflows".into()))?;
    let alpha = (transformed.original_alpha.len() == output_pixels)
        .then_some(crate::raster::AlphaPlane::U8(transformed.original_alpha));
    let frame = GrayFrame {
        width: transformed.preview.width,
        height: transformed.preview.height,
        rgba: transformed.preview.rgba,
        frame_index,
        alpha,
        quality: transformed.quality,
        origin_quality: Some(transformed.origin_quality),
        input: RasterInput::Source {
            frame: raw.frame.clone(),
            resolved_revision,
            acquisition_receipt: raw.public_receipt(),
        },
        encoding_basis: EncodingBasis::VerifiedSourceRule {
            encoding_id: "gray-dbz-v1".into(),
            encoding_version: 1,
            rule: transformed.rule_identity,
        },
        valid_time: Some(valid_time),
        geometry: None,
    };
    frame.validate().map_err(|_| display_error("gray frame failed shape validation"))?;
    Ok(GrayDecision::Applied(frame))
}

fn preserved_quality(rgba: &[u8], pixels: usize) -> (Vec<u16>, Vec<u16>, Vec<u8>) {
    let mut quality = Vec::with_capacity(pixels);
    let mut alpha = Vec::with_capacity(pixels);
    for pixel in rgba.chunks_exact(4) {
        alpha.push(pixel[3]);
        quality.push(if pixel[3] == 0 { QUALITY_MISSING } else { 0 });
    }
    let origin = quality.clone();
    (quality, origin, alpha)
}

#[derive(Clone, Debug)]
struct CatalogEntry {
    source: String,
    product: String,
    path_id: String,
    status: String,
    rule_version: Option<String>,
    config_hash: Option<String>,
    rule_file: Option<String>,
    evidence_file: Option<String>,
    blocked_reasons: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
struct DisplayRule {
    source: String,
    product: String,
    path_id: String,
    rule_version: String,
    config_hash: String,
    fingerprint_fields: Value,
    ordered_steps: Vec<Value>,
    input_constraints: Value,
    validation_status: String,
}

fn display_error(message: impl Into<String>) -> CoreError {
    CoreError::Transport(format!("legacy display: {}", message.into()))
}

fn copy_bytes(bytes: &[u8], temp_limit: u64) -> CoreResult<Vec<u8>> {
    let copy_bytes = u64::try_from(bytes.len())
        .map_err(|_| CoreError::ResourceLimit("legacy display copy size overflow".into()))?;
    if copy_bytes > temp_limit {
        return Err(CoreError::ResourceLimit(format!(
            "legacy display copy bytes {copy_bytes} > {temp_limit}"
        )));
    }
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(|_| CoreError::ResourceLimit("legacy display copy allocation failed".into()))?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

fn embedded_json(path: &str) -> Result<Value, String> {
    if path.is_empty()
        || path.contains(':')
        || path.contains('\\')
        || path.starts_with('/')
        || !path.ends_with(".json")
        || path.split('/').any(|part| part == "..")
    {
        return Err("matched legacy display resource path is unsafe".into());
    }
    // Historical rule files contain relative basenames. Accept both the old
    // resource root and the canonical gray root for callers that stored a
    // qualified resource reference before the rename.
    let path =
        path.strip_prefix("legacy_display/").or_else(|| path.strip_prefix("gray/")).unwrap_or(path);
    let text = EMBEDDED_RESOURCES
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, text)| *text)
        .ok_or_else(|| "matched legacy display resource is absent".to_owned())?;
    if text.len() > MAX_RESOURCE_BYTES {
        return Err("matched legacy display resource exceeds its limit".into());
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|_| "matched legacy display resource is not valid JSON".to_owned())?;
    if !value.is_object() {
        return Err("matched legacy display resource is not an object".into());
    }
    Ok(value)
}

fn load_catalog() -> Result<Vec<CatalogEntry>, String> {
    let index = embedded_json("index.json")?;
    if index.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("legacy display index has an unsupported schema".into());
    }
    let paths = index
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| "legacy display index has an unsupported schema".to_owned())?;
    let mut entries = Vec::with_capacity(paths.len());
    let mut seen = HashSet::with_capacity(paths.len());
    for item in paths {
        let object = item
            .as_object()
            .ok_or_else(|| "legacy display index has a malformed path".to_owned())?;
        let source = required_string(object.get("source"), "legacy display index source")?;
        let product = required_string(object.get("product"), "legacy display index product")?;
        let path_id = required_string(object.get("path_id"), "legacy display index path_id")?;
        if path_id != format!("{source}/{product}")
            && !path_id.starts_with(&format!("{source}/{product}/"))
        {
            return Err("legacy display index has a mismatched path identity".into());
        }
        if !seen.insert(path_id.to_owned()) {
            return Err("legacy display index contains a duplicate path".into());
        }
        let status = required_string(object.get("status"), "legacy display index status")?;
        if !["passed", "difference_pending", "blocked"].contains(&status) {
            return Err("legacy display index has an unknown status".into());
        }
        let blocked_reasons =
            object.get("blocked_reasons").and_then(Value::as_array).map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|item| !item.trim().is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            });
        entries.push(CatalogEntry {
            source: source.to_owned(),
            product: product.to_owned(),
            path_id: path_id.to_owned(),
            status: status.to_owned(),
            rule_version: optional_string(object.get("rule_version")),
            config_hash: optional_string(object.get("config_hash")),
            rule_file: optional_string(object.get("rule_file")),
            evidence_file: optional_string(object.get("evidence_file")),
            blocked_reasons,
        });
    }
    Ok(entries)
}

fn select_entry<'a>(
    entries: &'a [CatalogEntry],
    source: &str,
    product: &str,
    station: Option<&str>,
) -> Option<&'a CatalogEntry> {
    let product_path = format!("{source}/{product}");
    if let Some(station) = station.filter(|station| !station.is_empty()) {
        let station_path = format!("{product_path}/{station}");
        if let Some(entry) = entries.iter().find(|entry| {
            entry.path_id == station_path && entry.source == source && entry.product == product
        }) {
            return Some(entry);
        }
    }
    entries.iter().find(|entry| {
        entry.path_id == product_path && entry.source == source && entry.product == product
    })
}

fn select_entry_by_input_constraints<'a>(
    entries: &'a [CatalogEntry],
    source: &str,
    product: &str,
    format: &str,
    width: u32,
    height: u32,
) -> Result<Option<&'a CatalogEntry>, String> {
    let product_path = format!("{source}/{product}/");
    let mut matching = None;
    for entry in entries.iter().filter(|entry| {
        entry.source == source
            && entry.product == product
            && entry.status == "passed"
            && entry.path_id.starts_with(&product_path)
            && entry.rule_file.is_some()
    }) {
        let rule_file = entry.rule_file.as_deref().expect("filtered rule file");
        let rule_value = embedded_json(rule_file)?;
        let rule = validate_rule(&rule_value)?;
        if !rule.matches(source, product, &entry.path_id, entry.rule_version.as_deref())
            || rule.config_hash != entry.config_hash.as_deref().unwrap_or_default()
            || rule.validation_status != "passed"
        {
            continue;
        }
        if rule.input_constraints.get("station").is_some_and(|station| !station.is_null()) {
            continue;
        }
        let formats = rule.input_constraints.get("formats").and_then(Value::as_array);
        let decoded_size = rule
            .input_constraints
            .get("decoded_size")
            .and_then(Value::as_array)
            .filter(|size| size.len() == 2)
            .and_then(|size| Some((size[0].as_u64()?, size[1].as_u64()?)));
        if !formats.is_some_and(|formats| {
            formats.iter().any(|candidate| candidate.as_str() == Some(format))
        }) || decoded_size != Some((u64::from(width), u64::from(height)))
        {
            continue;
        }
        if matching.is_some() {
            // Dimensions may identify a rule only when the catalog has one
            // evidence-bound candidate; ambiguity remains unmatched.
            return Ok(None);
        }
        matching = Some(entry);
    }
    Ok(matching)
}

fn validate_rule(value: &Value) -> Result<DisplayRule, String> {
    let object =
        value.as_object().ok_or_else(|| "legacy display rule is not an object".to_owned())?;
    let source = required_string(object.get("source"), "source")?;
    let product = required_string(object.get("product"), "product")?;
    let path_id = required_string(object.get("path_id"), "path_id")?;
    if path_id != format!("{source}/{product}")
        && !path_id.starts_with(&format!("{source}/{product}/"))
    {
        return Err("path_id must belong to the declared source and product".into());
    }
    let rule_version = required_string(object.get("rule_version"), "rule_version")?;
    if object.get("encoding_version").and_then(Value::as_str) != Some(ENCODING_VERSION) {
        return Err("unknown legacy display encoding version".into());
    }
    required_string(object.get("legacy_reference"), "legacy_reference")?;
    let steps = object
        .get("ordered_steps")
        .and_then(Value::as_array)
        .filter(|steps| !steps.is_empty())
        .ok_or_else(|| {
            "ordered_steps must explicitly document the old operation sequence".to_owned()
        })?;
    for step in steps {
        let step_object = step
            .as_object()
            .ok_or_else(|| "legacy display step is unsupported or unidentified".to_owned())?;
        let operation = required_string(step_object.get("op"), "ordered_steps[].op")?;
        if !STEP_OPERATIONS.contains(&operation) {
            return Err("legacy display step is unsupported or unidentified".into());
        }
        required_string(step_object.get("basis"), "ordered_steps[].basis")?;
    }
    let constraints = object
        .get("input_constraints")
        .filter(|constraints| constraints.as_object().is_some_and(|map| !map.is_empty()))
        .ok_or_else(|| "input_constraints must explicitly identify admissible input".to_owned())?;
    let validation_status = required_string(object.get("validation_status"), "validation_status")?;
    if !["blocked", "difference_pending", "passed"].contains(&validation_status) {
        return Err("unknown display validation status".into());
    }
    let config_hash = required_hash(object.get("config_hash"), "config_hash")?;
    if canonical_rule_hash(value)? != config_hash {
        return Err("display configuration fingerprint mismatch".into());
    }
    let mut fingerprint_fields = Map::new();
    for field in FINGERPRINT_FIELDS {
        fingerprint_fields.insert(
            (*field).to_owned(),
            object.get(*field).expect("required fingerprint field was checked").clone(),
        );
    }
    Ok(DisplayRule {
        source: source.to_owned(),
        product: product.to_owned(),
        path_id: path_id.to_owned(),
        rule_version: rule_version.to_owned(),
        config_hash,
        fingerprint_fields: Value::Object(fingerprint_fields),
        ordered_steps: steps.clone(),
        input_constraints: constraints.clone(),
        validation_status: validation_status.to_owned(),
    })
}

impl DisplayRule {
    fn matches(&self, source: &str, product: &str, path_id: &str, version: Option<&str>) -> bool {
        self.source == source
            && self.product == product
            && self.path_id == path_id
            && Some(self.rule_version.as_str()) == version
    }
}

fn validate_evidence(value: &Value, rule: &DisplayRule) -> Result<bool, String> {
    let object = value.as_object().ok_or_else(|| "display evidence is not an object".to_owned())?;
    let path_id = required_string(object.get("path_id"), "path_id")?;
    let status = required_string(object.get("status"), "status")?;
    if !["blocked", "difference_pending", "passed"].contains(&status) {
        return Err("unknown display evidence status".into());
    }
    if object.get("scientific_status_unchanged").and_then(Value::as_bool) != Some(true) {
        return Err("display evidence cannot change scientific status".into());
    }
    let hashes = object
        .get("input_hashes")
        .map(Value::as_array)
        .flatten()
        .ok_or_else(|| "input_hashes must be an array".to_owned())?;
    for hash in hashes {
        required_hash(Some(hash), "input_hashes[]")?;
    }
    let reasons = string_array(object.get("blocked_reasons"), "blocked_reasons")?;
    let differences = object
        .get("intentional_differences")
        .map(Value::as_array)
        .flatten()
        .ok_or_else(|| "intentional_differences must be an array".to_owned())?;
    for difference in differences {
        if let Some(text) = difference.as_str() {
            if text.trim().is_empty() {
                return Err("intentional_differences[] must be nonempty".into());
            }
        } else if !difference.is_object() {
            return Err("intentional_differences[] must be a string or object".into());
        }
    }
    let crop = validate_optional_tuple(object.get("crop"), 4, false, "crop")?;
    let shape = validate_optional_tuple(object.get("shape"), 2, true, "shape")?;
    let pixel_diff_count = match object.get("pixel_diff_count") {
        None | Some(Value::Null) => None,
        Some(value) => {
            Some(value.as_u64().ok_or_else(|| "pixel_diff_count must be nonnegative".to_owned())?)
        }
    };
    let rule_version = optional_string(object.get("rule_version"));
    let config_hash = optional_valid_hash(object.get("config_hash"), "config_hash")?;
    let output_hash = optional_valid_hash(object.get("output_hash"), "output_hash")?;
    let baseline_hash = optional_valid_hash(object.get("baseline_hash"), "baseline_hash")?;
    let baseline_identity = optional_string(object.get("baseline_identity"));
    let sample_provenance = optional_string(object.get("sample_provenance"));
    let review_conclusion = optional_string(object.get("review_conclusion"));
    let alpha_comparison = optional_string(object.get("alpha_comparison"));
    let background_comparison = optional_string(object.get("background_comparison"));
    let missing_comparison = optional_string(object.get("missing_comparison"));

    if status == "blocked" && reasons.is_empty() {
        return Err("blocked display paths must enumerate missing evidence".into());
    }
    if status == "difference_pending"
        && pixel_diff_count.unwrap_or(0) == 0
        && differences.is_empty()
    {
        return Err("difference_pending must identify a measured or described difference".into());
    }
    if status == "passed" {
        if !reasons.is_empty() {
            return Err("passed display path still contains blockers".into());
        }
        for (name, item) in [
            ("rule_version", rule_version.as_deref()),
            ("baseline_identity", baseline_identity.as_deref()),
            ("sample_provenance", sample_provenance.as_deref()),
            ("review_conclusion", review_conclusion.as_deref()),
            ("alpha_comparison", alpha_comparison.as_deref()),
            ("background_comparison", background_comparison.as_deref()),
            ("missing_comparison", missing_comparison.as_deref()),
        ] {
            if item.is_none_or(|text| text.trim().is_empty()) {
                return Err(format!("{name} must be a non-empty string"));
            }
        }
        if hashes.is_empty() || crop.is_none() || shape.is_none() || pixel_diff_count.is_none() {
            return Err(
                "passed display evidence must include input, crop, dimensions and pixel comparison"
                    .into(),
            );
        }
        let has_difference = pixel_diff_count.unwrap_or(0) != 0
            || alpha_comparison.as_deref() != Some("identical")
            || background_comparison.as_deref() != Some("identical")
            || missing_comparison.as_deref() != Some("identical");
        if has_difference {
            let accepted_conclusion = review_conclusion
                .as_deref()
                .is_some_and(|conclusion| conclusion.starts_with("accepted:"));
            let accepted_differences = !differences.is_empty()
                && differences.iter().all(|difference| {
                    let Some(item) = difference.as_object() else {
                        return false;
                    };
                    item.get("accepted").and_then(Value::as_bool) == Some(true)
                        && ["reason", "impact", "reviewer"].iter().all(|key| {
                            item.get(*key)
                                .and_then(Value::as_str)
                                .is_some_and(|text| !text.trim().is_empty())
                        })
                });
            if !accepted_conclusion || !accepted_differences {
                return Err("display differences need explicit recorded review acceptance".into());
            }
        }
        if config_hash.is_none() || output_hash.is_none() || baseline_hash.is_none() {
            return Err("passed display evidence requires rule, output and baseline SHA-256".into());
        }
    }
    Ok(status == "passed"
        && path_id == rule.path_id
        && rule_version.as_deref() == Some(rule.rule_version.as_str())
        && config_hash.as_deref() == Some(rule.config_hash.as_str())
        && rule.config_hash == canonical_rule_hash(&rule.fingerprint_fields)?)
}

fn canonical_rule_hash(value: &Value) -> Result<String, String> {
    let object =
        value.as_object().ok_or_else(|| "legacy display rule is not an object".to_owned())?;
    let mut selected = Map::new();
    for field in FINGERPRINT_FIELDS {
        let item = object.get(*field).ok_or_else(|| {
            "legacy display rule has incomplete or non-JSON configuration".to_owned()
        })?;
        selected.insert((*field).to_owned(), item.clone());
    }
    let mut encoded = String::new();
    write_canonical_json(&Value::Object(selected), &mut encoded)?;
    let digest = Sha256::digest(encoded.as_bytes());
    Ok(hex_lower(&digest))
}

fn write_canonical_json(value: &Value, output: &mut String) -> Result<(), String> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(number) => output.push_str(&number.to_string()),
        Value::String(text) => output.push_str(
            &serde_json::to_string(text)
                .map_err(|_| "legacy display rule contains invalid JSON text".to_owned())?,
        ),
        Value::Array(items) => {
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_canonical_json(item, output)?;
            }
            output.push(']');
        }
        Value::Object(items) => {
            output.push('{');
            let mut keys = items.keys().collect::<Vec<_>>();
            keys.sort();
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                output.push_str(
                    &serde_json::to_string(key)
                        .map_err(|_| "legacy display rule contains invalid JSON key".to_owned())?,
                );
                output.push(':');
                write_canonical_json(&items[key], output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn required_string<'a>(value: Option<&'a Value>, name: &str) -> Result<&'a str, String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| format!("{name} must be a non-empty string"))
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn required_hash(value: Option<&Value>, name: &str) -> Result<String, String> {
    let text = required_string(value, name)?;
    if text.len() != 64
        || !text.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{name} must be a lowercase SHA-256 hex digest"));
    }
    Ok(text.to_owned())
}

fn optional_valid_hash(value: Option<&Value>, name: &str) -> Result<Option<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => required_hash(Some(value), name).map(Some),
    }
}

fn string_array(value: Option<&Value>, name: &str) -> Result<Vec<String>, String> {
    let values =
        value.and_then(Value::as_array).ok_or_else(|| format!("{name} must be an array"))?;
    values
        .iter()
        .map(|item| required_string(Some(item), &format!("{name}[]")).map(str::to_owned))
        .collect()
}

fn validate_optional_tuple(
    value: Option<&Value>,
    length: usize,
    positive: bool,
    name: &str,
) -> Result<Option<Vec<u64>>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let items = value
        .as_array()
        .filter(|items| items.len() == length)
        .ok_or_else(|| format!("{name} must contain {length} integer values"))?;
    let mut output = Vec::with_capacity(length);
    for item in items {
        let number = item
            .as_u64()
            .filter(|number| !positive || *number > 0)
            .ok_or_else(|| format!("{name} has invalid integer values"))?;
        output.push(number);
    }
    Ok(Some(output))
}

fn checked_pixel_count(width: u32, height: u32) -> CoreResult<u64> {
    if width == 0 || height == 0 {
        return Err(display_error("image dimensions must be positive"));
    }
    u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| CoreError::ResourceLimit("legacy display dimensions overflow".into()))
}

fn check_memory(pixels: u64, pixel_limit: u64, temp_limit: u64) -> CoreResult<()> {
    let temporary_bytes = pixels
        .checked_mul(32)
        .ok_or_else(|| CoreError::ResourceLimit("legacy display memory size overflow".into()))?;
    if pixels > pixel_limit {
        return Err(CoreError::ResourceLimit(format!(
            "legacy display pixels {pixels} > {pixel_limit}"
        )));
    }
    if temporary_bytes > temp_limit {
        return Err(CoreError::ResourceLimit(format!(
            "legacy display temporary bytes {temporary_bytes} > {temp_limit}"
        )));
    }
    Ok(())
}

fn positive_integer(value: Option<&Value>, label: &str) -> CoreResult<u64> {
    value
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| display_error(format!("{label} must be a positive integer")))
}

#[derive(Debug)]
enum TransformFailure {
    Invalid(String),
    Limit(String),
    Cancelled,
}

impl From<String> for TransformFailure {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}

#[derive(Debug)]
struct LegacyPalette {
    codes: Vec<u16>,
    invalid: Vec<bool>,
    keep: Vec<bool>,
    cookbook: Vec<f32>,
}

struct PixelState {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    values: Vec<f64>,
    mapped: Vec<bool>,
    quality: Option<Vec<u16>>,
    origin_quality: Option<Vec<u16>>,
    zero_colors: HashSet<[u8; 3]>,
    has_palette: bool,
    encoded: bool,
    legacy: Option<LegacyPalette>,
    channel_method: Option<String>,
}

struct TransformOutput {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    quality: Option<Vec<u16>>,
    origin_quality: Option<Vec<u16>>,
    original_alpha: Option<Vec<u8>>,
}

fn transform(
    width: u32,
    height: u32,
    rgba: &[u8],
    steps: &[Value],
    pixel_limit: u64,
    temp_limit: u64,
    collect_quality: bool,
    memory: &RasterMemoryBudget,
    cancellation: Option<&CancellationToken>,
) -> Result<TransformOutput, TransformFailure> {
    let pixels = (u64::from(width) * u64::from(height)) as usize;
    check_transform_memory(pixels as u64, pixel_limit, temp_limit)?;
    let state_bytes_per_pixel = if collect_quality { 28 } else { 20 };
    let _state_lease = reserve_memory(memory, pixels as u64, state_bytes_per_pixel)?;
    let mut initial_quality = if collect_quality { Some(try_filled(pixels, 0_u16)?) } else { None };
    let mut original_alpha = collect_quality.then(|| Vec::with_capacity(pixels));
    if let Some(quality) = &mut initial_quality {
        for (index, pixel) in rgba.chunks_exact(4).enumerate() {
            if pixel[3] == 0 {
                quality[index] |= QUALITY_MISSING;
            }
        }
    }
    if let Some(alpha) = &mut original_alpha {
        alpha.extend(rgba.chunks_exact(4).map(|pixel| pixel[3]));
    }
    let mut state = PixelState {
        width,
        height,
        rgba: try_clone(rgba)?,
        values: try_filled(pixels, f64::NAN)?,
        mapped: try_filled(pixels, false)?,
        quality: initial_quality.clone(),
        origin_quality: initial_quality,
        zero_colors: HashSet::new(),
        has_palette: false,
        encoded: false,
        legacy: None,
        channel_method: None,
    };

    for (step_index, step) in steps.iter().enumerate() {
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err(TransformFailure::Cancelled);
        }
        let object = step.as_object().ok_or_else(|| {
            TransformFailure::Invalid("legacy display step is not an object".into())
        })?;
        let op = object.get("op").and_then(Value::as_str).ok_or_else(|| {
            TransformFailure::Invalid("legacy display step has no operation".into())
        })?;
        match op {
            "color_preprocess" => preprocess_colors(&mut state, object)?,
            "background_mask" => background_mask(&mut state, object)?,
            "zero_palette" => add_zero_colors(&mut state, object)?,
            "palette" => apply_explicit_palette(&mut state, object)?,
            "legacy_palette" => apply_legacy_palette(&mut state, object)?,
            "legacy_channel_decode" => decode_legacy_channel(&mut state, object)?,
            "threshold" => apply_threshold(&mut state, object)?,
            "range_mask" | "disk_mask" => apply_mask(&mut state, object, op)?,
            "crop" => crop(&mut state, object, pixel_limit, temp_limit, memory)?,
            "legacy_crop" => legacy_crop(&mut state, object, pixel_limit, temp_limit, memory)?,
            "gap_repair" => {
                repair_gaps(&mut state, object, pixel_limit, temp_limit, memory, cancellation)?
            }
            "gray_encode" => encode_gray(&mut state, object)?,
            "legacy_luminance_alpha" => encode_luminance_alpha(&mut state)?,
            "resize" => {
                if step_index + 1 != steps.len() || !state.encoded {
                    return Err(TransformFailure::Invalid(
                        "legacy resize must be the final step after gray encoding".into(),
                    ));
                }
                resize_encoded_gray(
                    &mut state,
                    object,
                    pixel_limit,
                    temp_limit,
                    memory,
                    cancellation,
                )?;
            }
            other => {
                return Err(TransformFailure::Invalid(format!(
                    "unsupported legacy display step: {other}"
                )));
            }
        }
    }
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Err(TransformFailure::Cancelled);
    }
    if !state.encoded {
        return Err(TransformFailure::Invalid(
            "legacy display rule ended without explicit gray encoding".into(),
        ));
    }
    Ok(TransformOutput {
        width: state.width,
        height: state.height,
        rgba: state.rgba,
        quality: state.quality,
        origin_quality: state.origin_quality,
        original_alpha,
    })
}

fn try_filled<T: Clone>(length: usize, value: T) -> Result<Vec<T>, TransformFailure> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| TransformFailure::Limit("legacy display allocation failed".into()))?;
    output.resize(length, value);
    Ok(output)
}

fn try_clone<T: Clone>(source: &[T]) -> Result<Vec<T>, TransformFailure> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(source.len())
        .map_err(|_| TransformFailure::Limit("legacy display allocation failed".into()))?;
    output.extend_from_slice(source);
    Ok(output)
}

fn check_transform_memory(
    pixels: u64,
    pixel_limit: u64,
    temp_limit: u64,
) -> Result<(), TransformFailure> {
    let working = pixels
        .checked_mul(32)
        .ok_or_else(|| TransformFailure::Limit("legacy display memory size overflow".into()))?;
    if pixels > pixel_limit || working > temp_limit {
        return Err(TransformFailure::Limit(
            "legacy display pixel or temporary memory limit exceeded".into(),
        ));
    }
    Ok(())
}

fn reserve_memory(
    memory: &RasterMemoryBudget,
    pixels: u64,
    bytes_per_pixel: u64,
) -> Result<RasterBufferLease, TransformFailure> {
    let bytes = pixels
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| TransformFailure::Limit("legacy display buffer size overflow".into()))?;
    memory.reserve(bytes).map_err(|error| TransformFailure::Limit(error.to_string()))
}

fn json_integer(value: Option<&Value>, label: &str) -> Result<u64, TransformFailure> {
    value
        .and_then(Value::as_u64)
        .ok_or_else(|| TransformFailure::Invalid(format!("{label} must be a positive integer")))
}

fn positive_value(value: Option<&Value>, label: &str) -> Result<u64, TransformFailure> {
    let number = json_integer(value, label)?;
    if number == 0 {
        return Err(TransformFailure::Invalid(format!("{label} must be a positive integer")));
    }
    Ok(number)
}

fn finite_value(value: Option<&Value>, label: &str) -> Result<f64, TransformFailure> {
    value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
        .ok_or_else(|| TransformFailure::Invalid(format!("{label} must be a finite number")))
}

fn rgb(value: Option<&Value>, label: &str) -> Result<[u8; 3], TransformFailure> {
    let channels =
        value.and_then(Value::as_array).filter(|channels| channels.len() == 3).ok_or_else(
            || TransformFailure::Invalid(format!("{label} contains an invalid RGB color")),
        )?;
    let mut parsed = [0_u8; 3];
    for (index, channel) in channels.iter().enumerate() {
        parsed[index] = channel.as_u64().filter(|channel| *channel <= 255).ok_or_else(|| {
            TransformFailure::Invalid(format!("{label} contains an invalid RGB color"))
        })? as u8;
    }
    Ok(parsed)
}

fn color_list(
    value: Option<&Value>,
    label: &str,
    allow_empty: bool,
    reject_duplicates: bool,
) -> Result<Vec<[u8; 3]>, TransformFailure> {
    let colors = value
        .and_then(Value::as_array)
        .filter(|colors| allow_empty || !colors.is_empty())
        .ok_or_else(|| {
            TransformFailure::Invalid(format!("{label} requires an explicit RGB color list"))
        })?;
    let mut result = Vec::with_capacity(colors.len());
    let mut seen = HashSet::with_capacity(colors.len());
    for color in colors {
        let parsed = rgb(Some(color), label)?;
        if reject_duplicates && !seen.insert(parsed) {
            return Err(TransformFailure::Invalid(format!("{label} repeats an RGB color")));
        }
        result.push(parsed);
    }
    Ok(result)
}

fn pixel_offset(index: usize) -> usize {
    index * 4
}

fn pixel_rgb(rgba: &[u8], index: usize) -> [u8; 3] {
    let offset = pixel_offset(index);
    [rgba[offset], rgba[offset + 1], rgba[offset + 2]]
}

fn preprocess_colors(
    state: &mut PixelState,
    step: &Map<String, Value>,
) -> Result<(), TransformFailure> {
    if state.has_palette || state.encoded {
        return Err(TransformFailure::Invalid("color preprocess must precede the palette".into()));
    }
    let replacements = step
        .get("replacements")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| {
            TransformFailure::Invalid("color preprocess requires explicit replacements".into())
        })?;
    for item in replacements {
        let item = item
            .as_object()
            .ok_or_else(|| TransformFailure::Invalid("invalid color replacement".into()))?;
        let from = rgb(item.get("from"), "replacement source")?;
        let to = rgb(item.get("to"), "replacement target")?;
        for index in 0..state.values.len() {
            let offset = pixel_offset(index);
            if state.rgba[offset + 3] != 0 && pixel_rgb(&state.rgba, index) == from {
                state.rgba[offset..offset + 3].copy_from_slice(&to);
            }
        }
    }
    Ok(())
}

fn background_mask(
    state: &mut PixelState,
    step: &Map<String, Value>,
) -> Result<(), TransformFailure> {
    if state.encoded {
        return Err(TransformFailure::Invalid("background mask must precede gray encoding".into()));
    }
    let colors = color_list(step.get("colors"), "colors", false, true)?;
    let colors = colors.into_iter().collect::<HashSet<_>>();
    for index in 0..state.values.len() {
        if colors.contains(&pixel_rgb(&state.rgba, index)) {
            state.rgba[pixel_offset(index) + 3] = 0;
            state.values[index] = f64::NAN;
            state.mapped[index] = false;
            mark_quality(state, index, QUALITY_OUTSIDE_COVERAGE);
        }
    }
    Ok(())
}

fn add_zero_colors(
    state: &mut PixelState,
    step: &Map<String, Value>,
) -> Result<(), TransformFailure> {
    if state.has_palette || state.encoded {
        return Err(TransformFailure::Invalid(
            "zero palette must precede the value palette".into(),
        ));
    }
    let colors = color_list(step.get("colors"), "colors", false, true)?;
    if colors.iter().any(|color| state.zero_colors.contains(color)) {
        return Err(TransformFailure::Invalid("duplicate source zero color".into()));
    }
    state.zero_colors.extend(colors);
    Ok(())
}

fn apply_explicit_palette(
    state: &mut PixelState,
    step: &Map<String, Value>,
) -> Result<(), TransformFailure> {
    if state.has_palette || state.encoded {
        return Err(TransformFailure::Invalid(
            "exactly one evidence-bound palette is allowed".into(),
        ));
    }
    let entries = step
        .get("entries")
        .and_then(Value::as_array)
        .filter(|entries| !entries.is_empty())
        .ok_or_else(|| {
            TransformFailure::Invalid("palette requires explicit source RGB to dBZ entries".into())
        })?;
    let mut table = std::collections::HashMap::<[u8; 3], f64>::new();
    for color in &state.zero_colors {
        table.insert(*color, 0.0);
    }
    for entry in entries {
        let entry = entry
            .as_object()
            .ok_or_else(|| TransformFailure::Invalid("palette entry is invalid".into()))?;
        let color = rgb(entry.get("rgb"), "palette RGB")?;
        if table.contains_key(&color) {
            return Err(TransformFailure::Invalid(
                "palette contains duplicate or conflicting RGB entry".into(),
            ));
        }
        let dbz = finite_value(entry.get("dbz"), "palette dBZ")?;
        table.insert(color, dbz);
    }
    for index in 0..state.values.len() {
        let offset = pixel_offset(index);
        if state.rgba[offset + 3] == 0 {
            continue;
        }
        if let Some(value) = table.get(&pixel_rgb(&state.rgba, index)) {
            state.values[index] = *value;
            state.mapped[index] = true;
        } else {
            return Err(TransformFailure::Invalid(
                "unmapped opaque RGB pixels: no inferred luminance or WMS conversion".into(),
            ));
        }
    }
    state.has_palette = true;
    Ok(())
}

fn nearest_palette(rgba: &[u8], colors: &[[u8; 3]]) -> (Vec<u8>, Vec<u32>) {
    let pixels = rgba.len() / 4;
    let mut nearest = vec![0_u8; pixels];
    let mut distance = vec![0_u32; pixels];
    let classify = |index: usize, nearest_index: &mut u8, nearest_distance: &mut u32| {
        let source_offset = pixel_offset(index);
        let mut best_index = 0;
        let mut best_distance = u32::MAX;
        for (color_index, color) in colors.iter().enumerate() {
            let dr = i32::from(rgba[source_offset]) - i32::from(color[0]);
            let dg = i32::from(rgba[source_offset + 1]) - i32::from(color[1]);
            let db = i32::from(rgba[source_offset + 2]) - i32::from(color[2]);
            let squared = (dr * dr + dg * dg + db * db) as u32;
            if squared < best_distance {
                best_distance = squared;
                best_index = color_index;
            }
        }
        *nearest_index = best_index as u8;
        *nearest_distance = best_distance;
    };
    if pixels >= PARALLEL_RESIZE_MIN_PIXELS
        && let Some(pool) = resize_parallel_pool()
    {
        pool.install(|| {
            use rayon::prelude::*;
            nearest.par_iter_mut().zip(distance.par_iter_mut()).enumerate().for_each(
                |(index, (nearest_index, nearest_distance))| {
                    classify(index, nearest_index, nearest_distance);
                },
            );
        });
        return (nearest, distance);
    }
    for (index, (nearest_index, nearest_distance)) in
        nearest.iter_mut().zip(distance.iter_mut()).enumerate()
    {
        classify(index, nearest_index, nearest_distance);
    }
    (nearest, distance)
}

fn palette_distances(rgba: &[u8], colors: &[[u8; 3]]) -> Vec<u32> {
    let pixels = rgba.len() / 4;
    let mut distances = vec![0_u32; pixels];
    let nearest_distance = |index: usize| {
        let source_offset = pixel_offset(index);
        let mut best_distance = u32::MAX;
        for color in colors {
            let dr = i32::from(rgba[source_offset]) - i32::from(color[0]);
            let dg = i32::from(rgba[source_offset + 1]) - i32::from(color[1]);
            let db = i32::from(rgba[source_offset + 2]) - i32::from(color[2]);
            let squared = (dr * dr + dg * dg + db * db) as u32;
            best_distance = best_distance.min(squared);
        }
        best_distance
    };
    if pixels >= PARALLEL_RESIZE_MIN_PIXELS
        && let Some(pool) = resize_parallel_pool()
    {
        pool.install(|| {
            use rayon::prelude::*;
            distances
                .par_iter_mut()
                .enumerate()
                .for_each(|(index, distance)| *distance = nearest_distance(index));
        });
        return distances;
    }
    for (index, distance) in distances.iter_mut().enumerate() {
        *distance = nearest_distance(index);
    }
    distances
}

fn apply_legacy_palette(
    state: &mut PixelState,
    step: &Map<String, Value>,
) -> Result<(), TransformFailure> {
    if state.has_palette || state.encoded {
        return Err(TransformFailure::Invalid(
            "exactly one legacy palette operation is allowed".into(),
        ));
    }
    let colors = color_list(step.get("colors"), "legacy palette", false, true)?;
    if colors.len() > 255 {
        return Err(TransformFailure::Invalid(
            "legacy cookbook must cover the declared palette".into(),
        ));
    }
    let zero_colors = color_list(step.get("zero_colors"), "legacy zero palette", true, true)?;
    let value_threshold = finite_value(step.get("value_threshold"), "value_threshold")?;
    let zero_threshold = finite_value(step.get("zero_threshold"), "zero_threshold")?;
    if value_threshold < 0.0 || zero_threshold < 0.0 {
        return Err(TransformFailure::Invalid(
            "legacy palette thresholds cannot be negative".into(),
        ));
    }
    let cookbook_values = step
        .get("cookbook")
        .and_then(Value::as_array)
        .filter(|values| values.len() >= colors.len())
        .ok_or_else(|| {
            TransformFailure::Invalid("legacy cookbook must cover the declared palette".into())
        })?;
    let cookbook = cookbook_values
        .iter()
        .map(|value| finite_value(Some(value), "cookbook value").map(|number| number as f32))
        .collect::<Result<Vec<_>, _>>()?;

    let (nearest, distances) = nearest_palette(&state.rgba, &colors);
    let mut codes = Vec::with_capacity(nearest.len());
    let mut invalid = Vec::with_capacity(nearest.len());
    let mut keep = vec![true; nearest.len()];
    let mut invalid_count = 0_u64;
    let value_limit_squared = value_threshold * value_threshold;
    let zero_limit_squared = zero_threshold * zero_threshold;
    let zero_distances = if zero_colors.is_empty() {
        None
    } else {
        Some(palette_distances(&state.rgba, &zero_colors))
    };
    for index in 0..nearest.len() {
        let retain = zero_distances
            .as_ref()
            .is_none_or(|distances| f64::from(distances[index]) > zero_limit_squared);
        keep[index] = retain;
        let bad = retain && f64::from(distances[index]) > value_limit_squared;
        invalid.push(bad);
        if bad {
            invalid_count += 1;
        }
        codes.push(if retain { u16::from(nearest[index]) + 1 } else { 0 });
        if bad && state.rgba[pixel_offset(index) + 3] != 0 {
            mark_quality(state, index, QUALITY_UNKNOWN_COLOR);
        }
    }
    let invalid_limit = finite_value(step.get("invalid_fraction_limit"), "invalid_fraction_limit")?;
    if !(0.0..=1.0).contains(&invalid_limit) {
        return Err(TransformFailure::Invalid(
            "invalid_fraction_limit must be between zero and one".into(),
        ));
    }
    if !invalid.is_empty() && invalid_count as f64 / invalid.len() as f64 > invalid_limit {
        return Err(TransformFailure::Invalid(
            "legacy palette rejected this image: too many unmatched pixels".into(),
        ));
    }
    state.legacy = Some(LegacyPalette { codes, invalid, keep, cookbook });
    state.has_palette = true;
    Ok(())
}

fn decode_legacy_channel(
    state: &mut PixelState,
    step: &Map<String, Value>,
) -> Result<(), TransformFailure> {
    if state.has_palette
        || state.encoded
        || state.legacy.is_some()
        || state.channel_method.is_some()
    {
        return Err(TransformFailure::Invalid(
            "legacy channel decode must be the only source-value decoder".into(),
        ));
    }
    let method = step.get("method").and_then(Value::as_str).ok_or_else(|| {
        TransformFailure::Invalid("legacy channel decode method is unsupported".into())
    })?;
    let channel = json_integer(step.get("channel"), "legacy channel")?;
    let expected = match method {
        "rainviewer_v2_red" => 0,
        "windy_v2_green" => 1,
        _ => {
            return Err(TransformFailure::Invalid(
                "legacy channel decode method is unsupported".into(),
            ));
        }
    };
    if channel != expected {
        return Err(TransformFailure::Invalid(
            "legacy channel decode method or channel is unsupported".into(),
        ));
    }
    for index in 0..state.values.len() {
        let byte = state.rgba[pixel_offset(index) + expected as usize];
        let decoded = if method == "rainviewer_v2_red" {
            let mut number = f32::from(byte);
            if number >= 128.0 {
                number -= 128.0;
            }
            if number <= 32.0 {
                number = 0.0;
            }
            if number >= 32.0 {
                number -= 32.0;
            }
            f64::from(number)
        } else {
            (f64::from(byte) / 2.0).trunc()
        };
        state.values[index] = decoded;
        state.mapped[index] = true;
    }
    state.channel_method = Some(method.to_owned());
    state.has_palette = true;
    Ok(())
}

fn apply_threshold(
    state: &mut PixelState,
    step: &Map<String, Value>,
) -> Result<(), TransformFailure> {
    if !state.has_palette || state.encoded {
        return Err(TransformFailure::Invalid("threshold needs decoded palette values".into()));
    }
    let low = finite_value(step.get("min_dbz"), "min_dbz")?;
    let high = finite_value(step.get("max_dbz"), "max_dbz")?;
    let below = step.get("below").and_then(Value::as_str);
    let above = step.get("above").and_then(Value::as_str);
    if low > high
        || ![Some("mask"), Some("zero")].contains(&below)
        || ![Some("mask"), Some("cap")].contains(&above)
    {
        return Err(TransformFailure::Invalid(
            "threshold requires explicit bounds and below/above behavior".into(),
        ));
    }
    for index in 0..state.values.len() {
        if !state.mapped[index] {
            continue;
        }
        if state.values[index] < low {
            if below == Some("zero") {
                state.values[index] = 0.0;
                mark_quality(state, index, QUALITY_BELOW_DETECTION);
            } else {
                state.rgba[pixel_offset(index) + 3] = 0;
                state.mapped[index] = false;
                mark_quality(state, index, QUALITY_BELOW_DETECTION | QUALITY_MISSING);
            }
        }
        if state.mapped[index] && state.values[index] > high {
            if above == Some("cap") {
                state.values[index] = high;
                mark_quality(state, index, QUALITY_SOURCE_ANNOTATION);
            } else {
                state.rgba[pixel_offset(index) + 3] = 0;
                state.mapped[index] = false;
                mark_quality(state, index, QUALITY_SOURCE_ANNOTATION | QUALITY_MISSING);
            }
        }
    }
    Ok(())
}

fn bounds(
    step: &Map<String, Value>,
    key: &str,
    width: u32,
    height: u32,
) -> Result<(u32, u32, u32, u32), TransformFailure> {
    let values =
        step.get(key).and_then(Value::as_array).filter(|values| values.len() == 4).ok_or_else(
            || TransformFailure::Invalid(format!("{key} must specify integer pixel bounds")),
        )?;
    let parsed = values.iter().map(Value::as_u64).collect::<Option<Vec<_>>>().ok_or_else(|| {
        TransformFailure::Invalid(format!("{key} must specify integer pixel bounds"))
    })?;
    let [left, top, right, bottom] = <[u64; 4]>::try_from(parsed).map_err(|_| {
        TransformFailure::Invalid(format!("{key} must specify integer pixel bounds"))
    })?;
    if !(left < right && right <= u64::from(width) && top < bottom && bottom <= u64::from(height)) {
        return Err(TransformFailure::Invalid(format!("{key} exceeds the image bounds")));
    }
    Ok((left as u32, top as u32, right as u32, bottom as u32))
}

fn apply_mask(
    state: &mut PixelState,
    step: &Map<String, Value>,
    op: &str,
) -> Result<(), TransformFailure> {
    let width = state.width as usize;
    let height = state.height as usize;
    if op == "range_mask" {
        if state.encoded {
            return Err(TransformFailure::Invalid("range mask must precede gray encoding".into()));
        }
        let (left, top, right, bottom) = bounds(step, "bounds", state.width, state.height)?;
        for y in 0..height {
            for x in 0..width {
                if x < left as usize
                    || x >= right as usize
                    || y < top as usize
                    || y >= bottom as usize
                {
                    mask_pixel(state, y * width + x);
                }
            }
        }
        return Ok(());
    }
    if state.encoded {
        return Err(TransformFailure::Invalid("disk mask must precede gray encoding".into()));
    }
    if state.legacy.is_some()
        && step.get("method").and_then(Value::as_str) == Some("legacy_height_center_disk")
    {
        let radius = (f64::from(state.height) - 1.0) / 2.0;
        let radius_squared = radius * radius;
        let legacy = state.legacy.as_mut().expect("legacy presence checked");
        for y in 0..height {
            for x in 0..width {
                let dx = x as f64 - radius;
                let dy = y as f64 - radius;
                if dx * dx + dy * dy > radius_squared {
                    let index = y * width + x;
                    legacy.keep[index] = false;
                    legacy.codes[index] = 0;
                    if let Some(quality) = &mut state.quality {
                        quality[index] |= QUALITY_OUTSIDE_COVERAGE;
                    }
                    if let Some(origin) = &mut state.origin_quality {
                        origin[index] |= QUALITY_OUTSIDE_COVERAGE;
                    }
                }
            }
        }
        return Ok(());
    }
    let center = step
        .get("center")
        .and_then(Value::as_array)
        .filter(|center| center.len() == 2)
        .ok_or_else(|| {
            TransformFailure::Invalid("disk mask requires explicit pixel center".into())
        })?;
    let cx = finite_value(center.first(), "disk cx")?;
    let cy = finite_value(center.get(1), "disk cy")?;
    let radius = finite_value(step.get("radius"), "disk radius")?;
    if radius <= 0.0 {
        return Err(TransformFailure::Invalid("disk radius must be positive".into()));
    }
    let radius_squared = radius * radius;
    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            if dx * dx + dy * dy > radius_squared {
                mask_pixel(state, y * width + x);
            }
        }
    }
    Ok(())
}

fn mask_pixel(state: &mut PixelState, index: usize) {
    state.rgba[pixel_offset(index) + 3] = 0;
    state.values[index] = f64::NAN;
    state.mapped[index] = false;
    mark_quality(state, index, QUALITY_OUTSIDE_COVERAGE);
}

fn mark_quality(state: &mut PixelState, index: usize, bits: u16) {
    if let Some(quality) = &mut state.quality {
        quality[index] |= bits;
    }
    if let Some(origin) = &mut state.origin_quality {
        origin[index] |= bits;
    }
}

fn crop(
    state: &mut PixelState,
    step: &Map<String, Value>,
    pixel_limit: u64,
    temp_limit: u64,
    memory: &RasterMemoryBudget,
) -> Result<(), TransformFailure> {
    let (left, top, right, bottom) = bounds(step, "bounds", state.width, state.height)?;
    if state.legacy.is_some() {
        return Err(TransformFailure::Invalid(
            "crop after legacy palette cannot preserve the source code map exactly".into(),
        ));
    }
    let out_width = right - left;
    let out_height = bottom - top;
    let out_pixels_u64 = u64::from(out_width)
        .checked_mul(u64::from(out_height))
        .ok_or_else(|| TransformFailure::Limit("crop dimensions overflow".into()))?;
    check_transform_memory(out_pixels_u64, pixel_limit, temp_limit)?;
    let _output_lease =
        reserve_memory(memory, out_pixels_u64, if state.quality.is_some() { 28 } else { 20 })?;
    let out_pixels = out_width as usize * out_height as usize;
    let mut rgba = try_filled(out_pixels * 4, 0_u8)?;
    let mut values = try_filled(out_pixels, f64::NAN)?;
    let mut mapped = try_filled(out_pixels, false)?;
    let mut quality = state.quality.as_ref().map(|_| try_filled(out_pixels, 0_u16)).transpose()?;
    let mut origin =
        state.origin_quality.as_ref().map(|_| try_filled(out_pixels, 0_u16)).transpose()?;
    let input_width = state.width as usize;
    for y in 0..out_height as usize {
        for x in 0..out_width as usize {
            let source_index = (y + top as usize) * input_width + x + left as usize;
            let output_index = y * out_width as usize + x;
            rgba[pixel_offset(output_index)..pixel_offset(output_index) + 4].copy_from_slice(
                &state.rgba[pixel_offset(source_index)..pixel_offset(source_index) + 4],
            );
            values[output_index] = state.values[source_index];
            mapped[output_index] = state.mapped[source_index];
            if let (Some(output), Some(input)) = (&mut quality, &state.quality) {
                output[output_index] = input[source_index];
            }
            if let (Some(output), Some(input)) = (&mut origin, &state.origin_quality) {
                output[output_index] = input[source_index];
            }
        }
    }
    state.width = out_width;
    state.height = out_height;
    state.rgba = rgba;
    state.values = values;
    state.mapped = mapped;
    state.quality = quality;
    state.origin_quality = origin;
    Ok(())
}

fn legacy_crop(
    state: &mut PixelState,
    step: &Map<String, Value>,
    pixel_limit: u64,
    temp_limit: u64,
    memory: &RasterMemoryBudget,
) -> Result<(), TransformFailure> {
    if state.legacy.is_some() {
        return Err(TransformFailure::Invalid("legacy crop cannot follow a palette decode".into()));
    }
    let values = step
        .get("bounds")
        .and_then(Value::as_array)
        .filter(|values| values.len() == 4)
        .ok_or_else(|| {
            TransformFailure::Invalid(
                "legacy crop requires exact integer Pillow crop bounds".into(),
            )
        })?;
    let coordinates =
        values.iter().map(Value::as_i64).collect::<Option<Vec<_>>>().ok_or_else(|| {
            TransformFailure::Invalid(
                "legacy crop requires exact integer Pillow crop bounds".into(),
            )
        })?;
    let [left, top, right, bottom] = <[i64; 4]>::try_from(coordinates).map_err(|_| {
        TransformFailure::Invalid("legacy crop requires exact integer Pillow crop bounds".into())
    })?;
    let fill = rgb(step.get("pad_rgb"), "legacy crop pad_rgb")?;
    let out_width = right.checked_sub(left).filter(|width| *width > 0).ok_or_else(|| {
        TransformFailure::Invalid("legacy crop bounds must have positive extent".into())
    })?;
    let out_height = bottom.checked_sub(top).filter(|height| *height > 0).ok_or_else(|| {
        TransformFailure::Invalid("legacy crop bounds must have positive extent".into())
    })?;
    let out_width_u32 = u32::try_from(out_width)
        .map_err(|_| TransformFailure::Limit("legacy crop dimensions overflow".into()))?;
    let out_height_u32 = u32::try_from(out_height)
        .map_err(|_| TransformFailure::Limit("legacy crop dimensions overflow".into()))?;
    let out_pixel_count = u64::from(out_width_u32)
        .checked_mul(u64::from(out_height_u32))
        .ok_or_else(|| TransformFailure::Limit("legacy crop dimensions overflow".into()))?;
    check_transform_memory(out_pixel_count, pixel_limit, temp_limit)?;
    let _output_lease =
        reserve_memory(memory, out_pixel_count, if state.quality.is_some() { 28 } else { 20 })?;
    let out_pixels = out_pixel_count as usize;
    let mut rgba = try_filled(out_pixels * 4, 255_u8)?;
    for index in 0..out_pixels {
        rgba[pixel_offset(index)..pixel_offset(index) + 3].copy_from_slice(&fill);
    }
    let input_width = i64::from(state.width);
    let input_height = i64::from(state.height);
    let mut quality = state
        .quality
        .as_ref()
        .map(|_| try_filled(out_pixels, QUALITY_OUTSIDE_COVERAGE).map_err(|error| error))
        .transpose()?;
    let mut origin = state
        .origin_quality
        .as_ref()
        .map(|_| try_filled(out_pixels, QUALITY_OUTSIDE_COVERAGE).map_err(|error| error))
        .transpose()?;
    for output_y in 0..out_height {
        let source_y = top + output_y;
        if source_y < 0 || source_y >= input_height {
            continue;
        }
        for output_x in 0..out_width {
            let source_x = left + output_x;
            if source_x < 0 || source_x >= input_width {
                continue;
            }
            let source_index = source_y as usize * input_width as usize + source_x as usize;
            let output_index = output_y as usize * out_width as usize + output_x as usize;
            rgba[pixel_offset(output_index)..pixel_offset(output_index) + 4].copy_from_slice(
                &state.rgba[pixel_offset(source_index)..pixel_offset(source_index) + 4],
            );
            if let (Some(output), Some(input)) = (&mut quality, &state.quality) {
                output[output_index] = input[source_index];
            }
            if let (Some(output), Some(input)) = (&mut origin, &state.origin_quality) {
                output[output_index] = input[source_index];
            }
        }
    }
    state.width = out_width_u32;
    state.height = out_height_u32;
    state.rgba = rgba;
    state.values = try_filled(out_pixels, f64::NAN)?;
    state.mapped = try_filled(out_pixels, false)?;
    state.quality = quality;
    state.origin_quality = origin;
    Ok(())
}

fn repair_gaps(
    state: &mut PixelState,
    step: &Map<String, Value>,
    pixel_limit: u64,
    temp_limit: u64,
    memory: &RasterMemoryBudget,
    cancellation: Option<&CancellationToken>,
) -> Result<(), TransformFailure> {
    if let Some(legacy) = state.legacy.as_mut() {
        if !state.has_palette || state.encoded {
            return Err(TransformFailure::Invalid(
                "legacy gap repair needs a preceding legacy palette".into(),
            ));
        }
        match step.get("method").and_then(Value::as_str) {
            Some("zero_invalid") => {
                for (code, invalid) in legacy.codes.iter_mut().zip(&legacy.invalid) {
                    if *invalid {
                        *code = 0;
                    }
                }
                return Ok(());
            }
            Some("opencv_inpaint") => {
                let method =
                    step.get("inpaint_method").and_then(Value::as_str).unwrap_or("unknown");
                if method != "NS" {
                    return Err(TransformFailure::Invalid(
                        "legacy OpenCV inpaint method is unsupported".into(),
                    ));
                }
                let radius = finite_value(step.get("radius"), "inpaint radius")?;
                if radius <= 0.0 {
                    return Err(TransformFailure::Invalid(
                        "inpaint radius must be positive".into(),
                    ));
                }
                let repaired = opencv_ns_inpaint_codes(
                    state.width,
                    state.height,
                    &mut legacy.codes,
                    &legacy.invalid,
                    radius,
                    pixel_limit,
                    temp_limit,
                    memory,
                    cancellation,
                )?;
                if let Some(quality) = &mut state.quality {
                    for (index, did_repair) in repaired.into_iter().enumerate() {
                        if did_repair
                            && quality[index] & QUALITY_UNKNOWN_COLOR != 0
                            && quality[index] & (QUALITY_MISSING | QUALITY_OUTSIDE_COVERAGE) == 0
                            && legacy.codes[index] <= 224
                        {
                            quality[index] &= !QUALITY_UNKNOWN_COLOR;
                            quality[index] |= QUALITY_RECOVERED;
                        }
                    }
                }
                return Ok(());
            }
            _ => {
                return Err(TransformFailure::Invalid(
                    "unsupported legacy gap repair method".into(),
                ));
            }
        }
    }
    if !state.has_palette
        || state.encoded
        || step.get("method").and_then(Value::as_str) != Some("four_equal_neighbors")
    {
        return Err(TransformFailure::Invalid(
            "unsupported gap repair; exact source method evidence required".into(),
        ));
    }
    let passes = positive_value(step.get("max_passes"), "gap max_passes")?;
    if passes > 8 {
        return Err(TransformFailure::Invalid("gap repair iteration budget exceeded".into()));
    }
    let width = state.width as usize;
    let height = state.height as usize;
    for _ in 0..passes {
        let mut repair = vec![false; state.values.len()];
        for y in 1..height.saturating_sub(1) {
            for x in 1..width.saturating_sub(1) {
                let index = y * width + x;
                if state.rgba[pixel_offset(index) + 3] != 0 {
                    continue;
                }
                let neighbors = [index - width, index + width, index - 1, index + 1];
                let first = neighbors[0];
                if neighbors.iter().all(|neighbor| {
                    state.mapped[*neighbor] && state.values[*neighbor] == state.values[first]
                }) {
                    repair[index] = true;
                }
            }
        }
        if !repair.iter().any(|item| *item) {
            break;
        }
        for (index, should_repair) in repair.into_iter().enumerate() {
            if should_repair {
                state.values[index] = state.values[index - width];
                state.rgba[pixel_offset(index) + 3] = 255;
                state.mapped[index] = true;
            }
        }
    }
    Ok(())
}

/*
 * This Rust implementation follows the 8-bit, single-channel Navier-Stokes
 * path in OpenCV 4.14.0 modules/photo/src/inpaint.cpp.
 *
 * OpenCV Intel License Agreement notice:
 * Copyright (C) 2000, Intel Corporation, all rights reserved.
 * Third party copyrights are property of their respective owners.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions are met:
 *
 * 1. Redistributions of source code must retain the above copyright notice,
 *    this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright notice,
 *    this list of conditions and the following disclaimer in the documentation
 *    and/or other materials provided with the distribution.
 * 3. Neither the name of Intel Corporation nor the names of its contributors
 *    may be used to endorse or promote products derived from this software
 *    without specific prior written permission.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
 * AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
 * ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
 * LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
 * CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
 * SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
 * INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
 * CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
 * ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
 * POSSIBILITY OF SUCH DAMAGE.
 */
#[derive(Clone, Copy, Debug)]
struct InpaintQueueItem {
    time: f32,
    order: u64,
    index: usize,
}

impl PartialEq for InpaintQueueItem {
    fn eq(&self, other: &Self) -> bool {
        self.time.to_bits() == other.time.to_bits()
            && self.order == other.order
            && self.index == other.index
    }
}

impl Eq for InpaintQueueItem {}

impl PartialOrd for InpaintQueueItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for InpaintQueueItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other.time.total_cmp(&self.time).then_with(|| other.order.cmp(&self.order))
    }
}

fn opencv_ns_inpaint_codes(
    width: u32,
    height: u32,
    codes: &mut [u16],
    invalid: &[bool],
    radius: f64,
    pixel_limit: u64,
    temp_limit: u64,
    memory: &RasterMemoryBudget,
    cancellation: Option<&CancellationToken>,
) -> Result<Vec<bool>, TransformFailure> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| TransformFailure::Limit("inpaint dimensions overflow".into()))?;
    if pixels > pixel_limit || codes.len() as u64 != pixels || invalid.len() as u64 != pixels {
        return Err(TransformFailure::Limit(
            "legacy inpaint dimensions exceed the verified pixel extent".into(),
        ));
    }
    if !radius.is_finite() || radius <= 0.0 {
        return Err(TransformFailure::Invalid("inpaint radius must be positive".into()));
    }
    if !invalid.iter().any(|item| *item) {
        return Ok(vec![false; invalid.len()]);
    }
    if width < 2 || height < 2 {
        return Err(TransformFailure::Invalid(
            "legacy OpenCV NS inpaint requires at least a 2 × 2 image".into(),
        ));
    }

    let width = usize::try_from(width)
        .map_err(|_| TransformFailure::Limit("inpaint width overflow".into()))?;
    let height = usize::try_from(height)
        .map_err(|_| TransformFailure::Limit("inpaint height overflow".into()))?;
    let padded_width = width
        .checked_add(2)
        .ok_or_else(|| TransformFailure::Limit("inpaint width overflow".into()))?;
    let padded_height = height
        .checked_add(2)
        .ok_or_else(|| TransformFailure::Limit("inpaint height overflow".into()))?;
    let padded_pixels = padded_width
        .checked_mul(padded_height)
        .ok_or_else(|| TransformFailure::Limit("inpaint dimensions overflow".into()))?;
    let pixels_usize = width
        .checked_mul(height)
        .ok_or_else(|| TransformFailure::Limit("inpaint dimensions overflow".into()))?;
    let base_bytes = pixels
        .checked_mul(32)
        .ok_or_else(|| TransformFailure::Limit("inpaint memory size overflow".into()))?;
    let padded_bytes = u64::try_from(padded_pixels)
        .ok()
        .and_then(|length| {
            length.checked_mul(std::mem::size_of::<u8>() as u64 + std::mem::size_of::<f32>() as u64)
        })
        .ok_or_else(|| TransformFailure::Limit("inpaint memory size overflow".into()))?;
    let output_bytes = pixels
        .checked_mul(std::mem::size_of::<u8>() as u64)
        .ok_or_else(|| TransformFailure::Limit("inpaint memory size overflow".into()))?;
    let queue_bytes = pixels
        .checked_mul(std::mem::size_of::<InpaintQueueItem>() as u64)
        .ok_or_else(|| TransformFailure::Limit("inpaint memory size overflow".into()))?;
    let required_bytes = base_bytes
        .checked_add(padded_bytes)
        .and_then(|bytes| bytes.checked_add(output_bytes))
        .and_then(|bytes| bytes.checked_add(queue_bytes))
        .ok_or_else(|| TransformFailure::Limit("inpaint memory size overflow".into()))?;
    if required_bytes > temp_limit {
        return Err(TransformFailure::Limit(
            "legacy NS inpaint exceeds the temporary memory limit".into(),
        ));
    }
    let scratch_bytes = required_bytes
        .checked_sub(base_bytes)
        .ok_or_else(|| TransformFailure::Limit("inpaint memory size overflow".into()))?;
    let _scratch_lease = memory
        .reserve(scratch_bytes)
        .map_err(|error| TransformFailure::Limit(error.to_string()))?;

    let mut flags = try_filled(padded_pixels, 0_u8)?;
    let mut distance = try_filled(padded_pixels, 1_000_000.0_f32)?;
    let mut output = try_filled(pixels_usize, 0_u8)?;
    let mut repaired = try_filled(pixels_usize, false)?;
    let mut heap = BinaryHeap::new();
    heap.try_reserve(pixels_usize)
        .map_err(|_| TransformFailure::Limit("legacy inpaint allocation failed".into()))?;
    for (index, code) in codes.iter().enumerate() {
        let code = u8::try_from(*code).map_err(|_| {
            TransformFailure::Invalid("legacy inpaint source code exceeds uint8".into())
        })?;
        output[index] = code;
        if invalid[index] {
            let y = index / width + 1;
            let x = index % width + 1;
            flags[y * padded_width + x] = 2; // INSIDE
        }
    }

    // OpenCV's 3×3 cross dilation minus the source mask forms the initial band.
    for index in 0..pixels_usize {
        if !invalid[index] {
            continue;
        }
        let y = index / width + 1;
        let x = index % width + 1;
        for (next_y, next_x) in [(y - 1, x), (y, x - 1), (y + 1, x), (y, x + 1)] {
            if next_y > 0 && next_y < padded_height - 1 && next_x > 0 && next_x < padded_width - 1 {
                let next = next_y * padded_width + next_x;
                if flags[next] == 0 {
                    flags[next] = 1; // BAND
                    distance[next] = 0.0;
                }
            }
        }
    }

    let mut order = 0_u64;
    for (index, flag) in flags.iter().enumerate() {
        if *flag == 1 {
            heap.push(InpaintQueueItem { time: 0.0, order, index });
            order = order
                .checked_add(1)
                .ok_or_else(|| TransformFailure::Limit("inpaint queue order overflow".into()))?;
        }
    }

    let rows = i32::try_from(padded_height)
        .map_err(|_| TransformFailure::Limit("inpaint height overflow".into()))?;
    let cols = i32::try_from(padded_width)
        .map_err(|_| TransformFailure::Limit("inpaint width overflow".into()))?;
    let stride = padded_width;
    let range = if radius >= 100.5 {
        100_i32
    } else {
        let floor = radius.floor();
        let fraction = radius - floor;
        let rounded = if fraction < 0.5 {
            floor
        } else if fraction > 0.5 || (floor as u64) % 2 == 1 {
            floor + 1.0
        } else {
            floor
        };
        (rounded as i32).clamp(1, 100)
    };

    while let Some(item) = heap.pop() {
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err(TransformFailure::Cancelled);
        }
        let ii = i32::try_from(item.index / stride)
            .map_err(|_| TransformFailure::Limit("inpaint row overflow".into()))?;
        let jj = i32::try_from(item.index % stride)
            .map_err(|_| TransformFailure::Limit("inpaint column overflow".into()))?;
        flags[item.index] = 0; // KNOWN
        for (i, j) in [(ii - 1, jj), (ii, jj - 1), (ii + 1, jj), (ii, jj + 1)] {
            if i <= 0 || j <= 0 || i > rows - 1 || j > cols - 1 {
                continue;
            }
            let index = i as usize * stride + j as usize;
            if flags[index] != 2 {
                continue;
            }
            let dist = min4(
                fast_marching_solve(i - 1, j, i, j - 1, stride, &flags, &distance),
                fast_marching_solve(i + 1, j, i, j - 1, stride, &flags, &distance),
                fast_marching_solve(i - 1, j, i, j + 1, stride, &flags, &distance),
                fast_marching_solve(i + 1, j, i, j + 1, stride, &flags, &distance),
            );
            distance[index] = dist;

            let mut intensity_sum = 0.0_f32;
            let mut weight_sum = 1.0e-20_f32;
            for k in i - range..=i + range {
                let km = k - 1 + i32::from(k == 1);
                let kp = k - 1 - i32::from(k == rows - 2);
                for l in j - range..=j + range {
                    let lm = l - 1 + i32::from(l == 1);
                    let lp = l - 1 - i32::from(l == cols - 2);
                    if k <= 0 || l <= 0 || k >= rows - 1 || l >= cols - 1 {
                        continue;
                    }
                    let sample_index = k as usize * stride + l as usize;
                    if flags[sample_index] == 2
                        || (l - j) * (l - j) + (k - i) * (k - i) > range * range
                    {
                        continue;
                    }

                    let rx = (j - l) as f32;
                    let ry = (i - k) as f32;
                    let r_length = rx * rx + ry * ry;
                    let dst = 1.0_f32 / (r_length * r_length + 1.0_f32);

                    let grad_x = if flags[(k + 1) as usize * stride + l as usize] != 2 {
                        if flags[(k - 1) as usize * stride + l as usize] != 2 {
                            (i32::from(read_inpaint_pixel(&output, width, kp + 1, lm)?)
                                - i32::from(read_inpaint_pixel(&output, width, kp, lm)?))
                            .abs() as f32
                                + (i32::from(read_inpaint_pixel(&output, width, kp, lm)?)
                                    - i32::from(read_inpaint_pixel(&output, width, km - 1, lm)?))
                                .abs() as f32
                        } else {
                            ((i32::from(read_inpaint_pixel(&output, width, kp + 1, lm)?)
                                - i32::from(read_inpaint_pixel(&output, width, kp, lm)?))
                            .abs() as f32)
                                * 2.0_f32
                        }
                    } else if flags[(k - 1) as usize * stride + l as usize] != 2 {
                        ((i32::from(read_inpaint_pixel(&output, width, kp, lm)?)
                            - i32::from(read_inpaint_pixel(&output, width, km - 1, lm)?))
                        .abs() as f32)
                            * 2.0_f32
                    } else {
                        0.0_f32
                    };
                    let grad_y = if flags[k as usize * stride + (l + 1) as usize] != 2 {
                        if flags[k as usize * stride + (l - 1) as usize] != 2 {
                            (i32::from(read_inpaint_pixel(&output, width, km, lp + 1)?)
                                - i32::from(read_inpaint_pixel(&output, width, km, lm)?))
                            .abs() as f32
                                + (i32::from(read_inpaint_pixel(&output, width, km, lm)?)
                                    - i32::from(read_inpaint_pixel(&output, width, km, lm - 1)?))
                                .abs() as f32
                        } else {
                            ((i32::from(read_inpaint_pixel(&output, width, km, lp + 1)?)
                                - i32::from(read_inpaint_pixel(&output, width, km, lm)?))
                            .abs() as f32)
                                * 2.0_f32
                        }
                    } else if flags[k as usize * stride + (l - 1) as usize] != 2 {
                        ((i32::from(read_inpaint_pixel(&output, width, km, lm)?)
                            - i32::from(read_inpaint_pixel(&output, width, km, lm - 1)?))
                        .abs() as f32)
                            * 2.0_f32
                    } else {
                        0.0_f32
                    };
                    let grad_x = -grad_x;
                    let direction_dot = rx * grad_x + ry * grad_y;
                    let direction = if direction_dot.abs() <= 0.01_f32 {
                        0.000001_f32
                    } else {
                        (direction_dot / (r_length * (grad_x * grad_x + grad_y * grad_y)).sqrt())
                            .abs()
                    };
                    let weight = dst * direction;
                    intensity_sum +=
                        weight * f32::from(read_inpaint_pixel(&output, width, k - 1, l - 1)?);
                    weight_sum += weight;
                }
            }
            let value = f64::from(intensity_sum) / f64::from(weight_sum);
            output[(i as usize - 1) * width + (j as usize - 1)] =
                value.round_ties_even().clamp(0.0, 255.0) as u8;
            repaired[(i as usize - 1) * width + (j as usize - 1)] = true;
            flags[index] = 1; // BAND
            heap.push(InpaintQueueItem { time: dist, order, index });
            order = order
                .checked_add(1)
                .ok_or_else(|| TransformFailure::Limit("inpaint queue order overflow".into()))?;
        }
    }

    for (code, value) in codes.iter_mut().zip(output) {
        *code = u16::from(value);
    }
    Ok(repaired)
}

fn read_inpaint_pixel(
    output: &[u8],
    width: usize,
    row: i32,
    column: i32,
) -> Result<u8, TransformFailure> {
    let row = usize::try_from(row).map_err(|_| {
        TransformFailure::Invalid("OpenCV NS inpaint sample is outside image".into())
    })?;
    let column = usize::try_from(column).map_err(|_| {
        TransformFailure::Invalid("OpenCV NS inpaint sample is outside image".into())
    })?;
    output
        .get(
            row.checked_mul(width)
                .and_then(|offset| offset.checked_add(column))
                .unwrap_or(usize::MAX),
        )
        .copied()
        .ok_or_else(|| {
            TransformFailure::Invalid("OpenCV NS inpaint sample is outside image".into())
        })
}

fn min4(a: f32, b: f32, c: f32, d: f32) -> f32 {
    a.min(b).min(c.min(d))
}

fn fast_marching_solve(
    i1: i32,
    j1: i32,
    i2: i32,
    j2: i32,
    stride: usize,
    flags: &[u8],
    times: &[f32],
) -> f32 {
    let index1 = i1 as usize * stride + j1 as usize;
    let index2 = i2 as usize * stride + j2 as usize;
    let a11 = f64::from(times[index1]);
    let a22 = f64::from(times[index2]);
    let minimum = a11.min(a22);
    let solution = if flags[index1] != 2 {
        if flags[index2] != 2 {
            if (a11 - a22).abs() >= 1.0 {
                1.0 + minimum
            } else {
                (a11 + a22 + (2.0 - (a11 - a22) * (a11 - a22)).sqrt()) * 0.5
            }
        } else {
            1.0 + a11
        }
    } else if flags[index2] != 2 {
        1.0 + a22
    } else {
        1.0 + minimum
    };
    solution as f32
}

fn encode_gray(state: &mut PixelState, step: &Map<String, Value>) -> Result<(), TransformFailure> {
    if !state.has_palette || state.encoded {
        return Err(TransformFailure::Invalid(
            "gray encoding requires one provenance-bound palette".into(),
        ));
    }
    if let Some(legacy) = state.legacy.as_mut() {
        if step.get("method").and_then(Value::as_str) != Some("legacy_cookbook") {
            return Err(TransformFailure::Invalid(
                "legacy palette requires explicit cookbook encoding".into(),
            ));
        }
        for index in 0..legacy.codes.len() {
            if !legacy.keep[index] {
                legacy.codes[index] = 0;
            }
            let code = legacy.codes[index] as usize;
            if code >= legacy.cookbook.len() {
                return Err(TransformFailure::Invalid(
                    "inpainted legacy code exceeds the source cookbook".into(),
                ));
            }
            let value = if legacy.keep[index] { legacy.cookbook[code] } else { 0.0 };
            let scaled = value * 3.2_f32;
            let gray = scaled.clamp(0.0, 224.0) as u8;
            write_gray(&mut state.rgba, index, gray);
            state.rgba[pixel_offset(index) + 3] = 255;
        }
    } else if state.channel_method.is_some() {
        if step.get("method").and_then(Value::as_str) != Some("legacy_tile_uint8_then_clip_224") {
            return Err(TransformFailure::Invalid(
                "legacy tile channel decode requires its exact byte encoding".into(),
            ));
        }
        let arithmetic_dtype = step.get("arithmetic_dtype").and_then(Value::as_str);
        if ![Some("float32"), Some("float64")].contains(&arithmetic_dtype) {
            return Err(TransformFailure::Invalid(
                "legacy tile gray encoding requires the old arithmetic dtype".into(),
            ));
        }
        for index in 0..state.values.len() {
            let scaled = if arithmetic_dtype == Some("float32") {
                (state.values[index] as f32 / 5.0_f32 * 16.0_f32) as f64
            } else {
                state.values[index] / 5.0_f64 * 16.0_f64
            };
            let gray = float_to_wrapping_u8(scaled).min(224);
            write_gray(&mut state.rgba, index, gray);
            state.rgba[pixel_offset(index) + 3] = 255;
        }
    } else {
        for index in 0..state.values.len() {
            let value = if state.values[index].is_nan() { 0.0 } else { state.values[index] };
            let gray = (value * 16.0 / 5.0).clamp(0.0, 224.0) as u8;
            write_gray(&mut state.rgba, index, gray);
        }
    }
    state.encoded = true;
    Ok(())
}

fn float_to_wrapping_u8(value: f64) -> u8 {
    if !value.is_finite() {
        return 0;
    }
    value.trunc().rem_euclid(256.0) as u8
}

fn write_gray(rgba: &mut [u8], index: usize, value: u8) {
    let offset = pixel_offset(index);
    rgba[offset] = value;
    rgba[offset + 1] = value;
    rgba[offset + 2] = value;
}

fn encode_luminance_alpha(state: &mut PixelState) -> Result<(), TransformFailure> {
    if state.has_palette || state.encoded || state.legacy.is_some() {
        return Err(TransformFailure::Invalid(
            "legacy luminance conversion must be an explicit standalone encoding".into(),
        ));
    }
    for index in 0..state.values.len() {
        let offset = pixel_offset(index);
        let red = f32::from(state.rgba[offset]);
        let green = f32::from(state.rgba[offset + 1]);
        let blue = f32::from(state.rgba[offset + 2]);
        let alpha = f32::from(state.rgba[offset + 3]) / 255.0_f32;
        if alpha > 0.0 && alpha < 1.0 {
            mark_quality(state, index, QUALITY_SOURCE_ANNOTATION);
        }
        let mut luminance = 0.299_f32 * red + 0.587_f32 * green + 0.114_f32 * blue;
        luminance *= alpha;
        let scaled = luminance / 255.0_f32 * 224.0_f32;
        let gray = scaled.clamp(0.0, 224.0) as u8;
        write_gray(&mut state.rgba, index, gray);
        state.rgba[offset + 3] = 255;
    }
    state.encoded = true;
    Ok(())
}

fn resize_encoded_gray(
    state: &mut PixelState,
    step: &Map<String, Value>,
    pixel_limit: u64,
    temp_limit: u64,
    memory: &RasterMemoryBudget,
    cancellation: Option<&CancellationToken>,
) -> Result<(), TransformFailure> {
    let shape =
        step.get("shape").and_then(Value::as_array).filter(|shape| shape.len() == 2).ok_or_else(
            || {
                TransformFailure::Invalid(
                    "resize requires exact dimensions and supported source method".into(),
                )
            },
        )?;
    let out_width = u32::try_from(positive_value(shape.first(), "resize width")?)
        .map_err(|_| TransformFailure::Limit("resize width exceeds supported dimensions".into()))?;
    let out_height =
        u32::try_from(positive_value(shape.get(1), "resize height")?).map_err(|_| {
            TransformFailure::Limit("resize height exceeds supported dimensions".into())
        })?;
    let method = step.get("method").and_then(Value::as_str).unwrap_or("unknown");
    if !["nearest", "pillow_bicubic"].contains(&method) {
        return Err(TransformFailure::Invalid(format!(
            "legacy resize method {method} is unsupported"
        )));
    }
    let output_pixels = u64::from(out_width)
        .checked_mul(u64::from(out_height))
        .ok_or_else(|| TransformFailure::Limit("resize dimensions overflow".into()))?;
    let input_pixels = u64::from(state.width)
        .checked_mul(u64::from(state.height))
        .ok_or_else(|| TransformFailure::Limit("resize dimensions overflow".into()))?;
    if output_pixels > pixel_limit {
        return Err(TransformFailure::Limit(
            "legacy display resize exceeds the pixel limit".into(),
        ));
    }
    let intermediate_pixels = u64::from(out_width)
        .checked_mul(u64::from(state.height))
        .ok_or_else(|| TransformFailure::Limit("resize dimensions overflow".into()))?;
    let input_working = input_pixels
        .checked_mul(32)
        .ok_or_else(|| TransformFailure::Limit("resize memory size overflow".into()))?;
    let resize_working = intermediate_pixels
        .checked_add(
            output_pixels
                .checked_mul(10)
                .ok_or_else(|| TransformFailure::Limit("resize memory size overflow".into()))?,
        )
        .ok_or_else(|| TransformFailure::Limit("resize memory size overflow".into()))?;
    let peak_working = input_working
        .checked_add(resize_working)
        .ok_or_else(|| TransformFailure::Limit("resize memory size overflow".into()))?;
    if peak_working > temp_limit {
        return Err(TransformFailure::Limit(
            "legacy display resize exceeds the temporary memory limit".into(),
        ));
    }
    let _resize_lease = memory
        .reserve(resize_working)
        .map_err(|error| TransformFailure::Limit(error.to_string()))?;

    let gray =
        resize_u8_channel(&state.rgba, state.width, state.height, out_width, out_height, method)?;
    let quality = state
        .quality
        .as_deref()
        .map(|values| {
            propagate_quality_for_resize_inner(
                state.width as usize,
                state.height as usize,
                out_width as usize,
                out_height as usize,
                values,
                method,
                true,
                cancellation,
            )
        })
        .transpose()?;
    let origin_quality = state
        .origin_quality
        .as_deref()
        .map(|values| {
            propagate_quality_for_resize_inner(
                state.width as usize,
                state.height as usize,
                out_width as usize,
                out_height as usize,
                values,
                method,
                false,
                cancellation,
            )
        })
        .transpose()?;
    let output_length = usize::try_from(output_pixels)
        .ok()
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| TransformFailure::Limit("resize output size overflow".into()))?;
    let mut rgba = try_filled(output_length, 255_u8)?;
    for (pixel, value) in rgba.chunks_exact_mut(4).zip(gray) {
        pixel[..3].fill(value);
    }
    state.width = out_width;
    state.height = out_height;
    state.rgba = rgba;
    state.quality = quality;
    state.origin_quality = origin_quality;
    Ok(())
}

/// Propagate one gray quality plane through the exact resize support used by
/// the historical display transform. Every nonzero bicubic coefficient,
/// including negative weights, contributes to the output mask.
pub fn propagate_quality_for_resize(
    input_width: usize,
    input_height: usize,
    output_width: usize,
    output_height: usize,
    quality: &[u16],
    method: &str,
) -> CoreResult<Vec<u16>> {
    let limits = Limits::default();
    let input_pixels = input_width
        .checked_mul(input_height)
        .ok_or_else(|| CoreError::ResourceLimit("quality resize input shape overflow".into()))?;
    let output_pixels = output_width
        .checked_mul(output_height)
        .ok_or_else(|| CoreError::ResourceLimit("quality resize output shape overflow".into()))?;
    if input_width == 0 || input_height == 0 || output_width == 0 || output_height == 0 {
        return Err(display_error("quality resize dimensions must be positive"));
    }
    limits.validate_pixels(input_pixels as u64)?;
    limits.validate_pixels(output_pixels as u64)?;
    if quality.len() != input_pixels {
        return Err(display_error("quality plane does not match resize input shape"));
    }
    let _lease = limits.raster_memory_budget().reserve_shape(
        output_height as u64,
        output_width as u64,
        &[2],
    )?;
    let method = if method == "pillow_bicubic" { "pillow_bicubic" } else { method };
    propagate_quality_for_resize_inner(
        input_width,
        input_height,
        output_width,
        output_height,
        quality,
        method,
        true,
        None,
    )
    .map_err(|failure| match failure {
        TransformFailure::Limit(message) => CoreError::ResourceLimit(message),
        TransformFailure::Invalid(message) => display_error(message),
        TransformFailure::Cancelled => CoreError::Cancelled,
    })
    .and_then(|output| {
        if output.len() == output_pixels {
            Ok(output)
        } else {
            Err(display_error("quality resize produced an invalid output shape"))
        }
    })
}

fn propagate_quality_for_resize_inner(
    input_width: usize,
    input_height: usize,
    output_width: usize,
    output_height: usize,
    quality: &[u16],
    method: &str,
    mark_interpolated: bool,
    cancellation: Option<&CancellationToken>,
) -> Result<Vec<u16>, TransformFailure> {
    let input_pixels = input_width
        .checked_mul(input_height)
        .ok_or_else(|| TransformFailure::Limit("quality resize input shape overflow".into()))?;
    let output_pixels = output_width
        .checked_mul(output_height)
        .ok_or_else(|| TransformFailure::Limit("quality resize output shape overflow".into()))?;
    if input_width == 0 || input_height == 0 || output_width == 0 || output_height == 0 {
        return Err(TransformFailure::Invalid("quality resize dimensions must be positive".into()));
    }
    if quality.len() != input_pixels {
        return Err(TransformFailure::Invalid(
            "quality plane does not match resize input shape".into(),
        ));
    }
    if method == "nearest" {
        let mut output = try_filled(output_pixels, 0_u16)?;
        for y in 0..output_height {
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                return Err(TransformFailure::Cancelled);
            }
            let source_y = ((2 * y + 1) * input_height / (2 * output_height)).min(input_height - 1);
            for x in 0..output_width {
                let source_x =
                    ((2 * x + 1) * input_width / (2 * output_width)).min(input_width - 1);
                output[y * output_width + x] = quality[source_y * input_width + source_x];
            }
        }
        return Ok(output);
    }
    if method != "pillow_bicubic" {
        return Err(TransformFailure::Invalid(format!(
            "unsupported quality resize method {method}"
        )));
    }
    let horizontal = pillow_bicubic_kernels(input_width, output_width)?;
    let vertical = pillow_bicubic_kernels(input_height, output_height)?;
    let intermediate_len = input_height
        .checked_mul(output_width)
        .ok_or_else(|| TransformFailure::Limit("quality resize intermediate overflow".into()))?;
    let mut intermediate = try_filled(intermediate_len, 0_u16)?;
    let mut horizontal_multi = try_filled(output_width, false)?;
    for (x, kernel) in horizontal.iter().enumerate() {
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err(TransformFailure::Cancelled);
        }
        let contributing = kernel.weights.iter().filter(|weight| **weight != 0).count();
        horizontal_multi[x] = contributing > 1;
        for y in 0..input_height {
            let mut flags = 0_u16;
            for (offset, weight) in kernel.weights.iter().enumerate() {
                if *weight != 0 {
                    flags |= quality[y * input_width + kernel.start + offset];
                }
            }
            intermediate[y * output_width + x] = flags;
        }
    }
    let mut output = try_filled(output_pixels, 0_u16)?;
    for (y, kernel) in vertical.iter().enumerate() {
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err(TransformFailure::Cancelled);
        }
        let vertical_multi = kernel.weights.iter().filter(|weight| **weight != 0).count() > 1;
        for x in 0..output_width {
            let mut flags = 0_u16;
            for (offset, weight) in kernel.weights.iter().enumerate() {
                if *weight != 0 {
                    flags |= intermediate[(kernel.start + offset) * output_width + x];
                }
            }
            if mark_interpolated && (horizontal_multi[x] || vertical_multi) {
                flags |= QUALITY_INTERPOLATED;
            }
            output[y * output_width + x] = flags;
        }
    }
    Ok(output)
}

#[derive(Debug)]
struct ResampleKernel {
    start: usize,
    weights: Vec<i32>,
}

// Pillow's 8-bit resampler uses signed 22-bit fixed-point coefficients and
// clips after each separable pass. Keeping those details avoids the ±1 pixel
// drift produced by generic floating-point image resize implementations.
const PILLOW_PRECISION_BITS: u32 = 22;

fn resize_u8_channel(
    rgba: &[u8],
    input_width: u32,
    input_height: u32,
    output_width: u32,
    output_height: u32,
    method: &str,
) -> Result<Vec<u8>, TransformFailure> {
    resize_u8_channel_with_parallelism(
        rgba,
        input_width,
        input_height,
        output_width,
        output_height,
        method,
        None,
    )
}

fn resize_u8_channel_with_parallelism(
    rgba: &[u8],
    input_width: u32,
    input_height: u32,
    output_width: u32,
    output_height: u32,
    method: &str,
    force_parallel: Option<bool>,
) -> Result<Vec<u8>, TransformFailure> {
    let input_width = usize::try_from(input_width)
        .map_err(|_| TransformFailure::Limit("resize input width overflow".into()))?;
    let input_height = usize::try_from(input_height)
        .map_err(|_| TransformFailure::Limit("resize input height overflow".into()))?;
    let output_width = usize::try_from(output_width)
        .map_err(|_| TransformFailure::Limit("resize output width overflow".into()))?;
    let output_height = usize::try_from(output_height)
        .map_err(|_| TransformFailure::Limit("resize output height overflow".into()))?;
    let input_length = input_width
        .checked_mul(input_height)
        .ok_or_else(|| TransformFailure::Limit("resize input size overflow".into()))?;
    if rgba.len()
        != input_length
            .checked_mul(4)
            .ok_or_else(|| TransformFailure::Limit("resize input size overflow".into()))?
    {
        return Err(TransformFailure::Invalid(
            "resize input RGBA buffer has an invalid length".into(),
        ));
    }
    let output_length = output_width
        .checked_mul(output_height)
        .ok_or_else(|| TransformFailure::Limit("resize output size overflow".into()))?;
    let pool = force_parallel
        .unwrap_or(input_length >= PARALLEL_RESIZE_MIN_PIXELS)
        .then(resize_parallel_pool)
        .flatten();

    if method == "nearest" {
        let mut output = try_filled(output_length, 0_u8)?;
        let resize_row = |y: usize, output_row: &mut [u8]| {
            let source_y = ((2 * y + 1) * input_height / (2 * output_height)).min(input_height - 1);
            for (x, output_value) in output_row.iter_mut().enumerate() {
                let source_x =
                    ((2 * x + 1) * input_width / (2 * output_width)).min(input_width - 1);
                *output_value = rgba[(source_y * input_width + source_x) * 4];
            }
        };
        if let Some(pool) = pool {
            pool.install(|| {
                use rayon::prelude::*;
                output
                    .par_chunks_mut(output_width)
                    .enumerate()
                    .for_each(|(y, row)| resize_row(y, row));
            });
        } else {
            for (y, row) in output.chunks_mut(output_width).enumerate() {
                resize_row(y, row);
            }
        }
        return Ok(output);
    }

    let horizontal = pillow_bicubic_kernels(input_width, output_width)?;
    let vertical = pillow_bicubic_kernels(input_height, output_height)?;
    let intermediate_length = output_width
        .checked_mul(input_height)
        .ok_or_else(|| TransformFailure::Limit("resize intermediate size overflow".into()))?;
    let mut intermediate = try_filled(intermediate_length, 0_u8)?;
    let rounding = 1_i64 << (PILLOW_PRECISION_BITS - 1);
    let resize_horizontal_row = |y: usize, output_row: &mut [u8]| {
        let source_row = y * input_width;
        for (x, kernel) in horizontal.iter().enumerate() {
            let mut accumulator = 0_i64;
            for (offset, weight) in kernel.weights.iter().enumerate() {
                accumulator +=
                    i64::from(rgba[(source_row + kernel.start + offset) * 4]) * i64::from(*weight);
            }
            output_row[x] = ((accumulator + rounding) >> PILLOW_PRECISION_BITS).clamp(0, 255) as u8;
        }
    };
    if let Some(pool) = pool {
        pool.install(|| {
            use rayon::prelude::*;
            intermediate
                .par_chunks_mut(output_width)
                .enumerate()
                .for_each(|(y, row)| resize_horizontal_row(y, row));
        });
    } else {
        for (y, row) in intermediate.chunks_mut(output_width).enumerate() {
            resize_horizontal_row(y, row);
        }
    }

    let mut output = try_filled(output_length, 0_u8)?;
    let resize_vertical_row = |y: usize, output_row: &mut [u8]| {
        let kernel = &vertical[y];
        for (x, output_value) in output_row.iter_mut().enumerate() {
            let mut accumulator = 0_i64;
            for (offset, weight) in kernel.weights.iter().enumerate() {
                accumulator += i64::from(intermediate[(kernel.start + offset) * output_width + x])
                    * i64::from(*weight);
            }
            *output_value = ((accumulator + rounding) >> PILLOW_PRECISION_BITS).clamp(0, 255) as u8;
        }
    };
    if let Some(pool) = pool {
        pool.install(|| {
            use rayon::prelude::*;
            output
                .par_chunks_mut(output_width)
                .enumerate()
                .for_each(|(y, row)| resize_vertical_row(y, row));
        });
    } else {
        for (y, row) in output.chunks_mut(output_width).enumerate() {
            resize_vertical_row(y, row);
        }
    }
    Ok(output)
}

fn resize_parallel_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let thread_count = std::thread::available_parallelism().map_or(1, usize::from).min(4);
        if thread_count < 2 {
            return None;
        }
        rayon::ThreadPoolBuilder::new().num_threads(thread_count).build().ok()
    })
    .as_ref()
}

fn pillow_bicubic_kernels(
    input_size: usize,
    output_size: usize,
) -> Result<Vec<ResampleKernel>, TransformFailure> {
    let scale = input_size as f64 / output_size as f64;
    let filter_scale = scale.max(1.0);
    let support = 2.0 * filter_scale;
    let coefficient_scale = (1_u32 << PILLOW_PRECISION_BITS) as f64;
    let mut kernels = Vec::new();
    kernels
        .try_reserve_exact(output_size)
        .map_err(|_| TransformFailure::Limit("resize kernel allocation failed".into()))?;
    for output_index in 0..output_size {
        let center = (output_index as f64 + 0.5) * scale;
        let first = (center - support + 0.5).trunc().max(0.0) as usize;
        let end = (center + support + 0.5).trunc().min(input_size as f64) as usize;
        if first >= end {
            return Err(TransformFailure::Invalid(
                "legacy resize generated an empty Pillow filter kernel".into(),
            ));
        }
        let mut weights = Vec::new();
        weights
            .try_reserve_exact(end - first)
            .map_err(|_| TransformFailure::Limit("resize kernel allocation failed".into()))?;
        let mut total = 0.0_f64;
        for source_index in first..end {
            let distance = (source_index as f64 + 0.5 - center) / filter_scale;
            let weight = pillow_bicubic(distance);
            weights.push(weight);
            total += weight;
        }
        if !total.is_finite() || total == 0.0 {
            return Err(TransformFailure::Invalid(
                "legacy resize generated invalid Pillow filter weights".into(),
            ));
        }
        let mut fixed_weights = Vec::new();
        fixed_weights
            .try_reserve_exact(weights.len())
            .map_err(|_| TransformFailure::Limit("resize kernel allocation failed".into()))?;
        for weight in weights {
            let scaled = (weight / total) * coefficient_scale;
            let rounded = if scaled < 0.0 { -0.5 + scaled } else { 0.5 + scaled };
            fixed_weights.push(rounded.trunc() as i32);
        }
        kernels.push(ResampleKernel { start: first, weights: fixed_weights });
    }
    Ok(kernels)
}

fn pillow_bicubic(value: f64) -> f64 {
    let value = value.abs();
    if value < 1.0 {
        ((1.5 * value - 2.5) * value) * value + 1.0
    } else if value < 2.0 {
        (((-0.5 * value + 2.5) * value - 4.0) * value) + 2.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_and_historical_resource_roots_resolve_to_the_same_wire_json() {
        assert_eq!(
            embedded_json("gray/fr.json").unwrap(),
            embedded_json("legacy_display/fr.json").unwrap()
        );
        assert_eq!(
            embedded_json("gray/evidence/fr__composite.json").unwrap(),
            embedded_json("legacy_display/evidence/fr__composite.json").unwrap()
        );
    }

    #[test]
    fn embedded_passed_catalog_entries_have_matching_rule_and_evidence() {
        let entries = load_catalog().expect("embedded index is valid");
        let mut passed = 0;
        for entry in entries.iter().filter(|entry| entry.status == "passed") {
            let rule = validate_rule(&embedded_json(entry.rule_file.as_deref().unwrap()).unwrap())
                .unwrap();
            let evidence = embedded_json(entry.evidence_file.as_deref().unwrap()).unwrap();
            assert!(rule.matches(
                &entry.source,
                &entry.product,
                &entry.path_id,
                entry.rule_version.as_deref()
            ));
            assert_eq!(rule.config_hash, entry.config_hash.as_deref().unwrap());
            assert!(validate_evidence(&evidence, &rule).unwrap(), "{}", entry.path_id);
            passed += 1;
        }
        assert_eq!(passed, 15);
        assert_eq!(EMBEDDED_RESOURCES.len(), 35);
    }

    #[test]
    fn pillow_nearest_resize_uses_pixel_center_mapping() {
        let rgba = [0, 0, 0, 255, 1, 1, 1, 255, 2, 2, 2, 255, 3, 3, 3, 255, 4, 4, 4, 255];
        assert_eq!(resize_u8_channel(&rgba, 5, 1, 3, 1, "nearest").unwrap(), [0, 2, 4]);
    }

    #[test]
    fn parallel_pillow_bicubic_matches_serial_output_byte_for_byte() {
        let (width, height) = (513_u32, 509_u32);
        let rgba = (0..width as usize * height as usize)
            .flat_map(|index| {
                let value = (index.wrapping_mul(37) % 256) as u8;
                [value, value.wrapping_add(23), value.wrapping_mul(3), 255]
            })
            .collect::<Vec<_>>();
        let serial = resize_u8_channel_with_parallelism(
            &rgba,
            width,
            height,
            271,
            263,
            "pillow_bicubic",
            Some(false),
        )
        .unwrap();
        let parallel = resize_u8_channel_with_parallelism(
            &rgba,
            width,
            height,
            271,
            263,
            "pillow_bicubic",
            Some(true),
        )
        .unwrap();
        assert_eq!(parallel, serial);
    }

    #[test]
    fn blocked_and_unmatched_paths_preserve_rgba_with_reason() {
        let rgba = vec![17, 31, 47, 123];
        let blocked =
            apply_for_source("bmkg", "composite", None, "PNG", 1, 1, &rgba, &Limits::default())
                .unwrap();
        assert!(!blocked.applied);
        assert_eq!(blocked.rgba, rgba);
        assert!(blocked.reason.unwrap().contains("HTTP 403"));

        let unmatched = apply_for_source(
            "not-a-source",
            "composite",
            None,
            "PNG",
            1,
            1,
            &rgba,
            &Limits::default(),
        )
        .unwrap();
        assert!(!unmatched.applied);
        assert_eq!(unmatched.rgba, rgba);
        assert!(unmatched.reason.unwrap().contains("no validated"));

        let constrained = Limits { max_temp_bytes: 3, ..Limits::default() };
        assert!(matches!(
            apply_for_source("not-a-source", "composite", None, "PNG", 1, 1, &rgba, &constrained),
            Err(CoreError::ResourceLimit(_))
        ));
    }

    #[test]
    fn french_luminance_alpha_rule_applies_after_evidence_validation() {
        let mut rgba = Vec::with_capacity(700 * 600 * 4);
        for _ in 0..(700 * 600) {
            rgba.extend_from_slice(&[255, 0, 0, 128]);
        }
        let preview =
            apply_for_source("fr", "composite", None, "PNG", 700, 600, &rgba, &Limits::default())
                .unwrap();
        assert!(preview.applied);
        assert_eq!(preview.rule_version.as_deref(), Some("old-8d251601-fr-frcomp-replay-v1"));
        assert_eq!(&preview.rgba[..4], &[33, 33, 33, 255]);
    }

    #[test]
    fn rule_fingerprint_change_fails_closed() {
        let mut rule = embedded_json("fr.json").unwrap();
        rule["legacy_reference"] = Value::String("changed reference".into());
        let error = validate_rule(&rule).unwrap_err();
        assert!(error.contains("fingerprint mismatch"));
    }
}
