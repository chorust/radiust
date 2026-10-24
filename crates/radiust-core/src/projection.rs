use geographiclib_rs::{DirectGeodesic, Geodesic, InverseGeodesic};
use thiserror::Error;

use crate::errors::{CoreError, CoreResult};
use crate::limits::{RasterBufferLease, RasterMemoryBudget, RasterWorkControl};
use crate::model::{GeographicPoint, Grid, LocalViewGrid};

const MAX_RADIUS_KM: f64 = 500.0;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProjectionError {
    #[error("center or geographic coordinate is invalid")]
    InvalidCoordinate,
    #[error("local-view radius must be finite, positive, and at most 500 km")]
    InvalidRadius,
    #[error("local-view resolution must be finite and positive")]
    InvalidResolution,
    #[error("local-view grid dimensions overflow")]
    GridOverflow,
    #[error("local-view grid has {pixels} pixels, exceeding the configured limit {limit}")]
    PixelLimit { pixels: u64, limit: u64 },
    #[error("local-view grid allocation failed")]
    AllocationFailed,
    #[error("geodesic projection returned a non-finite coordinate")]
    NonFiniteProjection,
    #[error("constructed local grid is invalid")]
    InvalidGrid,
}

/// WGS84 ellipsoidal azimuthal-equidistant coordinates centered on a user point.
#[derive(Clone, Debug)]
pub struct AeqdWgs84 {
    center: GeographicPoint,
    geodesic: Geodesic,
}

impl AeqdWgs84 {
    pub fn new(center: GeographicPoint) -> Result<Self, ProjectionError> {
        validate_coordinate(center)?;
        Ok(Self { center: normalize_point(center), geodesic: Geodesic::wgs84() })
    }

    pub fn center(&self) -> GeographicPoint {
        self.center
    }

    /// Convert a WGS84 position to east/north meters from the center.
    pub fn forward(&self, point: GeographicPoint) -> Result<(f64, f64), ProjectionError> {
        validate_coordinate(point)?;
        let point = normalize_point(point);
        let (distance_m, azimuth_deg, _, _): (f64, f64, f64, f64) = self.geodesic.inverse(
            self.center.latitude,
            self.center.longitude,
            point.latitude,
            point.longitude,
        );
        if !distance_m.is_finite() || !azimuth_deg.is_finite() {
            return Err(ProjectionError::NonFiniteProjection);
        }
        if distance_m == 0.0 {
            return Ok((0.0, 0.0));
        }
        let azimuth_rad = azimuth_deg.to_radians();
        let x = distance_m * azimuth_rad.sin();
        let y = distance_m * azimuth_rad.cos();
        if x.is_finite() && y.is_finite() {
            Ok((x, y))
        } else {
            Err(ProjectionError::NonFiniteProjection)
        }
    }

    /// Convert east/north meters to a WGS84 position using the ellipsoidal direct solution.
    pub fn inverse(&self, x_m: f64, y_m: f64) -> Result<GeographicPoint, ProjectionError> {
        if !x_m.is_finite() || !y_m.is_finite() {
            return Err(ProjectionError::NonFiniteProjection);
        }
        let distance_m = x_m.hypot(y_m);
        if !distance_m.is_finite() {
            return Err(ProjectionError::NonFiniteProjection);
        }
        if distance_m == 0.0 {
            return Ok(self.center);
        }
        let azimuth_deg = x_m.atan2(y_m).to_degrees();
        let (latitude, longitude): (f64, f64) = self.geodesic.direct(
            self.center.latitude,
            self.center.longitude,
            azimuth_deg,
            distance_m,
        );
        let point = GeographicPoint { longitude: normalize_longitude(longitude), latitude };
        validate_coordinate(point)?;
        Ok(point)
    }
}

/// Build a checked, center-aligned circular grid with row zero at the north edge.
pub fn build_local_view_grid(
    center: GeographicPoint,
    radius_km: f64,
    requested_resolution_m: f64,
    max_pixels: u64,
) -> Result<LocalViewGrid, ProjectionError> {
    build_local_view_grid_inner(center, radius_km, requested_resolution_m, max_pixels, None, None)
        .map(|(grid, _)| grid)
        .map_err(|error| match error {
            LocalGridBuildError::Projection(error) => error,
            LocalGridBuildError::Core(_) => ProjectionError::AllocationFailed,
        })
}

pub(crate) fn build_local_view_grid_with_budget(
    center: GeographicPoint,
    radius_km: f64,
    requested_resolution_m: f64,
    max_pixels: u64,
    memory: &RasterMemoryBudget,
    control: &RasterWorkControl,
) -> CoreResult<(LocalViewGrid, RasterBufferLease)> {
    let (grid, lease) = build_local_view_grid_inner(
        center,
        radius_km,
        requested_resolution_m,
        max_pixels,
        Some(memory),
        Some(control),
    )
    .map_err(|error| match error {
        LocalGridBuildError::Projection(ProjectionError::PixelLimit { pixels, limit }) => {
            CoreError::ResourceLimit(format!("local-view pixels {pixels} > {limit}"))
        }
        LocalGridBuildError::Projection(ProjectionError::GridOverflow) => {
            CoreError::ResourceLimit("local-view grid dimensions overflow".into())
        }
        LocalGridBuildError::Projection(ProjectionError::AllocationFailed) => {
            CoreError::ResourceLimit("local-view grid allocation failed".into())
        }
        LocalGridBuildError::Projection(error) => CoreError::Storage(error.to_string()),
        LocalGridBuildError::Core(error) => error,
    })?;
    Ok((grid, lease.expect("budgeted local grid has a buffer lease")))
}

enum LocalGridBuildError {
    Projection(ProjectionError),
    Core(CoreError),
}

fn build_local_view_grid_inner(
    center: GeographicPoint,
    radius_km: f64,
    requested_resolution_m: f64,
    max_pixels: u64,
    memory: Option<&RasterMemoryBudget>,
    control: Option<&RasterWorkControl>,
) -> Result<(LocalViewGrid, Option<RasterBufferLease>), LocalGridBuildError> {
    if !radius_km.is_finite() || radius_km <= 0.0 || radius_km > MAX_RADIUS_KM {
        return Err(LocalGridBuildError::Projection(ProjectionError::InvalidRadius));
    }
    if !requested_resolution_m.is_finite() || requested_resolution_m <= 0.0 {
        return Err(LocalGridBuildError::Projection(ProjectionError::InvalidResolution));
    }
    let projection = AeqdWgs84::new(center).map_err(LocalGridBuildError::Projection)?;
    let center = projection.center();
    let radius_m = radius_km * 1000.0;
    let half_cells_f = (radius_m / requested_resolution_m).ceil();
    if !half_cells_f.is_finite() || half_cells_f < 1.0 || half_cells_f > usize::MAX as f64 {
        return Err(LocalGridBuildError::Projection(ProjectionError::GridOverflow));
    }
    let half_cells = half_cells_f as usize;
    let size = half_cells
        .checked_mul(2)
        .and_then(|diameter| diameter.checked_add(1))
        .ok_or(LocalGridBuildError::Projection(ProjectionError::GridOverflow))?;
    let pixels = size
        .checked_mul(size)
        .ok_or(LocalGridBuildError::Projection(ProjectionError::GridOverflow))?;
    let pixels_u64 = u64::try_from(pixels)
        .map_err(|_| LocalGridBuildError::Projection(ProjectionError::GridOverflow))?;
    if pixels_u64 > max_pixels {
        return Err(LocalGridBuildError::Projection(ProjectionError::PixelLimit {
            pixels: pixels_u64,
            limit: max_pixels,
        }));
    }
    if let Some(control) = control {
        control.check().map_err(LocalGridBuildError::Core)?;
    }
    let axis_bytes = u64::try_from(size)
        .ok()
        .and_then(|size| size.checked_mul(2 * std::mem::size_of::<f64>() as u64))
        .ok_or(LocalGridBuildError::Projection(ProjectionError::GridOverflow))?;
    let buffer_bytes = pixels_u64
        .checked_add(axis_bytes)
        .ok_or(LocalGridBuildError::Projection(ProjectionError::GridOverflow))?;
    let lease = memory
        .map(|memory| memory.reserve(buffer_bytes).map_err(LocalGridBuildError::Core))
        .transpose()?;
    let half_cells_f = half_cells as f64;
    let actual_resolution_m = radius_m / half_cells_f;
    let mut x = Vec::new();
    let mut y = Vec::new();
    let mut in_view = Vec::new();
    x.try_reserve_exact(size)
        .map_err(|_| LocalGridBuildError::Projection(ProjectionError::AllocationFailed))?;
    y.try_reserve_exact(size)
        .map_err(|_| LocalGridBuildError::Projection(ProjectionError::AllocationFailed))?;
    in_view
        .try_reserve_exact(pixels)
        .map_err(|_| LocalGridBuildError::Projection(ProjectionError::AllocationFailed))?;
    for column in 0..size {
        if column % 1024 == 0 {
            check_grid_control(control)?;
        }
        x.push((column as f64 - half_cells_f) / half_cells_f * radius_m);
    }
    for row in 0..size {
        check_grid_control(control)?;
        y.push((half_cells_f - row as f64) / half_cells_f * radius_m);
    }
    for &northing in &y {
        check_grid_control(control)?;
        for &easting in &x {
            in_view.push(easting.hypot(northing) <= radius_m);
        }
    }
    let crs = format!(
        "+proj=aeqd +lat_0={:.12} +lon_0={:.12} +datum=WGS84 +units=m +no_defs",
        center.latitude, center.longitude
    );
    let grid = Grid {
        shape: vec![size, size],
        crs: Some(crs),
        x,
        y,
        affine: Some([
            -radius_m - actual_resolution_m / 2.0,
            actual_resolution_m,
            0.0,
            radius_m + actual_resolution_m / 2.0,
            0.0,
            -actual_resolution_m,
        ]),
    };
    let result = LocalViewGrid {
        center,
        radius_m,
        requested_resolution_m,
        actual_resolution_m,
        grid,
        in_view,
        source_domain: None,
    };
    result.validate().map_err(|_| LocalGridBuildError::Projection(ProjectionError::InvalidGrid))?;
    Ok((result, lease))
}

fn check_grid_control(control: Option<&RasterWorkControl>) -> Result<(), LocalGridBuildError> {
    if let Some(control) = control {
        control.check().map_err(LocalGridBuildError::Core)?;
    }
    Ok(())
}

pub fn normalize_longitude(longitude: f64) -> f64 {
    (longitude + 180.0).rem_euclid(360.0) - 180.0
}

fn normalize_point(point: GeographicPoint) -> GeographicPoint {
    GeographicPoint { longitude: normalize_longitude(point.longitude), latitude: point.latitude }
}

fn validate_coordinate(point: GeographicPoint) -> Result<(), ProjectionError> {
    if point.longitude.is_finite()
        && point.latitude.is_finite()
        && (-180.0..=180.0).contains(&point.longitude)
        && (-90.0..=90.0).contains(&point.latitude)
    {
        Ok(())
    } else {
        Err(ProjectionError::InvalidCoordinate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_view_projection_keeps_center_and_cardinal_axes() {
        let projection =
            AeqdWgs84::new(GeographicPoint { longitude: 140.0, latitude: 35.0 }).unwrap();
        assert_eq!(projection.forward(projection.center()).unwrap(), (0.0, 0.0));
        let north = projection.inverse(0.0, 50_000.0).unwrap();
        let (x, y) = projection.forward(north).unwrap();
        assert!(x.abs() < 1e-6);
        assert!((y - 50_000.0).abs() < 1e-6);
        let east = projection.inverse(50_000.0, 0.0).unwrap();
        let (x, y) = projection.forward(east).unwrap();
        assert!((x - 50_000.0).abs() < 1e-6);
        assert!(y.abs() < 1e-6);
    }

    #[test]
    fn local_view_projection_builds_checked_default_grid() {
        let center = GeographicPoint { longitude: 180.0, latitude: 35.0 };
        let grid = build_local_view_grid(center, 50.0, 1000.0, 100_000).unwrap();
        assert_eq!(grid.grid.shape, [101, 101]);
        assert_eq!(grid.center.longitude, -180.0);
        assert_eq!(grid.grid.x[50], 0.0);
        assert_eq!(grid.grid.y[50], 0.0);
        assert!(grid.in_view[50 * 101 + 50]);
        assert!(!grid.in_view[0]);
        assert_eq!(grid.actual_resolution_m, 1000.0);
        assert!(matches!(
            build_local_view_grid(center, 50.0, 1000.0, 10),
            Err(ProjectionError::PixelLimit { .. })
        ));
    }

    #[test]
    fn local_view_projection_cardinal_tips_stay_in_view() {
        let center = GeographicPoint { longitude: 120.0, latitude: 30.0 };
        let grid = build_local_view_grid(center, 115.452, 2500.0, 100_000).unwrap();
        let size = grid.grid.shape[0];
        let mid = size / 2;
        let last = size - 1;
        assert_eq!(grid.grid.x[0], -grid.radius_m);
        assert_eq!(grid.grid.x[last], grid.radius_m);
        assert_eq!(grid.grid.y[0], grid.radius_m);
        assert_eq!(grid.grid.y[last], -grid.radius_m);
        assert!(grid.in_view[mid * size]);
        assert!(grid.in_view[mid * size + last]);
        assert!(grid.in_view[mid]);
        assert!(grid.in_view[last * size + mid]);
    }

    #[test]
    fn local_view_projection_rejects_invalid_inputs() {
        let ok = GeographicPoint { longitude: 120.0, latitude: 30.0 };
        let cases = [
            (ok, f64::NAN, 1000.0, ProjectionError::InvalidRadius),
            (ok, 0.0, 1000.0, ProjectionError::InvalidRadius),
            (ok, -1.0, 1000.0, ProjectionError::InvalidRadius),
            (ok, 500.001, 1000.0, ProjectionError::InvalidRadius),
            (ok, 50.0, 0.0, ProjectionError::InvalidResolution),
            (ok, 50.0, -5.0, ProjectionError::InvalidResolution),
            (ok, 50.0, f64::INFINITY, ProjectionError::InvalidResolution),
            (
                GeographicPoint { longitude: 120.0, latitude: 91.0 },
                50.0,
                1000.0,
                ProjectionError::InvalidCoordinate,
            ),
            (
                GeographicPoint { longitude: f64::NAN, latitude: 30.0 },
                50.0,
                1000.0,
                ProjectionError::InvalidCoordinate,
            ),
            (ok, 50.0, 1e-300, ProjectionError::GridOverflow),
        ];
        for (center, radius, resolution, expected) in cases {
            assert_eq!(
                build_local_view_grid(center, radius, resolution, 100_000).unwrap_err(),
                expected,
                "radius={radius} resolution={resolution} center={center:?}"
            );
        }
    }
}
