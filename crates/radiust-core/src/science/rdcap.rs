//! Bounded decoder for the verified RDCAP single-station CSR envelope.

use crate::errors::{CoreError, CoreResult, ProviderError};
use crate::limits::Limits;
use crate::model::{Grid, RadarField, RawFrame};
use sha2::{Digest, Sha256};

const QUALITY_MISSING: u16 = 1;

fn unverified() -> CoreError {
    CoreError::Provider(ProviderError::DecodeUnverified)
}

fn invalid_grid() -> CoreError {
    CoreError::Provider(ProviderError::InvalidGrid)
}

/// Decode a retained RDCAP raw frame only after its content binding verifies.
pub fn decode_rdcap(raw: &RawFrame, limits: &Limits) -> CoreResult<RadarField> {
    crate::source::rdcap::validate_binding(&raw.frame, &raw.artifacts)?;
    let artifact = raw
        .artifacts
        .iter()
        .find(|artifact| artifact.receipt.name == "file-response.json")
        .ok_or_else(invalid_grid)?;
    if artifact.receipt.media_type != "application/json" {
        return Err(unverified());
    }
    if artifact.receipt.size_bytes > limits.max_artifact_bytes
        || artifact.receipt.size_bytes > limits.max_frame_bytes
    {
        return Err(CoreError::ResourceLimit("RDCAP response exceeds configured limits".into()));
    }
    let metadata = std::fs::metadata(&artifact.path)
        .map_err(|_| CoreError::Storage("RDCAP response is unavailable".into()))?;
    if !metadata.is_file() || metadata.len() != artifact.receipt.size_bytes {
        return Err(invalid_grid());
    }
    let payload = std::fs::read(&artifact.path)
        .map_err(|_| CoreError::Storage("RDCAP response is unavailable".into()))?;
    if hex::encode(Sha256::digest(&payload)) != artifact.receipt.sha256 {
        return Err(invalid_grid());
    }
    let content: String = serde_json::from_slice(&payload).map_err(|_| unverified())?;
    decode_csr_text(raw, &content, limits, &artifact.receipt.sha256)
}

fn decode_csr_text(
    raw: &RawFrame,
    content: &str,
    limits: &Limits,
    content_sha256: &str,
) -> CoreResult<RadarField> {
    let palette = crate::rdcap_palette::palette();
    let lines = content.lines().collect::<Vec<_>>();
    if lines.len() != 7 {
        return Err(invalid_grid());
    }
    let header = lines[0].split(',').collect::<Vec<_>>();
    if header.len() != 11 {
        return Err(invalid_grid());
    }
    if header[2] != "T" || header[7] != "int16" || header[10] != "EPSG:4326" {
        return Err(unverified());
    }
    let width = parse_usize(header[0])?;
    let height = parse_usize(header[1])?;
    if width < 2 || height < 2 {
        return Err(invalid_grid());
    }
    let first_lon = parse_f64(header[3])?;
    let first_lat = parse_f64(header[4])?;
    let last_lon = parse_f64(header[5])?;
    let last_lat = parse_f64(header[6])?;
    let default_raw = parse_i16(header[8])?;
    let invalid_raw = parse_i16(header[9])?;
    if default_raw != -999 || invalid_raw != -999 {
        return Err(unverified());
    }
    if first_lon >= last_lon
        || first_lat == last_lat
        || [first_lon, first_lat, last_lon, last_lat].iter().any(|value| !value.is_finite())
        || first_lon < -180.0
        || last_lon > 180.0
        || first_lat.abs() > 90.0
        || last_lat.abs() > 90.0
    {
        return Err(invalid_grid());
    }

    let transform = lines[1]
        .strip_prefix("linearTransform(")
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(unverified)?;
    let (scale, offset) = transform.split_once(',').ok_or_else(unverified)?;
    let scale = parse_f64(scale)?;
    let offset = parse_f64(offset)?;
    if scale != 0.1 || offset != 0.0 {
        return Err(unverified());
    }
    validate_legend(lines[2])?;

    let cells = width.checked_mul(height).ok_or_else(invalid_grid)?;
    let cells_u64 = u64::try_from(cells).map_err(|_| invalid_grid())?;
    limits.validate_pixels(cells_u64)?;
    let ndv = parse_prefixed_usize(lines[3], "ndv")?;
    if ndv > cells {
        return Err(invalid_grid());
    }
    let expected_row_ptr = height.checked_add(1).ok_or_else(invalid_grid)?;
    let ndv_u64 = u64::try_from(ndv).map_err(|_| invalid_grid())?;
    let row_ptr_u64 = u64::try_from(expected_row_ptr).map_err(|_| invalid_grid())?;
    let decoded_bytes = cells_u64
        .checked_mul(
            (std::mem::size_of::<i16>() + std::mem::size_of::<f32>() + std::mem::size_of::<u16>())
                as u64,
        )
        .and_then(|bytes| {
            ndv_u64
                .checked_mul((std::mem::size_of::<usize>() + std::mem::size_of::<i16>()) as u64)
                .and_then(|sparse_bytes| bytes.checked_add(sparse_bytes))
        })
        .and_then(|bytes| {
            row_ptr_u64
                .checked_mul(std::mem::size_of::<usize>() as u64)
                .and_then(|row_bytes| bytes.checked_add(row_bytes))
        })
        .ok_or_else(|| CoreError::ResourceLimit("RDCAP decoded buffer size overflow".into()))?;
    if decoded_bytes > limits.max_frame_bytes || decoded_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "RDCAP decoded buffers exceed configured limits".into(),
        ));
    }

    let columns = parse_prefixed_list::<usize>(lines[4], "colIdx", ndv)?;
    let row_ptr = parse_prefixed_list::<usize>(lines[5], "rowPtr", expected_row_ptr)?;
    let sparse_values = parse_prefixed_list::<i16>(lines[6], "vals", ndv)?;
    if row_ptr.first() != Some(&0)
        || row_ptr.last() != Some(&ndv)
        || row_ptr.windows(2).any(|pair| pair[0] > pair[1] || pair[1] > ndv)
        || columns.iter().any(|column| *column >= width)
    {
        return Err(invalid_grid());
    }

    let mut dense = Vec::new();
    dense.try_reserve_exact(cells).map_err(|_| {
        CoreError::ResourceLimit("RDCAP dense buffer could not be allocated".into())
    })?;
    dense.resize(cells, default_raw);
    for row in 0..height {
        let start = row_ptr[row];
        let end = row_ptr[row + 1];
        let mut previous = None;
        for position in start..end {
            let column = columns[position];
            if previous.is_some_and(|value| column <= value) {
                return Err(invalid_grid());
            }
            previous = Some(column);
            dense[row * width + column] = sparse_values[position];
        }
    }
    if first_lat < last_lat {
        for row in 0..height / 2 {
            let opposite = height - row - 1;
            for column in 0..width {
                dense.swap(row * width + column, opposite * width + column);
            }
        }
    }

    let mut values = Vec::new();
    let mut quality = Vec::new();
    values.try_reserve_exact(cells).map_err(|_| {
        CoreError::ResourceLimit("RDCAP value buffer could not be allocated".into())
    })?;
    quality.try_reserve_exact(cells).map_err(|_| {
        CoreError::ResourceLimit("RDCAP quality buffer could not be allocated".into())
    })?;
    let mut annotation_cells = 0_usize;
    let mut missing_cells = 0_usize;
    for value in dense {
        if value == invalid_raw {
            values.push(f32::NAN);
            quality.push(QUALITY_MISSING);
            missing_cells += 1;
        } else if value == palette.annotation.raw_value {
            values.push(f32::NAN);
            quality.push(palette.annotation.quality);
            annotation_cells += 1;
        } else {
            values.push(f32::from(value) * scale as f32 + offset as f32);
            quality.push(0);
        }
    }

    let dx = (last_lon - first_lon) / (width - 1) as f64;
    let dy = (last_lat - first_lat).abs() / (height - 1) as f64;
    let west = first_lon;
    let north = first_lat.max(last_lat);
    let east = west + width as f64 * dx;
    let south = north - height as f64 * dy;
    let x = (0..width).map(|column| west + (column as f64 + 0.5) * dx).collect();
    let y = (0..height).map(|row| north - (row as f64 + 0.5) * dy).collect();
    let shape = vec![height, width];
    let station = raw.frame.station.as_deref().unwrap_or_default();
    let country = raw.frame.locator["country"].as_str().unwrap_or_default();
    let key = raw.frame.locator["key"].as_str().unwrap_or_default();
    let field = RadarField {
        name: "reflectivity".into(),
        values,
        shape: shape.clone(),
        quality,
        units: Some("dBZ".into()),
        valid_time: raw.frame.valid_time.clone(),
        grid: Grid {
            shape,
            crs: Some("EPSG:4326".into()),
            x,
            y,
            affine: Some([west, dx, 0.0, north, 0.0, -dy]),
        },
        provenance: vec![
            "source=rdcap".into(),
            "product=reflectivity".into(),
            format!("country={country}"),
            format!("station={station}"),
            format!("key={key}"),
            format!("frame_logical_id={}", raw.frame.logical_id),
            format!("raw_sha256={content_sha256}"),
            "native_crs=EPSG:4326".into(),
            "row_order=north_to_south".into(),
            "pixel_registration=T_top_left".into(),
            "decoder=rdcap-csr-v1".into(),
            format!(
                "annotation_rule={};raw={};role={};confidence={};cells={annotation_cells}",
                palette.annotation.rule_version,
                palette.annotation.raw_value,
                palette.annotation.role,
                palette.annotation.confidence,
            ),
            format!("missing_raw=-999;cells={missing_cells}"),
            format!("bounds_wsen={west},{south},{east},{north}"),
        ],
    };
    field.validate().map_err(|_| invalid_grid())?;
    Ok(field)
}

fn validate_legend(line: &str) -> CoreResult<()> {
    let palette = crate::rdcap_palette::palette();
    let tokens = line.split(',').map(parse_i32).collect::<CoreResult<Vec<_>>>()?;
    if tokens.len() != palette.classes.len() * 4 {
        return Err(unverified());
    }
    for (entry, class) in tokens.chunks_exact(4).zip(&palette.classes) {
        if entry[..3] != class.rgb.map(i32::from)
            || entry[3] != i32::from(class.raw_lower_threshold)
        {
            return Err(unverified());
        }
    }
    Ok(())
}

fn parse_prefixed_usize(line: &str, prefix: &str) -> CoreResult<usize> {
    let (name, value) = line.split_once(':').ok_or_else(invalid_grid)?;
    if name != prefix {
        return Err(invalid_grid());
    }
    parse_usize(value)
}

fn parse_prefixed_list<T: std::str::FromStr>(
    line: &str,
    prefix: &str,
    expected_count: usize,
) -> CoreResult<Vec<T>> {
    let (name, values) = line.split_once(':').ok_or_else(invalid_grid)?;
    if name != prefix {
        return Err(invalid_grid());
    }
    let token_count = if values.is_empty() {
        0
    } else {
        values.split(',').take(expected_count.saturating_add(1)).count()
    };
    if token_count != expected_count {
        return Err(invalid_grid());
    }
    let mut parsed = Vec::new();
    parsed.try_reserve_exact(expected_count).map_err(|_| {
        CoreError::ResourceLimit("RDCAP sparse buffer could not be allocated".into())
    })?;
    if expected_count != 0 {
        for value in values.split(',') {
            parsed.push(value.parse().map_err(|_| invalid_grid())?);
        }
    }
    Ok(parsed)
}

fn parse_usize(value: &str) -> CoreResult<usize> {
    value.parse().map_err(|_| invalid_grid())
}

fn parse_i16(value: &str) -> CoreResult<i16> {
    value.parse().map_err(|_| invalid_grid())
}

fn parse_i32(value: &str) -> CoreResult<i32> {
    value.parse().map_err(|_| unverified())
}

fn parse_f64(value: &str) -> CoreResult<f64> {
    let value = value.parse::<f64>().map_err(|_| unverified())?;
    if !value.is_finite() {
        return Err(unverified());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_counts_are_checked_before_parsing_or_reserving_storage() {
        struct UnexpectedParse;
        impl std::str::FromStr for UnexpectedParse {
            type Err = ();

            fn from_str(_: &str) -> Result<Self, Self::Err> {
                panic!("mismatched token counts must be rejected before parsing");
            }
        }

        let excessive = "0,".repeat(100_000);
        for (prefix, count) in [("colIdx", 0), ("rowPtr", 3), ("vals", 0)] {
            for values in [excessive.as_str(), ""] {
                if count == 0 && values.is_empty() {
                    continue;
                }
                let line = format!("{prefix}:{values}");
                assert!(matches!(
                    parse_prefixed_list::<UnexpectedParse>(&line, prefix, count),
                    Err(CoreError::Provider(ProviderError::InvalidGrid))
                ));
            }
        }
        assert!(matches!(
            parse_prefixed_list::<UnexpectedParse>("colIdx:0", "colIdx", usize::MAX),
            Err(CoreError::Provider(ProviderError::InvalidGrid))
        ));
    }
}
