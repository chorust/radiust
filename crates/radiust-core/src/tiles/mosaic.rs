use crate::errors::{CoreError, CoreResult};
use image::{RgbaImage, imageops};

#[derive(Debug, Clone)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub image: RgbaImage,
}

impl Tile {
    pub fn new(x: u32, y: u32, image: RgbaImage) -> Self {
        Self { x, y, image }
    }
}

pub fn mosaic(tiles: &[Tile], columns: u32, rows: u32) -> CoreResult<RgbaImage> {
    if columns == 0 || rows == 0 || tiles.len() as u64 != u64::from(columns) * u64::from(rows) {
        return Err(CoreError::Storage("mosaic has missing or invalid tiles".into()));
    }
    let first = tiles.first().ok_or_else(|| CoreError::Storage("mosaic has no tiles".into()))?;
    let tile_width = first.image.width();
    let tile_height = first.image.height();
    if tile_width == 0 || tile_height == 0 {
        return Err(CoreError::Storage("tile dimensions must be positive".into()));
    }
    let width = tile_width
        .checked_mul(columns)
        .ok_or_else(|| CoreError::Storage("mosaic width overflows".into()))?;
    let height = tile_height
        .checked_mul(rows)
        .ok_or_else(|| CoreError::Storage("mosaic height overflows".into()))?;
    let mut output = RgbaImage::new(width, height);
    let mut positions = std::collections::HashSet::new();
    for tile in tiles {
        if tile.x >= columns
            || tile.y >= rows
            || tile.image.dimensions() != (tile_width, tile_height)
        {
            return Err(CoreError::Storage("tile position or dimensions are invalid".into()));
        }
        if !positions.insert((tile.x, tile.y)) {
            return Err(CoreError::Storage("mosaic contains duplicate tile positions".into()));
        }
        imageops::replace(
            &mut output,
            &tile.image,
            i64::from(tile.x * tile_width),
            i64::from(tile.y * tile_height),
        );
    }
    Ok(output)
}
