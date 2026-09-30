//! Bounded, source-backed raw tile previews.
//!
//! Only layouts explicitly validated by `legacy` are composed. Composition
//! preserves the tile RGBA pixels and does not imply georeferencing or
//! scientific meaning.

use super::legacy::{TilePosition, VerifiedTileLayout, verified_tile_layout};
use crate::errors::{CoreError, CoreResult};
use crate::limits::Limits;
use crate::model::{ArtifactReceipt, Preview, PreviewMode, RawArtifact, RawFrame};
use image::{ImageDecoder, ImageReader};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

const EXPECTED_TILE_COUNT: usize = 4;
const MAX_TILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FRAME_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PREVIEW_PIXELS: u64 = 1_048_576;
const MAX_PREVIEW_MEMORY_BYTES: u64 = 64 * 1024 * 1024;
const DECODE_MEMORY_PER_PIXEL: u64 = 12;
const RGBA_BYTES_PER_PIXEL: u64 = 4;

/// A lossless 2×2 PNG tile composition for a verified source frame.
#[derive(Debug)]
pub struct SourceTilePreview {
    pub preview: Preview,
    /// Source tile encoding. Enabled layouts currently accept PNG only.
    pub format: String,
    /// Sum of the verified encoded tile sizes.
    pub size_bytes: u64,
    /// SHA-256 of the row-major composed RGBA bytes in `preview.rgba`.
    pub sha256: String,
    pub tile_count: usize,
}

/// Verify and compose a native source frame's complete raw tile set.
///
/// RainViewer's 512×512, Windy's 256×256, and BMKG's validated 256×256 PNG
/// tile layouts are supported. BMKG's provider response is not retained, so
/// its synthetic local test does not assert live availability. The function
/// validates the full frame locator, receipt names, byte counts, digests, image
/// dimensions, and artifact paths before returning a detached RGBA preview.
pub fn preview_source_tiles(raw: &RawFrame, limits: &Limits) -> CoreResult<SourceTilePreview> {
    let layout = verified_tile_layout(&raw.frame)?;
    let artifacts = index_artifacts(&raw.artifacts, &layout, limits)?;
    let (width, height, mosaic_pixels, output_bytes, decode_bytes) =
        validate_layout_budget(&layout, limits)?;

    let rgba_len = usize::try_from(output_bytes)
        .map_err(|_| resource_limit("raw tile preview dimensions exceed address space"))?;
    let mut rgba = vec![0_u8; rgba_len];

    for tile in &layout.tiles {
        let (artifact, path) = artifacts
            .get(&(tile.x, tile.y))
            .ok_or_else(|| transport_error("raw tile preview is missing tile coordinates"))?;
        verify_receipt(path, &artifact.receipt, limits)?;
        let tile_rgba = decode_tile(path, &layout, decode_bytes)?;
        verify_receipt(path, &artifact.receipt, limits)?;
        paste_tile(&mut rgba, width, &layout, tile, &tile_rgba)?;
    }

    let preview = Preview {
        width,
        height,
        rgba,
        frame: Some(raw.frame.clone()),
        mode: PreviewMode::Raw,
        rule_version: None,
    };
    preview
        .validate()
        .map_err(|_| transport_error("raw tile preview dimensions are inconsistent"))?;
    let mut digest = Sha256::new();
    digest.update(&preview.rgba);
    let size_bytes = raw
        .artifacts
        .iter()
        .try_fold(0_u64, |total, artifact| total.checked_add(artifact.receipt.size_bytes))
        .ok_or_else(|| resource_limit("raw tile preview byte count overflow"))?;

    debug_assert_eq!(mosaic_pixels, u64::from(width) * u64::from(height));
    Ok(SourceTilePreview {
        preview,
        format: "PNG".into(),
        size_bytes,
        sha256: hex::encode(digest.finalize()),
        tile_count: EXPECTED_TILE_COUNT,
    })
}

fn index_artifacts<'a>(
    artifacts: &'a [RawArtifact],
    layout: &VerifiedTileLayout,
    limits: &Limits,
) -> CoreResult<BTreeMap<(u32, u32), (&'a RawArtifact, PathBuf)>> {
    if artifacts.len() < EXPECTED_TILE_COUNT {
        return Err(transport_error("raw tile preview is missing tile coordinates"));
    }
    if artifacts.len() > EXPECTED_TILE_COUNT {
        return Err(transport_error("raw tile preview has an unexpected tile count"));
    }

    let expected = layout
        .tiles
        .iter()
        .map(|tile| (tile.name.as_str(), (tile.x, tile.y)))
        .collect::<BTreeMap<_, _>>();
    let mut by_position = BTreeMap::new();
    let mut canonical_paths = HashSet::new();
    let mut shared_parent: Option<PathBuf> = None;
    let mut total_bytes = 0_u64;

    for artifact in artifacts {
        let Some(&(x, y)) = expected.get(artifact.receipt.name.as_str()) else {
            return Err(transport_error("raw tile preview contains an unknown tile name"));
        };
        if by_position.contains_key(&(x, y)) {
            return Err(transport_error("raw tile preview contains duplicate tile coordinates"));
        }
        if artifact.receipt.media_type != "image/png" {
            return Err(transport_error("raw tile preview artifacts must be PNG images"));
        }
        validate_receipt_shape(&artifact.receipt)?;
        let artifact_cap = limits.max_artifact_bytes.min(MAX_TILE_BYTES);
        if artifact.receipt.size_bytes > artifact_cap {
            return Err(resource_limit("raw tile preview artifact exceeds the byte limit"));
        }
        total_bytes = total_bytes
            .checked_add(artifact.receipt.size_bytes)
            .ok_or_else(|| resource_limit("raw tile preview byte count overflow"))?;
        let frame_cap = limits.max_frame_bytes.min(limits.max_temp_bytes).min(MAX_FRAME_BYTES);
        if total_bytes > frame_cap {
            return Err(resource_limit("raw tile preview tiles exceed the byte limit"));
        }

        let (path, parent) = canonical_artifact_path(artifact.path.as_ref())?;
        if shared_parent.as_ref().is_some_and(|expected| expected != &parent) {
            return Err(transport_error("raw tile preview artifact path is unsafe"));
        }
        shared_parent.get_or_insert(parent);
        if !canonical_paths.insert(path.clone()) {
            return Err(transport_error("raw tile preview artifact path is unsafe"));
        }
        by_position.insert((x, y), (artifact, path));
    }

    if by_position.len() != EXPECTED_TILE_COUNT {
        return Err(transport_error("raw tile preview is missing tile coordinates"));
    }
    Ok(by_position)
}

fn validate_receipt_shape(receipt: &ArtifactReceipt) -> CoreResult<()> {
    if receipt.size_bytes == 0
        || receipt.sha256.len() != 64
        || !receipt.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || receipt.sha256.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(transport_error("raw tile preview artifact receipt is invalid"));
    }
    Ok(())
}

fn canonical_artifact_path(path: &Path) -> CoreResult<(PathBuf, PathBuf)> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        || path.file_name().is_none()
    {
        return Err(transport_error("raw tile preview artifact path is unsafe"));
    }
    let parent = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .ok_or_else(|| transport_error("raw tile preview artifact path is unsafe"))?;
    let link_metadata = std::fs::symlink_metadata(path)
        .map_err(|_| transport_error("raw tile preview artifact path is unsafe"))?;
    if !link_metadata.file_type().is_file() {
        return Err(transport_error("raw tile preview artifact path is unsafe"));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| transport_error("raw tile preview artifact path is unsafe"))?;
    if canonical.parent() != Some(parent.as_path()) {
        return Err(transport_error("raw tile preview artifact path is unsafe"));
    }
    Ok((canonical, parent))
}

fn validate_layout_budget(
    layout: &VerifiedTileLayout,
    limits: &Limits,
) -> CoreResult<(u32, u32, u64, u64, u64)> {
    if layout.tiles.len() != EXPECTED_TILE_COUNT
        || layout.columns != 2
        || layout.rows != 2
        || layout.tile_width == 0
        || layout.tile_height == 0
    {
        return Err(transport_error("raw tile preview layout is invalid"));
    }
    let width = layout
        .tile_width
        .checked_mul(layout.columns)
        .ok_or_else(|| resource_limit("raw tile preview dimensions overflow"))?;
    let height = layout
        .tile_height
        .checked_mul(layout.rows)
        .ok_or_else(|| resource_limit("raw tile preview dimensions overflow"))?;
    let tile_pixels = u64::from(layout.tile_width)
        .checked_mul(u64::from(layout.tile_height))
        .ok_or_else(|| resource_limit("raw tile preview dimensions overflow"))?;
    let mosaic_pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| resource_limit("raw tile preview dimensions overflow"))?;
    if mosaic_pixels > limits.max_pixels || mosaic_pixels > MAX_PREVIEW_PIXELS {
        return Err(resource_limit("raw tile preview exceeds the pixel limit"));
    }
    let output_bytes = mosaic_pixels
        .checked_mul(RGBA_BYTES_PER_PIXEL)
        .ok_or_else(|| resource_limit("raw tile preview memory size overflow"))?;
    let decode_bytes = tile_pixels
        .checked_mul(DECODE_MEMORY_PER_PIXEL)
        .ok_or_else(|| resource_limit("raw tile preview memory size overflow"))?;
    let total_memory = output_bytes
        .checked_add(decode_bytes)
        .ok_or_else(|| resource_limit("raw tile preview memory size overflow"))?;
    let memory_cap = limits.max_temp_bytes.min(MAX_PREVIEW_MEMORY_BYTES);
    if total_memory > memory_cap {
        return Err(resource_limit("raw tile preview exceeds the temporary memory limit"));
    }
    Ok((width, height, mosaic_pixels, output_bytes, decode_bytes))
}

fn verify_receipt(path: &Path, receipt: &ArtifactReceipt, limits: &Limits) -> CoreResult<()> {
    let cap = limits.max_artifact_bytes.min(MAX_TILE_BYTES);
    let mut file = open_artifact(path)?;
    let metadata =
        file.metadata().map_err(|_| transport_error("raw tile preview artifact cannot be read"))?;
    if !metadata.is_file() {
        return Err(transport_error("raw tile preview artifact path is unsafe"));
    }
    if metadata.len() > cap {
        return Err(resource_limit("raw tile preview artifact exceeds the byte limit"));
    }
    if metadata.len() != receipt.size_bytes {
        return Err(transport_error("raw tile preview artifact receipt does not match its bytes"));
    }
    let (size, digest) = hash_file(&mut file, cap)?;
    if size != receipt.size_bytes || hex::encode(digest) != receipt.sha256 {
        return Err(transport_error("raw tile preview artifact digest does not match its receipt"));
    }
    Ok(())
}

fn hash_file(file: &mut File, byte_cap: u64) -> CoreResult<(u64, [u8; 32])> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| transport_error("raw tile preview artifact cannot be read"))?;
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| transport_error("raw tile preview artifact cannot be read"))?;
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(count as u64)
            .ok_or_else(|| resource_limit("raw tile preview byte count overflow"))?;
        if bytes > byte_cap {
            return Err(resource_limit("raw tile preview artifact exceeds the byte limit"));
        }
        digest.update(&buffer[..count]);
    }
    let digest = digest.finalize();
    let mut result = [0_u8; 32];
    result.copy_from_slice(&digest);
    Ok((bytes, result))
}

fn decode_tile(path: &Path, layout: &VerifiedTileLayout, decode_bytes: u64) -> CoreResult<Vec<u8>> {
    let file = open_artifact(path)?;
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|_| transport_error("raw tile preview tile is not a valid PNG"))?;
    if reader.format() != Some(image::ImageFormat::Png) {
        return Err(transport_error("raw tile preview only accepts PNG images"));
    }
    let mut decoder_limits = image::Limits::default();
    decoder_limits.max_image_width = Some(layout.tile_width);
    decoder_limits.max_image_height = Some(layout.tile_height);
    decoder_limits.max_alloc = Some(decode_bytes);
    reader.limits(decoder_limits.clone());
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| transport_error("raw tile preview tile is not a valid PNG"))?;
    if decoder.dimensions() != (layout.tile_width, layout.tile_height) {
        return Err(transport_error(
            "raw tile preview tile dimensions do not match the verified layout",
        ));
    }
    decoder
        .set_limits(decoder_limits)
        .map_err(|_| resource_limit("raw tile preview exceeds the decoder memory limit"))?;
    let decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| transport_error("raw tile preview tile is not a valid PNG"))?;
    let rgba = decoded.into_rgba8().into_raw();
    let expected = u64::from(layout.tile_width)
        .checked_mul(u64::from(layout.tile_height))
        .and_then(|pixels| pixels.checked_mul(RGBA_BYTES_PER_PIXEL))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| resource_limit("raw tile preview dimensions overflow"))?;
    if rgba.len() != expected {
        return Err(transport_error("raw tile preview tile dimensions are inconsistent"));
    }
    Ok(rgba)
}

fn paste_tile(
    mosaic: &mut [u8],
    mosaic_width: u32,
    layout: &VerifiedTileLayout,
    tile: &TilePosition,
    rgba: &[u8],
) -> CoreResult<()> {
    let row_bytes = usize::try_from(u64::from(layout.tile_width) * RGBA_BYTES_PER_PIXEL)
        .map_err(|_| resource_limit("raw tile preview dimensions overflow"))?;
    let tile_height = usize::try_from(layout.tile_height)
        .map_err(|_| resource_limit("raw tile preview dimensions overflow"))?;
    let x_offset = usize::try_from(u64::from(tile.x) * u64::from(layout.tile_width) * 4)
        .map_err(|_| resource_limit("raw tile preview dimensions overflow"))?;
    let y_offset = usize::try_from(u64::from(tile.y) * u64::from(layout.tile_height))
        .map_err(|_| resource_limit("raw tile preview dimensions overflow"))?;
    let mosaic_stride = usize::try_from(u64::from(mosaic_width) * 4)
        .map_err(|_| resource_limit("raw tile preview dimensions overflow"))?;
    for row in 0..tile_height {
        let source_start = row * row_bytes;
        let destination_start = (y_offset + row) * mosaic_stride + x_offset;
        let source = rgba
            .get(source_start..source_start + row_bytes)
            .ok_or_else(|| transport_error("raw tile preview tile dimensions are inconsistent"))?;
        let destination = mosaic
            .get_mut(destination_start..destination_start + row_bytes)
            .ok_or_else(|| transport_error("raw tile preview layout is invalid"))?;
        destination.copy_from_slice(source);
    }
    Ok(())
}

fn open_artifact(path: &Path) -> CoreResult<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| transport_error("raw tile preview artifact path is unsafe"))
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|_| transport_error("raw tile preview artifact path is unsafe"))
    }
}

fn transport_error(message: &str) -> CoreError {
    CoreError::Transport(message.into())
}

fn resource_limit(message: &str) -> CoreError {
    CoreError::ResourceLimit(message.into())
}
