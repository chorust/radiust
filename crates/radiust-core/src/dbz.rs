//! Strict conversion from declared integer gray codes to pixel-space dBZ.

use crate::errors::{CoreError, CoreResult};
use crate::limits::{Limits, RasterBufferLease, RasterMemoryBudget};
use crate::model::{RadarField, RawFrame};
use crate::raster::{
    AlphaPlane, EncodingBasis, GrayFrame, InputReadReceipt, ModeInfo, PixelDbzField,
    ProcessingRecord, QUALITY_MISSING, QUALITY_OUTSIDE_COVERAGE, QUALITY_UNKNOWN_COLOR,
    RASTER_SCHEMA_VERSION, RasterInput, RasterInputIdentity, RasterResult, RasterResultData,
};
use image::AnimationDecoder;
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const ENCODING: &str = "gray-dbz-v1";
const FORMULA: &str = "effective_gray * 5/16";
const SOURCE_FORMULA: &str = "min(gray,224) * 5/16";

/// Decode a Python-style gray/RGB/RGBA array through the canonical strict
/// policy. `values` is row-major and `alpha_bit_depth` records the source
/// alpha dtype (0 for inputs without alpha).
pub fn decode_gray_array(
    width: usize,
    height: usize,
    channels: u8,
    values: &[f64],
    alpha_bit_depth: u8,
) -> CoreResult<(Vec<f32>, Vec<u16>)> {
    let pixels = checked_shape(width, height, &Limits::default())?;
    if !matches!(channels, 1 | 3 | 4) {
        return Err(invalid("gray array must have 1, 3, or 4 channels", None, None, None));
    }
    let expected = pixels
        .checked_mul(usize::from(channels))
        .ok_or_else(|| CoreError::ResourceLimit("gray array shape overflows".into()))?;
    if values.len() != expected {
        return Err(invalid("gray array length does not match shape", None, None, None));
    }
    if channels == 4 && !matches!(alpha_bit_depth, 8 | 16) {
        return Err(invalid("RGBA alpha must retain uint8 or uint16 dtype", None, None, None));
    }
    if channels != 4 && alpha_bit_depth != 0 {
        return Err(invalid(
            "alpha bit depth was supplied without an alpha channel",
            None,
            None,
            None,
        ));
    }

    let mut gray = Vec::with_capacity(pixels);
    let mut alpha8 = (channels == 4 && alpha_bit_depth == 8).then(|| Vec::with_capacity(pixels));
    let mut alpha16 = (channels == 4 && alpha_bit_depth == 16).then(|| Vec::with_capacity(pixels));
    for index in 0..pixels {
        let offset = index * usize::from(channels);
        let alpha_value = if channels == 4 {
            let value = values[offset + 3];
            let max = if alpha_bit_depth == 8 { 255.0 } else { 65_535.0 };
            if !value.is_finite() || value.fract() != 0.0 || !(0.0..=max).contains(&value) {
                let (row, column) = row_column(index, width);
                return Err(invalid(
                    "alpha must be an integer matching its uint8 or uint16 dtype",
                    Some(row),
                    Some(column),
                    Some(&safe_number(value)),
                ));
            }
            value as u16
        } else {
            u16::MAX
        };
        if let Some(plane) = &mut alpha8 {
            plane.push(alpha_value as u8);
        }
        if let Some(plane) = &mut alpha16 {
            plane.push(alpha_value);
        }
        let hidden = channels == 4 && alpha_value == 0;
        if hidden {
            gray.push(0.0);
            continue;
        }
        let code = values[offset];
        if channels >= 3 {
            let green = values[offset + 1];
            let blue = values[offset + 2];
            if !code.is_finite() || !green.is_finite() || !blue.is_finite() {
                let (row, column) = row_column(index, width);
                return Err(invalid(
                    "visible RGB channels must be finite grayscale values",
                    Some(row),
                    Some(column),
                    None,
                ));
            }
            if code != green || green != blue {
                return Err(non_gray(index, width));
            }
        }
        gray.push(code);
    }
    let alpha = if let Some(values) = alpha8 {
        Some(AlphaPlane::U8(values))
    } else {
        alpha16.map(AlphaPlane::U16)
    };
    let result = decode_gray_values(width, height, &gray, alpha, ENCODING)?;
    let RasterResultData::Pixel(field) = result.data else {
        return Err(invalid("gray array decoder returned a non-pixel result", None, None, None));
    };
    Ok((field.values.to_vec(), field.quality.to_vec()))
}

/// Convert an evidence-bound source gray frame to pixel-space dBZ. Source
/// gray values above 224 are clipped only in the numeric output; the original
/// RGBA pixels and quality masks remain unchanged.
pub fn decode_verified_gray_frame(gray: GrayFrame, limits: &Limits) -> CoreResult<RasterResult> {
    decode_verified_gray_frame_inner(gray, limits, None)
}

pub(crate) fn decode_verified_gray_frame_with_cancel(
    gray: GrayFrame,
    limits: &Limits,
    cancellation: &CancellationToken,
) -> CoreResult<RasterResult> {
    decode_verified_gray_frame_inner(gray, limits, Some(cancellation))
}

fn decode_verified_gray_frame_inner(
    gray: GrayFrame,
    limits: &Limits,
    cancellation: Option<&CancellationToken>,
) -> CoreResult<RasterResult> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Err(CoreError::Cancelled);
    }
    gray.validate()
        .map_err(|_| invalid("verified source gray frame is malformed", None, None, None))?;
    let rule = match &gray.encoding_basis {
        EncodingBasis::VerifiedSourceRule { encoding_id, encoding_version, rule }
            if encoding_id == ENCODING && *encoding_version == 1 =>
        {
            rule.clone()
        }
        _ => {
            return Err(invalid(
                "source gray frame has no verified gray-dBZ encoding rule",
                None,
                None,
                None,
            ));
        }
    };
    if !matches!(&gray.input, RasterInput::Source { .. }) {
        return Err(invalid("verified gray frame is not source-bound", None, None, None));
    }

    let count = checked_shape(gray.width as usize, gray.height as usize, limits)?;
    let _lease = limits.raster_memory_budget().reserve_shape(
        u64::from(gray.height),
        u64::from(gray.width),
        &[4, 2, 2, 1, 2, 2, 4, 1],
    )?;
    let mut values = Vec::with_capacity(count);
    let mut quality = gray.quality.clone();
    let mut origin_quality = gray.origin_quality.clone().unwrap_or_else(|| quality.clone());
    let mut encoding_adjustment = vec![0_u8; count];
    let mut clipped_pixel_count = 0_u64;
    let mut valid_clipped_pixel_count = 0_u64;
    let invalid_mask = QUALITY_MISSING | QUALITY_OUTSIDE_COVERAGE | QUALITY_UNKNOWN_COLOR;

    for index in 0..count {
        if index.is_multiple_of(8192) && cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err(CoreError::Cancelled);
        }
        let offset = index * 4;
        let pixel = &gray.rgba[offset..offset + 4];
        if pixel[0] != pixel[1] || pixel[1] != pixel[2] {
            return Err(invalid(
                "verified source gray output contains a visible non-gray pixel",
                Some(index / gray.width as usize),
                Some(index % gray.width as usize),
                None,
            ));
        }
        if gray.alpha.as_ref().and_then(|alpha| alpha.is_zero(index)) == Some(true) {
            quality[index] |= QUALITY_MISSING;
            origin_quality[index] |= QUALITY_MISSING;
        }
        let code = pixel[0];
        if code > 224 {
            encoding_adjustment[index] = 1;
            clipped_pixel_count += 1;
            if quality[index] & invalid_mask == 0 {
                valid_clipped_pixel_count += 1;
            }
        }
        if quality[index] & invalid_mask != 0 {
            values.push(f32::NAN);
        } else {
            values.push(f32::from(code.min(224)) * (5.0 / 16.0));
        }
    }

    let mut gray_digest = Sha256::new();
    gray_digest.update(b"radiust-verified-source-gray-rgba-v1\0");
    gray_digest.update(gray.width.to_le_bytes());
    gray_digest.update(gray.height.to_le_bytes());
    gray_digest.update(&gray.rgba);
    let gray_rgba_sha256 = hex::encode(gray_digest.finalize());
    let frame_index = gray.frame_index;
    let input = gray.input;
    let basis = gray.encoding_basis;
    let mut limitations = vec![
        "gray_encoding_quantization_is_not_reversible".to_owned(),
        "geolocation_unknown".to_owned(),
    ];
    let operations = rule
        .ordered_steps
        .iter()
        .filter_map(|step| step.get("op").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    if operations.iter().any(|operation| matches!(*operation, "crop" | "legacy_crop")) {
        limitations.push("crop_transform_has_no_verified_geospatial_mapping".into());
    }
    if operations.contains(&"gap_repair") {
        limitations.push("gray_gap_repair_is_not_invertible".into());
    }
    if operations.contains(&"resize") {
        limitations.push("gray_resampling_is_not_invertible".into());
    }
    if gray.alpha.is_none() {
        limitations.push("original_alpha_plane_not_available_at_output_shape".into());
    }
    let mut steps = vec![
        json!({"method":"verified_source_gray","source":rule.source,"product":rule.product,"path_id":rule.path_id,"rule_version":rule.rule_version,"config_hash":rule.config_hash,"evidence_ref":rule.evidence_ref,"ordered_steps":rule.ordered_steps}),
        json!({"method":"preserve_gray_rgba","sha256":gray_rgba_sha256,"width":gray.width,"height":gray.height}),
        json!({"method":"source_upper_clip","maximum_gray":224,"formula":SOURCE_FORMULA}),
    ];
    if let Some(frame_index) = frame_index {
        steps.push(json!({"method":"select_source_frame","frame_index":frame_index}));
    }
    let valid_time = gray.valid_time;
    let geometry = gray.geometry;
    let width = gray.width as usize;
    let height = gray.height as usize;
    let alpha = gray.alpha;
    let processing = ProcessingRecord {
        schema_version: RASTER_SCHEMA_VERSION,
        method: "verified_source_gray_dbz".into(),
        input_identity: serde_json::to_value(&input)
            .unwrap_or_else(|_| json!({"kind":"source_gray"})),
        encoding_basis: Some(basis.clone()),
        range_policy: Some("source-upper-clip-v1".into()),
        decoder_version: Some("1".into()),
        quality_policy_version: Some("source-gray-quality-v1".into()),
        formula: Some(SOURCE_FORMULA.into()),
        quantization_step: Some(5.0 / 16.0),
        alpha_bit_depth: alpha.as_ref().map(AlphaPlane::bit_depth),
        steps,
        limitations: limitations.clone(),
        clipped_pixel_count: Some(clipped_pixel_count),
        valid_clipped_pixel_count: Some(valid_clipped_pixel_count),
        upstream: None,
    };
    let field = PixelDbzField {
        variable: "reflectivity".into(),
        units: "dBZ".into(),
        width,
        height,
        values,
        quality,
        origin_quality: Some(origin_quality),
        encoding_adjustment: Some(encoding_adjustment),
        alpha,
        valid_time: valid_time.clone(),
        geometry: geometry.clone(),
        processing: processing.clone(),
    };
    field
        .validate()
        .map_err(|_| invalid("source dBZ result failed validation", None, None, None))?;
    let mode_info = ModeInfo {
        requested: Some("dbz".into()),
        actual: Some("dbz".into()),
        variable: Some("reflectivity".into()),
        units: Some("dBZ".into()),
        method: Some("verified_source_gray_dbz".into()),
        encoding: Some(ENCODING.into()),
        rule_version: Some(rule.rule_version.clone()),
        range_policy: Some("source-upper-clip-v1".into()),
        clipped_pixel_count: Some(clipped_pixel_count),
        valid_clipped_pixel_count: Some(valid_clipped_pixel_count),
        time_status: Some(if valid_time.is_some() { "source" } else { "unknown" }.into()),
        geolocation: Some(if geometry.is_some() { "verified" } else { "unknown" }.into()),
        limitations,
    };
    Ok(RasterResult {
        input,
        data: RasterResultData::Pixel(Arc::new(field)),
        processing,
        mode_info,
    })
}

/// Wrap an already-decoded native reflectivity field without changing its
/// values, quality, coordinate arrays, or allocation.
pub fn wrap_native_reflectivity(raw: &RawFrame, field: RadarField) -> CoreResult<RasterResult> {
    if field.name != "reflectivity" || field.units.as_deref() != Some("dBZ") {
        return Err(CoreError::UnitMismatch { variable: field.name, units: field.units });
    }
    field
        .validate()
        .map_err(|_| CoreError::Transport("native reflectivity field is malformed".into()))?;
    if field.valid_time != raw.frame.valid_time {
        return Err(CoreError::Transport(
            "native reflectivity time does not match the acquired frame".into(),
        ));
    }
    raw.frame
        .validate_identity()
        .map_err(|_| CoreError::Transport("native source frame identity is invalid".into()))?;
    let resolved_revision = crate::identity::resolved_revision(&[], raw.frame.revision.as_deref())
        .map_err(|_| CoreError::Transport("source acquisition revision is unavailable".into()))?;
    let input = RasterInput::Source {
        frame: raw.frame.clone(),
        resolved_revision,
        acquisition_receipt: raw.public_receipt(),
    };
    let has_geometry = field.grid.crs.is_some()
        && (field.grid.affine.is_some() || (!field.grid.x.is_empty() && !field.grid.y.is_empty()));
    let mut steps =
        field.provenance.iter().cloned().map(serde_json::Value::String).collect::<Vec<_>>();
    steps.push(json!({"method":"wrap_native_reflectivity","values_unchanged":true}));
    let processing = ProcessingRecord {
        schema_version: RASTER_SCHEMA_VERSION,
        method: "native_dbz".into(),
        input_identity: serde_json::to_value(&input)
            .unwrap_or_else(|_| json!({"kind":"source_native"})),
        encoding_basis: None,
        range_policy: Some("native".into()),
        decoder_version: field
            .provenance
            .iter()
            .find_map(|value| value.strip_prefix("decoder=").map(str::to_owned)),
        quality_policy_version: None,
        formula: None,
        quantization_step: None,
        alpha_bit_depth: None,
        steps,
        limitations: Vec::new(),
        clipped_pixel_count: None,
        valid_clipped_pixel_count: None,
        upstream: None,
    };
    let mode_info = ModeInfo {
        requested: Some("dbz".into()),
        actual: Some("dbz".into()),
        variable: Some("reflectivity".into()),
        units: Some("dBZ".into()),
        method: Some("native_dbz".into()),
        encoding: None,
        rule_version: None,
        range_policy: Some("native".into()),
        clipped_pixel_count: None,
        valid_clipped_pixel_count: None,
        time_status: Some("source".into()),
        geolocation: Some(if has_geometry { "verified" } else { "unknown" }.into()),
        limitations: Vec::new(),
    };
    Ok(RasterResult {
        input,
        data: RasterResultData::Native(Arc::new(field)),
        processing,
        mode_info,
    })
}

/// Historical array behavior retained for the compatibility decoder only.
/// It intentionally narrows finite 0..255 values to uint8 before applying the
/// former strict/unknown-color policy.
pub fn decode_gray_array_historical(
    width: usize,
    height: usize,
    channels: u8,
    values: &[f64],
    strict: bool,
    max_gray: i64,
) -> CoreResult<(Vec<f32>, Vec<u16>)> {
    let pixels = checked_shape(width, height, &Limits::default())?;
    if !matches!(channels, 1 | 3 | 4) {
        return Err(invalid("legacy gray input must have 1, 3, or 4 channels", None, None, None));
    }
    if !(0..=255).contains(&max_gray) {
        return Err(invalid("max_gray must be between 0 and 255", None, None, None));
    }
    let expected = pixels
        .checked_mul(usize::from(channels))
        .ok_or_else(|| CoreError::ResourceLimit("legacy gray array shape overflows".into()))?;
    if values.len() != expected {
        return Err(invalid("legacy gray input shape is inconsistent", None, None, None));
    }
    if values.iter().any(|value| !value.is_finite() || !(0.0..=255.0).contains(value)) {
        return Err(invalid(
            "legacy gray pixels must be finite values in the range 0..255",
            None,
            None,
            None,
        ));
    }

    let mut decoded = vec![f32::NAN; pixels];
    let mut quality = vec![0_u16; pixels];
    let mut unknown_count = 0_usize;
    for index in 0..pixels {
        let offset = index * usize::from(channels);
        let alpha = if channels == 4 { values[offset + 3] as u8 } else { 255 };
        if alpha == 0 {
            quality[index] |= QUALITY_MISSING;
            continue;
        }
        let gray = values[offset] as u8;
        let grayscale = channels < 3
            || (values[offset] == values[offset + 1] && values[offset + 1] == values[offset + 2]);
        if !grayscale || i64::from(gray) > max_gray {
            quality[index] |= QUALITY_UNKNOWN_COLOR;
            unknown_count += 1;
            continue;
        }
        decoded[index] = f32::from(gray) / 16.0 * 5.0;
    }
    if strict && unknown_count != 0 {
        return Err(invalid(
            &format!("{unknown_count} pixel(s) are not valid legacy grayscale reflectivity"),
            None,
            None,
            None,
        ));
    }
    Ok((decoded, quality))
}

struct GrayPixels {
    width: usize,
    height: usize,
    codes: Vec<u16>,
    alpha: Option<AlphaPlane>,
    source_bit_depth: u8,
    source_dtype: &'static str,
    channels: u8,
}

/// Decode a declared gray integer array using the strict local policy.
pub fn decode_gray_values(
    width: usize,
    height: usize,
    values: &[f64],
    alpha: Option<AlphaPlane>,
    declared_encoding: &str,
) -> CoreResult<RasterResult> {
    decode_gray_values_with_limits(
        width,
        height,
        values,
        alpha,
        declared_encoding,
        &Limits::default(),
    )
}

pub(crate) fn decode_gray_values_with_limits(
    width: usize,
    height: usize,
    values: &[f64],
    alpha: Option<AlphaPlane>,
    declared_encoding: &str,
    limits: &Limits,
) -> CoreResult<RasterResult> {
    if declared_encoding != ENCODING {
        return Err(invalid("unsupported declared encoding", None, None, Some(declared_encoding)));
    }
    let pixels = checked_shape(width, height, limits)?;
    if values.len() != pixels {
        return Err(invalid("array length does not match shape", None, None, None));
    }
    if let Some(plane) = &alpha {
        if plane.len() != pixels {
            return Err(invalid("alpha length does not match shape", None, None, None));
        }
    }

    let alpha_bytes = alpha.as_ref().map_or(0, |plane| match plane {
        AlphaPlane::U8(_) => 1,
        AlphaPlane::U16(_) => 2,
    });
    let _lease = limits.raster_memory_budget().reserve_shape(
        height as u64,
        width as u64,
        &[8, 2, 4, 2, 1, alpha_bytes],
    )?;

    let mut codes = Vec::with_capacity(pixels);
    for (index, value) in values.iter().copied().enumerate() {
        if alpha.as_ref().and_then(|plane| plane.is_zero(index)) == Some(true) {
            codes.push(0);
            continue;
        }
        if !value.is_finite() || value.fract() != 0.0 || !(0.0..=224.0).contains(&value) {
            let (row, column) = row_column(index, width);
            return Err(invalid(
                "gray code must be a finite integer from 0 through 224",
                Some(row),
                Some(column),
                Some(&safe_number(value)),
            ));
        }
        codes.push(value as u16);
    }

    let mut hasher = Sha256::new();
    hasher.update(b"radiust-local-gray-values-v1\0");
    hasher.update((width as u64).to_le_bytes());
    hasher.update((height as u64).to_le_bytes());
    hasher.update(ENCODING.as_bytes());
    for value in values {
        hasher.update(value.to_le_bytes());
    }
    if let Some(plane) = &alpha {
        match plane {
            AlphaPlane::U8(values) => {
                hasher.update([8]);
                hasher.update(values);
            }
            AlphaPlane::U16(values) => {
                hasher.update([16]);
                for value in values {
                    hasher.update(value.to_le_bytes());
                }
            }
        }
    } else {
        hasher.update([0]);
    }
    let digest = hex::encode(hasher.finalize());
    let size_bytes = (values.len() as u64)
        .checked_mul(8)
        .and_then(|size| size.checked_add((pixels as u64) * alpha_bytes))
        .ok_or_else(|| CoreError::ResourceLimit("gray array byte count overflows".into()))?;
    let pixels = GrayPixels {
        width,
        height,
        codes,
        alpha,
        source_bit_depth: 64,
        source_dtype: "f64",
        channels: if alpha_bytes == 0 { 1 } else { 2 },
    };
    build_result(pixels, digest, size_bytes.max(1), "application/x-radiust-gray-values", None)
}

/// Read one explicitly declared PNG, GIF, or WebP gray image and decode it.
pub fn decode_gray_file(
    path: impl AsRef<Path>,
    frame_index: Option<u32>,
) -> CoreResult<RasterResult> {
    decode_gray_file_with_limits(path.as_ref(), frame_index, &Limits::default())
}

pub(crate) fn decode_gray_file_with_limits(
    path: &Path,
    frame_index: Option<u32>,
    limits: &Limits,
) -> CoreResult<RasterResult> {
    let metadata = fs::metadata(path)
        .map_err(|_| CoreError::Temporary("local image could not be read".into()))?;
    let size = metadata.len();
    limits.validate_bytes(size, size)?;
    if size == 0 {
        return Err(invalid("image is empty or corrupt", None, None, None));
    }
    let size_usize = usize::try_from(size)
        .map_err(|_| CoreError::ResourceLimit("local image size exceeds this platform".into()))?;
    let memory = limits.raster_memory_budget();
    let file_lease = memory.reserve(size)?;
    let mut bytes = Vec::with_capacity(size_usize);
    let file = fs::File::open(path)
        .map_err(|_| CoreError::Temporary("local image could not be read".into()))?;
    use std::io::Read;
    file.take(size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| CoreError::Temporary("local image could not be read".into()))?;
    if bytes.len() as u64 != size {
        return Err(CoreError::ResourceLimit(
            "local image changed or exceeded its size limit".into(),
        ));
    }
    let digest = hex::encode(Sha256::digest(&bytes));
    if contains_science_annotation(&bytes) {
        return Err(invalid("file metadata identifies a scientific rendering", None, None, None));
    }

    let (pixels, media_type, selected_frame, raster_lease) =
        decode_image_bytes(&bytes, frame_index, limits, &memory)?;
    let result = build_result(pixels, digest, size, media_type, selected_frame);
    drop(raster_lease);
    drop(file_lease);
    result
}

fn decode_image_bytes(
    bytes: &[u8],
    frame_index: Option<u32>,
    limits: &Limits,
    memory: &RasterMemoryBudget,
) -> CoreResult<(GrayPixels, &'static str, Option<u32>, RasterBufferLease)> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| invalid("image format is not recognized", None, None, None))?;
    let format = reader
        .format()
        .ok_or_else(|| invalid("image format is not recognized", None, None, None))?;
    let media_type = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",
        _ => return Err(invalid("only PNG, GIF, and WebP images are supported", None, None, None)),
    };
    match format {
        ImageFormat::Gif => {
            let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
                .map_err(|_| invalid("image is corrupt or unsupported", None, None, None))?;
            let (width, height) = decoder.dimensions();
            checked_shape(width as usize, height as usize, limits)?;
            decoder.set_limits(image_limits(limits)).map_err(|_| {
                CoreError::ResourceLimit("GIF exceeds configured decoder limits".into())
            })?;
            let lease = reserve_image_shape(
                memory,
                height,
                width,
                u64::from(decoder.color_type().bytes_per_pixel()),
            )?;
            let (image, selected_frame) =
                select_animation_frame(decoder.into_frames(), frame_index)?;
            let image = DynamicImage::ImageRgba8(image);
            Ok((pixels_from_dynamic(image)?, media_type, selected_frame, lease))
        }
        ImageFormat::WebP => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
                .map_err(|_| invalid("image is corrupt or unsupported", None, None, None))?;
            if decoder.has_animation() {
                let (width, height) = decoder.dimensions();
                checked_shape(width as usize, height as usize, limits)?;
                decoder.set_limits(image_limits(limits)).map_err(|_| {
                    CoreError::ResourceLimit("WebP exceeds configured decoder limits".into())
                })?;
                let lease = reserve_image_shape(memory, height, width, 4)?;
                let (image, selected_frame) =
                    select_animation_frame(decoder.into_frames(), frame_index)?;
                let image = DynamicImage::ImageRgba8(image);
                Ok((pixels_from_dynamic(image)?, media_type, selected_frame, lease))
            } else {
                if frame_index.is_some_and(|index| index != 0) {
                    return Err(invalid("selected image frame does not exist", None, None, None));
                }
                let (image, lease) = decode_static(bytes, format, limits, memory)?;
                Ok((pixels_from_dynamic(image)?, media_type, frame_index, lease))
            }
        }
        ImageFormat::Png => {
            let is_apng = png_is_animated(bytes);
            if is_apng {
                let decoder = image::codecs::png::PngDecoder::with_limits(
                    Cursor::new(bytes),
                    image_limits(limits),
                )
                .map_err(|_| invalid("image is corrupt or unsupported", None, None, None))?;
                let (width, height) = decoder.dimensions();
                checked_shape(width as usize, height as usize, limits)?;
                let lease = reserve_image_shape(memory, height, width, 4)?;
                let frames = decoder
                    .apng()
                    .map_err(|_| invalid("image animation is corrupt", None, None, None))?;
                let (image, selected_frame) =
                    select_animation_frame(frames.into_frames(), frame_index)?;
                let image = DynamicImage::ImageRgba8(image);
                Ok((pixels_from_dynamic(image)?, media_type, selected_frame, lease))
            } else {
                if frame_index.is_some_and(|index| index != 0) {
                    return Err(invalid("selected image frame does not exist", None, None, None));
                }
                reader.limits(image_limits(limits));
                let decoder = reader
                    .into_decoder()
                    .map_err(|_| invalid("image is corrupt or unsupported", None, None, None))?;
                let (width, height) = decoder.dimensions();
                checked_shape(width as usize, height as usize, limits)?;
                let lease = reserve_image_shape(
                    memory,
                    height,
                    width,
                    u64::from(decoder.color_type().bytes_per_pixel()),
                )?;
                let image = DynamicImage::from_decoder(decoder)
                    .map_err(|_| invalid("image is corrupt or unsupported", None, None, None))?;
                Ok((pixels_from_dynamic(image)?, media_type, frame_index, lease))
            }
        }
        _ => unreachable!(),
    }
}

fn select_animation_frame<'a, I>(
    mut frames: I,
    frame_index: Option<u32>,
) -> CoreResult<(image::RgbaImage, Option<u32>)>
where
    I: Iterator<Item = image::ImageResult<image::Frame>>,
{
    if let Some(requested) = frame_index {
        for index in 0..=requested {
            let frame = frames
                .next()
                .ok_or_else(|| invalid("selected image frame does not exist", None, None, None))?
                .map_err(|_| invalid("image frame is corrupt", None, None, None))?;
            if index == requested {
                return Ok((frame.into_buffer(), Some(index)));
            }
        }
        unreachable!()
    }
    let first = frames
        .next()
        .ok_or_else(|| invalid("image contains no frame", None, None, None))?
        .map_err(|_| invalid("image frame is corrupt", None, None, None))?;
    if frames.next().is_some() {
        return Err(invalid("animated image requires an explicit frame index", None, None, None));
    }
    Ok((first.into_buffer(), None))
}

fn image_limits(limits: &Limits) -> image::Limits {
    let mut image_limits = image::Limits::default();
    image_limits.max_image_width = Some(u32::MAX);
    image_limits.max_image_height = Some(u32::MAX);
    image_limits.max_alloc = Some(limits.max_temp_bytes);
    image_limits
}

fn decode_static(
    bytes: &[u8],
    format: ImageFormat,
    limits: &Limits,
    memory: &RasterMemoryBudget,
) -> CoreResult<(DynamicImage, RasterBufferLease)> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(image_limits(limits));
    let decoder = reader
        .into_decoder()
        .map_err(|_| invalid("image is corrupt or unsupported", None, None, None))?;
    let (width, height) = decoder.dimensions();
    checked_shape(width as usize, height as usize, limits)?;
    let lease = reserve_image_shape(
        memory,
        height,
        width,
        u64::from(decoder.color_type().bytes_per_pixel()),
    )?;
    let image = DynamicImage::from_decoder(decoder)
        .map_err(|_| invalid("image is corrupt or unsupported", None, None, None))?;
    Ok((image, lease))
}

fn reserve_image_shape(
    memory: &RasterMemoryBudget,
    height: u32,
    width: u32,
    decoded_bytes_per_pixel: u64,
) -> CoreResult<RasterBufferLease> {
    memory.reserve_shape(
        u64::from(height),
        u64::from(width),
        &[decoded_bytes_per_pixel, 2, 4, 2, 1, 2],
    )
}

fn pixels_from_dynamic(image: DynamicImage) -> CoreResult<GrayPixels> {
    use image::DynamicImage::*;
    let (width, height) = image.dimensions();
    let width = width as usize;
    let height = height as usize;
    let count = width
        .checked_mul(height)
        .ok_or_else(|| CoreError::ResourceLimit("image shape overflows".into()))?;
    let mut codes = Vec::with_capacity(count);
    let (alpha, source_bit_depth, source_dtype, channels) = match image {
        ImageLuma8(buffer) => {
            codes.extend(buffer.into_raw().into_iter().map(u16::from));
            (None, 8, "u8", 1)
        }
        ImageLumaA8(buffer) => {
            let mut alpha = Vec::with_capacity(count);
            for pair in buffer.into_raw().chunks_exact(2) {
                codes.push(u16::from(pair[0]));
                alpha.push(pair[1]);
            }
            (Some(AlphaPlane::U8(alpha)), 8, "u8", 2)
        }
        ImageRgb8(buffer) => {
            for (index, rgb) in buffer.into_raw().chunks_exact(3).enumerate() {
                if rgb[0] != rgb[1] || rgb[1] != rgb[2] {
                    return Err(non_gray(index, width));
                }
                codes.push(u16::from(rgb[0]));
            }
            (None, 8, "u8", 3)
        }
        ImageRgba8(buffer) => {
            let mut alpha = Vec::with_capacity(count);
            for (index, rgba) in buffer.into_raw().chunks_exact(4).enumerate() {
                if rgba[3] != 0 && (rgba[0] != rgba[1] || rgba[1] != rgba[2]) {
                    return Err(non_gray(index, width));
                }
                codes.push(u16::from(rgba[0]));
                alpha.push(rgba[3]);
            }
            (Some(AlphaPlane::U8(alpha)), 8, "u8", 4)
        }
        ImageLuma16(buffer) => {
            codes = buffer.into_raw();
            (None, 16, "u16", 1)
        }
        ImageLumaA16(buffer) => {
            let mut alpha = Vec::with_capacity(count);
            for pair in buffer.into_raw().chunks_exact(2) {
                codes.push(pair[0]);
                alpha.push(pair[1]);
            }
            (Some(AlphaPlane::U16(alpha)), 16, "u16", 2)
        }
        ImageRgb16(buffer) => {
            for (index, rgb) in buffer.into_raw().chunks_exact(3).enumerate() {
                if rgb[0] != rgb[1] || rgb[1] != rgb[2] {
                    return Err(non_gray(index, width));
                }
                codes.push(rgb[0]);
            }
            (None, 16, "u16", 3)
        }
        ImageRgba16(buffer) => {
            let mut alpha = Vec::with_capacity(count);
            for (index, rgba) in buffer.into_raw().chunks_exact(4).enumerate() {
                if rgba[3] != 0 && (rgba[0] != rgba[1] || rgba[1] != rgba[2]) {
                    return Err(non_gray(index, width));
                }
                codes.push(rgba[0]);
                alpha.push(rgba[3]);
            }
            (Some(AlphaPlane::U16(alpha)), 16, "u16", 4)
        }
        _ => {
            return Err(invalid(
                "image pixel type is not an 8/16-bit integer gray format",
                None,
                None,
                None,
            ));
        }
    };
    if codes.len() != count {
        return Err(invalid("decoded image shape is inconsistent", None, None, None));
    }
    if let Some(alpha) = &alpha {
        alpha
            .validate(count)
            .map_err(|_| invalid("decoded alpha shape is inconsistent", None, None, None))?;
    }
    for (index, code) in codes.iter_mut().enumerate() {
        let visible = alpha.as_ref().and_then(|plane| plane.is_zero(index)) != Some(true);
        if visible && *code > 224 {
            let (row, column) = row_column(index, width);
            return Err(invalid(
                "visible gray code must be from 0 through 224",
                Some(row),
                Some(column),
                Some(&code.to_string()),
            ));
        }
        if !visible {
            // Hidden transparent samples do not contribute to scientific values.
            *code = 0;
        }
    }
    Ok(GrayPixels { width, height, codes, alpha, source_bit_depth, source_dtype, channels })
}

fn build_result(
    pixels: GrayPixels,
    content_sha256: String,
    size_bytes: u64,
    media_type: &str,
    frame_index: Option<u32>,
) -> CoreResult<RasterResult> {
    let count = checked_shape(pixels.width, pixels.height, &Limits::default())?;
    let mut values = Vec::with_capacity(count);
    let mut quality = vec![0_u16; count];
    for (index, code) in pixels.codes.iter().copied().enumerate() {
        let missing = pixels.alpha.as_ref().and_then(|alpha| alpha.is_zero(index)) == Some(true);
        if missing {
            values.push(f32::NAN);
            quality[index] |= QUALITY_MISSING;
        } else {
            values.push(f32::from(code) * (5.0 / 16.0));
        }
    }
    let identity = RasterInputIdentity {
        kind: "local_gray".into(),
        content_sha256: content_sha256.clone(),
        encoding_declared: ENCODING.into(),
        valid_time: None,
        geometry: None,
    };
    let receipt = InputReadReceipt {
        content_sha256,
        size_bytes,
        media_type: media_type.into(),
        width: u32::try_from(pixels.width)
            .map_err(|_| CoreError::ResourceLimit("image width exceeds supported range".into()))?,
        height: u32::try_from(pixels.height)
            .map_err(|_| CoreError::ResourceLimit("image height exceeds supported range".into()))?,
        source_bit_depth: pixels.source_bit_depth,
        source_dtype: pixels.source_dtype.into(),
        channels: pixels.channels,
        frame_index,
    };
    let input = RasterInput::Local { identity, read_receipt: receipt };
    let basis =
        EncodingBasis::UserDeclaration { encoding_id: ENCODING.into(), encoding_version: 1 };
    let processing = ProcessingRecord {
        schema_version: RASTER_SCHEMA_VERSION,
        method: "local_gray_dbz".into(),
        input_identity: serde_json::to_value(&input)
            .unwrap_or_else(|_| json!({"kind":"local_gray"})),
        encoding_basis: Some(basis.clone()),
        range_policy: Some("strict-v1".into()),
        decoder_version: Some("1".into()),
        quality_policy_version: Some("1".into()),
        formula: Some(FORMULA.into()),
        quantization_step: Some(5.0 / 16.0),
        alpha_bit_depth: pixels.alpha.as_ref().map(AlphaPlane::bit_depth),
        steps: vec![json!({"method":"gray_to_dbz","formula":FORMULA})],
        limitations: vec!["time_unknown".into(), "geolocation_unknown".into()],
        clipped_pixel_count: Some(0),
        valid_clipped_pixel_count: Some(0),
        upstream: None,
    };
    let field = PixelDbzField {
        variable: "reflectivity".into(),
        units: "dBZ".into(),
        width: pixels.width,
        height: pixels.height,
        values,
        quality,
        origin_quality: None,
        encoding_adjustment: Some(vec![0; count]),
        alpha: pixels.alpha,
        valid_time: None,
        geometry: None,
        processing: processing.clone(),
    };
    field.validate().map_err(|_| invalid("decoded result failed validation", None, None, None))?;
    let mode_info = ModeInfo {
        requested: Some("dbz".into()),
        actual: Some("dbz".into()),
        variable: Some("reflectivity".into()),
        units: Some("dBZ".into()),
        method: Some("local_gray_dbz".into()),
        encoding: Some(ENCODING.into()),
        rule_version: None,
        range_policy: Some("strict-v1".into()),
        clipped_pixel_count: Some(0),
        valid_clipped_pixel_count: Some(0),
        time_status: Some("unknown".into()),
        geolocation: Some("unknown".into()),
        limitations: vec!["time_unknown".into(), "geolocation_unknown".into()],
    };
    Ok(RasterResult {
        input,
        data: RasterResultData::Pixel(Arc::new(field)),
        processing,
        mode_info,
    })
}

fn checked_shape(width: usize, height: usize, limits: &Limits) -> CoreResult<usize> {
    if width == 0 || height == 0 {
        return Err(invalid("shape dimensions must be positive", None, None, None));
    }
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| CoreError::ResourceLimit("gray shape overflows".into()))?;
    limits.validate_pixels(pixels as u64)?;
    if u32::try_from(width).is_err() || u32::try_from(height).is_err() {
        return Err(CoreError::ResourceLimit("gray shape exceeds supported dimensions".into()));
    }
    Ok(pixels)
}

fn invalid(
    reason: &str,
    row: Option<usize>,
    column: Option<usize>,
    value: Option<&str>,
) -> CoreError {
    CoreError::InvalidGrayEncoding {
        reason: reason.into(),
        row,
        column,
        value: value.map(str::to_owned),
    }
}

fn row_column(index: usize, width: usize) -> (usize, usize) {
    (index / width, index % width)
}

fn safe_number(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value == f64::INFINITY {
        "Infinity".into()
    } else if value == f64::NEG_INFINITY {
        "-Infinity".into()
    } else {
        format!("{value}")
    }
}

fn non_gray(index: usize, width: usize) -> CoreError {
    let (row, column) = row_column(index, width);
    invalid("visible RGB channels are not equal", Some(row), Some(column), None)
}

fn png_is_animated(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return false;
    }
    let mut offset = 8_usize;
    while offset.checked_add(12).is_some_and(|end| end <= bytes.len()) {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let Some(end) = offset.checked_add(12).and_then(|value| value.checked_add(length)) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        let kind = &bytes[offset + 4..offset + 8];
        if kind == b"acTL" {
            return true;
        }
        if kind == b"IDAT" {
            return false;
        }
        offset = end;
    }
    false
}

fn contains_science_annotation(bytes: &[u8]) -> bool {
    ["reflectivity", "dbz", "scientific-result", "scientific_result"].iter().any(|marker| {
        bytes.windows(marker.len()).any(|window| window.eq_ignore_ascii_case(marker.as_bytes()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::RasterResultData;

    #[test]
    fn zero_gray_is_a_valid_measurement_without_alpha() {
        let result = decode_gray_values(1, 1, &[0.0], None, ENCODING).unwrap();
        let RasterResultData::Pixel(field) = result.data else { panic!("expected pixel field") };
        assert_eq!(field.values, [0.0]);
        assert_eq!(field.quality, [0]);
    }
}
