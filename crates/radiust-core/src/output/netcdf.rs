//! NetCDF4 writer and bounded single-frame reader.

use crate::errors::{CoreError, CoreResult};
use crate::limits::Limits;
use crate::model::{Grid, RadarField, parse_utc_time};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use netcdf::{AttributeValue, Extent};
use std::path::{Path, PathBuf};

const QUALITY_FLAGS: [u16; 7] = [1, 2, 4, 8, 16, 32, 64];
const QUALITY_MEANINGS: &str = "missing outside_coverage unknown_color recovered interpolated below_detection source_annotation";

fn storage_error(message: impl Into<String>) -> CoreError {
    CoreError::Storage(message.into())
}

fn netcdf_error() -> CoreError {
    storage_error("NetCDF4 operation failed")
}

fn valid_variable_name(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Write one two-dimensional field as a CF-1.8 NetCDF4 file.
///
/// The file is staged next to its destination and atomically published only
/// after netCDF has closed successfully, so failed writes never leave a
/// partially valid destination.
pub fn write_field(
    field: &RadarField,
    destination: impl AsRef<Path>,
    limits: &Limits,
) -> CoreResult<PathBuf> {
    field.validate().map_err(|error| storage_error(error.to_string()))?;
    if field.shape.len() != 2 {
        return Err(storage_error(
            "NetCDF4 output currently requires a two-dimensional radar field",
        ));
    }
    if !valid_variable_name(&field.name)
        || matches!(
            field.name.as_str(),
            "time" | "x" | "y" | "longitude" | "latitude" | "crs" | "quality"
        )
    {
        return Err(storage_error("NetCDF variable name is invalid or reserved"));
    }
    let pixels = field
        .shape
        .iter()
        .try_fold(1_u64, |total, size| total.checked_mul(*size as u64))
        .ok_or_else(|| CoreError::ResourceLimit("NetCDF field shape overflows".into()))?;
    limits.validate_pixels(pixels)?;
    let expected_bytes = pixels
        .checked_mul((std::mem::size_of::<f32>() + std::mem::size_of::<u16>()) as u64)
        .ok_or_else(|| CoreError::ResourceLimit("NetCDF field byte size overflows".into()))?;
    if expected_bytes > limits.max_frame_bytes || expected_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "NetCDF output exceeds configured frame or temporary-file limit".into(),
        ));
    }

    let destination = destination.as_ref();
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|_| storage_error("NetCDF output directory could not be prepared"))?;
    let staging = tempfile::Builder::new()
        .prefix(".radiust-netcdf-")
        .tempfile_in(parent)
        .map_err(|_| storage_error("NetCDF staging file could not be created"))?;
    let staging_path = staging.into_temp_path();

    write_staged_field(field, &staging_path)?;
    staging_path
        .persist(destination)
        .map_err(|_| storage_error("NetCDF file could not be atomically published"))?;
    Ok(destination.to_path_buf())
}

fn write_staged_field(field: &RadarField, path: &Path) -> CoreResult<()> {
    let height = field.shape[0];
    let width = field.shape[1];
    let valid_time = parse_utc_time(&field.valid_time)
        .map_err(|_| storage_error("NetCDF field valid_time is invalid"))?;
    let mut file = netcdf::create(path).map_err(|_| netcdf_error())?;
    file.add_dimension("y", height).map_err(|_| netcdf_error())?;
    file.add_dimension("x", width).map_err(|_| netcdf_error())?;
    file.add_attribute("Conventions", "CF-1.8").map_err(|_| netcdf_error())?;
    file.add_attribute("radiust_valid_time", field.valid_time.as_str())
        .map_err(|_| netcdf_error())?;
    file.add_attribute(
        "radiust_provenance",
        serde_json::to_string(&field.provenance).map_err(|_| netcdf_error())?,
    )
    .map_err(|_| netcdf_error())?;
    file.add_attribute(
        "radiust_affine",
        serde_json::to_string(&field.grid.affine).map_err(|_| netcdf_error())?,
    )
    .map_err(|_| netcdf_error())?;

    {
        let mut time = file.add_variable::<f64>("time", &[]).map_err(|_| netcdf_error())?;
        time.put_attribute("standard_name", "time").map_err(|_| netcdf_error())?;
        time.put_attribute("units", "seconds since 1970-01-01 00:00:00 UTC")
            .map_err(|_| netcdf_error())?;
        time.put_attribute("calendar", "proleptic_gregorian").map_err(|_| netcdf_error())?;
        let epoch_seconds = valid_time.timestamp() as f64
            + f64::from(valid_time.timestamp_subsec_nanos()) / 1_000_000_000.0;
        time.put_value(epoch_seconds, ()).map_err(|_| netcdf_error())?;
    }

    let crs = field.grid.crs.as_deref().unwrap_or("unknown");
    {
        let mut crs_variable = file.add_variable::<i32>("crs", &[]).map_err(|_| netcdf_error())?;
        crs_variable
            .put_attribute("long_name", "coordinate reference system")
            .map_err(|_| netcdf_error())?;
        crs_variable.put_attribute("spatial_ref", crs).map_err(|_| netcdf_error())?;
        crs_variable.put_attribute("units", "1").map_err(|_| netcdf_error())?;
        if crs.eq_ignore_ascii_case("EPSG:4326") {
            crs_variable
                .put_attribute("grid_mapping_name", "latitude_longitude")
                .map_err(|_| netcdf_error())?;
            crs_variable
                .put_attribute("semi_major_axis", 6_378_137.0_f64)
                .map_err(|_| netcdf_error())?;
            crs_variable
                .put_attribute("inverse_flattening", 298.257_223_563_f64)
                .map_err(|_| netcdf_error())?;
        }
        crs_variable.put_value(0, ()).map_err(|_| netcdf_error())?;
    }

    if !field.grid.x.is_empty() {
        let coordinate_name = if crs.eq_ignore_ascii_case("EPSG:4326") { "longitude" } else { "x" };
        let mut x =
            file.add_variable::<f64>(coordinate_name, &["x"]).map_err(|_| netcdf_error())?;
        if coordinate_name == "longitude" {
            x.put_attribute("standard_name", "longitude").map_err(|_| netcdf_error())?;
            x.put_attribute("units", "degrees_east").map_err(|_| netcdf_error())?;
        }
        x.put_values(&field.grid.x, ..).map_err(|_| netcdf_error())?;
    }
    if !field.grid.y.is_empty() {
        let coordinate_name = if crs.eq_ignore_ascii_case("EPSG:4326") { "latitude" } else { "y" };
        let mut y =
            file.add_variable::<f64>(coordinate_name, &["y"]).map_err(|_| netcdf_error())?;
        if coordinate_name == "latitude" {
            y.put_attribute("standard_name", "latitude").map_err(|_| netcdf_error())?;
            y.put_attribute("units", "degrees_north").map_err(|_| netcdf_error())?;
        }
        y.put_values(&field.grid.y, ..).map_err(|_| netcdf_error())?;
    }

    {
        let mut data =
            file.add_variable::<f32>(&field.name, &["y", "x"]).map_err(|_| netcdf_error())?;
        data.put_attribute("long_name", field.name.replace('_', " "))
            .map_err(|_| netcdf_error())?;
        data.put_attribute("grid_mapping", "crs").map_err(|_| netcdf_error())?;
        data.put_attribute("ancillary_variables", "quality").map_err(|_| netcdf_error())?;
        let coordinates = if crs.eq_ignore_ascii_case("EPSG:4326") {
            "time latitude longitude"
        } else {
            "time y x"
        };
        data.put_attribute("coordinates", coordinates).map_err(|_| netcdf_error())?;
        if let Some(units) = &field.units {
            data.put_attribute("units", units.as_str()).map_err(|_| netcdf_error())?;
        }
        data.put_values(&field.values, ..).map_err(|_| netcdf_error())?;
    }
    {
        let mut quality =
            file.add_variable::<u16>("quality", &["y", "x"]).map_err(|_| netcdf_error())?;
        quality.put_attribute("long_name", "quality flags").map_err(|_| netcdf_error())?;
        quality
            .put_attribute(
                "flag_masks",
                QUALITY_FLAGS.iter().map(|flag| i32::from(*flag)).collect::<Vec<_>>(),
            )
            .map_err(|_| netcdf_error())?;
        quality.put_attribute("flag_meanings", QUALITY_MEANINGS).map_err(|_| netcdf_error())?;
        quality.put_values(&field.quality, ..).map_err(|_| netcdf_error())?;
    }

    file.close().map_err(|_| netcdf_error())
}

/// Read one field from a local NetCDF file. If the selected variable has a
/// leading `time` dimension with multiple entries, `valid_time` is required;
/// the time slice is chosen before the field and quality arrays are loaded.
pub fn read_selected_field(
    path: impl AsRef<Path>,
    variable_name: &str,
    valid_time: Option<&str>,
    limits: &Limits,
) -> CoreResult<RadarField> {
    if !valid_variable_name(variable_name) {
        return Err(storage_error("NetCDF variable name is invalid"));
    }
    let file = netcdf::open(path).map_err(|_| storage_error("NetCDF file could not be opened"))?;
    let variable = file
        .variable(variable_name)
        .ok_or_else(|| storage_error("NetCDF variable was not found"))?;
    let dimensions = variable.dimensions();
    let is_time_series = dimensions.len() == 3 && dimensions[0].name() == "time";
    if dimensions.len() != 2 && !is_time_series {
        return Err(storage_error("NetCDF variable must have [y, x] or [time, y, x] dimensions"));
    }
    if dimensions.len() == 3 && !is_time_series {
        return Err(storage_error("NetCDF time dimension must be the leading field dimension"));
    }
    let height = dimensions[dimensions.len() - 2].len();
    let width = dimensions[dimensions.len() - 1].len();
    let pixels = (height as u64)
        .checked_mul(width as u64)
        .ok_or_else(|| CoreError::ResourceLimit("NetCDF field shape overflows".into()))?;
    limits.validate_pixels(pixels)?;
    let frame_bytes = pixels
        .checked_mul((std::mem::size_of::<f32>() + std::mem::size_of::<u16>()) as u64)
        .ok_or_else(|| CoreError::ResourceLimit("NetCDF field byte size overflows".into()))?;
    if frame_bytes > limits.max_frame_bytes || frame_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "NetCDF field exceeds configured frame or temporary-file limit".into(),
        ));
    }

    let axis =
        read_time_axis(&file, if is_time_series { Some(dimensions[0].len()) } else { None })?;
    let requested_time = valid_time
        .map(parse_utc_time)
        .transpose()
        .map_err(|_| storage_error("requested NetCDF valid_time is invalid"))?;
    let (time_index, selected_time) = match (is_time_series, axis.as_deref(), requested_time) {
        (true, Some(times), Some(requested)) => {
            let index = times.iter().position(|time| *time == requested).ok_or_else(|| {
                storage_error("requested time is absent from the NetCDF variable")
            })?;
            (Some(index), times[index])
        }
        (true, Some(times), None) if times.len() == 1 => (Some(0), times[0]),
        (true, Some(times), None) if times.len() > 1 => {
            return Err(storage_error("NetCDF time selection is ambiguous; provide valid_time"));
        }
        (true, Some(_), None) => {
            return Err(storage_error("NetCDF time coordinate is empty"));
        }
        (true, None, _) => {
            return Err(storage_error("NetCDF time coordinate is missing or invalid"));
        }
        (false, Some(times), Some(requested)) if times.first() == Some(&requested) => {
            (None, requested)
        }
        (false, Some(_), Some(_)) => {
            return Err(storage_error("requested time does not match the NetCDF field"));
        }
        (false, Some(times), None) if times.len() == 1 => (None, times[0]),
        (false, Some(_), None) => {
            return Err(storage_error(
                "NetCDF time coordinate must contain one value for a two-dimensional field",
            ));
        }
        (false, None, _) => return Err(storage_error("NetCDF valid_time coordinate is missing")),
    };

    let extents = field_extents(time_index, height, width);
    let mut values = variable.get_values::<f32, _>(extents.clone()).map_err(|_| netcdf_error())?;
    let mut quality = if let Some(quality_variable) = file.variable("quality") {
        let quality_dimensions = quality_variable.dimensions();
        if quality_dimensions.len() != dimensions.len()
            || quality_dimensions.iter().zip(dimensions.iter()).any(|(quality, field)| {
                quality.name() != field.name() || quality.len() != field.len()
            })
        {
            return Err(storage_error(
                "NetCDF quality dimensions do not match the selected variable",
            ));
        }
        quality_variable
            .get_values::<u16, _>(extents)
            .map_err(|_| storage_error("NetCDF quality values could not be read"))?
    } else {
        vec![0; values.len()]
    };
    if values.len() != pixels as usize || quality.len() != values.len() {
        return Err(storage_error(
            "NetCDF values and quality do not match the selected frame shape",
        ));
    }
    if let Some(fill) = variable.fill_value::<f32>().ok().flatten() {
        for (value, flag) in values.iter_mut().zip(&mut quality) {
            if *value == fill {
                *value = f32::NAN;
                *flag |= 1;
            }
        }
    }

    let grid = read_grid(&file, height, width)?;
    let units = text_attribute(&variable, "units");
    let provenance = text_attribute(&file, "radiust_provenance")
        .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
        .unwrap_or_default();
    let result = RadarField {
        name: variable_name.to_owned(),
        values,
        shape: vec![height, width],
        quality,
        units,
        valid_time: selected_time.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        grid,
        provenance,
    };
    result.validate().map_err(|error| storage_error(error.to_string()))?;
    Ok(result)
}

fn field_extents(time_index: Option<usize>, height: usize, width: usize) -> Vec<Extent> {
    let mut extents = Vec::with_capacity(if time_index.is_some() { 3 } else { 2 });
    if let Some(index) = time_index {
        extents.push(Extent::Index(index));
    }
    extents.push((0..height).into());
    extents.push((0..width).into());
    extents
}

fn read_time_axis(
    file: &netcdf::File,
    expected_len: Option<usize>,
) -> CoreResult<Option<Vec<DateTime<Utc>>>> {
    let Some(variable) = file.variable("time") else {
        let Some(value) = text_attribute(file, "radiust_valid_time") else {
            return Ok(None);
        };
        let values = vec![
            parse_utc_time(&value)
                .map_err(|_| storage_error("NetCDF valid_time attribute is invalid"))?,
        ];
        if expected_len.is_some_and(|length| length != values.len()) {
            return Err(storage_error(
                "NetCDF time coordinate length does not match its dimension",
            ));
        }
        return Ok(Some(values));
    };
    let values = if variable.dimensions().is_empty() {
        vec![
            variable
                .get_value::<f64, _>(())
                .map_err(|_| storage_error("NetCDF time coordinate is not numeric"))?,
        ]
    } else {
        variable
            .get_values::<f64, _>(..)
            .map_err(|_| storage_error("NetCDF time coordinate is not numeric"))?
    };
    if expected_len.is_some_and(|length| length != values.len()) {
        return Err(storage_error("NetCDF time coordinate length does not match its dimension"));
    }
    let units = text_attribute(&variable, "units")
        .ok_or_else(|| storage_error("NetCDF time coordinate is missing CF units"))?;
    let mut values = values
        .into_iter()
        .map(|value| {
            parse_cf_time(value, &units)
                .ok_or_else(|| storage_error("NetCDF time coordinate is invalid"))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    if expected_len.is_none()
        && values.len() == 1
        && let Some(exact_time) = text_attribute(file, "radiust_valid_time")
    {
        let exact_time = parse_utc_time(&exact_time)
            .map_err(|_| storage_error("NetCDF valid_time attribute is invalid"))?;
        let difference = exact_time.signed_duration_since(values[0]);
        if difference.num_nanoseconds().is_none_or(|nanos| nanos.unsigned_abs() > 1_000) {
            return Err(storage_error("NetCDF valid_time attribute conflicts with its coordinate"));
        }
        values[0] = exact_time;
    }
    Ok(Some(values))
}

fn parse_cf_time(value: f64, units: &str) -> Option<DateTime<Utc>> {
    if !value.is_finite() {
        return None;
    }
    let (unit, origin) = units.split_once(" since ")?;
    let seconds_per_unit = match unit.trim().to_ascii_lowercase().as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
        "min" | "mins" | "minute" | "minutes" => 60.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600.0,
        "d" | "day" | "days" => 86_400.0,
        "millisecond" | "milliseconds" | "msec" => 0.001,
        "microsecond" | "microseconds" | "usec" => 0.000_001,
        "nanosecond" | "nanoseconds" | "nsec" => 0.000_000_001,
        _ => return None,
    };
    let origin = parse_time_origin(origin)?;
    let nanos = (value * seconds_per_unit * 1_000_000_000.0).round();
    if !nanos.is_finite() || nanos < i64::MIN as f64 || nanos > i64::MAX as f64 {
        return None;
    }
    origin.checked_add_signed(Duration::nanoseconds(nanos as i64))
}

fn parse_time_origin(origin: &str) -> Option<DateTime<Utc>> {
    let value = origin.trim().trim_end_matches(" UTC").trim_end_matches(" utc").trim();
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }
    for format in
        ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"]
    {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Some(parsed.and_utc());
        }
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|value| value.and_utc())
}

fn read_grid(file: &netcdf::File, height: usize, width: usize) -> CoreResult<Grid> {
    let (x_name, y_name) = if file.variable("x").is_some() || file.variable("y").is_some() {
        ("x", "y")
    } else {
        ("longitude", "latitude")
    };
    let x = read_coordinate(file, x_name, width)?;
    let y = read_coordinate(file, y_name, height)?;
    let crs = file
        .variable("crs")
        .and_then(|variable| text_attribute(&variable, "spatial_ref"))
        .or_else(|| text_attribute(file, "spatial_ref"));
    let affine = text_attribute(file, "radiust_affine")
        .and_then(|value| serde_json::from_str::<Option<[f64; 6]>>(&value).ok())
        .flatten();
    Ok(Grid { shape: vec![height, width], crs, x, y, affine })
}

fn read_coordinate(file: &netcdf::File, name: &str, expected_len: usize) -> CoreResult<Vec<f64>> {
    let Some(variable) = file.variable(name) else {
        return Ok(Vec::new());
    };
    let values = variable
        .get_values::<f64, _>(..)
        .map_err(|_| storage_error("NetCDF grid coordinate could not be read"))?;
    if values.len() != expected_len {
        return Err(storage_error("NetCDF grid coordinate length does not match its dimension"));
    }
    Ok(values)
}

fn text_attribute<T>(variable: &T, name: &str) -> Option<String>
where
    T: AttributeSource,
{
    match variable.attribute_value(name)?.ok()? {
        AttributeValue::Str(value) => Some(value),
        AttributeValue::Strs(mut values) if values.len() == 1 => values.pop(),
        _ => None,
    }
}

trait AttributeSource {
    fn attribute_value(&self, name: &str) -> Option<netcdf::Result<AttributeValue>>;
}

impl AttributeSource for netcdf::Variable<'_> {
    fn attribute_value(&self, name: &str) -> Option<netcdf::Result<AttributeValue>> {
        netcdf::Variable::attribute_value(self, name)
    }
}

impl AttributeSource for netcdf::File {
    fn attribute_value(&self, name: &str) -> Option<netcdf::Result<AttributeValue>> {
        self.root()?.attribute_value(name)
    }
}
