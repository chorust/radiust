//! Native HTTP discovery and raw acquisition for Windy's current radar tiles.
//! The optional browser path uses isolated Chromium/CDP and preserves the
//! provider's original PNG response bytes.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::limits::Limits;
use crate::model::{
    ArtifactReceipt, DiscoveryTarget, FrameRef, Query, RawArtifact, RawFrame, TimeSelector,
    parse_utc_time,
};
use crate::source::browser::{ChromiumConfig, ChromiumSession, persist_response_body};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "windy";
const PRODUCT: &str = "reflectivity";
const HOST: &str = "rdr.windy.com";
const LOCATOR_VERSION: &str = "windy-legacy-v1";
const CADENCE_SECONDS: i64 = 300;
const ZOOM: u32 = 1;
const TILE_SIZE: u32 = 256;

pub struct WindySourceAdapter;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Tile {
    name: String,
    url: String,
}

impl SourceAdapter for WindySourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(HOST)
    }

    fn allows_artifact_url(&self, frame: &FrameRef, url: &Url) -> bool {
        let Some(tiles) = frame_plan(frame) else {
            return false;
        };
        tiles.iter().any(|tile| {
            tile.url == url.as_str()
                && url.scheme() == "https"
                && url.host_str().is_some_and(|host| self.allows_artifact_host(host))
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
        })
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            let selected = validate_target_and_query(&target, &context.query)?;
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source windy discovery requires network access".into(),
                ));
            }
            if context.request_budget.cancellation.is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            if !selected {
                return Ok(Vec::new());
            }

            let now = Utc::now();
            let frame_time = floor_time(now)?;
            if context.query.max_age_secs.is_some_and(|max_age| {
                let age = (now - frame_time).num_microseconds().unwrap_or(0) as f64 / 1_000_000.0;
                age >= 0.0 && age > max_age
            }) {
                return Ok(Vec::new());
            }
            Ok(vec![frame_from_time(frame_time)?])
        })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        context: SourceContext,
        temp_root: PathBuf,
    ) -> Option<BoxFuture<'static, CoreResult<RawFrame>>> {
        Some(Box::pin(async move {
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source windy acquisition requires network access".into(),
                ));
            }
            if context.request_budget.cancellation.is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            let tiles = frame_plan(&frame).ok_or_else(invalid_frame)?;
            if use_playwright(&context)? {
                return fetch_browser_tiles(frame, context, temp_root, tiles).await;
            }

            let mut artifacts = Vec::with_capacity(tiles.len());
            let mut total_bytes = 0_u64;
            for (index, tile) in tiles.into_iter().enumerate() {
                if context.request_budget.cancellation.is_cancelled() {
                    return Err(CoreError::Cancelled);
                }
                let url = Url::parse(&tile.url).map_err(|_| invalid_frame())?;
                if !self.allows_artifact_url(&frame, &url) {
                    return Err(invalid_frame());
                }

                let remaining_frame = context.limits.max_frame_bytes.saturating_sub(total_bytes);
                let remaining_temp = context.limits.max_temp_bytes.saturating_sub(total_bytes);
                let response_limit =
                    context.limits.max_artifact_bytes.min(remaining_frame).min(remaining_temp);
                let destination = temp_root.join(format!("{}.{}.png", uuid::Uuid::new_v4(), index));
                let receipt = context
                    .http_transport
                    .get_to_path_same_origin_limited_with_headers(
                        &tile.url,
                        &[],
                        &destination,
                        response_limit,
                    )
                    .await
                    .map_err(sanitize_request_error)?;
                let path =
                    tempfile::TempPath::try_from_path(destination.clone()).map_err(|_| {
                        let _ = std::fs::remove_file(destination);
                        CoreError::Temporary("source windy tile could not be retained".into())
                    })?;
                let metadata = tokio::fs::metadata(&path).await.map_err(|_| {
                    CoreError::Temporary("source windy tile could not be read".into())
                })?;
                if metadata.len() != receipt.size_bytes {
                    return Err(CoreError::Transport(
                        "source windy tile changed while being validated".into(),
                    ));
                }
                let validation_path = path.to_path_buf();
                let limits = context.limits.clone();
                tokio::task::spawn_blocking(move || validate_png_file(&validation_path, &limits))
                    .await
                    .map_err(|_| {
                        CoreError::Temporary("source windy tile could not be validated".into())
                    })??;
                total_bytes = total_bytes.saturating_add(receipt.size_bytes);
                context.limits.validate_bytes(receipt.size_bytes, total_bytes)?;
                if total_bytes > context.limits.max_temp_bytes {
                    return Err(CoreError::ResourceLimit(
                        "source windy tiles exceed the temporary byte limit".into(),
                    ));
                }
                artifacts.push(RawArtifact {
                    receipt: ArtifactReceipt {
                        name: tile.name,
                        media_type: "image/png".into(),
                        size_bytes: receipt.size_bytes,
                        sha256: receipt.sha256,
                    },
                    path,
                });
            }

            if context.request_budget.cancellation.is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            Ok(RawFrame { frame, artifacts, private_locator: None })
        }))
    }
}

async fn fetch_browser_tiles(
    frame: FrameRef,
    context: SourceContext,
    temp_root: PathBuf,
    tiles: Vec<Tile>,
) -> CoreResult<RawFrame> {
    let mut browser =
        ChromiumSession::launch(&context, &temp_root, ChromiumConfig::new([HOST])).await?;
    let mut artifacts = Vec::with_capacity(tiles.len());
    let mut total_bytes = 0_u64;
    let adapter = WindySourceAdapter;
    for tile in tiles {
        if context.request_budget.cancellation.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let url = Url::parse(&tile.url).map_err(|_| invalid_frame())?;
        if !adapter.allows_artifact_url(&frame, &url) {
            return Err(invalid_frame());
        }
        let remaining_frame = context.limits.max_frame_bytes.saturating_sub(total_bytes);
        let remaining_temp = context.limits.max_temp_bytes.saturating_sub(total_bytes);
        let response_limit =
            context.limits.max_artifact_bytes.min(remaining_frame).min(remaining_temp);
        if response_limit == 0 {
            return Err(CoreError::ResourceLimit(
                "source windy tiles exceed the configured byte limit".into(),
            ));
        }
        let response =
            browser.fetch_response(tile.url.as_str(), None, &[], None, response_limit).await?;
        if !(200..300).contains(&response.status) {
            let message = if matches!(response.status, 401 | 403) {
                format!("source windy browser access was rejected (HTTP {})", response.status)
            } else {
                format!("source windy browser request failed (HTTP {})", response.status)
            };
            return Err(CoreError::Transport(message));
        }
        if let Some(content_type) = response.content_type.as_deref()
            && !content_type
                .split(';')
                .next()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("image/png"))
        {
            return Err(CoreError::Transport("source windy tile media type is invalid".into()));
        }
        let size_bytes = response.body.len() as u64;
        let artifact = persist_response_body(
            response.body,
            &temp_root,
            tile.name,
            "image/png".into(),
            &context.limits,
            total_bytes,
        )
        .await?;
        let validation_path = artifact.path.to_path_buf();
        let limits = context.limits.clone();
        tokio::task::spawn_blocking(move || validate_png_file(&validation_path, &limits))
            .await
            .map_err(|_| CoreError::Temporary("source windy tile could not be validated".into()))??;
        total_bytes = total_bytes.saturating_add(size_bytes);
        artifacts.push(artifact);
    }
    browser.close().await?;
    Ok(RawFrame { frame, artifacts, private_locator: None })
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<bool> {
    let source_selected =
        query.source.as_deref().is_none_or(|source| source == SOURCE || source == "all")
            && (query.sources.is_empty() || query.sources.iter().any(|source| source == SOURCE));
    if target.source != SOURCE || !source_selected {
        return Err(CoreError::Transport("source windy received a mismatched query".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport(
            "source windy only supports the reflectivity product".into(),
        ));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source windy does not expose base times".into()));
    }
    if !matches!(query.selector, TimeSelector::Latest) {
        return Err(CoreError::Transport("source windy only supports latest frames".into()));
    }
    let station_matches = target.station.as_deref().is_none_or(|station| station == "global")
        && (query.stations.is_empty() || query.stations.iter().any(|station| station == "global"));
    Ok(station_matches)
}

fn floor_time(now: DateTime<Utc>) -> CoreResult<DateTime<Utc>> {
    let epoch = now.timestamp().div_euclid(CADENCE_SECONDS) * CADENCE_SECONDS;
    DateTime::from_timestamp(epoch, 0)
        .ok_or_else(|| CoreError::Transport("source windy frame time is invalid".into()))
}

fn frame_from_time(valid_time: DateTime<Utc>) -> CoreResult<FrameRef> {
    let tiles = tiles_for_time(valid_time);
    let revision = format!("windy-{}", valid_time.timestamp());
    let mut frame = FrameRef {
        source: SOURCE.into(),
        product: PRODUCT.into(),
        station: Some("global".into()),
        valid_time: valid_time.to_rfc3339_opts(SecondsFormat::Micros, true),
        base_time: None,
        logical_id: String::new(),
        revision: Some(revision.clone()),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": tiles[0].url,
            "name": tiles[0].name,
            "artifacts": tiles.iter().skip(1).map(|tile| json!({
                "url": tile.url,
                "name": tile.name,
                "role": "tile",
                "media_type": "image/png",
            })).collect::<Vec<_>>(),
            "station": "global",
            "revision": revision,
        }),
    };
    frame.logical_id = logical_id(&frame)
        .map_err(|_| CoreError::Transport("source windy frame identity is invalid".into()))?;
    Ok(frame)
}

fn tiles_for_time(valid_time: DateTime<Utc>) -> Vec<Tile> {
    let path_time = valid_time.format("%Y/%m/%d/%H%M");
    let max_time = valid_time.format("%Y%m%d%H%M%S");
    let mut tiles = Vec::with_capacity(((1 << ZOOM) * (1 << ZOOM)) as usize);
    for y in 0..(1 << ZOOM) {
        for x in 0..(1 << ZOOM) {
            tiles.push(Tile {
                name: format!("tile-z{ZOOM}-x{x}-y{y}.png"),
                url: format!(
                    "https://{HOST}/radar2/composite/{path_time}/{ZOOM}/{x}/{y}/reflectivity.png?multichannel=true&maxt={max_time}"
                ),
            });
        }
    }
    tiles
}

fn frame_plan(frame: &FrameRef) -> Option<Vec<Tile>> {
    if frame.source != SOURCE
        || frame.product != PRODUCT
        || frame.station.as_deref() != Some("global")
        || frame.base_time.is_some()
        || frame.locator_version != LOCATOR_VERSION
        || logical_id(frame).ok().as_deref() != Some(frame.logical_id.as_str())
    {
        return None;
    }
    let valid_time = parse_utc_time(&frame.valid_time).ok()?;
    if valid_time.timestamp_subsec_nanos() != 0
        || valid_time.timestamp().rem_euclid(CADENCE_SECONDS) != 0
    {
        return None;
    }
    let expected = tiles_for_time(valid_time);
    let revision = format!("windy-{}", valid_time.timestamp());
    if frame.revision.as_deref() != Some(revision.as_str())
        || frame.locator.get("station").and_then(Value::as_str) != Some("global")
        || frame.locator.get("revision").and_then(Value::as_str) != Some(revision.as_str())
        || frame.locator.get("url").and_then(Value::as_str)
            != expected.first().map(|tile| tile.url.as_str())
        || frame.locator.get("name").and_then(Value::as_str)
            != expected.first().map(|tile| tile.name.as_str())
    {
        return None;
    }
    let artifacts = frame.locator.get("artifacts")?.as_array()?;
    if artifacts.len() != expected.len() - 1 {
        return None;
    }
    for (artifact, tile) in artifacts.iter().zip(expected.iter().skip(1)) {
        if artifact.get("url").and_then(Value::as_str) != Some(tile.url.as_str())
            || artifact.get("name").and_then(Value::as_str) != Some(tile.name.as_str())
            || artifact.get("role").and_then(Value::as_str) != Some("tile")
            || artifact.get("media_type").and_then(Value::as_str) != Some("image/png")
        {
            return None;
        }
    }
    Some(expected)
}

fn use_playwright(context: &SourceContext) -> CoreResult<bool> {
    match context.source_options.get("use_playwright") {
        None => Ok(false),
        Some(serde_yaml_ng::Value::Bool(selected)) => Ok(*selected),
        Some(_) => {
            Err(CoreError::Transport("sources.windy.use_playwright must be a boolean".into()))
        }
    }
}

fn validate_png_file(path: &Path, limits: &Limits) -> CoreResult<()> {
    let reader = image::ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(|_| CoreError::Transport("source windy tile is not a valid PNG".into()))?;
    if reader.format() != Some(image::ImageFormat::Png) {
        return Err(CoreError::Transport("source windy tile is not a valid PNG".into()));
    }
    let dimensions = reader
        .into_dimensions()
        .map_err(|_| CoreError::Transport("source windy tile is not a valid PNG".into()))?;
    validate_png_dimensions(dimensions, limits)?;

    let image = image::ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(|_| CoreError::Transport("source windy tile is not a valid PNG".into()))?
        .decode()
        .map_err(|_| CoreError::Transport("source windy tile is not a valid PNG".into()))?;
    if (image.width(), image.height()) != dimensions {
        return Err(CoreError::Transport("source windy tile changed while being validated".into()));
    }
    Ok(())
}

fn validate_png_dimensions(dimensions: (u32, u32), limits: &Limits) -> CoreResult<()> {
    let (width, height) = dimensions;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| CoreError::ResourceLimit("source windy tile dimensions overflow".into()))?;
    limits.validate_pixels(pixels)?;
    if (width, height) != (TILE_SIZE, TILE_SIZE) {
        return Err(CoreError::Transport(
            "source windy tile dimensions do not match the 256x256 tile contract".into(),
        ));
    }
    Ok(())
}

fn invalid_frame() -> CoreError {
    CoreError::Transport("source windy frame locator is invalid".into())
}

fn sanitize_request_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source windy acquisition requires network access".into())
        }
        CoreError::ResourceLimit(_) => {
            CoreError::ResourceLimit("source windy tile exceeds configured limits".into())
        }
        _ => CoreError::Transport("source windy tile request failed".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{Limits, RequestBudget};
    use crate::model::TimeSelector;
    use crate::transport::ftp::FtpTransport;
    use crate::transport::http::{HttpRequestCoalescer, HttpTransport};
    use std::collections::BTreeMap;

    fn time() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-25T03:40:00Z").unwrap().to_utc()
    }

    fn target() -> DiscoveryTarget {
        DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None }
    }

    fn context(
        query: Query,
        allow_network: bool,
        options: BTreeMap<String, serde_yaml_ng::Value>,
    ) -> SourceContext {
        let limits = Limits::default();
        let request_budget = Arc::new(RequestBudget::new(&limits));
        SourceContext {
            query,
            allow_network,
            discovery_workers: 4,
            source_options: Arc::new(options),
            request_budget: request_budget.clone(),
            limits: limits.clone(),
            http_transport: Arc::new(
                HttpTransport::with_budget(limits.clone(), false, request_budget.clone()).unwrap(),
            ),
            ftp_transport: Arc::new(FtpTransport::with_budget(limits, false, request_budget)),
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        }
    }

    fn fixture_frame() -> FrameRef {
        frame_from_time(time()).unwrap()
    }

    #[test]
    fn current_locator_has_four_ordered_tiles_and_stable_identity() {
        let frame = fixture_frame();
        let plan = frame_plan(&frame).unwrap();
        assert_eq!(frame.valid_time, "2026-09-25T03:40:00.000000Z");
        assert_eq!(plan.len(), 4);
        assert_eq!(plan[0].name, "tile-z1-x0-y0.png");
        assert_eq!(plan[1].name, "tile-z1-x1-y0.png");
        assert_eq!(plan[2].name, "tile-z1-x0-y1.png");
        assert_eq!(plan[3].name, "tile-z1-x1-y1.png");
        assert_eq!(
            plan[0].url,
            "https://rdr.windy.com/radar2/composite/2026/09/25/0340/1/0/0/reflectivity.png?multichannel=true&maxt=20260925034000"
        );
        // Golden captured from Python WindySource._make_ref for this timestamp.
        assert_eq!(
            frame.logical_id,
            "56e37054321852caef29adac7b8b2ad107ff42bc502f41d2bade2980b68c8c4f"
        );
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());
    }

    #[test]
    fn artifact_policy_binds_every_tile_to_its_frame() {
        let frame = fixture_frame();
        let adapter = WindySourceAdapter;
        assert!(adapter.allows_artifact_host(HOST));
        assert!(!adapter.allows_artifact_host("evil.rdr.windy.com"));
        for tile in frame_plan(&frame).unwrap() {
            assert!(adapter.allows_artifact_url(&frame, &Url::parse(&tile.url).unwrap()));
        }
        for address in [
            "https://rdr.windy.com/radar2/composite/2026/09/25/0340/1/2/0/reflectivity.png?multichannel=true&maxt=20260925034000",
            "https://rdr.windy.com:8443/radar2/composite/2026/09/25/0340/1/0/0/reflectivity.png?multichannel=true&maxt=20260925034000",
            "https://evil.example/radar2/composite/2026/09/25/0340/1/0/0/reflectivity.png?multichannel=true&maxt=20260925034000",
        ] {
            assert!(!adapter.allows_artifact_url(&frame, &Url::parse(address).unwrap()));
        }

        let mut forged = frame;
        forged.locator["artifacts"][0]["name"] = json!("forged.png");
        forged.logical_id = logical_id(&forged).unwrap();
        assert!(frame_plan(&forged).is_none());
    }

    #[tokio::test]
    async fn discovery_preserves_latest_global_query_and_rejects_historical_selection() {
        let adapter = Arc::new(WindySourceAdapter);
        let frames = adapter
            .clone()
            .discover(target(), context(Query::default(), true, BTreeMap::new()))
            .await
            .unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].station.as_deref(), Some("global"));
        assert_eq!(
            parse_utc_time(&frames[0].valid_time).unwrap().timestamp().rem_euclid(CADENCE_SECONDS),
            0
        );

        let filtered = adapter
            .clone()
            .discover(
                target(),
                context(
                    Query { stations: vec!["unknown".into()], ..Query::default() },
                    true,
                    BTreeMap::new(),
                ),
            )
            .await
            .unwrap();
        assert!(filtered.is_empty());

        let historical = Query {
            selector: TimeSelector::At { time: "2026-09-25T03:40:00Z".into() },
            ..Query::default()
        };
        assert!(
            adapter.discover(target(), context(historical, true, BTreeMap::new())).await.is_err()
        );
    }

    #[test]
    fn png_validation_checks_signature_dimensions_and_resource_budget() {
        let context = context(Query::default(), true, BTreeMap::new());
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            TILE_SIZE,
            TILE_SIZE,
            image::Rgba([0, 128, 0, 255]),
        ))
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
        let staging = tempfile::tempdir().unwrap();
        let valid = staging.path().join("valid.png");
        std::fs::write(&valid, cursor.get_ref()).unwrap();
        validate_png_file(&valid, &context.limits).unwrap();
        let pixel_limited =
            Limits { max_pixels: u64::from(TILE_SIZE).pow(2) - 1, ..Limits::default() };
        assert!(validate_png_file(&valid, &pixel_limited).is_err());

        let invalid = staging.path().join("invalid.png");
        std::fs::write(&invalid, b"not a png").unwrap();
        assert!(validate_png_file(&invalid, &context.limits).is_err());

        let mut oversized_cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            TILE_SIZE + 1,
            TILE_SIZE,
            image::Rgba([0, 128, 0, 255]),
        ))
        .write_to(&mut oversized_cursor, image::ImageFormat::Png)
        .unwrap();
        let oversized = staging.path().join("oversized.png");
        std::fs::write(&oversized, oversized_cursor.get_ref()).unwrap();
        assert!(validate_png_file(&oversized, &context.limits).is_err());
    }

    #[tokio::test]
    async fn playwright_configuration_is_typed_and_network_gated() {
        let adapter = Arc::new(WindySourceAdapter);
        let options = BTreeMap::from([("use_playwright".into(), serde_yaml_ng::Value::Bool(true))]);
        let enabled_context = context(Query::default(), true, options.clone());
        assert!(use_playwright(&enabled_context).unwrap());
        let error = adapter
            .clone()
            .fetch_raw(fixture_frame(), context(Query::default(), false, options), PathBuf::new())
            .unwrap()
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::NetworkDisabled(_)));

        let options =
            BTreeMap::from([("use_playwright".into(), serde_yaml_ng::Value::String("yes".into()))]);
        let error = adapter
            .fetch_raw(fixture_frame(), context(Query::default(), true, options), PathBuf::new())
            .unwrap()
            .await
            .unwrap_err();
        assert!(
            matches!(error, CoreError::Transport(message) if message.contains("must be a boolean"))
        );
    }
}
