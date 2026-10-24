use crate::errors::{CoreError, CoreResult};
use crate::grid::{
    GridSampler, Resampling, continuous_longitude_axis, epsg_code, same_crs, supports_regrid_crs,
    validate_sampling_method, wgs84_to_source_grid,
};
use crate::limits::{Limits, RasterBufferLease, RasterMemoryBudget, RasterWorkControl};
use crate::model::{
    DiscoveryTarget, GeographicPoint, LocalViewAttempt, LocalViewCandidateState, LocalViewGrid,
    LocalViewMode, LocalViewRequest, LocalViewResampling, Preview, RadarField,
};
use crate::projection::{AeqdWgs84, ProjectionError};
use crate::source::catalog::{
    CatalogSpatialTarget, ObservationCoverageKind, PixelRegistration, RasterGeometryEvidence,
    SpatialAdmission, SpatialDomain, SpatialEvidenceStatus, SpatialSourceClass, SpatialTargetKind,
};
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct ProjectedField {
    pub grid: LocalViewGrid,
    pub field: RadarField,
}

pub(crate) fn project_raw_image_nearest(
    source: &Preview,
    geometry: &RasterGeometryEvidence,
    mut target: LocalViewGrid,
    limits: &Limits,
    memory: &RasterMemoryBudget,
    control: &RasterWorkControl,
) -> CoreResult<(LocalViewGrid, Vec<u8>, RasterBufferLease)> {
    source.validate().map_err(|_| CoreError::Storage("raw local-view image is invalid".into()))?;
    target.validate().map_err(|_| CoreError::Storage("local target grid is invalid".into()))?;
    control.check()?;
    if geometry.status != SpatialEvidenceStatus::Verified
        || geometry.evidence.is_empty()
        || geometry.crs.as_deref() != Some("EPSG:3857")
        || geometry.registration != PixelRegistration::Edge
        || geometry.shape != Some([source.height, source.width])
        || geometry.data_window != Some([0, 0, source.width, source.height])
        || geometry.artifact_sha256.is_none()
    {
        return Err(CoreError::Storage(
            "raw local view requires receipt-bound full-image geometry".into(),
        ));
    }
    let [origin_x, step_x, rotate_x, origin_y, rotate_y, step_y] = geometry
        .affine
        .ok_or_else(|| CoreError::Storage("raw local view geometry has no affine".into()))?;
    if rotate_x != 0.0 || rotate_y != 0.0 || step_x <= 0.0 || step_y >= 0.0 {
        return Err(CoreError::Storage(
            "rotated or degenerate raw image geometry is unsupported".into(),
        ));
    }
    let source_x_end = origin_x + step_x * f64::from(source.width);
    let source_y_end = origin_y + step_y * f64::from(source.height);
    if ![origin_x, source_x_end, origin_y, source_y_end].iter().all(|value| value.is_finite()) {
        return Err(CoreError::ResourceLimit("raw image edge coordinates overflow".into()));
    }
    let source_x_axis = [origin_x, source_x_end];
    let [height, width] = target.grid.shape.as_slice() else {
        return Err(CoreError::Storage("local target grid must be two-dimensional".into()));
    };
    let pixels = height
        .checked_mul(*width)
        .ok_or_else(|| CoreError::ResourceLimit("local target grid dimensions overflow".into()))?;
    limits.validate_pixels(
        u64::try_from(pixels).map_err(|_| {
            CoreError::ResourceLimit("local target grid dimensions overflow".into())
        })?,
    )?;
    let output_bytes = u64::try_from(pixels)
        .ok()
        .and_then(|pixels| pixels.checked_mul(5))
        .ok_or_else(|| CoreError::ResourceLimit("raw local-view output size overflows".into()))?;
    let output_lease = memory.reserve(output_bytes)?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(
        pixels.checked_mul(4).ok_or_else(|| {
            CoreError::ResourceLimit("raw local-view image size overflows".into())
        })?,
    )
    .map_err(|_| CoreError::ResourceLimit("raw local-view image allocation failed".into()))?;
    rgba.resize(pixels * 4, 0);
    let mut source_domain = Vec::new();
    source_domain
        .try_reserve_exact(pixels)
        .map_err(|_| CoreError::ResourceLimit("raw local-view mask allocation failed".into()))?;

    let projection = AeqdWgs84::new(target.center)
        .map_err(|_| CoreError::Storage("local projection center is invalid".into()))?;
    let width = *width;
    for (row, &northing) in target.grid.y.iter().enumerate() {
        control.check()?;
        for (column, &easting) in target.grid.x.iter().enumerate() {
            let index = row * width + column;
            if !target.in_view[index] {
                source_domain.push(false);
                continue;
            }
            if column % 256 == 0 {
                control.check()?;
            }
            let point = projection.inverse(easting, northing).map_err(map_projection_error)?;
            let Some((x, y)) = wgs84_to_source_grid(
                Some("EPSG:3857"),
                point.longitude,
                point.latitude,
                &source_x_axis,
            ) else {
                source_domain.push(false);
                continue;
            };
            let pixel_x = (x - origin_x) / step_x;
            let pixel_y = (y - origin_y) / step_y;
            if !pixel_x.is_finite()
                || !pixel_y.is_finite()
                || !(0.0..=f64::from(source.width)).contains(&pixel_x)
                || !(0.0..=f64::from(source.height)).contains(&pixel_y)
            {
                source_domain.push(false);
                continue;
            }
            source_domain.push(true);
            let source_column =
                ((pixel_x - 1.0).ceil().clamp(0.0, f64::from(source.width - 1))) as usize;
            let source_row =
                ((pixel_y - 1.0).ceil().clamp(0.0, f64::from(source.height - 1))) as usize;
            let source_index = (source_row * source.width as usize + source_column) * 4;
            let target_index = index * 4;
            rgba[target_index..target_index + 4]
                .copy_from_slice(&source.rgba[source_index..source_index + 4]);
        }
    }
    let center_index = (height / 2)
        .checked_mul(width)
        .and_then(|index| index.checked_add(width / 2))
        .ok_or_else(|| CoreError::ResourceLimit("local center index overflow".into()))?;
    if !source_domain.get(center_index).copied().unwrap_or(false) {
        return Err(CoreError::Storage("user location is outside the current frame domain".into()));
    }
    target.source_domain = Some(source_domain);
    target.validate().map_err(|_| CoreError::Storage("projected raw grid is invalid".into()))?;
    control.check()?;
    Ok((target, rgba, output_lease))
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SpatialRelation {
    InsideVerifiedCoverage,
    InsideProductDomain,
    NeedsProbe,
    Outside,
}

#[derive(Clone, Debug)]
pub struct LocalViewCandidate {
    pub spatial: CatalogSpatialTarget,
    pub relation: SpatialRelation,
    pub site_distance_m: Option<f64>,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct LocalViewCandidateSelection {
    pub candidates: Vec<LocalViewCandidate>,
    pub excluded: Vec<LocalViewAttempt>,
}

#[derive(Clone, Debug, Default)]
pub struct LocalViewSelectionPolicy {
    pub allow_network: bool,
    pub source_exclusions: BTreeMap<String, String>,
}

/// Filter the existing catalog targets and order eligible local-view probes
/// from evidence strength to stable source/product/station identity.
pub fn select_candidates(
    targets: Vec<CatalogSpatialTarget>,
    request: &LocalViewRequest,
    center: GeographicPoint,
    policy: &LocalViewSelectionPolicy,
) -> LocalViewCandidateSelection {
    let projection = AeqdWgs84::new(center).ok();
    let explicit_source = request.source.is_some();
    let mut selection = LocalViewCandidateSelection::default();
    for spatial in targets {
        if request.source.as_ref().is_some_and(|source| source != &spatial.target.source)
            || request
                .product
                .as_ref()
                .is_some_and(|product| spatial.target.product.as_ref() != Some(product))
            || request
                .station
                .as_ref()
                .is_some_and(|station| spatial.target.station.as_ref() != Some(station))
        {
            continue;
        }

        let exclusion = if spatial.availability == "retired" {
            Some("source_retired")
        } else if let Some(reason) = policy.source_exclusions.get(&spatial.target.source) {
            Some(reason.as_str())
        } else if !policy.allow_network {
            Some("network_restricted")
        } else if !supports_requested_mode(&spatial, request.mode, request.resampling) {
            Some("unsupported_mode_or_geometry")
        } else if !explicit_source && !admission_allows_automatic(spatial.admission.as_ref()) {
            Some("admission_unverified")
        } else {
            None
        };
        if let Some(reason) = exclusion {
            selection.excluded.push(excluded_attempt(&spatial, reason));
            continue;
        }

        let relation = spatial_relation(&spatial, center);
        let relation = match relation {
            Some(SpatialRelation::Outside) => {
                selection.excluded.push(excluded_attempt(&spatial, "outside_domain"));
                continue;
            }
            Some(relation) => relation,
            None if explicit_source && supports_native_numeric(&spatial.target) => {
                SpatialRelation::NeedsProbe
            }
            None => {
                selection.excluded.push(excluded_attempt(&spatial, "spatial_evidence_unknown"));
                continue;
            }
        };
        let site_distance_m = if spatial.kind == SpatialTargetKind::PhysicalStation
            && spatial.site_status == SpatialEvidenceStatus::Verified
        {
            spatial.site.and_then(|site| {
                projection
                    .as_ref()
                    .and_then(|projection| projection.forward(site).ok())
                    .map(|(east, north)| east.hypot(north))
            })
        } else {
            None
        };
        let reason = match relation {
            SpatialRelation::InsideVerifiedCoverage => "inside_verified_coverage",
            SpatialRelation::InsideProductDomain => "inside_product_domain_coverage_unknown",
            SpatialRelation::NeedsProbe => "needs_probe_coverage_unknown",
            SpatialRelation::Outside => {
                unreachable!("outside candidates are excluded before ranking")
            }
        }
        .to_owned();
        selection.candidates.push(LocalViewCandidate {
            spatial,
            relation,
            site_distance_m,
            reason,
        });
    }
    selection.candidates.sort_by(|left, right| {
        left.relation
            .cmp(&right.relation)
            .then_with(|| {
                if request.mode == LocalViewMode::Auto {
                    mode_preference(&left.spatial).cmp(&mode_preference(&right.spatial))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| {
                source_class_rank(left.spatial.source_class)
                    .cmp(&source_class_rank(right.spatial.source_class))
            })
            .then_with(|| match (left.site_distance_m, right.site_distance_m) {
                (Some(left), Some(right)) => left.total_cmp(&right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| left.spatial.target.cmp(&right.spatial.target))
    });
    selection
}

fn excluded_attempt(spatial: &CatalogSpatialTarget, reason: &str) -> LocalViewAttempt {
    LocalViewAttempt {
        target: spatial.target.clone(),
        state: LocalViewCandidateState::Excluded,
        discovery_status: None,
        reason: reason.to_owned(),
        error: None,
        observation_coverage: spatial.observation_coverage.clone(),
        spatial_evidence: Some(crate::model::LocalViewSpatialEvidence::from_target(
            spatial, None, reason,
        )),
    }
}

fn admission_allows_automatic(admission: Option<&SpatialAdmission>) -> bool {
    admission.is_some_and(|admission| {
        admission.automatic
            && admission.status == SpatialEvidenceStatus::Verified
            && !admission.evidence.is_empty()
            && admission.purposes.iter().any(|purpose| purpose == "local_view")
    })
}

fn supports_requested_mode(
    target: &CatalogSpatialTarget,
    mode: LocalViewMode,
    resampling: LocalViewResampling,
) -> bool {
    if resampling == LocalViewResampling::Bilinear {
        return supports_native_numeric(&target.target);
    }
    match mode {
        LocalViewMode::Auto | LocalViewMode::Decoded | LocalViewMode::Dbz => {
            supports_native_numeric(&target.target)
                || (mode == LocalViewMode::Auto && supports_raw_image(target))
        }
        LocalViewMode::Raw => supports_raw_image(target),
    }
}

fn supports_raw_image(target: &CatalogSpatialTarget) -> bool {
    target.target.source == "pt"
        && target.target.product.as_deref() == Some("composite")
        && target.geometry.as_ref().is_some_and(|geometry| {
            geometry.status == SpatialEvidenceStatus::Verified
                && geometry.crs.as_deref() == Some("EPSG:3857")
                && geometry.registration == PixelRegistration::Edge
                && geometry.shape == Some([1526, 1500])
                && geometry.affine.is_some()
                && geometry.data_window == Some([0, 0, 1500, 1526])
                && !geometry.evidence.is_empty()
        })
}

fn mode_preference(target: &CatalogSpatialTarget) -> u8 {
    if supports_native_numeric(&target.target) { 0 } else { 1 }
}

fn supports_native_numeric(target: &DiscoveryTarget) -> bool {
    matches!(
        (target.source.as_str(), target.product.as_deref()),
        ("rainviewer", Some("composite")) | ("rdcap", Some("reflectivity"))
    )
}

fn spatial_relation(
    target: &CatalogSpatialTarget,
    center: GeographicPoint,
) -> Option<SpatialRelation> {
    if let Some(coverage) = target
        .observation_coverage
        .as_ref()
        .filter(|coverage| coverage.status == SpatialEvidenceStatus::Verified)
    {
        match coverage_contains(coverage.kind, coverage.geometry.as_ref(), center) {
            Some(true) => return Some(SpatialRelation::InsideVerifiedCoverage),
            Some(false) => return Some(SpatialRelation::Outside),
            None => {}
        }
    }
    if let Some(domain) = target.product_domain.as_ref().filter(|domain| {
        matches!(domain.status, SpatialEvidenceStatus::Verified | SpatialEvidenceStatus::Declared)
            && !domain.evidence.is_empty()
    }) {
        match domain_contains(domain, center) {
            Some(true) => return Some(SpatialRelation::InsideProductDomain),
            Some(false) if domain.status == SpatialEvidenceStatus::Verified => {
                return Some(SpatialRelation::Outside);
            }
            Some(false) | None => {}
        }
    }
    if (supports_native_numeric(&target.target) || supports_raw_image(target))
        && target.geometry.as_ref().is_none_or(|geometry| {
            !matches!(
                geometry.status,
                SpatialEvidenceStatus::Conflicting | SpatialEvidenceStatus::Unsupported
            )
        })
    {
        Some(SpatialRelation::NeedsProbe)
    } else {
        None
    }
}

fn coverage_contains(
    kind: ObservationCoverageKind,
    geometry: Option<&serde_json::Value>,
    point: GeographicPoint,
) -> Option<bool> {
    let geometry = geometry?;
    match kind {
        ObservationCoverageKind::GeodesicCircle => {
            let center = GeographicPoint {
                longitude: geometry.get("longitude")?.as_f64()?,
                latitude: geometry.get("latitude")?.as_f64()?,
            };
            let radius_m = geometry.get("radius_m")?.as_f64()?;
            if !radius_m.is_finite() || radius_m <= 0.0 {
                return None;
            }
            let projection = AeqdWgs84::new(center).ok()?;
            let (east, north) = projection.forward(point).ok()?;
            Some(east.hypot(north) <= radius_m)
        }
        ObservationCoverageKind::Polygon => geojson_polygon_contains(geometry, point),
        ObservationCoverageKind::Mask | ObservationCoverageKind::Unknown => None,
    }
}

fn domain_contains(domain: &SpatialDomain, point: GeographicPoint) -> Option<bool> {
    let geometry = domain.geometry.as_ref()?;
    if geometry.get("type").and_then(serde_json::Value::as_str) == Some("Polygon") {
        return geojson_polygon_contains(geometry, point);
    }
    let bbox = geometry.get("bbox")?.as_array()?;
    if bbox.len() != 4 {
        return None;
    }
    let west = bbox[0].as_f64()?;
    let south = bbox[1].as_f64()?;
    let east = bbox[2].as_f64()?;
    let north = bbox[3].as_f64()?;
    if [west, south, east, north].iter().any(|value| !value.is_finite())
        || south > north
        || south < -90.0
        || north > 90.0
        || west.abs() > 180.0
        || east.abs() > 180.0
    {
        return None;
    }
    let longitude_inside = if west <= east {
        (west..=east).contains(&point.longitude)
    } else {
        point.longitude >= west || point.longitude <= east
    };
    Some(longitude_inside && (south..=north).contains(&point.latitude))
}

fn geojson_polygon_contains(geometry: &serde_json::Value, point: GeographicPoint) -> Option<bool> {
    let rings = geometry.get("coordinates")?.as_array()?;
    let exterior = polygon_ring_contains(rings.first()?, point)?;
    let mut in_hole = false;
    for ring in &rings[1..] {
        in_hole |= polygon_ring_contains(ring, point)?;
    }
    Some(exterior && !in_hole)
}

fn polygon_ring_contains(ring: &serde_json::Value, point: GeographicPoint) -> Option<bool> {
    let ring = ring.as_array()?;
    if ring.len() < 4 || ring.first()? != ring.last()? {
        return None;
    }
    let coordinate = |value: &serde_json::Value| {
        let pair = value.as_array()?;
        let longitude = pair.first()?.as_f64()?;
        let latitude = pair.get(1)?.as_f64()?;
        (longitude.is_finite() && latitude.is_finite())
            .then(|| (unwrap_longitude(longitude, point.longitude), latitude))
    };
    let mut previous = coordinate(ring.first()?)?;
    let mut inside = false;
    let mut boundary = false;
    for vertex in &ring[1..] {
        let current = coordinate(vertex)?;
        let (x1, y1) = previous;
        let (x2, y2) = current;
        let cross = (point.longitude - x1) * (y2 - y1) - (point.latitude - y1) * (x2 - x1);
        boundary |= cross == 0.0
            && (x1.min(x2)..=x1.max(x2)).contains(&point.longitude)
            && (y1.min(y2)..=y1.max(y2)).contains(&point.latitude);
        if (y1 > point.latitude) != (y2 > point.latitude)
            && point.longitude < (x2 - x1) * (point.latitude - y1) / (y2 - y1) + x1
        {
            inside = !inside;
        }
        previous = current;
    }
    Some(inside || boundary)
}

fn unwrap_longitude(longitude: f64, reference: f64) -> f64 {
    reference + (longitude - reference + 180.0).rem_euclid(360.0) - 180.0
}

fn source_class_rank(class: SpatialSourceClass) -> u8 {
    match class {
        SpatialSourceClass::Official => 0,
        SpatialSourceClass::ThirdParty => 1,
        SpatialSourceClass::Unknown => 2,
    }
}

/// Project one field into an AEQD grid using exact native nearest samples.
///
/// Geometry must be verified for the current source field. The source CRS is
/// limited to EPSG:4326 and EPSG:3857; unsupported or conflicting geometry is
/// rejected before allocating the output planes.
pub fn project_field_nearest(
    source: &RadarField,
    geometry: &RasterGeometryEvidence,
    target: LocalViewGrid,
) -> CoreResult<ProjectedField> {
    let limits = Limits::default();
    for shape in [&source.shape, &target.grid.shape] {
        let pixels = shape
            .iter()
            .try_fold(1_u64, |count, &size| count.checked_mul(u64::try_from(size).ok()?))
            .ok_or_else(|| {
                CoreError::ResourceLimit("local projection dimensions overflow".into())
            })?;
        limits.validate_pixels(pixels)?;
    }
    project_field_inner(
        source,
        geometry,
        target,
        Resampling::Nearest,
        &limits.raster_memory_budget(),
        None,
    )
    .map(|(projected, _)| projected)
}

#[cfg(test)]
pub(crate) fn project_field_with_resampling(
    source: &RadarField,
    geometry: &RasterGeometryEvidence,
    target: LocalViewGrid,
    resampling: Resampling,
) -> CoreResult<ProjectedField> {
    project_field_inner(
        source,
        geometry,
        target,
        resampling,
        &RasterMemoryBudget::new(u64::MAX),
        None,
    )
    .map(|(projected, _)| projected)
}

#[cfg(test)]
pub(crate) fn project_field_nearest_with_budget(
    source: &RadarField,
    geometry: &RasterGeometryEvidence,
    target: LocalViewGrid,
    memory: &RasterMemoryBudget,
    control: &RasterWorkControl,
) -> CoreResult<(ProjectedField, RasterBufferLease)> {
    project_field_with_resampling_with_budget(
        source,
        geometry,
        target,
        Resampling::Nearest,
        memory,
        control,
    )
}

pub(crate) fn project_field_with_resampling_with_budget(
    source: &RadarField,
    geometry: &RasterGeometryEvidence,
    target: LocalViewGrid,
    resampling: Resampling,
    memory: &RasterMemoryBudget,
    control: &RasterWorkControl,
) -> CoreResult<(ProjectedField, RasterBufferLease)> {
    project_field_inner(source, geometry, target, resampling, memory, Some(control))
}

fn project_field_inner(
    source: &RadarField,
    geometry: &RasterGeometryEvidence,
    mut target: LocalViewGrid,
    resampling: Resampling,
    memory: &RasterMemoryBudget,
    control: Option<&RasterWorkControl>,
) -> CoreResult<(ProjectedField, RasterBufferLease)> {
    target.validate().map_err(|_| CoreError::Storage("local target grid is invalid".into()))?;
    if let Some(control) = control {
        control.check()?;
    }
    validate_sampling_method(source, resampling)?;
    let [source_height, source_width] = source.shape.as_slice() else {
        return Err(CoreError::Storage("local projection requires a two-dimensional field".into()));
    };
    let source_axis_count = source_width
        .checked_add(*source_height)
        .and_then(|count| u64::try_from(count).ok())
        .ok_or_else(|| CoreError::ResourceLimit("source axis size overflows".into()))?;
    // Edge registration temporarily holds edges, centers, and normalized axes.
    let sampler_axis_count = source_axis_count
        .checked_mul(if geometry.registration == PixelRegistration::Edge { 3 } else { 1 })
        .and_then(|count| {
            count.checked_add(if geometry.registration == PixelRegistration::Edge { 2 } else { 0 })
        })
        .ok_or_else(|| CoreError::ResourceLimit("sampler axis size overflows".into()))?;
    let sampler_lease = memory.reserve(
        sampler_axis_count
            .checked_mul(std::mem::size_of::<f64>() as u64)
            .ok_or_else(|| CoreError::ResourceLimit("sampler axis buffer size overflows".into()))?,
    )?;
    let sampler = trusted_sampler(source, geometry)?;
    if let Some(control) = control {
        control.check()?;
    }
    let projection = AeqdWgs84::new(target.center)
        .map_err(|_| CoreError::Storage("local projection center is invalid".into()))?;
    let [height, width] = target.grid.shape.as_slice() else {
        return Err(CoreError::Storage("local target grid must be two-dimensional".into()));
    };
    let pixels = height
        .checked_mul(*width)
        .ok_or_else(|| CoreError::ResourceLimit("local target grid dimensions overflow".into()))?;
    let pixels_u64 = u64::try_from(pixels)
        .map_err(|_| CoreError::ResourceLimit("local target grid dimensions overflow".into()))?;
    let axis_count = u64::try_from(target.grid.x.len().saturating_add(target.grid.y.len()))
        .map_err(|_| CoreError::ResourceLimit("local target axes overflow".into()))?;
    let output_bytes = pixels_u64
        .checked_mul(
            (std::mem::size_of::<f32>() + std::mem::size_of::<u16>() + std::mem::size_of::<bool>())
                as u64,
        )
        .and_then(|bytes| {
            axis_count
                .checked_mul(2 * std::mem::size_of::<f64>() as u64)
                .and_then(|axes| bytes.checked_add(axes))
        })
        .ok_or_else(|| CoreError::ResourceLimit("local output buffer size overflows".into()))?;
    let output_lease = memory.reserve(output_bytes)?;
    let mut values = Vec::new();
    let mut quality = Vec::new();
    let mut source_domain = Vec::new();
    values
        .try_reserve_exact(pixels)
        .map_err(|_| CoreError::ResourceLimit("local target values allocation failed".into()))?;
    quality
        .try_reserve_exact(pixels)
        .map_err(|_| CoreError::ResourceLimit("local target quality allocation failed".into()))?;
    source_domain
        .try_reserve_exact(pixels)
        .map_err(|_| CoreError::ResourceLimit("local target mask allocation failed".into()))?;

    let width = *width;
    for (row, &northing) in target.grid.y.iter().enumerate() {
        if let Some(control) = control {
            control.check()?;
        }
        for (column, &easting) in target.grid.x.iter().enumerate() {
            let index = row * width + column;
            let sample = if target.in_view[index] {
                let point = projection.inverse(easting, northing).map_err(map_projection_error)?;
                match wgs84_to_source_grid(
                    source.grid.crs.as_deref(),
                    point.longitude,
                    point.latitude,
                    sampler.x_axis(),
                ) {
                    Some((x, y)) => sampler.sample_local(x, y, resampling),
                    None => sampler.sample_local(f64::NAN, f64::NAN, resampling),
                }
            } else {
                sampler.sample_local(f64::NAN, f64::NAN, resampling)
            };
            values.push(sample.value);
            quality.push(sample.quality);
            source_domain.push(sample.in_source_domain);
        }
    }
    let center_index = (height / 2)
        .checked_mul(width)
        .and_then(|index| index.checked_add(width / 2))
        .ok_or_else(|| CoreError::ResourceLimit("local center index overflow".into()))?;
    if !source_domain.get(center_index).copied().unwrap_or(false) {
        return Err(CoreError::Storage("user location is outside the current frame domain".into()));
    }
    target.source_domain = Some(source_domain);
    let mut provenance = source.provenance.clone();
    provenance.push(format!(
        "operation=local_view,projection=local-aeqd-wgs84-v1,resampling={}",
        resampling.as_str()
    ));
    let field = RadarField {
        name: source.name.clone(),
        values,
        shape: target.grid.shape.clone(),
        quality,
        units: source.units.clone(),
        valid_time: source.valid_time.clone(),
        grid: target.grid.clone(),
        provenance,
    };
    field.validate().map_err(|_| CoreError::Storage("projected local field is invalid".into()))?;
    target.validate().map_err(|_| CoreError::Storage("projected local grid is invalid".into()))?;
    drop(sampler);
    drop(sampler_lease);
    if let Some(control) = control {
        control.check()?;
    }
    Ok((ProjectedField { grid: target, field }, output_lease))
}

fn trusted_sampler<'a>(
    source: &'a RadarField,
    geometry: &RasterGeometryEvidence,
) -> CoreResult<GridSampler<'a>> {
    if geometry.status != SpatialEvidenceStatus::Verified || geometry.evidence.is_empty() {
        return Err(CoreError::Storage(
            "local projection requires verified source geometry evidence".into(),
        ));
    }
    let Some(geometry_crs) = geometry.crs.as_deref() else {
        return Err(CoreError::Storage("verified source geometry has no CRS".into()));
    };
    if !supports_regrid_crs(Some(geometry_crs), Some("EPSG:4326")) {
        return Err(CoreError::Storage("local projection does not support this source CRS".into()));
    }
    if !same_crs(source.grid.crs.as_deref(), Some(geometry_crs)) {
        return Err(CoreError::Storage("field CRS conflicts with its geometry evidence".into()));
    }
    let [height, width] = source.shape.as_slice() else {
        return Err(CoreError::Storage("local projection requires a two-dimensional field".into()));
    };
    let shape = [
        u32::try_from(*height).map_err(|_| {
            CoreError::ResourceLimit("source height exceeds catalog geometry range".into())
        })?,
        u32::try_from(*width).map_err(|_| {
            CoreError::ResourceLimit("source width exceeds catalog geometry range".into())
        })?,
    ];
    if geometry.shape != Some(shape) {
        return Err(CoreError::Storage("field shape conflicts with its geometry evidence".into()));
    }
    if geometry.data_window.is_some_and(|window| window != [0, 0, shape[1], shape[0]]) {
        return Err(CoreError::Storage(
            "partial data windows are not supported for numeric fields".into(),
        ));
    }
    if geometry.affine.is_some_and(|evidence| {
        source.grid.affine.is_some_and(|field| !affines_match(evidence, field))
    }) {
        return Err(CoreError::Storage("field affine conflicts with its geometry evidence".into()));
    }
    match geometry.registration {
        PixelRegistration::Center => {
            let x_axis = if epsg_code(Some(geometry_crs)) == Some(4326) {
                continuous_longitude_axis(&source.grid.x).ok_or_else(|| {
                    CoreError::Storage("geographic source longitude axis is not continuous".into())
                })?
            } else {
                source.grid.x.clone()
            };
            GridSampler::pixel_centers_with_x_axis(source, x_axis)
        }
        PixelRegistration::Edge => {
            let affine = geometry.affine.ok_or_else(|| {
                CoreError::Storage("edge registration requires an affine transform".into())
            })?;
            if affine[2] != 0.0 || affine[4] != 0.0 || affine[1] == 0.0 || affine[5] == 0.0 {
                return Err(CoreError::Storage(
                    "rotated or degenerate source grids are not supported".into(),
                ));
            }
            let x_edges = affine_axis_edges(affine[0], affine[1], *width)?;
            let y_edges = affine_axis_edges(affine[3], affine[5], *height)?;
            GridSampler::pixel_edges(source, &x_edges, &y_edges)
        }
        PixelRegistration::Unknown => {
            Err(CoreError::Storage("source pixel registration is unverified".into()))
        }
    }
}

fn affine_axis_edges(origin: f64, step: f64, count: usize) -> CoreResult<Vec<f64>> {
    let edge_count = count
        .checked_add(1)
        .ok_or_else(|| CoreError::ResourceLimit("source edge count overflows".into()))?;
    let mut edges = Vec::new();
    edges
        .try_reserve_exact(edge_count)
        .map_err(|_| CoreError::ResourceLimit("source edge allocation failed".into()))?;
    for index in 0..edge_count {
        let coordinate = origin + index as f64 * step;
        if !coordinate.is_finite() {
            return Err(CoreError::Storage("source affine produces a non-finite edge".into()));
        }
        edges.push(coordinate);
    }
    Ok(edges)
}

fn affines_match(left: [f64; 6], right: [f64; 6]) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| (left - right).abs() <= 1e-9 * left.abs().max(right.abs()).max(1.0))
}

fn map_projection_error(error: ProjectionError) -> CoreError {
    CoreError::Storage(format!("local projection failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GeographicPoint, Grid};
    use crate::projection::build_local_view_grid;
    use crate::source::catalog::{
        CatalogProvenance, ObservationCoverage, SpatialAdmission, SpatialDomain,
    };
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    fn spatial_target(
        source: &str,
        station: Option<&str>,
        source_class: SpatialSourceClass,
    ) -> CatalogSpatialTarget {
        CatalogSpatialTarget {
            target: DiscoveryTarget {
                source: source.into(),
                product: Some(if matches!(source, "rainviewer" | "sg") {
                    "composite".into()
                } else {
                    "reflectivity".into()
                }),
                station: station.map(str::to_owned),
            },
            kind: if station.is_some() {
                SpatialTargetKind::PhysicalStation
            } else {
                SpatialTargetKind::ProductSelector
            },
            source_class,
            availability: "available".into(),
            required_extras: vec![],
            site: None,
            site_status: SpatialEvidenceStatus::Unknown,
            product_domain: None,
            observation_coverage: None,
            geometry: None,
            admission: None,
        }
    }

    fn verified_admission(purposes: &[&str]) -> SpatialAdmission {
        SpatialAdmission {
            status: SpatialEvidenceStatus::Verified,
            automatic: true,
            purposes: purposes.iter().map(|purpose| (*purpose).into()).collect(),
            checked_at: Some("2026-10-10".into()),
            evidence: vec![CatalogProvenance {
                source: "controlled fixture".into(),
                reference: Some("fixture".into()),
                observed_at: Some("2026-10-10".into()),
            }],
        }
    }

    fn local_request(source: Option<&str>) -> LocalViewRequest {
        LocalViewRequest { source: source.map(str::to_owned), ..LocalViewRequest::default() }
    }

    fn selection_policy() -> LocalViewSelectionPolicy {
        LocalViewSelectionPolicy { allow_network: true, ..LocalViewSelectionPolicy::default() }
    }

    fn evidence(registration: PixelRegistration) -> RasterGeometryEvidence {
        RasterGeometryEvidence {
            status: SpatialEvidenceStatus::Verified,
            crs: Some("EPSG:4326".into()),
            registration,
            shape: Some([2, 2]),
            affine: None,
            data_window: None,
            artifact_sha256: None,
            evidence: vec![CatalogProvenance {
                source: "controlled fixture".into(),
                reference: None,
                observed_at: None,
            }],
        }
    }

    fn field(x: Vec<f64>, y: Vec<f64>) -> RadarField {
        RadarField {
            name: "reflectivity".into(),
            values: vec![1.0, 2.0, 3.0, 4.0],
            shape: vec![2, 2],
            quality: vec![8, 16, 32, 64],
            units: Some("dBZ".into()),
            valid_time: "2026-10-10T00:00:00Z".into(),
            grid: Grid { shape: vec![2, 2], crs: Some("EPSG:4326".into()), x, y, affine: None },
            provenance: vec!["source=fixture".into()],
        }
    }

    #[test]
    fn local_view_projection_preserves_values_quality_and_separate_masks() {
        let source = field(vec![-0.005, 0.005], vec![0.005, -0.005]);
        let target = build_local_view_grid(
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            1.0,
            1000.0,
            100,
        )
        .unwrap();
        let projected =
            project_field_nearest(&source, &evidence(PixelRegistration::Center), target).unwrap();
        assert_eq!(projected.field.shape, [3, 3]);
        assert!(projected.grid.in_view[4]);
        assert!(projected.grid.source_domain.as_ref().unwrap()[4]);
        assert_eq!(projected.field.values[4], 3.0);
        assert_eq!(projected.field.quality[4], 32);
        assert!(!projected.grid.in_view[0]);
        assert!(projected.field.values[0].is_nan());
        assert_eq!(projected.field.quality[0], 3);
    }

    #[test]
    fn local_view_bilinear_resamples_dbz_in_z_domain_and_preserves_quality_flags() {
        let mut source = field(vec![-0.005, 0.005], vec![0.005, -0.005]);
        source.values = vec![0.0, 10.0, 20.0, 30.0];
        source.quality = vec![8, 0, 0, 0];
        let target = build_local_view_grid(
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            1.0,
            1000.0,
            100,
        )
        .unwrap();

        let projected = project_field_with_resampling(
            &source,
            &evidence(PixelRegistration::Center),
            target,
            Resampling::Bilinear,
        )
        .unwrap();
        let center = (projected.field.shape[0] / 2) * projected.field.shape[1]
            + projected.field.shape[1] / 2;
        let expected_dbz = 10.0 * ((1.0 + 10.0 + 100.0 + 1000.0) / 4.0_f64).log10();
        assert!((f64::from(projected.field.values[center]) - expected_dbz).abs() < 1e-4);
        assert_eq!(projected.field.quality[center], 8 | 16);
        assert!(
            projected.field.provenance.iter().any(|item| item.ends_with("resampling=bilinear"))
        );

        let mut category = source;
        category.name = "category".into();
        let target = build_local_view_grid(
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            1.0,
            1000.0,
            100,
        )
        .unwrap();
        assert!(
            project_field_with_resampling(
                &category,
                &evidence(PixelRegistration::Center),
                target,
                Resampling::Bilinear,
            )
            .is_err()
        );
    }

    #[test]
    fn local_view_projection_accounts_buffers_until_result_owners_drop() {
        let memory = RasterMemoryBudget::new(10_000);
        let control = RasterWorkControl::new(
            tokio_util::sync::CancellationToken::new(),
            std::time::Instant::now() + std::time::Duration::from_secs(10),
        );
        let (target, grid_lease) = crate::projection::build_local_view_grid_with_budget(
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            0.01,
            5.0,
            100,
            &memory,
            &control,
        )
        .unwrap();
        assert_eq!(grid_lease.bytes(), 5 * 5 + 5 * 2 * std::mem::size_of::<f64>() as u64);
        assert_eq!(memory.reserved_bytes(), grid_lease.bytes());

        let (projected, output_lease) = project_field_nearest_with_budget(
            &field(vec![-0.005, 0.005], vec![0.005, -0.005]),
            &evidence(PixelRegistration::Center),
            target,
            &memory,
            &control,
        )
        .unwrap();
        assert_eq!(memory.reserved_bytes(), grid_lease.bytes() + output_lease.bytes());
        drop(projected);
        assert_eq!(memory.reserved_bytes(), grid_lease.bytes() + output_lease.bytes());
        drop(output_lease);
        assert_eq!(memory.reserved_bytes(), grid_lease.bytes());
        drop(grid_lease);
        assert_eq!(memory.reserved_bytes(), 0);
    }

    #[test]
    fn local_view_projection_rejects_unverified_or_unsupported_geometry() {
        let source = field(vec![-0.005, 0.005], vec![0.005, -0.005]);
        let target = build_local_view_grid(
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            1.0,
            1000.0,
            100,
        )
        .unwrap();
        let mut unverified = evidence(PixelRegistration::Center);
        unverified.status = SpatialEvidenceStatus::Unknown;
        assert!(project_field_nearest(&source, &unverified, target.clone()).is_err());

        let mut unsupported = evidence(PixelRegistration::Center);
        unsupported.crs = Some("EPSG:3821".into());
        assert!(project_field_nearest(&source, &unsupported, target).is_err());
    }

    #[test]
    fn local_view_projection_reads_affine_edge_registered_fields() {
        let source = RadarField {
            grid: Grid {
                shape: vec![2, 2],
                crs: Some("EPSG:4326".into()),
                x: vec![],
                y: vec![],
                affine: None,
            },
            ..field(vec![], vec![])
        };
        let mut edge_evidence = evidence(PixelRegistration::Edge);
        edge_evidence.affine = Some([-0.01, 0.01, 0.0, 0.01, 0.0, -0.01]);
        let target = build_local_view_grid(
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            1.0,
            1000.0,
            100,
        )
        .unwrap();
        let projected = project_field_nearest(&source, &edge_evidence, target).unwrap();
        assert!(projected.grid.source_domain.as_ref().unwrap()[4]);
        assert_eq!(projected.field.values[4], 3.0);
    }

    #[test]
    fn local_view_product_domain_bbox_handles_the_date_line() {
        let domain = SpatialDomain {
            status: SpatialEvidenceStatus::Verified,
            crs: Some("EPSG:4326".into()),
            geometry: Some(serde_json::json!({"bbox": [170.0, -10.0, -170.0, 10.0]})),
            evidence: vec![CatalogProvenance {
                source: "controlled dateline fixture".into(),
                reference: None,
                observed_at: None,
            }],
        };
        assert_eq!(
            domain_contains(&domain, GeographicPoint { longitude: 179.0, latitude: 0.0 }),
            Some(true)
        );
        assert_eq!(
            domain_contains(&domain, GeographicPoint { longitude: -179.0, latitude: 0.0 }),
            Some(true)
        );
        assert_eq!(
            domain_contains(&domain, GeographicPoint { longitude: 0.0, latitude: 0.0 }),
            Some(false)
        );
    }

    #[test]
    fn local_view_selection_requires_evidence_for_automatic_admission() {
        let mut missing = spatial_target("rdcap", None, SpatialSourceClass::Official);
        missing.admission = None;
        let mut wrong_purpose = spatial_target("rainviewer", None, SpatialSourceClass::ThirdParty);
        wrong_purpose.admission = Some(verified_admission(&["other"]));
        let mut admitted = spatial_target("rdcap", Some("TW01"), SpatialSourceClass::Official);
        admitted.admission = Some(verified_admission(&["local_view"]));
        admitted.site = Some(GeographicPoint { longitude: 0.01, latitude: 0.0 });
        admitted.site_status = SpatialEvidenceStatus::Verified;

        let result = select_candidates(
            vec![missing, wrong_purpose, admitted],
            &local_request(None),
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            &selection_policy(),
        );

        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].spatial.target.station.as_deref(), Some("TW01"));
        assert!(result.candidates[0].site_distance_m.unwrap() > 0.0);
        assert_eq!(result.excluded.len(), 2);
        assert!(result.excluded.iter().all(|attempt| attempt.reason == "admission_unverified"));
    }

    #[test]
    fn local_view_selection_explicit_source_bypasses_admission_but_not_scope_or_domain() {
        let mut inside = spatial_target("rdcap", Some("inside"), SpatialSourceClass::Official);
        inside.product_domain = Some(SpatialDomain {
            status: SpatialEvidenceStatus::Verified,
            crs: Some("EPSG:4326".into()),
            geometry: Some(serde_json::json!({ "bbox": [-1.0, -1.0, 1.0, 1.0] })),
            evidence: vec![CatalogProvenance {
                source: "controlled fixture".into(),
                reference: None,
                observed_at: None,
            }],
        });
        let mut outside = spatial_target("rdcap", Some("outside"), SpatialSourceClass::Official);
        outside.observation_coverage = Some(ObservationCoverage {
            kind: ObservationCoverageKind::GeodesicCircle,
            status: SpatialEvidenceStatus::Verified,
            geometry: Some(serde_json::json!({
                "longitude": 10.0,
                "latitude": 10.0,
                "radius_m": 1000.0
            })),
            evidence: vec![CatalogProvenance {
                source: "controlled fixture".into(),
                reference: None,
                observed_at: None,
            }],
        });
        let unrelated = spatial_target("rainviewer", None, SpatialSourceClass::ThirdParty);

        let result = select_candidates(
            vec![outside, unrelated, inside],
            &local_request(Some("rdcap")),
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            &selection_policy(),
        );

        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].spatial.target.station.as_deref(), Some("inside"));
        assert_eq!(result.candidates[0].relation, SpatialRelation::InsideProductDomain);
        assert_eq!(result.excluded.len(), 1);
        assert_eq!(result.excluded[0].target.station.as_deref(), Some("outside"));
        assert_eq!(result.excluded[0].reason, "outside_domain");
    }

    #[test]
    fn local_view_selection_orders_evidence_source_class_distance_and_stable_key() {
        let center = GeographicPoint { longitude: 0.0, latitude: 0.0 };
        let mut near = spatial_target("rdcap", Some("near"), SpatialSourceClass::Official);
        near.site = Some(GeographicPoint { longitude: 0.01, latitude: 0.0 });
        near.site_status = SpatialEvidenceStatus::Verified;
        let mut far = spatial_target("rdcap", Some("far"), SpatialSourceClass::Official);
        far.site = Some(GeographicPoint { longitude: 0.03, latitude: 0.0 });
        far.site_status = SpatialEvidenceStatus::Verified;
        let mut unknown_site =
            spatial_target("rdcap", Some("unknown"), SpatialSourceClass::Official);
        unknown_site.site = Some(GeographicPoint { longitude: 0.001, latitude: 0.0 });
        unknown_site.site_status = SpatialEvidenceStatus::Declared;
        let third_party = spatial_target("rainviewer", None, SpatialSourceClass::ThirdParty);
        let mut needs_probe = spatial_target("rdcap", Some("probe"), SpatialSourceClass::Official);
        needs_probe.product_domain = Some(SpatialDomain {
            status: SpatialEvidenceStatus::Unknown,
            crs: None,
            geometry: None,
            evidence: vec![],
        });

        let result = select_candidates(
            vec![third_party, unknown_site, far, needs_probe, near.clone()],
            &local_request(Some("rdcap")),
            center,
            &selection_policy(),
        );

        let ordered = result
            .candidates
            .iter()
            .map(|candidate| candidate.spatial.target.station.as_deref().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ordered, ["near", "far", "probe", "unknown"]);
        assert!(result.candidates[0].site_distance_m < result.candidates[1].site_distance_m);
        assert_eq!(result.candidates[2].site_distance_m, None);
        assert_eq!(result.candidates[3].site_distance_m, None);

        let mut coverage = spatial_target("rdcap", Some("coverage"), SpatialSourceClass::Official);
        coverage.observation_coverage = Some(ObservationCoverage {
            kind: ObservationCoverageKind::GeodesicCircle,
            status: SpatialEvidenceStatus::Verified,
            geometry: Some(serde_json::json!({
                "longitude": 0.0,
                "latitude": 0.0,
                "radius_m": 1000.0
            })),
            evidence: vec![CatalogProvenance {
                source: "controlled fixture".into(),
                reference: None,
                observed_at: None,
            }],
        });
        let result = select_candidates(
            vec![near, coverage],
            &local_request(Some("rdcap")),
            center,
            &selection_policy(),
        );
        assert_eq!(result.candidates[0].spatial.target.station.as_deref(), Some("coverage"));
        assert_eq!(result.candidates[0].relation, SpatialRelation::InsideVerifiedCoverage);
    }

    #[test]
    fn local_view_selection_explains_access_and_mode_exclusions() {
        let mut retired = spatial_target("rdcap", Some("retired"), SpatialSourceClass::Official);
        retired.availability = "retired".into();
        let network = spatial_target("rdcap", Some("network"), SpatialSourceClass::Official);
        let mut request = local_request(Some("rdcap"));
        request.mode = LocalViewMode::Raw;
        let mut policy = selection_policy();
        policy.allow_network = false;

        let result = select_candidates(
            vec![retired, network.clone()],
            &request,
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            &policy,
        );

        assert!(result.candidates.is_empty());
        assert_eq!(result.excluded.len(), 2);
        assert!(result.excluded.iter().any(|attempt| attempt.reason == "source_retired"));
        assert!(result.excluded.iter().any(|attempt| attempt.reason == "network_restricted"));

        let mut bilinear = local_request(Some("rdcap"));
        bilinear.resampling = LocalViewResampling::Bilinear;
        let supported = select_candidates(
            vec![network.clone()],
            &bilinear,
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            &selection_policy(),
        );
        assert_eq!(supported.candidates.len(), 1);

        let unsupported_target = spatial_target("au", None, SpatialSourceClass::Official);
        let mut unsupported_request = local_request(Some("au"));
        unsupported_request.resampling = LocalViewResampling::Bilinear;
        let unsupported = select_candidates(
            vec![unsupported_target.clone()],
            &unsupported_request,
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            &selection_policy(),
        );
        assert!(unsupported.candidates.is_empty());
        assert_eq!(unsupported.excluded[0].reason, "unsupported_mode_or_geometry");

        let categorical = spatial_target("sg", None, SpatialSourceClass::Official);
        let mut categorical_request = local_request(Some("sg"));
        categorical_request.resampling = LocalViewResampling::Bilinear;
        let excluded = select_candidates(
            vec![categorical],
            &categorical_request,
            GeographicPoint { longitude: 0.0, latitude: 0.0 },
            &selection_policy(),
        );
        assert!(excluded.candidates.is_empty());
        assert_eq!(excluded.excluded[0].reason, "unsupported_mode_or_geometry");
    }

    #[test]
    fn local_view_raw_image_uses_edge_registered_nearest_and_keeps_source_alpha() {
        let width = 2;
        let height = 2;
        let affine =
            crate::grid::web_mercator_affine_from_bounds([-0.01, -0.01, 0.01, 0.01], width, height)
                .unwrap();
        let geometry = RasterGeometryEvidence {
            status: SpatialEvidenceStatus::Verified,
            crs: Some("EPSG:3857".into()),
            registration: PixelRegistration::Edge,
            shape: Some([height, width]),
            affine: Some(affine),
            data_window: Some([0, 0, width, height]),
            artifact_sha256: Some("a".repeat(64)),
            evidence: vec![CatalogProvenance {
                source: "controlled image overlay".into(),
                reference: Some("unit-test".into()),
                observed_at: Some("2026-10-10".into()),
            }],
        };
        let source = Preview {
            width,
            height,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 0, 0, 0, 255, 255, 255, 255, 0, 255],
            frame: None,
            mode: crate::model::PreviewMode::Raw,
            rule_version: None,
        };
        // Keep the control inside the northwest pixel: the four-pixel junction
        // is sensitive to platform-dependent Mercator rounding at latitude zero.
        let center = GeographicPoint { longitude: -0.001, latitude: 0.001 };
        let target = build_local_view_grid(center, 2.0, 1000.0, 100).unwrap();
        let control = RasterWorkControl::new(
            CancellationToken::new(),
            (Instant::now() + Duration::from_secs(2)).into(),
        );
        let (projected, rgba, _lease) = project_raw_image_nearest(
            &source,
            &geometry,
            target,
            &Limits::default(),
            &RasterMemoryBudget::new(10_000),
            &control,
        )
        .unwrap();
        assert_eq!(projected.grid.shape, [5, 5]);
        assert_eq!(&rgba[(2 * 5 + 2) * 4..(2 * 5 + 2) * 4 + 4], &[255, 0, 0, 255]);
        assert!(projected.source_domain.as_ref().unwrap()[2 * 5 + 3]);
        assert_eq!(&rgba[(2 * 5 + 3) * 4..(2 * 5 + 3) * 4 + 4], &[0, 255, 0, 0]);
        assert!(!projected.in_view[0]);
        assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);

        let mut partial = geometry;
        partial.data_window = Some([1, 0, 1, 2]);
        assert!(
            project_raw_image_nearest(
                &source,
                &partial,
                build_local_view_grid(center, 2.0, 1000.0, 100).unwrap(),
                &Limits::default(),
                &RasterMemoryBudget::new(10_000),
                &control,
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod spatial_fallback_tests {
    use super::*;

    #[test]
    fn local_view_polygon_excludes_holes_and_keeps_unknown_geometry_unknown() {
        let polygon = serde_json::json!({"type": "Polygon", "coordinates": [
            [[-2.0,-2.0],[2.0,-2.0],[2.0,2.0],[-2.0,2.0],[-2.0,-2.0]],
            [[-0.5,-0.5],[0.5,-0.5],[0.5,0.5],[-0.5,0.5],[-0.5,-0.5]]
        ]});
        assert_eq!(
            geojson_polygon_contains(&polygon, GeographicPoint { longitude: 0.0, latitude: 0.0 }),
            Some(false)
        );
        assert_eq!(
            geojson_polygon_contains(&polygon, GeographicPoint { longitude: 1.0, latitude: 0.0 }),
            Some(true)
        );
        assert_eq!(
            geojson_polygon_contains(&polygon, GeographicPoint { longitude: 3.0, latitude: 0.0 }),
            Some(false)
        );
        let mut malformed = polygon;
        malformed["coordinates"][1] = serde_json::json!([[0, 0], [1, 0], [1, 1], [0, 1]]);
        assert_eq!(
            geojson_polygon_contains(&malformed, GeographicPoint { longitude: 1.0, latitude: 0.0 }),
            None
        );
    }

    #[test]
    fn local_view_unknown_verified_coverage_falls_through_to_domain_and_probe() {
        let catalog = crate::source::catalog::SourceCatalog::builtin().unwrap();
        let mut target = catalog.spatial_targets(Some(&["rainviewer".into()])).unwrap().remove(0);
        target.observation_coverage = Some(crate::source::catalog::ObservationCoverage {
            kind: ObservationCoverageKind::Mask,
            status: SpatialEvidenceStatus::Verified,
            geometry: None,
            evidence: vec![],
        });
        target.product_domain = None;
        let center = GeographicPoint { longitude: 0.0, latitude: 0.0 };
        assert_eq!(spatial_relation(&target, center), Some(SpatialRelation::NeedsProbe));
        target.product_domain = Some(SpatialDomain {
            status: SpatialEvidenceStatus::Verified,
            crs: Some("EPSG:4326".into()),
            geometry: Some(serde_json::json!({"bbox": [-1.0,-1.0,1.0,1.0]})),
            evidence: vec![crate::source::catalog::CatalogProvenance {
                source: "controlled fixture".into(),
                reference: None,
                observed_at: None,
            }],
        });
        assert_eq!(spatial_relation(&target, center), Some(SpatialRelation::InsideProductDomain));
        assert_eq!(
            spatial_relation(&target, GeographicPoint { longitude: 2.0, latitude: 0.0 }),
            Some(SpatialRelation::Outside)
        );
    }
}
