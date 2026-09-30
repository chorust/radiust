//! Pure-Rust GeoTIFF data, quality, and provenance outputs.

use std::borrow::Cow;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use tiff::decoder::{Decoder, DecodingResult};
use tiff::encoder::colortype::{Gray16, Gray32Float};
use tiff::encoder::{Compression, DeflateLevel, TiffEncoder, TiffKindStandard};
use tiff::tags::Tag;

use crate::errors::{CoreError, CoreResult};
use crate::limits::Limits;
use crate::model::{Grid, RadarField, parse_utc_time};

const STRIP_ROWS: usize = 64;
const WEB_MERCATOR_RADIUS: f64 = 6_378_137.0;
const WEB_MERCATOR_MAX_LATITUDE: f64 = 85.051_128_779_806_6;
const WEB_MERCATOR_MAX_COORDINATE: f64 = 20_037_508.342_789_244;
const GEOTIFF_PROVENANCE_SCHEMA: u32 = 2;
const SOURCE_Y_FLIPPED_TAG: Tag = Tag::Unknown(65_000);

#[derive(Clone, Copy, Debug)]
struct GeoTransform {
    west: f64,
    north: f64,
    dx: f64,
    dy: f64,
    flip_y: bool,
}

impl GeoTransform {
    fn coefficients(self) -> [f64; 6] {
        [self.west, self.dx, 0.0, self.north, 0.0, -self.dy]
    }
}

fn storage_error(message: impl Into<String>) -> CoreError {
    CoreError::Storage(message.into())
}

/// Read a field written by [`write_field`], requiring its quality and
/// provenance sidecars to agree with the data raster.
///
/// The returned grid uses the CRS and pixel centers encoded in the GeoTIFF.
/// In particular, a geographic source grid that the writer projected to
/// Web Mercator is returned as EPSG:3857 with meter-valued centers.
pub fn read_selected_field(path: impl AsRef<Path>, limits: &Limits) -> CoreResult<RadarField> {
    let path = path.as_ref();
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| storage_error("GeoTIFF input filename must be valid UTF-8"))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let quality_path = parent.join(format!("{stem}_quality.tif"));
    let provenance_path = parent.join(format!("{stem}_provenance.json"));

    let mut group_bytes = 0_u64;
    let (data_file, _) = open_bounded_regular_file(path, limits, &mut group_bytes)?;
    let (quality_file, _) = open_bounded_regular_file(&quality_path, limits, &mut group_bytes)?;
    let (provenance_file, provenance_size) =
        open_bounded_regular_file(&provenance_path, limits, &mut group_bytes)?;
    let provenance_bytes = read_bounded_file(provenance_file, provenance_size)?;
    let provenance: GeoTiffProvenance = serde_json::from_slice(&provenance_bytes)
        .map_err(|_| storage_error("GeoTIFF provenance is missing or invalid"))?;
    validate_provenance(&provenance)?;

    let mut data_decoder = bounded_decoder(data_file, limits)?;
    let (width, height) = data_decoder
        .dimensions()
        .map_err(|_| storage_error("GeoTIFF data dimensions could not be read"))?;
    let pixels = validate_read_dimensions(width, height, limits)?;
    validate_field_byte_budget(pixels, limits)?;
    let data_tags = validate_raster_tags(
        &mut data_decoder,
        width,
        height,
        &provenance,
        &provenance.variable,
        RasterKind::Data,
    )?;
    if data_decoder.more_images() {
        return Err(storage_error("GeoTIFF data must contain exactly one raster image"));
    }
    let values = match data_decoder
        .read_image()
        .map_err(|_| storage_error("GeoTIFF data pixels could not be decoded"))?
    {
        DecodingResult::F32(values) if values.len() as u64 == pixels => values,
        _ => return Err(storage_error("GeoTIFF data must be a single-band float32 raster")),
    };
    drop(data_decoder);

    let mut quality_decoder = bounded_decoder(quality_file, limits)?;
    let quality_dimensions = quality_decoder
        .dimensions()
        .map_err(|_| storage_error("GeoTIFF quality dimensions could not be read"))?;
    if quality_dimensions != (width, height) {
        return Err(storage_error("GeoTIFF quality dimensions do not match the data raster"));
    }
    let quality_tags = validate_raster_tags(
        &mut quality_decoder,
        width,
        height,
        &provenance,
        "quality",
        RasterKind::Quality,
    )?;
    if !same_raster_geometry(&data_tags, &quality_tags) || quality_decoder.more_images() {
        return Err(storage_error(
            "GeoTIFF quality orientation, transform, or CRS does not match the data raster",
        ));
    }
    let quality = match quality_decoder
        .read_image()
        .map_err(|_| storage_error("GeoTIFF quality pixels could not be decoded"))?
    {
        DecodingResult::U16(values) if values.len() as u64 == pixels => values,
        _ => return Err(storage_error("GeoTIFF quality must be a single-band uint16 raster")),
    };

    let transform = data_tags.transform;
    let x = (0..width)
        .map(|column| transform.west + (f64::from(column) + 0.5) * transform.dx)
        .collect::<Vec<_>>();
    let mut y = (0..height)
        .map(|row| transform.north - (f64::from(row) + 0.5) * transform.dy)
        .collect::<Vec<_>>();
    validate_output_centers(&x, &y, data_tags.epsg)?;

    let mut values = values;
    let mut quality = quality;
    if provenance.flip_y {
        reverse_rows(&mut values, width as usize, height as usize);
        reverse_rows(&mut quality, width as usize, height as usize);
        y.reverse();
    }

    let result = RadarField {
        name: provenance.variable,
        values,
        shape: vec![height as usize, width as usize],
        quality,
        units: provenance.units,
        valid_time: provenance.valid_time,
        grid: Grid {
            shape: vec![height as usize, width as usize],
            crs: Some(provenance.crs),
            x,
            y,
            affine: Some(transform.coefficients()),
        },
        provenance: provenance.provenance,
    };
    result.validate().map_err(|error| storage_error(error.to_string()))?;
    Ok(result)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeoTiffProvenance {
    schema_version: u32,
    variable: String,
    units: Option<String>,
    grid: String,
    source_crs: String,
    crs: String,
    coordinate_operation: String,
    transform: [f64; 6],
    flip_y: bool,
    valid_time: String,
    processing_history: Vec<Value>,
    provenance: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RasterKind {
    Data,
    Quality,
}

#[derive(Clone, Copy, Debug)]
struct RasterGeometry {
    width: u32,
    height: u32,
    transform: GeoTransform,
    epsg: u16,
}

fn validate_provenance(provenance: &GeoTiffProvenance) -> CoreResult<()> {
    if provenance.schema_version != GEOTIFF_PROVENANCE_SCHEMA
        || !valid_field_variable(&provenance.variable)
        || provenance.units.as_deref().is_some_and(|units| !valid_provenance_text(units))
        || provenance.provenance.iter().any(|entry| !valid_provenance_text(entry))
        || !provenance.processing_history.is_empty()
        || parse_utc_time(&provenance.valid_time).is_err()
    {
        return Err(storage_error("GeoTIFF identity, time, units, or provenance is invalid"));
    }
    let source_crs = canonical_crs(&provenance.source_crs);
    let (expected_crs, expected_operation, expected_grid) = match source_crs.as_deref() {
        Some("EPSG:4326") if provenance.crs == "EPSG:4326" => {
            ("EPSG:4326", "identity", "geographic")
        }
        Some("EPSG:4326") if provenance.crs == "EPSG:3857" => {
            ("EPSG:3857", "spherical_web_mercator", "projected")
        }
        Some("EPSG:3821") if provenance.crs == "EPSG:3821" => {
            ("EPSG:3821", "identity", "geographic")
        }
        _ => return Err(storage_error("GeoTIFF provenance declares an unsupported CRS path")),
    };
    if provenance.crs != expected_crs
        || provenance.coordinate_operation != expected_operation
        || provenance.grid != expected_grid
    {
        return Err(storage_error("GeoTIFF provenance CRS and coordinate operation disagree"));
    }
    let [west, dx, x_rotation, north, y_rotation, negative_dy] = provenance.transform;
    if [west, dx, x_rotation, north, y_rotation, negative_dy].iter().any(|value| !value.is_finite())
        || dx <= 0.0
        || negative_dy >= 0.0
        || x_rotation != 0.0
        || y_rotation != 0.0
    {
        return Err(storage_error("GeoTIFF provenance transform is invalid"));
    }
    Ok(())
}

fn canonical_crs(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_uppercase();
    matches!(normalized.as_str(), "EPSG:4326" | "EPSG:3821").then_some(normalized)
}

fn valid_field_variable(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && !value
            .chars()
            .any(|character| matches!(character, '/' | '\\' | '\0') || character.is_control())
}

fn valid_provenance_text(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

fn validate_read_dimensions(width: u32, height: u32, limits: &Limits) -> CoreResult<u64> {
    if width < 2 || height < 2 {
        return Err(storage_error("GeoTIFF field dimensions must both be at least two"));
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| CoreError::ResourceLimit("GeoTIFF dimensions overflow".into()))?;
    limits.validate_pixels(pixels)?;
    Ok(pixels)
}

fn validate_field_byte_budget(pixels: u64, limits: &Limits) -> CoreResult<()> {
    let raw_bytes = pixels
        .checked_mul((std::mem::size_of::<f32>() + std::mem::size_of::<u16>()) as u64)
        .ok_or_else(|| CoreError::ResourceLimit("GeoTIFF field size overflows".into()))?;
    if raw_bytes > limits.max_frame_bytes || raw_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "GeoTIFF field exceeds configured frame or temporary-file limit".into(),
        ));
    }
    Ok(())
}

fn open_bounded_regular_file(
    path: &Path,
    limits: &Limits,
    group_bytes: &mut u64,
) -> CoreResult<(File, u64)> {
    let path_metadata = fs::symlink_metadata(path)
        .map_err(|_| storage_error("GeoTIFF artifact is missing or could not be inspected"))?;
    if !path_metadata.is_file() || path_metadata.file_type().is_symlink() {
        return Err(storage_error("GeoTIFF artifact must be a regular file"));
    }
    let size = path_metadata.len();
    *group_bytes = group_bytes
        .checked_add(size)
        .ok_or_else(|| CoreError::ResourceLimit("GeoTIFF artifact group size overflows".into()))?;
    limits.validate_bytes(size, *group_bytes)?;
    if *group_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "GeoTIFF artifact group exceeds configured temporary-file limit".into(),
        ));
    }
    let file = open_without_following_symlink(path)
        .map_err(|_| storage_error("GeoTIFF artifact could not be opened safely"))?;
    let file_metadata = file
        .metadata()
        .map_err(|_| storage_error("GeoTIFF artifact metadata could not be read"))?;
    if !file_metadata.is_file() || file_metadata.len() != size {
        return Err(storage_error("GeoTIFF artifact changed while it was being opened"));
    }
    Ok((file, size))
}

fn open_without_following_symlink(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn read_bounded_file(file: File, size: u64) -> CoreResult<Vec<u8>> {
    let capacity = usize::try_from(size)
        .map_err(|_| CoreError::ResourceLimit("GeoTIFF provenance is too large".into()))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| CoreError::ResourceLimit("GeoTIFF provenance allocation failed".into()))?;
    file.take(size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| storage_error("GeoTIFF provenance could not be read"))?;
    if bytes.len() as u64 != size {
        return Err(storage_error("GeoTIFF provenance changed while it was being read"));
    }
    Ok(bytes)
}

fn bounded_decoder<R: Read + Seek>(reader: R, limits: &Limits) -> CoreResult<Decoder<R>> {
    let to_usize = |value: u64| usize::try_from(value).unwrap_or(usize::MAX);
    let mut decoder_limits = tiff::decoder::Limits::default();
    decoder_limits.decoding_buffer_size = to_usize(limits.max_frame_bytes);
    decoder_limits.intermediate_buffer_size = to_usize(limits.max_frame_bytes);
    decoder_limits.ifd_value_size = to_usize(limits.max_artifact_bytes);
    Decoder::new(reader)
        .map(|decoder| decoder.with_limits(decoder_limits))
        .map_err(|_| storage_error("GeoTIFF raster could not be opened"))
}

fn validate_raster_tags<R: Read + Seek>(
    decoder: &mut Decoder<R>,
    width: u32,
    height: u32,
    provenance: &GeoTiffProvenance,
    expected_description: &str,
    kind: RasterKind,
) -> CoreResult<RasterGeometry> {
    let expected_color = match kind {
        RasterKind::Data => tiff::ColorType::Gray(32),
        RasterKind::Quality => tiff::ColorType::Gray(16),
    };
    if decoder.colortype().map_err(|_| storage_error("GeoTIFF sample type could not be read"))?
        != expected_color
    {
        return Err(storage_error(match kind {
            RasterKind::Data => "GeoTIFF data must be a single-band float32 raster",
            RasterKind::Quality => "GeoTIFF quality must be a single-band uint16 raster",
        }));
    }
    let bits = decoder
        .get_tag_u16_vec(Tag::BitsPerSample)
        .map_err(|_| storage_error("GeoTIFF BitsPerSample tag is invalid"))?;
    let sample_format = decoder
        .get_tag_u16_vec(Tag::SampleFormat)
        .map_err(|_| storage_error("GeoTIFF SampleFormat tag is invalid"))?;
    let expected_bits = match kind {
        RasterKind::Data => 32,
        RasterKind::Quality => 16,
    };
    let expected_format = match kind {
        RasterKind::Data => 3,
        RasterKind::Quality => 1,
    };
    if bits != [expected_bits]
        || sample_format != [expected_format]
        || decoder
            .get_tag_unsigned::<u16>(Tag::PhotometricInterpretation)
            .map_err(|_| storage_error("GeoTIFF photometric interpretation is invalid"))?
            != 1
        || decoder
            .find_tag_unsigned::<u16>(Tag::SamplesPerPixel)
            .map_err(|_| storage_error("GeoTIFF sample count is invalid"))?
            .unwrap_or(1)
            != 1
        || decoder
            .find_tag_unsigned::<u16>(Tag::PlanarConfiguration)
            .map_err(|_| storage_error("GeoTIFF planar configuration is invalid"))?
            .unwrap_or(1)
            != 1
        || decoder
            .find_tag_unsigned::<u16>(Tag::Orientation)
            .map_err(|_| storage_error("GeoTIFF orientation tag is invalid"))?
            .unwrap_or(1)
            != 1
        || decoder
            .find_tag_unsigned::<u16>(SOURCE_Y_FLIPPED_TAG)
            .map_err(|_| storage_error("GeoTIFF source row direction is invalid"))?
            != Some(u16::from(provenance.flip_y))
    {
        return Err(storage_error("GeoTIFF band encoding is not supported"));
    }
    if decoder
        .get_tag_ascii_string(Tag::ImageDescription)
        .map_err(|_| storage_error("GeoTIFF image description is missing"))?
        != expected_description
    {
        return Err(storage_error("GeoTIFF band identity does not match its sidecar"));
    }
    match kind {
        RasterKind::Data => {
            if decoder
                .get_tag_ascii_string(Tag::GdalNodata)
                .map_err(|_| storage_error("GeoTIFF nodata declaration is missing"))?
                .trim_matches('\0')
                .trim()
                .to_ascii_lowercase()
                != "nan"
            {
                return Err(storage_error("GeoTIFF data nodata declaration is unsupported"));
            }
        }
        RasterKind::Quality => {
            if decoder
                .find_tag(Tag::GdalNodata)
                .map_err(|_| storage_error("GeoTIFF quality nodata tag is invalid"))?
                .is_some()
            {
                return Err(storage_error("GeoTIFF quality raster must not declare nodata"));
            }
        }
    }

    let target_epsg = parse_epsg(&provenance.crs)
        .ok_or_else(|| storage_error("GeoTIFF sidecar CRS is invalid"))?;
    let scale = decoder
        .get_tag_f64_vec(Tag::ModelPixelScaleTag)
        .map_err(|_| storage_error("GeoTIFF pixel scale tag is invalid"))?;
    let tiepoint = decoder
        .get_tag_f64_vec(Tag::ModelTiepointTag)
        .map_err(|_| storage_error("GeoTIFF tie point tag is invalid"))?;
    let keys = decoder
        .get_tag_u16_vec(Tag::GeoKeyDirectoryTag)
        .map_err(|_| storage_error("GeoTIFF CRS keys are invalid"))?;
    if decoder
        .find_tag(Tag::ModelTransformationTag)
        .map_err(|_| storage_error("GeoTIFF model transformation tag is invalid"))?
        .is_some()
        || scale.len() != 3
        || tiepoint.len() != 6
        || !same_f64_slice(&scale, &[provenance.transform[1], -provenance.transform[5], 0.0])
        || !same_f64_slice(
            &tiepoint,
            &[0.0, 0.0, 0.0, provenance.transform[0], provenance.transform[3], 0.0],
        )
        || !same_f64_slice(&provenance.transform, &transform_from_tags(&scale, &tiepoint)?)
        || keys != geo_key_directory(target_epsg)
    {
        return Err(storage_error("GeoTIFF tags disagree with provenance transform or CRS"));
    }
    let transform = GeoTransform {
        west: provenance.transform[0],
        north: provenance.transform[3],
        dx: provenance.transform[1],
        dy: -provenance.transform[5],
        flip_y: provenance.flip_y,
    };
    if width < 2 || height < 2 {
        return Err(storage_error("GeoTIFF dimensions are unsupported"));
    }
    Ok(RasterGeometry { width, height, transform, epsg: target_epsg })
}

fn transform_from_tags(scale: &[f64], tiepoint: &[f64]) -> CoreResult<[f64; 6]> {
    if scale.len() != 3 || tiepoint.len() != 6 {
        return Err(storage_error("GeoTIFF transform tags have invalid lengths"));
    }
    Ok([tiepoint[3], scale[0], 0.0, tiepoint[4], 0.0, -scale[1]])
}

fn parse_epsg(value: &str) -> Option<u16> {
    match value {
        "EPSG:4326" => Some(4326),
        "EPSG:3821" => Some(3821),
        "EPSG:3857" => Some(3857),
        _ => None,
    }
}

fn same_f64_slice(left: &[f64], right: &[f64]) -> bool {
    left == right
}

fn same_raster_geometry(left: &RasterGeometry, right: &RasterGeometry) -> bool {
    left.width == right.width
        && left.height == right.height
        && left.epsg == right.epsg
        && left.transform.flip_y == right.transform.flip_y
        && same_f64_slice(&left.transform.coefficients(), &right.transform.coefficients())
}

fn validate_output_centers(x: &[f64], y: &[f64], epsg: u16) -> CoreResult<()> {
    if x.iter().chain(y).any(|value| !value.is_finite()) {
        return Err(storage_error("GeoTIFF pixel centers are not finite"));
    }
    let valid = match epsg {
        4326 => {
            x.iter().all(|value| (-180.0..=180.0).contains(value))
                && y.iter().all(|value| value.abs() <= WEB_MERCATOR_MAX_LATITUDE)
        }
        3821 => {
            x.iter().all(|value| (-180.0..=180.0).contains(value))
                && y.iter().all(|value| (-90.0..=90.0).contains(value))
        }
        3857 => {
            x.iter().all(|value| value.abs() <= WEB_MERCATOR_MAX_COORDINATE)
                && y.iter().all(|value| value.abs() <= WEB_MERCATOR_MAX_COORDINATE)
        }
        _ => false,
    };
    if !valid {
        return Err(storage_error("GeoTIFF pixel centers are outside the supported CRS domain"));
    }
    Ok(())
}

fn reverse_rows<T>(values: &mut [T], width: usize, height: usize) {
    for row in 0..height / 2 {
        let other = height - row - 1;
        for column in 0..width {
            values.swap(row * width + column, other * width + column);
        }
    }
}

/// Write float32 values, uint16 quality flags, and sorted JSON provenance as
/// a transactional GeoTIFF artifact group. The raster orientation and center
/// coordinate handling match the existing Python writer.
pub fn write_field(
    field: &RadarField,
    destination: impl AsRef<Path>,
    limits: &Limits,
) -> CoreResult<Vec<PathBuf>> {
    field.validate().map_err(|error| storage_error(error.to_string()))?;
    if !valid_field_variable(&field.name)
        || field.units.as_deref().is_some_and(|units| !valid_provenance_text(units))
        || field.provenance.iter().any(|entry| !valid_provenance_text(entry))
    {
        return Err(storage_error("GeoTIFF identity, units, or provenance is invalid"));
    }
    let [height, width] = field.shape.as_slice() else {
        return Err(storage_error("GeoTIFF output requires a two-dimensional field"));
    };
    let geometry = geo_geometry(field)?;
    let pixels = (*height as u64)
        .checked_mul(*width as u64)
        .ok_or_else(|| CoreError::ResourceLimit("GeoTIFF dimensions overflow".into()))?;
    limits.validate_pixels(pixels)?;
    let raw_bytes = pixels
        .checked_mul((std::mem::size_of::<f32>() + std::mem::size_of::<u16>()) as u64)
        .ok_or_else(|| CoreError::ResourceLimit("GeoTIFF field size overflows".into()))?;
    if raw_bytes > limits.max_frame_bytes || raw_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "GeoTIFF output exceeds configured frame or temporary-file limit".into(),
        ));
    }

    let destination = destination.as_ref().to_path_buf();
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|_| storage_error("GeoTIFF output directory could not be prepared"))?;
    let stem = destination
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| storage_error("GeoTIFF output filename must be valid UTF-8"))?;
    let quality_name = format!("{stem}_quality.tif");
    let provenance_name = format!("{stem}_provenance.json");
    let output_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| storage_error("GeoTIFF output filename must be valid UTF-8"))?;

    let stage =
        tempfile::Builder::new().prefix(".radiust-geotiff-").tempdir_in(parent).map_err(|_| {
            CoreError::Temporary("GeoTIFF staging directory could not be created".into())
        })?;
    let data_stage = stage.path().join(output_name);
    let quality_stage = stage.path().join(&quality_name);
    let provenance_stage = stage.path().join(&provenance_name);
    write_data_raster(&data_stage, field, *width, *height, geometry.transform, geometry.epsg)?;
    write_quality_raster(
        &quality_stage,
        field,
        *width,
        *height,
        geometry.transform,
        geometry.epsg,
    )?;
    write_provenance(&provenance_stage, field, geometry)?;

    let staged = [&data_stage, &quality_stage, &provenance_stage];
    let mut total_bytes = 0_u64;
    for path in staged {
        let metadata = fs::symlink_metadata(path)
            .map_err(|_| storage_error("GeoTIFF staged output could not be inspected"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(storage_error("GeoTIFF staged output is not a regular file"));
        }
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| CoreError::ResourceLimit("GeoTIFF output size overflows".into()))?;
        limits.validate_bytes(metadata.len(), total_bytes)?;
        if total_bytes > limits.max_temp_bytes {
            return Err(CoreError::ResourceLimit(
                "GeoTIFF output exceeds configured temporary-file limit".into(),
            ));
        }
    }

    let outputs = [
        (data_stage, destination.clone()),
        (quality_stage, parent.join(&quality_name)),
        (provenance_stage, parent.join(&provenance_name)),
    ];
    publish_group(stage.path(), &outputs)?;
    Ok(outputs.into_iter().map(|(_, path)| path).collect())
}

#[derive(Clone, Copy, Debug)]
struct GeoGeometry {
    transform: GeoTransform,
    epsg: u16,
    coordinate_operation: &'static str,
}

fn geo_geometry(field: &RadarField) -> CoreResult<GeoGeometry> {
    let [height, width] = field.shape.as_slice() else {
        return Err(storage_error("GeoTIFF output requires a two-dimensional field"));
    };
    let x = &field.grid.x;
    let y = &field.grid.y;
    if x.len() != *width || y.len() != *height || x.len() < 2 || y.len() < 2 {
        return Err(storage_error("GeoTIFF requires at least two center coordinates on each axis"));
    }
    match field.grid.crs.as_deref().map(str::trim).map(str::to_ascii_uppercase).as_deref() {
        Some("EPSG:4326") => {
            if x.iter().any(|value| !value.is_finite() || !(-180.0..=180.0).contains(value))
                || y.iter()
                    .any(|value| !value.is_finite() || value.abs() > WEB_MERCATOR_MAX_LATITUDE)
            {
                return Err(storage_error(
                    "GeoTIFF EPSG:4326 coordinates are outside the supported Web Mercator domain",
                ));
            }
            if let Some(transform) = regular_geo_transform(x, y) {
                return Ok(GeoGeometry { transform, epsg: 4326, coordinate_operation: "identity" });
            }

            let projected_x = x
                .iter()
                .map(|longitude| WEB_MERCATOR_RADIUS * longitude.to_radians())
                .collect::<Vec<_>>();
            let projected_y =
                y.iter().map(|latitude| web_mercator_y(*latitude)).collect::<Vec<_>>();
            let transform = regular_geo_transform(&projected_x, &projected_y).ok_or_else(|| {
                storage_error(
                    "GeoTIFF requires regular source coordinates or regular Web Mercator coordinates",
                )
            })?;
            if projected_x.iter().any(|value| value.abs() > WEB_MERCATOR_MAX_COORDINATE)
                || projected_y.iter().any(|value| value.abs() > WEB_MERCATOR_MAX_COORDINATE)
            {
                return Err(storage_error(
                    "GeoTIFF coordinates exceed the Web Mercator world extent",
                ));
            }
            Ok(GeoGeometry {
                transform,
                epsg: 3857,
                coordinate_operation: "spherical_web_mercator",
            })
        }
        Some("EPSG:3821") => {
            if x.iter().any(|value| !value.is_finite() || !(-180.0..=180.0).contains(value))
                || y.iter().any(|value| !value.is_finite() || !(-90.0..=90.0).contains(value))
            {
                return Err(storage_error(
                    "GeoTIFF EPSG:3821 coordinates must be longitude/latitude degrees",
                ));
            }
            let transform = regular_geo_transform(x, y).ok_or_else(|| {
                storage_error("GeoTIFF requires regularly spaced center coordinates")
            })?;
            Ok(GeoGeometry { transform, epsg: 3821, coordinate_operation: "identity" })
        }
        _ => Err(storage_error(
            "GeoTIFF currently supports only validated EPSG:4326 and EPSG:3821 grids",
        )),
    }
}

fn regular_geo_transform(x: &[f64], y: &[f64]) -> Option<GeoTransform> {
    let dx = regular_step(x)?;
    let dy = regular_step(y)?;
    if dx <= 0.0 {
        return None;
    }
    let flip_y = dy > 0.0;
    let north_center = if flip_y { *y.last()? } else { y[0] };
    Some(GeoTransform {
        west: x[0] - dx / 2.0,
        north: north_center + dy.abs() / 2.0,
        dx,
        dy: dy.abs(),
        flip_y,
    })
}

fn regular_step(axis: &[f64]) -> Option<f64> {
    if axis.len() < 2 || axis.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let step = axis[1] - axis[0];
    if step == 0.0
        || axis.windows(2).any(|pair| {
            let difference = pair[1] - pair[0];
            difference.signum() != step.signum() || !approximately_equal(difference, step)
        })
    {
        return None;
    }
    Some(step)
}

fn web_mercator_y(latitude: f64) -> f64 {
    let latitude = latitude.to_radians();
    WEB_MERCATOR_RADIUS * (std::f64::consts::FRAC_PI_4 + latitude / 2.0).tan().ln()
}

fn approximately_equal(left: f64, right: f64) -> bool {
    let scale = left.abs().max(right.abs()).max(1.0);
    (left - right).abs() <= 1e-8 * right.abs() + 32.0 * f64::EPSILON * scale
}

fn geo_key_directory(epsg: u16) -> Vec<u16> {
    let keys: &[(u16, u16)] = match epsg {
        4326 => &[(1024, 2), (1025, 1), (2048, 4326), (2054, 9102)],
        3821 => &[(1024, 2), (1025, 1), (2048, 3821), (2054, 9102)],
        3857 => &[(1024, 1), (1025, 1), (2048, 4326), (2054, 9102), (3072, 3857), (3076, 9001)],
        _ => &[],
    };
    let mut directory = vec![1, 1, 0, keys.len() as u16];
    for (key, value) in keys {
        directory.extend_from_slice(&[*key, 0, 1, *value]);
    }
    directory
}

fn geographic_epsg(epsg: u16) -> bool {
    matches!(epsg, 4326 | 3821)
}

fn write_geotags<W: std::io::Write + std::io::Seek>(
    image: &mut tiff::encoder::ImageEncoder<'_, W, Gray32Float, TiffKindStandard>,
    transform: GeoTransform,
    epsg: u16,
    nodata: bool,
    description: &str,
) -> CoreResult<()> {
    image
        .encoder()
        .write_tag(Tag::ModelPixelScaleTag, [transform.dx, transform.dy, 0.0])
        .map_err(|_| storage_error("GeoTIFF pixel scale could not be written"))?;
    image
        .encoder()
        .write_tag(Tag::ModelTiepointTag, [0.0, 0.0, 0.0, transform.west, transform.north, 0.0])
        .map_err(|_| storage_error("GeoTIFF tie point could not be written"))?;
    image
        .encoder()
        .write_tag(Tag::GeoKeyDirectoryTag, geo_key_directory(epsg).as_slice())
        .map_err(|_| storage_error("GeoTIFF CRS keys could not be written"))?;
    image
        .encoder()
        .write_tag(Tag::ImageDescription, description)
        .map_err(|_| storage_error("GeoTIFF band description could not be written"))?;
    image
        .encoder()
        .write_tag(SOURCE_Y_FLIPPED_TAG, u16::from(transform.flip_y))
        .map_err(|_| storage_error("GeoTIFF source row direction could not be written"))?;
    if nodata {
        image
            .encoder()
            .write_tag(Tag::GdalNodata, "nan")
            .map_err(|_| storage_error("GeoTIFF nodata value could not be written"))?;
    }
    Ok(())
}

fn write_data_raster(
    path: &Path,
    field: &RadarField,
    width: usize,
    height: usize,
    transform: GeoTransform,
    epsg: u16,
) -> CoreResult<()> {
    let file =
        File::create(path).map_err(|_| storage_error("GeoTIFF data file could not be created"))?;
    let mut tiff = TiffEncoder::new(file)
        .map_err(|_| storage_error("GeoTIFF encoder could not be initialized"))?
        .with_compression(Compression::Deflate(DeflateLevel::Fast));
    let tiff_width = u32::try_from(width)
        .map_err(|_| CoreError::ResourceLimit("GeoTIFF width exceeds TIFF limits".into()))?;
    let tiff_height = u32::try_from(height)
        .map_err(|_| CoreError::ResourceLimit("GeoTIFF height exceeds TIFF limits".into()))?;
    let mut image = tiff
        .new_image::<Gray32Float>(tiff_width, tiff_height)
        .map_err(|_| storage_error("GeoTIFF data raster could not be initialized"))?;
    write_geotags(&mut image, transform, epsg, true, &field.name)?;
    image
        .rows_per_strip(height.clamp(1, STRIP_ROWS) as u32)
        .map_err(|_| storage_error("GeoTIFF strip size could not be configured"))?;
    let values = oriented_values(&field.values, width, height, transform.flip_y);
    image
        .write_data(values.as_ref())
        .map_err(|_| storage_error("GeoTIFF data raster could not be finalized"))?;
    sync_file(path)
}

fn write_quality_raster(
    path: &Path,
    field: &RadarField,
    width: usize,
    height: usize,
    transform: GeoTransform,
    epsg: u16,
) -> CoreResult<()> {
    let file = File::create(path)
        .map_err(|_| storage_error("GeoTIFF quality file could not be created"))?;
    let mut tiff = TiffEncoder::new(file)
        .map_err(|_| storage_error("GeoTIFF encoder could not be initialized"))?
        .with_compression(Compression::Deflate(DeflateLevel::Fast));
    let tiff_width = u32::try_from(width)
        .map_err(|_| CoreError::ResourceLimit("GeoTIFF width exceeds TIFF limits".into()))?;
    let tiff_height = u32::try_from(height)
        .map_err(|_| CoreError::ResourceLimit("GeoTIFF height exceeds TIFF limits".into()))?;
    let mut image = tiff
        .new_image::<Gray16>(tiff_width, tiff_height)
        .map_err(|_| storage_error("GeoTIFF quality raster could not be initialized"))?;
    image
        .encoder()
        .write_tag(Tag::ModelPixelScaleTag, [transform.dx, transform.dy, 0.0])
        .map_err(|_| storage_error("GeoTIFF pixel scale could not be written"))?;
    image
        .encoder()
        .write_tag(Tag::ModelTiepointTag, [0.0, 0.0, 0.0, transform.west, transform.north, 0.0])
        .map_err(|_| storage_error("GeoTIFF tie point could not be written"))?;
    image
        .encoder()
        .write_tag(Tag::GeoKeyDirectoryTag, geo_key_directory(epsg).as_slice())
        .map_err(|_| storage_error("GeoTIFF CRS keys could not be written"))?;
    image
        .encoder()
        .write_tag(Tag::ImageDescription, "quality")
        .map_err(|_| storage_error("GeoTIFF band description could not be written"))?;
    image
        .encoder()
        .write_tag(SOURCE_Y_FLIPPED_TAG, u16::from(transform.flip_y))
        .map_err(|_| storage_error("GeoTIFF source row direction could not be written"))?;
    image
        .rows_per_strip(height.clamp(1, STRIP_ROWS) as u32)
        .map_err(|_| storage_error("GeoTIFF strip size could not be configured"))?;
    let values = oriented_values(&field.quality, width, height, transform.flip_y);
    image
        .write_data(values.as_ref())
        .map_err(|_| storage_error("GeoTIFF quality raster could not be finalized"))?;
    sync_file(path)
}

fn oriented_values<T: Copy>(
    values: &[T],
    width: usize,
    height: usize,
    flip_y: bool,
) -> Cow<'_, [T]> {
    if !flip_y {
        return Cow::Borrowed(values);
    }
    let mut output = Vec::with_capacity(values.len());
    for row in (0..height).rev() {
        let start = row * width;
        output.extend_from_slice(&values[start..start + width]);
    }
    Cow::Owned(output)
}

fn write_provenance(path: &Path, field: &RadarField, geometry: GeoGeometry) -> CoreResult<()> {
    let source_crs = field.grid.crs.as_deref().unwrap_or("unknown");
    let mut document = json!({
        "schema_version": GEOTIFF_PROVENANCE_SCHEMA,
        "variable": field.name,
        "units": field.units,
        "grid": if geographic_epsg(geometry.epsg) { "geographic" } else { "projected" },
        "source_crs": source_crs,
        "crs": format!("EPSG:{}", geometry.epsg),
        "coordinate_operation": geometry.coordinate_operation,
        "transform": geometry.transform.coefficients(),
        "flip_y": geometry.transform.flip_y,
        "valid_time": field.valid_time,
        "processing_history": [],
        "provenance": field.provenance,
    });
    sort_json_keys(&mut document);
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|_| storage_error("GeoTIFF provenance could not be serialized"))?;
    fs::write(path, bytes).map_err(|_| storage_error("GeoTIFF provenance could not be written"))?;
    sync_file(path)
}

fn sort_json_keys(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(sort_json_keys),
        Value::Object(values) => {
            values.values_mut().for_each(sort_json_keys);
            values.sort_keys();
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn sync_file(path: &Path) -> CoreResult<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| storage_error("GeoTIFF staged file could not be flushed"))
}

fn publish_group(stage_root: &Path, outputs: &[(PathBuf, PathBuf); 3]) -> CoreResult<()> {
    let backup_root = stage_root.join("backup");
    fs::create_dir(&backup_root)
        .map_err(|_| storage_error("GeoTIFF backup directory could not be created"))?;
    let mut backups = Vec::new();
    for (index, (_, target)) in outputs.iter().enumerate() {
        match fs::symlink_metadata(target) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                let backup = backup_root.join(index.to_string());
                if let Err(error) = fs::rename(target, &backup) {
                    restore_group(&[], &backups);
                    return Err(storage_error(format!(
                        "GeoTIFF prior output could not be staged: {error}"
                    )));
                }
                backups.push((backup, target.clone()));
            }
            Ok(_) => {
                restore_group(&[], &backups);
                return Err(storage_error("GeoTIFF output target is not a regular file"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                restore_group(&[], &backups);
                return Err(storage_error("GeoTIFF output target could not be inspected"));
            }
        }
    }

    let mut installed = Vec::new();
    for (source, target) in outputs {
        if let Err(error) = fs::rename(source, target) {
            restore_group(&installed, &backups);
            return Err(storage_error(format!("GeoTIFF artifact publication failed: {error}")));
        }
        installed.push(target.clone());
    }
    Ok(())
}

fn restore_group(installed: &[PathBuf], backups: &[(PathBuf, PathBuf)]) {
    for path in installed {
        let _ = fs::remove_file(path);
    }
    for (backup, target) in backups.iter().rev() {
        let _ = fs::rename(backup, target);
    }
}
