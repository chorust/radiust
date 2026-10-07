//! Static built-in source catalog and deterministic target expansion.

use crate::model::DiscoveryTarget;
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
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
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
    #[error("unknown source: {0}")]
    UnknownSource(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_has_twenty_five_sources_and_seventy_four_targets() {
        let catalog = SourceCatalog::builtin().unwrap();
        assert_eq!(catalog.sources.len(), 25);
        assert_eq!(catalog.expand_targets(None).unwrap().len(), 74);
        assert_eq!(catalog.source("uk").unwrap().availability, "retired");
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
}
