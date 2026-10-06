use radiust_core::identity::{ProcessingSpec, processing_hash};
use radiust_core::limits::Limits;
use radiust_core::{gray, legacy_display};
use serde_json::Value;

#[test]
fn canonical_and_compatibility_modules_share_the_same_gray_implementation() {
    let input = include_bytes!("../../../tests/fixtures/sources/fr/raw/FRCOMP.png");
    let preview = radiust_core::preview::preview_bytes(input, &Limits::default()).unwrap();
    let canonical: gray::GrayPreview = gray::apply_for_source(
        "fr",
        "composite",
        Some("FRCOMP"),
        &preview.format,
        preview.preview.width,
        preview.preview.height,
        &preview.preview.rgba,
        &Limits::default(),
    )
    .unwrap();
    let compatibility: legacy_display::LegacyDisplayPreview = legacy_display::apply_for_source(
        "fr",
        "composite",
        Some("FRCOMP"),
        &preview.format,
        preview.preview.width,
        preview.preview.height,
        &preview.preview.rgba,
        &Limits::default(),
    )
    .unwrap();

    assert!(canonical.applied);
    assert_eq!(canonical.rgba, compatibility.rgba);
    assert_eq!((canonical.width, canonical.height), (compatibility.width, compatibility.height));
    assert_eq!(canonical.rule_version, compatibility.rule_version);
}

#[test]
fn canonical_resource_copies_preserve_historical_wire_bytes_and_rule_hashes() {
    for path in ["index.json", "fr.json", "evidence/fr__composite.json"] {
        let old = std::fs::read(format!(
            "{}/python/radiust/resources/legacy_display/{path}",
            env!("CARGO_MANIFEST_DIR").rsplit_once("/crates/radiust-core").unwrap().0
        ))
        .unwrap();
        let new = std::fs::read(format!(
            "{}/python/radiust/resources/gray/{path}",
            env!("CARGO_MANIFEST_DIR").rsplit_once("/crates/radiust-core").unwrap().0
        ))
        .unwrap();
        assert_eq!(old, new, "canonical mapping changed historical bytes at {path}");
    }
}

#[test]
fn historical_processing_identity_and_tw_locator_version_are_unchanged() {
    let root = env!("CARGO_MANIFEST_DIR").rsplit_once("/crates/radiust-core").unwrap().0;
    let identity: Value = serde_json::from_slice(
        &std::fs::read(format!("{root}/tests/fixtures/rust-migration/output/identity.json"))
            .unwrap(),
    )
    .unwrap();
    let processing: ProcessingSpec =
        serde_json::from_value(identity["processing_identity"].clone()).unwrap();
    assert_eq!(processing_hash(&processing).unwrap(), identity["processing_hash"]);

    let tw: Value = serde_json::from_slice(
        &std::fs::read(format!("{root}/tests/fixtures/sources/tw/fixture.json")).unwrap(),
    )
    .unwrap();
    for frame in tw["frames"].as_array().unwrap() {
        assert_eq!(frame["locator_version"], "tw-legacy-v1");
    }
}
