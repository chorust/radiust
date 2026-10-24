use radiust_core::source::catalog::{SourceCatalog, SpatialEvidenceStatus};

#[test]
fn local_view_external_snapshot_and_relation_evidence_round_trip_without_claiming_coverage() {
    let snapshot = "a".repeat(64);
    let catalog = serde_json::json!({
        "schema_version": 1,
        "sources": [{
            "id": "fixture",
            "products": [{
                "id": "composite",
                "metadata": {"spatial": {
                    "schema_version": 1,
                    "kind": "product_selector",
                    "source_class": "unknown",
                    "external_catalog_refs": [
                        {"catalog":"external", "snapshot_sha256":snapshot, "row":1,
                         "name":"candidate same site", "relation":"same_site", "status":"unverified",
                         "source_url":"https://example.invalid/radar_database/#show_export=1",
                         "snapshot_date":"2026-10-09", "filter":"country_id=IDN; nearest three",
                         "distance_km":0.2, "distance_method":"Haversine", "review_note":"review pending"},
                        {"catalog":"external", "snapshot_sha256":snapshot, "row":2,
                         "name":"co-located candidate", "relation":"colocated", "status":"unverified"},
                        {"catalog":"external", "snapshot_sha256":snapshot, "row":3,
                         "name":"possible successor", "relation":"successor", "status":"unverified"},
                        {"catalog":"external", "snapshot_sha256":snapshot, "row":4,
                         "name":"closed historical site", "relation":"conflicting", "status":"unverified",
                         "review_note":"closed row retained as an unresolved historical candidate"}
                    ],
                    "external_range_claim":{"source":"external directory", "value":240.0, "unit":"km", "status":"unverified"},
                    "product_domain":{"status":"declared", "crs":"EPSG:4326",
                        "geometry":{"bbox":[170.0,-10.0,-170.0,10.0]},
                        "evidence":[{"source":"controlled metadata fixture"}]},
                    "admission":{"status":"unverified", "automatic":false, "purposes":["local_view"]}
                }}
            }]
        }]
    });
    let parsed = SourceCatalog::parse(&catalog.to_string()).unwrap();
    let product_spatial = parsed.source("fixture").unwrap().products[0]
        .metadata
        .as_ref()
        .unwrap()
        .spatial
        .as_ref()
        .unwrap();
    assert_eq!(product_spatial.external_catalog_refs.len(), 4);
    assert_eq!(
        product_spatial.external_catalog_refs[0].snapshot_date.as_deref(),
        Some("2026-10-09")
    );
    assert_eq!(
        product_spatial.external_catalog_refs[0].filter.as_deref(),
        Some("country_id=IDN; nearest three")
    );
    assert_eq!(
        product_spatial.external_catalog_refs[0].distance_method.as_deref(),
        Some("Haversine")
    );
    assert_eq!(
        product_spatial.external_catalog_refs[3].relation,
        radiust_core::source::catalog::ExternalCatalogRelation::Conflicting
    );
    assert_eq!(product_spatial.external_catalog_refs[3].status, SpatialEvidenceStatus::Unverified);

    let targets = parsed.spatial_targets(Some(&["fixture".into()])).unwrap();
    assert_eq!(targets.len(), 1);
    assert!(targets[0].observation_coverage.is_none());
    assert_eq!(targets[0].admission.as_ref().unwrap().automatic, false);
    assert_eq!(
        targets[0].product_domain.as_ref().unwrap().geometry.as_ref().unwrap()["bbox"],
        serde_json::json!([170.0, -10.0, -170.0, 10.0])
    );
    assert_eq!(parsed.expand_targets(Some(&["fixture".into()])).unwrap().len(), 1);
}
