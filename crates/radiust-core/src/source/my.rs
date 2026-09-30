//! Native discovery adapter for Malaysia's live composite images.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef};
use crate::source::{SourceAdapter, SourceContext};
use crate::transport::http::HttpMetadata;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::sync::Arc;

const EAST_URL: &str = "https://www.met.gov.my/data/radar_east.gif";
const PENINSULAR_URL: &str = "https://www.met.gov.my/data/radar_peninsular.gif";
const PRODUCT: &str = "composite";
const PAYLOAD_MEDIA_TYPE: &str = "image/png";
const LOCATOR_VERSION: &str = "my-legacy-v1";

/// Discovers Malaysia's east and peninsular composite image frames.
pub struct MySourceAdapter;

impl SourceAdapter for MySourceAdapter {
    fn source_id(&self) -> &'static str {
        "my"
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case("www.met.gov.my")
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            if target.source != "my" {
                return Err(CoreError::Transport("source my received an invalid target".into()));
            }
            if target.product.as_deref().is_some_and(|product| product != PRODUCT) {
                return Err(CoreError::Transport(
                    "source my only supports the composite product".into(),
                ));
            }
            let station = target.station.as_deref().ok_or_else(|| {
                CoreError::Transport("source my requires a supported station".into())
            })?;
            let url = station_url(station).ok_or_else(|| {
                CoreError::Transport("source my requires a supported station".into())
            })?;

            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source my discovery requires network access".into(),
                ));
            }

            let metadata = context
                .http_transport
                .head_metadata(url)
                .await
                .map_err(sanitize_transport_error)?;

            Ok(vec![frame_from_metadata(station, &metadata)?])
        })
    }
}

fn station_url(station: &str) -> Option<&'static str> {
    match station {
        "east" => Some(EAST_URL),
        "peninsular" => Some(PENINSULAR_URL),
        _ => None,
    }
}

fn sanitize_transport_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        _ => CoreError::Transport("source my metadata request failed".into()),
    }
}

fn frame_from_metadata(station: &str, metadata: &HttpMetadata) -> CoreResult<FrameRef> {
    let url = station_url(station)
        .ok_or_else(|| CoreError::Transport("source my requires a supported station".into()))?;
    if !(200..300).contains(&metadata.status) {
        return Err(CoreError::Transport("source my metadata response was not successful".into()));
    }

    let raw_modified = header(metadata, "last-modified")
        .ok_or_else(|| CoreError::Transport("source my response omitted Last-Modified".into()))?;
    let modified = DateTime::parse_from_rfc2822(raw_modified)
        .map_err(|_| CoreError::Transport("source my response has invalid Last-Modified".into()))?;

    let content_type = header(metadata, "content-type")
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !matches!(content_type.as_str(), "image/gif" | "image/png") {
        return Err(CoreError::Transport(
            "source my metadata response has an unsupported image Content-Type".into(),
        ));
    }

    let valid_time =
        modified.with_timezone(&Utc).checked_sub_signed(Duration::minutes(9)).ok_or_else(|| {
            CoreError::Transport("source my response has invalid Last-Modified".into())
        })?;
    let timestamp = valid_time.timestamp();
    let valid_time = valid_time.to_rfc3339_opts(SecondsFormat::Micros, true);
    let name = format!("my_{station}.png");
    let mut frame = FrameRef {
        source: "my".into(),
        product: PRODUCT.into(),
        station: Some(station.into()),
        valid_time,
        base_time: None,
        logical_id: String::new(),
        revision: Some(format!("{station}-{timestamp}")),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": url,
            "artifacts": [],
            "station": station,
            "name": name,
            "media_type": PAYLOAD_MEDIA_TYPE,
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source my frame identity could not be computed".into())
    })?;
    Ok(frame)
}

fn header<'a>(metadata: &'a HttpMetadata, name: &str) -> Option<&'a str> {
    metadata
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;
    use std::collections::BTreeMap;

    fn metadata(status: u16, headers: &[(&str, &str)]) -> HttpMetadata {
        HttpMetadata {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn valid_metadata_builds_expected_frame_and_identity() {
        let response = metadata(
            200,
            &[
                ("Last-Modified", "Mon, 29 Dec 2025 06:59:01 GMT"),
                ("Content-Type", "image/gif; charset=binary"),
            ],
        );
        let frame = frame_from_metadata("peninsular", &response).unwrap();

        assert_eq!(frame.source, "my");
        assert_eq!(frame.product, "composite");
        assert_eq!(frame.station.as_deref(), Some("peninsular"));
        assert_eq!(frame.valid_time, "2025-12-29T06:50:01.000000Z");
        assert_eq!(frame.revision.as_deref(), Some("peninsular-1766991001"));
        assert_eq!(frame.locator_version, "my-legacy-v1");
        assert_eq!(frame.locator["url"], PENINSULAR_URL);
        assert_eq!(frame.locator["artifacts"], json!([]));
        assert_eq!(frame.locator["station"], "peninsular");
        assert_eq!(frame.locator["name"], "my_peninsular.png");
        assert_eq!(frame.locator["media_type"], PAYLOAD_MEDIA_TYPE);

        let identity = frame_identity(&frame).unwrap();
        assert!(identity["locator"].get("url").is_none());
        assert_eq!(identity["locator"]["artifacts"], json!([]));
        assert_eq!(identity["locator"]["station"], "peninsular");
        assert_eq!(identity["locator"]["name"], "my_peninsular.png");
        assert_eq!(identity["locator"]["media_type"], PAYLOAD_MEDIA_TYPE);
        assert_eq!(
            frame.logical_id,
            "dae808b7c42bbfd5999bec584aa165e31319d046c8a4e739d8ab497e8b656a81"
        );
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());
    }

    #[test]
    fn missing_last_modified_is_rejected_without_echoing_response_values() {
        let response = metadata(
            200,
            &[("Content-Type", "image/png"), ("X-Diagnostic", "private-response-marker")],
        );
        let error = frame_from_metadata("east", &response).unwrap_err().to_string();
        assert!(error.contains("omitted Last-Modified"));
        assert!(!error.contains("private-response-marker"));
        assert!(!error.contains("met.gov.my"));
    }

    #[test]
    fn invalid_content_type_is_rejected_without_echoing_header_value() {
        let response = metadata(
            200,
            &[
                ("Last-Modified", "Mon, 29 Dec 2025 06:59:01 GMT"),
                ("Content-Type", "application/private-response-marker"),
            ],
        );
        let error = frame_from_metadata("east", &response).unwrap_err().to_string();
        assert!(error.contains("unsupported image Content-Type"));
        assert!(!error.contains("private-response-marker"));
        assert!(!error.contains("met.gov.my"));
    }

    #[test]
    fn last_modified_must_include_a_timezone() {
        let response = metadata(
            200,
            &[("Last-Modified", "Mon, 29 Dec 2025 06:59:01"), ("Content-Type", "image/png")],
        );
        assert!(frame_from_metadata("east", &response).is_err());
    }

    #[test]
    fn non_success_status_is_rejected() {
        let response = metadata(
            302,
            &[("Last-Modified", "Mon, 29 Dec 2025 06:59:01 GMT"), ("Content-Type", "image/png")],
        );
        assert!(frame_from_metadata("east", &response).is_err());
    }
}
