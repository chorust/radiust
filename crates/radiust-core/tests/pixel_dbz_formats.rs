use radiust_core::limits::Limits;
use radiust_core::model::{Grid, RadarField};
use radiust_core::output::{
    self, GeographicOutputRequest, RasterOutputFormat, RasterOutputProfile,
};
use radiust_core::raster::{AlphaPlane, PixelDbzField, ProcessingRecord};
use serde_json::json;
use sha2::Digest;

fn field(valid_time: Option<&str>) -> PixelDbzField {
    PixelDbzField {
        variable: "reflectivity".into(),
        units: "dBZ".into(),
        width: 5,
        height: 1,
        values: vec![0.0, 0.3125, 14.0, 70.0, 3.5],
        quality: vec![0, 1, 2, 32, 64],
        origin_quality: Some(vec![0, 0, 4, 8, 16]),
        encoding_adjustment: Some(vec![0, 0, 1, 0, 0]),
        alpha: Some(AlphaPlane::U16(vec![0, 1, 255, 256, 65_535])),
        valid_time: valid_time.map(str::to_owned),
        geometry: None,
        processing: ProcessingRecord {
            schema_version: 1,
            method: "local_gray".into(),
            input_identity: json!({"kind":"local_gray","content_sha256":"a".repeat(64)}),
            encoding_basis: None,
            range_policy: Some("strict-v1".into()),
            decoder_version: Some("1".into()),
            quality_policy_version: Some("1".into()),
            formula: Some("min(gray,224)*5/16".into()),
            quantization_step: Some(0.3125),
            alpha_bit_depth: Some(16),
            steps: vec![],
            limitations: vec!["pixel coordinates only".into()],
            clipped_pixel_count: Some(1),
            valid_clipped_pixel_count: Some(1),
            upstream: None,
        },
    }
}

#[test]
fn pixel_netcdf_roundtrips_values_quality_history_and_original_alpha() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pixel.nc");
    let expected = field(None);

    output::netcdf::write_pixel_dbz(&expected, &path, &Limits::default()).unwrap();
    let actual = output::netcdf::read_pixel_dbz(&path, &Limits::default()).unwrap();

    assert_eq!(actual, expected);
}

#[test]
fn pixel_zarr_roundtrips_known_time_and_original_alpha() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pixel.zarr");
    let expected = field(Some("2026-10-03T01:02:03Z"));

    output::zarr::write_pixel_dbz(&expected, &path, &Limits::default()).unwrap();
    let actual = output::zarr::read_pixel_dbz(&path, &Limits::default()).unwrap();

    assert_eq!(actual, expected);
}

#[test]
fn pixel_netcdf_and_zarr_preserve_alpha8_dtype_and_time_metadata() {
    for (filename, zarr, valid_time) in
        [("alpha8.nc", false, Some("2026-10-03T01:02:03Z")), ("alpha8.zarr", true, None)]
    {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(filename);
        let mut expected = field(valid_time);
        expected.alpha = Some(AlphaPlane::U8(vec![0, 1, 127, 128, 255]));
        expected.processing.alpha_bit_depth = Some(8);
        if zarr {
            output::zarr::write_pixel_dbz(&expected, &path, &Limits::default()).unwrap();
            assert_eq!(output::zarr::read_pixel_dbz(&path, &Limits::default()).unwrap(), expected);
        } else {
            output::netcdf::write_pixel_dbz(&expected, &path, &Limits::default()).unwrap();
            assert_eq!(
                output::netcdf::read_pixel_dbz(&path, &Limits::default()).unwrap(),
                expected
            );
        }
    }
}

#[test]
fn pixel_output_preflight_allows_unknown_time_and_rejects_fake_geography() {
    assert!(
        output::preflight_raster_output(
            RasterOutputProfile::Pixel,
            RasterOutputFormat::NetCdf,
            None,
            GeographicOutputRequest::default(),
        )
        .is_ok()
    );
    assert!(
        output::preflight_raster_output(
            RasterOutputProfile::Pixel,
            RasterOutputFormat::GeoTiff,
            None,
            GeographicOutputRequest::default(),
        )
        .is_err()
    );
}

#[test]
fn pixel_geotiff_roundtrips_trusted_geometry_quality_adjustment_and_alpha16() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("trusted.tif");
    let mut expected = field(None);
    expected.width = 2;
    expected.height = 2;
    expected.values = vec![0.0, 0.3125, 14.0, 70.0];
    expected.quality = vec![0, 1, 2, 32];
    expected.origin_quality = Some(vec![0, 4, 8, 16]);
    expected.encoding_adjustment = Some(vec![0, 0, 1, 0]);
    expected.alpha = Some(AlphaPlane::U16(vec![0, 1, 256, 65_535]));
    expected.geometry = Some(radiust_core::raster::GeometryEvidence {
        source: "verified-fixture".into(),
        crs: Some("EPSG:4326".into()),
        x: vec![120.0, 121.0],
        y: vec![23.5, 24.5],
        affine: Some([119.5, 1.0, 0.0, 25.0, 0.0, -1.0]),
        mapping_complete: true,
    });

    let artifacts = output::geotiff::write_pixel_dbz(&expected, &path, &Limits::default()).unwrap();
    assert_eq!(artifacts.len(), 6);
    let actual = output::geotiff::read_pixel_dbz(&path, &Limits::default()).unwrap();

    assert_eq!(actual.values, expected.values);
    assert_eq!(actual.quality, expected.quality);
    assert_eq!(actual.origin_quality, expected.origin_quality);
    assert_eq!(actual.encoding_adjustment, expected.encoding_adjustment);
    assert_eq!(actual.alpha, expected.alpha);
    assert_eq!(actual.valid_time, None);
    assert_eq!(actual.processing, expected.processing);
    assert!(output::geotiff::read_selected_field(&path, &Limits::default()).is_err());
    let geometry = actual.geometry.unwrap();
    assert_eq!(geometry.crs.as_deref(), Some("EPSG:4326"));
    assert_eq!(geometry.x, vec![120.0, 121.0]);
    assert_eq!(geometry.y, vec![23.5, 24.5]);
    assert!(geometry.mapping_complete);

    expected.origin_quality = None;
    expected.encoding_adjustment = None;
    expected.alpha = None;
    expected.processing.alpha_bit_depth = None;
    let artifacts = output::geotiff::write_pixel_dbz(&expected, &path, &Limits::default()).unwrap();
    assert_eq!(artifacts.len(), 3);
    assert!(!temp.path().join("trusted_origin_quality.tif").exists());
    assert!(!temp.path().join("trusted_encoding_adjustment.tif").exists());
    assert!(!temp.path().join("trusted_alpha.tif").exists());
    expected.geometry.as_mut().unwrap().source = "geotiff".into();
    assert_eq!(output::geotiff::read_pixel_dbz(&path, &Limits::default()).unwrap(), expected);

    let result = output::read_raster_result(&path, None, None, &Limits::default()).unwrap();
    assert!(matches!(result.data, radiust_core::raster::RasterResultData::Pixel(_)));
    let (identity, receipt, upstream) = match result.input {
        radiust_core::raster::RasterInput::NumericFile {
            identity,
            read_receipt,
            upstream_provenance,
        } => (identity, read_receipt, upstream_provenance),
        _ => panic!("GeoTIFF should receive a numeric file identity"),
    };
    assert_eq!(receipt.components.len(), 3);
    assert!(!receipt.components.iter().any(|part| part.role == "alpha"));
    assert_eq!(identity.selection["variable"], "reflectivity");
    assert_eq!(identity.selection["valid_time"], serde_json::Value::Null);
    assert!(upstream.unwrap().input_identity.is_some());
}

#[test]
fn pixel_geotiff_rejects_untrusted_or_incomplete_mapping() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unknown.tif");
    let mut value = field(None);
    value.width = 2;
    value.height = 2;
    value.values = vec![0.0; 4];
    value.quality = vec![0; 4];
    value.origin_quality = None;
    value.encoding_adjustment = None;
    value.alpha = None;
    value.processing.alpha_bit_depth = None;
    assert!(output::geotiff::write_pixel_dbz(&value, &path, &Limits::default()).is_err());
}

#[test]
fn pixel_geotiff_rejects_affine_that_disagrees_with_center_axes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("mismatch.tif");
    let mut value = field(None);
    value.width = 2;
    value.height = 2;
    value.values = vec![0.0; 4];
    value.quality = vec![0; 4];
    value.origin_quality = None;
    value.encoding_adjustment = None;
    value.alpha = None;
    value.processing.alpha_bit_depth = None;
    value.geometry = Some(radiust_core::raster::GeometryEvidence {
        source: "verified-fixture".into(),
        crs: Some("EPSG:4326".into()),
        x: vec![120.0, 121.0],
        y: vec![23.5, 24.5],
        affine: Some([118.5, 1.0, 0.0, 25.0, 0.0, -1.0]),
        mapping_complete: true,
    });
    assert!(output::geotiff::write_pixel_dbz(&value, &path, &Limits::default()).is_err());
}

#[test]
fn trusted_geometry_cannot_be_reused_after_only_the_pixel_shape_changes() {
    let mut value = field(None);
    value.width = 2;
    value.height = 2;
    value.values = vec![0.0; 4];
    value.quality = vec![0; 4];
    value.origin_quality = None;
    value.encoding_adjustment = None;
    value.alpha = None;
    value.processing.alpha_bit_depth = None;
    let evidence = radiust_core::raster::GeometryEvidence {
        source: "verified-fixture".into(),
        crs: Some("EPSG:4326".into()),
        x: vec![120.0, 121.0],
        y: vec![23.5, 24.5],
        affine: Some([119.5, 1.0, 0.0, 25.0, 0.0, -1.0]),
        mapping_complete: true,
    };
    assert!(evidence.trusted_grid(2, 2).is_some());
    value.width = 3;
    value.values = vec![0.0; 6];
    value.quality = vec![0; 6];
    value.geometry = Some(evidence.clone());
    assert!(evidence.trusted_grid(3, 2).is_none());
    let temp = tempfile::tempdir().unwrap();
    assert!(
        output::geotiff::write_pixel_dbz(
            &value,
            temp.path().join("wrong_extent.tif"),
            &Limits::default(),
        )
        .is_err()
    );
}

#[test]
fn pixel_geotiff_roundtrips_uint8_alpha_without_inventing_time() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("alpha8.tif");
    let mut expected = field(None);
    expected.width = 2;
    expected.height = 2;
    expected.values = vec![1.0, 2.0, 3.0, 4.0];
    expected.quality = vec![0; 4];
    expected.origin_quality = None;
    expected.encoding_adjustment = None;
    expected.alpha = Some(AlphaPlane::U8(vec![0, 1, 128, 255]));
    expected.processing.alpha_bit_depth = Some(8);
    expected.geometry = Some(radiust_core::raster::GeometryEvidence {
        source: "verified-fixture".into(),
        crs: Some("EPSG:4326".into()),
        x: vec![120.0, 121.0],
        y: vec![24.5, 23.5],
        affine: Some([119.5, 1.0, 0.0, 25.0, 0.0, -1.0]),
        mapping_complete: true,
    });
    output::geotiff::write_pixel_dbz(&expected, &path, &Limits::default()).unwrap();
    let actual = output::geotiff::read_pixel_dbz(&path, &Limits::default()).unwrap();
    assert_eq!(actual.alpha, expected.alpha);
    assert_eq!(actual.valid_time, None);
    assert_eq!(actual.values, expected.values);
}

#[test]
fn pixel_png_uses_alpha_only_for_preview_and_records_that_it_is_display_only() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("preview.png");
    let mut expected = field(None);
    expected.quality.fill(0);

    let paths =
        output::png::write_pixel_dbz_png(&expected, &path, &json!({}), &Limits::default()).unwrap();
    let image = image::open(&paths[0]).unwrap().to_rgba8();
    assert_eq!(image.pixels().map(|pixel| pixel[3]).collect::<Vec<_>>(), [0, 1, 1, 1, 255]);
    let sidecar: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&paths[1]).unwrap()).unwrap();
    assert_eq!(sidecar["mode"], "dbz");
    assert_eq!(sidecar["units"], "dBZ");
    assert_eq!(sidecar["numeric_values_stored"], false);
    assert_eq!(sidecar["original_alpha_bit_depth"], 16);
}

#[test]
fn reading_a_pixel_file_creates_a_current_numeric_receipt_and_keeps_embedded_identity_upstream() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("input.nc");
    let expected = field(None);
    output::netcdf::write_pixel_dbz(&expected, &path, &Limits::default()).unwrap();

    let result = output::read_dbz(&path, None, None, &Limits::default()).unwrap();
    let radiust_core::raster::RasterInput::NumericFile {
        identity,
        read_receipt,
        upstream_provenance,
    } = result.input
    else {
        panic!("numeric file reader returned another input kind")
    };
    assert_eq!(identity.content_digest, read_receipt.content_digest);
    assert_eq!(
        identity.content_digest,
        hex::encode(sha2::Sha256::digest(std::fs::read(&path).unwrap()))
    );
    assert_eq!(identity.variable, "reflectivity");
    assert!(upstream_provenance.as_ref().unwrap().input_identity.is_some());
    assert!(matches!(result.data, radiust_core::raster::RasterResultData::Pixel(_)));
}

#[test]
fn numeric_pixel_read_can_be_rewritten_without_a_frame_reference_or_second_conversion() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("input.nc");
    let expected = field(None);
    output::netcdf::write_pixel_dbz(&expected, &source, &Limits::default()).unwrap();
    let result = output::read_dbz(&source, None, None, &Limits::default()).unwrap();
    let original_identity = match &result.input {
        radiust_core::raster::RasterInput::NumericFile { identity, .. } => {
            identity.content_digest.clone()
        }
        _ => panic!("numeric input identity expected"),
    };

    let committed = output::write_raster_result(
        &result,
        temp.path().join("out"),
        "derived/copied.nc",
        "netcdf",
        &json!({}),
        false,
        &Limits::default(),
    )
    .unwrap();
    let copied_path = temp.path().join("out/derived/copied.nc");
    let actual = output::read_dbz(&copied_path, None, None, &Limits::default()).unwrap();

    let radiust_core::raster::RasterResultData::Pixel(actual_field) = actual.data else {
        panic!("pixel output should remain a pixel dBZ raster")
    };
    assert_eq!(actual_field.values, expected.values);
    assert_eq!(actual_field.quality, expected.quality);
    assert_eq!(actual_field.processing, expected.processing);
    let radiust_core::raster::RasterInput::NumericFile { identity, .. } = actual.input else {
        panic!("rewritten input should have its own numeric identity")
    };
    assert_eq!(identity.content_digest, original_identity);
    assert_ne!(committed.manifest.output_id, "b".repeat(64));
    assert_eq!(committed.manifest.revision, original_identity);
    assert!(temp.path().join("out/derived/copied.nc.manifest.json").exists());
}

#[test]
fn native_numeric_read_keeps_real_units_and_dbz_read_rejects_rain_rate() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rain.nc");
    let field = RadarField {
        name: "rain_rate".into(),
        values: vec![1.0, 2.0],
        shape: vec![1, 2],
        quality: vec![0, 0],
        units: Some("mm/h".into()),
        valid_time: "2026-10-03T01:02:03Z".into(),
        grid: Grid {
            shape: vec![1, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![120.0, 120.1],
            y: vec![24.0],
            affine: Some([119.95, 0.1, 0.0, 24.05, 0.0, -0.1]),
        },
        provenance: vec!["native fixture".into()],
    };
    output::netcdf::write_field(&field, &path, &Limits::default()).unwrap();

    let generic = output::read_raster_result(&path, None, None, &Limits::default()).unwrap();
    let radiust_core::raster::RasterResultData::Native(actual) = generic.data else {
        panic!("native NetCDF should use the existing RadarField reader")
    };
    assert_eq!(actual.units.as_deref(), Some("mm/h"));
    let error = output::read_dbz(&path, None, None, &Limits::default()).unwrap_err();
    assert!(matches!(error, radiust_core::errors::CoreError::UnitMismatch { .. }));
}

#[test]
fn native_zarr_read_rejects_a_mismatched_requested_time() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("native.zarr");
    let field = RadarField {
        name: "reflectivity".into(),
        values: vec![12.0, 18.0],
        shape: vec![1, 2],
        quality: vec![0, 0],
        units: Some("dBZ".into()),
        valid_time: "2026-09-18T03:42:00Z".into(),
        grid: Grid {
            shape: vec![1, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![120.0, 120.1],
            y: vec![24.0],
            affine: Some([119.95, 0.1, 0.0, 24.05, 0.0, -0.1]),
        },
        provenance: vec!["native Zarr fixture".into()],
    };
    output::zarr::write_field(&field, &path, &Limits::default()).unwrap();

    let result =
        output::read_raster_result(&path, None, Some("2000-01-01T00:00:00Z"), &Limits::default());

    assert!(matches!(
        result,
        Err(radiust_core::errors::CoreError::Storage(message))
            if message.contains("requested time does not match Zarr provenance")
    ));
}
