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

pub(crate) fn web_mercator_affine_from_bounds(
    bounds_wsen: [f64; 4],
    width: u32,
    height: u32,
) -> CoreResult<[f64; 6]> {
    let [west, south, east, north] = bounds_wsen;
    if width == 0
        || height == 0
        || ![west, south, east, north].iter().all(|value| value.is_finite())
        || west < -180.0
        || east > 180.0
        || west >= east
        || south >= north
        || south < -WEB_MERCATOR_MAX_LATITUDE_DEGREES
        || north > WEB_MERCATOR_MAX_LATITUDE_DEGREES
    {
        return Err(CoreError::Storage("Web Mercator image bounds are invalid".into()));
    }
    let west_x = WEB_MERCATOR_RADIUS_METERS * west.to_radians();
    let east_x = WEB_MERCATOR_RADIUS_METERS * east.to_radians();
    let south_y = WEB_MERCATOR_RADIUS_METERS
        * (std::f64::consts::FRAC_PI_4 + south.to_radians() / 2.0).tan().ln();
    let north_y = WEB_MERCATOR_RADIUS_METERS
        * (std::f64::consts::FRAC_PI_4 + north.to_radians() / 2.0).tan().ln();
    let affine = [
        west_x,
        (east_x - west_x) / f64::from(width),
        0.0,
        north_y,
        0.0,
        (south_y - north_y) / f64::from(height),
    ];
    if affine.iter().all(|value| value.is_finite()) && affine[1] > 0.0 && affine[5] < 0.0 {
        Ok(affine)
    } else {
        Err(CoreError::Storage("Web Mercator image affine is invalid".into()))
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GridSample {
    pub value: f32,
    pub quality: u16,
    pub in_source_domain: bool,
}

#[derive(Clone, Copy)]
enum OutsideQuality {
    PreserveBoundary,
    MissingOutside,
}

/// Preprocessed axes and sampling semantics for one regular raster.
pub(crate) struct GridSampler<'a> {
    source: &'a RadarField,
    width: usize,
    height: usize,
    x_axis: Vec<f64>,
    y_axis: Vec<f64>,
    x_descending: bool,
    y_descending: bool,
    x_domain: (f64, f64),
    y_domain: (f64, f64),
    dbz: bool,
}

impl<'a> GridSampler<'a> {
    pub(crate) fn pixel_centers(source: &'a RadarField) -> CoreResult<Self> {
        source
            .validate()
            .map_err(|_| CoreError::Storage("source field shape is invalid".into()))?;
        let [height, width] = source.shape.as_slice() else {
            return Err(CoreError::Storage("sampling requires a two-dimensional field".into()));
        };
        validate_axis(&source.grid.x, *width, "source x")?;
        validate_axis(&source.grid.y, *height, "source y")?;
        Self::from_axes(source, source.grid.x.clone(), source.grid.y.clone(), None, None)
    }

    pub(crate) fn pixel_centers_with_x_axis(
        source: &'a RadarField,
        x_axis: Vec<f64>,
    ) -> CoreResult<Self> {
        source
            .validate()
            .map_err(|_| CoreError::Storage("source field shape is invalid".into()))?;
        let [height, width] = source.shape.as_slice() else {
            return Err(CoreError::Storage("sampling requires a two-dimensional field".into()));
        };
        validate_axis(&x_axis, *width, "source x")?;
        validate_axis(&source.grid.y, *height, "source y")?;
        Self::from_axes(source, x_axis, source.grid.y.clone(), None, None)
    }

    pub(crate) fn pixel_edges(
        source: &'a RadarField,
        x_edges: &[f64],
        y_edges: &[f64],
    ) -> CoreResult<Self> {
        source
            .validate()
            .map_err(|_| CoreError::Storage("source field shape is invalid".into()))?;
        let [height, width] = source.shape.as_slice() else {
            return Err(CoreError::Storage("sampling requires a two-dimensional field".into()));
        };
        let x_centers = centers_from_edges(x_edges, *width, "source x")?;
        let y_centers = centers_from_edges(y_edges, *height, "source y")?;
        validate_axis(&x_centers, *width, "source x")?;
        validate_axis(&y_centers, *height, "source y")?;
        for (provided, derived, axis_name) in
            [(&source.grid.x, &x_centers, "source x"), (&source.grid.y, &y_centers, "source y")]
        {
            if !provided.is_empty()
                && (provided.len() != derived.len()
                    || provided.iter().zip(derived).any(|(left, right)| {
                        (left - right).abs() > 1e-9 * left.abs().max(right.abs()).max(1.0)
                    }))
            {
                return Err(CoreError::Storage(format!(
                    "{axis_name} centers conflict with the declared pixel edges"
                )));
            }
        }
        let x_domain = ordered_bounds(x_edges);
        let y_domain = ordered_bounds(y_edges);
        Self::from_axes(source, x_centers, y_centers, Some(x_domain), Some(y_domain))
    }

    fn from_axes(
        source: &'a RadarField,
        x: Vec<f64>,
        y: Vec<f64>,
        x_edges: Option<(f64, f64)>,
        y_edges: Option<(f64, f64)>,
    ) -> CoreResult<Self> {
        let [height, width] = source.shape.as_slice() else {
            return Err(CoreError::Storage("sampling requires a two-dimensional field".into()));
        };
        let x_descending = x.len() > 1 && x.first() > x.last();
        let y_descending = y.len() > 1 && y.first() > y.last();
        let x_domain = x_edges.unwrap_or_else(|| ordered_bounds(&x));
        let y_domain = y_edges.unwrap_or_else(|| ordered_bounds(&y));
        Ok(Self {
            source,
            width: *width,
            height: *height,
            x_axis: ascending_axis(&x, x_descending),
            y_axis: ascending_axis(&y, y_descending),
            x_descending,
            y_descending,
            x_domain,
            y_domain,
            dbz: source.units.as_deref().is_some_and(|units| units.eq_ignore_ascii_case("dbz")),
        })
    }

    pub(crate) fn sample_local(&self, x: f64, y: f64, method: Resampling) -> GridSample {
        self.sample(x, y, method, OutsideQuality::MissingOutside)
    }

    pub(crate) fn x_axis(&self) -> &[f64] {
        &self.x_axis
    }

    fn sample(&self, x: f64, y: f64, method: Resampling, outside: OutsideQuality) -> GridSample {
        let in_source_domain = x.is_finite()
            && y.is_finite()
            && x >= self.x_domain.0
            && x <= self.x_domain.1
            && y >= self.y_domain.0
            && y <= self.y_domain.1;
        match method {
            Resampling::Nearest => {
                let column = nearest_index(&self.x_axis, x);
                let row = nearest_index(&self.y_axis, y);
                let index = source_index(
                    row,
                    column,
                    self.width,
                    self.height,
                    self.x_descending,
                    self.y_descending,
                );
                if in_source_domain {
                    GridSample {
                        value: self.source.values[index],
                        quality: self.source.quality[index],
                        in_source_domain,
                    }
                } else {
                    let quality = match outside {
                        OutsideQuality::PreserveBoundary => {
                            self.source.quality[index] | QUALITY_OUTSIDE
                        }
                        OutsideQuality::MissingOutside => QUALITY_MISSING | QUALITY_OUTSIDE,
                    };
                    GridSample { value: f32::NAN, quality, in_source_domain }
                }
            }
            Resampling::Bilinear => bilinear_sample(
                self.source,
                &self.x_axis,
                &self.y_axis,
                self.width,
                self.height,
                self.x_descending,
                self.y_descending,
                x,
                y,
                self.dbz,
                in_source_domain,
                outside,
            ),
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

pub(crate) fn same_crs(left: Option<&str>, right: Option<&str>) -> bool {
    left == right || same_epsg(left, right)
}

pub(crate) fn epsg_code(crs: Option<&str>) -> Option<u32> {
    let crs = crs?.trim();
    let (authority, code) = crs.split_once(':')?;
    if !authority.eq_ignore_ascii_case("EPSG") {
        return None;
    }
    code.parse().ok()
}

/// Map WGS84 coordinates to a supported source grid, choosing the longitude
/// branch nearest the source x-axis while retaining each CRS's valid domain.
pub(crate) fn wgs84_to_source_grid(
    source_crs: Option<&str>,
    longitude: f64,
    latitude: f64,
    source_x_axis: &[f64],
) -> Option<(f64, f64)> {
    if source_x_axis.is_empty()
        || source_x_axis.iter().any(|value| !value.is_finite())
        || !longitude.is_finite()
        || !latitude.is_finite()
    {
        return None;
    }
    let transform = crs_transform(source_crs, Some("EPSG:4326"))?;
    let source_midpoint = f64::midpoint(source_x_axis[0], *source_x_axis.last()?);
    let reference_longitude = match transform {
        CrsTransform::Identity => source_midpoint,
        CrsTransform::GeographicToWebMercator | CrsTransform::WebMercatorToGeographic => {
            (source_midpoint / WEB_MERCATOR_RADIUS_METERS).to_degrees()
        }
    };
    let branch = nearest_longitude_branch(longitude, reference_longitude);
    let x = transform.target_x_to_source(branch);
    let y = transform.target_y_to_source(latitude);
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

fn nearest_longitude_branch(longitude: f64, reference_longitude: f64) -> f64 {
    let center_turn = ((reference_longitude - longitude) / 360.0).round();
    let candidates =
        [center_turn - 1.0, center_turn, center_turn + 1.0].map(|turn| longitude + 360.0 * turn);
    candidates
        .into_iter()
        .min_by(|left, right| {
            (left - reference_longitude).abs().total_cmp(&(right - reference_longitude).abs())
        })
        .unwrap_or(longitude)
}

/// Unwrap a geographic source x axis onto one monotonic longitude branch.
pub(crate) fn continuous_longitude_axis(axis: &[f64]) -> Option<Vec<f64>> {
    if axis.is_empty() || axis.iter().any(|value| !value.is_finite()) {
        return None;
    }
    if axis.len() == 1 {
        return Some(axis.to_vec());
    }

    let mut result = Vec::with_capacity(axis.len());
    result.push(axis[0]);
    let mut direction = 0.0_f64;
    for pair in axis.windows(2) {
        let raw_delta = pair[1] - pair[0];
        let delta = raw_delta - (raw_delta / 360.0).round() * 360.0;
        if !delta.is_finite() || delta == 0.0 {
            return None;
        }
        if direction == 0.0 {
            direction = delta.signum();
        } else if delta.signum() != direction {
            return None;
        }
        result.push(result.last().copied()? + delta);
    }
    ((result.last().copied()? - result[0]).abs() <= 360.0).then_some(result)
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
    let sampler = GridSampler::pixel_centers(source)?;
    let [target_height, target_width] = target.shape.as_slice() else {
        return Err(CoreError::Storage("target grid must be two-dimensional".into()));
    };
    validate_axis(&target.x, *target_width, "target x")?;
    validate_axis(&target.y, *target_height, "target y")?;
    let transform =
        crs_transform(source.grid.crs.as_deref(), target.crs.as_deref()).ok_or_else(|| {
            CoreError::Storage("regridding between different CRS values is unsupported".into())
        })?;
    validate_sampling_method(source, method)?;

    let output_cells = u64::try_from(*target_height)
        .ok()
        .and_then(|height| {
            u64::try_from(*target_width).ok().and_then(|width| height.checked_mul(width))
        })
        .ok_or_else(|| CoreError::ResourceLimit("target grid dimensions overflow".into()))?;
    limits.validate_pixels(output_cells)?;
    let output_len = usize::try_from(output_cells)
        .map_err(|_| CoreError::ResourceLimit("target grid dimensions overflow".into()))?;
    let mut values = Vec::with_capacity(output_len);
    let mut quality = Vec::with_capacity(output_len);

    if transform == CrsTransform::Identity {
        for &target_y in &target.y {
            for &target_x in &target.x {
                let sample =
                    sampler.sample(target_x, target_y, method, OutsideQuality::PreserveBoundary);
                values.push(sample.value);
                quality.push(sample.quality);
            }
        }
    } else {
        for &target_y in &target.y {
            let target_y = transform.target_y_to_source(target_y);
            for &target_x in &target.x {
                let sample = sampler.sample(
                    transform.target_x_to_source(target_x),
                    target_y,
                    method,
                    OutsideQuality::PreserveBoundary,
                );
                values.push(sample.value);
                quality.push(sample.quality);
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

pub(crate) fn validate_sampling_method(source: &RadarField, method: Resampling) -> CoreResult<()> {
    if method == Resampling::Bilinear
        && matches!(source.name.to_ascii_lowercase().as_str(), "quality" | "category" | "class")
    {
        return Err(CoreError::Storage(
            "categorical fields cannot use bilinear interpolation".into(),
        ));
    }
    Ok(())
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

fn centers_from_edges(edges: &[f64], count: usize, name: &str) -> CoreResult<Vec<f64>> {
    let expected_edges = count
        .checked_add(1)
        .ok_or_else(|| CoreError::ResourceLimit(format!("{name} edge count overflows")))?;
    validate_axis(edges, expected_edges, name)?;
    Ok(edges.windows(2).map(|pair| f64::midpoint(pair[0], pair[1])).collect())
}

fn ordered_bounds(axis: &[f64]) -> (f64, f64) {
    if axis.first() <= axis.last() {
        (axis[0], axis[axis.len() - 1])
    } else {
        (axis[axis.len() - 1], axis[0])
    }
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
    in_source_domain: bool,
    outside: OutsideQuality,
) -> GridSample {
    let Some((x0, x1, wx)) = bracket(x_axis, target_x) else {
        return GridSample {
            value: f32::NAN,
            quality: if in_source_domain { QUALITY_MISSING } else { outside_flags(outside) },
            in_source_domain,
        };
    };
    let Some((y0, y1, wy)) = bracket(y_axis, target_y) else {
        return GridSample {
            value: f32::NAN,
            quality: if in_source_domain { QUALITY_MISSING } else { outside_flags(outside) },
            in_source_domain,
        };
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
        return GridSample {
            value: f32::NAN,
            quality: combined_quality | if invalid_value { QUALITY_MISSING } else { 0 },
            in_source_domain,
        };
    }
    if dbz {
        total = 10.0 * total.log10();
    }
    GridSample {
        value: total as f32,
        quality: combined_quality | if active_count > 1 { QUALITY_INTERPOLATED } else { 0 },
        in_source_domain,
    }
}

fn outside_flags(outside: OutsideQuality) -> u16 {
    match outside {
        OutsideQuality::PreserveBoundary => QUALITY_OUTSIDE,
        OutsideQuality::MissingOutside => QUALITY_MISSING | QUALITY_OUTSIDE,
    }
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
    fn local_view_outside_samples_do_not_inherit_boundary_quality() {
        let recovered = 1 << 8;
        let source =
            field(vec![0.0, 1.0], vec![0.0, 1.0], vec![10.0, 20.0, 30.0, 40.0], vec![recovered; 4]);
        let sampler = GridSampler::pixel_centers(&source).unwrap();
        let local = sampler.sample_local(-0.1, 0.5, Resampling::Nearest);
        assert!(local.value.is_nan());
        assert_eq!(local.quality, QUALITY_MISSING | QUALITY_OUTSIDE);
        assert!(!local.in_source_domain);

        let legacy =
            regrid_regular(&source, target(vec![-0.1], vec![0.5]), Resampling::Nearest, &limits())
                .unwrap();
        assert_eq!(legacy.quality, [recovered | QUALITY_OUTSIDE]);
    }

    #[test]
    fn pixel_edge_registration_uses_edges_for_nearest_domain_checks() {
        let source = field(vec![0.5, 1.5], vec![1.5, 0.5], vec![1.0, 2.0, 3.0, 4.0], vec![0; 4]);
        let sampler =
            GridSampler::pixel_edges(&source, &[0.0, 1.0, 2.0], &[2.0, 1.0, 0.0]).unwrap();
        let edge_pixel = sampler.sample_local(0.1, 1.9, Resampling::Nearest);
        assert_eq!(edge_pixel.value, 1.0);
        assert_eq!(edge_pixel.quality, 0);
        assert!(edge_pixel.in_source_domain);
        assert!(GridSampler::pixel_edges(&source, &[0.0, 2.0], &[2.0, 1.0, 0.0]).is_err());
    }

    #[test]
    fn edge_registered_bilinear_keeps_domain_when_neighbors_are_missing() {
        let source = field(vec![0.5, 1.5], vec![1.5, 0.5], vec![1.0, 2.0, 3.0, 4.0], vec![0; 4]);
        let sampler =
            GridSampler::pixel_edges(&source, &[0.0, 1.0, 2.0], &[2.0, 1.0, 0.0]).unwrap();
        for (x, y) in [(0.1, 1.0), (1.0, 1.9), (0.0, 2.0)] {
            let ring = sampler.sample_local(x, y, Resampling::Bilinear);
            assert!(ring.value.is_nan());
            assert_eq!(ring.quality, QUALITY_MISSING);
            assert!(ring.in_source_domain);
        }
        let outside = sampler.sample_local(-0.1, 1.0, Resampling::Bilinear);
        assert!(outside.value.is_nan());
        assert_eq!(outside.quality, QUALITY_MISSING | QUALITY_OUTSIDE);
        assert!(!outside.in_source_domain);
        let inner = sampler.sample_local(1.0, 1.0, Resampling::Bilinear);
        assert!(inner.in_source_domain);
        assert!(inner.value.is_finite());
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
    fn local_view_unwraps_date_line_axes_and_maps_web_mercator_domain() {
        let ascending = continuous_longitude_axis(&[179.0, 179.5, -180.0, -179.5]).unwrap();
        assert_eq!(ascending, [179.0, 179.5, 180.0, 180.5]);
        let wrapped = wgs84_to_source_grid(Some("EPSG:4326"), -179.5, 4.0, &ascending).unwrap();
        assert!((wrapped.0 - 180.5).abs() < 1e-12);
        assert!((wrapped.1 - 4.0).abs() < 1e-12);

        let descending = continuous_longitude_axis(&[-179.5, -180.0, 179.5, 179.0]).unwrap();
        assert_eq!(descending, [-179.5, -180.0, -180.5, -181.0]);
        let wrapped = wgs84_to_source_grid(Some("EPSG:4326"), 179.5, 4.0, &descending).unwrap();
        assert!((wrapped.0 + 180.5).abs() < 1e-12);

        let radius = WEB_MERCATOR_RADIUS_METERS;
        let x_axis = [radius * 99.0_f64.to_radians(), radius * 101.0_f64.to_radians()];
        let projected = wgs84_to_source_grid(Some("EPSG:3857"), 100.5, 40.0, &x_axis).unwrap();
        let expected_x = radius * 100.5_f64.to_radians();
        let expected_y = radius * (std::f64::consts::FRAC_PI_4 + 20.0_f64.to_radians()).tan().ln();
        assert!((projected.0 - expected_x).abs() < 1e-8);
        assert!((projected.1 - expected_y).abs() < 1e-8);
        assert!(wgs84_to_source_grid(Some("EPSG:3857"), 100.0, 86.0, &x_axis).is_none());
        assert!(continuous_longitude_axis(&[0.0, 1.0, 0.0]).is_none());
    }

    #[test]
    fn local_view_builds_edge_registered_web_mercator_image_affine() {
        let affine =
            web_mercator_affine_from_bounds([-19.87034, 30.01467, -12.89697, 35.96053], 1500, 1526)
                .unwrap();
        let expected = [
            -2_211_956.1306892154,
            517.5146650087266,
            0.0,
            4_295_191.719145755,
            0.0,
            -517.5334498787525,
        ];
        for (actual, expected) in affine.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-8);
        }
        assert!(web_mercator_affine_from_bounds([0.0, 86.0, 1.0, 87.0], 10, 10).is_err());
        assert!(web_mercator_affine_from_bounds([1.0, 0.0, -1.0, 1.0], 10, 10).is_err());
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
