//! Static built-in source catalog and deterministic target expansion.

use crate::model::{DiscoveryTarget, GeographicPoint};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const BUILTIN_CATALOG: &str = include_str!("../../resources/builtin/catalog.json");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CatalogProduct {
    pub id: String,
    #[serde(default)]
    pub variables: Vec<String>,
    #[serde(default)]
    pub units: serde_json::Value,
    #[serde(default = "geographic_grid_kind")]
    pub native_grid_kind: String,
    #[serde(default)]
    pub cadence_seconds: Option<f64>,
    #[serde(default)]
    pub publication_delay_seconds: Option<f64>,
    #[serde(default)]
    pub suggested_max_age_seconds: Option<f64>,
    #[serde(default)]
    pub historical: bool,
    #[serde(default)]
    pub forecast: bool,
    #[serde(default)]
    pub default: bool,
    #[serde(default = "valid_time_binding_policy")]
    pub time_binding_policy: String,
    #[serde(default)]
    pub mutable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<CatalogMetadata>,
}

fn geographic_grid_kind() -> String {
    "geographic".to_owned()
}

fn valid_time_binding_policy() -> String {
    "valid_time".to_owned()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CatalogStation {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub longitude: Option<f64>,
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub product_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<CatalogMetadata>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CatalogSource {
    pub id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "available")]
    pub availability: String,
    #[serde(default)]
    pub availability_evidence: Option<String>,
    #[serde(default)]
    pub adapter_version: Option<String>,
    #[serde(default)]
    pub required_extras: Vec<String>,
    #[serde(default)]
    pub products: Vec<CatalogProduct>,
    #[serde(default)]
    pub stations: Vec<CatalogStation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<CatalogMetadata>,
}

/// Additive metadata shared by source, product, station, and live catalog
/// updates. Unknown extension keys round-trip without changing catalog v1.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CatalogMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_query: Option<RecentQueryCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub directory_conflicts: Vec<CatalogDirectoryConflict>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<CatalogProvenance>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub country_capabilities: BTreeMap<String, CountryCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial: Option<CatalogSpatialMetadata>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialTargetKind {
    PhysicalStation,
    RegionSelector,
    ProductSelector,
    Composite,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialEvidenceStatus {
    Verified,
    Declared,
    Unverified,
    Conflicting,
    Unsupported,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialSourceClass {
    Official,
    ThirdParty,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CatalogSpatialMetadata {
    #[serde(default)]
    pub schema_version: u8,
    #[serde(default)]
    pub kind: SpatialTargetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinate_reference: Option<String>,
    #[serde(default)]
    pub source_class: SpatialSourceClass,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_class_evidence: Vec<CatalogProvenance>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub field_evidence: BTreeMap<String, Vec<SpatialFieldEvidence>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_catalog_refs: Vec<ExternalCatalogReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_range_claim: Option<ExternalRangeClaim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_domain: Option<SpatialDomain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_coverage: Option<ObservationCoverage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<RasterGeometryEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission: Option<SpatialAdmission>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpatialFieldEvidence {
    pub status: SpatialEvidenceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_values: Vec<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCatalogRelation {
    SameSite,
    Colocated,
    Successor,
    Conflicting,
    #[default]
    Candidate,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExternalCatalogReference {
    pub catalog: String,
    pub snapshot_sha256: String,
    pub row: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub relation: ExternalCatalogRelation,
    #[serde(default)]
    pub status: SpatialEvidenceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_km: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_note: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExternalRangeClaim {
    pub source: String,
    pub value: f64,
    pub unit: String,
    #[serde(default)]
    pub status: SpatialEvidenceStatus,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpatialDomain {
    pub status: SpatialEvidenceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<CatalogProvenance>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationCoverageKind {
    GeodesicCircle,
    Polygon,
    Mask,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ObservationCoverage {
    #[serde(default)]
    pub kind: ObservationCoverageKind,
    pub status: SpatialEvidenceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<CatalogProvenance>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelRegistration {
    Center,
    Edge,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RasterGeometryEvidence {
    pub status: SpatialEvidenceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crs: Option<String>,
    #[serde(default)]
    pub registration: PixelRegistration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affine: Option<[f64; 6]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_window: Option<[u32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<CatalogProvenance>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpatialAdmission {
    pub status: SpatialEvidenceStatus,
    #[serde(default)]
    pub automatic: bool,
    #[serde(default)]
    pub purposes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<CatalogProvenance>,
}

impl CatalogSpatialMetadata {
    fn validate(&self) -> bool {
        self.schema_version == 1
            && self.coordinate_reference.as_ref().is_none_or(|crs| !crs.trim().is_empty())
            && self.field_evidence.keys().all(|field| !field.trim().is_empty())
            && (self.source_class == SpatialSourceClass::Unknown
                || !self.source_class_evidence.is_empty())
            && self.external_catalog_refs.iter().all(|reference| {
                !reference.catalog.trim().is_empty()
                    && reference.row > 0
                    && reference.snapshot_sha256.len() == 64
                    && reference.snapshot_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && reference
                        .distance_km
                        .is_none_or(|distance| distance.is_finite() && distance >= 0.0)
            })
            && self.external_range_claim.as_ref().is_none_or(|claim| {
                !claim.source.trim().is_empty()
                    && !claim.unit.trim().is_empty()
                    && claim.value.is_finite()
                    && claim.value > 0.0
            })
            && self.product_domain.as_ref().is_none_or(|domain| {
                domain.crs.as_ref().is_none_or(|crs| !crs.trim().is_empty())
                    && (domain.status != SpatialEvidenceStatus::Verified
                        || !domain.evidence.is_empty())
            })
            && self.observation_coverage.as_ref().is_none_or(|coverage| {
                coverage.status != SpatialEvidenceStatus::Verified || !coverage.evidence.is_empty()
            })
            && self.geometry.as_ref().is_none_or(|geometry| {
                geometry.crs.as_ref().is_none_or(|crs| !crs.trim().is_empty())
                    && (geometry.status != SpatialEvidenceStatus::Verified
                        || !geometry.evidence.is_empty())
                    && geometry
                        .affine
                        .is_none_or(|affine| affine.iter().all(|value| value.is_finite()))
                    && geometry.shape.is_none_or(|shape| shape[0] > 0 && shape[1] > 0)
                    && geometry.data_window.is_none_or(|window| window[2] > 0 && window[3] > 0)
                    && geometry.artifact_sha256.as_ref().is_none_or(|hash| {
                        hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            })
            && self.admission.as_ref().is_none_or(|admission| {
                !admission.automatic
                    || (admission.status == SpatialEvidenceStatus::Verified
                        && !admission.evidence.is_empty()
                        && admission.purposes.iter().any(|purpose| purpose == "local_view"))
            })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RecentQueryCapability {
    #[serde(default)]
    pub latest: bool,
    #[serde(default)]
    pub exact_at: bool,
    #[serde(default)]
    pub range: bool,
    #[serde(default)]
    pub historical: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CatalogDirectoryConflict {
    pub station_id: String,
    pub field: String,
    #[serde(default)]
    pub values: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<CatalogProvenance>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CatalogProvenance {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogCapabilityStatus {
    Verified,
    Blocked,
    Unsupported,
    #[default]
    Unverified,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CountryCapabilities {
    #[serde(default)]
    pub discovery: CatalogCapabilityStatus,
    #[serde(default)]
    pub raw_acquisition: CatalogCapabilityStatus,
    #[serde(default)]
    pub science: CatalogCapabilityStatus,
    #[serde(default)]
    pub readback: CatalogCapabilityStatus,
}

/// A provider's one-call dynamic station directory response. The source id
/// makes the update's namespace explicit before Engine merges it with the
/// static schema-v1 snapshot.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StationCatalogUpdate {
    pub source_id: String,
    #[serde(default)]
    pub stations: Vec<CatalogStation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<CatalogMetadata>,
}

fn available() -> String {
    "available".to_owned()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceCatalog {
    pub schema_version: u32,
    pub sources: Vec<CatalogSource>,
}

/// Spatial read view over existing discovery targets. It never expands the
/// catalog or upgrades legacy coordinates/evidence into trusted coverage.
#[derive(Clone, Debug)]
pub struct CatalogSpatialTarget {
    pub target: DiscoveryTarget,
    pub kind: SpatialTargetKind,
    pub source_class: SpatialSourceClass,
    pub availability: String,
    pub required_extras: Vec<String>,
    pub site: Option<GeographicPoint>,
    pub site_status: SpatialEvidenceStatus,
    pub product_domain: Option<SpatialDomain>,
    pub observation_coverage: Option<ObservationCoverage>,
    pub geometry: Option<RasterGeometryEvidence>,
    pub admission: Option<SpatialAdmission>,
}

impl SourceCatalog {
    pub fn builtin() -> Result<Self, CatalogError> {
        Self::parse(BUILTIN_CATALOG)
    }

    pub fn parse(input: &str) -> Result<Self, CatalogError> {
        let catalog: Self = serde_json::from_str(input).map_err(|_| CatalogError::InvalidJson)?;
        if catalog.schema_version != 1 {
            return Err(CatalogError::UnsupportedVersion(catalog.schema_version));
        }
        let mut ids = BTreeSet::new();
        for source in &catalog.sources {
            if source.id.trim().is_empty() || !ids.insert(source.id.clone()) {
                return Err(CatalogError::InvalidSourceId);
            }
            let mut products = BTreeSet::new();
            for product in &source.products {
                if product.id.trim().is_empty() || !products.insert(product.id.clone()) {
                    return Err(CatalogError::InvalidProduct { source_id: source.id.clone() });
                }
            }
            let mut stations = BTreeSet::new();
            for station in &source.stations {
                if station.id.trim().is_empty() || !stations.insert(station.id.clone()) {
                    return Err(CatalogError::InvalidStation { source_id: source.id.clone() });
                }
            }
            let spatial_metadata = std::iter::once(source.metadata.as_ref())
                .chain(source.products.iter().map(|product| product.metadata.as_ref()))
                .chain(source.stations.iter().map(|station| station.metadata.as_ref()));
            if spatial_metadata
                .flatten()
                .filter_map(|metadata| metadata.spatial.as_ref())
                .any(|spatial| !spatial.validate())
            {
                return Err(CatalogError::InvalidSpatialMetadata { source_id: source.id.clone() });
            }
        }
        Ok(catalog)
    }

    pub fn source(&self, source_id: &str) -> Option<&CatalogSource> {
        self.sources.iter().find(|source| source.id == source_id)
    }

    pub fn default_product(&self, source_id: &str) -> Option<&str> {
        let products = &self.source(source_id)?.products;
        products
            .iter()
            .find(|product| product.default)
            .or_else(|| (products.len() == 1).then(|| &products[0]))
            .map(|product| product.id.as_str())
    }

    pub fn expand_targets(
        &self,
        source_ids: Option<&[String]>,
    ) -> Result<Vec<DiscoveryTarget>, CatalogError> {
        let selected: Vec<&CatalogSource> = match source_ids {
            Some(ids) => {
                let mut sources = Vec::with_capacity(ids.len());
                for id in ids {
                    sources.push(
                        self.source(id).ok_or_else(|| CatalogError::UnknownSource(id.clone()))?,
                    );
                }
                sources
            }
            None => self.sources.iter().collect(),
        };
        let mut targets = BTreeSet::new();
        for source in selected {
            if source.products.is_empty() {
                targets.insert(DiscoveryTarget {
                    source: source.id.clone(),
                    product: None,
                    station: None,
                });
            }
            for product in &source.products {
                let stations: Vec<&CatalogStation> = source
                    .stations
                    .iter()
                    .filter(|station| {
                        station.product_ids.is_empty()
                            || station.product_ids.iter().any(|id| id == &product.id)
                    })
                    .collect();
                if stations.is_empty() {
                    targets.insert(DiscoveryTarget {
                        source: source.id.clone(),
                        product: Some(product.id.clone()),
                        station: None,
                    });
                } else {
                    for station in stations {
                        targets.insert(DiscoveryTarget {
                            source: source.id.clone(),
                            product: Some(product.id.clone()),
                            station: Some(station.id.clone()),
                        });
                    }
                }
            }
        }
        Ok(targets.into_iter().collect())
    }

    pub fn spatial_targets(
        &self,
        source_ids: Option<&[String]>,
    ) -> Result<Vec<CatalogSpatialTarget>, CatalogError> {
        let mut targets = self
            .expand_targets(source_ids)?
            .into_iter()
            .map(|target| {
                let source = self
                    .source(&target.source)
                    .ok_or_else(|| CatalogError::UnknownSource(target.source.clone()))?;
                let product = target
                    .product
                    .as_deref()
                    .and_then(|id| source.products.iter().find(|product| product.id == id));
                let station = target
                    .station
                    .as_deref()
                    .and_then(|id| source.stations.iter().find(|station| station.id == id));
                let source_spatial =
                    source.metadata.as_ref().and_then(|metadata| metadata.spatial.as_ref());
                let product_spatial = product
                    .and_then(|product| product.metadata.as_ref())
                    .and_then(|metadata| metadata.spatial.as_ref());
                let station_spatial = station
                    .and_then(|station| station.metadata.as_ref())
                    .and_then(|metadata| metadata.spatial.as_ref());
                let site = station.and_then(|station| {
                    let point = GeographicPoint {
                        longitude: station.longitude?,
                        latitude: station.latitude?,
                    };
                    (point.longitude.is_finite()
                        && point.latitude.is_finite()
                        && (-180.0..=180.0).contains(&point.longitude)
                        && (-90.0..=90.0).contains(&point.latitude))
                    .then_some(point)
                });
                let site_status = match (station_spatial, site) {
                    (Some(spatial), Some(site)) => {
                        coordinate_evidence_status(&spatial.field_evidence, site)
                    }
                    _ => SpatialEvidenceStatus::Unknown,
                };
                Ok(CatalogSpatialTarget {
                    target,
                    kind: station_spatial
                        .or(product_spatial)
                        .or(source_spatial)
                        .map(|spatial| spatial.kind)
                        .unwrap_or_default(),
                    source_class: product_spatial
                        .map(|spatial| spatial.source_class)
                        .filter(|class| *class != SpatialSourceClass::Unknown)
                        .or_else(|| source_spatial.map(|spatial| spatial.source_class))
                        .unwrap_or_default(),
                    availability: source.availability.clone(),
                    required_extras: source.required_extras.clone(),
                    site,
                    site_status,
                    product_domain: station_spatial
                        .and_then(|spatial| spatial.product_domain.clone())
                        .or_else(|| {
                            product_spatial.and_then(|spatial| spatial.product_domain.clone())
                        })
                        .or_else(|| {
                            source_spatial.and_then(|spatial| spatial.product_domain.clone())
                        }),
                    observation_coverage: station_spatial
                        .and_then(|spatial| spatial.observation_coverage.clone())
                        .or_else(|| {
                            product_spatial.and_then(|spatial| spatial.observation_coverage.clone())
                        })
                        .or_else(|| {
                            source_spatial.and_then(|spatial| spatial.observation_coverage.clone())
                        }),
                    geometry: station_spatial
                        .and_then(|spatial| spatial.geometry.clone())
                        .or_else(|| product_spatial.and_then(|spatial| spatial.geometry.clone()))
                        .or_else(|| source_spatial.and_then(|spatial| spatial.geometry.clone())),
                    admission: station_spatial
                        .and_then(|spatial| spatial.admission.clone())
                        .or_else(|| product_spatial.and_then(|spatial| spatial.admission.clone()))
                        .or_else(|| source_spatial.and_then(|spatial| spatial.admission.clone())),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        targets.extend(crate::source::legacy_spatial_targets(self, source_ids)?);
        targets.sort_by(|left, right| left.target.cmp(&right.target));
        Ok(targets)
    }
}

pub(crate) fn coordinate_evidence_status(
    evidence: &BTreeMap<String, Vec<SpatialFieldEvidence>>,
    point: GeographicPoint,
) -> SpatialEvidenceStatus {
    let longitude = evidence.get("longitude");
    let latitude = evidence.get("latitude");
    let fields_match = |items: Option<&Vec<SpatialFieldEvidence>>, expected: f64| {
        items.is_some_and(|items| {
            !items.is_empty()
                && items.iter().all(|item| {
                    item.conflict_values.is_empty()
                        && item
                            .value
                            .as_ref()
                            .and_then(serde_json::Value::as_f64)
                            .is_some_and(|value| (value - expected).abs() <= 1e-9)
                })
        })
    };
    if [longitude, latitude].into_iter().flatten().flatten().any(|item| {
        item.status == SpatialEvidenceStatus::Conflicting || !item.conflict_values.is_empty()
    }) {
        return SpatialEvidenceStatus::Conflicting;
    }
    if [longitude, latitude]
        .into_iter()
        .flatten()
        .flatten()
        .any(|item| item.status == SpatialEvidenceStatus::Unsupported)
    {
        return SpatialEvidenceStatus::Unsupported;
    }
    if !fields_match(longitude, point.longitude) || !fields_match(latitude, point.latitude) {
        return SpatialEvidenceStatus::Unverified;
    }
    let statuses = [longitude, latitude]
        .into_iter()
        .flatten()
        .flat_map(|items| items.iter().map(|item| item.status))
        .collect::<Vec<_>>();
    if statuses.iter().all(|status| *status == SpatialEvidenceStatus::Verified) {
        SpatialEvidenceStatus::Verified
    } else if statuses.iter().all(|status| *status == SpatialEvidenceStatus::Declared) {
        SpatialEvidenceStatus::Declared
    } else if statuses.iter().any(|status| *status == SpatialEvidenceStatus::Unverified) {
        SpatialEvidenceStatus::Unverified
    } else {
        SpatialEvidenceStatus::Unknown
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CatalogError {
    #[error("source catalog is not valid JSON")]
    InvalidJson,
    #[error("unsupported source catalog version {0}")]
    UnsupportedVersion(u32),
    #[error("source IDs must be non-empty and unique")]
    InvalidSourceId,
    #[error("product IDs must be non-empty and unique for {source_id}")]
    InvalidProduct { source_id: String },
    #[error("station IDs must be non-empty and unique for {source_id}")]
    InvalidStation { source_id: String },
    #[error("spatial metadata is invalid for {source_id}")]
    InvalidSpatialMetadata { source_id: String },
    #[error("unknown source: {0}")]
    UnknownSource(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_has_twenty_five_sources_and_seventy_five_targets() {
        let catalog = SourceCatalog::builtin().unwrap();
        assert_eq!(catalog.sources.len(), 25);
        assert_eq!(catalog.expand_targets(None).unwrap().len(), 75);
        assert_eq!(catalog.source("uk").unwrap().availability, "retired");
    }

    #[test]
    fn spatial_read_view_preserves_unknown_coverage_and_verified_admission() {
        let catalog = SourceCatalog::builtin().unwrap();
        let rdcap = catalog.spatial_targets(Some(&["rdcap".into()])).unwrap();
        assert_eq!(rdcap.len(), 48);
        let chlh =
            rdcap.iter().find(|entry| entry.target.station.as_deref() == Some("TWRCHL")).unwrap();
        assert_eq!(chlh.kind, SpatialTargetKind::PhysicalStation);
        assert_eq!(chlh.site_status, SpatialEvidenceStatus::Declared);
        assert_eq!(chlh.site.unwrap().longitude, 121.620079);
        assert_eq!(chlh.admission.as_ref().unwrap().automatic, false);
        assert_eq!(
            chlh.observation_coverage.as_ref().unwrap().status,
            SpatialEvidenceStatus::Unknown
        );

        let rainviewer = catalog.spatial_targets(Some(&["rainviewer".into()])).unwrap();
        assert_eq!(rainviewer.len(), 1);
        assert_eq!(rainviewer[0].kind, SpatialTargetKind::Composite);
        assert_eq!(
            rainviewer[0].geometry.as_ref().unwrap().status,
            SpatialEvidenceStatus::Verified
        );
        assert_eq!(
            rainviewer[0].observation_coverage.as_ref().unwrap().status,
            SpatialEvidenceStatus::Unknown
        );
        assert!(rainviewer[0].admission.as_ref().unwrap().automatic);
        assert_eq!(
            rainviewer[0].admission.as_ref().unwrap().status,
            SpatialEvidenceStatus::Verified
        );
        assert_eq!(rainviewer[0].admission.as_ref().unwrap().purposes, ["local_view"]);
        assert_eq!(catalog.expand_targets(None).unwrap().len(), 75);
    }

    #[test]
    fn membership_image_does_not_inherit_scientific_or_spatial_admission() {
        let catalog = SourceCatalog::builtin().unwrap();
        let product = catalog
            .source("tw")
            .unwrap()
            .products
            .iter()
            .find(|product| product.id == "observation-wide")
            .unwrap();
        assert!(product.variables.is_empty());
        assert_eq!(product.native_grid_kind, "pixel");
        let metadata = product.metadata.as_ref().unwrap();
        assert_eq!(metadata.extensions["scientific_decode"], "blocked");
        assert!(!metadata.extensions.contains_key("gray_dbz_encoding"));

        let spatial = metadata.spatial.as_ref().unwrap();
        assert_eq!(spatial.kind, SpatialTargetKind::ProductSelector);
        assert_eq!(spatial.geometry.as_ref().unwrap().status, SpatialEvidenceStatus::Unknown);
        assert_eq!(
            spatial.observation_coverage.as_ref().unwrap().status,
            SpatialEvidenceStatus::Unknown
        );
        assert!(!spatial.admission.as_ref().unwrap().automatic);
    }

    #[test]
    fn unknown_source_is_not_silently_ignored() {
        let catalog = SourceCatalog::builtin().unwrap();
        let err = catalog.expand_targets(Some(&["no_such_source".into()])).unwrap_err();
        assert_eq!(err, CatalogError::UnknownSource("no_such_source".into()));
    }

    #[test]
    fn catalog_preserves_python_sdk_product_metadata_from_the_rust_catalog() {
        let catalog = SourceCatalog::builtin().unwrap();
        let tw_grid = catalog
            .source("tw")
            .unwrap()
            .products
            .iter()
            .find(|product| product.id == "grid")
            .unwrap();
        assert_eq!(tw_grid.native_grid_kind, "geographic");
        assert!(tw_grid.historical == false);
        assert_eq!(tw_grid.time_binding_policy, "valid_time");

        let sg = catalog
            .source("sg")
            .unwrap()
            .products
            .iter()
            .find(|product| product.id == "composite")
            .unwrap();
        assert_eq!(sg.native_grid_kind, "cartesian");
        assert_eq!(sg.variables, ["rain_intensity"]);
        assert_eq!(sg.units["rain_intensity"], "1");
    }

    #[test]
    fn legacy_catalog_v1_parses_without_metadata_and_does_not_gain_null_fields() {
        let catalog = SourceCatalog::parse(
            r#"{"schema_version":1,"sources":[{"id":"legacy","products":[{"id":"p"}],"stations":[{"id":"s"}]}]}"#,
        )
        .unwrap();
        let source = &catalog.sources[0];
        assert!(source.metadata.is_none());
        assert!(source.products[0].metadata.is_none());
        assert!(source.stations[0].metadata.is_none());

        let value = serde_json::to_value(source).unwrap();
        assert!(value.get("metadata").is_none());
        assert!(value["products"][0].get("metadata").is_none());
        assert!(value["stations"][0].get("metadata").is_none());
    }

    #[test]
    fn station_catalog_update_models_country_capabilities_conflicts_and_provenance() {
        let update: StationCatalogUpdate = serde_json::from_value(serde_json::json!({
            "source_id": "rdcap",
            "stations": [{
                "id": "TWBALE",
                "metadata": {"country": "TWN"}
            }],
            "metadata": {
                "country": "TWN",
                "recent_query": {"latest": true, "exact_at": true, "range": true},
                "directory_conflicts": [{
                    "station_id": "TWBALE",
                    "field": "status",
                    "values": ["Active", "Inactive"],
                    "provenance": [{"source": "country directory"}]
                }],
                "provenance": [{"source": "country directory", "reference": "fixture"}],
                "country_capabilities": {
                    "TWN": {"discovery": "verified", "raw_acquisition": "blocked"}
                },
                "provider_extension": {"kept": true}
            }
        }))
        .unwrap();

        let metadata = update.metadata.unwrap();
        assert_eq!(metadata.country.as_deref(), Some("TWN"));
        assert!(metadata.recent_query.unwrap().range);
        assert_eq!(metadata.directory_conflicts.len(), 1);
        assert_eq!(metadata.provenance[0].source, "country directory");
        assert_eq!(
            metadata.country_capabilities["TWN"].raw_acquisition,
            CatalogCapabilityStatus::Blocked
        );
        assert_eq!(metadata.extensions["provider_extension"]["kept"], true);
    }

    #[test]
    fn spatial_metadata_is_versioned_and_preserves_unknown_and_conflicting_evidence() {
        let input = serde_json::json!({
            "schema_version": 1,
            "sources": [{
                "id": "fixture",
                "stations": [{
                    "id": "selector",
                    "metadata": {
                        "spatial": {
                            "schema_version": 1,
                            "kind": "unknown",
                            "field_evidence": {
                                "longitude": [{
                                    "status": "conflicting",
                                    "conflict_values": [121.0, 121.5]
                                }]
                            },
                            "product_domain": {"status": "unknown"},
                            "observation_coverage": {"kind": "unknown", "status": "unknown"},
                            "geometry": {"status": "unverified", "registration": "unknown"},
                            "admission": {"status": "unverified", "automatic": false},
                            "future_spatial_field": {"retained": true}
                        }
                    }
                }]
            }]
        });
        let catalog = SourceCatalog::parse(&input.to_string()).unwrap();
        let spatial =
            catalog.sources[0].stations[0].metadata.as_ref().unwrap().spatial.as_ref().unwrap();
        assert_eq!(spatial.kind, SpatialTargetKind::Unknown);
        assert_eq!(
            spatial.field_evidence["longitude"][0].status,
            SpatialEvidenceStatus::Conflicting
        );
        assert_eq!(spatial.field_evidence["longitude"][0].conflict_values.len(), 2);
        assert_eq!(spatial.geometry.as_ref().unwrap().registration, PixelRegistration::Unknown);
        assert_eq!(spatial.extensions["future_spatial_field"]["retained"], true);

        let roundtrip = serde_json::to_value(&catalog).unwrap();
        assert_eq!(
            roundtrip["sources"][0]["stations"][0]["metadata"]["spatial"]["future_spatial_field"]["retained"],
            true
        );

        let invalid_version = serde_json::json!({
            "schema_version": 1,
            "sources": [{"id": "fixture", "products": [{
                "id": "p", "metadata": {"spatial": {"schema_version": 2}}
            }]}]
        });
        assert!(matches!(
            SourceCatalog::parse(&invalid_version.to_string()),
            Err(CatalogError::InvalidSpatialMetadata { .. })
        ));
    }

    #[test]
    fn automatic_spatial_admission_requires_verified_evidence() {
        let input = serde_json::json!({
            "schema_version": 1,
            "sources": [{"id": "fixture", "products": [{
                "id": "p",
                "metadata": {"spatial": {
                    "schema_version": 1,
                    "admission": {"status": "unverified", "automatic": true}
                }}
            }]}]
        });
        assert!(matches!(
            SourceCatalog::parse(&input.to_string()),
            Err(CatalogError::InvalidSpatialMetadata { .. })
        ));

        let verified = serde_json::json!({
            "schema_version": 1,
            "sources": [{"id": "fixture", "products": [{
                "id": "p",
                "metadata": {"spatial": {
                    "schema_version": 1,
                    "admission": {
                        "status": "verified",
                        "automatic": true,
                        "purposes": ["local_view"],
                        "evidence": [{"source": "fixture evidence"}]
                    }
                }}
            }]}]
        });
        assert!(SourceCatalog::parse(&verified.to_string()).is_ok());
    }

    fn parse_spatial(spatial: serde_json::Value) -> Result<SourceCatalog, CatalogError> {
        let input = serde_json::json!({
            "schema_version": 1,
            "sources": [{"id": "fixture", "products": [{
                "id": "p",
                "metadata": {"spatial": spatial}
            }]}]
        });
        SourceCatalog::parse(&input.to_string())
    }

    #[test]
    fn verified_spatial_claims_require_evidence_and_automatic_admission_local_view_purpose() {
        let evidence = serde_json::json!([{"source": "fixture evidence"}]);
        let invalid = [
            serde_json::json!({"schema_version": 1,
                "observation_coverage": {"kind": "geodesic_circle", "status": "verified"}}),
            serde_json::json!({"schema_version": 1,
                "product_domain": {"status": "verified"}}),
            serde_json::json!({"schema_version": 1, "admission": {
                "status": "verified", "automatic": true, "evidence": evidence}}),
            serde_json::json!({"schema_version": 1, "admission": {
                "status": "verified", "automatic": true, "purposes": ["discovery"],
                "evidence": evidence}}),
        ];
        for spatial in invalid {
            assert!(
                matches!(
                    parse_spatial(spatial.clone()),
                    Err(CatalogError::InvalidSpatialMetadata { .. })
                ),
                "{spatial}"
            );
        }
        let valid = serde_json::json!({"schema_version": 1,
            "observation_coverage": {"kind": "geodesic_circle", "status": "verified",
                "evidence": evidence},
            "product_domain": {"status": "verified", "evidence": evidence},
            "admission": {"status": "verified", "automatic": true,
                "purposes": ["local_view"], "evidence": evidence}});
        assert!(parse_spatial(valid).is_ok());
    }
}
