//! Raw-only native locator adapter for BMKG's public radar tile endpoint.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector, parse_utc_time};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "bmkg";
const PRODUCT: &str = "composite";
const HOST: &str = "inasiam.bmkg.go.id";
const PATH_PREFIX: &str = "/api23/mpl_req/radar/radar/0";
const LOCATOR_VERSION: &str = "bmkg-legacy-v1";
const CADENCE_SECONDS: i64 = 600;
const ZOOM: u32 = 1;

/// BMKG publishes a synthetic latest timestamp and four TMS tiles. It has no
/// verified scientific decoder, so this adapter only discovers and acquires
/// the original tile bytes.
pub struct BmkgSourceAdapter;

impl SourceAdapter for BmkgSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(HOST)
    }

    fn allows_artifact_url(&self, frame: &FrameRef, url: &Url) -> bool {
        let Some(plan) = frame_plan(frame) else {
            return false;
        };
        plan.urls.iter().any(|address| {
            Url::parse(address).ok().as_ref() == Some(url)
                && url.scheme() == "https"
                && url.host_str().is_some_and(|host| self.allows_artifact_host(host))
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query() == Some("overlays=contourf")
                && url.fragment().is_none()
        })
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            validate_target_and_query(&target, &context.query)?;
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source bmkg discovery requires network access".into(),
                ));
            }
            select_latest_at(&context.query, Utc::now())
        })
    }
}

#[derive(Clone, Debug)]
struct FramePlan {
    urls: Vec<String>,
    names: Vec<String>,
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport("source bmkg received a mismatched source query".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport("source bmkg only supports the composite product".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source bmkg does not expose base times".into()));
    }
    if !matches!(query.selector, TimeSelector::Latest) {
        return Err(CoreError::Transport("source bmkg only supports latest frames".into()));
    }
    Ok(())
}

fn select_latest_at(query: &Query, now: DateTime<Utc>) -> CoreResult<Vec<FrameRef>> {
    if !query.stations.is_empty() && !query.stations.iter().any(|station| station == "global") {
        return Ok(Vec::new());
    }
    let epoch_seconds = now.timestamp().div_euclid(CADENCE_SECONDS) * CADENCE_SECONDS;
    let valid_time = DateTime::from_timestamp(epoch_seconds, 0)
        .ok_or_else(|| CoreError::Transport("source bmkg frame time is invalid".into()))?;
    if query.max_age_secs.is_some_and(|max_age| {
        let age = (now - valid_time).num_microseconds().unwrap_or(0) as f64 / 1_000_000.0;
        age >= 0.0 && age > max_age
    }) {
        return Ok(Vec::new());
    }
    Ok(vec![frame_from_time(valid_time)?])
}

fn frame_from_time(valid_time: DateTime<Utc>) -> CoreResult<FrameRef> {
    let epoch_seconds = valid_time.timestamp();
    let stamp = valid_time.format("%Y%m%d%H%M").to_string();
    let plan = frame_plan_for_time(epoch_seconds, &stamp)
        .ok_or_else(|| CoreError::Transport("source bmkg frame locator is invalid".into()))?;
    let revision = format!("bmkg-{epoch_seconds}");
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
            "url": plan.urls[0],
            "artifacts": plan.urls.iter().zip(&plan.names).skip(1).map(|(url, name)| json!({
                "url": url,
                "name": name,
                "role": "tile",
                "media_type": "image/png",
            })).collect::<Vec<_>>(),
            "station": "global",
            "revision": revision,
            "name": plan.names[0],
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source bmkg frame identity could not be computed".into())
    })?;
    Ok(frame)
}

fn frame_plan(frame: &FrameRef) -> Option<FramePlan> {
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
    let epoch_seconds = valid_time.timestamp();
    if valid_time.timestamp_subsec_nanos() != 0 || epoch_seconds.rem_euclid(CADENCE_SECONDS) != 0 {
        return None;
    }
    let stamp = valid_time.format("%Y%m%d%H%M").to_string();
    let plan = frame_plan_for_time(epoch_seconds, &stamp)?;
    let revision = format!("bmkg-{epoch_seconds}");
    if frame.revision.as_deref() != Some(revision.as_str())
        || frame.locator.get("station").and_then(Value::as_str) != Some("global")
        || frame.locator.get("revision").and_then(Value::as_str) != Some(revision.as_str())
        || frame.locator.get("name").and_then(Value::as_str)
            != plan.names.first().map(String::as_str)
        || frame.locator.get("url").and_then(Value::as_str) != plan.urls.first().map(String::as_str)
    {
        return None;
    }
    let artifacts = frame.locator.get("artifacts")?.as_array()?;
    if artifacts.len() != 3 {
        return None;
    }
    for (artifact, (url, name)) in artifacts.iter().zip(plan.urls.iter().zip(&plan.names).skip(1)) {
        if artifact.get("url").and_then(Value::as_str) != Some(url)
            || artifact.get("name").and_then(Value::as_str) != Some(name)
            || artifact.get("role").and_then(Value::as_str) != Some("tile")
            || artifact.get("media_type").and_then(Value::as_str) != Some("image/png")
        {
            return None;
        }
    }
    Some(plan)
}

pub(super) fn has_verified_raw_tile_plan(frame: &FrameRef) -> bool {
    frame_plan(frame).is_some()
}

fn frame_plan_for_time(epoch_seconds: i64, stamp: &str) -> Option<FramePlan> {
    if stamp.len() != 12
        || !stamp.bytes().all(|byte| byte.is_ascii_digit())
        || epoch_seconds.rem_euclid(CADENCE_SECONDS) != 0
    {
        return None;
    }
    let mut urls = Vec::with_capacity(4);
    let mut names = Vec::with_capacity(4);
    for y in 0..(1 << ZOOM) {
        for x in 0..(1 << ZOOM) {
            let tms_y = (1 << ZOOM) - 1 - y;
            urls.push(format!(
                "https://{HOST}{PATH_PREFIX}/{stamp}/{stamp}/{ZOOM}/{x}/{tms_y}.png?overlays=contourf"
            ));
            names.push(format!("tile-z{ZOOM}-x{x}-y{y}.png"));
        }
    }
    Some(FramePlan { urls, names })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;
    use crate::model::{ArtifactReceipt, RawArtifact, RawFrame};
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use sha2::{Digest, Sha256};
    use std::io::{Cursor, Write};

    fn fixture_time() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-25T03:40:00Z").unwrap().to_utc()
    }

    fn fixture_frame() -> FrameRef {
        frame_from_time(fixture_time()).unwrap()
    }

    #[test]
    fn cadence_floor_builds_the_four_tms_urls_and_stable_legacy_identity() {
        let frame = fixture_frame();
        let plan = frame_plan(&frame).unwrap();
        assert_eq!(frame.valid_time, "2026-09-25T03:40:00.000000Z");
        assert_eq!(plan.urls.len(), 4);
        assert_eq!(plan.names[0], "tile-z1-x0-y0.png");
        assert_eq!(
            plan.urls[0],
            "https://inasiam.bmkg.go.id/api23/mpl_req/radar/radar/0/202609250340/202609250340/1/0/1.png?overlays=contourf"
        );
        assert_eq!(
            plan.urls[2],
            "https://inasiam.bmkg.go.id/api23/mpl_req/radar/radar/0/202609250340/202609250340/1/0/0.png?overlays=contourf"
        );
        assert_eq!(frame.locator_version, LOCATOR_VERSION);
        assert_eq!(
            frame.logical_id,
            "43e87f6106b36704a235dcabaf769d8f6f1f24aad1d587a9bc009e3bf4428517"
        );
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());
        assert_eq!(frame_identity(&frame).unwrap()["locator"]["url"], Value::Null);
    }

    #[test]
    fn artifact_policy_accepts_only_the_four_urls_bound_to_the_frame() {
        let frame = fixture_frame();
        let adapter = BmkgSourceAdapter;
        assert!(adapter.allows_artifact_host(HOST));
        assert!(!adapter.allows_artifact_host("evil.bmkg.go.id"));
        for url in frame_plan(&frame).unwrap().urls {
            assert!(adapter.allows_artifact_url(&frame, &Url::parse(&url).unwrap()));
        }
        for value in [
            "https://evil.example/api23/mpl_req/radar/radar/0/202609250340/202609250340/1/0/1.png?overlays=contourf",
            "https://inasiam.bmkg.go.id.evil.example/api23/mpl_req/radar/radar/0/202609250340/202609250340/1/0/1.png?overlays=contourf",
            "https://inasiam.bmkg.go.id/api23/mpl_req/radar/radar/0/202609250340/202609250340/1/2/0.png?overlays=contourf",
            "https://inasiam.bmkg.go.id:8443/api23/mpl_req/radar/radar/0/202609250340/202609250340/1/0/1.png?overlays=contourf",
        ] {
            assert!(!adapter.allows_artifact_url(&frame, &Url::parse(value).unwrap()));
        }
    }

    #[test]
    fn validated_raw_tiles_compose_in_xyz_order_without_changing_pixels() {
        let frame = fixture_frame();
        let plan = frame_plan(&frame).unwrap();
        let colors = [
            Rgba([10, 20, 30, 255]),
            Rgba([40, 50, 60, 255]),
            Rgba([70, 80, 90, 255]),
            Rgba([100, 110, 120, 255]),
        ];
        let artifacts = plan
            .names
            .iter()
            .zip(colors)
            .map(|(name, color)| {
                let image = RgbaImage::from_pixel(256, 256, color);
                let mut encoded = Cursor::new(Vec::new());
                DynamicImage::ImageRgba8(image).write_to(&mut encoded, ImageFormat::Png).unwrap();
                let bytes = encoded.into_inner();
                let mut file = tempfile::NamedTempFile::new().unwrap();
                file.write_all(&bytes).unwrap();
                RawArtifact {
                    receipt: ArtifactReceipt {
                        name: name.clone(),
                        media_type: "image/png".into(),
                        size_bytes: bytes.len() as u64,
                        sha256: hex::encode(Sha256::digest(&bytes)),
                    },
                    path: file.into_temp_path(),
                }
            })
            .collect();
        let raw = RawFrame { frame, artifacts, private_locator: None };

        let preview =
            crate::source::tiles::preview_source_tiles(&raw, &crate::limits::Limits::default())
                .unwrap();
        assert_eq!((preview.preview.width, preview.preview.height), (512, 512));
        assert_eq!(preview.tile_count, 4);
        for (index, expected) in colors.iter().enumerate() {
            let x = (index as u32 % 2) * 256 + 128;
            let y = (index as u32 / 2) * 256 + 128;
            let offset = ((y * 512 + x) * 4) as usize;
            assert_eq!(&preview.preview.rgba[offset..offset + 4], &expected.0);
        }
    }

    #[test]
    fn raw_tile_preview_rejects_a_forged_bmkg_locator() {
        let mut frame = fixture_frame();
        frame.locator["artifacts"][0]["url"] =
            Value::String("https://inasiam.bmkg.go.id/forged.png?overlays=contourf".into());
        frame.logical_id = logical_id(&frame).unwrap();
        let raw = RawFrame { frame, artifacts: Vec::new(), private_locator: None };

        let error =
            crate::source::tiles::preview_source_tiles(&raw, &crate::limits::Limits::default())
                .unwrap_err();
        assert!(
            matches!(error, CoreError::Transport(message) if message == "raw tile preview frame locator is invalid")
        );
    }

    #[test]
    fn forged_frame_or_tile_metadata_cannot_change_the_acquisition_plan() {
        let mut frame = fixture_frame();
        frame.locator["artifacts"][0]["url"] =
            Value::String("https://inasiam.bmkg.go.id/other.png?overlays=contourf".into());
        frame.logical_id = logical_id(&frame).unwrap();
        assert!(frame_plan(&frame).is_none());

        let mut frame = fixture_frame();
        frame.station = Some("MAJ".into());
        frame.logical_id = logical_id(&frame).unwrap();
        assert!(frame_plan(&frame).is_none());
    }

    #[test]
    fn latest_uses_the_cadence_floor_and_respects_age_and_station_filters() {
        let now = DateTime::parse_from_rfc3339("2026-09-25T03:49:59Z").unwrap().to_utc();
        let latest = select_latest_at(&Query::default(), now).unwrap();
        assert_eq!(latest[0].valid_time, "2026-09-25T03:40:00.000000Z");
        let stale = Query { max_age_secs: Some(10.0), ..Query::default() };
        assert!(select_latest_at(&stale, now).unwrap().is_empty());
        let other_station = Query { stations: vec!["CGK".into()], ..Query::default() };
        assert!(select_latest_at(&other_station, now).unwrap().is_empty());
        let global = Query { stations: vec!["global".into()], ..Query::default() };
        assert_eq!(select_latest_at(&global, now).unwrap().len(), 1);
    }

    #[test]
    fn only_latest_composite_queries_without_base_time_are_supported() {
        let target =
            DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None };
        assert!(validate_target_and_query(&target, &Query::default()).is_ok());
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { source: "id".into(), ..target.clone() },
                &Query::default()
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target,
                &Query { product: Some("rain".into()), ..Query::default() }
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target,
                &Query {
                    selector: TimeSelector::At { time: "2026-09-25T03:40:00Z".into() },
                    ..Query::default()
                }
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target,
                &Query { base_time: Some("2026-09-25T03:40:00Z".into()), ..Query::default() }
            )
            .is_err()
        );
    }
}
