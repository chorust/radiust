//! RDCAP station identity parsing and provider-specific helpers.

use crate::errors::ProviderError;
use crate::errors::{CoreError, CoreResult};
use crate::model::{ArtifactReceipt, DiscoveryTarget, FrameRef, RawArtifact, RawFrame};
use crate::source::catalog::{
    CatalogDirectoryConflict, CatalogMetadata, CatalogProvenance, CatalogStation,
    CountryCapabilities, RecentQueryCapability, StationCatalogUpdate,
};
use crate::source::{SourceAdapter, SourceContext};
use crate::transport::http::HttpGetPolicy;
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use url::Url;

const COUNTRIES: [&str; 3] = ["TWN", "JPN", "PHL"];
const SOURCE: &str = "rdcap";
const PRODUCT: &str = "reflectivity";
const BASE_URL: &str = "https://rdcap.cwa.gov.tw";
const COUNTRY_LIST_PATH: &str = "/data_access/get_country_list";
const RADAR_LIST_PATH: &str = "/data_access/get_radar_list";
const RADAR_DATA_PATH: &str = "/data_access/get_radar_data";
const FILE_PATH: &str = "/file";
const INDEX_REFERER_PATH: &str = "/data_access/radar_map";
const MAX_DIRECTORY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RdcapStationId<'a> {
    pub country: &'a str,
    pub station_code: &'a str,
}

/// Parses only a canonical public RDCAP station id such as `TWN/RCHL`.
pub(crate) fn parse_station_id(value: &str) -> Option<RdcapStationId<'_>> {
    let (country, station_code) = value.split_once('/')?;
    if value.matches('/').count() != 1
        || !COUNTRIES.contains(&country)
        || !valid_station_code(station_code)
    {
        return None;
    }
    Some(RdcapStationId { country, station_code })
}

/// Normalizes a directory country/code pair into its canonical public id.
/// Directory codes are trimmed and uppercased; the result still has to obey
/// the strict public id grammar.
pub(crate) fn normalize_catalog_station(country: &str, code: &str) -> Option<String> {
    let country = country.trim().to_ascii_uppercase();
    let code = code.trim().to_ascii_uppercase();
    if !COUNTRIES.contains(&country.as_str()) || !valid_station_code(&code) {
        return None;
    }
    Some(format!("{country}/{code}"))
}

/// Query selectors may use a short code before Engine resolves uniqueness
/// against the complete station directory. Canonical ids remain strict.
pub(crate) fn is_valid_station_selection(value: &str) -> bool {
    parse_station_id(value).is_some() || valid_station_code(value)
}

pub(crate) fn is_valid_station_id(value: &str) -> bool {
    parse_station_id(value).is_some()
}

pub(crate) fn resolve_station_selection(
    selection: &str,
    station_ids: &[String],
) -> Result<String, ProviderError> {
    if parse_station_id(selection).is_some() {
        return station_ids
            .iter()
            .find(|station_id| station_id.as_str() == selection)
            .cloned()
            .ok_or(ProviderError::UnknownStation);
    }
    if !valid_station_code(selection) {
        return Err(ProviderError::UnknownStation);
    }
    let mut matches = station_ids.iter().filter(|station_id| {
        parse_station_id(station_id).is_some_and(|parsed| parsed.station_code == selection)
    });
    let Some(found) = matches.next() else {
        return Err(ProviderError::UnknownStation);
    };
    if matches.next().is_some() {
        return Err(ProviderError::AmbiguousIndex);
    }
    Ok(found.clone())
}

pub(crate) fn epoch_millis_to_utc(key: &str) -> Result<DateTime<Utc>, ProviderError> {
    if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ProviderError::UnexpectedBody);
    }
    let millis = key.parse::<i64>().map_err(|_| ProviderError::UnexpectedBody)?;
    DateTime::from_timestamp_millis(millis).ok_or(ProviderError::UnexpectedBody)
}

pub(crate) fn epoch_millis_to_rfc3339(key: &str) -> Result<String, ProviderError> {
    Ok(epoch_millis_to_utc(key)?.to_rfc3339_opts(SecondsFormat::Micros, true))
}

fn valid_station_code(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

#[derive(Clone)]
pub(crate) struct RdcapSourceAdapter {
    base_url: String,
}

impl Default for RdcapSourceAdapter {
    fn default() -> Self {
        Self { base_url: BASE_URL.to_owned() }
    }
}

impl RdcapSourceAdapter {
    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    fn map_referer(&self) -> String {
        self.endpoint(INDEX_REFERER_PATH)
    }

    fn station_referer(&self, country: &str, station_code: &str) -> String {
        format!(
            "{}/data_access/radar_display/{country}/{station_code}",
            self.base_url.trim_end_matches('/')
        )
    }

    async fn request_station_index(
        &self,
        context: &SourceContext,
        country: &str,
        station_code: &str,
    ) -> CoreResult<Vec<FrameRef>> {
        if !context.allow_network {
            return Err(CoreError::NetworkDisabled(
                "RDCAP index request requires network access".into(),
            ));
        }
        let referer = self.station_referer(country, station_code);
        let headers = [
            ("X-Requested-With", "XMLHttpRequest"),
            ("Referer", referer.as_str()),
            ("Accept", "application/json, text/plain, */*"),
        ];
        let payload = context
            .http_transport
            .post_form_bytes_with_headers(
                &self.endpoint(RADAR_DATA_PATH),
                &[("country", country), ("radar_name", station_code), ("datetime", "")],
                &headers,
            )
            .await
            .map_err(sanitize_index_error)?;
        parse_station_index(&payload, &format!("{country}/{station_code}"), &self.base_url)
            .map_err(CoreError::Provider)
    }

    async fn fetch_raw_with_ticket(
        &self,
        mut frame: FrameRef,
        context: SourceContext,
        temp_root: std::path::PathBuf,
    ) -> CoreResult<RawFrame> {
        if !context.allow_network {
            return Err(CoreError::NetworkDisabled(
                "RDCAP raw acquisition requires network access".into(),
            ));
        }
        let (country, station_code, key, mut ticket_url, referer) =
            validate_frame_ticket(&frame, &self.base_url)?;
        let mut used_tickets = BTreeSet::new();
        let mut file_requests = 0_u8;
        let mut index_refreshes = 0_u8;
        loop {
            if used_tickets.insert(ticket_url.clone()) {
                if file_requests >= 3 {
                    return Err(CoreError::Provider(ProviderError::TicketExhausted));
                }
                file_requests += 1;
                let destination = temp_root.join(format!("rdcap-{}.tmp", uuid::Uuid::new_v4()));
                let headers = [("Referer", referer.as_str())];
                let response = context
                    .http_transport
                    .get_to_path_same_origin_path_limited_with_policy(
                        &ticket_url,
                        &headers,
                        &destination,
                        context
                            .limits
                            .max_frame_bytes
                            .min(context.limits.max_artifact_bytes)
                            .min(context.limits.max_temp_bytes),
                        HttpGetPolicy::SingleAttempt,
                        FILE_PATH,
                    )
                    .await;
                match response {
                    Ok(receipt) => {
                        let bytes = match tokio::fs::read(&destination).await {
                            Ok(bytes) => bytes,
                            Err(_) => {
                                let _ = tokio::fs::remove_file(&destination).await;
                                return Err(CoreError::Temporary(
                                    "RDCAP response could not be validated".into(),
                                ));
                            }
                        };
                        match validate_file_response(&bytes) {
                            Ok(true) => {
                                frame.revision = Some(receipt.sha256.clone());
                                let binding = match binding_bytes(&frame, &receipt) {
                                    Ok(binding) => binding,
                                    Err(error) => {
                                        let _ = tokio::fs::remove_file(&destination).await;
                                        return Err(error);
                                    }
                                };
                                let Some(combined_bytes) =
                                    receipt.size_bytes.checked_add(binding.len() as u64)
                                else {
                                    let _ = tokio::fs::remove_file(&destination).await;
                                    return Err(CoreError::ResourceLimit(
                                        "RDCAP frame size overflow".into(),
                                    ));
                                };
                                let frame_limit = context
                                    .limits
                                    .max_frame_bytes
                                    .min(context.limits.max_temp_bytes)
                                    .min(context.limits.max_artifact_bytes.saturating_mul(2));
                                if combined_bytes > frame_limit
                                    || binding.len() as u64 > context.limits.max_artifact_bytes
                                {
                                    let _ = tokio::fs::remove_file(&destination).await;
                                    return Err(CoreError::ResourceLimit(
                                        "RDCAP raw frame exceeds configured limits".into(),
                                    ));
                                }
                                let binding_path = match write_temp_artifact(&temp_root, &binding) {
                                    Ok(path) => path,
                                    Err(error) => {
                                        let _ = tokio::fs::remove_file(&destination).await;
                                        return Err(error);
                                    }
                                };
                                let binding_digest = hex::encode(Sha256::digest(&binding));
                                let path = match tempfile::TempPath::try_from_path(&destination) {
                                    Ok(path) => path,
                                    Err(_) => {
                                        let _ = tokio::fs::remove_file(&destination).await;
                                        return Err(CoreError::Temporary(
                                            "RDCAP response could not be retained".into(),
                                        ));
                                    }
                                };
                                return Ok(RawFrame {
                                    frame,
                                    artifacts: vec![
                                        RawArtifact {
                                            receipt: ArtifactReceipt {
                                                name: "file-response.json".into(),
                                                media_type: "application/json".into(),
                                                size_bytes: receipt.size_bytes,
                                                sha256: receipt.sha256,
                                            },
                                            path,
                                        },
                                        RawArtifact {
                                            receipt: ArtifactReceipt {
                                                name: "binding.json".into(),
                                                media_type: "application/json".into(),
                                                size_bytes: binding.len() as u64,
                                                sha256: binding_digest,
                                            },
                                            path: binding_path,
                                        },
                                    ],
                                    private_locator: None,
                                });
                            }
                            Ok(false) => {
                                let _ = tokio::fs::remove_file(&destination).await;
                            }
                            Err(error) => {
                                let _ = tokio::fs::remove_file(&destination).await;
                                return Err(CoreError::Provider(error));
                            }
                        }
                    }
                    Err(error) => {
                        let _ = tokio::fs::remove_file(&destination).await;
                        let (error, retryable) = classify_ticket_error(error);
                        if !retryable {
                            return Err(error);
                        }
                    }
                }
            }

            if index_refreshes >= 2 {
                return Err(CoreError::Provider(ProviderError::TicketExhausted));
            }
            index_refreshes += 1;
            let refreshed = self.request_station_index(&context, &country, &station_code).await?;
            let Some(refreshed_frame) = refreshed.into_iter().find(|candidate| {
                candidate.locator.get("key").and_then(Value::as_str) == Some(key.as_str())
            }) else {
                return Err(CoreError::Provider(ProviderError::SelectedFrameDisappeared));
            };
            if refreshed_frame.logical_id != frame.logical_id {
                return Err(CoreError::Provider(ProviderError::AmbiguousIndex));
            }
            let (_, _, _, refreshed_ticket, _) =
                validate_frame_ticket(&refreshed_frame, &self.base_url)?;
            ticket_url = refreshed_ticket;
        }
    }

    #[cfg(test)]
    fn with_origin_for_test(origin: String) -> Self {
        Self { base_url: origin }
    }
}

impl SourceAdapter for RdcapSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn refresh_station_catalog(
        self: Arc<Self>,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Option<StationCatalogUpdate>>> {
        Box::pin(async move {
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "RDCAP catalog refresh requires network access".into(),
                ));
            }
            let referer = self.map_referer();
            let headers = [
                ("X-Requested-With", "XMLHttpRequest"),
                ("Referer", referer.as_str()),
                ("Accept", "application/json, text/plain, */*"),
            ];
            let country_payload = context
                .http_transport
                .post_form_bytes_with_headers(&self.endpoint(COUNTRY_LIST_PATH), &[], &headers)
                .await
                .map_err(sanitize_catalog_error)?;
            let countries = parse_country_list(&country_payload)?;
            let radar_payload = context
                .http_transport
                .post_form_bytes_with_headers(&self.endpoint(RADAR_LIST_PATH), &[], &headers)
                .await
                .map_err(sanitize_catalog_error)?;
            let observed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            parse_radar_directory(&radar_payload, &countries, &observed_at).map(Some)
        })
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            let station = target
                .station
                .as_deref()
                .and_then(parse_station_id)
                .ok_or(CoreError::Provider(ProviderError::UnknownStation))?;
            let (country, station_code) = (station.country, station.station_code);
            self.request_station_index(&context, country, station_code).await
        })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        context: SourceContext,
        temp_root: std::path::PathBuf,
    ) -> Option<BoxFuture<'static, CoreResult<RawFrame>>> {
        Some(Box::pin(async move { self.fetch_raw_with_ticket(frame, context, temp_root).await }))
    }
}

fn sanitize_catalog_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("RDCAP catalog refresh requires network access".into())
        }
        CoreError::ResourceLimit(_) => {
            CoreError::ResourceLimit("RDCAP catalog response exceeds configured limits".into())
        }
        CoreError::HttpStatus { status: 401 | 403, .. } => {
            CoreError::Provider(ProviderError::AccessDenied)
        }
        _ => CoreError::Provider(ProviderError::CatalogUnavailable),
    }
}

fn sanitize_index_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("RDCAP index request requires network access".into())
        }
        CoreError::ResourceLimit(_) => {
            CoreError::ResourceLimit("RDCAP index exceeds configured limits".into())
        }
        CoreError::HttpStatus { status: 401 | 403, .. } => {
            CoreError::Provider(ProviderError::AccessDenied)
        }
        CoreError::HttpStatus { status: 408, .. } => CoreError::Provider(ProviderError::Timeout),
        _ => CoreError::Provider(ProviderError::CatalogUnavailable),
    }
}

fn validate_frame_ticket(
    frame: &FrameRef,
    base_url: &str,
) -> CoreResult<(String, String, String, String, String)> {
    if frame.source != SOURCE
        || frame.product != PRODUCT
        || frame.locator_version != "rdcap-csr-v1"
        || frame.validate_identity().is_err()
    {
        return Err(CoreError::Provider(ProviderError::UnexpectedBody));
    }
    let station_id =
        frame.station.as_deref().ok_or(CoreError::Provider(ProviderError::UnexpectedBody))?;
    let station =
        parse_station_id(station_id).ok_or(CoreError::Provider(ProviderError::UnexpectedBody))?;
    let locator =
        frame.locator.as_object().ok_or(CoreError::Provider(ProviderError::UnexpectedBody))?;
    if locator
        .keys()
        .any(|key| !matches!(key.as_str(), "country" | "station_code" | "key" | "url" | "headers"))
        || locator.get("country").and_then(Value::as_str) != Some(station.country)
        || locator.get("station_code").and_then(Value::as_str) != Some(station.station_code)
    {
        return Err(CoreError::Provider(ProviderError::UnexpectedBody));
    }
    let key = normalize_epoch_key(
        locator.get("key").ok_or(CoreError::Provider(ProviderError::UnexpectedBody))?,
    )
    .map_err(CoreError::Provider)?;
    let expected_time = epoch_millis_to_utc(&key).map_err(CoreError::Provider)?;
    let actual_time = DateTime::parse_from_rfc3339(&frame.valid_time)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| CoreError::Provider(ProviderError::UnexpectedBody))?;
    if actual_time != expected_time {
        return Err(CoreError::Provider(ProviderError::UnexpectedBody));
    }
    let ticket_url = locator
        .get("url")
        .and_then(Value::as_str)
        .ok_or(CoreError::Provider(ProviderError::UnexpectedBody))?;
    validate_file_ticket(ticket_url, base_url).map_err(CoreError::Provider)?;
    let expected_referer = format!(
        "{}/data_access/radar_display/{}/{}",
        base_url.trim_end_matches('/'),
        station.country,
        station.station_code
    );
    let headers = locator
        .get("headers")
        .and_then(Value::as_object)
        .ok_or(CoreError::Provider(ProviderError::UnexpectedBody))?;
    if headers.len() != 1
        || headers.get("Referer").and_then(Value::as_str) != Some(expected_referer.as_str())
    {
        return Err(CoreError::Provider(ProviderError::UnexpectedBody));
    }
    Ok((
        station.country.into(),
        station.station_code.into(),
        key,
        ticket_url.into(),
        expected_referer,
    ))
}

fn validate_file_response(bytes: &[u8]) -> Result<bool, ProviderError> {
    if bytes.is_empty() {
        return Ok(false);
    }
    let content: String =
        serde_json::from_slice(bytes).map_err(|_| ProviderError::UnexpectedBody)?;
    Ok(!content.trim().is_empty())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RdcapBinding {
    schema_version: u8,
    source: String,
    product: String,
    station: String,
    country: String,
    station_code: String,
    key: String,
    valid_time: String,
    logical_id: String,
    content_sha256: String,
    content_size_bytes: u64,
}

pub(crate) fn binding_bytes(
    frame: &FrameRef,
    content: &crate::transport::http::HttpBodyReceipt,
) -> CoreResult<Vec<u8>> {
    let station_id =
        frame.station.as_deref().ok_or(CoreError::Provider(ProviderError::UnexpectedBody))?;
    let station =
        parse_station_id(station_id).ok_or(CoreError::Provider(ProviderError::UnexpectedBody))?;
    let key = frame
        .locator
        .get("key")
        .and_then(Value::as_str)
        .ok_or(CoreError::Provider(ProviderError::UnexpectedBody))?;
    let binding = RdcapBinding {
        schema_version: 1,
        source: frame.source.clone(),
        product: frame.product.clone(),
        station: station_id.to_owned(),
        country: station.country.to_owned(),
        station_code: station.station_code.to_owned(),
        key: key.to_owned(),
        valid_time: crate::identity::normalize_time(&frame.valid_time)
            .map_err(|_| CoreError::Provider(ProviderError::UnexpectedBody))?,
        logical_id: frame.logical_id.clone(),
        content_sha256: content.sha256.clone(),
        content_size_bytes: content.size_bytes,
    };
    serde_json::to_vec_pretty(&binding)
        .map_err(|_| CoreError::Temporary("RDCAP binding could not be serialized".into()))
}

fn write_temp_artifact(root: &std::path::Path, bytes: &[u8]) -> CoreResult<tempfile::TempPath> {
    use std::io::Write;
    let mut file = tempfile::Builder::new()
        .prefix("rdcap-binding-")
        .tempfile_in(root)
        .map_err(|_| CoreError::Temporary("RDCAP binding staging is unavailable".into()))?;
    file.write_all(bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| CoreError::Temporary("RDCAP binding staging failed".into()))?;
    Ok(file.into_temp_path())
}

pub(crate) fn validate_binding(frame: &FrameRef, artifacts: &[RawArtifact]) -> CoreResult<()> {
    if frame.source != SOURCE || frame.product != PRODUCT || frame.validate_identity().is_err() {
        return Err(CoreError::Integrity("RDCAP raw binding does not match its frame".into()));
    }
    let data = artifacts
        .iter()
        .find(|artifact| artifact.receipt.name == "file-response.json")
        .ok_or_else(|| CoreError::Integrity("RDCAP raw response is missing".into()))?;
    let binding_artifact = artifacts
        .iter()
        .find(|artifact| artifact.receipt.name == "binding.json")
        .ok_or_else(|| CoreError::Integrity("RDCAP raw binding is missing".into()))?;
    if artifacts.len() != 2 {
        return Err(CoreError::Integrity("RDCAP raw artifact set is invalid".into()));
    }
    let data_bytes = std::fs::read(&data.path)
        .map_err(|_| CoreError::Integrity("RDCAP raw response could not be read".into()))?;
    let binding_bytes = std::fs::read(&binding_artifact.path)
        .map_err(|_| CoreError::Integrity("RDCAP binding could not be read".into()))?;
    if data_bytes.len() as u64 != data.receipt.size_bytes
        || hex::encode(Sha256::digest(&data_bytes)) != data.receipt.sha256
        || !validate_file_response(&data_bytes).unwrap_or(false)
        || binding_bytes.len() as u64 != binding_artifact.receipt.size_bytes
        || hex::encode(Sha256::digest(&binding_bytes)) != binding_artifact.receipt.sha256
    {
        return Err(CoreError::Integrity("RDCAP raw receipts are invalid".into()));
    }
    let binding: RdcapBinding = serde_json::from_slice(&binding_bytes)
        .map_err(|_| CoreError::Integrity("RDCAP binding is invalid".into()))?;
    let station_id = frame.station.as_deref().unwrap_or_default();
    let station = parse_station_id(station_id)
        .ok_or_else(|| CoreError::Integrity("RDCAP binding station is invalid".into()))?;
    let key = frame.locator.get("key").and_then(Value::as_str).unwrap_or_default();
    let expected_time = crate::identity::normalize_time(&frame.valid_time)
        .map_err(|_| CoreError::Integrity("RDCAP binding time is invalid".into()))?;
    if binding.schema_version != 1
        || binding.source != SOURCE
        || binding.product != PRODUCT
        || binding.station != station_id
        || binding.country != station.country
        || binding.station_code != station.station_code
        || binding.key != key
        || binding.valid_time != expected_time
        || binding.logical_id != frame.logical_id
        || binding.content_sha256 != data.receipt.sha256
        || binding.content_size_bytes != data.receipt.size_bytes
        || frame.revision.as_deref() != Some(data.receipt.sha256.as_str())
    {
        return Err(CoreError::Integrity("RDCAP raw binding does not match its frame".into()));
    }
    Ok(())
}

fn classify_ticket_error(error: CoreError) -> (CoreError, bool) {
    match error {
        CoreError::Cancelled => (CoreError::Cancelled, false),
        CoreError::NetworkDisabled(_) => (
            CoreError::NetworkDisabled("RDCAP raw acquisition requires network access".into()),
            false,
        ),
        CoreError::ResourceLimit(_) => {
            (CoreError::ResourceLimit("RDCAP raw response exceeds configured limits".into()), false)
        }
        CoreError::HttpStatus { status: 401 | 403, .. } => {
            (CoreError::Provider(ProviderError::AccessDenied), false)
        }
        CoreError::HttpStatus { status: 408, .. } => {
            (CoreError::Provider(ProviderError::Timeout), true)
        }
        CoreError::HttpStatus { retryable, status } => {
            (CoreError::HttpStatus { status, retryable }, retryable)
        }
        CoreError::Transport(_) => (CoreError::Transport("RDCAP file request failed".into()), true),
        other => (other, false),
    }
}

fn parse_station_index(
    payload: &[u8],
    station_id: &str,
    base_url: &str,
) -> Result<Vec<FrameRef>, ProviderError> {
    let station = parse_station_id(station_id).ok_or(ProviderError::UnknownStation)?;
    let value: Value =
        serde_json::from_slice(payload).map_err(|_| ProviderError::UnexpectedBody)?;
    let entries = value
        .as_object()
        .and_then(|object| object.get("list"))
        .and_then(Value::as_array)
        .ok_or(ProviderError::UnexpectedBody)?;
    let referer = format!(
        "{}/data_access/radar_display/{}/{}",
        base_url.trim_end_matches('/'),
        station.country,
        station.station_code
    );
    let mut by_key = BTreeMap::<String, (String, FrameRef)>::new();
    let mut ticket_keys = BTreeMap::<String, String>::new();
    for entry in entries {
        let object = entry.as_object().ok_or(ProviderError::UnexpectedBody)?;
        let raw_key = object.get("key").ok_or(ProviderError::UnexpectedBody)?;
        let key = normalize_epoch_key(raw_key)?;
        let valid_time = epoch_millis_to_rfc3339(&key)?;
        let urls =
            object.get("url").and_then(Value::as_array).ok_or(ProviderError::UnexpectedBody)?;
        if urls.len() != 1 {
            return Err(ProviderError::UnexpectedBody);
        }
        let ticket_url = urls[0].as_str().ok_or(ProviderError::UnexpectedBody)?;
        validate_file_ticket(ticket_url, base_url)?;
        if ticket_keys
            .insert(ticket_url.to_owned(), key.clone())
            .is_some_and(|existing_key| existing_key != key)
        {
            return Err(ProviderError::AmbiguousIndex);
        }

        let mut stable_description = Value::Object(object.clone());
        let stable_object =
            stable_description.as_object_mut().ok_or(ProviderError::UnexpectedBody)?;
        stable_object.remove("url");
        stable_object.insert("key".into(), Value::String(key.clone()));
        let stable_description = crate::identity::canonical_json(&stable_description)
            .map_err(|_| ProviderError::UnexpectedBody)?;
        if let Some((existing_description, _)) = by_key.get(&key) {
            if existing_description != &stable_description {
                return Err(ProviderError::AmbiguousIndex);
            }
            continue;
        }
        let mut frame = FrameRef {
            source: SOURCE.into(),
            product: PRODUCT.into(),
            station: Some(station_id.into()),
            valid_time,
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "rdcap-csr-v1".into(),
            locator: json!({
                "country": station.country,
                "station_code": station.station_code,
                "key": key,
                "url": ticket_url,
                "headers": {"Referer": referer},
            }),
        };
        frame.logical_id =
            crate::identity::logical_id(&frame).map_err(|_| ProviderError::UnexpectedBody)?;
        by_key.insert(key, (stable_description, frame));
    }
    Ok(by_key.into_values().map(|(_, frame)| frame).collect())
}

fn normalize_epoch_key(value: &Value) -> Result<String, ProviderError> {
    let raw = match value {
        Value::String(value) => value.clone(),
        Value::Number(number) if number.is_i64() || number.is_u64() => number.to_string(),
        _ => return Err(ProviderError::UnexpectedBody),
    };
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ProviderError::UnexpectedBody);
    }
    let millis = raw.parse::<i64>().map_err(|_| ProviderError::UnexpectedBody)?;
    DateTime::from_timestamp_millis(millis).ok_or(ProviderError::UnexpectedBody)?;
    Ok(millis.to_string())
}

fn validate_file_ticket(ticket_url: &str, base_url: &str) -> Result<(), ProviderError> {
    let ticket = Url::parse(ticket_url).map_err(|_| ProviderError::UnexpectedBody)?;
    let expected = Url::parse(base_url).map_err(|_| ProviderError::UnexpectedBody)?;
    let query = ticket.query_pairs().collect::<Vec<_>>();
    let loopback_test_origin = expected.scheme() == "http"
        && expected
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
    if (ticket.scheme() != "https" && !(loopback_test_origin && ticket.scheme() == "http"))
        || ticket.origin() != expected.origin()
        || ticket.path() != FILE_PATH
        || !ticket.username().is_empty()
        || ticket.password().is_some()
        || ticket.fragment().is_some()
        || query.len() != 1
        || query[0].0 != "ft"
        || query[0].1.is_empty()
    {
        return Err(ProviderError::UnexpectedBody);
    }
    Ok(())
}

fn parse_country_list(payload: &[u8]) -> CoreResult<BTreeMap<String, String>> {
    let records = parse_directory_array(payload)?;
    let mut countries = BTreeMap::new();
    for item in records {
        let (Some(name), Some(code)) = (
            item.get("Country").and_then(Value::as_str),
            item.get("Country_key").and_then(Value::as_str),
        ) else {
            continue;
        };
        if COUNTRIES.contains(&code) {
            countries.insert(name.trim().to_owned(), code.to_owned());
        }
    }
    if countries.is_empty() {
        return Err(CoreError::Provider(ProviderError::UnexpectedBody));
    }
    Ok(countries)
}

fn parse_radar_directory(
    payload: &[u8],
    countries: &BTreeMap<String, String>,
    observed_at: &str,
) -> CoreResult<StationCatalogUpdate> {
    let records = parse_directory_array(payload)?;
    let record_count = records.len();
    let provenance = CatalogProvenance {
        source: "RDCAP live radar directory".into(),
        reference: Some("https://rdcap.cwa.gov.tw/data_access/get_radar_list".into()),
        observed_at: Some(observed_at.to_owned()),
    };
    let mut grouped = BTreeMap::<String, Vec<DirectoryStationRecord>>::new();
    for item in records {
        let Some(country_name) = item.get("Country").and_then(Value::as_str) else {
            continue;
        };
        let Some(country) = countries.get(country_name.trim()) else {
            continue;
        };
        let Some(code) = item.get("Short_name").and_then(Value::as_str) else {
            continue;
        };
        let Some(station_id) = normalize_catalog_station(country, code) else {
            continue;
        };
        grouped.entry(station_id).or_default().push(DirectoryStationRecord::from_value(item));
    }
    if grouped.is_empty() {
        return Err(CoreError::Provider(ProviderError::CatalogUnavailable));
    }

    let mut conflicts = Vec::new();
    let mut stations = Vec::with_capacity(grouped.len());
    for (station_id, mut rows) in grouped {
        rows.sort_by(|a, b| a.record_id.cmp(&b.record_id));
        let country = parse_station_id(&station_id)
            .map(|parsed| parsed.country.to_owned())
            .ok_or(CoreError::Provider(ProviderError::UnexpectedBody))?;
        let station_code = parse_station_id(&station_id)
            .map(|parsed| parsed.station_code.to_owned())
            .ok_or(CoreError::Provider(ProviderError::UnexpectedBody))?;
        let names = unique_values(rows.iter().filter_map(|row| row.name.as_deref()));
        let display_name = names
            .iter()
            .find(|name| !name.eq_ignore_ascii_case(&station_code))
            .or_else(|| names.first())
            .cloned()
            .unwrap_or_else(|| station_id.clone());
        let statuses = unique_values(rows.iter().filter_map(|row| row.status.as_deref()));
        let coordinate_pairs = rows.iter().filter_map(|row| row.coordinates).collect::<Vec<_>>();
        let unique_coordinates = unique_coordinates(&coordinate_pairs);
        let complete_coordinates = coordinate_pairs.len() == rows.len();
        let (longitude, latitude) = if complete_coordinates && unique_coordinates.len() == 1 {
            (Some(unique_coordinates[0].0), Some(unique_coordinates[0].1))
        } else {
            (None, None)
        };

        let mut station_conflicts = Vec::new();
        if statuses.len() > 1 {
            let conflict = CatalogDirectoryConflict {
                station_id: station_id.clone(),
                field: "Status".into(),
                values: statuses.iter().cloned().map(Value::String).collect(),
                provenance: vec![provenance.clone()],
            };
            conflicts.push(conflict.clone());
            station_conflicts.push(conflict);
        }
        if unique_coordinates.len() > 1 {
            let conflict = CatalogDirectoryConflict {
                station_id: station_id.clone(),
                field: "Longitude/Latitude".into(),
                values: unique_coordinates.iter().map(|(lon, lat)| json!([lon, lat])).collect(),
                provenance: vec![provenance.clone()],
            };
            conflicts.push(conflict.clone());
            station_conflicts.push(conflict);
        }

        let mut extensions = BTreeMap::new();
        extensions.insert("source_id".into(), json!(SOURCE));
        extensions.insert("country_name".into(), json!(country_name_for(&country)));
        extensions.insert(
            "directory_record_ids".into(),
            json!(rows.iter().map(|row| row.record_id.as_str()).collect::<Vec<_>>()),
        );
        extensions.insert("directory_statuses".into(), json!(statuses));
        extensions
            .insert("snapshot_date".into(), json!(observed_at.get(..10).unwrap_or(observed_at)));
        extensions.insert(
            "recent_query".into(),
            json!({"latest":true,"exact_at":true,"range":true,"historical":false}),
        );
        extensions.insert("historical".into(), json!(false));
        extensions.insert("scan_height".into(), json!("unknown"));
        extensions.insert("upstream_qc".into(), json!("unknown"));
        extensions.insert(
            "directory_records".into(),
            Value::Array(rows.iter().map(DirectoryStationRecord::safe_json).collect()),
        );
        let metadata = CatalogMetadata {
            country: Some(country),
            recent_query: Some(RecentQueryCapability {
                latest: true,
                exact_at: true,
                range: true,
                historical: false,
                max_age_seconds: None,
            }),
            directory_conflicts: station_conflicts,
            provenance: vec![provenance.clone()],
            country_capabilities: BTreeMap::new(),
            extensions,
        };
        stations.push(CatalogStation {
            id: station_id,
            name: display_name,
            longitude,
            latitude,
            product_ids: vec![PRODUCT.into()],
            metadata: Some(metadata),
        });
    }

    let capabilities = country_capability_defaults();
    let mut extensions = BTreeMap::new();
    extensions.insert("countries".into(), json!(COUNTRIES));
    extensions.insert("observed_at".into(), json!(observed_at));
    extensions.insert("directory_record_count".into(), json!(record_count));
    extensions.insert("unique_station_count".into(), json!(stations.len()));
    let update_metadata = CatalogMetadata {
        country: None,
        recent_query: Some(RecentQueryCapability {
            latest: true,
            exact_at: true,
            range: true,
            historical: false,
            max_age_seconds: None,
        }),
        directory_conflicts: conflicts,
        provenance: vec![provenance],
        country_capabilities: capabilities,
        extensions,
    };
    Ok(StationCatalogUpdate { source_id: SOURCE.into(), stations, metadata: Some(update_metadata) })
}

fn parse_directory_array(payload: &[u8]) -> CoreResult<Vec<Value>> {
    if payload.len() > MAX_DIRECTORY_BYTES {
        return Err(CoreError::ResourceLimit("RDCAP directory exceeds parser limit".into()));
    }
    let value: Value = serde_json::from_slice(payload)
        .map_err(|_| CoreError::Provider(ProviderError::UnexpectedBody))?;
    value.as_array().cloned().ok_or(CoreError::Provider(ProviderError::UnexpectedBody))
}

#[derive(Clone)]
struct DirectoryStationRecord {
    record_id: String,
    name: Option<String>,
    status: Option<String>,
    coordinates: Option<(f64, f64)>,
    raw: Value,
}

impl DirectoryStationRecord {
    fn from_value(raw: Value) -> Self {
        let record_id = raw
            .get("Id")
            .map(|value| match value {
                Value::String(id) => id.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        let name = raw
            .get("Name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        let status = raw
            .get("Status")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        let longitude = raw.get("Longitude").and_then(parse_number);
        let latitude = raw.get("Latitude").and_then(parse_number);
        let coordinates = longitude.zip(latitude).filter(|(lon, lat)| {
            lon.is_finite()
                && lat.is_finite()
                && (-180.0..=180.0).contains(lon)
                && (-90.0..=90.0).contains(lat)
        });
        Self { record_id, name, status, coordinates, raw }
    }

    fn safe_json(&self) -> Value {
        const FIELDS: [&str; 11] = [
            "Id",
            "Name",
            "Country",
            "Short_name",
            "Longitude",
            "Latitude",
            "Status",
            "Owner",
            "County",
            "Elevation",
            "Band",
        ];
        let mut record = serde_json::Map::new();
        for key in FIELDS {
            if let Some(value) = self.raw.get(key) {
                record.insert(key.to_owned(), value.clone());
            }
        }
        if let Some(value) = self.raw.get("Polarization") {
            record.insert("Polarization".into(), value.clone());
        }
        if let Some(value) = self.raw.get("Installation_date") {
            record.insert("Installation_date".into(), value.clone());
        }
        Value::Object(record)
    }
}

fn parse_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn unique_values<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
    values.map(str::to_owned).collect::<BTreeSet<_>>().into_iter().collect()
}

fn unique_coordinates(values: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut result = Vec::new();
    for pair in values {
        if !result.contains(pair) {
            result.push(*pair);
        }
    }
    result
}

fn country_name_for(country: &str) -> &'static str {
    match country {
        "TWN" => "Taiwan",
        "JPN" => "Japan",
        "PHL" => "Philippines",
        _ => "Unknown",
    }
}

fn country_capability_defaults() -> BTreeMap<String, CountryCapabilities> {
    COUNTRIES.into_iter().map(|country| (country.into(), CountryCapabilities::default())).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        COUNTRIES, RdcapSourceAdapter, SOURCE, epoch_millis_to_rfc3339, epoch_millis_to_utc,
        is_valid_station_selection, normalize_catalog_station, parse_country_list,
        parse_radar_directory, parse_station_id, parse_station_index, resolve_station_selection,
    };
    use crate::errors::{CoreError, ProviderError};
    use crate::limits::{Limits, RequestBudget};
    use crate::model::{FrameRef, Query};
    use crate::source::{SourceAdapter, SourceContext};
    use crate::transport::ftp::FtpTransport;
    use crate::transport::http::{HttpRequestCoalescer, HttpTransport};
    use serde_json::json;
    use serde_yaml_ng::Value as YamlValue;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;

    fn raw_test_context(max_frame_bytes: Option<u64>) -> SourceContext {
        let mut limits = Limits::default();
        if let Some(max_frame_bytes) = max_frame_bytes {
            limits.max_frame_bytes = max_frame_bytes;
        }
        let budget = Arc::new(RequestBudget::new(&limits));
        SourceContext {
            query: Query { source: Some(SOURCE.into()), ..Query::default() },
            allow_network: true,
            discovery_workers: 1,
            source_options: Arc::new(BTreeMap::<String, YamlValue>::new()),
            request_budget: budget.clone(),
            limits: limits.clone(),
            http_transport: Arc::new(
                HttpTransport::with_budget(limits.clone(), false, budget.clone()).unwrap(),
            ),
            ftp_transport: Arc::new(FtpTransport::with_budget(limits, false, budget)),
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        }
    }

    fn raw_test_frame(origin: &str, ticket: &str, key: &str) -> FrameRef {
        let mut frame = FrameRef {
            source: SOURCE.into(),
            product: "reflectivity".into(),
            station: Some("TWN/RCHL".into()),
            valid_time: epoch_millis_to_rfc3339(key).unwrap(),
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "rdcap-csr-v1".into(),
            locator: json!({
                "country": "TWN",
                "station_code": "RCHL",
                "key": key,
                "url": ticket,
                "headers": {"Referer": format!("{origin}/data_access/radar_display/TWN/RCHL")},
            }),
        };
        frame.logical_id = crate::identity::logical_id(&frame).unwrap();
        frame
    }

    async fn read_loopback_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 2048];
        loop {
            let count = stream.read(&mut chunk).await.unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find_map(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + length {
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    async fn scripted_server(replies: Vec<(u16, Vec<u8>)>) -> (String, JoinHandle<Vec<String>>) {
        scripted_server_for(move |_| replies).await
    }

    async fn scripted_server_for(
        replies_for_origin: impl FnOnce(&str) -> Vec<(u16, Vec<u8>)>,
    ) -> (String, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://{address}");
        let replies = replies_for_origin(&origin);
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, body) in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                requests.push(read_loopback_request(&mut stream).await);
                let reason = if status == 503 { "Service Unavailable" } else { "OK" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            }
            requests
        });
        (origin, task)
    }

    fn index_json(key: &str, ticket: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "header": null,
            "list": [{"key": key, "url": [ticket]}],
        }))
        .unwrap()
    }

    #[test]
    fn parses_only_canonical_country_station_ids() {
        assert_eq!(
            parse_station_id("TWN/RCHL"),
            Some(super::RdcapStationId { country: "TWN", station_code: "RCHL" })
        );
        for invalid in [
            "TWN/",
            "USA/RCHL",
            "TWN/RCHL/EXTRA",
            "TWN/../RCHL",
            "TWN/rchl",
            "TWN/RCH-L",
            "TWN%2FRCHL",
        ] {
            assert!(parse_station_id(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn normalizes_catalog_codes_without_widening_the_public_grammar() {
        assert_eq!(normalize_catalog_station(" twn ", " rchl ").as_deref(), Some("TWN/RCHL"));
        assert!(normalize_catalog_station("USA", "RCHL").is_none());
        assert!(normalize_catalog_station("TWN", "RCH-L").is_none());
        assert!(is_valid_station_selection("RCHL"));
        assert!(!is_valid_station_selection("../RCHL"));
    }

    #[test]
    fn short_station_resolution_requires_a_unique_country_match() {
        let ids = vec!["TWN/RCHL".into(), "JPN/RCHL".into(), "PHL/SUBI".into()];
        assert_eq!(resolve_station_selection("SUBI", &ids).unwrap(), "PHL/SUBI");
        assert_eq!(
            resolve_station_selection("RCHL", &ids),
            Err(crate::errors::ProviderError::AmbiguousIndex)
        );
        assert_eq!(
            resolve_station_selection("NOPE", &ids),
            Err(crate::errors::ProviderError::UnknownStation)
        );
        assert_eq!(
            resolve_station_selection("USA/SUBI", &ids),
            Err(crate::errors::ProviderError::UnknownStation)
        );
    }

    #[test]
    fn epoch_milliseconds_preserve_real_subminute_time_precision() {
        assert_eq!(
            epoch_millis_to_rfc3339("1790834708000").unwrap(),
            "2026-10-01T06:05:08.000000Z"
        );
        assert_eq!(
            epoch_millis_to_rfc3339("1790834268000").unwrap(),
            "2026-10-01T05:57:48.000000Z"
        );
        assert!(epoch_millis_to_utc("not-a-key").is_err());
        assert!(epoch_millis_to_utc("179083470800000000000").is_err());
    }

    #[test]
    fn live_directory_parser_merges_duplicates_and_preserves_conflicts_safely() {
        const SAMPLE_DIRECTORY: &str =
            include_str!("../../../../validation-results/rdcap-analysis/catalog.json");
        let country_payload = br#"[
            {"Country":"Taiwan","Country_key":"TWN"},
            {"Country":"Japan","Country_key":"JPN"},
            {"Country":"Philippines","Country_key":"PHL"},
            {"Country":"Asia","Country_key":"ASIA"}
        ]"#;
        let countries = parse_country_list(country_payload).unwrap();
        let update = parse_radar_directory(
            SAMPLE_DIRECTORY.as_bytes(),
            &countries,
            "2026-10-01T10:00:00.000Z",
        )
        .unwrap();
        assert_eq!(update.source_id, SOURCE);
        assert_eq!(update.stations.len(), 48);
        let bale = update.stations.iter().find(|station| station.id == "PHL/BALE").unwrap();
        let metadata = bale.metadata.as_ref().unwrap();
        assert_eq!(metadata.extensions["directory_record_ids"], json!(["3004", "5031"]));
        assert_eq!(metadata.extensions["directory_statuses"], json!(["Active", "Inactive"]));
        assert!(
            metadata.directory_conflicts.iter().any(|conflict| {
                conflict.station_id == "PHL/BALE" && conflict.field == "Status"
            })
        );
        assert_eq!(bale.longitude, Some(121.6331));
        assert_eq!(bale.latitude, Some(15.7502));
        let serialized = serde_json::to_string(&update).unwrap();
        assert!(!serialized.contains("ticket"));
        assert!(!serialized.contains("ft="));
        assert_eq!(update.metadata.as_ref().unwrap().country_capabilities.len(), COUNTRIES.len());
    }

    #[test]
    fn live_directory_keeps_unpublished_coordinates_unknown() {
        let countries =
            parse_country_list(br#"[{"Country":"Taiwan","Country_key":"TWN"}]"#).unwrap();
        let update = parse_radar_directory(
            br#"[{"Id":"9999","Name":"Unknown","Short_name":"UNKN","Country":"Taiwan","Status":"Active"}]"#,
            &countries,
            "2026-10-01T10:00:00.000Z",
        )
        .unwrap();
        assert_eq!(update.stations.len(), 1);
        assert_eq!(update.stations[0].longitude, None);
        assert_eq!(update.stations[0].latitude, None);
    }

    #[test]
    fn timeline_parser_checks_epoch_ms_and_keeps_tickets_out_of_stable_identity() {
        let make_index = |ticket: &str| {
            serde_json::to_vec(&json!({
                "header": "https://rdcap.cwa.gov.tw/file?ft=unused-header-ticket",
                "list": [{"key": "1790834708000", "url": [ticket]}],
            }))
            .unwrap()
        };
        let first = parse_station_index(
            &make_index("https://rdcap.cwa.gov.tw/file?ft=private-ticket-one"),
            "TWN/RCHL",
            "https://rdcap.cwa.gov.tw",
        )
        .unwrap();
        let second = parse_station_index(
            &make_index("https://rdcap.cwa.gov.tw/file?ft=private-ticket-two"),
            "TWN/RCHL",
            "https://rdcap.cwa.gov.tw",
        )
        .unwrap();
        assert_eq!(first[0].valid_time, "2026-10-01T06:05:08.000000Z");
        assert_eq!(first[0].locator_version, "rdcap-csr-v1");
        assert_eq!(first[0].logical_id, second[0].logical_id);
        let identity = crate::identity::frame_identity(&first[0]).unwrap();
        assert_eq!(
            identity["locator"],
            json!({
                "country":"TWN",
                "station_code":"RCHL",
                "key":"1790834708000",
            })
        );
        let public_frame = serde_json::to_string(&first[0]).unwrap();
        assert!(!public_frame.contains("private-ticket"));
        assert!(!serde_json::to_string(&identity).unwrap().contains("unused-header-ticket"));
    }

    #[test]
    fn timeline_parser_deduplicates_ticket_changes_and_rejects_stable_conflicts() {
        let same_key = serde_json::to_vec(&json!({
            "list": [
                {"key": 1790834708000_u64, "url": ["https://rdcap.cwa.gov.tw/file?ft=first"]},
                {"key": "1790834708000", "url": ["https://rdcap.cwa.gov.tw/file?ft=second"]},
            ]
        }))
        .unwrap();
        let frames =
            parse_station_index(&same_key, "TWN/RCHL", "https://rdcap.cwa.gov.tw").unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].locator["url"], "https://rdcap.cwa.gov.tw/file?ft=first");

        let conflict = serde_json::to_vec(&json!({
            "list": [
                {"key": "1790834708000", "url": ["https://rdcap.cwa.gov.tw/file?ft=one"], "quality": "A"},
                {"key": "1790834708000", "url": ["https://rdcap.cwa.gov.tw/file?ft=two"], "quality": "B"},
            ]
        }))
        .unwrap();
        assert_eq!(
            parse_station_index(&conflict, "TWN/RCHL", "https://rdcap.cwa.gov.tw"),
            Err(crate::errors::ProviderError::AmbiguousIndex)
        );
    }

    #[test]
    fn timeline_parser_rejects_unknown_ticket_shapes_html_and_reused_tickets() {
        for payload in [
            br#"<html>denied</html>"#.as_slice(),
            br#"{"list":null}"#.as_slice(),
            br#"{"list":[{"key":1790834708000,"url":[]}]}"#.as_slice(),
            br#"{"list":[{"key":1790834708000,"url":["https://other.invalid/file?ft=x"]}]}"#.as_slice(),
            br#"{"list":[{"key":1790834708000,"url":["https://rdcap.cwa.gov.tw/file?ft=x&next=y"]}]}"#.as_slice(),
            br#"{"list":[{"key":1.5,"url":["https://rdcap.cwa.gov.tw/file?ft=x"]}]}"#.as_slice(),
        ] {
            assert_eq!(
                parse_station_index(payload, "TWN/RCHL", "https://rdcap.cwa.gov.tw"),
                Err(crate::errors::ProviderError::UnexpectedBody)
            );
        }

        let reused_ticket = serde_json::to_vec(&json!({
            "list": [
                {"key": "1790834708000", "url": ["https://rdcap.cwa.gov.tw/file?ft=reused"]},
                {"key": "1790835072000", "url": ["https://rdcap.cwa.gov.tw/file?ft=reused"]},
            ]
        }))
        .unwrap();
        assert_eq!(
            parse_station_index(&reused_ticket, "TWN/RCHL", "https://rdcap.cwa.gov.tw"),
            Err(crate::errors::ProviderError::AmbiguousIndex)
        );
    }

    #[test]
    fn timeline_parser_checks_ticket_ownership_even_for_duplicate_keys() {
        let entries = [
            json!({"key": "1790834708000", "url": ["https://rdcap.cwa.gov.tw/file?ft=first"]}),
            json!({"key": "1790834708000", "url": ["https://rdcap.cwa.gov.tw/file?ft=second"]}),
            json!({"key": "1790835072000", "url": ["https://rdcap.cwa.gov.tw/file?ft=second"]}),
        ];
        for order in [[0, 1, 2], [2, 0, 1], [1, 2, 0]] {
            let payload = serde_json::to_vec(&json!({
                "list": order.map(|index| entries[index].clone()),
            }))
            .unwrap();
            assert_eq!(
                parse_station_index(&payload, "TWN/RCHL", "https://rdcap.cwa.gov.tw"),
                Err(ProviderError::AmbiguousIndex)
            );
        }
    }

    #[tokio::test]
    async fn raw_fetch_uses_one_get_and_preserves_the_exact_json_envelope_bytes() {
        let payload = serde_json::to_vec(&"901,901,T,117.12,19.49,126.12,28.49,int16").unwrap();
        let (origin, server) = scripted_server(vec![(200, payload.clone())]).await;
        let context = raw_test_context(None);
        let adapter = Arc::new(RdcapSourceAdapter::with_origin_for_test(origin.clone()));
        let temp = tempfile::tempdir().unwrap();
        let frame = raw_test_frame(
            &origin,
            &format!("{origin}/file?ft=single-use-ticket"),
            "1790834708000",
        );

        let raw =
            adapter.fetch_raw(frame, context, temp.path().to_path_buf()).unwrap().await.unwrap();
        let requests = server.await.unwrap();

        assert_eq!(requests.len(), 1, "no HEAD/header GET or duplicate ticket GET");
        assert!(requests[0].starts_with("GET /file?ft=single-use-ticket HTTP/1.1\r\n"));
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains(&format!("referer: {origin}/data_access/radar_display/twn/rchl\r\n"))
        );
        assert_eq!(raw.artifacts.len(), 2);
        assert_eq!(raw.artifacts[0].receipt.name, "file-response.json");
        assert_eq!(std::fs::read(&raw.artifacts[0].path).unwrap(), payload);
        assert_eq!(raw.frame.revision.as_deref(), Some(raw.artifacts[0].receipt.sha256.as_str()));
        assert_eq!(raw.artifacts[1].receipt.name, "binding.json");
        let binding_bytes = std::fs::read(&raw.artifacts[1].path).unwrap();
        assert!(!String::from_utf8_lossy(&binding_bytes).contains("single-use-ticket"));
        assert_eq!(
            binding_bytes,
            super::binding_bytes(
                &raw.frame,
                &crate::transport::http::HttpBodyReceipt {
                    size_bytes: raw.artifacts[0].receipt.size_bytes,
                    sha256: raw.artifacts[0].receipt.sha256.clone(),
                },
            )
            .unwrap()
        );
        super::validate_binding(&raw.frame, &raw.artifacts).unwrap();
        let path = raw.artifacts[0].path.to_path_buf();
        drop(raw);
        assert!(!path.exists(), "dropping the raw frame removes temporary response bytes");
    }

    #[tokio::test]
    async fn empty_file_content_refreshes_the_same_key_and_uses_a_new_ticket_once() {
        let key = "1790834708000";
        let (origin, server) = scripted_server_for(|origin| {
            vec![
                (200, b"\"\"".to_vec()),
                (200, index_json(key, &format!("{origin}/file?ft=second-ticket"))),
                (200, serde_json::to_vec(&"valid csr payload").unwrap()),
            ]
        })
        .await;
        let ticket_one = format!("{origin}/file?ft=first-ticket");
        let context = raw_test_context(None);
        let adapter = Arc::new(RdcapSourceAdapter::with_origin_for_test(origin.clone()));
        let temp = tempfile::tempdir().unwrap();
        let frame = raw_test_frame(&origin, &ticket_one, key);

        let raw =
            adapter.fetch_raw(frame, context, temp.path().to_path_buf()).unwrap().await.unwrap();
        let requests = server.await.unwrap();

        assert_eq!(requests.len(), 3);
        assert!(requests[0].starts_with("GET /file?ft=first-ticket HTTP/1.1\r\n"));
        assert!(requests[1].starts_with("POST /data_access/get_radar_data HTTP/1.1\r\n"));
        assert!(requests[1].ends_with("\r\n\r\ncountry=TWN&radar_name=RCHL&datetime="));
        assert!(requests[2].starts_with("GET /file?ft=second-ticket HTTP/1.1\r\n"));
        assert_eq!(raw.frame.valid_time, "2026-10-01T06:05:08.000000Z");
        assert_eq!(
            raw.artifacts[0].receipt.size_bytes,
            raw.artifacts[0].path.metadata().unwrap().len()
        );
        assert_eq!(
            std::fs::read(&raw.artifacts[0].path).unwrap(),
            serde_json::to_vec(&"valid csr payload").unwrap()
        );
    }

    #[tokio::test]
    async fn retry_stops_when_the_selected_key_disappears() {
        let key = "1790834708000";
        let other_key = "1790834709000";
        let (origin, server) = scripted_server_for(|origin| {
            vec![
                (503, Vec::new()),
                (200, index_json(other_key, &format!("{origin}/file?ft=other-key"))),
            ]
        })
        .await;
        let context = raw_test_context(None);
        let adapter = Arc::new(RdcapSourceAdapter::with_origin_for_test(origin.clone()));
        let temp = tempfile::tempdir().unwrap();
        let frame = raw_test_frame(&origin, &format!("{origin}/file?ft=first"), key);

        let error = adapter
            .fetch_raw(frame, context, temp.path().to_path_buf())
            .unwrap()
            .await
            .unwrap_err();
        let requests = server.await.unwrap();

        assert!(matches!(error, CoreError::Provider(ProviderError::SelectedFrameDisappeared)));
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /file?ft=first HTTP/1.1\r\n"));
        assert!(requests[1].starts_with("POST /data_access/get_radar_data HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn refreshed_old_ticket_is_never_requested_twice() {
        let key = "1790834708000";
        let (origin, server) = scripted_server_for(|origin| {
            let old_ticket = format!("{origin}/file?ft=old-ticket");
            let new_ticket = format!("{origin}/file?ft=new-ticket");
            vec![
                (503, Vec::new()),
                (200, index_json(key, &old_ticket)),
                (200, index_json(key, &new_ticket)),
                (200, serde_json::to_vec(&"fresh payload").unwrap()),
            ]
        })
        .await;
        let context = raw_test_context(None);
        let adapter = Arc::new(RdcapSourceAdapter::with_origin_for_test(origin.clone()));
        let temp = tempfile::tempdir().unwrap();
        let frame = raw_test_frame(&origin, &format!("{origin}/file?ft=old-ticket"), key);

        let raw =
            adapter.fetch_raw(frame, context, temp.path().to_path_buf()).unwrap().await.unwrap();
        let requests = server.await.unwrap();

        assert_eq!(requests.len(), 4);
        assert_eq!(requests.iter().filter(|request| request.starts_with("GET ")).count(), 2);
        assert!(requests[0].starts_with("GET /file?ft=old-ticket HTTP/1.1\r\n"));
        assert!(requests[1].starts_with("POST /data_access/get_radar_data HTTP/1.1\r\n"));
        assert!(requests[2].starts_with("POST /data_access/get_radar_data HTTP/1.1\r\n"));
        assert!(requests[3].starts_with("GET /file?ft=new-ticket HTTP/1.1\r\n"));
        assert_eq!(
            std::fs::read(&raw.artifacts[0].path).unwrap(),
            serde_json::to_vec(&"fresh payload").unwrap()
        );
    }

    #[tokio::test]
    async fn access_denial_is_not_retried_or_refreshed() {
        let key = "1790834708000";
        let (origin, server) = scripted_server(vec![(403, Vec::new())]).await;
        let context = raw_test_context(None);
        let adapter = Arc::new(RdcapSourceAdapter::with_origin_for_test(origin.clone()));
        let temp = tempfile::tempdir().unwrap();
        let frame = raw_test_frame(&origin, &format!("{origin}/file?ft=forbidden"), key);

        let error = adapter
            .fetch_raw(frame, context, temp.path().to_path_buf())
            .unwrap()
            .await
            .unwrap_err();
        let requests = server.await.unwrap();

        assert!(matches!(error, CoreError::Provider(ProviderError::AccessDenied)));
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /file?ft=forbidden HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn raw_fetch_caps_attempts_at_three_file_gets_and_two_index_refreshes() {
        let key = "1790834708000";
        let (origin, server) = scripted_server_for(|origin| {
            vec![
                (503, Vec::new()),
                (200, index_json(key, &format!("{origin}/file?ft=second"))),
                (503, Vec::new()),
                (200, index_json(key, &format!("{origin}/file?ft=third"))),
                (503, Vec::new()),
            ]
        })
        .await;
        let context = raw_test_context(None);
        let adapter = Arc::new(RdcapSourceAdapter::with_origin_for_test(origin.clone()));
        let temp = tempfile::tempdir().unwrap();
        let frame = raw_test_frame(&origin, &format!("{origin}/file?ft=first"), key);

        let error = adapter
            .fetch_raw(frame, context, temp.path().to_path_buf())
            .unwrap()
            .await
            .unwrap_err();
        let requests = server.await.unwrap();

        assert!(matches!(error, CoreError::Provider(ProviderError::TicketExhausted)));
        assert_eq!(requests.len(), 5);
        assert_eq!(requests.iter().filter(|request| request.starts_with("GET ")).count(), 3);
        assert_eq!(requests.iter().filter(|request| request.starts_with("POST ")).count(), 2);
    }

    #[tokio::test]
    async fn html_and_oversized_responses_fail_without_index_refresh() {
        let key = "1790834708000";
        let (origin, server) = scripted_server(vec![(200, b"<html>denied</html>".to_vec())]).await;
        let context = raw_test_context(None);
        let adapter = Arc::new(RdcapSourceAdapter::with_origin_for_test(origin.clone()));
        let temp = tempfile::tempdir().unwrap();
        let frame = raw_test_frame(&origin, &format!("{origin}/file?ft=html"), key);
        let error = adapter
            .fetch_raw(frame, context, temp.path().to_path_buf())
            .unwrap()
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::Provider(ProviderError::UnexpectedBody)));
        assert_eq!(server.await.unwrap().len(), 1);

        let (origin, server) =
            scripted_server(vec![(200, serde_json::to_vec(&"too long").unwrap())]).await;
        let context = raw_test_context(Some(4));
        let adapter = Arc::new(RdcapSourceAdapter::with_origin_for_test(origin.clone()));
        let frame = raw_test_frame(&origin, &format!("{origin}/file?ft=large"), key);
        let error = adapter
            .fetch_raw(frame, context, temp.path().to_path_buf())
            .unwrap()
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::ResourceLimit(_)));
        assert_eq!(server.await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancellation_during_file_get_removes_staging_and_stops_without_refresh() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://{address}");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_loopback_request(&mut stream).await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            let body = serde_json::to_vec(&"late response").unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(&body).await;
            request
        });
        let context = raw_test_context(None);
        let cancellation = context.request_budget.cancellation.clone();
        let cancel_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancellation.cancel();
        });
        let adapter = Arc::new(RdcapSourceAdapter::with_origin_for_test(origin.clone()));
        let temp = tempfile::tempdir().unwrap();
        let frame = raw_test_frame(&origin, &format!("{origin}/file?ft=cancel"), "1790834708000");

        let error = adapter
            .fetch_raw(frame, context, temp.path().to_path_buf())
            .unwrap()
            .await
            .unwrap_err();
        cancel_task.await.unwrap();
        let request = server.await.unwrap();

        assert!(matches!(error, CoreError::Cancelled));
        assert!(request.starts_with("GET /file?ft=cancel HTTP/1.1\r\n"));
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn catalog_refresh_posts_ajax_forms_with_shared_transport_headers() {
        use crate::limits::{Limits, RequestBudget};
        use crate::model::Query;
        use crate::source::SourceContext;
        use crate::transport::ftp::FtpTransport;
        use crate::transport::http::{HttpRequestCoalescer, HttpTransport};
        use serde_yaml_ng::Value as YamlValue;
        use std::collections::BTreeMap;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};

        async fn request_text(stream: &mut TcpStream) -> String {
            let mut bytes = Vec::new();
            let mut chunk = [0_u8; 2048];
            loop {
                let count = stream.read(&mut chunk).await.unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&chunk[..count]);
                let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let length = headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find_map(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= header_end + 4 + length {
                    break;
                }
            }
            String::from_utf8(bytes).unwrap()
        }

        async fn reply(stream: &mut TcpStream, body: &[u8]) {
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let country_response = br#"[
            {"Country":"Taiwan","Country_key":"TWN"},
            {"Country":"Japan","Country_key":"JPN"},
            {"Country":"Philippines","Country_key":"PHL"}
        ]"#;
        let radar_response = br#"[
            {"Id":"1003","Country":"Taiwan","Short_name":"rchl","Name":"Hua-Lien","Longitude":"121.620079","Latitude":"23.990305","Status":"Active","ft":"discard-me"},
            {"Id":"3004","Country":"Philippines","Short_name":"BALE","Name":"Baler","Longitude":"121.6331","Latitude":"15.7502","Status":"Inactive"},
            {"Id":"5031","Country":"Philippines","Short_name":"BALE","Name":"BALE","Longitude":"121.6331","Latitude":"15.7502","Status":"Active"},
            {"Id":"x","Country":"Asia","Short_name":"ASIA","Name":"Asia","Longitude":"0","Latitude":"0","Status":"Active"}
        ]"#;
        let server = tokio::spawn(async move {
            let (mut country, _) = listener.accept().await.unwrap();
            let first = request_text(&mut country).await;
            let lower = first.to_ascii_lowercase();
            assert!(first.starts_with("POST /data_access/get_country_list HTTP/1.1\r\n"));
            assert!(lower.contains("x-requested-with: xmlhttprequest\r\n"));
            assert!(
                lower.contains(&format!("referer: http://{address}/data_access/radar_map\r\n"))
            );
            assert!(lower.contains("content-type: application/x-www-form-urlencoded\r\n"));
            assert!(first.ends_with("\r\n\r\n"));
            reply(&mut country, country_response).await;

            let (mut radar, _) = listener.accept().await.unwrap();
            let second = request_text(&mut radar).await;
            let lower = second.to_ascii_lowercase();
            assert!(second.starts_with("POST /data_access/get_radar_list HTTP/1.1\r\n"));
            assert!(lower.contains("x-requested-with: xmlhttprequest\r\n"));
            assert!(
                lower.contains(&format!("referer: http://{address}/data_access/radar_map\r\n"))
            );
            assert!(second.ends_with("\r\n\r\n"));
            reply(&mut radar, radar_response).await;
        });

        let limits = Limits::default();
        let budget = Arc::new(RequestBudget::new(&limits));
        let context = SourceContext {
            query: Query { source: Some(SOURCE.into()), ..Query::default() },
            allow_network: true,
            discovery_workers: 1,
            source_options: Arc::new(BTreeMap::<String, YamlValue>::new()),
            request_budget: budget.clone(),
            limits: limits.clone(),
            http_transport: Arc::new(
                HttpTransport::with_budget(limits.clone(), false, budget.clone()).unwrap(),
            ),
            ftp_transport: Arc::new(FtpTransport::with_budget(limits, false, budget)),
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        };
        let adapter =
            Arc::new(RdcapSourceAdapter::with_origin_for_test(format!("http://{address}")));
        let update = adapter.refresh_station_catalog(context).await.unwrap().unwrap();
        server.await.unwrap();
        assert_eq!(update.stations.len(), 2);
        let bale = update.stations.iter().find(|station| station.id == "PHL/BALE").unwrap();
        assert_eq!(bale.metadata.as_ref().unwrap().directory_conflicts[0].field, "Status");
        let serialized = serde_json::to_string(&update).unwrap();
        assert!(!serialized.contains("discard-me"));
    }

    #[tokio::test]
    async fn station_discovery_posts_the_empty_datetime_form_and_returns_ticket_private_frame() {
        use crate::limits::{Limits, RequestBudget};
        use crate::model::{DiscoveryTarget, Query};
        use crate::source::SourceContext;
        use crate::transport::ftp::FtpTransport;
        use crate::transport::http::{HttpRequestCoalescer, HttpTransport};
        use serde_yaml_ng::Value as YamlValue;
        use std::collections::BTreeMap;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let index = format!(
            r#"{{"header":"http://{address}/file?ft=unused-header","list":[{{"key":1790834708000,"url":["http://{address}/file?ft=private-ticket"]}}]}}"#
        );
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0_u8; 2048];
            loop {
                let count = stream.read(&mut chunk).await.unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&chunk[..count]);
                let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let length = headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find_map(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= header_end + 4 + length {
                    break;
                }
            }
            let request = String::from_utf8(bytes).unwrap();
            let lower = request.to_ascii_lowercase();
            assert!(request.starts_with("POST /data_access/get_radar_data HTTP/1.1\r\n"));
            assert!(lower.contains("x-requested-with: xmlhttprequest\r\n"));
            assert!(lower.contains(&format!(
                "referer: http://{address}/data_access/radar_display/twn/rchl\r\n"
            )));
            assert!(lower.contains("content-type: application/x-www-form-urlencoded\r\n"));
            assert!(request.ends_with("\r\n\r\ncountry=TWN&radar_name=RCHL&datetime="));
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                index.len(),
                index
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        let limits = Limits::default();
        let budget = Arc::new(RequestBudget::new(&limits));
        let context = SourceContext {
            query: Query { source: Some(SOURCE.into()), ..Query::default() },
            allow_network: true,
            discovery_workers: 1,
            source_options: Arc::new(BTreeMap::<String, YamlValue>::new()),
            request_budget: budget.clone(),
            limits: limits.clone(),
            http_transport: Arc::new(
                HttpTransport::with_budget(limits.clone(), false, budget.clone()).unwrap(),
            ),
            ftp_transport: Arc::new(FtpTransport::with_budget(limits, false, budget)),
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        };
        let adapter =
            Arc::new(RdcapSourceAdapter::with_origin_for_test(format!("http://{address}")));
        let frames = adapter
            .discover(
                DiscoveryTarget {
                    source: SOURCE.into(),
                    product: Some("reflectivity".into()),
                    station: Some("TWN/RCHL".into()),
                },
                context,
            )
            .await
            .unwrap();
        server.await.unwrap();

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].valid_time, "2026-10-01T06:05:08.000000Z");
        assert_eq!(frames[0].locator["key"], "1790834708000");
        assert_eq!(frames[0].locator["url"], format!("http://{address}/file?ft=private-ticket"));
        assert!(!serde_json::to_string(&frames[0]).unwrap().contains("private-ticket"));
        let identity = crate::identity::frame_identity(&frames[0]).unwrap();
        assert!(!serde_json::to_string(&identity).unwrap().contains("private-ticket"));
    }
}
