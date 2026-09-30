//! Bounded raw-image preview for local file inspection.

use crate::errors::{CoreError, CoreResult};
use crate::limits::Limits;
use crate::model::{Preview, PreviewMode, RawArtifact};
use image::{ImageDecoder, ImageReader};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

// A 16-bit RGBA decode can occupy 8 bytes per pixel while the 8-bit RGBA
// preview buffer is being produced. Reserve both buffers before decoding.
const DECODE_BYTES_PER_PIXEL: u64 = 12;

pub struct RawImagePreview {
    pub preview: Preview,
    pub format: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Decode one selected local raster image after enforcing byte and pixel
/// bounds. This path preserves source pixels and does not invoke science,
/// legacy display rules, or frame prefetch.
pub fn preview_bytes(bytes: &[u8], limits: &Limits) -> CoreResult<RawImagePreview> {
    if bytes.len() as u64 > limits.max_artifact_bytes {
        return Err(CoreError::ResourceLimit("raw image exceeds artifact byte limit".into()));
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| CoreError::Transport("unsupported or malformed image".into()))?;
    let format = reader
        .format()
        .ok_or_else(|| CoreError::Transport("image format could not be identified".into()))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| CoreError::Transport("image dimensions could not be read".into()))?;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| CoreError::ResourceLimit("image dimensions overflow".into()))?;
    validate_decode_budget(pixels, limits)?;
    let decoded = image::load_from_memory_with_format(bytes, format)
        .map_err(|_| CoreError::Transport("image decoding failed".into()))?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    finish_preview(width, height, decoded.to_rgba8().into_raw(), format, bytes.len() as u64, digest)
}

/// Preview an image file without copying its encoded bytes into memory.
/// Dimensions are checked before decoding, and the content digest is computed
/// in a bounded second pass over the selected file.
pub fn preview_file(path: impl AsRef<Path>, limits: &Limits) -> CoreResult<RawImagePreview> {
    let path = path.as_ref();
    let metadata = std::fs::metadata(path)
        .map_err(|_| CoreError::Transport("preview file could not be opened".into()))?;
    if metadata.len() > limits.max_artifact_bytes {
        return Err(CoreError::ResourceLimit("raw image exceeds artifact byte limit".into()));
    }

    let reader = ImageReader::open(path)
        .map_err(|_| CoreError::Transport("preview file could not be opened".into()))?
        .with_guessed_format()
        .map_err(|_| CoreError::Transport("unsupported or malformed image".into()))?;
    let format = reader
        .format()
        .ok_or_else(|| CoreError::Transport("image format could not be identified".into()))?;
    let decoder = reader
        .into_decoder()
        .map_err(|_| CoreError::Transport("image dimensions could not be read".into()))?;
    let (width, height) = decoder.dimensions();
    let pixels = validate_dimensions(width, height, limits)?;
    validate_decode_budget(pixels, limits)?;
    let decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| CoreError::Transport("image decoding failed".into()))?;

    let mut file = File::open(path)
        .map_err(|_| CoreError::Transport("preview file could not be opened".into()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size_bytes = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| CoreError::Transport("preview file could not be read".into()))?;
        if count == 0 {
            break;
        }
        size_bytes = size_bytes.saturating_add(count as u64);
        if size_bytes > limits.max_artifact_bytes {
            return Err(CoreError::ResourceLimit("raw image exceeds artifact byte limit".into()));
        }
        digest.update(&buffer[..count]);
    }
    if size_bytes != metadata.len() {
        return Err(CoreError::Transport("preview file changed while it was read".into()));
    }

    finish_preview(width, height, decoded.to_rgba8().into_raw(), format, size_bytes, digest)
}

/// Preview one acquired image artifact only after its bytes match the
/// adapter-provided receipt. Local-file previews have no source receipt and
/// should continue to use [`preview_file`].
pub fn preview_artifact(artifact: &RawArtifact, limits: &Limits) -> CoreResult<RawImagePreview> {
    if !artifact.receipt.media_type.starts_with("image/") {
        return Err(CoreError::Transport(
            "raw image preview artifact media type is not an image".into(),
        ));
    }
    let receipt = &artifact.receipt;
    if receipt.name.trim().is_empty()
        || receipt.size_bytes == 0
        || receipt.sha256.len() != 64
        || !receipt.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || receipt.sha256.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(CoreError::Transport("raw image preview receipt is invalid".into()));
    }
    limits.validate_bytes(receipt.size_bytes, receipt.size_bytes)?;

    let preview = preview_file(&artifact.path, limits)?;
    if preview.size_bytes != receipt.size_bytes || preview.sha256 != receipt.sha256 {
        return Err(CoreError::Transport(
            "raw image preview artifact does not match its receipt".into(),
        ));
    }
    Ok(preview)
}

fn validate_dimensions(width: u32, height: u32, limits: &Limits) -> CoreResult<u64> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| CoreError::ResourceLimit("image dimensions overflow".into()))?;
    limits.validate_pixels(pixels)?;
    Ok(pixels)
}

fn validate_decode_budget(pixels: u64, limits: &Limits) -> CoreResult<()> {
    limits.validate_pixels(pixels)?;
    let temporary_bytes = pixels
        .checked_mul(DECODE_BYTES_PER_PIXEL)
        .ok_or_else(|| CoreError::ResourceLimit("image decode memory size overflow".into()))?;
    if temporary_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(format!(
            "image decode temporary bytes {temporary_bytes} > {}",
            limits.max_temp_bytes
        )));
    }
    Ok(())
}

fn finish_preview(
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    format: image::ImageFormat,
    size_bytes: u64,
    digest: Sha256,
) -> CoreResult<RawImagePreview> {
    let preview =
        Preview { width, height, rgba, frame: None, mode: PreviewMode::Raw, rule_version: None };
    preview
        .validate()
        .map_err(|_| CoreError::Transport("decoded image dimensions are inconsistent".into()))?;
    Ok(RawImagePreview {
        preview,
        format: format!("{format:?}").to_ascii_uppercase(),
        size_bytes,
        sha256: hex::encode(digest.finalize()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_source_raster_is_previewed_as_raw_pixels() {
        let bytes =
            include_bytes!("../../../tests/fixtures/sources/au/raw/IDR021.T.202609180511.png");
        let preview = preview_bytes(bytes, &Limits::default()).unwrap();
        assert_eq!((preview.preview.width, preview.preview.height), (512, 512));
        assert_eq!(preview.preview.mode, PreviewMode::Raw);
        assert!(
            preview.preview.frame.is_none(),
            "local pixels must not acquire invented source time"
        );
        assert_eq!(&preview.preview.rgba[..4], &[192, 192, 192, 255]);
        assert_eq!(preview.format, "PNG");
        assert_eq!(
            preview.sha256,
            "4ab60f3c01df485d6d5bf24417733425a4900e160c43b4543b51bc124d7a0c8c"
        );

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/sources/au/raw/IDR021.T.202609180511.png"
        );
        let file_preview = preview_file(path, &Limits::default()).unwrap();
        assert_eq!(file_preview.format, preview.format);
        assert_eq!(file_preview.size_bytes, preview.size_bytes);
        assert_eq!(file_preview.sha256, preview.sha256);
        assert_eq!(file_preview.preview.rgba, preview.preview.rgba);
    }
}
