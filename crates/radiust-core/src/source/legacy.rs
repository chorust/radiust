//! Fixed, source-backed tile layouts that are safe to expose as raw previews.
//!
//! This deliberately does not infer a layout from artifact filenames. A layout
//! is enabled only when the complete native frame identity and locator match a
//! known adapter contract. BMKG is enabled only when its adapter validates the
//! complete generated four-tile locator; the composition remains raw-only and
//! does not assert that the provider currently serves the tiles.

use crate::errors::{CoreError, CoreResult};
use crate::model::{FrameRef, parse_utc_time};
use serde_json::Value;

const RAINVIEWER_SOURCE: &str = "rainviewer";
const RAINVIEWER_PRODUCT: &str = "composite";
const RAINVIEWER_LOCATOR_VERSION: &str = "rainviewer-v2";
const RAINVIEWER_API: &str = "https://api.rainviewer.com/public/weather-maps.json";
const RAINVIEWER_ORIGIN: &str = "https://tilecache.rainviewer.com";

const WINDY_SOURCE: &str = "windy";
const WINDY_PRODUCT: &str = "reflectivity";
const WINDY_LOCATOR_VERSION: &str = "windy-legacy-v1";
const WINDY_HOST: &str = "rdr.windy.com";
const WINDY_CADENCE_SECONDS: i64 = 300;

const ZOOM: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TilePosition {
    pub name: String,
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct VerifiedTileLayout {
    pub tile_width: u32,
    pub tile_height: u32,
    pub columns: u32,
    pub rows: u32,
    pub tiles: Vec<TilePosition>,
}

pub(super) fn verified_tile_layout(frame: &FrameRef) -> CoreResult<VerifiedTileLayout> {
    frame.validate_identity().map_err(|_| invalid_locator())?;

    match frame.source.as_str() {
        RAINVIEWER_SOURCE => rainviewer_layout(frame),
        WINDY_SOURCE => windy_layout(frame),
        "bmkg" if super::bmkg::has_verified_raw_tile_plan(frame) => Ok(fixed_layout(256)),
        "bmkg" => Err(invalid_locator()),
        _ => Err(CoreError::Transport(
            "raw tile preview layout is unsupported for this source".into(),
        )),
    }
}

fn rainviewer_layout(frame: &FrameRef) -> CoreResult<VerifiedTileLayout> {
    let locator = frame.locator.as_object().ok_or_else(invalid_locator)?;
    if frame.product != RAINVIEWER_PRODUCT
        || frame.station.is_some()
        || frame.base_time.is_some()
        || frame.locator_version != RAINVIEWER_LOCATOR_VERSION
        || !has_exact_keys(
            &frame.locator,
            &["api_url", "host", "path", "tile_size", "zoom", "color", "options"],
        )
        || locator.get("api_url").and_then(Value::as_str) != Some(RAINVIEWER_API)
        || locator.get("host").and_then(Value::as_str) != Some(RAINVIEWER_ORIGIN)
        || locator.get("tile_size").and_then(Value::as_u64) != Some(512)
        || locator.get("zoom").and_then(Value::as_u64) != Some(u64::from(ZOOM))
        || locator.get("color").and_then(Value::as_u64) != Some(2)
        || locator.get("options").and_then(Value::as_str) != Some("0_0")
    {
        return Err(invalid_locator());
    }

    let path = locator.get("path").and_then(Value::as_str).ok_or_else(invalid_locator)?;
    let revision = path.strip_prefix("/v2/radar/").ok_or_else(invalid_locator)?;
    if revision.is_empty()
        || revision.len() > 128
        || !revision.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || path != format!("/v2/radar/{revision}")
        || frame.revision.as_deref() != Some(revision)
        || parse_utc_time(&frame.valid_time).is_err()
    {
        return Err(invalid_locator());
    }

    Ok(fixed_layout(512))
}

fn windy_layout(frame: &FrameRef) -> CoreResult<VerifiedTileLayout> {
    let locator = frame.locator.as_object().ok_or_else(invalid_locator)?;
    if frame.product != WINDY_PRODUCT
        || frame.station.as_deref() != Some("global")
        || frame.base_time.is_some()
        || frame.locator_version != WINDY_LOCATOR_VERSION
        || !has_exact_keys(&frame.locator, &["url", "name", "artifacts", "station", "revision"])
        || locator.get("station").and_then(Value::as_str) != Some("global")
    {
        return Err(invalid_locator());
    }

    let valid_time = parse_utc_time(&frame.valid_time).map_err(|_| invalid_locator())?;
    if valid_time.timestamp_subsec_nanos() != 0
        || valid_time.timestamp().rem_euclid(WINDY_CADENCE_SECONDS) != 0
    {
        return Err(invalid_locator());
    }
    let revision = format!("windy-{}", valid_time.timestamp());
    if frame.revision.as_deref() != Some(revision.as_str())
        || locator.get("revision").and_then(Value::as_str) != Some(revision.as_str())
    {
        return Err(invalid_locator());
    }

    let path_time = valid_time.format("%Y/%m/%d/%H%M").to_string();
    let max_time = valid_time.format("%Y%m%d%H%M%S").to_string();
    let expected = tile_positions().into_iter().map(|tile| {
        let url = format!(
            "https://{WINDY_HOST}/radar2/composite/{path_time}/{ZOOM}/{}/{}/reflectivity.png?multichannel=true&maxt={max_time}",
            tile.x, tile.y
        );
        (tile, url)
    });
    let expected = expected.collect::<Vec<_>>();
    if locator.get("url").and_then(Value::as_str) != expected.first().map(|(_, url)| url.as_str())
        || locator.get("name").and_then(Value::as_str)
            != expected.first().map(|(tile, _)| tile.name.as_str())
    {
        return Err(invalid_locator());
    }

    let artifacts =
        locator.get("artifacts").and_then(Value::as_array).ok_or_else(invalid_locator)?;
    if artifacts.len() != expected.len() - 1 {
        return Err(invalid_locator());
    }
    for (artifact, (tile, url)) in artifacts.iter().zip(expected.iter().skip(1)) {
        if !has_exact_keys(artifact, &["url", "name", "role", "media_type"])
            || artifact.get("url").and_then(Value::as_str) != Some(url.as_str())
            || artifact.get("name").and_then(Value::as_str) != Some(tile.name.as_str())
            || artifact.get("role").and_then(Value::as_str) != Some("tile")
            || artifact.get("media_type").and_then(Value::as_str) != Some("image/png")
        {
            return Err(invalid_locator());
        }
    }

    Ok(fixed_layout(256))
}

fn fixed_layout(tile_size: u32) -> VerifiedTileLayout {
    VerifiedTileLayout {
        tile_width: tile_size,
        tile_height: tile_size,
        columns: 1 << ZOOM,
        rows: 1 << ZOOM,
        tiles: tile_positions(),
    }
}

fn tile_positions() -> Vec<TilePosition> {
    (0..(1 << ZOOM))
        .flat_map(|y| {
            (0..(1 << ZOOM)).map(move |x| TilePosition {
                name: format!("tile-z{ZOOM}-x{x}-y{y}.png"),
                x,
                y,
            })
        })
        .collect()
}

fn has_exact_keys(value: &Value, expected: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
    })
}

fn invalid_locator() -> CoreError {
    CoreError::Transport("raw tile preview frame locator is invalid".into())
}
