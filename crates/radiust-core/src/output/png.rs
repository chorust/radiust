//! PNG rendering and a stable JSON description of the rendering parameters.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use image::{ImageFormat, RgbaImage};
use serde_json::{Value, json};

use crate::errors::{CoreError, CoreResult};
use crate::limits::Limits;
use crate::model::{Preview, PreviewMode, RadarField};

const PALETTE_ID: &str = "default";
const PALETTE_VERSION: &str = "1";
const PALETTE_COLORS: [[u8; 3]; 6] =
    [[0, 0, 0], [43, 131, 186], [171, 221, 164], [255, 255, 191], [253, 174, 97], [215, 25, 28]];
const INVALID_QUALITY: u16 = 1 | 2 | 4;

/// Write a radar field as an RGBA PNG with a deterministic render sidecar.
///
/// Supported options are palette, vmin, vmax, and variable. The only currently
/// defined palette is default version 1.
pub fn write_png(
    field: &RadarField,
    output: impl AsRef<Path>,
    options: &Value,
) -> CoreResult<Vec<PathBuf>> {
    validate_field(field)?;
    let options = parse_options(field, options)?;
    let [height, width] = field.shape.as_slice() else {
        return Err(storage_error("PNG output requires a two-dimensional field"));
    };
    let width = *width;
    let height = *height;
    let width_u32 =
        u32::try_from(width).map_err(|_| storage_error("PNG width exceeds image limits"))?;
    let height_u32 =
        u32::try_from(height).map_err(|_| storage_error("PNG height exceeds image limits"))?;
    let (vmin, vmax) = value_range(field, options.vmin, options.vmax)?;
    let rgba = render_pixels(field, width, height, vmin, vmax)?;

    let output = output.as_ref().to_path_buf();
    let image = RgbaImage::from_raw(width_u32, height_u32, rgba)
        .ok_or_else(|| storage_error("RGBA pixels do not match image dimensions"))?;
    image
        .save_with_format(&output, ImageFormat::Png)
        .map_err(|error| storage_error(format!("PNG encoding failed: {error}")))?;

    let sidecar = sidecar_path(&output);
    let mut sidecar_document = sidecar_document(field, width, height, vmin, vmax);
    sort_json_keys(&mut sidecar_document);
    let sidecar_json = serde_json::to_string_pretty(&sidecar_document)
        .map_err(|error| storage_error(format!("render sidecar serialization failed: {error}")))?;
    std::fs::write(&sidecar, sidecar_json)
        .map_err(|error| storage_error(format!("render sidecar write failed: {error}")))?;
    Ok(vec![output, sidecar])
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

/// Render an in-memory field into the same decoded RGBA preview used by PNG
/// output, without creating temporary files or a sidecar.
pub fn preview_field(field: &RadarField, limits: &Limits) -> CoreResult<Preview> {
    preview_field_with_options(field, limits, &Value::Null)
}

/// Render an in-memory scientific field with the supported PNG palette/range
/// options, without creating temporary files or a sidecar.
pub fn preview_field_with_options(
    field: &RadarField,
    limits: &Limits,
    options: &Value,
) -> CoreResult<Preview> {
    validate_field(field)?;
    let options = parse_options(field, options)?;
    let [height, width] = field.shape.as_slice() else {
        return Err(storage_error("PNG preview requires a two-dimensional field"));
    };
    let pixels = (*height as u64)
        .checked_mul(*width as u64)
        .ok_or_else(|| CoreError::ResourceLimit("preview dimensions overflow".into()))?;
    limits.validate_pixels(pixels)?;
    let bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| CoreError::ResourceLimit("preview buffer size overflows".into()))?;
    if bytes > limits.max_frame_bytes || bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit("preview exceeds configured byte limits".into()));
    }
    let width_u32 =
        u32::try_from(*width).map_err(|_| storage_error("preview width exceeds image limits"))?;
    let height_u32 =
        u32::try_from(*height).map_err(|_| storage_error("preview height exceeds image limits"))?;
    let (vmin, vmax) = value_range(field, options.vmin, options.vmax)?;
    let rgba = render_pixels(field, *width, *height, vmin, vmax)?;
    let preview = Preview {
        width: width_u32,
        height: height_u32,
        rgba,
        frame: None,
        mode: PreviewMode::Decoded,
        rule_version: Some(format!("{PALETTE_ID}-{PALETTE_VERSION}")),
    };
    preview.validate().map_err(|_| storage_error("rendered preview is invalid"))?;
    Ok(preview)
}

#[derive(Clone, Copy, Debug, Default)]
struct RenderOptions {
    vmin: Option<f64>,
    vmax: Option<f64>,
}

fn parse_options(field: &RadarField, options: &Value) -> CoreResult<RenderOptions> {
    if !options.is_null() && !options.is_object() {
        return Err(storage_error("PNG options must be a JSON object"));
    }

    if let Some(palette) = option_string(options, "palette")?
        && palette != PALETTE_ID
    {
        return Err(storage_error(format!("unknown palette: {palette}")));
    }
    if let Some(variable) = option_string(options, "variable")?
        && variable != field.name
    {
        return Err(storage_error(format!("unknown variable: {variable}")));
    }
    Ok(RenderOptions {
        vmin: option_number(options, "vmin")?,
        vmax: option_number(options, "vmax")?,
    })
}

fn option_string<'a>(options: &'a Value, name: &str) -> CoreResult<Option<&'a str>> {
    let Some(value) = options.get(name) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_str()
        .map(Some)
        .ok_or_else(|| storage_error(format!("PNG option {name} must be a string or null")))
}

fn option_number(options: &Value, name: &str) -> CoreResult<Option<f64>> {
    let Some(value) = options.get(name) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = value.as_f64().ok_or_else(|| {
        storage_error(format!("PNG option {name} must be a finite number or null"))
    })?;
    if !number.is_finite() {
        return Err(storage_error(format!("PNG option {name} must be finite")));
    }
    Ok(Some(number))
}

fn validate_field(field: &RadarField) -> CoreResult<()> {
    field
        .validate()
        .map_err(|_| storage_error("field values, quality, and grid shape must agree"))?;
    let [height, width] = field.shape.as_slice() else {
        return Err(storage_error("PNG output requires a two-dimensional field"));
    };
    if *height == 0 || *width == 0 {
        return Err(storage_error("PNG dimensions must be positive"));
    }
    crate::identity::normalize_time(&field.valid_time)
        .map_err(|_| storage_error("field valid_time must be ISO-8601 with a timezone"))?;
    validate_axis(&field.grid.x, *width, "x")?;
    validate_axis(&field.grid.y, *height, "y")?;
    Ok(())
}

fn validate_axis(axis: &[f64], expected: usize, name: &str) -> CoreResult<()> {
    if axis.is_empty() {
        // An empty axis means the model has no coordinate values to orient by.
        return Ok(());
    }
    if axis.len() != expected || axis.iter().any(|value| !value.is_finite()) {
        return Err(storage_error(format!(
            "{name} coordinates must be finite and match the grid shape"
        )));
    }
    let mut direction = 0_i8;
    for pair in axis.windows(2) {
        let next = if pair[1] > pair[0] {
            1
        } else if pair[1] < pair[0] {
            -1
        } else {
            return Err(storage_error(format!("{name} coordinates must be strictly monotonic")));
        };
        if direction != 0 && direction != next {
            return Err(storage_error(format!("{name} coordinates must be strictly monotonic")));
        }
        direction = next;
    }
    Ok(())
}

fn value_range(
    field: &RadarField,
    selected_min: Option<f64>,
    selected_max: Option<f64>,
) -> CoreResult<(f64, f64)> {
    let mut finite_range: Option<(f64, f64)> = None;
    for &value in &field.values {
        if value.is_finite() {
            let value = f64::from(value);
            finite_range = Some(match finite_range {
                Some((minimum, maximum)) => (minimum.min(value), maximum.max(value)),
                None => (value, value),
            });
        }
    }
    let (minimum, maximum) = finite_range.unwrap_or((0.0, 1.0));
    let (vmin, vmax) = if minimum == maximum && selected_min.is_none() && selected_max.is_none() {
        // A constant field is still a valid image. Expand a stable relative
        // range so every finite sample maps to the palette midpoint. Explicit
        // caller limits retain the strict vmin < vmax validation below.
        let margin = minimum.abs().max(1.0) * 0.01;
        (minimum - margin, maximum + margin)
    } else {
        (selected_min.unwrap_or(minimum), selected_max.unwrap_or(maximum))
    };
    let range = vmax - vmin;
    if !range.is_finite() || vmax <= vmin {
        return Err(storage_error("vmax must be greater than vmin"));
    }
    Ok((vmin, vmax))
}

fn render_pixels(
    field: &RadarField,
    width: usize,
    height: usize,
    vmin: f64,
    vmax: f64,
) -> CoreResult<Vec<u8>> {
    let pixel_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| storage_error("PNG pixel buffer dimensions overflow"))?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(pixel_bytes)
        .map_err(|error| storage_error(format!("PNG pixel buffer allocation failed: {error}")))?;
    rgba.resize(pixel_bytes, 0);

    let reverse_rows = field.grid.y.len() > 1 && field.grid.y[0] < field.grid.y[height - 1];
    let reverse_columns = field.grid.x.len() > 1 && field.grid.x[0] > field.grid.x[width - 1];
    let range = vmax - vmin;
    for output_row in 0..height {
        let source_row = if reverse_rows { height - output_row - 1 } else { output_row };
        for output_column in 0..width {
            let source_column =
                if reverse_columns { width - output_column - 1 } else { output_column };
            let source_index = source_row * width + source_column;
            let output_index = (output_row * width + output_column) * 4;
            let value = field.values[source_index];
            if !value.is_finite() {
                continue;
            }
            let color = interpolate_color(f64::from(value), vmin, range);
            rgba[output_index..output_index + 3].copy_from_slice(&color);
            if field.quality[source_index] & INVALID_QUALITY == 0 {
                rgba[output_index + 3] = 255;
            }
        }
    }
    Ok(rgba)
}

fn interpolate_color(value: f64, vmin: f64, range: f64) -> [u8; 3] {
    let normalized = ((value - vmin) / range).clamp(0.0, 1.0);
    let position = normalized * (PALETTE_COLORS.len() - 1) as f64;
    let lower = (position.floor() as usize).min(PALETTE_COLORS.len() - 2);
    let fraction = position - lower as f64;
    std::array::from_fn(|channel| {
        let start = f64::from(PALETTE_COLORS[lower][channel]);
        let end = f64::from(PALETTE_COLORS[lower + 1][channel]);
        (start + (end - start) * fraction) as u8
    })
}

fn sidecar_path(output: &Path) -> PathBuf {
    let mut name: OsString = output.file_stem().unwrap_or(output.as_os_str()).to_os_string();
    name.push(".render.json");
    output.with_file_name(name)
}

fn sidecar_document(
    field: &RadarField,
    width: usize,
    height: usize,
    vmin: f64,
    vmax: f64,
) -> Value {
    let legend = (0..5)
        .map(|index| {
            let value = match index {
                0 => vmin,
                4 => vmax,
                _ => vmin + (vmax - vmin) * (index as f64 / 4.0),
            };
            format_general(value)
        })
        .collect::<Vec<_>>();
    let valid_time = crate::identity::normalize_time(&field.valid_time)
        .unwrap_or_else(|_| field.valid_time.clone());
    let title = format!(
        "{} / {} / {} / {} / {} [{}]",
        provenance_value(field, "source").unwrap_or("unknown"),
        provenance_value(field, "product").unwrap_or("unknown"),
        provenance_value(field, "station").unwrap_or("unknown"),
        valid_time,
        field.name,
        field.units.as_deref().unwrap_or("unknown"),
    );
    json!({
        "schema_version": 1,
        "palette": {"id": PALETTE_ID, "version": PALETTE_VERSION},
        "vmin": vmin,
        "vmax": vmax,
        "shape": [height, width],
        "grid": grid_kind(field),
        "crs": &field.grid.crs,
        "variable": &field.name,
        "title": title,
        "legend": legend,
        "provenance": provenance_document(field),
    })
}

fn provenance_document(field: &RadarField) -> Value {
    let mut document = serde_json::Map::new();
    for entry in &field.provenance {
        if let Some((key, value)) = entry.split_once('=') {
            if !key.is_empty() {
                document.insert(key.to_owned(), Value::String(value.to_owned()));
            }
        } else {
            document.insert(entry.clone(), Value::Bool(true));
        }
    }
    Value::Object(document)
}

fn provenance_value<'a>(field: &'a RadarField, key: &str) -> Option<&'a str> {
    field.provenance.iter().find_map(|entry| {
        let (entry_key, value) = entry.split_once('=')?;
        (entry_key == key).then_some(value)
    })
}

fn grid_kind(field: &RadarField) -> &'static str {
    match field.grid.crs.as_deref().map(str::to_ascii_uppercase).as_deref() {
        Some("EPSG:4326" | "OGC:CRS84" | "CRS84" | "URN:OGC:DEF:CRS:EPSG::4326") => "geographic",
        _ => "cartesian",
    }
}

/// Format values like Python's default g formatter, with six significant digits.
fn format_general(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let magnitude = value.abs();
    let exponent = magnitude.log10().floor() as i32;
    if !(-4..6).contains(&exponent) {
        let scientific = format!("{value:.5e}");
        let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
        let mantissa = trim_decimal(mantissa.to_owned());
        let exponent = exponent.parse::<i32>().unwrap_or_default();
        format!("{mantissa}e{exponent:+03}")
    } else {
        let precision = (5 - exponent).max(0) as usize;
        trim_decimal(format!("{value:.precision$}"))
    }
}

fn trim_decimal(mut value: String) -> String {
    if value.contains('.') {
        while value.ends_with('0') {
            value.pop();
        }
        if value.ends_with('.') {
            value.pop();
        }
    }
    value
}

fn storage_error(message: impl Into<String>) -> CoreError {
    CoreError::Storage(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Grid;

    fn field(
        values: Vec<f32>,
        quality: Vec<u16>,
        shape: [usize; 2],
        x: Vec<f64>,
        y: Vec<f64>,
    ) -> RadarField {
        RadarField {
            name: "reflectivity".into(),
            values,
            shape: shape.to_vec(),
            quality,
            units: Some("dBZ".into()),
            valid_time: "2026-09-24T00:00:00Z".into(),
            grid: Grid { shape: shape.to_vec(), crs: Some("EPSG:4326".into()), x, y, affine: None },
            provenance: vec![
                "source=test".into(),
                "product=composite".into(),
                "station=east".into(),
            ],
        }
    }

    fn saved_rgba(path: &Path) -> Vec<u8> {
        image::open(path).unwrap().to_rgba8().into_raw()
    }

    #[test]
    fn default_palette_uses_linear_interpolation_between_color_stops() {
        assert_eq!(interpolate_color(0.4, 0.0, 1.0), [171, 221, 164]);
        assert_eq!(interpolate_color(0.5, 0.0, 1.0), [213, 238, 177]);
        assert_eq!(interpolate_color(0.25, 0.0, 1.0), [75, 153, 180]);
    }

    #[test]
    fn ascending_y_and_descending_x_are_reversed_in_the_png() {
        let field = field(
            vec![1.0, 0.0, 3.0, 2.0],
            vec![0; 4],
            [2, 2],
            vec![20.0, 10.0],
            vec![-20.0, 20.0],
        );
        let rgba = render_pixels(&field, 2, 2, 0.0, 3.0).unwrap();
        assert_eq!(&rgba[0..3], &interpolate_color(2.0, 0.0, 3.0));
        assert_eq!(&rgba[4..7], &interpolate_color(3.0, 0.0, 3.0));
        assert_eq!(&rgba[8..11], &interpolate_color(0.0, 0.0, 3.0));
        assert_eq!(&rgba[12..15], &interpolate_color(1.0, 0.0, 3.0));
    }

    #[test]
    fn constant_decoded_fields_produce_a_midpoint_palette_preview() {
        let field = field(vec![3.0; 4], vec![0; 4], [2, 2], vec![], vec![]);
        let preview = preview_field(&field, &Limits::default()).unwrap();
        let midpoint = [213, 238, 177];
        assert_eq!((preview.width, preview.height), (2, 2));
        for pixel in preview.rgba.chunks_exact(4) {
            assert_eq!(&pixel[..3], midpoint);
            assert_eq!(pixel[3], 255);
        }
    }

    #[test]
    fn decoded_preview_uses_legacy_palette_and_value_range_options() {
        let field = field(vec![0.0, 1.0], vec![0, 0], [1, 2], vec![], vec![]);
        let preview = preview_field_with_options(
            &field,
            &Limits::default(),
            &json!({"palette": "default", "vmin": 0.0, "vmax": 0.5}),
        )
        .unwrap();
        assert_eq!(&preview.rgba[0..3], &PALETTE_COLORS[0]);
        assert_eq!(&preview.rgba[4..7], &PALETTE_COLORS[5]);
        assert_eq!(preview.rule_version.as_deref(), Some("default-1"));

        let error =
            preview_field_with_options(&field, &Limits::default(), &json!({"palette": "rainbow"}))
                .unwrap_err();
        assert!(
            matches!(error, CoreError::Storage(message) if message.contains("unknown palette"))
        );
    }

    #[test]
    fn non_finite_values_and_quality_flags_are_transparent() {
        let field = field(
            vec![0.0, 1.0, f32::NAN, f32::INFINITY, 2.0],
            vec![0, 1, 0, 0, 2 | 4],
            [1, 5],
            vec![],
            vec![],
        );
        let rgba = render_pixels(&field, 5, 1, 0.0, 2.0).unwrap();
        assert_eq!(rgba[3], 255);
        assert_eq!(rgba[7], 0);
        assert_eq!(&rgba[8..12], &[0, 0, 0, 0]);
        assert_eq!(&rgba[12..16], &[0, 0, 0, 0]);
        assert_eq!(rgba[19], 0);
        // A quality-masked finite pixel retains RGB under alpha 0, like Python.
        assert_eq!(&rgba[4..7], &interpolate_color(1.0, 0.0, 2.0));
    }

    #[test]
    fn writer_returns_png_and_stable_v1_sidecar_paths_and_fields() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("radar.sample.png");
        let field = field(vec![0.0, 1.0], vec![0, 0], [1, 2], vec![10.0, 11.0], vec![5.0]);
        let options = json!({
            "palette": "default",
            "vmin": -1.0,
            "vmax": 2.0,
            "variable": "reflectivity",
        });
        let paths = write_png(&field, &output, &options).unwrap();
        assert_eq!(paths[0], output);
        assert_eq!(paths[1], directory.path().join("radar.sample.render.json"));
        assert_eq!(saved_rgba(&paths[0]).len(), 8);

        let sidecar_text = std::fs::read_to_string(&paths[1]).unwrap();
        let sidecar: Value = serde_json::from_str(&sidecar_text).unwrap();
        assert_eq!(sidecar["schema_version"], 1);
        assert_eq!(sidecar["palette"], json!({"id": "default", "version": "1"}));
        assert_eq!(sidecar["vmin"], -1.0);
        assert_eq!(sidecar["vmax"], 2.0);
        assert_eq!(sidecar["shape"], json!([1, 2]));
        assert_eq!(sidecar["grid"], "geographic");
        assert_eq!(sidecar["crs"], "EPSG:4326");
        assert_eq!(sidecar["variable"], "reflectivity");
        assert_eq!(
            sidecar["title"],
            "test / composite / east / 2026-09-24T00:00:00.000000Z / reflectivity [dBZ]"
        );
        assert_eq!(sidecar["legend"], json!(["-1", "-0.25", "0.5", "1.25", "2"]));
        assert_eq!(
            sidecar["provenance"],
            json!({"source": "test", "product": "composite", "station": "east"})
        );
        assert!(sidecar_text.starts_with("{\n  \"crs\""));

        write_png(&field, &output, &options).unwrap();
        assert_eq!(std::fs::read_to_string(&paths[1]).unwrap(), sidecar_text);
    }

    #[test]
    fn writer_rejects_unknown_palette_mismatched_variable_and_invalid_ranges() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("radar.png");
        let field = field(vec![0.0, 1.0], vec![0, 0], [1, 2], vec![], vec![]);

        let error = write_png(&field, &output, &json!({"palette": "rainbow"})).unwrap_err();
        assert!(
            matches!(error, CoreError::Storage(message) if message.contains("unknown palette"))
        );
        let error = write_png(&field, &output, &json!({"variable": "rain_rate"})).unwrap_err();
        assert!(
            matches!(error, CoreError::Storage(message) if message.contains("unknown variable"))
        );
        let error = write_png(&field, &output, &json!({"vmin": 2.0, "vmax": 2.0})).unwrap_err();
        assert!(
            matches!(error, CoreError::Storage(message) if message.contains("vmax must be greater"))
        );
    }
}
