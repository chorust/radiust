//! Native raw-source adapter for the Australian Bureau of Meteorology radar FTP.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::limits::Limits;
use crate::model::{
    ArtifactReceipt, DiscoveryTarget, FrameRef, Query, RawArtifact, RawFrame, TimeSelector,
    parse_utc_time,
};
use crate::source::{SourceAdapter, SourceContext};
#[cfg(test)]
use crate::transport::ftp::FtpObject;
use crate::transport::ftp::FtpReceipt;
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use serde_json::json;
#[cfg(test)]
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
#[cfg(test)]
use std::io::Write;
#[cfg(test)]
use std::path::Path;
use std::sync::Arc;
#[cfg(test)]
use tempfile::NamedTempFile;
use url::Url;

const SOURCE: &str = "au";
const PRODUCT: &str = "composite";
const FTP_HOST: &str = "ftp.bom.gov.au";
const FTP_ROOT: &str = "ftp://ftp.bom.gov.au/anon/gen/radar/";
const FTP_ROOT_PATH: &str = "/anon/gen/radar/";
const LOCATOR_VERSION: &str = "au-legacy-v1";
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Discovers BoM's IDR composite frames and preserves the original PNG bytes.
pub struct AuSourceAdapter;

impl SourceAdapter for AuSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(FTP_HOST)
    }

    fn allows_artifact_url(&self, frame: &FrameRef, url: &Url) -> bool {
        frame_url_entry(frame, url).is_some()
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
                    "source au discovery requires network access".into(),
                ));
            }
            if context.request_budget.cancellation.is_cancelled() {
                return Err(CoreError::Cancelled);
            }

            let listing = tokio::select! {
                biased;
                _ = context.request_budget.cancellation.cancelled() => {
                    return Err(CoreError::Cancelled);
                }
                result = context.ftp_transport.names(FTP_ROOT, "anonymous", "anonymous") => {
                    result.map_err(sanitize_ftp_error)?
                }
            };
            let listed_bytes =
                listing.iter().try_fold(0_u64, |total, item| total.checked_add(item.len() as u64));
            if listed_bytes.is_none_or(|total| total > context.limits.max_artifact_bytes) {
                return Err(CoreError::ResourceLimit(
                    "source au FTP listing exceeds configured limits".into(),
                ));
            }
            parse_listing(&target, &context.query, &listing)
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
                    "source au acquisition requires network access".into(),
                ));
            }
            let url = frame
                .locator
                .get("url")
                .and_then(serde_json::Value::as_str)
                .and_then(|url| Url::parse(url).ok())
                .ok_or_else(invalid_frame_locator)?;
            let entry = frame_url_entry(&frame, &url).ok_or_else(invalid_frame_locator)?;
            if context.request_budget.cancellation.is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            tokio::fs::create_dir_all(&temp_root).await.map_err(|_| {
                CoreError::Temporary("source au temporary directory could not be prepared".into())
            })?;
            let destination = temp_root.join(format!("{}.png", uuid::Uuid::new_v4()));
            let receipt = tokio::select! {
                biased;
                _ = context.request_budget.cancellation.cancelled() => {
                    return Err(CoreError::Cancelled);
                }
                result = context.ftp_transport.get_to_path_limited(
                    url.as_str(),
                    "anonymous",
                    "anonymous",
                    &destination,
                    context.limits.max_frame_bytes.min(context.limits.max_temp_bytes),
                ) => {
                    result.map_err(sanitize_ftp_error)?
                }
            };
            raw_frame_from_path(frame, entry, receipt, destination, &context.limits)
        }))
    }
}

#[derive(Clone, Debug)]
struct Entry {
    filename: String,
    station: String,
    valid_time: DateTime<Utc>,
    revision: String,
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    let query_selects_au =
        query.source.as_deref().is_none_or(|source| source == SOURCE || source == "all")
            && (query.sources.is_empty() || query.sources.iter().any(|source| source == SOURCE));
    if target.source != SOURCE || !query_selects_au {
        return Err(CoreError::Transport("source au received an invalid target".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport("source au only supports composite".into()));
    }
    if target.station.as_deref().is_some_and(|station| !is_station_id(station))
        || query.stations.iter().any(|station| !is_station_id(station))
    {
        return Err(CoreError::Transport("source au requires AUdd station identifiers".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source au does not expose base times".into()));
    }
    Ok(())
}

fn is_station_id(station: &str) -> bool {
    station.len() == 4
        && station.starts_with("AU")
        && station.as_bytes()[2..].iter().all(u8::is_ascii_digit)
}

/// Parse the legacy IDRdd1 filename family; filename timestamps are UTC.
fn parse_filename(filename: &str) -> Option<Entry> {
    let bytes = filename.as_bytes();
    if !filename.is_ascii()
        || bytes.len() != 25
        || &bytes[..3] != b"IDR"
        || !bytes[3..5].iter().all(u8::is_ascii_digit)
        || &bytes[5..9] != b"1.T."
        || !bytes[9..21].iter().all(u8::is_ascii_digit)
        || &bytes[21..] != b".png"
    {
        return None;
    }
    let timestamp = &filename[9..21];
    let valid_time = NaiveDateTime::parse_from_str(timestamp, "%Y%m%d%H%M").ok()?.and_utc();
    let station = format!("AU{}", &filename[3..5]);
    let revision = format!("IDR{}1-{timestamp}", &filename[3..5]);
    Some(Entry { filename: filename.to_owned(), station, valid_time, revision })
}

fn parse_listing_name(line: &str) -> Option<Entry> {
    if line.contains('\\') || line.ends_with('/') {
        return None;
    }
    let filename = if let Some(filename) = line.strip_prefix(FTP_ROOT_PATH) {
        filename
    } else if let Some(filename) = line.strip_prefix("anon/gen/radar/") {
        filename
    } else if !line.contains('/') {
        line
    } else {
        return None;
    };
    if filename.contains('/') || filename.is_empty() {
        return None;
    }
    parse_filename(filename)
}

fn parse_listing(
    target: &DiscoveryTarget,
    query: &Query,
    listing: &[String],
) -> CoreResult<Vec<FrameRef>> {
    validate_target_and_query(target, query)?;
    if target.station.as_deref().is_some_and(|station| {
        !query.stations.is_empty() && !query.stations.iter().any(|requested| requested == station)
    }) {
        return Ok(Vec::new());
    }

    let mut entries = listing
        .iter()
        .filter_map(|line| parse_listing_name(line))
        .filter(|entry| {
            target.station.as_deref().is_none_or(|station| station == entry.station)
                && (query.stations.is_empty()
                    || query.stations.iter().any(|station| station == &entry.station))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.filename
            .cmp(&right.filename)
            .then_with(|| left.station.cmp(&right.station))
            .then_with(|| left.valid_time.cmp(&right.valid_time))
    });
    entries.dedup_by(|left, right| left.filename == right.filename);

    entries = match &query.selector {
        TimeSelector::Latest => {
            let mut latest = std::collections::BTreeMap::<String, DateTime<Utc>>::new();
            for entry in &entries {
                latest
                    .entry(entry.station.clone())
                    .and_modify(|time| *time = (*time).max(entry.valid_time))
                    .or_insert(entry.valid_time);
            }
            entries
                .into_iter()
                .filter(|entry| latest.get(&entry.station) == Some(&entry.valid_time))
                .collect()
        }
        TimeSelector::At { time } => {
            let requested = parse_utc_time(time).map_err(|_| {
                CoreError::Transport("source au query contains an invalid timestamp".into())
            })?;
            entries.into_iter().filter(|entry| entry.valid_time == requested).collect()
        }
        TimeSelector::Range { start, end } => {
            let start = parse_utc_time(start).map_err(|_| {
                CoreError::Transport("source au query contains an invalid timestamp".into())
            })?;
            let end = parse_utc_time(end).map_err(|_| {
                CoreError::Transport("source au query contains an invalid timestamp".into())
            })?;
            entries
                .into_iter()
                .filter(|entry| entry.valid_time >= start && entry.valid_time < end)
                .collect()
        }
    };

    let mut frames = entries.into_iter().map(frame_from_entry).collect::<CoreResult<Vec<_>>>()?;
    frames.sort_by(|left, right| {
        left.valid_time
            .cmp(&right.valid_time)
            .then_with(|| left.station.cmp(&right.station))
            .then_with(|| left.logical_id.cmp(&right.logical_id))
    });
    Ok(frames)
}

fn frame_from_entry(entry: Entry) -> CoreResult<FrameRef> {
    let mut frame = FrameRef {
        source: SOURCE.into(),
        product: PRODUCT.into(),
        station: Some(entry.station.clone()),
        valid_time: entry.valid_time.to_rfc3339_opts(SecondsFormat::Micros, true),
        base_time: None,
        logical_id: String::new(),
        revision: Some(entry.revision.clone()),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": format!("{FTP_ROOT}{}", entry.filename),
            "artifacts": [],
            "station": entry.station,
            "revision": entry.revision,
            "name": entry.filename,
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source au frame identity could not be computed".into())
    })?;
    Ok(frame)
}

fn frame_url_entry(frame: &FrameRef, url: &Url) -> Option<Entry> {
    if frame.source != SOURCE
        || frame.product != PRODUCT
        || frame.locator_version != LOCATOR_VERSION
    {
        return None;
    }
    let declared = frame.locator.get("url")?.as_str()?;
    if declared != url.as_str()
        || url.scheme() != "ftp"
        || !url.host_str()?.eq_ignore_ascii_case(FTP_HOST)
        || url.port().is_some_and(|port| port != 21)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let filename = url.path().strip_prefix(FTP_ROOT_PATH)?;
    if filename.contains('/') {
        return None;
    }
    let entry = parse_filename(filename)?;
    if frame.station.as_deref() != Some(entry.station.as_str())
        || frame.revision.as_deref() != Some(entry.revision.as_str())
        || parse_utc_time(&frame.valid_time).ok()? != entry.valid_time
        || frame.locator.get("name")?.as_str()? != entry.filename
        || frame.locator.get("station")?.as_str()? != entry.station
        || frame.locator.get("revision")?.as_str()? != entry.revision
        || logical_id(frame).ok()? != frame.logical_id
    {
        return None;
    }
    Some(entry)
}

fn invalid_frame_locator() -> CoreError {
    CoreError::Transport("source au frame locator is outside the allowed FTP subtree".into())
}

fn sanitize_ftp_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source au FTP access requires network opt-in".into())
        }
        CoreError::ResourceLimit(_) => {
            CoreError::ResourceLimit("source au FTP response exceeds configured limits".into())
        }
        CoreError::Temporary(_) => {
            CoreError::Temporary("source au temporary artifact could not be written".into())
        }
        _ => CoreError::Transport("source au FTP request failed".into()),
    }
}

#[cfg(test)]
fn raw_frame_from_object(
    frame: FrameRef,
    entry: Entry,
    object: FtpObject,
    temp_root: &Path,
    limits: &Limits,
) -> CoreResult<RawFrame> {
    let size = object.size_bytes;
    limits.validate_bytes(size, size)?;
    if size > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "source au artifact exceeds configured temporary storage limit".into(),
        ));
    }
    if size != object.bytes.len() as u64
        || hex::encode(Sha256::digest(&object.bytes)) != object.sha256
        || !object.bytes.starts_with(PNG_SIGNATURE)
    {
        return Err(CoreError::Transport("source au FTP receipt failed validation".into()));
    }

    fs::create_dir_all(temp_root).map_err(|_| {
        CoreError::Temporary("source au temporary directory could not be prepared".into())
    })?;
    let mut temporary = NamedTempFile::new_in(temp_root).map_err(|_| {
        CoreError::Temporary("source au temporary file could not be created".into())
    })?;
    temporary
        .write_all(&object.bytes)
        .map_err(|_| CoreError::Temporary("source au temporary artifact write failed".into()))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| CoreError::Temporary("source au temporary artifact sync failed".into()))?;
    if temporary
        .as_file()
        .metadata()
        .map_err(|_| CoreError::Temporary("source au temporary artifact metadata failed".into()))?
        .len()
        != size
    {
        return Err(CoreError::Transport("source au temporary artifact size mismatch".into()));
    }
    let path = temporary.into_temp_path();
    Ok(RawFrame {
        frame,
        artifacts: vec![RawArtifact {
            receipt: ArtifactReceipt {
                name: entry.filename,
                media_type: "image/png".into(),
                size_bytes: object.size_bytes,
                sha256: object.sha256,
            },
            path,
        }],
        private_locator: None,
    })
}

fn raw_frame_from_path(
    frame: FrameRef,
    entry: Entry,
    receipt: FtpReceipt,
    destination: std::path::PathBuf,
    limits: &Limits,
) -> CoreResult<RawFrame> {
    let size = receipt.size_bytes;
    limits.validate_bytes(size, size)?;
    if size > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "source au artifact exceeds configured temporary storage limit".into(),
        ));
    }
    let path = tempfile::TempPath::try_from_path(destination).map_err(|_| {
        CoreError::Temporary("source au temporary artifact could not be retained".into())
    })?;
    let mut file = fs::File::open(&path).map_err(|_| {
        CoreError::Temporary("source au temporary artifact could not be read".into())
    })?;
    let mut signature = [0; PNG_SIGNATURE.len()];
    file.read_exact(&mut signature)
        .map_err(|_| CoreError::Transport("source au FTP response is not a complete PNG".into()))?;
    if signature != *PNG_SIGNATURE {
        return Err(CoreError::Transport("source au FTP response is not a PNG".into()));
    }
    let actual_size = file
        .metadata()
        .map_err(|_| CoreError::Temporary("source au temporary artifact metadata failed".into()))?
        .len();
    if actual_size != size {
        return Err(CoreError::Transport("source au temporary artifact size mismatch".into()));
    }
    Ok(RawFrame {
        frame,
        artifacts: vec![RawArtifact {
            receipt: ArtifactReceipt {
                name: entry.filename,
                media_type: "image/png".into(),
                size_bytes: receipt.size_bytes,
                sha256: receipt.sha256,
            },
            path,
        }],
        private_locator: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::RequestBudget;
    use crate::model::TimeSelector;
    use crate::transport::ftp::FtpTransport;
    use crate::transport::http::{HttpRequestCoalescer, HttpTransport};

    const LISTING: &[&str] = &[
        "IDR011.T.202609181100.png",
        "/anon/gen/radar/IDR011.T.202609181200.png",
        "IDR021.T.202609181130.png",
        "IDR011.T.202609181300.gif",
        "IDR011.T.202602301100.png",
        "../IDR031.T.202609181100.png",
        "IDR0x1.T.202609181100.png",
        "IDR011.T.202609181100.png/",
    ];

    fn target() -> DiscoveryTarget {
        DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None }
    }

    fn query() -> Query {
        Query { source: Some(SOURCE.into()), ..Query::default() }
    }

    fn context(query: Query, allow_network: bool) -> SourceContext {
        let limits = Limits::default();
        let request_budget = Arc::new(RequestBudget::new(&limits));
        SourceContext {
            query,
            allow_network,
            discovery_workers: 1,
            source_options: Arc::new(Default::default()),
            request_budget: request_budget.clone(),
            limits: limits.clone(),
            http_transport: Arc::new(
                HttpTransport::with_budget(limits.clone(), allow_network, request_budget)
                    .expect("HTTP transport"),
            ),
            ftp_transport: Arc::new(FtpTransport::new(limits, allow_network)),
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        }
    }

    fn listing() -> Vec<String> {
        LISTING.iter().map(|line| (*line).to_owned()).collect()
    }

    #[test]
    fn parses_legacy_idr_family_with_utc_station_revision_and_identity() {
        let frames = parse_listing(&target(), &query(), &listing()).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].station.as_deref(), Some("AU02"));
        assert_eq!(frames[0].valid_time, "2026-09-18T11:30:00.000000Z");
        assert_eq!(frames[0].revision.as_deref(), Some("IDR021-202609181130"));
        assert_eq!(frames[1].station.as_deref(), Some("AU01"));
        assert_eq!(frames[1].valid_time, "2026-09-18T12:00:00.000000Z");
        assert_eq!(frames[1].locator_version, LOCATOR_VERSION);
        assert_eq!(
            frames[1].locator["url"],
            "ftp://ftp.bom.gov.au/anon/gen/radar/IDR011.T.202609181200.png"
        );
        assert_eq!(frames[1].locator["name"], "IDR011.T.202609181200.png");
        assert_eq!(frames[1].logical_id, logical_id(&frames[1]).unwrap());
    }

    #[test]
    fn product_station_and_time_selectors_match_the_legacy_query_rules() {
        let station_target = DiscoveryTarget { station: Some("AU01".into()), ..target() };
        let at = Query {
            selector: TimeSelector::At { time: "2026-09-18T14:00:00+02:00".into() },
            ..query()
        };
        let frames = parse_listing(&station_target, &at, &listing()).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].valid_time, "2026-09-18T12:00:00.000000Z");

        let range = Query {
            selector: TimeSelector::Range {
                start: "2026-09-18T11:00:00Z".into(),
                end: "2026-09-18T12:00:00Z".into(),
            },
            ..query()
        };
        let frames = parse_listing(&target(), &range, &listing()).unwrap();
        assert!(
            frames.iter().all(|frame| frame.valid_time.as_str() < "2026-09-18T12:00:00.000000Z")
        );
        assert_eq!(frames.len(), 2);

        assert!(
            parse_listing(
                &target(),
                &Query { product: Some("rain".into()), ..query() },
                &listing(),
            )
            .is_err()
        );
        assert!(
            parse_listing(
                &DiscoveryTarget { station: Some("AU1".into()), ..target() },
                &query(),
                &listing(),
            )
            .is_err()
        );
    }

    #[test]
    fn latest_selects_the_most_recent_frame_per_station() {
        let frames = parse_listing(&target(), &query(), &listing()).unwrap();
        assert_eq!(
            frames.iter().filter(|frame| frame.station.as_deref() == Some("AU01")).count(),
            1
        );
        assert_eq!(
            frames
                .iter()
                .find(|frame| frame.station.as_deref() == Some("AU01"))
                .unwrap()
                .valid_time,
            "2026-09-18T12:00:00.000000Z"
        );
    }

    #[test]
    fn ftp_url_policy_rejects_hosts_paths_schemes_credentials_and_substitution() {
        let frame = frame_from_entry(parse_filename("IDR011.T.202609181200.png").unwrap()).unwrap();
        let adapter = AuSourceAdapter;
        let allowed = Url::parse(&format!("{FTP_ROOT}IDR011.T.202609181200.png")).unwrap();
        assert!(adapter.allows_artifact_url(&frame, &allowed));

        for rejected in [
            "ftp://evil.example/anon/gen/radar/IDR011.T.202609181200.png",
            "ftp://ftp.bom.gov.au/anon/gen/radar-old/IDR011.T.202609181200.png",
            "ftp://ftp.bom.gov.au/anon/gen/radar/sub/IDR011.T.202609181200.png",
            "ftp://ftp.bom.gov.au/anon/gen/radar/../IDR011.T.202609181200.png",
            "ftps://ftp.bom.gov.au/anon/gen/radar/IDR011.T.202609181200.png",
            "ftp://user:pass@ftp.bom.gov.au/anon/gen/radar/IDR011.T.202609181200.png",
            "ftp://ftp.bom.gov.au:2121/anon/gen/radar/IDR011.T.202609181200.png",
            "ftp://ftp.bom.gov.au/anon/gen/radar/IDR021.T.202609181200.png",
        ] {
            let mut forged = frame.clone();
            forged.locator["url"] = json!(rejected);
            forged.logical_id = logical_id(&forged).unwrap();
            let url = Url::parse(rejected).unwrap();
            assert!(!adapter.allows_artifact_url(&forged, &url), "accepted {rejected}");
        }
    }

    #[tokio::test]
    async fn discovery_honors_network_opt_in_without_contacting_ftp() {
        let error = Arc::new(AuSourceAdapter)
            .discover(target(), context(query(), false))
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::NetworkDisabled(_)));
    }

    #[test]
    fn ftp_receipt_is_retained_in_a_private_temp_file_until_raw_frame_drop() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = [PNG_SIGNATURE.as_slice(), b"offline sample"].concat();
        let object = FtpObject {
            size_bytes: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(&bytes)),
            bytes: bytes.clone(),
        };
        let frame = frame_from_entry(parse_filename("IDR011.T.202609181200.png").unwrap()).unwrap();
        let raw = raw_frame_from_object(
            frame,
            parse_filename("IDR011.T.202609181200.png").unwrap(),
            object,
            directory.path(),
            &Limits::default(),
        )
        .unwrap();
        let path = raw.artifacts[0].path.to_path_buf();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(raw.artifacts[0].receipt.size_bytes, bytes.len() as u64);
        assert_eq!(raw.artifacts[0].receipt.sha256, hex::encode(Sha256::digest(&bytes)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        drop(raw);
        assert!(!path.exists());
    }

    #[test]
    fn streamed_ftp_receipt_is_retained_without_copying_and_removed_with_raw_frame() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = [PNG_SIGNATURE.as_slice(), b"streamed sample"].concat();
        let destination = directory.path().join("downloaded.png");
        fs::write(&destination, &bytes).unwrap();
        let receipt = FtpReceipt {
            size_bytes: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(&bytes)),
        };
        let raw = raw_frame_from_path(
            frame_from_entry(parse_filename("IDR011.T.202609181200.png").unwrap()).unwrap(),
            parse_filename("IDR011.T.202609181200.png").unwrap(),
            receipt,
            destination,
            &Limits::default(),
        )
        .unwrap();
        let path = raw.artifacts[0].path.to_path_buf();
        assert_eq!(raw.artifacts[0].receipt.size_bytes, bytes.len() as u64);
        assert_eq!(raw.artifacts[0].receipt.sha256, hex::encode(Sha256::digest(&bytes)));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        drop(raw);
        assert!(!path.exists());
    }
}
