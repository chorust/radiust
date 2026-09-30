//! Native directory adapter for Weather Underground's legacy radar tile plan.
//!
//! The API key is required from source-specific configuration and never enters
//! a frame locator. Tile locators are retained for discovery compatibility;
//! the native adapter does not enable raw acquisition without a verified
//! provider response contract.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector};
use crate::source::{SourceAdapter, SourceContext, SourceOptions};
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::sync::Arc;

const SOURCE: &str = "wunderground";
const PRODUCT: &str = "composite";
const STATION: &str = "global";
const ENDPOINT: &str = "https://api0.weather.com/v3/TileServer/tile";
const CADENCE_SECONDS: i64 = 300;
const TILE_ZOOM: u8 = 1;
const LOCATOR_VERSION: &str = "wunderground-legacy-v1";

/// Produces the evidenced latest-only tile locator without enabling fetches.
pub struct WundergroundSourceAdapter;

impl SourceAdapter for WundergroundSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            validate_target_and_query(&target, &context.query)?;
            if !has_api_key(&context.source_options) {
                return Err(CoreError::Transport(
                    "source wunderground requires sources.wunderground.api_key".into(),
                ));
            }
            discover_at(&target, &context.query, Utc::now())
        })
    }
}

fn has_api_key(options: &SourceOptions) -> bool {
    options
        .get("api_key")
        .and_then(serde_yaml_ng::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport(
            "source wunderground received a mismatched source query".into(),
        ));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport(
            "source wunderground only supports the composite product".into(),
        ));
    }
    if target.station.as_deref().is_some_and(|station| station != STATION)
        || query.stations.iter().any(|station| station != STATION)
    {
        return Err(CoreError::Transport(
            "source wunderground only supports the global station".into(),
        ));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source wunderground does not expose base times".into()));
    }
    if !matches!(query.selector, TimeSelector::Latest) {
        return Err(CoreError::Transport("source wunderground only supports latest frames".into()));
    }
    Ok(())
}

fn discover_at(
    target: &DiscoveryTarget,
    query: &Query,
    now: DateTime<Utc>,
) -> CoreResult<Vec<FrameRef>> {
    validate_target_and_query(target, query)?;
    let timestamp = now.timestamp().div_euclid(CADENCE_SECONDS) * CADENCE_SECONDS;
    let valid_time = DateTime::from_timestamp(timestamp, 0)
        .ok_or_else(|| CoreError::Transport("source wunderground frame time is invalid".into()))?;
    let valid_time = valid_time.to_rfc3339_opts(SecondsFormat::Micros, true);

    let mut artifacts = Vec::with_capacity(3);
    let mut primary_url = None;
    for y in 0..(1_u32 << TILE_ZOOM) {
        for x in 0..(1_u32 << TILE_ZOOM) {
            let url =
                format!("{ENDPOINT}?product=wuRadarMosaic&ts={timestamp}&xyz={x}:{y}:{TILE_ZOOM}");
            let artifact = json!({
                "url": url,
                "name": format!("tile-z{TILE_ZOOM}-x{x}-y{y}.png"),
                "role": "tile",
                "media_type": "image/png",
            });
            if primary_url.is_none() {
                primary_url = Some(artifact["url"].clone());
            } else {
                artifacts.push(artifact);
            }
        }
    }

    let mut frame = FrameRef {
        source: SOURCE.into(),
        product: PRODUCT.into(),
        station: Some(STATION.into()),
        valid_time,
        base_time: None,
        logical_id: String::new(),
        // Cadence flooring is an unverified locator assumption, not an
        // upstream revision identifier.
        revision: None,
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": primary_url.ok_or_else(|| {
                CoreError::Transport("source wunderground tile plan is empty".into())
            })?,
            "artifacts": artifacts,
            "time_semantics": "unverified_cadence_floor_assumption",
            "tile_zoom": TILE_ZOOM,
            "tile_layout": "xyz-2x2-global",
            "geometry_status": "web_mercator_locator_only",
            "scientific_decode": "blocked_pending_verified_tile_semantics",
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source wunderground frame identity is invalid".into())
    })?;
    Ok(vec![frame])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{Limits, RequestBudget};
    use crate::transport::ftp::FtpTransport;
    use crate::transport::http::{HttpRequestCoalescer, HttpTransport};
    use serde_yaml_ng::Value as YamlValue;
    use std::collections::BTreeMap;

    fn context(api_key: Option<&str>) -> SourceContext {
        let limits = Limits::default();
        let request_budget = Arc::new(RequestBudget::new(&limits));
        let http_transport = Arc::new(
            HttpTransport::with_budget(limits.clone(), false, request_budget.clone()).unwrap(),
        );
        let source_options = api_key
            .map(|key| BTreeMap::from([("api_key".into(), YamlValue::String(key.into()))]))
            .unwrap_or_default();
        SourceContext {
            query: Query { source: Some(SOURCE.into()), ..Query::default() },
            allow_network: false,
            discovery_workers: 1,
            source_options: Arc::new(source_options),
            request_budget,
            limits: limits.clone(),
            http_transport,
            ftp_transport: Arc::new(FtpTransport::new(limits, false)),
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        }
    }

    fn target() -> DiscoveryTarget {
        DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None }
    }

    #[tokio::test]
    async fn requires_external_key_and_never_puts_it_in_the_discovered_locator() {
        let adapter = Arc::new(WundergroundSourceAdapter);
        let missing = adapter.clone().discover(target(), context(None)).await.unwrap_err();
        assert!(
            matches!(missing, CoreError::Transport(message) if message.contains("sources.wunderground.api_key"))
        );

        let key = "fixture-only-wu-key";
        let frames = adapter.clone().discover(target(), context(Some(key))).await.unwrap();
        let frame = &frames[0];
        assert_eq!(frame.station.as_deref(), Some(STATION));
        assert_eq!(frame.locator["artifacts"].as_array().unwrap().len(), 3);
        assert_eq!(frame.locator["time_semantics"], "unverified_cadence_floor_assumption");
        assert!(!frame.locator.to_string().contains(key));
        assert!(!serde_json::to_string(frame).unwrap().contains(key));
        assert!(!adapter.allows_artifact_host("api0.weather.com"));
    }

    #[test]
    fn cadence_locator_is_latest_only_and_does_not_claim_a_provider_revision() {
        let fixed_now = DateTime::parse_from_rfc3339("2026-09-18T04:12:34Z").unwrap().to_utc();
        let frames = discover_at(&target(), &Query::default(), fixed_now).unwrap();
        let frame = &frames[0];
        assert_eq!(frame.valid_time, "2026-09-18T04:10:00.000000Z");
        assert_eq!(frame.revision, None);
        assert!(frame.locator["url"].as_str().unwrap().starts_with(ENDPOINT));

        let at_query = Query {
            selector: TimeSelector::At { time: frame.valid_time.clone() },
            ..Query::default()
        };
        assert!(discover_at(&target(), &at_query, fixed_now).is_err());
    }
}
