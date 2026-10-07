#[path = "rdcap/acquisition.rs"]
mod acquisition;
#[path = "rdcap/batch.rs"]
mod batch;
#[path = "rdcap/catalog.rs"]
mod catalog;
#[path = "rdcap/discovery.rs"]
mod discovery;
#[path = "rdcap/output.rs"]
mod output;
#[path = "rdcap/persistence.rs"]
mod persistence;
#[path = "rdcap/science.rs"]
mod science;
#[allow(dead_code)]
#[path = "rdcap/support.rs"]
mod support;

use sha2::{Digest, Sha256};
use std::fs;

#[test]
fn research_fixture_manifest_hashes_match_the_checked_in_payloads() {
    let manifest: serde_json::Value = serde_json::from_str(support::FIXTURE_MANIFEST).unwrap();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["status"], "offline_research_fixture_not_live_acceptance");
    let stations = manifest["stations"].as_array().unwrap();
    assert_eq!(stations.len(), 3);

    for station in stations {
        let id = station["station_id"].as_str().unwrap();
        assert!(matches!(id, "TWRCHL" | "JPISHI" | "PHSUBI"));
        for file in station["files"].as_object().unwrap().values() {
            let relative_path = file["path"].as_str().unwrap();
            let path = support::repository_root().join(relative_path);
            let bytes = fs::read(path).unwrap();
            assert_eq!(bytes.len(), file["bytes"].as_u64().unwrap() as usize, "{id}");
            let digest = hex::encode(Sha256::digest(bytes));
            assert_eq!(digest, file["sha256"].as_str().unwrap(), "{id}");
        }
    }
}

#[test]
fn reconstructed_fixtures_do_not_claim_to_retain_tickets_or_http_responses() {
    let manifest: serde_json::Value = serde_json::from_str(support::FIXTURE_MANIFEST).unwrap();
    assert_eq!(manifest["provenance"]["native_http_file_response_retained"], false);
    assert_eq!(manifest["provenance"]["tickets_or_temporary_credentials_retained"], false);
    assert_eq!(manifest["provenance"]["reconstructed_envelopes_are_original_responses"], false);

    for station in manifest["stations"].as_array().unwrap() {
        let path =
            support::repository_root().join(station["files"]["index"]["path"].as_str().unwrap());
        let index: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(index["_fixture"]["ticket_values"], "omitted");
        assert!(
            index["list"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| { entry["url"].as_array().is_some_and(|urls| urls.is_empty()) })
        );
    }
}
