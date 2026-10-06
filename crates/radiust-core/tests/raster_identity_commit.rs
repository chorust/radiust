use radiust_core::errors::CoreError;
use radiust_core::identity::{
    ProcessingSpec, local_gray_identity, local_numeric_identity, logical_id, raster_commit_identity,
};
use radiust_core::limits::Limits;
use radiust_core::model::{ArtifactReceipt, FrameRef, RawFrameReceipt};
use radiust_core::raster::{
    AlphaPlane, GeometryEvidence, InputReadReceipt, NumericFileIdentity, NumericReadReceipt,
    PixelDbzField, ProcessingRecord, RasterInput, RasterInputIdentity, RasterResult,
    RasterResultData, UpstreamProvenance,
};
use radiust_core::storage::{
    LocalCommitStatus, LocalRasterCommitRequest, LocalStore, StagedArtifact,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

fn numeric_input(digest: &str) -> RasterInput {
    let identity = NumericFileIdentity {
        kind: "local_numeric".into(),
        format: "netcdf".into(),
        content_digest: digest.into(),
        variable: "reflectivity".into(),
        selection: json!({"time": null}),
        valid_time: None,
        geometry: None,
    };
    let read_receipt = NumericReadReceipt {
        content_digest: digest.into(),
        format: "netcdf".into(),
        size_bytes: 123,
        variable: "reflectivity".into(),
        components: vec![],
        selection: json!({"time": null}),
        units: Some("dBZ".into()),
        validated_schema: Some("pixel-dbz-v1".into()),
    };
    RasterInput::NumericFile {
        identity,
        read_receipt,
        upstream_provenance: Some(UpstreamProvenance {
            input_identity: Some(json!({"kind":"source","id":"historical"})),
            processing_record: None,
            manifest_output_id: Some("b".repeat(64)),
        }),
    }
}

fn numeric_commit_request(
    artifact_source: &std::path::Path,
    revision: &str,
    overwrite: bool,
) -> LocalRasterCommitRequest {
    let input = numeric_input(revision);
    let processing_spec = ProcessingSpec {
        format: "netcdf".into(),
        options: json!({"method":"file_dbz","profile":"pixel-dbz-v1"}),
        ..ProcessingSpec::default()
    };
    let identity_receipt = raster_commit_identity(&input, &processing_spec).unwrap();
    LocalRasterCommitRequest {
        input,
        identity_receipt,
        explicit_ref: None,
        processing_spec,
        output_name: "derived/reflectivity.nc".into(),
        artifacts: vec![StagedArtifact {
            name: "reflectivity.nc".into(),
            relative_uri: "derived/reflectivity.nc".into(),
            role: "data".into(),
            media_type: "application/x-netcdf".into(),
            source: artifact_source.to_path_buf(),
        }],
        raw_complete: false,
        overwrite,
    }
}

fn gray_commit_request(
    artifact_source: &std::path::Path,
    revision: &str,
    overwrite: bool,
) -> LocalRasterCommitRequest {
    let input = RasterInput::Local {
        identity: RasterInputIdentity {
            kind: "local_gray".into(),
            content_sha256: revision.into(),
            encoding_declared: "gray-dbz-v1".into(),
            valid_time: None,
            geometry: None,
        },
        read_receipt: InputReadReceipt {
            content_sha256: revision.into(),
            size_bytes: 123,
            media_type: "image/png".into(),
            width: 2,
            height: 2,
            source_bit_depth: 8,
            source_dtype: "u8".into(),
            channels: 1,
            frame_index: None,
        },
    };
    let processing_spec = ProcessingSpec {
        output_kind: "dbz".into(),
        format: "netcdf".into(),
        options: json!({"encoding":"gray-dbz-v1","range_policy":"strict-v1"}),
        ..ProcessingSpec::default()
    };
    let identity_receipt = raster_commit_identity(&input, &processing_spec).unwrap();
    LocalRasterCommitRequest {
        input,
        identity_receipt,
        explicit_ref: None,
        processing_spec,
        output_name: "derived/gray.nc".into(),
        artifacts: vec![StagedArtifact {
            name: "gray.nc".into(),
            relative_uri: "derived/gray.nc".into(),
            role: "data".into(),
            media_type: "application/x-netcdf".into(),
            source: artifact_source.to_path_buf(),
        }],
        raw_complete: false,
        overwrite,
    }
}

#[test]
fn verified_numeric_identity_commits_without_a_frame_reference() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_source = temp.path().join("payload.nc");
    std::fs::write(&artifact_source, b"new numeric output").unwrap();
    let input = numeric_input(&"a".repeat(64));
    let processing_spec = ProcessingSpec {
        format: "netcdf".into(),
        options: json!({"method":"file_dbz","profile":"pixel-dbz-v1"}),
        ..ProcessingSpec::default()
    };
    let identity_receipt = raster_commit_identity(&input, &processing_spec).unwrap();
    let store = LocalStore::new(temp.path().join("store"), Limits::default()).unwrap();
    let committed = store
        .commit_raster(LocalRasterCommitRequest {
            input,
            identity_receipt,
            explicit_ref: None,
            processing_spec,
            output_name: "derived/reflectivity.nc".into(),
            artifacts: vec![StagedArtifact {
                name: "reflectivity.nc".into(),
                relative_uri: "derived/reflectivity.nc".into(),
                role: "data".into(),
                media_type: "application/x-netcdf".into(),
                source: artifact_source,
            }],
            raw_complete: false,
            overwrite: false,
        })
        .unwrap();

    assert_eq!(committed.manifest.revision, "a".repeat(64));
    assert_eq!(committed.manifest.logical_id.len(), 64);
    assert!(!committed.manifest.raw_complete);
    assert_eq!(committed.manifest.schema_version, 1);
    assert!(store.root().join("derived/reflectivity.nc.manifest.json").exists());
}

#[test]
fn raster_commit_rejects_a_receipt_that_does_not_match_the_input() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_source = temp.path().join("payload.nc");
    std::fs::write(&artifact_source, b"new numeric output").unwrap();
    let input = numeric_input(&"a".repeat(64));
    let processing_spec = ProcessingSpec::default();
    let mut identity_receipt = raster_commit_identity(&input, &processing_spec).unwrap();
    identity_receipt.identity.output_id = "c".repeat(64);
    let store = LocalStore::new(temp.path().join("store"), Limits::default()).unwrap();

    let result = store.commit_raster(LocalRasterCommitRequest {
        input,
        identity_receipt,
        explicit_ref: None,
        processing_spec,
        output_name: "derived/reflectivity.nc".into(),
        artifacts: vec![StagedArtifact {
            name: "reflectivity.nc".into(),
            relative_uri: "derived/reflectivity.nc".into(),
            role: "data".into(),
            media_type: "application/x-netcdf".into(),
            source: artifact_source,
        }],
        raw_complete: false,
        overwrite: false,
    });

    assert!(result.is_err());
    assert!(!store.root().join("derived/reflectivity.nc.manifest.json").exists());
}

#[test]
fn explicit_frame_reference_is_rejected_for_a_local_numeric_input() {
    let temp = tempfile::tempdir().unwrap();
    let artifact_source = temp.path().join("payload.nc");
    std::fs::write(&artifact_source, b"new numeric output").unwrap();
    let input = numeric_input(&"a".repeat(64));
    let processing_spec = ProcessingSpec::default();
    let identity_receipt = raster_commit_identity(&input, &processing_spec).unwrap();
    let store = LocalStore::new(temp.path().join("store"), Limits::default()).unwrap();
    let error = store
        .commit_raster(LocalRasterCommitRequest {
            input,
            identity_receipt,
            explicit_ref: Some(FrameRef {
                source: "tw".into(),
                product: "grid".into(),
                station: None,
                valid_time: "2026-10-03T00:00:00Z".into(),
                base_time: None,
                logical_id: String::new(),
                revision: None,
                locator_version: "tw-legacy-v1".into(),
                locator: json!({"revision":"legacy"}),
            }),
            processing_spec,
            output_name: "derived/reflectivity.nc".into(),
            artifacts: vec![StagedArtifact {
                name: "reflectivity.nc".into(),
                relative_uri: "derived/reflectivity.nc".into(),
                role: "data".into(),
                media_type: "application/x-netcdf".into(),
                source: artifact_source,
            }],
            raw_complete: false,
            overwrite: false,
        })
        .unwrap_err();

    assert!(error.to_string().contains("conflicts with a local raster input"));
}

#[test]
fn pixel_geotiff_receipt_hashes_every_declared_component() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("reflectivity.tif");
    let field = PixelDbzField {
        variable: "reflectivity".into(),
        units: "dBZ".into(),
        width: 2,
        height: 2,
        values: vec![0.0, 0.3125, 14.0, 70.0],
        quality: vec![0, 1, 2, 32],
        origin_quality: Some(vec![0, 4, 8, 16]),
        encoding_adjustment: Some(vec![0, 0, 1, 0]),
        alpha: Some(AlphaPlane::U8(vec![0, 1, 128, 255])),
        valid_time: None,
        geometry: Some(GeometryEvidence {
            source: "verified-fixture".into(),
            crs: Some("EPSG:4326".into()),
            x: vec![120.0, 121.0],
            y: vec![24.5, 23.5],
            affine: Some([119.5, 1.0, 0.0, 25.0, 0.0, -1.0]),
            mapping_complete: true,
        }),
        processing: ProcessingRecord {
            schema_version: 1,
            method: "local_gray".into(),
            input_identity: json!({"kind":"local_gray","content_sha256":"a".repeat(64)}),
            encoding_basis: None,
            range_policy: Some("strict-v1".into()),
            decoder_version: Some("1".into()),
            quality_policy_version: Some("1".into()),
            formula: Some("gray*5/16".into()),
            quantization_step: Some(0.3125),
            alpha_bit_depth: Some(8),
            steps: vec![],
            limitations: vec![],
            clipped_pixel_count: Some(0),
            valid_clipped_pixel_count: Some(0),
            upstream: None,
        },
    };
    radiust_core::output::geotiff::write_pixel_dbz(&field, &path, &Limits::default()).unwrap();

    let result =
        radiust_core::output::read_raster_result(&path, None, None, &Limits::default()).unwrap();
    let RasterInput::NumericFile { identity, read_receipt, upstream_provenance } = result.input
    else {
        panic!("GeoTIFF input should be a receipt-bound NumericFile")
    };
    assert_eq!(identity.content_digest, read_receipt.content_digest);
    assert_eq!(identity.selection["variable"], "reflectivity");
    assert!(upstream_provenance.unwrap().input_identity.is_some());
    assert_eq!(
        read_receipt.components.iter().map(|part| part.role.as_str()).collect::<Vec<_>>(),
        ["alpha", "data", "encoding_adjustment", "origin_quality", "provenance", "quality"]
    );
    for component in &read_receipt.components {
        let bytes = std::fs::read(temp.path().join(&component.relative_key)).unwrap();
        assert_eq!(component.size_bytes, bytes.len() as u64);
        assert_eq!(component.sha256, hex::encode(Sha256::digest(&bytes)));
    }

    let other_root = tempfile::tempdir().unwrap();
    for component in &read_receipt.components {
        std::fs::copy(
            temp.path().join(&component.relative_key),
            other_root.path().join(&component.relative_key),
        )
        .unwrap();
    }
    let copied = radiust_core::output::read_raster_result(
        other_root.path().join("reflectivity.tif"),
        None,
        None,
        &Limits::default(),
    )
    .unwrap();
    let RasterInput::NumericFile { identity: copied_identity, .. } = copied.input else {
        panic!("copied GeoTIFF should have a numeric identity")
    };
    assert_eq!(identity, copied_identity);
}

#[test]
fn corrupted_geotiff_component_is_rejected_before_numeric_read_succeeds() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("reflectivity.tif");
    let field = pixel_field_for_geotiff();
    radiust_core::output::geotiff::write_pixel_dbz(&field, &path, &Limits::default()).unwrap();
    std::fs::write(temp.path().join("reflectivity_quality.tif"), b"corrupted component").unwrap();
    assert!(
        radiust_core::output::read_raster_result(&path, None, None, &Limits::default()).is_err()
    );
}

fn pixel_field_for_geotiff() -> PixelDbzField {
    PixelDbzField {
        variable: "reflectivity".into(),
        units: "dBZ".into(),
        width: 2,
        height: 2,
        values: vec![0.0, 0.3125, 14.0, 70.0],
        quality: vec![0, 1, 2, 32],
        origin_quality: None,
        encoding_adjustment: None,
        alpha: None,
        valid_time: None,
        geometry: Some(GeometryEvidence {
            source: "verified-fixture".into(),
            crs: Some("EPSG:4326".into()),
            x: vec![120.0, 121.0],
            y: vec![24.5, 23.5],
            affine: Some([119.5, 1.0, 0.0, 25.0, 0.0, -1.0]),
            mapping_complete: true,
        }),
        processing: ProcessingRecord {
            schema_version: 1,
            method: "local_gray".into(),
            input_identity: json!({"kind":"local_gray","content_sha256":"a".repeat(64)}),
            encoding_basis: None,
            range_policy: Some("strict-v1".into()),
            decoder_version: Some("1".into()),
            quality_policy_version: Some("1".into()),
            formula: Some("gray*5/16".into()),
            quantization_step: Some(0.3125),
            alpha_bit_depth: None,
            steps: vec![],
            limitations: vec![],
            clipped_pixel_count: Some(0),
            valid_clipped_pixel_count: Some(0),
            upstream: None,
        },
    }
}

#[test]
fn local_identity_domains_include_selection_and_ignore_filesystem_roots() {
    let digest = "d".repeat(64);
    let gray = RasterInputIdentity {
        kind: "local_gray".into(),
        content_sha256: digest.clone(),
        encoding_declared: "gray-dbz-v1".into(),
        valid_time: None,
        geometry: None,
    };
    let gray_identity = local_gray_identity(&gray).unwrap();
    assert_eq!(gray_identity.1, digest);

    let base = NumericFileIdentity {
        kind: "local_numeric".into(),
        format: "netcdf".into(),
        content_digest: gray_identity.1.clone(),
        variable: "reflectivity".into(),
        selection: json!({"time_index":0}),
        valid_time: None,
        geometry: None,
    };
    let numeric_identity = local_numeric_identity(&base).unwrap();
    assert_ne!(gray_identity.0, numeric_identity.0);

    let other_variable = NumericFileIdentity { variable: "rain_rate".into(), ..base.clone() };
    let other_selection =
        NumericFileIdentity { selection: json!({"time_index":1}), ..base.clone() };
    assert_ne!(numeric_identity.0, local_numeric_identity(&other_variable).unwrap().0);
    assert_ne!(numeric_identity.0, local_numeric_identity(&other_selection).unwrap().0);
    // Neither identity contains a source path, so the same verified bytes and
    // declaration/selection produce the same value after copying to another root.
    assert_eq!(numeric_identity, local_numeric_identity(&base).unwrap());
}

#[test]
fn source_commit_identity_binds_resolved_revision_and_processing_policy() {
    let mut frame = FrameRef {
        source: "fr".into(),
        product: "composite".into(),
        station: Some("FRCOMP".into()),
        valid_time: "2026-10-06T00:00:00Z".into(),
        base_time: None,
        logical_id: String::new(),
        revision: None,
        locator_version: "fixture-v1".into(),
        locator: json!({"fixture":"source identity"}),
    };
    frame.logical_id = logical_id(&frame).unwrap();
    let input = |revision: &str| RasterInput::Source {
        frame: frame.clone(),
        resolved_revision: revision.into(),
        acquisition_receipt: RawFrameReceipt {
            frame: frame.clone(),
            artifacts: vec![ArtifactReceipt {
                name: "frame.png".into(),
                media_type: "image/png".into(),
                size_bytes: 1,
                sha256: "a".repeat(64),
            }],
        },
    };
    let input_a = input(&"a".repeat(64));
    let spec = ProcessingSpec {
        output_kind: "dbz".into(),
        options: json!({"decoder":"gray-dbz-v1","range":"source-upper-clip-v1"}),
        ..ProcessingSpec::default()
    };
    let revision_a = raster_commit_identity(&input_a, &spec).unwrap();
    let numeric_identity = raster_commit_identity(&numeric_input(&"a".repeat(64)), &spec).unwrap();
    let revision_b = raster_commit_identity(&input(&"b".repeat(64)), &spec).unwrap();
    let changed_policy = ProcessingSpec {
        options: json!({"decoder":"gray-dbz-v2","range":"source-upper-clip-v1"}),
        ..spec.clone()
    };
    let policy_b = raster_commit_identity(&input_a, &changed_policy).unwrap();

    assert_eq!(revision_a.identity.logical_id, revision_b.identity.logical_id);
    assert_ne!(revision_a.identity.input_kind, numeric_identity.identity.input_kind);
    assert_ne!(revision_a.identity.logical_id, numeric_identity.identity.logical_id);
    assert_ne!(revision_a.identity.revision, revision_b.identity.revision);
    assert_ne!(revision_a.identity.output_id, revision_b.identity.output_id);
    assert_ne!(revision_a.identity.processing_hash, policy_b.identity.processing_hash);
    assert_ne!(revision_a.identity.output_id, policy_b.identity.output_id);
}

#[test]
fn numeric_raster_transaction_skips_repairs_overwrites_and_cancels_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("payload.nc");
    std::fs::write(&source, b"numeric artifact v1").unwrap();
    let store = LocalStore::new(temp.path().join("store"), Limits::default()).unwrap();

    let first =
        store.commit_raster(numeric_commit_request(&source, &"a".repeat(64), false)).unwrap();
    assert_eq!(first.status, LocalCommitStatus::Written);
    let skipped =
        store.commit_raster(numeric_commit_request(&source, &"a".repeat(64), false)).unwrap();
    assert_eq!(skipped.status, LocalCommitStatus::Skipped);
    assert_eq!(skipped.manifest.generation, first.manifest.generation);

    let output = store.root().join("derived/reflectivity.nc");
    std::fs::write(&output, b"corrupt bytes").unwrap();
    let repaired =
        store.commit_raster(numeric_commit_request(&source, &"a".repeat(64), false)).unwrap();
    assert_eq!(repaired.status, LocalCommitStatus::Written);
    assert_eq!(std::fs::read(&output).unwrap(), b"numeric artifact v1");
    assert!(radiust_core::storage::manifest::is_complete(store.root(), &repaired.manifest));

    std::fs::write(&source, b"numeric artifact v2").unwrap();
    let conflict = store.commit_raster(numeric_commit_request(&source, &"b".repeat(64), false));
    assert!(matches!(conflict, Err(CoreError::OutputConflict)));
    let overwritten =
        store.commit_raster(numeric_commit_request(&source, &"b".repeat(64), true)).unwrap();
    assert_eq!(overwritten.status, LocalCommitStatus::Written);
    assert_ne!(overwritten.manifest.generation, repaired.manifest.generation);
    assert_eq!(std::fs::read(&output).unwrap(), b"numeric artifact v2");

    let cancel_store =
        LocalStore::new(temp.path().join("cancel-store"), Limits::default()).unwrap();
    let cancelled_source = temp.path().join("cancel.nc");
    std::fs::write(&cancelled_source, b"cancel me").unwrap();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let result = cancel_store.commit_raster_cancellable(
        numeric_commit_request(&cancelled_source, &"c".repeat(64), false),
        &cancellation,
    );
    assert!(matches!(result, Err(CoreError::Cancelled)));
    assert!(!cancel_store.root().join("derived/reflectivity.nc").exists());
    assert!(!cancel_store.root().join("derived/reflectivity.nc.manifest.json").exists());
    assert!(
        std::fs::read_dir(cancel_store.root())
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().starts_with(".radiust-stage-"))
    );
}

#[test]
fn local_gray_identity_commit_skips_verified_output_and_repairs_corruption() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("gray-output.nc");
    std::fs::write(&source, b"gray-derived numeric artifact").unwrap();
    let store = LocalStore::new(temp.path().join("store"), Limits::default()).unwrap();

    let first = store.commit_raster(gray_commit_request(&source, &"d".repeat(64), false)).unwrap();
    assert_eq!(first.status, LocalCommitStatus::Written);
    assert_eq!(first.manifest.revision, "d".repeat(64));
    assert!(!first.manifest.raw_complete);
    let repeated =
        store.commit_raster(gray_commit_request(&source, &"d".repeat(64), false)).unwrap();
    assert_eq!(repeated.status, LocalCommitStatus::Skipped);
    assert_eq!(repeated.manifest.generation, first.manifest.generation);

    std::fs::write(store.root().join("derived/gray.nc"), b"corrupted output").unwrap();
    let repaired =
        store.commit_raster(gray_commit_request(&source, &"d".repeat(64), false)).unwrap();
    assert_eq!(repaired.status, LocalCommitStatus::Written);
    assert_eq!(
        std::fs::read(store.root().join("derived/gray.nc")).unwrap(),
        b"gray-derived numeric artifact"
    );
    assert!(radiust_core::storage::manifest::is_complete(store.root(), &repaired.manifest));
}

#[test]
fn numeric_read_receipt_hashes_single_file_and_resave_does_not_reuse_upstream_id() {
    let temp = tempfile::tempdir().unwrap();
    let input_path = temp.path().join("input.nc");
    let copied_path = temp.path().join("other-root/input.nc");
    let mut original = pixel_field_for_geotiff();
    original.geometry = None;
    original.processing.upstream = Some(UpstreamProvenance {
        input_identity: Some(json!({"kind":"legacy_source","logical_id":"old"})),
        processing_record: Some(json!({"method":"old-decoder"})),
        manifest_output_id: Some("b".repeat(64)),
    });
    radiust_core::output::netcdf::write_pixel_dbz(&original, &input_path, &Limits::default())
        .unwrap();
    std::fs::create_dir_all(copied_path.parent().unwrap()).unwrap();
    std::fs::copy(&input_path, &copied_path).unwrap();
    let bytes = std::fs::read(&input_path).unwrap();
    let expected_sha = hex::encode(Sha256::digest(&bytes));

    let first =
        radiust_core::output::read_raster_result(&input_path, None, None, &Limits::default())
            .unwrap();
    let second =
        radiust_core::output::read_raster_result(&copied_path, None, None, &Limits::default())
            .unwrap();
    let (
        RasterInput::NumericFile { identity, read_receipt, upstream_provenance },
        RasterInput::NumericFile { identity: copied_identity, .. },
    ) = (&first.input, &second.input)
    else {
        panic!("numeric NetCDF reads should have receipt-bound file identities")
    };
    assert_eq!(identity.content_digest, expected_sha);
    assert_eq!(read_receipt.size_bytes, bytes.len() as u64);
    assert_eq!(identity, copied_identity);
    let upstream = upstream_provenance.as_ref().unwrap();
    assert_eq!(upstream.manifest_output_id, None);
    let upstream_id =
        upstream.processing_record.as_ref().unwrap()["upstream"]["manifest_output_id"]
            .as_str()
            .unwrap();
    assert_eq!(upstream_id, "b".repeat(64));

    let RasterResultData::Pixel(field) = &first.data else {
        panic!("Pixel NetCDF read should preserve its data profile")
    };
    let result = RasterResult {
        input: first.input.clone(),
        data: RasterResultData::Pixel(field.clone()),
        processing: first.processing.clone(),
        mode_info: first.mode_info.clone(),
    };
    let saved = radiust_core::output::write_raster_result(
        &result,
        temp.path().join("resaved"),
        "reflectivity.nc",
        "netcdf",
        &json!({}),
        false,
        &Limits::default(),
    )
    .unwrap();
    assert_ne!(saved.manifest.output_id, upstream_id);
    assert_eq!(saved.manifest.revision, expected_sha);
    assert_eq!(saved.manifest.schema_version, 1);
}

#[test]
fn zarr_numeric_receipt_hashes_the_complete_tree_independent_of_root() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("input.zarr");
    let copied_path = temp.path().join("copy/input.zarr");
    let mut field = pixel_field_for_geotiff();
    field.geometry = None;
    radiust_core::output::zarr::write_pixel_dbz(&field, &path, &Limits::default()).unwrap();

    let first =
        radiust_core::output::read_raster_result(&path, None, None, &Limits::default()).unwrap();
    let RasterInput::NumericFile { identity, read_receipt, .. } = &first.input else {
        panic!("Zarr input should receive a numeric identity")
    };
    assert!(read_receipt.components.len() > 3);
    for component in &read_receipt.components {
        let bytes = std::fs::read(path.join(&component.relative_key)).unwrap();
        assert_eq!(component.size_bytes, bytes.len() as u64);
        assert_eq!(component.sha256, hex::encode(Sha256::digest(bytes)));
    }

    for component in &read_receipt.components {
        let source = path.join(&component.relative_key);
        let destination = copied_path.join(&component.relative_key);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::copy(source, destination).unwrap();
    }
    let copied =
        radiust_core::output::read_raster_result(&copied_path, None, None, &Limits::default())
            .unwrap();
    let RasterInput::NumericFile { identity: copied_identity, .. } = copied.input else {
        panic!("copied Zarr input should receive a numeric identity")
    };
    assert_eq!(identity, &copied_identity);

    std::fs::write(copied_path.join("extra-unreferenced-component"), b"part of the input tree")
        .unwrap();
    let extended =
        radiust_core::output::read_raster_result(&copied_path, None, None, &Limits::default())
            .unwrap();
    let RasterInput::NumericFile { identity: extended_identity, read_receipt, .. } = extended.input
    else {
        panic!("extended Zarr input should still be readable")
    };
    assert_ne!(copied_identity, extended_identity);
    assert!(
        read_receipt
            .components
            .iter()
            .any(|component| component.relative_key == "extra-unreferenced-component")
    );
}
