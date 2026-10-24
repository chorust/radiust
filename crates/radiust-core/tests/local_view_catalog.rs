use radiust_core::source::catalog::{
    ExternalCatalogRelation, SourceCatalog, SpatialEvidenceStatus, SpatialTargetKind,
};
use std::collections::BTreeSet;

#[test]
fn local_station_index_keeps_legacy_channels_separate_without_expanding_all() {
    let catalog = SourceCatalog::builtin().unwrap();
    assert_eq!(catalog.expand_targets(None).unwrap().len(), 75);
    assert_eq!(catalog.spatial_targets(None).unwrap().len(), 75);

    for (source, product) in [("id", "composite"), ("id_sidarma", "cmax")] {
        let targets = catalog.spatial_targets(Some(&[source.to_owned()])).unwrap();
        let stations =
            targets.iter().filter(|target| target.target.station.is_some()).collect::<Vec<_>>();
        assert_eq!(stations.len(), 47);
        assert!(stations.iter().all(|target| {
            target.target.source == source
                && target.target.product.as_deref() == Some(product)
                && target.site_status == SpatialEvidenceStatus::Declared
                && target
                    .geometry
                    .as_ref()
                    .is_some_and(|value| value.status == SpatialEvidenceStatus::Unknown)
                && target
                    .observation_coverage
                    .as_ref()
                    .is_some_and(|value| value.status == SpatialEvidenceStatus::Unknown)
                && !target.admission.as_ref().is_some_and(|admission| admission.automatic)
        }));
        assert!(stations.iter().any(|target| {
            target.target.station.as_deref() == Some("MCRC")
                && target.kind == SpatialTargetKind::Unknown
        }));
    }
}

#[test]
fn external_station_candidates_keep_review_status_and_ranges_out_of_coverage() {
    let catalog = SourceCatalog::builtin().unwrap();
    let rdcap = catalog.source("rdcap").unwrap();
    assert_eq!(rdcap.stations.len(), 48);
    let twrcwf = rdcap.stations.iter().find(|station| station.id == "TWRCWF").unwrap();
    let spatial = twrcwf.metadata.as_ref().unwrap().spatial.as_ref().unwrap();
    let reference = &spatial.external_catalog_refs[0];
    assert_eq!(reference.catalog, "Tropical Globe Radar Database CSV");
    assert_eq!(reference.snapshot_date.as_deref(), Some("2026-10-09"));
    assert_eq!(reference.snapshot_sha256.len(), 64);
    assert_eq!(reference.relation, ExternalCatalogRelation::Colocated);
    assert_eq!(reference.status, SpatialEvidenceStatus::Unverified);
    assert_eq!(
        spatial.observation_coverage.as_ref().unwrap().status,
        SpatialEvidenceStatus::Unknown
    );
    assert_eq!(
        spatial.external_range_claim.as_ref().unwrap().status,
        SpatialEvidenceStatus::Unverified
    );

    let sidarma = include_str!("../resources/builtin/sources/id_sidarma.json");
    let resource: serde_json::Value = serde_json::from_str(sidarma).unwrap();
    let sorong =
        resource["radars"].as_array().unwrap().iter().find(|radar| radar["id"] == "SOR").unwrap();
    let refs = sorong["metadata"]["spatial"]["external_catalog_refs"].as_array().unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0]["relation"], "conflicting");
    assert_eq!(refs[1]["relation"], "successor");
}

#[test]
fn local_view_inventory_retains_all_station_candidates_and_selector_kinds() {
    let catalog = SourceCatalog::builtin().unwrap();
    let rdcap = catalog.source("rdcap").unwrap();
    let rdcap_candidates = rdcap
        .stations
        .iter()
        .flat_map(|station| {
            station
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.spatial.as_ref())
                .into_iter()
                .flat_map(|spatial| &spatial.external_catalog_refs)
        })
        .collect::<Vec<_>>();
    assert_eq!(rdcap.stations.len(), 48);
    assert_eq!(rdcap_candidates.len(), 48);
    assert!(rdcap_candidates.iter().all(|reference| {
        reference.snapshot_date.as_deref() == Some("2026-10-09")
            && reference.filter.as_deref().is_some_and(|filter| filter.contains("country_id="))
            && reference
                .distance_method
                .as_deref()
                .is_some_and(|method| method.contains("Haversine"))
            && reference.status == SpatialEvidenceStatus::Unverified
    }));

    let phbale = rdcap
        .metadata
        .as_ref()
        .unwrap()
        .directory_conflicts
        .iter()
        .find(|conflict| conflict.station_id == "PHBALE" && conflict.field == "Status")
        .unwrap();
    assert_eq!(phbale.values, ["Inactive", "Active"]);

    let mut selectors = BTreeSet::new();
    for source in &catalog.sources {
        for product in &source.products {
            if let Some(spatial) =
                product.metadata.as_ref().and_then(|metadata| metadata.spatial.as_ref())
            {
                if spatial.kind == SpatialTargetKind::ProductSelector {
                    for selector in spatial
                        .extensions
                        .get("selector_ids")
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(serde_json::Value::as_str)
                    {
                        selectors.insert((source.id.as_str(), selector));
                    }
                }
            }
        }
        for station in &source.stations {
            if station
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.spatial.as_ref())
                .is_some_and(|spatial| spatial.kind == SpatialTargetKind::RegionSelector)
            {
                selectors.insert((source.id.as_str(), station.id.as_str()));
            }
        }
    }
    for expected in [
        ("my", "peninsular"),
        ("my", "east"),
        ("es", "ESCOMP"),
        ("pt", "PTST2"),
        ("sg", "SGCOMP"),
        ("tw", "CV1_3600"),
        ("tw-http", "CV1_3600"),
        ("ph", "PHCOMP4"),
        ("th", "cmp1"),
        ("th", "kkn240Loop"),
    ] {
        assert!(selectors.contains(&expected), "missing selector {expected:?}");
    }
    assert_eq!(selectors.len(), 10);

    let sidarma: serde_json::Value =
        serde_json::from_str(include_str!("../resources/builtin/sources/id_sidarma.json")).unwrap();
    let radars = sidarma["radars"].as_array().unwrap();
    assert_eq!(radars.len(), 47);
    let matched_candidates = rdcap_candidates.len()
        + radars
            .iter()
            .flat_map(|radar| {
                radar["metadata"]["spatial"]["external_catalog_refs"].as_array().unwrap()
            })
            .filter(|reference| reference["relation"] != "successor")
            .count();
    assert_eq!(matched_candidates, 95);
}
