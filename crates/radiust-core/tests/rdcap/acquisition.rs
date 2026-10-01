//! RDCAP single-use ticket and bounded acquisition contract tests.

use radiust_core::identity::{logical_id, safe_ref};
use radiust_core::model::{
    DiscoveryItem, DiscoveryReport, DiscoveryStatus, DiscoveryTarget, FrameRef, Query,
};
use serde_json::json;

fn rdcap_frame(ticket: &str, key: &str) -> FrameRef {
    let mut frame = FrameRef {
        source: "rdcap".into(),
        product: "reflectivity".into(),
        station: Some("TWN/RCHL".into()),
        valid_time: "2026-10-01T06:05:08.000000Z".into(),
        base_time: None,
        logical_id: String::new(),
        revision: None,
        locator_version: "rdcap-csr-v1".into(),
        locator: json!({
            "country": "TWN",
            "station_code": "RCHL",
            "key": key,
            "url": ticket,
            "headers": {"Referer": "https://rdcap.cwa.gov.tw/data_access/radar_display/TWN/RCHL"},
        }),
    };
    frame.logical_id = logical_id(&frame).unwrap();
    frame
}

#[test]
fn ticket_rotation_keeps_the_selected_logical_frame_and_safe_ref_secret_free() {
    let first = rdcap_frame("https://rdcap.cwa.gov.tw/file?ft=first-secret", "1790834708000");
    let refreshed = rdcap_frame("https://rdcap.cwa.gov.tw/file?ft=rotated-secret", "1790834708000");
    let next_time = rdcap_frame("https://rdcap.cwa.gov.tw/file?ft=next-secret", "1790834709000");

    assert_eq!(first.logical_id, refreshed.logical_id);
    assert_ne!(first.logical_id, next_time.logical_id);
    let safe = serde_json::to_string(&safe_ref(&first).unwrap()).unwrap();
    assert!(!safe.contains("first-secret"));
    assert!(!safe.contains("Referer"));
}

#[test]
fn public_discovery_json_omits_private_frame_locator_and_single_use_ticket() {
    let frame = rdcap_frame("https://rdcap.cwa.gov.tw/file?ft=private-ticket", "1790834708000");
    let query = Query {
        source: Some("rdcap".into()),
        product: Some("reflectivity".into()),
        stations: vec!["TWN/RCHL".into()],
        ..Query::default()
    };
    let report = DiscoveryReport::from_items(
        query,
        vec![DiscoveryItem {
            target: DiscoveryTarget {
                source: "rdcap".into(),
                product: Some("reflectivity".into()),
                station: Some("TWN/RCHL".into()),
            },
            status: DiscoveryStatus::Success,
            valid_time: Some(frame.valid_time.clone()),
            frame: Some(frame),
            error: None,
        }],
        false,
    )
    .unwrap();
    let document = serde_json::to_string(&report.safe_document()).unwrap();
    assert!(!document.contains("private-ticket"));
    assert!(!document.contains("rdcap.cwa.gov.tw"));
    assert!(!document.contains("locator"));
    assert!(document.contains("TWN/RCHL"));
}
