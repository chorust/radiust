//! Bounded decoder for the verified RDCAP single-station CSR envelope.

use super::BudgetedRadarField;
use crate::errors::{CoreError, CoreResult, ProviderError};
use crate::limits::{Limits, RasterMemoryBudget, RasterWorkControl};
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
    let memory = limits.raster_memory_budget();
    Ok(decode_rdcap_with_memory(raw, limits, &memory)?.field)
}

pub(crate) fn decode_rdcap_with_memory(
    raw: &RawFrame,
    limits: &Limits,
    memory: &RasterMemoryBudget,
) -> CoreResult<BudgetedRadarField> {
    decode_rdcap_inner(raw, limits, memory, None)
}

pub(crate) fn decode_rdcap_with_memory_and_control(
    raw: &RawFrame,
    limits: &Limits,
    memory: &RasterMemoryBudget,
    control: &RasterWorkControl,
) -> CoreResult<BudgetedRadarField> {
    decode_rdcap_inner(raw, limits, memory, Some(control))
}

fn decode_rdcap_inner(
    raw: &RawFrame,
    limits: &Limits,
    memory: &RasterMemoryBudget,
    control: Option<&RasterWorkControl>,
) -> CoreResult<BudgetedRadarField> {
    check_raster_work(control)?;
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
    let input_bytes = metadata
        .len()
        .checked_mul(3)
        .ok_or_else(|| CoreError::ResourceLimit("RDCAP input buffer size overflow".into()))?;
    let _input_lease = memory.reserve(input_bytes)?;
    let payload = std::fs::read(&artifact.path)
        .map_err(|_| CoreError::Storage("RDCAP response is unavailable".into()))?;
    check_raster_work(control)?;
    if hex::encode(Sha256::digest(&payload)) != artifact.receipt.sha256 {
        return Err(invalid_grid());
    }
    let content: String = serde_json::from_slice(&payload).map_err(|_| unverified())?;
    check_raster_work(control)?;
    decode_csr_text_with_memory(raw, &content, limits, memory, &artifact.receipt.sha256, control)
}

fn decode_csr_text_with_memory(
    raw: &RawFrame,
    content: &str,
    limits: &Limits,
    memory: &RasterMemoryBudget,
    content_sha256: &str,
    control: Option<&RasterWorkControl>,
) -> CoreResult<BudgetedRadarField> {
    check_raster_work(control)?;
    let palette = crate::rdcap_palette::palette();
    let mut lines = content.lines();
    let line0 = lines.next().ok_or_else(invalid_grid)?;
    let line1 = lines.next().ok_or_else(invalid_grid)?;
    let line2 = lines.next().ok_or_else(invalid_grid)?;
    let line3 = lines.next().ok_or_else(invalid_grid)?;
    let line4 = lines.next().ok_or_else(invalid_grid)?;
    let line5 = lines.next().ok_or_else(invalid_grid)?;
    let line6 = lines.next().ok_or_else(invalid_grid)?;
    if lines.next().is_some() {
        return Err(invalid_grid());
    }
    let header = line0.split(',').take(12).collect::<Vec<_>>();
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

    let transform = line1
        .strip_prefix("linearTransform(")
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(unverified)?;
    let (scale, offset) = transform.split_once(',').ok_or_else(unverified)?;
    let scale = parse_f64(scale)?;
    let offset = parse_f64(offset)?;
    if scale != 0.1 || offset != 0.0 {
        return Err(unverified());
    }
    validate_legend(line2)?;

    let cells = width.checked_mul(height).ok_or_else(invalid_grid)?;
    let cells_u64 = u64::try_from(cells).map_err(|_| invalid_grid())?;
    limits.validate_pixels(cells_u64)?;
    let ndv = parse_prefixed_usize(line3, "ndv")?;
    if ndv > cells {
        return Err(invalid_grid());
    }
    let expected_row_ptr = height.checked_add(1).ok_or_else(invalid_grid)?;
    let ndv_u64 = u64::try_from(ndv).map_err(|_| invalid_grid())?;
    let row_ptr_u64 = u64::try_from(expected_row_ptr).map_err(|_| invalid_grid())?;
    let dense_bytes = cells_u64
        .checked_mul(std::mem::size_of::<i16>() as u64)
        .ok_or_else(|| CoreError::ResourceLimit("RDCAP decoded buffer size overflow".into()))?;
    let output_plane_bytes = cells_u64
        .checked_mul((std::mem::size_of::<f32>() + std::mem::size_of::<u16>()) as u64)
        .ok_or_else(|| CoreError::ResourceLimit("RDCAP decoded buffer size overflow".into()))?;
    let sparse_bytes = ndv_u64
        .checked_mul((std::mem::size_of::<usize>() + std::mem::size_of::<i16>()) as u64)
        .ok_or_else(|| CoreError::ResourceLimit("RDCAP decoded buffer size overflow".into()))?;
    let row_ptr_bytes = row_ptr_u64
        .checked_mul(std::mem::size_of::<usize>() as u64)
        .ok_or_else(|| CoreError::ResourceLimit("RDCAP decoded buffer size overflow".into()))?;
    let axis_count = width
        .checked_add(height)
        .and_then(|count| u64::try_from(count).ok())
        .ok_or_else(|| CoreError::ResourceLimit("RDCAP axis buffer size overflow".into()))?;
    let axes_bytes = axis_count
        .checked_mul(std::mem::size_of::<f64>() as u64)
        .ok_or_else(|| CoreError::ResourceLimit("RDCAP axis buffer size overflow".into()))?;
    let scratch_bytes = dense_bytes
        .checked_add(sparse_bytes)
        .and_then(|bytes| bytes.checked_add(row_ptr_bytes))
        .ok_or_else(|| CoreError::ResourceLimit("RDCAP decoded buffer size overflow".into()))?;
    let output_bytes = output_plane_bytes
        .checked_add(axes_bytes)
        .ok_or_else(|| CoreError::ResourceLimit("RDCAP decoded buffer size overflow".into()))?;
    let decoded_bytes = scratch_bytes
        .checked_add(output_bytes)
        .ok_or_else(|| CoreError::ResourceLimit("RDCAP decoded buffer size overflow".into()))?;
    if decoded_bytes > limits.max_frame_bytes || decoded_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "RDCAP decoded buffers exceed configured limits".into(),
        ));
    }
    let scratch_lease = memory.reserve(scratch_bytes)?;
    let output_lease = memory.reserve(output_bytes)?;

    let columns = parse_prefixed_list_with_control::<usize>(line4, "colIdx", ndv, control)?;
    let row_ptr =
        parse_prefixed_list_with_control::<usize>(line5, "rowPtr", expected_row_ptr, control)?;
    let sparse_values = parse_prefixed_list_with_control::<i16>(line6, "vals", ndv, control)?;
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
        check_raster_work(control)?;
        let start = row_ptr[row];
        let end = row_ptr[row + 1];
        let mut previous = None;
        for position in start..end {
            if position % 65_536 == 0 {
                check_raster_work(control)?;
            }
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
            check_raster_work(control)?;
            let opposite = height - row - 1;
            for column in 0..width {
                dense.swap(row * width + column, opposite * width + column);
            }
        }
    }
    drop(columns);
    drop(row_ptr);
    drop(sparse_values);

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
    for (index, value) in dense.into_iter().enumerate() {
        if index % 65_536 == 0 {
            check_raster_work(control)?;
        }
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
    drop(scratch_lease);

    let dx = (last_lon - first_lon) / (width - 1) as f64;
    let dy = (last_lat - first_lat).abs() / (height - 1) as f64;
    let west = first_lon;
    let north = first_lat.max(last_lat);
    let east = west + width as f64 * dx;
    let south = north - height as f64 * dy;
    let mut x = Vec::new();
    let mut y = Vec::new();
    x.try_reserve_exact(width)
        .map_err(|_| CoreError::ResourceLimit("RDCAP x-axis allocation failed".into()))?;
    y.try_reserve_exact(height)
        .map_err(|_| CoreError::ResourceLimit("RDCAP y-axis allocation failed".into()))?;
    for column in 0..width {
        x.push(west + (column as f64 + 0.5) * dx);
    }
    for row in 0..height {
        check_raster_work(control)?;
        y.push(north - (row as f64 + 0.5) * dy);
    }
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
    Ok(BudgetedRadarField { field, _leases: vec![output_lease] })
}

fn validate_legend(line: &str) -> CoreResult<()> {
    let palette = crate::rdcap_palette::palette();
    let mut tokens = line.split(',');
    for class in &palette.classes {
        let expected = [
            i32::from(class.rgb[0]),
            i32::from(class.rgb[1]),
            i32::from(class.rgb[2]),
            i32::from(class.raw_lower_threshold),
        ];
        for expected in expected {
            if parse_i32(tokens.next().ok_or_else(unverified)?)? != expected {
                return Err(unverified());
            }
        }
    }
    if tokens.next().is_some() {
        return Err(unverified());
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

#[cfg(test)]
fn parse_prefixed_list<T: std::str::FromStr>(
    line: &str,
    prefix: &str,
    expected_count: usize,
) -> CoreResult<Vec<T>> {
    parse_prefixed_list_with_control(line, prefix, expected_count, None)
}

fn parse_prefixed_list_with_control<T: std::str::FromStr>(
    line: &str,
    prefix: &str,
    expected_count: usize,
    control: Option<&RasterWorkControl>,
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
        for (index, value) in values.split(',').enumerate() {
            if index % 65_536 == 0 {
                check_raster_work(control)?;
            }
            parsed.push(value.parse().map_err(|_| invalid_grid())?);
        }
    }
    Ok(parsed)
}

fn check_raster_work(control: Option<&RasterWorkControl>) -> CoreResult<()> {
    if let Some(control) = control {
        control.check()?;
    }
    Ok(())
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
