//! Receipt-bound source tile layouts that are safe to expose as raw previews.
//!
//! This deliberately does not infer a layout from artifact filenames. A layout
//! is enabled only when the owning adapter validates the complete frame identity
//! and locator (RainViewer's science contract, or Windy's and BMKG's generated
//! raw tile plan); the composition remains raw-only and does not assert that the
//! provider currently serves the tiles. XYZ coordinates stay in the plan;
//! positions here are relative to the selected mosaic, including wrapped views.
use super::tile_plan::{PlanTile, TilePlan};
use crate::errors::{CoreError, CoreResult};
use crate::model::FrameRef;

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
    let (size, max_zoom, keys): (u32, u32, &[&str]) = match frame.source.as_str() {
        "rainviewer" => {
            super::rainviewer::validate_science_frame(frame).map_err(|_| invalid_locator())?;
            (
                512,
                super::rainviewer::MAX_ZOOM,
                &["api_url", "host", "path", "tile_size", "zoom", "color", "options"],
            )
        }
        "windy" if super::windy::has_verified_raw_tile_plan(frame) => {
            (256, super::windy::MAX_ZOOM, &["url", "name", "artifacts", "station", "revision"])
        }
        "bmkg" if super::bmkg::has_verified_raw_tile_plan(frame) => {
            (256, super::bmkg::MAX_ZOOM, &["url", "name", "artifacts", "station", "revision"])
        }
        "windy" | "bmkg" => return Err(invalid_locator()),
        _ => {
            return Err(CoreError::Transport(
                "raw tile preview layout is unsupported for this source".into(),
            ));
        }
    };
    let locator = frame.locator.as_object().ok_or_else(invalid_locator)?;
    if locator.len() != keys.len() + usize::from(locator.contains_key("tile_selection"))
        || !keys.iter().all(|key| locator.contains_key(*key))
        || locator.get("artifacts").and_then(|v| v.as_array()).is_some_and(|artifacts| {
            artifacts.iter().any(|artifact| {
                artifact.as_object().is_none_or(|a| {
                    a.len() != 4
                        || !["url", "name", "role", "media_type"]
                            .iter()
                            .all(|key| a.contains_key(*key))
                })
            })
        })
    {
        return Err(invalid_locator());
    }
    let plan = TilePlan::from_locator(frame, max_zoom).map_err(|_| invalid_locator())?;
    Ok(VerifiedTileLayout {
        tile_width: size,
        tile_height: size,
        columns: plan.columns,
        rows: plan.rows,
        tiles: plan
            .tiles()
            .map(|PlanTile { x, y, column, row }| TilePosition {
                name: plan.name(x, y),
                x: column,
                y: row,
            })
            .collect(),
    })
}

fn invalid_locator() -> CoreError {
    CoreError::Transport("raw tile preview frame locator is invalid".into())
}
