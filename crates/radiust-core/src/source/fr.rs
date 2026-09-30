//! Native discovery adapter for the Météo-France radar WMS.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, Duration, SecondsFormat, Timelike, Utc};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::sync::Arc;

const WMS_BASE_URL: &str = "https://rwg.meteofrance.com/geoservices/Radar-mapcache-WMS";
const PAGE_URL: &str = "https://meteofrance.com/images-radar";
const PRODUCT: &str = "composite";
const STATION: &str = "FRCOMP";
const WMS_LAYER: &str = "BASE_REFLECTIVITY";
const WMS_STYLE: &str = "synopsis_reflectivity_oppidum_transparence";
const LOCATOR_VERSION: &str = "fr-legacy-v1";
const BBOX: [f64; 4] = [-5.5, 41.0, 10.0, 51.5];
const WIDTH: u32 = 700;
const HEIGHT: u32 = 600;

/// Discovers the current Météo-France national composite without contacting
/// the radar page. The ephemeral WMS session token is acquired at download time.
pub struct FrSourceAdapter;

impl SourceAdapter for FrSourceAdapter {
    fn source_id(&self) -> &'static str {
        "fr"
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move { discover_at(&target, &context.query, Utc::now()) })
    }
}

fn discover_at(
    target: &DiscoveryTarget,
    query: &Query,
    now: DateTime<Utc>,
) -> CoreResult<Vec<FrameRef>> {
    if target.source != "fr" {
        return Err(CoreError::Transport("source fr received an invalid target".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport("source fr only supports the composite product".into()));
    }
    if target.station.as_deref().is_some_and(|station| station != STATION) {
        return Err(CoreError::Transport("source fr only supports the FRCOMP station".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source fr does not expose base times".into()));
    }
    if !matches!(query.selector, TimeSelector::Latest) {
        return Err(CoreError::Transport("source fr only supports latest frames".into()));
    }
    if !query.stations.is_empty() && !query.stations.iter().any(|station| station == STATION) {
        return Ok(Vec::new());
    }

    Ok(vec![frame_at(now)?])
}

fn current_frame_time(now: DateTime<Utc>) -> DateTime<Utc> {
    let guarded = now - Duration::minutes(5);
    let minute = guarded.minute() - guarded.minute() % 15;
    guarded
        .with_minute(minute)
        .and_then(|time| time.with_second(0))
        .and_then(|time| time.with_nanosecond(0))
        .expect("minute and second are in range")
}

fn frame_at(now: DateTime<Utc>) -> CoreResult<FrameRef> {
    let frame_time = current_frame_time(now);
    let timestamp = frame_time.format("%Y%m%dT%H%M%SZ").to_string();
    let name = format!("{STATION}_{timestamp}.png");
    let request_bbox = projected_bbox(BBOX);
    let url = wms_url(&request_bbox, &timestamp);

    let mut frame = FrameRef {
        source: "fr".into(),
        product: PRODUCT.into(),
        station: Some(STATION.into()),
        valid_time: frame_time.to_rfc3339_opts(SecondsFormat::Micros, true),
        base_time: None,
        logical_id: String::new(),
        revision: Some(timestamp.clone()),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": url,
            "artifacts": [],
            "bbox": BBOX,
            "headers": {
                "Accept": "*/*",
                "Referer": PAGE_URL,
                "User-Agent": "Mozilla/5.0 (compatible; radiust/1)",
            },
            "station": STATION,
            "revision": timestamp,
            "name": name,
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source fr frame identity could not be computed".into())
    })?;
    Ok(frame)
}

fn projected_bbox(bbox: [f64; 4]) -> [String; 4] {
    let origin_shift = 20_037_508.342_789_244_f64;
    let project = |longitude: f64, latitude: f64| {
        let x = longitude * origin_shift / 180.0;
        let latitude = latitude.clamp(-89.9, 89.9);
        let y = ((90.0 + latitude) * std::f64::consts::PI / 360.0).tan().ln() * origin_shift
            / std::f64::consts::PI;
        (x, y)
    };

    let (x0, y0) = project(bbox[0], bbox[1]);
    let (x1, y1) = project(bbox[2], bbox[3]);
    [x0, y0, x1, y1].map(|coordinate| format!("{coordinate:.6}"))
}

fn wms_url(request_bbox: &[String; 4], timestamp: &str) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("service", "WMS")
        .append_pair("request", "GetMap")
        .append_pair("version", "1.3.0")
        .append_pair("layers", WMS_LAYER)
        .append_pair("styles", WMS_STYLE)
        .append_pair("format", "image/png")
        .append_pair("transparent", "true")
        .append_pair("crs", "EPSG:3857")
        .append_pair("bbox", &request_bbox.join(","))
        .append_pair("width", &WIDTH.to_string())
        .append_pair("height", &HEIGHT.to_string())
        .append_pair("time", &format_wms_time(timestamp));
    format!("{WMS_BASE_URL}?{}", query.finish())
}

fn format_wms_time(timestamp: &str) -> String {
    // Revisions use YYYYmmddTHHMMSSZ while WMS TIME uses ISO-8601.
    format!(
        "{}-{}-{}T{}:{}:{}Z",
        &timestamp[0..4],
        &timestamp[4..6],
        &timestamp[6..8],
        &timestamp[9..11],
        &timestamp[11..13],
        &timestamp[13..15],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;
    use crate::limits::{Limits, RequestBudget};
    use crate::transport::ftp::FtpTransport;
    use crate::transport::http::{HttpRequestCoalescer, HttpTransport};
    use chrono::TimeZone;
    use url::Url;

    fn fixed_now(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value).unwrap().with_timezone(&Utc)
    }

    fn target() -> DiscoveryTarget {
        DiscoveryTarget { source: "fr".into(), product: Some(PRODUCT.into()), station: None }
    }

    fn query() -> Query {
        Query { source: Some("fr".into()), ..Query::default() }
    }

    #[test]
    fn floors_the_five_minute_guard_to_a_fifteen_minute_boundary() {
        assert_eq!(
            current_frame_time(fixed_now("2026-09-18T02:53:34Z")).to_rfc3339(),
            "2026-09-18T02:45:00+00:00"
        );
        assert_eq!(
            current_frame_time(fixed_now("2026-09-18T03:05:00Z")).to_rfc3339(),
            "2026-09-18T03:00:00+00:00"
        );
    }

    #[test]
    fn builds_unsigned_getmap_reference_with_legacy_identity_and_revision() {
        let frame = frame_at(fixed_now("2026-09-18T02:53:34Z")).unwrap();

        assert_eq!(frame.source, "fr");
        assert_eq!(frame.product, PRODUCT);
        assert_eq!(frame.station.as_deref(), Some(STATION));
        assert_eq!(frame.valid_time, "2026-09-18T02:45:00.000000Z");
        assert_eq!(frame.revision.as_deref(), Some("20260918T024500Z"));
        assert_eq!(frame.locator_version, LOCATOR_VERSION);
        assert_eq!(frame.locator["artifacts"], json!([]));
        assert_eq!(frame.locator["bbox"], json!([-5.5, 41.0, 10.0, 51.5]));
        assert_eq!(frame.locator["station"], STATION);
        assert_eq!(frame.locator["name"], "FRCOMP_20260918T024500Z.png");
        assert_eq!(frame.locator["headers"]["Accept"], "*/*");
        assert_eq!(frame.locator["headers"]["Referer"], PAGE_URL);

        let url = Url::parse(frame.locator["url"].as_str().unwrap()).unwrap();
        assert_eq!(url.path(), "/geoservices/Radar-mapcache-WMS");
        let params = url.query_pairs().into_owned().collect::<Vec<_>>();
        assert_eq!(
            params,
            vec![
                ("service".into(), "WMS".into()),
                ("request".into(), "GetMap".into()),
                ("version".into(), "1.3.0".into()),
                ("layers".into(), WMS_LAYER.into()),
                ("styles".into(), WMS_STYLE.into()),
                ("format".into(), "image/png".into()),
                ("transparent".into(), "true".into()),
                ("crs".into(), "EPSG:3857".into()),
                (
                    "bbox".into(),
                    "-612257.199363,5012341.663848,1113194.907933,6710219.083221".into(),
                ),
                ("width".into(), "700".into()),
                ("height".into(), "600".into()),
                ("time".into(), "2026-09-18T02:45:00Z".into()),
            ]
        );
        assert!(!frame.locator["url"].as_str().unwrap().contains("token"));

        // Golden digest is also the Python logical_id for the equivalent
        // LegacyImageSource reference after volatile URL and headers removal.
        assert_eq!(
            frame.logical_id,
            "58392fc521ba267e4c7d85c600edcb49fa14791f57532a5680981fb3f936bdca"
        );
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());
        let identity = frame_identity(&frame).unwrap();
        assert!(identity["locator"].get("url").is_none());
        assert!(identity["locator"].get("headers").is_none());
        assert_eq!(identity["locator"]["bbox"], json!([-5.5, 41.0, 10.0, 51.5]));
        assert_eq!(identity["locator"]["revision"], "20260918T024500Z");
        assert_eq!(identity["locator"]["name"], "FRCOMP_20260918T024500Z.png");
    }

    #[test]
    fn discovery_is_local_latest_only_and_rejects_base_times() {
        let refs =
            discover_at(&target(), &query(), Utc.with_ymd_and_hms(2026, 9, 18, 2, 53, 34).unwrap())
                .unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].valid_time, "2026-09-18T02:45:00.000000Z");

        let at_query =
            Query { selector: TimeSelector::At { time: "2026-09-18T02:45:00Z".into() }, ..query() };
        assert!(discover_at(&target(), &at_query, Utc::now()).is_err());

        let base_time_query = Query { base_time: Some("2026-09-18T02:45:00Z".into()), ..query() };
        assert!(discover_at(&target(), &base_time_query, Utc::now()).is_err());

        let unsupported_station =
            DiscoveryTarget { station: Some("not-frcomp".into()), ..target() };
        assert!(discover_at(&unsupported_station, &query(), Utc::now()).is_err());
    }

    #[tokio::test]
    async fn adapter_discovers_without_network_or_a_session_token() {
        let limits = Limits::default();
        let request_budget = Arc::new(RequestBudget::new(&limits));
        let http_transport = Arc::new(
            HttpTransport::with_budget(limits.clone(), false, request_budget.clone()).unwrap(),
        );
        let context = SourceContext {
            query: query(),
            allow_network: false,
            discovery_workers: 4,
            source_options: Arc::new(Default::default()),
            request_budget,
            ftp_transport: Arc::new(FtpTransport::new(limits.clone(), false)),
            limits,
            http_transport,
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        };

        let frames = Arc::new(FrSourceAdapter).discover(target(), context).await.unwrap();
        assert_eq!(frames.len(), 1);
        assert!(!frames[0].locator["url"].as_str().unwrap().contains("token"));
    }
}
