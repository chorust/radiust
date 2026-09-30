//! Native raw-data adapter for Taiwan CWA's image and numeric-grid products.
//!
//! The image product retains its official companion metadata and is explicitly
//! marked geometry-unverified. The numeric product remains the provider's raw
//! JSON here; its scientific decoder belongs to the separately gated science
//! migration and must preserve the provider's TWD67 grid semantics.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{
    ArtifactReceipt, DiscoveryTarget, FrameRef, Query, RawArtifact, RawFrame, parse_utc_time,
};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use futures_util::stream::{self, StreamExt, TryStreamExt};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "tw";
const STATION: &str = "CV1_3600";
const PRODUCT_OBSERVATION: &str = "observation";
const PRODUCT_GRID: &str = "grid";
const OBSERVATION_KEY: &str = "O-A0058-005";
const GRID_KEY: &str = "O-A0059-001";
const BUCKET_BASE: &str = "https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation";
const HOST: &str = "cwaopendata.s3.ap-northeast-1.amazonaws.com";
const LOCATOR_VERSION: &str = "tw-cwa-v2";
const GRID_HEADER_BYTES: u64 = 64 * 1024;
const GRID_CHUNK_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, PartialEq)]
struct ObservationMetadata {
    valid_time: DateTime<Utc>,
    extent: [f64; 4],
    shape: [u32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GridMetadata {
    pub(crate) valid_time: DateTime<Utc>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) west: f64,
    pub(crate) south: f64,
    pub(crate) step: f64,
}

enum FramePlan {
    Observation { metadata: ObservationMetadata },
    Grid { metadata: GridMetadata },
}

impl FramePlan {
    fn urls(&self) -> Vec<(&'static str, &'static str, &'static str)> {
        match self {
            Self::Observation { .. } => vec![
                (
                    "data",
                    "image/png",
                    "https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation/O-A0058-005.png",
                ),
                (
                    "metadata",
                    "application/json",
                    "https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation/O-A0058-005.json",
                ),
            ],
            Self::Grid { .. } => vec![(
                "data",
                "application/json",
                "https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation/O-A0059-001.json",
            )],
        }
    }
}

pub struct TwSourceAdapter;

impl SourceAdapter for TwSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(HOST)
    }

    fn allows_artifact_url(&self, frame: &FrameRef, url: &Url) -> bool {
        frame_plan(frame).is_some_and(|plan| {
            plan.urls().iter().any(|(_, _, expected)| *expected == url.as_str())
                && url.scheme() == "https"
                && url.host_str().is_some_and(|host| host.eq_ignore_ascii_case(HOST))
        })
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            validate_target_and_query(&target, &context.query)?;
            if target.station.as_deref().is_some_and(|station| station != STATION)
                || (!context.query.stations.is_empty()
                    && !context.query.stations.iter().any(|station| station == STATION))
            {
                return Ok(Vec::new());
            }
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source tw discovery requires network access".into(),
                ));
            }
            let product =
                target.product.as_deref().or(context.query.product.as_deref()).ok_or_else(
                    || CoreError::Transport("source tw requires a product target".into()),
                )?;
            let frame = match product {
                PRODUCT_OBSERVATION => {
                    let url = format!("{BUCKET_BASE}/{OBSERVATION_KEY}.json");
                    let payload = get_discovery_payload(&context, &url).await?;
                    let metadata = parse_observation_metadata(&payload)?;
                    validate_observation_limits(&metadata, &context)?;
                    observation_frame(&metadata)?
                }
                PRODUCT_GRID => {
                    let url = format!("{BUCKET_BASE}/{GRID_KEY}.json");
                    let (size, etag) = grid_object_metadata(&context, &url).await?;
                    let payload = context
                        .http_transport
                        .get_range_bytes(&url, 0, size.min(GRID_HEADER_BYTES) - 1, size, &etag)
                        .await
                        .map_err(|error| sanitize_request_error(error, "discovery"))?;
                    let metadata = parse_grid_header(&payload)?;
                    validate_grid_limits(&metadata, &context)?;
                    grid_frame(&metadata)?
                }
                _ => return Err(CoreError::Transport("source tw product is unsupported".into())),
            };
            Ok(vec![frame])
        })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        context: SourceContext,
        temp_root: std::path::PathBuf,
    ) -> Option<BoxFuture<'static, CoreResult<RawFrame>>> {
        Some(Box::pin(async move {
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source tw acquisition requires network access".into(),
                ));
            }
            let plan = frame_plan(&frame)
                .ok_or_else(|| CoreError::Transport("source tw frame locator is invalid".into()))?;
            let raw_artifacts = match &plan {
                FramePlan::Observation { metadata: expected } => {
                    let metadata_url = format!("{BUCKET_BASE}/{OBSERVATION_KEY}.json");
                    let metadata_artifact = download_artifact(
                        &context,
                        &temp_root,
                        &metadata_url,
                        "O-A0058-005.json",
                        "application/json",
                        0,
                    )
                    .await?;
                    let metadata_bytes = read_artifact(&metadata_artifact)?;
                    let current = parse_observation_metadata(&metadata_bytes)?;
                    validate_observation_limits(&current, &context)?;
                    if &current != expected {
                        return Err(CoreError::Transport(
                            "source tw observation metadata changed after discovery; rediscover latest".into(),
                        ));
                    }
                    let image_url = format!("{BUCKET_BASE}/{OBSERVATION_KEY}.png");
                    let image_artifact = download_artifact(
                        &context,
                        &temp_root,
                        &image_url,
                        "O-A0058-005.png",
                        "image/png",
                        metadata_artifact.receipt.size_bytes,
                    )
                    .await?;
                    validate_png_file(&image_artifact, current.shape, &context)?;
                    vec![image_artifact, metadata_artifact]
                }
                FramePlan::Grid { metadata: expected } => {
                    let grid_url = format!("{BUCKET_BASE}/{GRID_KEY}.json");
                    let artifact = download_grid_artifact(&context, &temp_root, &grid_url).await?;
                    let payload = read_artifact(&artifact)?;
                    let current = parse_grid_metadata(&payload)?;
                    validate_grid_limits(&current, &context)?;
                    if &current != expected {
                        return Err(CoreError::Transport(
                            "source tw grid time or geometry changed after discovery".into(),
                        ));
                    }
                    vec![artifact]
                }
            };
            Ok(RawFrame { frame, artifacts: raw_artifacts, private_locator: None })
        }))
    }
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport("source tw received a mismatched source query".into()));
    }
    if target
        .product
        .as_deref()
        .is_some_and(|product| product != PRODUCT_OBSERVATION && product != PRODUCT_GRID)
        || query
            .product
            .as_deref()
            .is_some_and(|product| product != PRODUCT_OBSERVATION && product != PRODUCT_GRID)
    {
        return Err(CoreError::Transport("source tw product is unsupported".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source tw does not expose base times".into()));
    }
    Ok(())
}

async fn get_discovery_payload(context: &SourceContext, url: &str) -> CoreResult<Arc<[u8]>> {
    let payload = context
        .http_transport
        .get_bytes_coalesced(url, &[], &context.request_coalescer)
        .await
        .map_err(|error| sanitize_request_error(error, "discovery"))?;
    context.limits.validate_bytes(payload.len() as u64, payload.len() as u64)?;
    Ok(payload)
}

async fn grid_object_metadata(context: &SourceContext, url: &str) -> CoreResult<(u64, String)> {
    let response = context
        .http_transport
        .head_metadata(url)
        .await
        .map_err(|error| sanitize_request_error(error, "grid metadata"))?;
    if response.status != 200 {
        return Err(CoreError::Transport("source tw grid object is unavailable".into()));
    }
    let size = response
        .headers
        .get("content-length")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_metadata("grid object"))?;
    let etag = response
        .headers
        .get("etag")
        .filter(|value| value.starts_with('"') && value.ends_with('"'))
        .ok_or_else(|| invalid_metadata("grid object"))?;
    context.limits.validate_bytes(size, size)?;
    if size > context.limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "source tw grid exceeds temporary storage limit".into(),
        ));
    }
    Ok((size, etag.clone()))
}

/// CWA places the complete metadata before the large numeric content string.
/// Close that string and its enclosing objects to validate the header as JSON;
/// the acquired full document is independently checked before decode.
fn parse_grid_header(payload: &[u8]) -> CoreResult<GridMetadata> {
    if let Ok(metadata) = parse_grid_metadata(payload) {
        return Ok(metadata);
    }
    let text = std::str::from_utf8(payload).map_err(|_| invalid_metadata("grid header"))?;
    let (header, content) =
        text.split_once("\"content\"").ok_or_else(|| invalid_metadata("grid header"))?;
    if !content
        .trim_start()
        .strip_prefix(':')
        .is_some_and(|value| value.trim_start().starts_with('"'))
    {
        return Err(invalid_metadata("grid header"));
    }
    let document = [header, "\"content\":\"\"}}}}"].concat();
    parse_grid_metadata(document.as_bytes())
}

async fn download_grid_artifact(
    context: &SourceContext,
    temp_root: &Path,
    url: &str,
) -> CoreResult<RawArtifact> {
    let (size, etag) = grid_object_metadata(context, url).await?;
    std::fs::create_dir_all(temp_root).map_err(|_| {
        CoreError::Temporary("source tw temporary directory could not be prepared".into())
    })?;
    let staging = tempfile::NamedTempFile::new_in(temp_root).map_err(|_| {
        CoreError::Temporary("source tw grid staging file could not be created".into())
    })?;
    let mut file = tokio::fs::File::from_std(staging.reopen().map_err(|_| {
        CoreError::Temporary("source tw grid staging file could not be opened".into())
    })?);
    let concurrency =
        context.limits.host_concurrency.min(context.limits.request_concurrency).clamp(1, 4);
    let mut chunks = stream::iter((0..size).step_by(GRID_CHUNK_BYTES as usize))
        .map(|start| {
            let etag = &etag;
            async move {
                context
                    .http_transport
                    .get_range_bytes(
                        url,
                        start,
                        (start + GRID_CHUNK_BYTES - 1).min(size - 1),
                        size,
                        etag,
                    )
                    .await
                    .map_err(|error| sanitize_request_error(error, "grid acquisition"))
            }
        })
        .buffered(concurrency);
    let mut hasher = Sha256::new();
    let mut written = 0_u64;
    while let Some(bytes) = chunks.try_next().await? {
        use tokio::io::AsyncWriteExt;
        file.write_all(&bytes)
            .await
            .map_err(|_| CoreError::Temporary("source tw grid staging write failed".into()))?;
        hasher.update(&bytes);
        written += bytes.len() as u64;
    }
    if written != size {
        return Err(CoreError::Transport("source tw grid download is incomplete".into()));
    }
    file.sync_all()
        .await
        .map_err(|_| CoreError::Temporary("source tw grid staging sync failed".into()))?;
    drop(file);
    Ok(RawArtifact {
        receipt: ArtifactReceipt {
            name: "O-A0059-001.json".into(),
            media_type: "application/json".into(),
            size_bytes: written,
            sha256: hex::encode(hasher.finalize()),
        },
        path: staging.into_temp_path(),
    })
}

async fn download_artifact(
    context: &SourceContext,
    temp_root: &Path,
    url: &str,
    name: &str,
    media_type: &str,
    bytes_already_staged: u64,
) -> CoreResult<RawArtifact> {
    let remaining = context
        .limits
        .max_artifact_bytes
        .min(context.limits.max_frame_bytes.saturating_sub(bytes_already_staged))
        .min(context.limits.max_temp_bytes.saturating_sub(bytes_already_staged));
    if remaining == 0 {
        return Err(CoreError::ResourceLimit(
            "source tw artifacts exceed configured frame limits".into(),
        ));
    }
    std::fs::create_dir_all(temp_root).map_err(|_| {
        CoreError::Temporary("source tw temporary directory could not be prepared".into())
    })?;
    let destination = temp_root.join(format!("tw-{}.part", uuid::Uuid::new_v4()));
    let receipt = context
        .http_transport
        .get_to_path_same_origin_limited_with_headers(url, &[], &destination, remaining)
        .await
        .map_err(|error| sanitize_request_error(error, "acquisition"))?;
    let total = bytes_already_staged
        .checked_add(receipt.size_bytes)
        .ok_or_else(|| CoreError::ResourceLimit("source tw frame byte count overflow".into()))?;
    context.limits.validate_bytes(receipt.size_bytes, total)?;
    if total > context.limits.max_temp_bytes {
        let _ = std::fs::remove_file(&destination);
        return Err(CoreError::ResourceLimit(
            "source tw artifacts exceed temporary storage limit".into(),
        ));
    }
    let path = tempfile::TempPath::try_from_path(destination).map_err(|_| {
        CoreError::Temporary("source tw temporary artifact could not be retained".into())
    })?;
    Ok(RawArtifact {
        receipt: ArtifactReceipt {
            name: name.into(),
            media_type: media_type.into(),
            size_bytes: receipt.size_bytes,
            sha256: receipt.sha256,
        },
        path,
    })
}

fn read_artifact(artifact: &RawArtifact) -> CoreResult<Vec<u8>> {
    std::fs::read(&artifact.path)
        .map_err(|_| CoreError::Temporary("source tw staged artifact could not be read".into()))
}

fn validate_png_file(
    artifact: &RawArtifact,
    expected: [u32; 2],
    context: &SourceContext,
) -> CoreResult<()> {
    let mut file = std::fs::File::open(&artifact.path)
        .map_err(|_| CoreError::Temporary("source tw staged image could not be opened".into()))?;
    let mut header = [0_u8; 24];
    file.read_exact(&mut header)
        .map_err(|_| CoreError::Transport("source tw observation image is truncated".into()))?;
    validate_png_dimensions(&header, expected, context)
}

fn sanitize_request_error(error: CoreError, stage: &str) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled(format!("source tw {stage} requires network access"))
        }
        CoreError::ResourceLimit(_) => CoreError::ResourceLimit(format!(
            "source tw {stage} response exceeds configured limits"
        )),
        _ => CoreError::Transport(format!("source tw {stage} request failed")),
    }
}

fn validate_observation_limits(
    metadata: &ObservationMetadata,
    context: &SourceContext,
) -> CoreResult<()> {
    let pixels = u64::from(metadata.shape[0])
        .checked_mul(u64::from(metadata.shape[1]))
        .ok_or_else(|| CoreError::ResourceLimit("source tw image dimensions overflow".into()))?;
    context.limits.validate_pixels(pixels)
}

fn validate_grid_limits(metadata: &GridMetadata, context: &SourceContext) -> CoreResult<()> {
    let cells = u64::from(metadata.width)
        .checked_mul(u64::from(metadata.height))
        .and_then(|count| count.checked_mul(2))
        .ok_or_else(|| CoreError::ResourceLimit("source tw grid dimensions overflow".into()))?;
    context.limits.validate_pixels(cells)
}

fn parse_observation_metadata(payload: &[u8]) -> CoreResult<ObservationMetadata> {
    let document = parse_json(payload, "observation")?;
    let provider = document.get("cwaopendata").ok_or_else(|| invalid_metadata("observation"))?;
    if provider.get("dataid").and_then(Value::as_str) != Some(OBSERVATION_KEY) {
        return Err(invalid_metadata("observation"));
    }
    let dataset = provider.get("dataset").ok_or_else(|| invalid_metadata("observation"))?;
    let resource = dataset.get("resource").ok_or_else(|| invalid_metadata("observation"))?;
    let expected_url = format!("{BUCKET_BASE}/{OBSERVATION_KEY}.png");
    if resource.get("ProductURL").and_then(Value::as_str) != Some(expected_url.as_str())
        || resource.get("mimeType").and_then(Value::as_str) != Some("image/png")
    {
        return Err(invalid_metadata("observation"));
    }
    let parameters = dataset
        .get("datasetInfo")
        .and_then(|value| value.get("parameterSet"))
        .ok_or_else(|| invalid_metadata("observation"))?;
    let (west, east) = parse_range(parameters.get("LongitudeRange").and_then(Value::as_str))
        .ok_or_else(|| invalid_metadata("observation"))?;
    let (south, north) = parse_range(parameters.get("LatitudeRange").and_then(Value::as_str))
        .ok_or_else(|| invalid_metadata("observation"))?;
    if !(-180.0..=180.0).contains(&west)
        || !(-180.0..=180.0).contains(&east)
        || !(-90.0..=90.0).contains(&south)
        || !(-90.0..=90.0).contains(&north)
        || west >= east
        || south >= north
    {
        return Err(invalid_metadata("observation"));
    }
    let dimension = parameters
        .get("ImageDimension")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_metadata("observation"))?;
    let (width, height) =
        parse_dimensions(dimension).ok_or_else(|| invalid_metadata("observation"))?;
    let valid_time = provider_time(
        dataset
            .get("DateTime")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_metadata("observation"))?,
    )
    .ok_or_else(|| invalid_metadata("observation"))?;
    Ok(ObservationMetadata {
        valid_time,
        extent: [west, south, east, north],
        shape: [height, width],
    })
}

fn parse_grid_metadata(payload: &[u8]) -> CoreResult<GridMetadata> {
    let document = parse_json(payload, "grid")?;
    parse_grid_metadata_value(&document)
}

fn parse_grid_metadata_value(document: &Value) -> CoreResult<GridMetadata> {
    let provider = document.get("cwaopendata").ok_or_else(|| invalid_metadata("grid"))?;
    if provider.get("dataid").and_then(Value::as_str) != Some(GRID_KEY) {
        return Err(invalid_metadata("grid"));
    }
    let dataset = provider.get("dataset").ok_or_else(|| invalid_metadata("grid"))?;
    let parameters = dataset
        .get("datasetInfo")
        .and_then(|value| value.get("parameterSet"))
        .ok_or_else(|| invalid_metadata("grid"))?;
    let contents = dataset.get("contents").ok_or_else(|| invalid_metadata("grid"))?;
    if parameters.get("Reflectivity").and_then(Value::as_str) != Some("dBZ")
        || !contents
            .get("contentDescription")
            .and_then(Value::as_str)
            .is_some_and(|description| description.contains("TWD67"))
        || contents.get("content").and_then(Value::as_str).is_none()
    {
        return Err(invalid_metadata("grid"));
    }
    let width = parameters
        .get("GridDimensionX")
        .and_then(value_u32)
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_metadata("grid"))?;
    let height = parameters
        .get("GridDimensionY")
        .and_then(value_u32)
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_metadata("grid"))?;
    let pixels =
        u64::from(width).checked_mul(u64::from(height)).ok_or_else(|| invalid_metadata("grid"))?;
    if pixels > 500_000_000 {
        return Err(CoreError::ResourceLimit(
            "source tw grid dimensions exceed safety limit".into(),
        ));
    }
    let west =
        parameter_f64(parameters, "StartPointLongitude").ok_or_else(|| invalid_metadata("grid"))?;
    let south =
        parameter_f64(parameters, "StartPointLatitude").ok_or_else(|| invalid_metadata("grid"))?;
    let step =
        parameter_f64(parameters, "GridResolution").ok_or_else(|| invalid_metadata("grid"))?;
    if !west.is_finite() || !south.is_finite() || !step.is_finite() || step <= 0.0 {
        return Err(invalid_metadata("grid"));
    }
    let valid_time = provider_time(
        parameters
            .get("DateTime")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_metadata("grid"))?,
    )
    .ok_or_else(|| invalid_metadata("grid"))?;
    Ok(GridMetadata { valid_time, width, height, west, south, step })
}

pub(crate) fn validate_science_grid(
    frame: &FrameRef,
    document: &Value,
) -> CoreResult<GridMetadata> {
    if frame.source == SOURCE
        && frame.product == PRODUCT_GRID
        && frame.locator_version == "tw-legacy-v1"
    {
        let expected_time = parse_utc_time(&frame.valid_time)
            .map_err(|_| CoreError::Transport("source tw legacy frame time is invalid".into()))?;
        let revision = format!("{GRID_KEY}-{}", expected_time.timestamp());
        if frame.station.as_deref() != Some(STATION)
            || frame.base_time.is_some()
            || frame.locator.get("station").and_then(Value::as_str) != Some(STATION)
            || frame.revision.as_deref() != Some(revision.as_str())
            || frame.locator.get("revision").and_then(Value::as_str) != Some(revision.as_str())
            || logical_id(frame).ok().as_deref() != Some(frame.logical_id.as_str())
        {
            return Err(CoreError::Transport("source tw legacy grid frame is invalid".into()));
        }
        let current = parse_grid_metadata_value(document)?;
        if current.valid_time != expected_time {
            return Err(CoreError::Transport(
                "source tw legacy grid time changed after discovery".into(),
            ));
        }
        return Ok(current);
    }

    let Some(FramePlan::Grid { metadata: expected }) = frame_plan(frame) else {
        return Err(CoreError::Transport("source tw grid frame locator is invalid".into()));
    };
    let current = parse_grid_metadata_value(document)?;
    if current != expected {
        return Err(CoreError::Transport(
            "source tw grid time or geometry changed after discovery".into(),
        ));
    }
    Ok(current)
}

fn parse_json(payload: &[u8], product: &str) -> CoreResult<Value> {
    serde_json::from_slice(payload)
        .map_err(|_| CoreError::Transport(format!("source tw {product} metadata is invalid JSON")))
}

fn invalid_metadata(product: &str) -> CoreError {
    CoreError::Transport(format!("source tw {product} metadata is invalid or unsupported"))
}

fn parse_range(value: Option<&str>) -> Option<(f64, f64)> {
    let value = value?;
    for (index, byte) in value.as_bytes().iter().enumerate() {
        if *byte != b'-' || index == 0 {
            continue;
        }
        let left = value[..index].trim().parse::<f64>().ok()?;
        let right = value[index + 1..].trim().parse::<f64>().ok()?;
        if left.is_finite() && right.is_finite() {
            return Some((left, right));
        }
    }
    None
}

fn parse_dimensions(value: &str) -> Option<(u32, u32)> {
    let value = value.to_ascii_lowercase();
    let (width, height) = value.split_once('x')?;
    let width = width.trim().parse::<u32>().ok()?;
    let height = height.trim().parse::<u32>().ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

fn provider_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value).ok().map(|time| time.with_timezone(&Utc))
}

fn value_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .or_else(|| value.as_str()?.parse::<u32>().ok())
}

fn parameter_f64(parameters: &Value, key: &str) -> Option<f64> {
    let value = parameters.get(key)?;
    value.as_f64().or_else(|| value.as_str()?.parse::<f64>().ok())
}

fn observation_frame(metadata: &ObservationMetadata) -> CoreResult<FrameRef> {
    let revision = format!("{OBSERVATION_KEY}-{}", metadata.valid_time.timestamp());
    let image_url = format!("{BUCKET_BASE}/{OBSERVATION_KEY}.png");
    let metadata_url = format!("{BUCKET_BASE}/{OBSERVATION_KEY}.json");
    build_frame(
        PRODUCT_OBSERVATION,
        &revision,
        metadata.valid_time,
        json!({
            "url": image_url,
            "name": "O-A0058-005.png",
            "media_type": "image/png",
            "artifacts": [{
                "url": metadata_url,
                "name": "O-A0058-005.json",
                "role": "metadata",
                "media_type": "application/json",
            }],
            "station": STATION,
            "revision": revision,
            "time_semantics": "provider_metadata_time",
            "geometry_status": "unverified",
            "provider_declared_extent": metadata.extent,
            "provider_image_shape": metadata.shape,
            "object_transport": "anonymous_s3_https",
        }),
    )
}

fn grid_frame(metadata: &GridMetadata) -> CoreResult<FrameRef> {
    let revision = format!("{GRID_KEY}-{}", metadata.valid_time.timestamp());
    let url = format!("{BUCKET_BASE}/{GRID_KEY}.json");
    build_frame(
        PRODUCT_GRID,
        &revision,
        metadata.valid_time,
        json!({
            "url": url,
            "name": "O-A0059-001.json",
            "media_type": "application/json",
            "artifacts": [],
            "station": STATION,
            "revision": revision,
            "time_semantics": "provider_grid_time",
            "geometry_status": "provider_native_twd67",
            "native_crs": "EPSG:3821",
            "grid_dimension": [metadata.height, metadata.width],
            "grid_origin": [metadata.west, metadata.south],
            "grid_resolution": metadata.step,
        }),
    )
}

fn build_frame(
    product: &str,
    revision: &str,
    valid_time: DateTime<Utc>,
    locator: Value,
) -> CoreResult<FrameRef> {
    let mut frame = FrameRef {
        source: SOURCE.into(),
        product: product.into(),
        station: Some(STATION.into()),
        valid_time: valid_time.to_rfc3339_opts(SecondsFormat::Micros, true),
        base_time: None,
        logical_id: String::new(),
        revision: Some(revision.into()),
        locator_version: LOCATOR_VERSION.into(),
        locator,
    };
    frame.logical_id = logical_id(&frame)
        .map_err(|_| CoreError::Transport("source tw frame identity is invalid".into()))?;
    Ok(frame)
}

fn frame_plan(frame: &FrameRef) -> Option<FramePlan> {
    if frame.source != SOURCE
        || frame.station.as_deref() != Some(STATION)
        || frame.base_time.is_some()
        || frame.locator_version != LOCATOR_VERSION
        || logical_id(frame).ok().as_deref() != Some(frame.logical_id.as_str())
        || frame.locator.get("station").and_then(Value::as_str) != Some(STATION)
    {
        return None;
    }
    let expected_time = parse_utc_time(&frame.valid_time).ok()?;
    let revision = frame.revision.as_deref()?;
    let declared_revision = frame.locator.get("revision").and_then(Value::as_str)?;
    if revision != declared_revision {
        return None;
    }
    match frame.product.as_str() {
        PRODUCT_OBSERVATION => {
            let extent = frame.locator.get("provider_declared_extent")?.as_array()?;
            let extent: [f64; 4] =
                extent.iter().map(Value::as_f64).collect::<Option<Vec<_>>>()?.try_into().ok()?;
            let shape = frame.locator.get("provider_image_shape")?.as_array()?;
            let shape: [u32; 2] =
                shape.iter().map(value_u32).collect::<Option<Vec<_>>>()?.try_into().ok()?;
            let metadata = ObservationMetadata { valid_time: expected_time, extent, shape };
            let (west, south, east, north) = (extent[0], extent[1], extent[2], extent[3]);
            if frame.locator.get("time_semantics").and_then(Value::as_str)
                != Some("provider_metadata_time")
                || frame.locator.get("geometry_status").and_then(Value::as_str)
                    != Some("unverified")
                || frame.locator.get("object_transport").and_then(Value::as_str)
                    != Some("anonymous_s3_https")
                || frame.locator.get("url").and_then(Value::as_str)
                    != Some(format!("{BUCKET_BASE}/{OBSERVATION_KEY}.png").as_str())
                || frame.locator.get("name").and_then(Value::as_str) != Some("O-A0058-005.png")
                || frame.locator.get("media_type").and_then(Value::as_str) != Some("image/png")
                || revision != format!("{OBSERVATION_KEY}-{}", expected_time.timestamp())
                || ![west, south, east, north].iter().all(|value| value.is_finite())
                || shape.contains(&0)
            {
                return None;
            }
            let artifacts = frame.locator.get("artifacts")?.as_array()?;
            if artifacts.len() != 1
                || artifacts[0].get("url").and_then(Value::as_str)
                    != Some(format!("{BUCKET_BASE}/{OBSERVATION_KEY}.json").as_str())
                || artifacts[0].get("name").and_then(Value::as_str) != Some("O-A0058-005.json")
                || artifacts[0].get("role").and_then(Value::as_str) != Some("metadata")
                || artifacts[0].get("media_type").and_then(Value::as_str)
                    != Some("application/json")
            {
                return None;
            }
            Some(FramePlan::Observation { metadata })
        }
        PRODUCT_GRID => {
            let metadata = GridMetadata {
                valid_time: expected_time,
                width: value_u32(frame.locator.get("grid_dimension")?.get(1)?)?,
                height: value_u32(frame.locator.get("grid_dimension")?.get(0)?)?,
                west: frame.locator.get("grid_origin")?.get(0)?.as_f64()?,
                south: frame.locator.get("grid_origin")?.get(1)?.as_f64()?,
                step: frame.locator.get("grid_resolution")?.as_f64()?,
            };
            if frame.locator.get("time_semantics").and_then(Value::as_str)
                != Some("provider_grid_time")
                || frame.locator.get("geometry_status").and_then(Value::as_str)
                    != Some("provider_native_twd67")
                || frame.locator.get("native_crs").and_then(Value::as_str) != Some("EPSG:3821")
                || frame.locator.get("url").and_then(Value::as_str)
                    != Some(format!("{BUCKET_BASE}/{GRID_KEY}.json").as_str())
                || frame.locator.get("name").and_then(Value::as_str) != Some("O-A0059-001.json")
                || frame.locator.get("media_type").and_then(Value::as_str)
                    != Some("application/json")
                || !frame.locator.get("artifacts")?.as_array()?.is_empty()
                || revision != format!("{GRID_KEY}-{}", expected_time.timestamp())
                || metadata.width == 0
                || metadata.height == 0
                || !metadata.west.is_finite()
                || !metadata.south.is_finite()
                || !metadata.step.is_finite()
                || metadata.step <= 0.0
            {
                return None;
            }
            Some(FramePlan::Grid { metadata })
        }
        _ => None,
    }
}

fn validate_png_dimensions(
    payload: &[u8],
    expected: [u32; 2],
    context: &SourceContext,
) -> CoreResult<()> {
    if payload.len() < 24
        || payload.get(..8) != Some(b"\x89PNG\r\n\x1a\n")
        || payload.get(12..16) != Some(b"IHDR")
    {
        return Err(CoreError::Transport(
            "source tw observation image is not a valid PNG header".into(),
        ));
    }
    let width = u32::from_be_bytes(payload[16..20].try_into().expect("four bytes"));
    let height = u32::from_be_bytes(payload[20..24].try_into().expect("four bytes"));
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| CoreError::ResourceLimit("source tw image dimensions overflow".into()))?;
    context.limits.validate_pixels(pixels)?;
    if [height, width] != expected {
        return Err(CoreError::Transport(
            "source tw observation image dimensions disagree with provider metadata".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;
    use sha2::{Digest, Sha256};

    const OBSERVATION_JSON: &[u8] =
        include_bytes!("../../../../tests/fixtures/sources/tw/raw/O-A0058-005.json");
    const OBSERVATION_PNG: &[u8] =
        include_bytes!("../../../../tests/fixtures/sources/tw/raw/O-A0058-005.png");
    const GRID_JSON: &[u8] =
        include_bytes!("../../../../tests/fixtures/sources/tw/raw/O-A0059-001.json");

    fn numeric_grid(time: &str, content: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "cwaopendata": {
                "dataid": GRID_KEY,
                "dataset": {
                    "datasetInfo": {"parameterSet": {
                        "StartPointLongitude": "115.0",
                        "StartPointLatitude": "18.0",
                        "GridResolution": "0.0125",
                        "GridDimensionX": "3",
                        "GridDimensionY": "2",
                        "DateTime": time,
                        "Reflectivity": "dBZ",
                    }},
                    "contents": {
                        "contentDescription": "southwest first, TWD67",
                        "content": content,
                    },
                },
            },
        }))
        .unwrap()
    }

    fn raw_grid(payload: &[u8]) -> RawFrame {
        let metadata = parse_grid_metadata(payload).unwrap();
        let frame = grid_frame(&metadata).unwrap();
        let staging = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(staging.path(), payload).unwrap();
        RawFrame {
            frame,
            artifacts: vec![RawArtifact {
                receipt: ArtifactReceipt {
                    name: "O-A0059-001.json".into(),
                    media_type: "application/json".into(),
                    size_bytes: payload.len() as u64,
                    sha256: hex::encode(Sha256::digest(payload)),
                },
                path: staging.into_temp_path(),
            }],
            private_locator: None,
        }
    }

    #[test]
    fn official_observation_metadata_and_png_dimensions_are_bound_together() {
        let metadata = parse_observation_metadata(OBSERVATION_JSON).unwrap();
        assert_eq!(metadata.valid_time.to_rfc3339(), "2026-09-18T03:40:00+00:00");
        assert_eq!(metadata.extent, [115.0, 17.75, 126.5, 29.25]);
        assert_eq!(metadata.shape, [3600, 3600]);
        assert_eq!(
            hex::encode(Sha256::digest(OBSERVATION_PNG)),
            "abfb1f72fa191d154e942b8be3bdab61610ae70f07cc137d41f557b4597be293"
        );
        let context = test_context();
        validate_png_dimensions(OBSERVATION_PNG, metadata.shape, &context).unwrap();
        let frame = observation_frame(&metadata).unwrap();
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());
        assert_eq!(
            frame.logical_id,
            crate::identity::digest(&frame_identity(&frame).unwrap()).unwrap()
        );
        assert_eq!(
            frame_plan(&frame).unwrap().urls().iter().map(|item| item.0).collect::<Vec<_>>(),
            ["data", "metadata"]
        );
        let adapter = TwSourceAdapter;
        assert!(adapter.allows_artifact_url(
            &frame,
            &Url::parse(&format!("{BUCKET_BASE}/{OBSERVATION_KEY}.png")).unwrap()
        ));
        assert!(adapter.allows_artifact_url(
            &frame,
            &Url::parse(&format!("{BUCKET_BASE}/{OBSERVATION_KEY}.json")).unwrap()
        ));
        assert!(!adapter.allows_artifact_url(
            &frame,
            &Url::parse(&format!("{BUCKET_BASE}/other.png")).unwrap()
        ));
    }

    #[test]
    fn numeric_product_keeps_provider_time_native_crs_and_bounded_grid_metadata() {
        let payload = numeric_grid("2026-09-20T12:30:00+08:00", "1,-99,3,4,-999,6");
        let metadata = parse_grid_metadata(&payload).unwrap();
        assert_eq!(metadata.valid_time.to_rfc3339(), "2026-09-20T04:30:00+00:00");
        assert_eq!((metadata.width, metadata.height), (3, 2));
        assert_eq!((metadata.west, metadata.south, metadata.step), (115.0, 18.0, 0.0125));
        let frame = grid_frame(&metadata).unwrap();
        assert_eq!(frame.locator["native_crs"], "EPSG:3821");
        assert_eq!(frame.locator["grid_dimension"], json!([2, 3]));
        assert!(TwSourceAdapter.allows_artifact_url(
            &frame,
            &Url::parse(&format!("{BUCKET_BASE}/{GRID_KEY}.json")).unwrap()
        ));
    }

    #[test]
    fn official_numeric_grid_fixture_retains_provider_time_geometry_and_bytes() {
        assert_eq!(
            hex::encode(Sha256::digest(GRID_JSON)),
            "96f97544d0e2f3795b69cb2cbfe3f906a03bb4232d1eb62f15d5740445c56c90"
        );
        let metadata = parse_grid_metadata(GRID_JSON).unwrap();
        assert_eq!(metadata.valid_time.to_rfc3339(), "2026-09-20T04:30:00+00:00");
        assert_eq!((metadata.width, metadata.height), (921, 881));
        assert_eq!((metadata.west, metadata.south, metadata.step), (115.0, 18.0, 0.0125));
        let frame = grid_frame(&metadata).unwrap();
        assert_eq!(frame.revision.as_deref(), Some("O-A0059-001-1789878600"));
        assert!(TwSourceAdapter.allows_artifact_url(
            &frame,
            &Url::parse(&format!("{BUCKET_BASE}/{GRID_KEY}.json")).unwrap()
        ));
    }

    #[test]
    fn numeric_grid_discovery_reads_metadata_from_a_bounded_prefix() {
        let expected = parse_grid_metadata(GRID_JSON).unwrap();
        assert_eq!(parse_grid_header(&GRID_JSON[..GRID_HEADER_BYTES as usize]).unwrap(), expected);
        let payload = numeric_grid("2026-09-20T12:30:00+08:00", "1,2,3,4,5,6");
        assert_eq!(parse_grid_header(&payload).unwrap(), parse_grid_metadata(&payload).unwrap());
        assert!(parse_grid_header(&GRID_JSON[..100]).is_err());
        assert!(parse_grid_header(b"<html>not a grid</html>").is_err());
        let header = String::from_utf8(GRID_JSON[..GRID_HEADER_BYTES as usize].to_vec()).unwrap();
        assert!(parse_grid_header(header.replace("TWD67", "WGS84").as_bytes()).is_err());
        assert!(
            parse_grid_header(header.replace("O-A0059-001", "O-A0058-005").as_bytes()).is_err()
        );
    }

    #[tokio::test]
    async fn official_numeric_grid_decoder_matches_python_arrays_and_quality() {
        let raw = Arc::new(raw_grid(GRID_JSON));
        let engine = crate::engine::Engine::new(
            crate::config::CoreConfig::default(),
            crate::source::SourceRegistry::default(),
        )
        .unwrap();
        let field = engine.decode_science(raw).await.unwrap();

        assert_eq!(field.name, "reflectivity");
        assert_eq!(field.shape, [881, 921]);
        assert_eq!(field.grid.crs.as_deref(), Some("EPSG:3821"));
        assert_eq!(field.units.as_deref(), Some("dBZ"));
        assert_eq!(field.valid_time, "2026-09-20T04:30:00.000000Z");
        assert_eq!(field.grid.x[0], 115.0);
        assert_eq!(field.grid.x[920], 126.5);
        assert_eq!(field.grid.y[0], 18.0);
        assert_eq!(field.grid.y[880], 29.0);
        assert!(field.values[0].is_nan());
        assert_eq!(field.quality[0], 2);
        assert!(field.values[357].is_nan());
        assert_eq!(field.quality[357], 1);
        let valid_sample = 262 * 921 + 580;
        assert_eq!(field.values[valid_sample], 18.0);
        assert_eq!(field.quality[valid_sample], 0);
        assert_eq!(field.quality.iter().filter(|&&value| value == 0).count(), 1_056);
        assert_eq!(field.quality.iter().filter(|&&value| value == 1).count(), 610_042);
        assert_eq!(field.quality.iter().filter(|&&value| value == 2).count(), 200_303);

        let values_bytes =
            field.values.iter().flat_map(|value| value.to_le_bytes()).collect::<Vec<_>>();
        let quality_bytes =
            field.quality.iter().flat_map(|value| value.to_le_bytes()).collect::<Vec<_>>();
        assert_eq!(
            hex::encode(Sha256::digest(values_bytes)),
            "91953e30495cc60eae963d3858f36245b3ac25ecf161e8be0f87ac9a05bef2cc"
        );
        assert_eq!(
            hex::encode(Sha256::digest(quality_bytes)),
            "92bd2361bf3d449bd09e9380fe521947c5a4f4a0d6e28bb5f8ee33a36afcc058"
        );
        assert!(field.provenance.iter().any(|entry| entry == "decoder=cwa-O-A0059-001-v1"));
        field.validate().unwrap();
    }

    #[test]
    fn numeric_grid_decoder_preserves_missing_codes_order_and_resource_guards() {
        let raw = raw_grid(&numeric_grid(
            "2026-09-20T12:30:00+08:00",
            "1.0,-9.900E+01,3.0,-9.990E+02,5.0,6.0",
        ));
        let field =
            crate::science::decode_tw_grid(&raw, &crate::limits::Limits::default()).unwrap();
        assert_eq!(field.shape, [2, 3]);
        assert_eq!(field.grid.crs.as_deref(), Some("EPSG:3821"));
        assert_eq!(field.grid.x, [115.0, 115.0125, 115.025]);
        assert_eq!(field.grid.y, [18.0, 18.0125]);
        assert_eq!(field.values[0], 1.0);
        assert!(field.values[1].is_nan());
        assert_eq!(field.values[2], 3.0);
        assert!(field.values[3].is_nan());
        assert_eq!(field.values[4..], [5.0, 6.0]);
        assert_eq!(field.quality, [0, 1, 0, 2, 0, 0]);

        let small_limits =
            crate::limits::Limits { max_pixels: 10, ..crate::limits::Limits::default() };
        assert!(matches!(
            crate::science::decode_tw_grid(&raw, &small_limits),
            Err(CoreError::ResourceLimit(_))
        ));
    }

    #[test]
    fn numeric_grid_decoder_rejects_incomplete_nonfinite_and_changed_artifacts() {
        for content in ["1,2,3,4,5", "1,2,3,4,5,nan", "1,2,3,4,5,6,7"] {
            let raw = raw_grid(&numeric_grid("2026-09-20T12:30:00+08:00", content));
            assert!(
                crate::science::decode_tw_grid(&raw, &crate::limits::Limits::default()).is_err()
            );
        }

        let mut raw = raw_grid(GRID_JSON);
        raw.artifacts[0].receipt.sha256 = "00".repeat(32);
        assert!(matches!(
            crate::science::decode_tw_grid(&raw, &crate::limits::Limits::default()),
            Err(CoreError::Transport(_))
        ));

        let mut raw = raw_grid(GRID_JSON);
        raw.frame.locator["grid_resolution"] = json!(0.025);
        assert!(matches!(
            crate::science::decode_tw_grid(&raw, &crate::limits::Limits::default()),
            Err(CoreError::Transport(_))
        ));
    }

    #[test]
    fn metadata_rejects_changed_product_identity_time_geometry_and_png_dimensions() {
        let mut wrong_product: Value = serde_json::from_slice(OBSERVATION_JSON).unwrap();
        wrong_product["cwaopendata"]["dataid"] = json!("O-A0058-001");
        assert!(parse_observation_metadata(&serde_json::to_vec(&wrong_product).unwrap()).is_err());
        assert!(parse_grid_metadata(&numeric_grid("not-a-time", "1,2,3,4,5,6")).is_err());
        assert!(validate_png_dimensions(OBSERVATION_PNG, [3, 2], &test_context()).is_err());
        assert!(parse_range(Some("115-126-130")).is_none());
        assert!(parse_dimensions("0x3600").is_none());
    }

    #[test]
    fn observation_and_grid_metadata_obey_the_shared_pixel_budget() {
        let observation = parse_observation_metadata(OBSERVATION_JSON).unwrap();
        let grid =
            parse_grid_metadata(&numeric_grid("2026-09-20T12:30:00+08:00", "1,2,3,4,5,6")).unwrap();
        let mut context = test_context();
        context.limits.max_pixels = 10;
        assert!(validate_observation_limits(&observation, &context).is_err());
        assert!(validate_grid_limits(&grid, &context).is_err());
    }

    #[test]
    fn changed_locator_cannot_reuse_a_valid_tw_logical_identity() {
        let metadata = parse_observation_metadata(OBSERVATION_JSON).unwrap();
        let mut frame = observation_frame(&metadata).unwrap();
        frame.locator["artifacts"][0]["url"] = json!("https://evil.invalid/metadata.json");
        assert!(frame_plan(&frame).is_none());
    }

    fn test_context() -> SourceContext {
        let limits = crate::limits::Limits::default();
        let budget = Arc::new(crate::limits::RequestBudget::new(&limits));
        SourceContext {
            query: Query::default(),
            allow_network: false,
            discovery_workers: 4,
            source_options: Arc::new(Default::default()),
            request_budget: budget.clone(),
            http_transport: Arc::new(
                crate::transport::http::HttpTransport::with_budget(limits.clone(), false, budget)
                    .unwrap(),
            ),
            ftp_transport: Arc::new(crate::transport::ftp::FtpTransport::new(
                limits.clone(),
                false,
            )),
            request_coalescer: Arc::new(crate::transport::http::HttpRequestCoalescer::default()),
            limits,
        }
    }
}
