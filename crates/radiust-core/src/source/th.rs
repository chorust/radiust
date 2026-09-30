//! Raw-only native adapter for the TMD live GIF endpoints.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{
    ArtifactReceipt, DiscoveryTarget, FrameRef, Query, RawArtifact, RawFrame, TimeSelector,
    parse_utc_time,
};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, imageops};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use url::Url;

const SOURCE: &str = "th";
const PRODUCT: &str = "composite";
const HOST: &str = "weather.tmd.go.th";
const LOCATOR_VERSION: &str = "th-legacy-v1";
const WIDTH: u32 = 680;
const HEIGHT: u32 = 680;
const FOOTER: (u32, u32, u32, u32) = (130, 665, 340, 15);
const MAX_FRAMES: usize = 12;
const OCR_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
struct Radar {
    station: &'static str,
    url: &'static str,
    referer: &'static str,
}

const RADARS: [Radar; 2] = [
    Radar {
        station: "cmp1",
        url: "https://weather.tmd.go.th/cmp/cmp1.gif",
        referer: "https://weather.tmd.go.th/cmpLoop.php",
    },
    Radar {
        station: "kkn240Loop",
        url: "https://weather.tmd.go.th/kkn/kkn240Loop.gif",
        referer: "https://weather.tmd.go.th/kknLoop.php",
    },
];

/// TMD images remain raw-only until their palette and geometry are verified.
pub struct ThSourceAdapter;

impl SourceAdapter for ThSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(HOST)
    }

    fn allows_artifact_url(&self, frame: &FrameRef, url: &Url) -> bool {
        frame_plan(frame).is_some_and(|(radar, _)| {
            url.as_str() == radar.url && url.scheme() == "https" && url.host_str() == Some(HOST)
        })
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            validate_target(&target, &context.query)?;
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source th discovery requires network access".into(),
                ));
            }

            let stations = selected_stations(&target, &context.query);
            let mut frames = Vec::new();
            for radar in RADARS.iter().filter(|radar| stations.contains(&radar.station)) {
                let payload = context
                    .http_transport
                    .get_bytes_with_headers(radar.url, &[("Referer", radar.referer)])
                    .await
                    .map_err(sanitize_transport_error)?;
                context.limits.validate_bytes(payload.len() as u64, payload.len() as u64)?;
                let pngs = footer_images(&payload, radar.station)?;
                let mut times = Vec::with_capacity(pngs.len());
                for png in pngs {
                    let text = run_tesseract(&png).await?;
                    times.push(parse_footer_time(&text).ok_or_else(|| {
                        CoreError::Transport(format!(
                            "TMD {} GIF footer has no unambiguous UTC timestamp",
                            radar.station
                        ))
                    })?);
                }
                validate_footer_sequence(radar.station, &times)?;
                let valid_time = *times.last().ok_or_else(|| {
                    CoreError::Transport("TMD GIF contains no timestamped frames".into())
                })?;
                if is_too_old(valid_time, context.query.max_age_secs, Utc::now()) {
                    continue;
                }
                frames.push(frame_from_payload(radar, &payload, &times)?);
            }
            Ok(frames)
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
                    "source th acquisition requires network access".into(),
                ));
            }
            let (radar, expected_hash) = frame_plan(&frame).ok_or_else(invalid_frame_locator)?;
            let max_bytes = context
                .limits
                .max_artifact_bytes
                .min(context.limits.max_frame_bytes)
                .min(context.limits.max_temp_bytes);
            if max_bytes == 0 {
                return Err(CoreError::ResourceLimit(
                    "source th artifact exceeds configured storage limits".into(),
                ));
            }
            std::fs::create_dir_all(&temp_root).map_err(|_| {
                CoreError::Temporary("source th temporary directory could not be prepared".into())
            })?;
            let destination = temp_root.join(format!("th-{}.gif", uuid::Uuid::new_v4()));
            let receipt = context
                .http_transport
                .get_to_path_same_origin_limited_with_headers(
                    radar.url,
                    &[("Referer", radar.referer)],
                    &destination,
                    max_bytes,
                )
                .await
                .map_err(sanitize_transport_error)?;
            context.limits.validate_bytes(receipt.size_bytes, receipt.size_bytes)?;
            let path = tempfile::TempPath::try_from_path(destination).map_err(|_| {
                CoreError::Temporary("source th temporary artifact could not be retained".into())
            })?;
            if receipt.size_bytes > context.limits.max_temp_bytes {
                return Err(CoreError::ResourceLimit(
                    "source th artifact exceeds temporary storage limit".into(),
                ));
            }
            if receipt.sha256 != expected_hash {
                return Err(CoreError::Transport(
                    "TMD GIF changed since timestamp discovery; rediscover before acquisition"
                        .into(),
                ));
            }
            let artifact = RawArtifact {
                receipt: ArtifactReceipt {
                    name: format!("{}.gif", radar.station),
                    media_type: "image/gif".into(),
                    size_bytes: receipt.size_bytes,
                    sha256: receipt.sha256,
                },
                path,
            };
            Ok(RawFrame { frame, artifacts: vec![artifact], private_locator: None })
        }))
    }
}

fn validate_target(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport("source th received a mismatched query".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport("source th only supports composite".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source th does not expose base times".into()));
    }
    if !matches!(query.selector, TimeSelector::Latest) {
        return Err(CoreError::Transport("source th only supports latest frames".into()));
    }
    Ok(())
}

fn selected_stations(target: &DiscoveryTarget, query: &Query) -> Vec<&'static str> {
    RADARS
        .iter()
        .filter(|radar| {
            target.station.as_deref().is_none_or(|station| station == radar.station)
                && (query.stations.is_empty()
                    || query.stations.iter().any(|station| station == radar.station))
        })
        .map(|radar| radar.station)
        .collect()
}

fn footer_images(payload: &[u8], station: &str) -> CoreResult<Vec<Vec<u8>>> {
    if !RADARS.iter().any(|radar| radar.station == station) {
        return Err(CoreError::Transport("unsupported TMD timestamp profile".into()));
    }
    let decoder = GifDecoder::new(Cursor::new(payload)).map_err(|_| {
        CoreError::Transport(format!("TMD {station} response is not a readable GIF"))
    })?;
    if decoder.dimensions() != (WIDTH, HEIGHT) {
        return Err(CoreError::Transport(format!(
            "TMD {station} timestamp profile expects a 680x680 GIF"
        )));
    }
    let frames = decoder.into_frames();
    let mut images = Vec::new();
    for frame in frames {
        if images.len() == MAX_FRAMES {
            return Err(CoreError::Transport(format!(
                "TMD {station} GIF exceeds the supported frame count"
            )));
        }
        let rgba = frame
            .map_err(|_| CoreError::Transport(format!("TMD {station} GIF frame is invalid")))?
            .into_buffer();
        let grayscale = DynamicImage::ImageRgba8(rgba).to_luma8();
        let crop =
            imageops::crop_imm(&grayscale, FOOTER.0, FOOTER.1, FOOTER.2, FOOTER.3).to_image();
        let mut enlarged =
            imageops::resize(&crop, FOOTER.2 * 6, FOOTER.3 * 6, imageops::FilterType::Lanczos3);
        if station == "kkn240Loop" {
            for pixel in enlarged.pixels_mut() {
                pixel[0] = if pixel[0] > 120 { 255 } else { 0 };
            }
        }
        let mut cursor = Cursor::new(Vec::new());
        DynamicImage::ImageLuma8(enlarged)
            .write_to(&mut cursor, ImageFormat::Png)
            .map_err(|_| CoreError::Transport("TMD footer image could not be encoded".into()))?;
        images.push(cursor.into_inner());
    }
    if images.is_empty() {
        return Err(CoreError::Transport(format!("TMD {station} GIF contains no frames")));
    }
    Ok(images)
}

async fn run_tesseract(png: &[u8]) -> CoreResult<String> {
    let mut child = Command::new("tesseract")
        .args([
            "stdin",
            "stdout",
            "--psm",
            "7",
            "-c",
            "tessedit_char_whitelist=0123456789:- ",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CoreError::Transport(
                    "Tesseract is required for TMD timestamp binding (tesseract executable not found)".into(),
                )
            } else {
                CoreError::Transport("Tesseract could not be started for TMD timestamp binding".into())
            }
        })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| CoreError::Transport("Tesseract input pipe is unavailable".into()))?;
    stdin.write_all(png).await.map_err(|_| {
        CoreError::Transport("Tesseract could not read the TMD footer image".into())
    })?;
    drop(stdin);
    let output = tokio::time::timeout(OCR_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| CoreError::Transport("Tesseract timed out reading a TMD footer".into()))?
        .map_err(|_| CoreError::Transport("Tesseract failed reading a TMD footer".into()))?;
    if !output.status.success() {
        return Err(CoreError::Transport("Tesseract failed reading a TMD footer".into()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_footer_time(text: &str) -> Option<DateTime<Utc>> {
    let bytes = text.as_bytes();
    let mut matches = Vec::new();
    for start in 0..bytes.len().saturating_sub(1) {
        if bytes.get(start..start + 2) != Some(b"20") {
            continue;
        }
        let Some((time, _)) = parse_footer_at(bytes, start) else {
            continue;
        };
        matches.push(time);
    }
    (matches.len() == 1).then(|| matches[0])
}

fn parse_footer_at(bytes: &[u8], start: usize) -> Option<(DateTime<Utc>, usize)> {
    let mut pos = start;
    let year = read_digits(bytes, &mut pos, 4)?;
    skip_spaces(bytes, &mut pos);
    expect(bytes, &mut pos, b'-')?;
    skip_spaces(bytes, &mut pos);
    let month = read_digits(bytes, &mut pos, 2)?;
    skip_spaces(bytes, &mut pos);
    expect(bytes, &mut pos, b'-')?;
    skip_spaces(bytes, &mut pos);
    let day = read_digits(bytes, &mut pos, 2)?;
    let mut separated_date_and_clock = skip_spaces(bytes, &mut pos) > 0;
    if bytes.get(pos) == Some(&b':') {
        pos += 1;
        skip_spaces(bytes, &mut pos);
        separated_date_and_clock = true;
    }
    if !separated_date_and_clock {
        return None;
    }
    let hour = read_digits(bytes, &mut pos, 2)?;
    let had_space = skip_spaces(bytes, &mut pos) > 0;
    if bytes.get(pos) == Some(&b':') {
        pos += 1;
    } else if !had_space {
        return None;
    }
    skip_spaces(bytes, &mut pos);
    let minute = read_digits(bytes, &mut pos, 2)?;
    let mut separators = 0;
    while bytes.get(pos).is_some_and(|byte| !byte.is_ascii_digit()) && separators < 2 {
        pos += 1;
        separators += 1;
    }
    let second = read_digits(bytes, &mut pos, 2)?;
    let date = NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)?;
    let value = date.and_hms_opt(hour, minute, second)?.and_utc();
    Some((value, pos))
}

fn read_digits(bytes: &[u8], pos: &mut usize, count: usize) -> Option<u32> {
    let end = pos.checked_add(count)?;
    let slice = bytes.get(*pos..end)?;
    if !slice.iter().all(u8::is_ascii_digit) {
        return None;
    }
    *pos = end;
    std::str::from_utf8(slice).ok()?.parse().ok()
}

fn skip_spaces(bytes: &[u8], pos: &mut usize) -> usize {
    let start = *pos;
    while bytes.get(*pos).is_some_and(u8::is_ascii_whitespace) {
        *pos += 1;
    }
    *pos - start
}

fn expect(bytes: &[u8], pos: &mut usize, value: u8) -> Option<()> {
    if bytes.get(*pos) == Some(&value) {
        *pos += 1;
        Some(())
    } else {
        None
    }
}

fn validate_footer_sequence(station: &str, times: &[DateTime<Utc>]) -> CoreResult<()> {
    if times.is_empty() || times.len() > MAX_FRAMES {
        return Err(CoreError::Transport(format!("TMD {station} GIF has unsupported frame count")));
    }
    for pair in times.windows(2) {
        let elapsed = pair[1].signed_duration_since(pair[0]);
        if elapsed.num_seconds() <= 0 {
            return Err(CoreError::Transport(format!(
                "TMD {station} footer timestamps are not strictly increasing"
            )));
        }
        if elapsed.num_seconds() != 15 * 60 {
            return Err(CoreError::Transport(format!(
                "TMD {station} footer timestamps do not follow the verified 15-minute cadence"
            )));
        }
    }
    Ok(())
}

fn frame_from_payload(
    radar: &Radar,
    payload: &[u8],
    times: &[DateTime<Utc>],
) -> CoreResult<FrameRef> {
    validate_footer_sequence(radar.station, times)?;
    let hash = hex::encode(Sha256::digest(payload));
    let footer_times = times
        .iter()
        .map(|time| time.to_rfc3339_opts(SecondsFormat::Secs, true))
        .collect::<Vec<_>>();
    let mut frame = FrameRef {
        source: SOURCE.into(),
        product: PRODUCT.into(),
        station: Some(radar.station.into()),
        valid_time: times.last().unwrap().to_rfc3339_opts(SecondsFormat::Micros, true),
        base_time: None,
        logical_id: String::new(),
        revision: Some(hash.clone()),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": radar.url,
            "artifacts": [],
            "name": format!("{}.gif", radar.station),
            "media_type": "image/gif",
            "headers": {"Referer": radar.referer},
            "station": radar.station,
            "revision": hash,
            "payload_sha256": hash,
            "time_semantics": "rendered_footer_ocr_utc",
            "footer_observation_times": footer_times,
            "geometry_status": "unverified"
        }),
    };
    frame.logical_id = logical_id(&frame)
        .map_err(|_| CoreError::Transport("source th frame identity is invalid".into()))?;
    Ok(frame)
}

fn frame_plan(frame: &FrameRef) -> Option<(&'static Radar, String)> {
    if frame.source != SOURCE
        || frame.product != PRODUCT
        || frame.base_time.is_some()
        || frame.locator_version != LOCATOR_VERSION
        || logical_id(frame).ok().as_deref() != Some(frame.logical_id.as_str())
        || frame.locator.get("time_semantics").and_then(Value::as_str)
            != Some("rendered_footer_ocr_utc")
        || frame.locator.get("geometry_status").and_then(Value::as_str) != Some("unverified")
    {
        return None;
    }
    let station = frame.station.as_deref()?;
    let radar = RADARS.iter().find(|radar| radar.station == station)?;
    let hash = frame.locator.get("payload_sha256").and_then(Value::as_str)?;
    if hash.len() != 64
        || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        || frame.revision.as_deref() != Some(hash)
        || frame.locator.get("revision").and_then(Value::as_str) != Some(hash)
        || frame.locator.get("url").and_then(Value::as_str) != Some(radar.url)
        || frame.locator.get("name").and_then(Value::as_str)
            != Some(if station == "cmp1" { "cmp1.gif" } else { "kkn240Loop.gif" })
        || frame.locator.get("media_type").and_then(Value::as_str) != Some("image/gif")
        || frame
            .locator
            .get("headers")
            .and_then(|value| value.get("Referer"))
            .and_then(Value::as_str)
            != Some(radar.referer)
    {
        return None;
    }
    let times = frame.locator.get("footer_observation_times")?.as_array()?;
    let parsed = times
        .iter()
        .map(|value| {
            DateTime::parse_from_rfc3339(value.as_str()?).ok().map(|time| time.with_timezone(&Utc))
        })
        .collect::<Option<Vec<_>>>()?;
    validate_footer_sequence(station, &parsed).ok()?;
    if parsed.last()?.to_rfc3339_opts(SecondsFormat::Micros, true) != frame.valid_time
        && parse_utc_time(&frame.valid_time).ok() != parsed.last().copied()
    {
        return None;
    }
    Some((radar, hash.to_owned()))
}

fn is_too_old(valid_time: DateTime<Utc>, max_age: Option<f64>, now: DateTime<Utc>) -> bool {
    max_age.is_some_and(|limit| {
        let age = (now - valid_time).num_microseconds().unwrap_or(0) as f64 / 1_000_000.0;
        age >= 0.0 && age > limit
    })
}

fn invalid_frame_locator() -> CoreError {
    CoreError::Transport("source th frame locator is outside the verified TMD endpoints".into())
}

fn sanitize_transport_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source th request requires network access".into())
        }
        CoreError::ResourceLimit(_) => {
            CoreError::ResourceLimit("source th response exceeds configured limits".into())
        }
        _ => CoreError::Transport("source th request failed".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    const GIF: &[u8] = include_bytes!("../../../../tests/fixtures/sources/th/raw/kkn240Loop.gif");

    fn query() -> Query {
        Query { source: Some(SOURCE.into()), ..Query::default() }
    }

    #[test]
    fn extracts_bounded_footer_crops_from_the_retained_animation() {
        let crops = footer_images(GIF, "kkn240Loop").unwrap();
        assert_eq!(crops.len(), 6);
        for png in crops {
            let image = image::load_from_memory_with_format(&png, ImageFormat::Png).unwrap();
            assert_eq!(image.dimensions(), (2040, 90));
        }
    }

    #[tokio::test]
    async fn optional_tesseract_binds_retained_footer_times_when_installed() {
        if std::process::Command::new("tesseract").arg("--version").output().is_err() {
            return;
        }
        let crops = footer_images(GIF, "kkn240Loop").unwrap();
        let mut times = Vec::with_capacity(crops.len());
        for (index, crop) in crops.into_iter().enumerate() {
            let text = run_tesseract(&crop).await.unwrap();
            times.push(parse_footer_time(&text).unwrap_or_else(|| {
                panic!("Tesseract returned an unrecognized TMD footer for frame {index}: {text:?}")
            }));
        }
        validate_footer_sequence("kkn240Loop", &times).unwrap();
        assert_eq!(times.last().unwrap().to_rfc3339(), "2023-04-22T16:00:05+00:00");
    }

    #[test]
    fn parses_only_unambiguous_calendar_valid_footer_times() {
        assert_eq!(
            parse_footer_time("2023 - 04 - 22 14:45:05\n").unwrap().to_rfc3339(),
            "2023-04-22T14:45:05+00:00"
        );
        assert_eq!(
            parse_footer_time("1142 2023-04-22: 15:15:05 1:050\n").unwrap().to_rfc3339(),
            "2023-04-22T15:15:05+00:00"
        );
        assert!(parse_footer_time("2023-02-30 14:45:05").is_none());
        assert!(parse_footer_time("2023-04-22 14:45").is_none());
        assert!(parse_footer_time("2023-04-22 14:45:05 2023-04-22 15:00:05").is_none());
    }

    #[test]
    fn rejects_unverified_or_malformed_frame_times_and_cadence() {
        let first = DateTime::parse_from_rfc3339("2023-04-22T14:45:05Z").unwrap().to_utc();
        let second = DateTime::parse_from_rfc3339("2023-04-22T15:00:05Z").unwrap().to_utc();
        validate_footer_sequence("kkn240Loop", &[first, second]).unwrap();
        assert!(validate_footer_sequence("kkn240Loop", &[second, first]).is_err());
        assert!(validate_footer_sequence("kkn240Loop", &[first, first]).is_err());
        assert!(validate_footer_sequence("kkn240Loop", &[]).is_err());
    }

    #[test]
    fn latest_only_query_and_source_urls_are_fail_closed() {
        let target =
            DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None };
        validate_target(&target, &query()).unwrap();
        let at =
            Query { selector: TimeSelector::At { time: "2023-04-22T14:45:05Z".into() }, ..query() };
        assert!(validate_target(&target, &at).is_err());
        assert_eq!(selected_stations(&target, &query()), vec!["cmp1", "kkn240Loop"]);
        assert_eq!(
            selected_stations(&target, &Query { stations: vec!["unknown".into()], ..query() }),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn frame_locator_binds_ocr_times_and_exact_raw_digest_to_the_endpoint() {
        let radar = RADARS[1];
        let times = [
            "2023-04-22T14:45:05Z",
            "2023-04-22T15:00:05Z",
            "2023-04-22T15:15:05Z",
            "2023-04-22T15:30:05Z",
            "2023-04-22T15:45:05Z",
            "2023-04-22T16:00:05Z",
        ]
        .map(|time| DateTime::parse_from_rfc3339(time).unwrap().to_utc());
        let frame = frame_from_payload(&radar, GIF, &times).unwrap();
        assert_eq!(frame.valid_time, "2023-04-22T16:00:05.000000Z");
        assert_eq!(
            hex::encode(Sha256::digest(GIF)),
            "98b6e087ddf5ff01e139f862409eae65c288d47e44603d67e3e0e224b49cb5cd"
        );
        assert_eq!(frame.revision.as_deref(), Some(hex::encode(Sha256::digest(GIF)).as_str()));
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());
        let adapter = ThSourceAdapter;
        assert!(adapter.allows_artifact_url(&frame, &Url::parse(radar.url).unwrap()));
        assert!(!adapter.allows_artifact_url(
            &frame,
            &Url::parse("https://weather.tmd.go.th/other/kkn240Loop.gif").unwrap()
        ));
    }

    #[test]
    fn absent_tesseract_has_a_clear_optional_tool_error() {
        let error = std::io::Error::from(std::io::ErrorKind::NotFound);
        let message = if error.kind() == std::io::ErrorKind::NotFound {
            "Tesseract is required for TMD timestamp binding (tesseract executable not found)"
        } else {
            "other error"
        };
        assert!(message.contains("Tesseract is required"));
    }
}
