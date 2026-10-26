//! Verified source-to-field decoders owned by the Rust core.

pub use crate::dbz::wrap_native_reflectivity;
use crate::errors::{CoreError, CoreResult};
use crate::limits::{Limits, RasterBufferLease, RasterMemoryBudget, RasterWorkControl};
use crate::model::{Grid, RadarField, RawFrame};
use crate::source::rainviewer::{self, TILE_SIZE};
use crate::source::tile_plan::PlanTile;
use crate::source::tw;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::OnceLock;

mod rdcap;
pub use rdcap::decode_rdcap;
pub(crate) use rdcap::decode_rdcap_with_memory_and_control;

pub(crate) struct BudgetedRadarField {
    pub field: RadarField,
    pub _leases: Vec<RasterBufferLease>,
}

const UNIVERSAL_BLUE_RGBA: &str = r#"
63615914 66635a19 69665c1e 6c685d24 6f6b5f29 726e612e 75706234
78736439 7c75653e 7f786744 827b6949 857d6a4e 88806c54 8b826d59
8e856f5e 92887164 9e93756e aa9e7978 b6a97e82 c2b4828c cec08796
d2c48ba0 d6c88faa dacc93b4 ded097be 88ddeeff 6cd1ebff 51c5e8ff
36bae5ff 1baee2ff 00a3e0ff 009ad5ff 0091caff 0088bfff 007fb4ff
0077aaff 0070a3ff 00699cff 006295ff 005b8eff 005588ff 005180ff
004e78ff 004a70ff 004768ff ffee00ff ffe000ff ffd200ff ffc500ff
ffb700ff ffaa00ff ff9f00ff ff9500ff ff8b00ff ff8100ff ff4400ff
f23600ff e62800ff d91b00ff cd0d00ff c10000ff a80000ff 8f0000ff
760000ff 5d0000ff ffaaffff ff9fffff ff95ffff ff8bffff ff81ffff
ff77ffff ff6cffff ff62ffff ff58ffff ff4effff ffffffff ffffffff
ffffffff ffffffff ffffffff ffffffff ffffffff ffffffff ffffffff ffffffff
00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff
00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff
00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff
"#;

const QUALITY_VALID: u16 = 0;
const QUALITY_TRANSPARENT: u16 = 1;
const QUALITY_UNKNOWN_COLOR: u16 = 4;
const TW_QUALITY_MISSING: u16 = 1;
const TW_QUALITY_OUTSIDE: u16 = 2;

/// Decode only the RainViewer Universal Blue tile format whose color-to-dBZ
/// mapping and Web Mercator pixel centers have a retained provider fixture.
/// Other tile sources remain raw-only until their own scientific references
/// pass the same kind of validation.
pub fn decode_rainviewer(raw: &RawFrame, limits: &Limits) -> CoreResult<RadarField> {
    let memory = limits.raster_memory_budget();
    Ok(decode_rainviewer_with_memory(raw, limits, &memory)?.field)
}

pub(crate) fn decode_rainviewer_with_memory(
    raw: &RawFrame,
    limits: &Limits,
    memory: &RasterMemoryBudget,
) -> CoreResult<BudgetedRadarField> {
    decode_rainviewer_inner(raw, limits, memory, None)
}

pub(crate) fn decode_rainviewer_with_memory_and_control(
    raw: &RawFrame,
    limits: &Limits,
    memory: &RasterMemoryBudget,
    control: &RasterWorkControl,
) -> CoreResult<BudgetedRadarField> {
    decode_rainviewer_inner(raw, limits, memory, Some(control))
}

fn decode_rainviewer_inner(
    raw: &RawFrame,
    limits: &Limits,
    memory: &RasterMemoryBudget,
    control: Option<&RasterWorkControl>,
) -> CoreResult<BudgetedRadarField> {
    check_raster_work(control)?;
    let plan = rainviewer::validate_science_frame(&raw.frame)?;
    let expected = plan
        .tiles()
        .map(|PlanTile { x, y, column, row }| (plan.name(x, y), (column, row)))
        .collect::<HashMap<_, _>>();
    let tile_artifacts = expected.len();
    if raw.artifacts.len() != tile_artifacts {
        return Err(CoreError::Transport(format!(
            "RainViewer frame has {} tiles, expected {tile_artifacts}",
            raw.artifacts.len()
        )));
    }

    let world_width = plan
        .columns
        .checked_mul(TILE_SIZE)
        .ok_or_else(|| CoreError::ResourceLimit("RainViewer grid dimensions overflow".into()))?;
    let world_height = plan.rows * TILE_SIZE;
    let world_pixels = u64::from(world_width)
        .checked_mul(u64::from(world_height))
        .ok_or_else(|| CoreError::ResourceLimit("RainViewer grid dimensions overflow".into()))?;
    limits.validate_pixels(world_pixels)?;
    let rgba_len = usize::try_from(world_pixels)
        .ok()
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| CoreError::ResourceLimit("RainViewer grid dimensions overflow".into()))?;
    let row_bytes = usize::try_from(world_width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| CoreError::ResourceLimit("RainViewer grid dimensions overflow".into()))?;
    let tile_row_bytes = TILE_SIZE as usize * 4;
    let axes_bytes = (u64::from(world_width) + u64::from(world_height))
        .checked_mul(std::mem::size_of::<f64>() as u64)
        .ok_or_else(|| CoreError::ResourceLimit("RainViewer axis buffer size overflow".into()))?;
    let mosaic_bytes = u64::try_from(rgba_len)
        .map_err(|_| CoreError::ResourceLimit("RainViewer mosaic size overflow".into()))?;
    let mosaic_lease = memory.reserve(mosaic_bytes)?;
    let mut mosaic = Vec::new();
    mosaic
        .try_reserve_exact(rgba_len)
        .map_err(|_| CoreError::ResourceLimit("RainViewer mosaic could not be allocated".into()))?;
    mosaic.resize(rgba_len, 0_u8);
    let mut seen = std::collections::HashSet::new();
    let mut frame_bytes = 0_u64;

    for artifact in &raw.artifacts {
        check_raster_work(control)?;
        if artifact.receipt.media_type != "image/png" {
            return Err(CoreError::Transport("RainViewer tile is not a PNG artifact".into()));
        }
        let &(tile_x, tile_y) = expected.get(&artifact.receipt.name).ok_or_else(|| {
            CoreError::Transport("RainViewer tile is outside the selected plan".into())
        })?;
        if !seen.insert((tile_x, tile_y)) {
            return Err(CoreError::Transport("RainViewer frame contains a duplicate tile".into()));
        }

        let metadata = std::fs::metadata(&artifact.path)
            .map_err(|_| CoreError::Temporary("RainViewer tile could not be read".into()))?;
        if !metadata.is_file() || metadata.len() != artifact.receipt.size_bytes {
            return Err(CoreError::Transport(
                "RainViewer tile size changed after acquisition".into(),
            ));
        }
        frame_bytes = frame_bytes.saturating_add(metadata.len());
        limits.validate_bytes(metadata.len(), frame_bytes)?;
        let tile_pixel_bytes = u64::from(TILE_SIZE)
            .checked_mul(u64::from(TILE_SIZE))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| CoreError::ResourceLimit("RainViewer tile size overflow".into()))?;
        let tile_work_lease = memory.reserve_buffers(&[
            metadata
                .len()
                .checked_mul(2)
                .ok_or_else(|| CoreError::ResourceLimit("RainViewer tile size overflow".into()))?,
            tile_pixel_bytes,
            // Non-RGBA PNGs retain their decoded pixels while converting to RGBA.
            tile_pixel_bytes,
        ])?;
        let bytes = std::fs::read(&artifact.path)
            .map_err(|_| CoreError::Temporary("RainViewer tile could not be read".into()))?;
        let digest = hex::encode(Sha256::digest(&bytes));
        if digest != artifact.receipt.sha256 {
            return Err(CoreError::Transport(
                "RainViewer tile digest does not match its receipt".into(),
            ));
        }

        let reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
        let mut decoder = reader
            .into_decoder()
            .map_err(|_| CoreError::Transport("RainViewer tile PNG could not be decoded".into()))?;
        let (width, height) = decoder.dimensions();
        if width != TILE_SIZE || height != TILE_SIZE {
            return Err(CoreError::Transport("RainViewer tile dimensions are invalid".into()));
        }
        let mut image_limits = image::Limits::default();
        image_limits.max_image_width = Some(TILE_SIZE);
        image_limits.max_image_height = Some(TILE_SIZE);
        image_limits.max_alloc = Some(u64::from(TILE_SIZE) * u64::from(TILE_SIZE) * 4);
        decoder.set_limits(image_limits).map_err(|_| {
            CoreError::ResourceLimit("RainViewer tile exceeds decoder limits".into())
        })?;
        let rgba = DynamicImage::from_decoder(decoder)
            .map_err(|_| CoreError::Transport("RainViewer tile PNG could not be decoded".into()))?
            .into_rgba8()
            .into_raw();

        for row in 0..TILE_SIZE as usize {
            check_raster_work(control)?;
            let source_start = row * tile_row_bytes;
            let destination_start = (tile_y as usize * TILE_SIZE as usize + row) * row_bytes
                + tile_x as usize * tile_row_bytes;
            mosaic[destination_start..destination_start + tile_row_bytes]
                .copy_from_slice(&rgba[source_start..source_start + tile_row_bytes]);
        }
        drop(tile_work_lease);
    }
    if seen.len() != tile_artifacts {
        return Err(CoreError::Transport("RainViewer frame is missing a tile".into()));
    }

    let (values, quality, output_lease) = decode_rgba_with_control(&mosaic, memory, control)?;
    drop(mosaic);
    drop(mosaic_lease);
    let axes_lease = memory.reserve(axes_bytes)?;
    check_raster_work(control)?;
    let (x, y) = plan.axes(TILE_SIZE);
    let shape = vec![world_height as usize, world_width as usize];
    let tile_origin = raw.frame.locator["host"].as_str().unwrap_or_default();
    let tile_path = raw.frame.locator["path"].as_str().unwrap_or_default();
    let field = RadarField {
        name: "reflectivity".into(),
        values,
        shape: shape.clone(),
        quality,
        units: Some("dBZ".into()),
        valid_time: raw.frame.valid_time.clone(),
        grid: Grid { shape, crs: Some("EPSG:4326".into()), x, y, affine: None },
        provenance: vec![
            "source=rainviewer".into(),
            "product=composite".into(),
            format!("upstream_uri={tile_origin}{tile_path}"),
            format!("tile_plan=zoom-{},size-{TILE_SIZE},universal-blue-v2", plan.zoom),
            format!(
                "tile_extent=x-{},y-{},columns-{},rows-{}",
                plan.x_start, plan.y_start, plan.columns, plan.rows
            ),
            "decoder=rainviewer-universal-blue-v1".into(),
        ]
        .into_iter()
        .chain((plan.zoom != rainviewer::ONLINE_VERIFIED_ZOOM).then(|| {
            format!(
                "tile_zoom_verification=offline-only,online-verified-zoom-{}",
                rainviewer::ONLINE_VERIFIED_ZOOM
            )
        }))
        .collect(),
    };
    field
        .validate()
        .map_err(|_| CoreError::Transport("RainViewer decoded field is inconsistent".into()))?;
    Ok(BudgetedRadarField { field, _leases: vec![output_lease, axes_lease] })
}

/// Decode the CWA O-A0059-001 numeric reflectivity grid while preserving its
/// declared southwest-first TWD67 ordering and explicit missing/outside codes.
pub fn decode_tw_grid(raw: &RawFrame, limits: &Limits) -> CoreResult<RadarField> {
    let memory = limits.raster_memory_budget();
    Ok(decode_tw_grid_with_memory(raw, limits, &memory)?.field)
}

pub(crate) fn decode_tw_grid_with_memory(
    raw: &RawFrame,
    limits: &Limits,
    memory: &RasterMemoryBudget,
) -> CoreResult<BudgetedRadarField> {
    if raw.artifacts.len() != 1 {
        return Err(CoreError::Transport("source tw grid must contain one JSON artifact".into()));
    }
    let artifact = &raw.artifacts[0];
    if artifact.receipt.name != "O-A0059-001.json"
        || artifact.receipt.media_type != "application/json"
    {
        return Err(CoreError::Transport("source tw grid artifact identity is invalid".into()));
    }
    let file = std::fs::metadata(&artifact.path)
        .map_err(|_| CoreError::Temporary("source tw grid artifact could not be read".into()))?;
    if !file.is_file() || file.len() != artifact.receipt.size_bytes {
        return Err(CoreError::Transport("source tw grid artifact size changed".into()));
    }
    limits.validate_bytes(file.len(), file.len())?;
    let parser_bytes = file
        .len()
        .checked_mul(3)
        .ok_or_else(|| CoreError::ResourceLimit("source tw JSON buffer size overflow".into()))?;
    let _parser_lease = memory.reserve(parser_bytes)?;
    let payload = std::fs::read(&artifact.path)
        .map_err(|_| CoreError::Temporary("source tw grid artifact could not be read".into()))?;
    if hex::encode(Sha256::digest(&payload)) != artifact.receipt.sha256 {
        return Err(CoreError::Transport(
            "source tw grid digest does not match its receipt".into(),
        ));
    }
    let document: serde_json::Value = serde_json::from_slice(&payload)
        .map_err(|_| CoreError::Transport("source tw grid JSON is invalid".into()))?;
    let metadata = tw::validate_science_grid(&raw.frame, &document)?;
    let cells = u64::from(metadata.width)
        .checked_mul(u64::from(metadata.height))
        .ok_or_else(|| CoreError::ResourceLimit("source tw grid dimensions overflow".into()))?;
    let decoded_pixels = cells
        .checked_mul(2)
        .ok_or_else(|| CoreError::ResourceLimit("source tw grid dimensions overflow".into()))?;
    limits.validate_pixels(decoded_pixels)?;
    let cell_count = usize::try_from(cells)
        .map_err(|_| CoreError::ResourceLimit("source tw grid dimensions overflow".into()))?;
    let axes_bytes = u64::from(metadata.width)
        .checked_add(u64::from(metadata.height))
        .and_then(|count| count.checked_mul(std::mem::size_of::<f64>() as u64))
        .ok_or_else(|| CoreError::ResourceLimit("source tw axis buffer size overflow".into()))?;
    let output_lease = memory.reserve_buffers(&[
        cells
            .checked_mul((std::mem::size_of::<f32>() + std::mem::size_of::<u16>()) as u64)
            .ok_or_else(|| {
                CoreError::ResourceLimit("source tw output buffer size overflow".into())
            })?,
        axes_bytes,
    ])?;
    let content = document
        .get("cwaopendata")
        .and_then(|provider| provider.get("dataset"))
        .and_then(|dataset| dataset.get("contents"))
        .and_then(|contents| contents.get("content"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| CoreError::Transport("source tw grid values are missing".into()))?;
    let mut values = Vec::new();
    let mut quality = Vec::new();
    values
        .try_reserve_exact(cell_count)
        .map_err(|_| CoreError::ResourceLimit("source tw values could not be allocated".into()))?;
    quality
        .try_reserve_exact(cell_count)
        .map_err(|_| CoreError::ResourceLimit("source tw quality could not be allocated".into()))?;
    for token in content.split(',') {
        if values.len() == cell_count {
            return Err(CoreError::Transport("source tw grid has too many values".into()));
        }
        let value = token
            .trim()
            .parse::<f32>()
            .map_err(|_| CoreError::Transport("source tw grid contains an invalid value".into()))?;
        if !value.is_finite() {
            return Err(CoreError::Transport("source tw grid contains a non-finite value".into()));
        }
        match value {
            -99.0 => {
                values.push(f32::NAN);
                quality.push(TW_QUALITY_MISSING);
            }
            -999.0 => {
                values.push(f32::NAN);
                quality.push(TW_QUALITY_OUTSIDE);
            }
            _ => {
                values.push(value);
                quality.push(QUALITY_VALID);
            }
        }
    }
    if values.len() != cell_count {
        return Err(CoreError::Transport("source tw grid has an incomplete value array".into()));
    }

    let mut x = Vec::new();
    let mut y = Vec::new();
    x.try_reserve_exact(metadata.width as usize)
        .map_err(|_| CoreError::ResourceLimit("source tw x-axis allocation failed".into()))?;
    y.try_reserve_exact(metadata.height as usize)
        .map_err(|_| CoreError::ResourceLimit("source tw y-axis allocation failed".into()))?;
    for column in 0..metadata.width {
        x.push(metadata.west + metadata.step * f64::from(column));
    }
    for row in 0..metadata.height {
        y.push(metadata.south + metadata.step * f64::from(row));
    }
    let shape = vec![metadata.height as usize, metadata.width as usize];
    let field = RadarField {
        name: "reflectivity".into(),
        values,
        shape: shape.clone(),
        quality,
        units: Some("dBZ".into()),
        valid_time: raw.frame.valid_time.clone(),
        grid: Grid { shape, crs: Some("EPSG:3821".into()), x, y, affine: None },
        provenance: vec![
            "source=tw".into(),
            "product=grid".into(),
            format!("station={}", raw.frame.station.as_deref().unwrap_or_default()),
            format!("upstream_uri={}", raw.frame.locator["url"].as_str().unwrap_or_default()),
            "native_crs=EPSG:3821".into(),
            "decoder=cwa-O-A0059-001-v1".into(),
            "missing_values=-99:invalid,-999:outside_coverage_or_quality_removed".into(),
        ],
    };
    field
        .validate()
        .map_err(|_| CoreError::Transport("source tw decoded grid is inconsistent".into()))?;
    Ok(BudgetedRadarField { field, _leases: vec![output_lease] })
}

fn color_table() -> &'static HashMap<u32, f32> {
    static TABLE: OnceLock<HashMap<u32, f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        UNIVERSAL_BLUE_RGBA
            .split_whitespace()
            .enumerate()
            .filter_map(|(index, hex)| {
                let packed = u32::from_str_radix(hex, 16).ok()?;
                Some((packed, index as f32 - 10.0))
            })
            .fold(HashMap::new(), |mut table, (color, dbz)| {
                table.entry(color).or_insert(dbz);
                table
            })
    })
}

#[cfg(test)]
fn decode_rgba(
    rgba: &[u8],
    memory: &RasterMemoryBudget,
) -> CoreResult<(Vec<f32>, Vec<u16>, RasterBufferLease)> {
    decode_rgba_with_control(rgba, memory, None)
}

fn decode_rgba_with_control(
    rgba: &[u8],
    memory: &RasterMemoryBudget,
    control: Option<&RasterWorkControl>,
) -> CoreResult<(Vec<f32>, Vec<u16>, RasterBufferLease)> {
    if rgba.len() % 4 != 0 {
        return Err(CoreError::Transport("RainViewer mosaic has an incomplete pixel".into()));
    }
    let pixels = rgba.len() / 4;
    let pixel_bytes = u64::try_from(pixels)
        .ok()
        .and_then(|pixels| {
            pixels.checked_mul((std::mem::size_of::<f32>() + std::mem::size_of::<u16>()) as u64)
        })
        .ok_or_else(|| CoreError::ResourceLimit("RainViewer output buffer size overflow".into()))?;
    let lease = memory.reserve(pixel_bytes)?;
    let mut values = Vec::new();
    let mut quality = Vec::new();
    values
        .try_reserve_exact(pixels)
        .map_err(|_| CoreError::ResourceLimit("RainViewer values could not be allocated".into()))?;
    quality.try_reserve_exact(pixels).map_err(|_| {
        CoreError::ResourceLimit("RainViewer quality could not be allocated".into())
    })?;
    let colors = color_table();
    for (index, pixel) in rgba.chunks_exact(4).enumerate() {
        if index % 65_536 == 0 {
            check_raster_work(control)?;
        }
        let alpha = pixel[3];
        let packed = u32::from_be_bytes([pixel[0], pixel[1], pixel[2], alpha]);
        if alpha == 0 {
            values.push(f32::NAN);
            quality.push(QUALITY_TRANSPARENT);
        } else if let Some(dbz) = colors.get(&packed) {
            values.push(*dbz);
            quality.push(QUALITY_VALID);
        } else {
            values.push(f32::NAN);
            quality.push(QUALITY_UNKNOWN_COLOR);
        }
    }
    Ok((values, quality, lease))
}

fn check_raster_work(control: Option<&RasterWorkControl>) -> CoreResult<()> {
    if let Some(control) = control {
        control.check()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_and_unknown_colors_keep_distinct_quality_codes() {
        let rgba = [
            0x63, 0x61, 0x59, 0x14, // -10 dBZ, valid color.
            0x63, 0x61, 0x59, 0x00, // Transparent even if RGB resembles a table color.
            0x12, 0x34, 0x56, 0xff, // Unknown opaque color.
        ];
        let budget = RasterMemoryBudget::new(18);
        let (values, quality, lease) = decode_rgba(&rgba, &budget).unwrap();
        assert_eq!(values[0], -10.0);
        assert!(values[1].is_nan());
        assert!(values[2].is_nan());
        assert_eq!(quality, [QUALITY_VALID, QUALITY_TRANSPARENT, QUALITY_UNKNOWN_COLOR]);
        assert_eq!(lease.bytes(), 18);
        assert_eq!(budget.reserved_bytes(), 18);
        drop(lease);
        assert_eq!(budget.reserved_bytes(), 0);

        assert!(matches!(
            decode_rgba(&rgba, &RasterMemoryBudget::new(17)),
            Err(CoreError::ResourceLimit(_))
        ));
    }

    #[test]
    fn official_universal_blue_mapping_contains_all_declared_dbz_entries() {
        assert_eq!(UNIVERSAL_BLUE_RGBA.split_whitespace().count(), 106);
        assert_eq!(color_table().get(&0x6361_5914), Some(&-10.0));
        // The official table repeats white; first-match behavior preserves the
        // Python adapter's deterministic lowest-dBZ mapping.
        assert_eq!(color_table().get(&0xffff_ffff), Some(&65.0));
    }
}
