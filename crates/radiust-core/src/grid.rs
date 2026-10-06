//! Regridding for regular one-dimensional geographic and Cartesian axes.
//!
//! Same-CRS grids are supported directly; cross-CRS sampling currently has a
//! verified EPSG:4326/EPSG:3857 Web Mercator transform only.

use crate::errors::{CoreError, CoreResult};
use crate::limits::Limits;
use crate::model::{Grid, RadarField};

const QUALITY_MISSING: u16 = 1;
const QUALITY_OUTSIDE: u16 = 2;
const QUALITY_UNKNOWN: u16 = 4;
const QUALITY_INTERPOLATED: u16 = 16;
const QUALITY_BELOW_DETECTION: u16 = 32;
const QUALITY_SOURCE_ANNOTATION: u16 = 64;
const INVALID_BILINEAR_FLAGS: u16 = QUALITY_MISSING
    | QUALITY_OUTSIDE
    | QUALITY_UNKNOWN
    | QUALITY_BELOW_DETECTION
    | QUALITY_SOURCE_ANNOTATION;
// EPSG:3857 spherical Web Mercator parameters; formulas follow PROJ's
// documented Web Mercator operation.
const WEB_MERCATOR_RADIUS_METERS: f64 = 6_378_137.0;
const WEB_MERCATOR_MAX_LATITUDE_DEGREES: f64 = 85.051_128_779_806_6;
const WEB_MERCATOR_MAX_COORDINATE_METERS: f64 = std::f64::consts::PI * WEB_MERCATOR_RADIUS_METERS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resampling {
    Nearest,
    Bilinear,
}

impl Resampling {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "nearest" => Some(Self::Nearest),
            "bilinear" => Some(Self::Bilinear),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Bilinear => "bilinear",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CrsTransform {
    Identity,
    GeographicToWebMercator,
    WebMercatorToGeographic,
}

impl CrsTransform {
    /// Convert a target-grid x coordinate into the source grid's CRS.
    fn target_x_to_source(self, value: f64) -> f64 {
        if !value.is_finite() {
            return f64::NAN;
        }
        match self {
            Self::Identity => value,
            Self::GeographicToWebMercator => {
                if value.abs() > 180.0 {
                    f64::NAN
                } else {
                    WEB_MERCATOR_RADIUS_METERS * value.to_radians()
                }
            }
            Self::WebMercatorToGeographic => {
                if value.abs() > WEB_MERCATOR_MAX_COORDINATE_METERS {
                    f64::NAN
                } else {
                    (value / WEB_MERCATOR_RADIUS_METERS).to_degrees()
                }
            }
        }
    }

    /// Convert a target-grid y coordinate into the source grid's CRS.
    fn target_y_to_source(self, value: f64) -> f64 {
        if !value.is_finite() {
            return f64::NAN;
        }
        match self {
            Self::Identity => value,
            Self::GeographicToWebMercator => {
                if value.abs() > WEB_MERCATOR_MAX_LATITUDE_DEGREES {
                    return f64::NAN;
                }
                let latitude = value.to_radians();
                WEB_MERCATOR_RADIUS_METERS
                    * (std::f64::consts::FRAC_PI_4 + latitude / 2.0).tan().ln()
            }
            Self::WebMercatorToGeographic => {
                if value.abs() > WEB_MERCATOR_MAX_COORDINATE_METERS {
                    return f64::NAN;
                }
                (2.0 * (value / WEB_MERCATOR_RADIUS_METERS).exp().atan()
                    - std::f64::consts::FRAC_PI_2)
                    .to_degrees()
            }
        }
    }
}

/// Return whether `regrid_regular` can map coordinates from `target_crs` into
/// `source_crs` without an unverified datum or projection operation.
pub fn supports_regrid_crs(source_crs: Option<&str>, target_crs: Option<&str>) -> bool {
    crs_transform(source_crs, target_crs).is_some()
}

/// Validate that trusted pixel coordinates map every row and column to a
/// unique, ordered center in a declared CRS.
pub fn has_complete_pixel_mapping(
    width: usize,
    height: usize,
    crs: Option<&str>,
    x: &[f64],
    y: &[f64],
) -> bool {
    let supported_crs = crs.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_uppercase().as_str(),
            "EPSG:4326" | "EPSG:3821" | "EPSG:3857"
        )
    });
    supported_crs
        && width >= 2
        && height >= 2
        && x.len() == width
        && y.len() == height
        && valid_center_axis(x)
        && valid_center_axis(y)
}

fn valid_center_axis(values: &[f64]) -> bool {
    if values.len() < 2 || values.iter().any(|value| !value.is_finite()) {
        return false;
    }
    let direction = (values[1] - values[0]).signum();
    direction != 0.0 && values.windows(2).all(|pair| (pair[1] - pair[0]).signum() == direction)
}

fn crs_transform(source_crs: Option<&str>, target_crs: Option<&str>) -> Option<CrsTransform> {
    if source_crs == target_crs || same_epsg(source_crs, target_crs) {
        return Some(CrsTransform::Identity);
    }
    match (epsg_code(source_crs)?, epsg_code(target_crs)?) {
        // Target coordinates are transformed into the source grid CRS.
        (4326, 3857) => Some(CrsTransform::WebMercatorToGeographic),
        (3857, 4326) => Some(CrsTransform::GeographicToWebMercator),
        _ => None,
    }
}

fn same_epsg(left: Option<&str>, right: Option<&str>) -> bool {
    matches!((epsg_code(left), epsg_code(right)), (Some(left), Some(right)) if left == right)
}

fn epsg_code(crs: Option<&str>) -> Option<u32> {
    let crs = crs?.trim();
    let (authority, code) = crs.split_once(':')?;
    if !authority.eq_ignore_ascii_case("EPSG") {
        return None;
    }
    code.parse().ok()
}

/// Regrid a field between regular axes.
///
/// Axes may be increasing or decreasing. Nearest-neighbour ties select the
/// lower coordinate, matching the Python implementation's first-index rule.
/// Cross-CRS sampling is limited to verified EPSG:4326/EPSG:3857 Web Mercator
/// transformations; other differing CRS pairs are rejected.
pub fn regrid_regular(
    source: &RadarField,
    target: Grid,
    method: Resampling,
    limits: &Limits,
) -> CoreResult<RadarField> {
    source.validate().map_err(|_| CoreError::Storage("source field shape is invalid".into()))?;
    let [source_height, source_width] = source.shape.as_slice() else {
        return Err(CoreError::Storage("regridding requires a two-dimensional field".into()));
    };
    let [target_height, target_width] = target.shape.as_slice() else {
        return Err(CoreError::Storage("target grid must be two-dimensional".into()));
    };
    validate_axis(&source.grid.x, *source_width, "source x")?;
    validate_axis(&source.grid.y, *source_height, "source y")?;
    validate_axis(&target.x, *target_width, "target x")?;
    validate_axis(&target.y, *target_height, "target y")?;
    let transform =
        crs_transform(source.grid.crs.as_deref(), target.crs.as_deref()).ok_or_else(|| {
            CoreError::Storage("regridding between different CRS values is unsupported".into())
        })?;
    if method == Resampling::Bilinear
        && matches!(source.name.to_ascii_lowercase().as_str(), "quality" | "category" | "class")
    {
        return Err(CoreError::Storage(
            "categorical fields cannot use bilinear interpolation".into(),
        ));
    }

    let output_cells = u64::try_from(*target_height)
        .ok()
        .and_then(|height| {
            u64::try_from(*target_width).ok().and_then(|width| height.checked_mul(width))
        })
        .ok_or_else(|| CoreError::ResourceLimit("target grid dimensions overflow".into()))?;
    limits.validate_pixels(output_cells)?;
    let output_len = usize::try_from(output_cells)
        .map_err(|_| CoreError::ResourceLimit("target grid dimensions overflow".into()))?;

    let source_x_descending =
        source.grid.x.len() > 1 && source.grid.x.first() > source.grid.x.last();
    let source_y_descending =
        source.grid.y.len() > 1 && source.grid.y.first() > source.grid.y.last();
    let x_axis = ascending_axis(&source.grid.x, source_x_descending);
    let y_axis = ascending_axis(&source.grid.y, source_y_descending);
    let dbz = source.units.as_deref().is_some_and(|units| units.eq_ignore_ascii_case("dbz"));
    let mut values = Vec::with_capacity(output_len);
    let mut quality = Vec::with_capacity(output_len);

    let sample_at = |target_x: f64,
                     target_y: f64,
                     values: &mut Vec<f32>,
                     quality: &mut Vec<u16>| {
        match method {
            Resampling::Nearest => {
                let inside = target_x.is_finite()
                    && target_y.is_finite()
                    && in_axis_bounds(&x_axis, target_x)
                    && in_axis_bounds(&y_axis, target_y);
                let column = nearest_index(&x_axis, target_x);
                let row = nearest_index(&y_axis, target_y);
                let source_index = source_index(
                    row,
                    column,
                    *source_width,
                    *source_height,
                    source_x_descending,
                    source_y_descending,
                );
                values.push(if inside { source.values[source_index] } else { f32::NAN });
                quality.push(if inside {
                    source.quality[source_index]
                } else {
                    source.quality[source_index] | QUALITY_OUTSIDE
                });
            }
            Resampling::Bilinear => bilinear_sample(
                source,
                &x_axis,
                &y_axis,
                *source_width,
                *source_height,
                source_x_descending,
                source_y_descending,
                target_x,
                target_y,
                dbz,
                values,
                quality,
            ),
        }
    };

    if transform == CrsTransform::Identity {
        for &target_y in &target.y {
            for &target_x in &target.x {
                sample_at(target_x, target_y, &mut values, &mut quality);
            }
        }
    } else {
        for &target_y in &target.y {
            let target_y = transform.target_y_to_source(target_y);
            for &target_x in &target.x {
                sample_at(
                    transform.target_x_to_source(target_x),
                    target_y,
                    &mut values,
                    &mut quality,
                );
            }
        }
    }

    let mut provenance = source.provenance.clone();
    provenance.push(format!("operation=regrid,resampling={}", method.as_str()));
    if transform != CrsTransform::Identity {
        provenance.push(format!(
            "operation=crs_transform,method=web_mercator,source_crs={},target_crs={}",
            source.grid.crs.as_deref().unwrap_or("unknown"),
            target.crs.as_deref().unwrap_or("unknown")
        ));
    }
    let output = RadarField {
        name: source.name.clone(),
        values,
        shape: target.shape.clone(),
        quality,
        units: source.units.clone(),
        valid_time: source.valid_time.clone(),
        grid: target,
        provenance,
    };
    output.validate().map_err(|_| CoreError::Storage("regridded field shape is invalid".into()))?;
    Ok(output)
}

fn validate_axis(axis: &[f64], expected: usize, name: &str) -> CoreResult<()> {
    if axis.len() != expected || axis.is_empty() || axis.iter().any(|value| !value.is_finite()) {
        return Err(CoreError::Storage(format!("{name} coordinates do not match the grid shape")));
    }
    let mut direction = 0_i8;
    for pair in axis.windows(2) {
        let next = if pair[1] > pair[0] {
            1
        } else if pair[1] < pair[0] {
            -1
        } else {
            return Err(CoreError::Storage(format!("{name} coordinates are not monotonic")));
        };
        if direction != 0 && direction != next {
            return Err(CoreError::Storage(format!("{name} coordinates are not monotonic")));
        }
        direction = next;
    }
    Ok(())
}

fn ascending_axis(axis: &[f64], descending: bool) -> Vec<f64> {
    if descending { axis.iter().rev().copied().collect() } else { axis.to_vec() }
}

fn in_axis_bounds(axis: &[f64], value: f64) -> bool {
    value.is_finite() && value >= axis[0] && value <= axis[axis.len() - 1]
}

fn nearest_index(axis: &[f64], value: f64) -> usize {
    if axis.len() == 1 || !value.is_finite() || value <= axis[0] {
        return 0;
    }
    let last = axis.len() - 1;
    if value >= axis[last] {
        return last;
    }
    let upper = axis.partition_point(|coordinate| *coordinate < value);
    let lower = upper - 1;
    if (axis[upper] - value) < (value - axis[lower]) { upper } else { lower }
}

fn bracket(axis: &[f64], value: f64) -> Option<(usize, usize, f64)> {
    if !in_axis_bounds(axis, value) {
        return None;
    }
    if axis.len() == 1 || value == axis[0] {
        return Some((0, 0, 0.0));
    }
    let last = axis.len() - 1;
    if value == axis[last] {
        return Some((last, last, 0.0));
    }
    let upper = axis.partition_point(|coordinate| *coordinate <= value);
    let lower = upper - 1;
    Some((lower, upper, (value - axis[lower]) / (axis[upper] - axis[lower])))
}

#[allow(clippy::too_many_arguments)]
fn bilinear_sample(
    source: &RadarField,
    x_axis: &[f64],
    y_axis: &[f64],
    source_width: usize,
    source_height: usize,
    source_x_descending: bool,
    source_y_descending: bool,
    target_x: f64,
    target_y: f64,
    dbz: bool,
    values: &mut Vec<f32>,
    quality: &mut Vec<u16>,
) {
    let Some((x0, x1, wx)) = bracket(x_axis, target_x) else {
        values.push(f32::NAN);
        quality.push(QUALITY_OUTSIDE);
        return;
    };
    let Some((y0, y1, wy)) = bracket(y_axis, target_y) else {
        values.push(f32::NAN);
        quality.push(QUALITY_OUTSIDE);
        return;
    };
    let samples = [
        (y0, x0, (1.0 - wy) * (1.0 - wx)),
        (y0, x1, (1.0 - wy) * wx),
        (y1, x0, wy * (1.0 - wx)),
        (y1, x1, wy * wx),
    ];
    let mut total = 0.0_f64;
    let mut combined_quality = 0_u16;
    let mut active_count = 0_u8;
    let mut invalid_value = false;
    let mut invalid_quality = false;

    for (row, column, weight) in samples {
        if weight <= 0.0 {
            continue;
        }
        active_count += 1;
        let index = source_index(
            row,
            column,
            source_width,
            source_height,
            source_x_descending,
            source_y_descending,
        );
        let flag = source.quality[index];
        combined_quality |= flag;
        invalid_quality |= flag & INVALID_BILINEAR_FLAGS != 0;
        let sample = f64::from(source.values[index]);
        let contribution = if dbz { 10.0_f64.powf(sample / 10.0) } else { sample };
        if !contribution.is_finite() {
            invalid_value = true;
        }
        total += contribution * weight;
    }

    if invalid_value || invalid_quality {
        values.push(f32::NAN);
        quality.push(combined_quality | if invalid_value { QUALITY_MISSING } else { 0 });
        return;
    }
    if dbz {
        total = 10.0 * total.log10();
    }
    values.push(total as f32);
    quality.push(combined_quality | if active_count > 1 { QUALITY_INTERPOLATED } else { 0 });
}

fn source_index(
    row: usize,
    column: usize,
    width: usize,
    height: usize,
    x_descending: bool,
    y_descending: bool,
) -> usize {
    let source_row = if y_descending { height - 1 - row } else { row };
    let source_column = if x_descending { width - 1 - column } else { column };
    source_row * width + source_column
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits { max_pixels: 100, ..Limits::default() }
    }

    fn field(x: Vec<f64>, y: Vec<f64>, values: Vec<f32>, quality: Vec<u16>) -> RadarField {
        let shape = vec![y.len(), x.len()];
        RadarField {
            name: "reflectivity".into(),
            values,
            shape: shape.clone(),
            quality,
            units: Some("dBZ".into()),
            valid_time: "2026-09-25T00:00:00Z".into(),
            grid: Grid { shape, crs: Some("EPSG:4326".into()), x, y, affine: None },
            provenance: vec!["source=fixture".into()],
        }
    }

    fn target(x: Vec<f64>, y: Vec<f64>) -> Grid {
        let shape = vec![y.len(), x.len()];
        Grid { shape, crs: Some("EPSG:4326".into()), x, y, affine: None }
    }

    #[test]
    fn nearest_preserves_descending_source_order_tie_rule_and_outside_quality() {
        let ascending =
            field(vec![0.0, 1.0], vec![0.0, 1.0], vec![1.0, 2.0, 3.0, 4.0], vec![0, 1, 4, 0]);
        let descending =
            field(vec![1.0, 0.0], vec![1.0, 0.0], vec![4.0, 3.0, 2.0, 1.0], vec![0, 4, 1, 0]);
        let grid = target(vec![-1.0, 0.5, 1.0], vec![0.25, 0.75]);

        let expected =
            regrid_regular(&ascending, grid.clone(), Resampling::Nearest, &limits()).unwrap();
        let actual = regrid_regular(&descending, grid, Resampling::Nearest, &limits()).unwrap();

        assert!(expected.values[0].is_nan() && expected.values[3].is_nan());
        assert_eq!(&expected.values[1..3], &[1.0, 2.0]);
        assert_eq!(&expected.values[4..6], &[3.0, 4.0]);
        assert_eq!(actual.values[1], 1.0);
        assert_eq!(actual.values[2], 2.0);
        assert_eq!(actual.values[4], 3.0);
        assert_eq!(expected.quality, actual.quality);
        assert_eq!(actual.quality, [2, 0, 1, 6, 4, 0]);
        assert!(actual.values[0].is_nan() && actual.values[3].is_nan());
    }

    #[test]
    fn bilinear_averages_dbz_in_linear_power_and_marks_interpolation() {
        let source = field(vec![0.0, 1.0], vec![0.0, 1.0], vec![0.0, 10.0, 20.0, 30.0], vec![0; 4]);
        let output =
            regrid_regular(&source, target(vec![0.5], vec![0.5]), Resampling::Bilinear, &limits())
                .unwrap();
        let expected = 10.0 * ((1.0_f64 + 10.0 + 100.0 + 1000.0) / 4.0).log10();
        assert!((f64::from(output.values[0]) - expected).abs() < 1.0e-5);
        assert_eq!(output.quality, [QUALITY_INTERPOLATED]);
    }

    #[test]
    fn bilinear_ignores_zero_weight_invalid_neighbors_but_marks_active_missing() {
        let source =
            field(vec![0.0, 1.0], vec![0.0, 1.0], vec![5.0, f32::NAN, 9.0, 10.0], vec![0, 1, 0, 0]);
        let exact =
            regrid_regular(&source, target(vec![0.0], vec![0.0]), Resampling::Bilinear, &limits())
                .unwrap();
        assert_eq!(exact.values, [5.0]);
        assert_eq!(exact.quality, [0]);

        let between =
            regrid_regular(&source, target(vec![0.5], vec![0.0]), Resampling::Bilinear, &limits())
                .unwrap();
        assert!(between.values[0].is_nan());
        assert_eq!(between.quality, [1]);
    }

    #[test]
    fn bilinear_propagates_source_annotation_as_invalid_quality() {
        let source = field(
            vec![0.0, 1.0],
            vec![0.0, 1.0],
            vec![f32::NAN, 10.0, 20.0, 30.0],
            vec![65, 0, 0, 0],
        );
        let output =
            regrid_regular(&source, target(vec![0.5], vec![0.5]), Resampling::Bilinear, &limits())
                .unwrap();
        assert!(output.values[0].is_nan());
        assert_eq!(output.quality, [65]);
    }

    #[test]
    fn bilinear_outside_and_quality_bits_match_field_contract() {
        let source = field(vec![0.0, 1.0], vec![0.0, 1.0], vec![1.0, 2.0, 3.0, 4.0], vec![0; 4]);
        let output = regrid_regular(
            &source,
            target(vec![-1.0, 0.5], vec![0.5]),
            Resampling::Bilinear,
            &limits(),
        )
        .unwrap();
        assert!(output.values[0].is_nan());
        assert_eq!(output.quality, [QUALITY_OUTSIDE, QUALITY_INTERPOLATED]);
    }

    #[test]
    fn web_mercator_transform_matches_proj_reference_and_roundtrips() {
        // PROJ's official webmerc example maps longitude 2°, latitude 49° to
        // approximately (222638.98 m, 6274861.39 m).
        let forward = crs_transform(Some("EPSG:3857"), Some("EPSG:4326")).unwrap();
        let x = forward.target_x_to_source(2.0);
        let y = forward.target_y_to_source(49.0);
        assert!((x - 222_638.98).abs() < 0.01);
        assert!((y - 6_274_861.39).abs() < 0.01);

        let inverse = crs_transform(Some("EPSG:4326"), Some("EPSG:3857")).unwrap();
        assert!((inverse.target_x_to_source(x) - 2.0).abs() < 1.0e-12);
        assert!((inverse.target_y_to_source(y) - 49.0).abs() < 1.0e-12);
    }

    #[test]
    fn cross_crs_regrid_transforms_target_coordinates_in_both_directions() {
        let geographic =
            field(vec![1.5, 3.0], vec![48.0, 50.0], vec![10.0, 20.0, 30.0, 40.0], vec![0, 1, 4, 8]);
        let project = |longitude: f64, latitude: f64| {
            let x = WEB_MERCATOR_RADIUS_METERS * longitude.to_radians();
            let y = WEB_MERCATOR_RADIUS_METERS
                * (std::f64::consts::FRAC_PI_4 + latitude.to_radians() / 2.0).tan().ln();
            (x, y)
        };
        let (x, y) = project(2.2, 49.2);
        let mut projected_target = target(vec![x], vec![y]);
        projected_target.crs = Some("EPSG:3857".into());

        let from_geographic =
            regrid_regular(&geographic, projected_target, Resampling::Nearest, &limits()).unwrap();
        assert_eq!(from_geographic.values, [30.0]);
        assert_eq!(from_geographic.quality, [4]);
        assert!(from_geographic.provenance.iter().any(|item| item.contains("method=web_mercator")));

        let (x0, y0) = project(1.5, 48.0);
        let (x1, y1) = project(3.0, 50.0);
        let projected_source = RadarField {
            name: "reflectivity".into(),
            values: vec![10.0, 20.0, 30.0, 40.0],
            shape: vec![2, 2],
            quality: vec![0, 1, 4, 8],
            units: Some("dBZ".into()),
            valid_time: "2026-09-25T00:00:00Z".into(),
            grid: Grid {
                shape: vec![2, 2],
                crs: Some("EPSG:3857".into()),
                x: vec![x0, x1],
                y: vec![y0, y1],
                affine: None,
            },
            provenance: vec!["source=fixture".into()],
        };
        let from_projected = regrid_regular(
            &projected_source,
            target(vec![2.2], vec![49.2]),
            Resampling::Nearest,
            &limits(),
        )
        .unwrap();
        assert_eq!(from_projected.values, [30.0]);
        assert_eq!(from_projected.quality, [4]);
    }

    #[test]
    fn web_mercator_projection_domain_is_fail_closed() {
        let geographic_to_web = crs_transform(Some("EPSG:3857"), Some("EPSG:4326")).unwrap();
        assert!(geographic_to_web.target_x_to_source(181.0).is_nan());
        assert!(geographic_to_web.target_y_to_source(90.0).is_nan());
        let web_to_geographic = crs_transform(Some("EPSG:4326"), Some("EPSG:3857")).unwrap();
        assert!(
            web_to_geographic.target_x_to_source(WEB_MERCATOR_MAX_COORDINATE_METERS + 1.0).is_nan()
        );
        assert!(
            web_to_geographic.target_y_to_source(WEB_MERCATOR_MAX_COORDINATE_METERS + 1.0).is_nan()
        );
        assert!(!supports_regrid_crs(Some("EPSG:3821"), Some("EPSG:4326")));
    }

    #[test]
    fn rejects_nonmonotonic_axes_different_crs_and_pixel_budget() {
        let source = field(vec![0.0, 1.0], vec![0.0, 1.0], vec![1.0; 4], vec![0; 4]);
        assert!(
            regrid_regular(
                &source,
                target(vec![0.0, 0.0], vec![0.0]),
                Resampling::Nearest,
                &limits(),
            )
            .is_err()
        );
        let mut different_crs = target(vec![0.0], vec![0.0]);
        different_crs.crs = Some("EPSG:3821".into());
        assert!(regrid_regular(&source, different_crs, Resampling::Nearest, &limits()).is_err());
        let small_limit = Limits { max_pixels: 1, ..limits() };
        assert!(
            regrid_regular(
                &source,
                target(vec![0.0, 1.0], vec![0.0, 1.0]),
                Resampling::Nearest,
                &small_limit,
            )
            .is_err()
        );
    }

    #[test]
    fn bilinear_rejects_categorical_fields() {
        let mut source = field(vec![0.0], vec![0.0], vec![1.0], vec![0]);
        source.name = "category".into();
        assert!(
            regrid_regular(&source, target(vec![0.0], vec![0.0]), Resampling::Bilinear, &limits(),)
                .is_err()
        );
    }
}
