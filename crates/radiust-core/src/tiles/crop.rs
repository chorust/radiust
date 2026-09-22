use crate::errors::{CoreError, CoreResult};
use image::{RgbaImage, imageops};

pub fn crop_lossless(
    image: &RgbaImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> CoreResult<RgbaImage> {
    if width == 0 || height == 0 {
        return Err(CoreError::Storage("crop dimensions must be positive".into()));
    }
    let right = x
        .checked_add(width)
        .ok_or_else(|| CoreError::Storage("crop overflows image bounds".into()))?;
    let bottom = y
        .checked_add(height)
        .ok_or_else(|| CoreError::Storage("crop overflows image bounds".into()))?;
    if right > image.width() || bottom > image.height() {
        return Err(CoreError::Storage("crop is outside image bounds".into()));
    }
    Ok(imageops::crop_imm(image, x, y, width, height).to_image())
}
