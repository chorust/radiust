//! Shared Web Mercator XYZ selection. Plans are bounded and carried in frame identity.
use crate::errors::{CoreError, CoreResult};
use crate::model::GeographicPoint;
use serde::{Deserialize, Serialize};

const MAX_LAT: f64 = 85.0511287798066;
const MAX_TILES: u32 = 64;
/// Highest zoom any XYZ request may name, before a source applies its own limit.
pub(crate) const MAX_REQUEST_ZOOM: u32 = 22;
/// Mosaic pixel budget. It keeps every plan inside the raw preview's 64 MiB
/// memory cap (RGBA mosaic plus one tile of decode memory) at 256 and 512 px.
const MAX_MOSAIC_PIXELS: u64 = 15_000_000;

/// Acquisition viewport, independent of the decoded output grid. A west value
/// greater than east denotes a view crossing the antimeridian, at most 180° wide;
/// a wider wrapped span is rejected as a probable west/east swap.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TileRequest {
    pub bbox: Option<[f64; 4]>,
    pub zoom: Option<u32>,
    /// Desired ground spacing at the viewport's center, used for automatic zoom.
    pub resolution_m: Option<f64>,
}

impl TileRequest {
    pub fn validate(&self) -> CoreResult<()> {
        if self.zoom.is_some_and(|z| z > MAX_REQUEST_ZOOM)
            || self.resolution_m.is_some_and(|r| !r.is_finite() || r <= 0.0)
            || self.bbox.is_some_and(|[w, s, e, n]| {
                ![w, s, e, n].iter().all(|v| v.is_finite())
                    || !(-180.0..=180.0).contains(&w)
                    || !(-180.0..=180.0).contains(&e)
                    || !(-90.0..=90.0).contains(&s)
                    || !(-90.0..=90.0).contains(&n)
                    || w == e
                    || (w == 180.0 && e == -180.0)
                    || (w > e && e + 360.0 - w > 180.0)
                    || s >= n
            })
        {
            return Err(CoreError::Transport("invalid tile viewport, zoom, or resolution".into()));
        }
        Ok(())
    }

    pub(crate) fn local(
        center: GeographicPoint,
        radius_km: f64,
        resolution_m: f64,
        zoom: Option<u32>,
    ) -> Self {
        // A conservative spherical bound using a radius below WGS84's minimum
        // curvature radius. Include a sampling halo for nearest/bilinear lookup.
        let angular =
            ((radius_km * 1000.0 + resolution_m * 2.0) / 6_335_000.0).min(std::f64::consts::PI);
        let lat = center.latitude.to_radians();
        let south = (lat - angular).to_degrees().max(-90.0);
        let north = (lat + angular).to_degrees().min(90.0);
        let (west, east) = if lat.abs() + angular >= std::f64::consts::FRAC_PI_2 {
            (-180.0, 180.0)
        } else {
            let delta = (angular.sin() / lat.cos()).asin().to_degrees();
            (
                crate::projection::normalize_longitude(center.longitude - delta),
                crate::projection::normalize_longitude(center.longitude + delta),
            )
        };
        Self { bbox: Some([west, south, east, north]), zoom, resolution_m: Some(resolution_m) }
    }
}

/// One tile of a plan: its XYZ address and its position in the mosaic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PlanTile {
    pub x: u32,
    pub y: u32,
    pub column: u32,
    pub row: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TilePlan {
    pub zoom: u32,
    pub x_start: u32,
    pub y_start: u32,
    pub columns: u32,
    pub rows: u32,
}

impl Default for TilePlan {
    fn default() -> Self {
        Self { zoom: 1, x_start: 0, y_start: 0, columns: 2, rows: 2 }
    }
}

impl TilePlan {
    /// The legacy global plan, which frames serialize without a selection.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn pixels(&self, tile_size: u32) -> u64 {
        u64::from(self.columns) * u64::from(self.rows) * u64::from(tile_size).pow(2)
    }

    pub fn select(
        request: Option<&TileRequest>,
        tile_size: u32,
        max_zoom: u32,
    ) -> CoreResult<Self> {
        let Some(request) = request else {
            return Ok(Self::default());
        };
        request.validate()?;
        if request.zoom.is_some_and(|z| z > max_zoom) {
            return Err(CoreError::Transport(format!("tile source supports zoom 0..={max_zoom}")));
        }
        let bbox = request.bbox.unwrap_or([-180.0, -MAX_LAT, 180.0, MAX_LAT]);
        let [w, s, e, n] = bbox;
        if s >= MAX_LAT || n <= -MAX_LAT {
            return Err(CoreError::Transport(
                "tile viewport is outside Web Mercator coverage".into(),
            ));
        }
        let span = if e > w { e - w } else { e + 360.0 - w };
        let w = if w == 180.0 { -180.0 } else { w };
        let mut zoom = request.zoom.unwrap_or_else(|| {
            let z = if let Some(resolution) = request.resolution_m {
                (40_075_016.68557849 * ((s + n) * 0.5).to_radians().cos()
                    / (f64::from(tile_size) * resolution))
                    .log2()
                    .ceil()
            } else {
                (1024.0 * 360.0 / (f64::from(tile_size) * span)).log2().ceil()
            };
            z.max(0.0).min(f64::from(max_zoom)) as u32
        });
        loop {
            let size = 1_u32 << zoom;
            let scale = f64::from(size);
            let x0 = ((w + 180.0) / 360.0 * scale).floor().max(0.0) as u32;
            let x1 = (((w + span + 180.0) / 360.0 * scale).ceil() as u32).max(x0 + 1);
            let y0 = (mercator_y(n) * scale).floor().max(0.0) as u32;
            let y1 = (mercator_y(s) * scale).ceil().min(scale) as u32;
            let plan = Self {
                zoom,
                x_start: x0.min(size - 1),
                y_start: y0.min(size - 1),
                columns: (x1 - x0).min(size),
                rows: y1.saturating_sub(y0).max(1),
            };
            if u64::from(plan.columns) * u64::from(plan.rows) <= u64::from(MAX_TILES)
                && plan.pixels(tile_size) <= MAX_MOSAIC_PIXELS
            {
                plan.validate(max_zoom)?;
                return Ok(plan);
            }
            if request.zoom.is_some() || zoom == 0 {
                return Err(CoreError::ResourceLimit(
                    "tile viewport exceeds the tile or mosaic pixel limit".into(),
                ));
            }
            zoom -= 1;
        }
    }

    pub fn validate(&self, max_zoom: u32) -> CoreResult<()> {
        if self.zoom > max_zoom.min(MAX_REQUEST_ZOOM) {
            return Err(CoreError::Transport("invalid tile plan zoom".into()));
        }
        let size = 1_u32 << self.zoom;
        if self.columns == 0
            || self.rows == 0
            || self.columns > size
            || self.x_start >= size
            || self.y_start >= size
            || self.rows > size - self.y_start
            || u64::from(self.columns) * u64::from(self.rows) > u64::from(MAX_TILES)
        {
            return Err(CoreError::Transport("invalid tile plan extent".into()));
        }
        Ok(())
    }

    pub fn from_locator(frame: &crate::model::FrameRef, max_zoom: u32) -> CoreResult<Self> {
        let plan = match frame.locator.get("tile_selection") {
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|_| CoreError::Transport("invalid tile selection".into()))?,
            None => Self::default(),
        };
        plan.validate(max_zoom)?;
        Ok(plan)
    }

    /// Canonical XYZ coordinates and relative mosaic offsets, in display order.
    pub fn tiles(&self) -> impl Iterator<Item = PlanTile> + '_ {
        (0..self.rows).flat_map(move |row| {
            (0..self.columns).map(move |column| PlanTile {
                x: (self.x_start + column) % (1 << self.zoom),
                y: self.y_start + row,
                column,
                row,
            })
        })
    }

    pub fn name(&self, x: u32, y: u32) -> String {
        format!("tile-z{}-x{x}-y{y}.png", self.zoom)
    }

    pub fn axes(&self, tile_size: u32) -> (Vec<f64>, Vec<f64>) {
        let world = f64::from(tile_size) * f64::from(1_u32 << self.zoom);
        let x = (0..self.columns * tile_size)
            .map(|i| {
                (f64::from(self.x_start) * f64::from(tile_size) + f64::from(i) + 0.5) / world
                    * 360.0
                    - 180.0
            })
            .collect();
        let y = (0..self.rows * tile_size)
            .map(|i| {
                let normalized =
                    (f64::from(self.y_start) * f64::from(tile_size) + f64::from(i) + 0.5) / world;
                (std::f64::consts::PI * (1.0 - 2.0 * normalized)).sinh().atan().to_degrees()
            })
            .collect();
        (x, y)
    }
}

fn mercator_y(latitude: f64) -> f64 {
    let lat = latitude.clamp(-MAX_LAT, MAX_LAT).to_radians();
    ((1.0 - lat.tan().asinh() / std::f64::consts::PI) / 2.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_selects_only_intersecting_tiles_and_automatic_zoom_is_bounded() {
        let request =
            TileRequest { bbox: Some([10.0, 20.0, 11.0, 21.0]), zoom: Some(5), resolution_m: None };
        let plan = TilePlan::select(Some(&request), 512, 7).unwrap();
        assert_eq!(plan, TilePlan { zoom: 5, x_start: 16, y_start: 14, columns: 1, rows: 1 });
        let automatic = TilePlan::select(
            Some(&TileRequest { zoom: None, resolution_m: Some(1000.0), ..request }),
            512,
            7,
        )
        .unwrap();
        assert_eq!(automatic.zoom, 7);
        assert!(automatic.columns * automatic.rows <= 64);
        assert!(automatic.pixels(512) <= MAX_MOSAIC_PIXELS);
        let coarse = TilePlan::select(
            Some(&TileRequest {
                bbox: Some([-170.0, -80.0, 170.0, 80.0]),
                zoom: None,
                resolution_m: Some(1.0),
            }),
            512,
            7,
        )
        .unwrap();
        assert!(coarse.zoom < 7);
        assert!(coarse.columns * coarse.rows <= 64);
    }

    #[test]
    fn requests_resolving_to_the_global_default_keep_the_default_identity() {
        let zoom_one = TileRequest { zoom: Some(1), ..TileRequest::default() };
        assert!(TilePlan::select(Some(&zoom_one), 512, 7).unwrap().is_default());
        assert!(TilePlan::select(Some(&TileRequest::default()), 512, 7).unwrap().is_default());
        let zoom_two = TileRequest { zoom: Some(2), ..TileRequest::default() };
        assert!(!TilePlan::select(Some(&zoom_two), 512, 7).unwrap().is_default());
    }

    #[test]
    fn automatic_plans_fit_the_raw_preview_pixel_budget() {
        let request = TileRequest { resolution_m: Some(1.0), ..TileRequest::default() };
        let plan = TilePlan::select(Some(&request), 512, 7).unwrap();
        assert!(plan.pixels(512) <= MAX_MOSAIC_PIXELS);
        // 8x8 tiles at 512 px would be 16.8 Mpx; an explicit zoom must be refused.
        let explicit = TileRequest { zoom: Some(3), ..TileRequest::default() };
        assert!(TilePlan::select(Some(&explicit), 512, 7).is_err());
    }

    #[test]
    fn wrapped_view_preserves_xyz_order_and_continuous_pixel_centers() {
        let plan = TilePlan::select(
            Some(&TileRequest {
                bbox: Some([170.0, 1.0, -170.0, 10.0]),
                zoom: Some(3),
                resolution_m: None,
            }),
            512,
            7,
        )
        .unwrap();
        assert_eq!(
            plan.tiles().map(|t| (t.x, t.y, t.column, t.row)).collect::<Vec<_>>(),
            [(7, 3, 0, 0), (0, 3, 1, 0)]
        );
        let (x, y) = plan.axes(512);
        assert_eq!((x.len(), y.len()), (1024, 512));
        assert!((x[0] - 135.0439453125).abs() < 1e-10);
        assert!((x[1023] - 224.9560546875).abs() < 1e-10);
        assert!(x.windows(2).all(|v| v[0] < v[1]));
        assert!(y.windows(2).all(|v| v[0] > v[1]));
    }

    #[test]
    fn boundary_alignment_poles_and_invalid_or_oversized_requests() {
        let select = |bbox, zoom| {
            TilePlan::select(
                Some(&TileRequest { bbox: Some(bbox), zoom: Some(zoom), resolution_m: None }),
                512,
                7,
            )
        };
        let aligned = select([-180.0, 0.0, 0.0, MAX_LAT], 1).unwrap();
        assert_eq!(aligned.tiles().count(), 1);
        assert_eq!((aligned.x_start, aligned.y_start), (0, 0));
        assert!(select([0.0, 86.0, 10.0, 90.0], 2).is_err());
        assert!(select([-180.0, -80.0, 180.0, 80.0], 7).is_err());
        assert!(select([0.0, 0.0, 10.0, 10.0], 8).is_err());
        assert!(select([f64::NAN, 0.0, 10.0, 10.0], 2).is_err());
        assert!(select([10.0, 0.0, 10.0, 10.0], 2).is_err());
        assert!(select([180.0, 0.0, -180.0, 10.0], 2).is_err());
        // West/east swapped: a 350° wrapped span is rejected, not wrapped.
        assert!(select([20.0, 0.0, 10.0, 10.0], 2).is_err());
        assert_eq!(select([180.0, 1.0, -170.0, 10.0], 2).unwrap().x_start, 0);
        assert_eq!(select([-180.0, -90.0, 180.0, 90.0], 0).unwrap().tiles().count(), 1);
        assert!(
            TilePlan { zoom: 22, x_start: 0, y_start: 0, columns: u32::MAX, rows: u32::MAX }
                .validate(22)
                .is_err()
        );
    }

    #[test]
    fn local_view_includes_the_date_line_and_high_latitude_bound() {
        let local = TileRequest::local(
            GeographicPoint { longitude: 179.9, latitude: 50.0 },
            50.0,
            1000.0,
            None,
        );
        let [w, _, e, _] = local.bbox.unwrap();
        assert!(w > e);
        assert!(TilePlan::select(Some(&local), 512, 7).is_ok());
        let polar = TileRequest::local(
            GeographicPoint { longitude: 0.0, latitude: 89.0 },
            500.0,
            1000.0,
            None,
        );
        assert_eq!(polar.bbox.unwrap()[0], -180.0);
        assert_eq!(polar.bbox.unwrap()[2], 180.0);
    }
}
