//! Native writers for rendered radar outputs.

use crate::errors::{CoreError, CoreResult, ProviderError};
use crate::identity::{ProcessingSpec, raster_commit_identity};
use crate::limits::Limits;
use crate::model::{FrameRef, RadarField, parse_utc_time};
use crate::raster::{
    FileComponentDigest, GeometryEvidence, ModeInfo, NumericFileIdentity, NumericReadReceipt,
    ProcessingRecord, RasterInput, RasterResult, RasterResultData, UpstreamProvenance,
};
use crate::storage::{LocalCommitResult, LocalRasterCommitRequest, LocalStore, StagedArtifact};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub mod geotiff;
pub mod netcdf;
pub mod png;
pub mod zarr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterOutputProfile {
    Pixel,
    Native,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterOutputFormat {
    Png,
    NetCdf,
    Zarr,
    GeoTiff,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GeographicOutputRequest {
    pub bbox: bool,
    pub resolution: bool,
    pub regrid: bool,
}

pub fn preflight_raster_output(
    profile: RasterOutputProfile,
    format: RasterOutputFormat,
    geometry: Option<&GeometryEvidence>,
    geographic: GeographicOutputRequest,
) -> CoreResult<()> {
    let needs_geography = format == RasterOutputFormat::GeoTiff
        || geographic.bbox
        || geographic.resolution
        || geographic.regrid;
    if profile == RasterOutputProfile::Pixel {
        if needs_geography
            && !geometry.is_some_and(|value| {
                value.mapping_complete
                    && value.crs.as_deref().is_some_and(|crs| !crs.trim().is_empty())
            })
        {
            return Err(CoreError::Provider(ProviderError::InvalidGrid));
        }
        if format == RasterOutputFormat::GeoTiff && geometry.is_none() {
            return Err(CoreError::Provider(ProviderError::InvalidGrid));
        }
    }
    Ok(())
}

/// Read a supported numeric file as a new, receipt-bound input. Metadata
/// embedded by an earlier Radiust run is retained only as upstream provenance.
pub fn read_raster_result(
    path: impl AsRef<Path>,
    variable: Option<&str>,
    valid_time: Option<&str>,
    limits: &Limits,
) -> CoreResult<RasterResult> {
    read_raster_result_with_hook(path.as_ref(), variable, valid_time, limits, || {})
}

fn read_raster_result_with_hook<F>(
    path: &Path,
    variable: Option<&str>,
    valid_time: Option<&str>,
    limits: &Limits,
    after_decode: F,
) -> CoreResult<RasterResult>
where
    F: FnOnce(),
{
    let format = inspect_numeric_format(path)?;
    let before = numeric_file_receipt(path, format, limits)?;
    let (data, variable_name, units, selected_time, geometry, processing, upstream) = match format {
        "netcdf" if netcdf::is_pixel_dbz_file(path)? => {
            if variable.is_some_and(|value| value != "reflectivity") {
                return Err(CoreError::Storage(
                    "pixel dBZ NetCDF contains only reflectivity".into(),
                ));
            }
            let field = netcdf::read_pixel_dbz(path, limits)?;
            if valid_time
                .is_some_and(|requested| !times_match(field.valid_time.as_deref(), requested))
            {
                return Err(CoreError::Storage(
                    "requested time does not match the pixel dBZ file".into(),
                ));
            }
            let processing = field.processing.clone();
            let upstream = Some(UpstreamProvenance {
                input_identity: Some(processing.input_identity.clone()),
                processing_record: serde_json::to_value(&processing).ok(),
                manifest_output_id: None,
            });
            let time = field.valid_time.clone();
            let geometry = field.geometry.clone();
            let data = RasterResultData::Pixel(Arc::new(field));
            (
                data,
                "reflectivity".to_owned(),
                Some("dBZ".to_owned()),
                time,
                geometry,
                processing,
                upstream,
            )
        }
        "zarr" if zarr::is_pixel_dbz_store(path, limits)? => {
            if variable.is_some_and(|value| value != "reflectivity") {
                return Err(CoreError::Storage("pixel dBZ Zarr contains only reflectivity".into()));
            }
            let field = zarr::read_pixel_dbz(path, limits)?;
            if valid_time
                .is_some_and(|requested| !times_match(field.valid_time.as_deref(), requested))
            {
                return Err(CoreError::Storage(
                    "requested time does not match the pixel dBZ file".into(),
                ));
            }
            let processing = field.processing.clone();
            let upstream = Some(UpstreamProvenance {
                input_identity: Some(processing.input_identity.clone()),
                processing_record: serde_json::to_value(&processing).ok(),
                manifest_output_id: None,
            });
            let time = field.valid_time.clone();
            let geometry = field.geometry.clone();
            let data = RasterResultData::Pixel(Arc::new(field));
            (
                data,
                "reflectivity".to_owned(),
                Some("dBZ".to_owned()),
                time,
                geometry,
                processing,
                upstream,
            )
        }
        "geotiff" if geotiff::is_pixel_dbz_file(path, limits)? => {
            if variable.is_some_and(|value| value != "reflectivity") {
                return Err(CoreError::Storage(
                    "pixel dBZ GeoTIFF contains only reflectivity".into(),
                ));
            }
            let field = geotiff::read_pixel_dbz(path, limits)?;
            if valid_time
                .is_some_and(|requested| !times_match(field.valid_time.as_deref(), requested))
            {
                return Err(CoreError::Storage(
                    "requested time does not match the pixel dBZ GeoTIFF".into(),
                ));
            }
            let processing = field.processing.clone();
            let upstream = Some(UpstreamProvenance {
                input_identity: Some(processing.input_identity.clone()),
                processing_record: serde_json::to_value(&processing).ok(),
                manifest_output_id: None,
            });
            let time = field.valid_time.clone();
            let geometry = field.geometry.clone();
            let data = RasterResultData::Pixel(Arc::new(field));
            (
                data,
                "reflectivity".to_owned(),
                Some("dBZ".to_owned()),
                time,
                geometry,
                processing,
                upstream,
            )
        }
        "netcdf" => {
            let selected = netcdf::select_data_variable(path, variable)?;
            let field = netcdf::read_selected_field(path, &selected, valid_time, limits)?;
            let geometry = geometry_from_field(&field);
            let time = Some(field.valid_time.clone());
            let units = field.units.clone();
            let upstream = Some(UpstreamProvenance {
                input_identity: None,
                processing_record: Some(json!({"provenance": &field.provenance})),
                manifest_output_id: None,
            });
            (
                RasterResultData::Native(Arc::new(field)),
                selected,
                units,
                time,
                geometry,
                native_processing_record(),
                upstream,
            )
        }
        "zarr" => {
            let field = zarr::read_selected_field(path, variable, limits)?;
            if valid_time.is_some_and(|requested| !times_match(Some(&field.valid_time), requested))
            {
                return Err(CoreError::Storage(
                    "requested time does not match Zarr provenance".into(),
                ));
            }
            let variable_name = field.name.clone();
            let geometry = geometry_from_field(&field);
            let time = Some(field.valid_time.clone());
            let units = field.units.clone();
            let upstream = Some(UpstreamProvenance {
                input_identity: None,
                processing_record: Some(json!({"provenance": &field.provenance})),
                manifest_output_id: None,
            });
            (
                RasterResultData::Native(Arc::new(field)),
                variable_name,
                units,
                time,
                geometry,
                native_processing_record(),
                upstream,
            )
        }
        "geotiff" => {
            let field = geotiff::read_selected_field(path, limits)?;
            if variable.is_some_and(|requested| requested != field.name) {
                return Err(CoreError::Storage(
                    "requested variable does not match GeoTIFF provenance".into(),
                ));
            }
            if valid_time.is_some_and(|requested| !times_match(Some(&field.valid_time), requested))
            {
                return Err(CoreError::Storage(
                    "requested time does not match GeoTIFF provenance".into(),
                ));
            }
            let variable_name = field.name.clone();
            let geometry = geometry_from_field(&field);
            let time = Some(field.valid_time.clone());
            let units = field.units.clone();
            let upstream = Some(UpstreamProvenance {
                input_identity: None,
                processing_record: Some(json!({"provenance": &field.provenance})),
                manifest_output_id: None,
            });
            (
                RasterResultData::Native(Arc::new(field)),
                variable_name,
                units,
                time,
                geometry,
                native_processing_record(),
                upstream,
            )
        }
        _ => return Err(CoreError::Storage("numeric input format is unsupported".into())),
    };
    after_decode();
    let after = numeric_file_receipt(path, format, limits)?;
    if before != after {
        return Err(CoreError::Storage("numeric input changed while it was being read".into()));
    }
    let selection = json!({
        "variable": &variable_name,
        "valid_time": &selected_time,
    });
    let identity = NumericFileIdentity {
        kind: "local_numeric".into(),
        format: format.into(),
        content_digest: before.content_digest.clone(),
        variable: variable_name.clone(),
        selection: selection.clone(),
        valid_time: selected_time.clone(),
        geometry: geometry.clone(),
    };
    identity.validate().map_err(|error| CoreError::Storage(error.into()))?;
    let mut processing = processing;
    if processing.method == "file_native" {
        processing.input_identity = serde_json::to_value(&identity)
            .map_err(|_| CoreError::Storage("numeric identity could not be serialized".into()))?;
    }
    let read_receipt = NumericReadReceipt {
        content_digest: before.content_digest,
        format: format.into(),
        size_bytes: before.size_bytes,
        variable: variable_name.clone(),
        components: before.components,
        selection,
        units: units.clone(),
        validated_schema: matches!(&data, RasterResultData::Pixel(_))
            .then(|| "pixel-dbz-v1".into()),
    };
    read_receipt.validate().map_err(|error| CoreError::Storage(error.into()))?;
    let input = RasterInput::NumericFile { identity, read_receipt, upstream_provenance: upstream };
    let mode_info = ModeInfo {
        requested: None,
        actual: Some(
            if matches!(&data, RasterResultData::Pixel(_)) { "dbz" } else { "scientific" }.into(),
        ),
        variable: Some(variable_name),
        units,
        method: Some(
            if matches!(&data, RasterResultData::Pixel(_)) { "file_dbz" } else { "file_native" }
                .into(),
        ),
        encoding: None,
        rule_version: None,
        range_policy: None,
        clipped_pixel_count: processing.clipped_pixel_count,
        valid_clipped_pixel_count: processing.valid_clipped_pixel_count,
        time_status: Some(if selected_time.is_some() { "known" } else { "unknown" }.into()),
        geolocation: Some(
            if geometry.as_ref().is_some_and(|value| value.mapping_complete) {
                "georeferenced"
            } else {
                "pixel_coordinates"
            }
            .into(),
        ),
        limitations: processing.limitations.clone(),
    };
    Ok(RasterResult { input, data, processing, mode_info })
}

/// Read a dBZ raster, rejecting numeric variables whose stored units do not
/// establish reflectivity.
pub fn read_dbz(
    path: impl AsRef<Path>,
    variable: Option<&str>,
    valid_time: Option<&str>,
    limits: &Limits,
) -> CoreResult<RasterResult> {
    let result = read_raster_result(path, variable, valid_time, limits)?;
    match &result.data {
        RasterResultData::Pixel(_) => Ok(result),
        RasterResultData::Native(field)
            if field.name == "reflectivity" && field.units.as_deref() == Some("dBZ") =>
        {
            Ok(result)
        }
        RasterResultData::Native(field) => Err(CoreError::UnitMismatch {
            variable: field.name.clone(),
            units: field.units.clone(),
        }),
        RasterResultData::NativeDataset { owner, index } => {
            let field = owner
                .fields
                .get(*index)
                .ok_or_else(|| CoreError::Storage("selected dataset field is missing".into()))?;
            if field.name == "reflectivity" && field.units.as_deref() == Some("dBZ") {
                Ok(result)
            } else {
                Err(CoreError::UnitMismatch {
                    variable: field.name.clone(),
                    units: field.units.clone(),
                })
            }
        }
    }
}

/// Encode and transactionally commit a raster without manufacturing a
/// FrameRef. Local gray and numeric-file inputs use their verified receipts.
pub fn write_raster_result(
    result: &RasterResult,
    output_root: impl AsRef<Path>,
    output_name: &str,
    format: &str,
    options: &Value,
    overwrite: bool,
    limits: &Limits,
) -> CoreResult<LocalCommitResult> {
    write_raster_result_cancellable(
        result,
        output_root,
        output_name,
        format,
        options,
        overwrite,
        limits,
        &CancellationToken::new(),
    )
}

pub fn write_raster_result_cancellable(
    result: &RasterResult,
    output_root: impl AsRef<Path>,
    output_name: &str,
    format: &str,
    options: &Value,
    overwrite: bool,
    limits: &Limits,
    cancellation: &CancellationToken,
) -> CoreResult<LocalCommitResult> {
    write_raster_result_with_ref_cancellable(
        result,
        output_root,
        output_name,
        format,
        options,
        overwrite,
        limits,
        None,
        cancellation,
    )
}

pub fn write_raster_result_with_ref_cancellable(
    result: &RasterResult,
    output_root: impl AsRef<Path>,
    output_name: &str,
    format: &str,
    options: &Value,
    overwrite: bool,
    limits: &Limits,
    explicit_ref: Option<FrameRef>,
    cancellation: &CancellationToken,
) -> CoreResult<LocalCommitResult> {
    write_raster_result_inner(
        result,
        output_root,
        output_name,
        format,
        options,
        overwrite,
        limits,
        explicit_ref,
        None,
        cancellation,
    )
}

pub fn write_raster_result_with_raw_cancellable(
    result: &RasterResult,
    raw: &crate::model::RawFrame,
    output_root: impl AsRef<Path>,
    output_name: &str,
    format: &str,
    options: &Value,
    overwrite: bool,
    limits: &Limits,
    cancellation: &CancellationToken,
) -> CoreResult<LocalCommitResult> {
    write_raster_result_inner(
        result,
        output_root,
        output_name,
        format,
        options,
        overwrite,
        limits,
        None,
        Some(raw),
        cancellation,
    )
}

fn write_raster_result_inner(
    result: &RasterResult,
    output_root: impl AsRef<Path>,
    output_name: &str,
    format: &str,
    options: &Value,
    overwrite: bool,
    limits: &Limits,
    explicit_ref: Option<FrameRef>,
    raw: Option<&crate::model::RawFrame>,
    cancellation: &CancellationToken,
) -> CoreResult<LocalCommitResult> {
    if !options.is_object() && !options.is_null() {
        return Err(CoreError::Storage("writer options must be a JSON object".into()));
    }
    let profile = match &result.data {
        RasterResultData::Pixel(_) => RasterOutputProfile::Pixel,
        RasterResultData::Native(_) | RasterResultData::NativeDataset { .. } => {
            RasterOutputProfile::Native
        }
    };
    let output_format = match format {
        "png" => RasterOutputFormat::Png,
        "netcdf" => RasterOutputFormat::NetCdf,
        "zarr" => RasterOutputFormat::Zarr,
        "geotiff" => RasterOutputFormat::GeoTiff,
        _ => return Err(CoreError::Storage("output format is unsupported".into())),
    };
    let geometry = match &result.data {
        RasterResultData::Pixel(field) => field.geometry.clone(),
        RasterResultData::Native(field) => geometry_from_field(field),
        RasterResultData::NativeDataset { owner, index } => {
            owner.fields.get(*index).and_then(geometry_from_field)
        }
    };
    preflight_raster_output(
        profile,
        output_format,
        geometry.as_ref(),
        GeographicOutputRequest::default(),
    )?;
    let store = LocalStore::new(output_root, limits.clone())?;
    let output_path = Path::new(output_name);
    if !crate::storage::manifest::is_safe_relative_path(output_name) {
        return Err(CoreError::Storage("output name must be a safe relative path".into()));
    }
    let leaf = output_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CoreError::Storage("output filename is invalid".into()))?;
    let workspace = tempfile::Builder::new()
        .prefix(".radiust-raster-write-")
        .tempdir_in(store.root())
        .map_err(|_| {
            CoreError::Temporary("raster output staging directory could not be created".into())
        })?;
    let encoded_path = workspace.path().join(leaf);
    let mut processing_options = json!({
        "mode": &result.mode_info.actual,
        "processing_record": &result.processing,
        "writer_options": options,
        "raster_profile": if profile == RasterOutputProfile::Pixel { "pixel-dbz-v1" } else { "native" },
    });
    if let Some(map) = processing_options.as_object_mut() {
        map.insert("processing_schema_version".into(), json!(1));
    }
    let mut processing_spec = ProcessingSpec {
        output_kind: result.mode_info.actual.clone().unwrap_or_else(|| "decoded".into()),
        format: format.into(),
        variable: result.mode_info.variable.clone(),
        grid: if profile == RasterOutputProfile::Pixel { "pixel" } else { "native" }.into(),
        options: processing_options,
        ..ProcessingSpec::default()
    };
    processing_spec.decoder_version =
        result.processing.decoder_version.clone().unwrap_or_else(|| "1".into());
    processing_spec.resource_version =
        result.processing.quality_policy_version.clone().unwrap_or_else(|| "1".into());
    processing_spec.encoder_version =
        if profile == RasterOutputProfile::Pixel { "pixel-dbz-v1" } else { "1" }.into();
    let identity_receipt = raster_commit_identity(&result.input, &processing_spec)
        .map_err(|error| CoreError::Storage(error.to_string()))?;
    let output_files = match (&result.data, format) {
        (RasterResultData::Pixel(field), "png") => {
            png::write_pixel_dbz_png(field, &encoded_path, options, limits)?
        }
        (RasterResultData::Pixel(field), "netcdf") => {
            vec![netcdf::write_pixel_dbz(field, &encoded_path, limits)?]
        }
        (RasterResultData::Pixel(field), "zarr") => {
            zarr::write_pixel_dbz(field, &encoded_path, limits)?;
            zarr::collect_output_files(&encoded_path)?.into_iter().map(|(_, path)| path).collect()
        }
        (RasterResultData::Pixel(field), "geotiff") => {
            geotiff::write_pixel_dbz(field, &encoded_path, limits)?
        }
        (RasterResultData::Native(field), "png") => png::write_png(field, &encoded_path, options)?,
        (RasterResultData::Native(field), "netcdf") => {
            vec![netcdf::write_field(field, &encoded_path, limits)?]
        }
        (RasterResultData::Native(field), "geotiff") => {
            geotiff::write_field(field, &encoded_path, limits)?
        }
        (RasterResultData::Native(field), "zarr") => {
            zarr::write_field(field, &encoded_path, limits)?;
            zarr::collect_output_files(&encoded_path)?.into_iter().map(|(_, path)| path).collect()
        }
        (RasterResultData::NativeDataset { owner, index }, "png") => png::write_png(
            owner
                .fields
                .get(*index)
                .ok_or_else(|| CoreError::Storage("selected dataset field is missing".into()))?,
            &encoded_path,
            options,
        )?,
        (RasterResultData::NativeDataset { owner, index }, "netcdf") => vec![netcdf::write_field(
            owner
                .fields
                .get(*index)
                .ok_or_else(|| CoreError::Storage("selected dataset field is missing".into()))?,
            &encoded_path,
            limits,
        )?],
        (RasterResultData::NativeDataset { owner, index }, "geotiff") => geotiff::write_field(
            owner
                .fields
                .get(*index)
                .ok_or_else(|| CoreError::Storage("selected dataset field is missing".into()))?,
            &encoded_path,
            limits,
        )?,
        (RasterResultData::NativeDataset { owner, index }, "zarr") => {
            zarr::write_field(
                owner.fields.get(*index).ok_or_else(|| {
                    CoreError::Storage("selected dataset field is missing".into())
                })?,
                &encoded_path,
                limits,
            )?;
            zarr::collect_output_files(&encoded_path)?.into_iter().map(|(_, path)| path).collect()
        }
        (RasterResultData::Pixel(_), _) => {
            return Err(CoreError::Provider(ProviderError::InvalidGrid));
        }
        (RasterResultData::Native(_) | RasterResultData::NativeDataset { .. }, _) => {
            return Err(CoreError::Storage("output format is unsupported".into()));
        }
    };
    let mut artifacts = Vec::with_capacity(output_files.len());
    let mut total_size = 0_u64;
    for path in output_files {
        if path.is_dir() {
            return Err(CoreError::Storage(
                "writer output enumeration returned a directory".into(),
            ));
        }
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            CoreError::Storage("staged raster artifact could not be inspected".into())
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(CoreError::Storage("staged raster artifact is not a regular file".into()));
        }
        total_size = total_size
            .checked_add(metadata.len())
            .ok_or_else(|| CoreError::ResourceLimit("raster output size overflows".into()))?;
        limits.validate_bytes(metadata.len(), total_size)?;
        if total_size > limits.max_temp_bytes / 3 {
            return Err(CoreError::ResourceLimit(
                "raster output exceeds transactional temporary-storage budget".into(),
            ));
        }
        let relative_file = if format == "zarr" {
            let relative = path.strip_prefix(&encoded_path).map_err(|_| {
                CoreError::Storage("Zarr output path escaped its staging store".into())
            })?;
            format!(
                "{output_name}/{}",
                relative
                    .to_str()
                    .ok_or_else(|| CoreError::Storage("Zarr output key is invalid".into()))?
                    .replace('\\', "/")
            )
        } else if path == encoded_path {
            output_name.to_owned()
        } else {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| CoreError::Storage("raster sidecar name is invalid".into()))?;
            match output_path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
                Some(parent) => format!("{}/{name}", parent.to_string_lossy().replace('\\', "/")),
                None => name.to_owned(),
            }
        };
        let name = relative_file
            .strip_prefix(output_path.parent().and_then(Path::to_str).unwrap_or(""))
            .unwrap_or(&relative_file)
            .trim_start_matches('/')
            .to_owned();
        let is_metadata = relative_file.ends_with(".json")
            || relative_file.ends_with(".zattrs")
            || relative_file.ends_with(".zarray")
            || relative_file.ends_with(".zgroup")
            || relative_file.ends_with(".zmetadata");
        let media_type = if relative_file.ends_with(".png") {
            "image/png"
        } else if relative_file.ends_with(".nc") {
            "application/x-netcdf"
        } else if relative_file.ends_with(".tif") {
            "image/tiff"
        } else if is_metadata {
            "application/json"
        } else {
            "application/octet-stream"
        };
        artifacts.push(StagedArtifact {
            name,
            relative_uri: relative_file,
            role: if is_metadata { "metadata" } else { "data" }.into(),
            media_type: media_type.into(),
            source: path,
        });
    }
    if let Some(raw) = raw {
        append_raster_raw_artifacts(
            workspace.path(),
            output_name,
            raw,
            &result.input,
            limits,
            &mut artifacts,
            &mut total_size,
        )?;
    }
    store.commit_raster_cancellable(
        LocalRasterCommitRequest {
            input: result.input.clone(),
            identity_receipt,
            explicit_ref,
            processing_spec,
            output_name: output_name.into(),
            artifacts,
            raw_complete: raw.is_some(),
            overwrite,
        },
        cancellation,
    )
}

fn append_raster_raw_artifacts(
    workspace: &Path,
    output_name: &str,
    raw: &crate::model::RawFrame,
    input: &RasterInput,
    limits: &Limits,
    artifacts: &mut Vec<StagedArtifact>,
    total_size: &mut u64,
) -> CoreResult<()> {
    let RasterInput::Source { frame, acquisition_receipt, .. } = input else {
        return Err(CoreError::Storage(
            "raw artifacts can only be attached to a source raster".into(),
        ));
    };
    if acquisition_receipt != &raw.public_receipt()
        || crate::identity::logical_id(frame).ok() != crate::identity::logical_id(&raw.frame).ok()
    {
        return Err(CoreError::Storage(
            "raw artifacts do not match the source raster receipt".into(),
        ));
    }
    if raw.artifacts.is_empty() {
        return Err(CoreError::Storage("source raw receipt contains no artifacts".into()));
    }

    let group = Path::new(output_name)
        .parent()
        .and_then(Path::to_str)
        .filter(|value| !value.is_empty())
        .map(|value| value.replace('\\', "/"));
    let relative = |name: &str| match &group {
        Some(group) => format!("{group}/{name}"),
        None => name.to_owned(),
    };
    for artifact in &raw.artifacts {
        if !crate::storage::manifest::is_safe_relative_path(&artifact.receipt.name) {
            return Err(CoreError::Storage("raw artifact name is unsafe".into()));
        }
        let (size, digest) =
            crate::storage::manifest::hash_file(artifact.path.as_ref(), limits.max_artifact_bytes)?;
        if size != artifact.receipt.size_bytes || digest != artifact.receipt.sha256 {
            return Err(CoreError::Storage("raw artifact does not match its receipt".into()));
        }
        *total_size = total_size
            .checked_add(size)
            .ok_or_else(|| CoreError::ResourceLimit("raw output size overflow".into()))?;
        limits.validate_bytes(size, *total_size)?;
        artifacts.push(StagedArtifact {
            name: format!("raw/{}", artifact.receipt.name),
            relative_uri: relative(&format!("raw/{}", artifact.receipt.name)),
            role: "data".into(),
            media_type: artifact.receipt.media_type.clone(),
            source: artifact.path.to_path_buf(),
        });
    }
    let manifest_path = workspace.join("raw-manifest.json");
    fs::write(&manifest_path, crate::raw_manifest::encode(raw)?)
        .map_err(|error| CoreError::Temporary(format!("raw manifest staging failed: {error}")))?;
    let (size, _) = crate::storage::manifest::hash_file(&manifest_path, limits.max_artifact_bytes)?;
    *total_size = total_size
        .checked_add(size)
        .ok_or_else(|| CoreError::ResourceLimit("raw output size overflow".into()))?;
    limits.validate_bytes(size, *total_size)?;
    artifacts.push(StagedArtifact {
        name: "raw-manifest.json".into(),
        relative_uri: relative("raw-manifest.json"),
        role: "metadata".into(),
        media_type: "application/json".into(),
        source: manifest_path,
    });
    if *total_size > limits.max_temp_bytes / 3 {
        return Err(CoreError::ResourceLimit(
            "raw and decoded output plus transactional copies exceed the temporary-storage budget"
                .into(),
        ));
    }
    Ok(())
}

fn native_processing_record() -> ProcessingRecord {
    ProcessingRecord {
        schema_version: 1,
        method: "file_native".into(),
        input_identity: Value::Null,
        encoding_basis: None,
        range_policy: None,
        decoder_version: None,
        quality_policy_version: None,
        formula: None,
        quantization_step: None,
        alpha_bit_depth: None,
        steps: vec![],
        limitations: vec![],
        clipped_pixel_count: None,
        valid_clipped_pixel_count: None,
        upstream: None,
    }
}

fn times_match(stored: Option<&str>, requested: &str) -> bool {
    stored
        .and_then(|value| parse_utc_time(value).ok())
        .zip(parse_utc_time(requested).ok())
        .is_some_and(|(left, right)| left == right)
}

pub fn geometry_from_field(field: &RadarField) -> Option<GeometryEvidence> {
    let height = *field.shape.get(field.shape.len().checked_sub(2)?)?;
    let width = *field.shape.last()?;
    let crs = field.grid.crs.clone();
    Some(GeometryEvidence {
        source: "numeric_file".into(),
        crs: crs.clone(),
        x: field.grid.x.clone(),
        y: field.grid.y.clone(),
        affine: field.grid.affine,
        mapping_complete: crs.as_ref().is_some_and(|value| !value.trim().is_empty())
            && field.grid.x.len() == width
            && field.grid.y.len() == height,
    })
}

fn inspect_numeric_format(path: &Path) -> CoreResult<&'static str> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| CoreError::Storage("numeric input could not be inspected".into()))?;
    if metadata.file_type().is_symlink() {
        return Err(CoreError::Storage("numeric input cannot be a symbolic link".into()));
    }
    if metadata.is_dir() {
        return Ok("zarr");
    }
    if !metadata.is_file() {
        return Err(CoreError::Storage(
            "numeric input must be a regular file or Zarr directory".into(),
        ));
    }
    match path.extension().and_then(|value| value.to_str()).map(str::to_ascii_lowercase).as_deref()
    {
        Some("nc" | "netcdf") => Ok("netcdf"),
        Some("tif" | "tiff") => Ok("geotiff"),
        _ => Err(CoreError::Storage("numeric input extension is unsupported".into())),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct HashedInput {
    content_digest: String,
    size_bytes: u64,
    components: Vec<FileComponentDigest>,
}

fn numeric_file_receipt(path: &Path, format: &str, limits: &Limits) -> CoreResult<HashedInput> {
    match format {
        "netcdf" => {
            let (size, digest) =
                crate::storage::manifest::hash_file(path, limits.max_artifact_bytes)?;
            Ok(HashedInput { content_digest: digest, size_bytes: size, components: vec![] })
        }
        "geotiff" => {
            let files = geotiff::component_paths(path, limits)?;
            let mut components = files
                .iter()
                .map(|(role, component_path)| {
                    let (size_bytes, sha256) = crate::storage::manifest::hash_file(
                        component_path,
                        limits.max_artifact_bytes,
                    )?;
                    let relative_key = component_path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .ok_or_else(|| {
                            CoreError::Storage("GeoTIFF component name is invalid".into())
                        })?
                        .to_owned();
                    Ok(FileComponentDigest {
                        role: (*role).into(),
                        relative_key,
                        size_bytes,
                        sha256,
                    })
                })
                .collect::<CoreResult<Vec<_>>>()?;
            components.sort_by(|left, right| {
                (&left.role, &left.relative_key).cmp(&(&right.role, &right.relative_key))
            });
            let size_bytes = components
                .iter()
                .try_fold(0_u64, |total, item| total.checked_add(item.size_bytes))
                .ok_or_else(|| {
                    CoreError::ResourceLimit("GeoTIFF component size overflows".into())
                })?;
            limits.validate_bytes(size_bytes, size_bytes)?;
            let content_digest = digest_components(&components)?;
            limits.validate_bytes(size_bytes, size_bytes)?;
            Ok(HashedInput { content_digest, size_bytes, components })
        }
        "zarr" => {
            let root = fs::canonicalize(path).map_err(|_| {
                CoreError::Storage("Zarr input directory could not be resolved".into())
            })?;
            let mut components = Vec::new();
            collect_numeric_tree(&root, &root, limits, &mut components)?;
            components.sort_by(|left, right| {
                (&left.role, &left.relative_key).cmp(&(&right.role, &right.relative_key))
            });
            let size_bytes = components
                .iter()
                .try_fold(0_u64, |total, item| total.checked_add(item.size_bytes))
                .ok_or_else(|| CoreError::ResourceLimit("Zarr component size overflows".into()))?;
            let content_digest = digest_components(&components)?;
            Ok(HashedInput { content_digest, size_bytes, components })
        }
        _ => Err(CoreError::Storage("numeric input format is unsupported".into())),
    }
}

fn collect_numeric_tree(
    root: &Path,
    directory: &Path,
    limits: &Limits,
    output: &mut Vec<FileComponentDigest>,
) -> CoreResult<()> {
    for entry in fs::read_dir(directory)
        .map_err(|_| CoreError::Storage("Zarr input directory could not be read".into()))?
    {
        let entry =
            entry.map_err(|_| CoreError::Storage("Zarr input entry could not be read".into()))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| CoreError::Storage("Zarr input entry could not be inspected".into()))?;
        if metadata.file_type().is_symlink() {
            return Err(CoreError::Storage("Zarr input contains a symbolic link".into()));
        }
        if metadata.is_dir() {
            collect_numeric_tree(root, &path, limits, output)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(CoreError::Storage(
                "Zarr input contains an unsupported filesystem entry".into(),
            ));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| CoreError::Storage("Zarr component escapes its root".into()))?;
        let relative_key = relative
            .to_str()
            .ok_or_else(|| CoreError::Storage("Zarr component path is not valid UTF-8".into()))?
            .replace('\\', "/");
        if relative_key.starts_with('/')
            || relative_key.split('/').any(|part| part.is_empty() || part == "..")
        {
            return Err(CoreError::Storage("Zarr input contains an unsafe component path".into()));
        }
        let (size_bytes, sha256) =
            crate::storage::manifest::hash_file(&path, limits.max_artifact_bytes)?;
        output.push(FileComponentDigest {
            role: "zarr_component".into(),
            relative_key,
            size_bytes,
            sha256,
        });
    }
    Ok(())
}

fn digest_components(components: &[FileComponentDigest]) -> CoreResult<String> {
    let value = serde_json::to_value(components)
        .map_err(|_| CoreError::Storage("numeric input receipt could not be serialized".into()))?;
    crate::identity::digest(&value).map_err(|error| CoreError::Storage(error.to_string()))
}

#[cfg(test)]
mod raster_preflight_tests {
    use super::*;
    use crate::raster::{AlphaPlane, GeometryEvidence, PixelDbzField, ProcessingRecord};
    use serde_json::json;

    fn mutation_test_field(geometry: bool) -> PixelDbzField {
        PixelDbzField {
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
            geometry: geometry.then(|| GeometryEvidence {
                source: "verified-test-mapping".into(),
                crs: Some("EPSG:4326".into()),
                x: vec![120.0, 121.0],
                y: vec![24.0, 23.0],
                affine: Some([119.5, 1.0, 0.0, 24.5, 0.0, -1.0]),
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
        }
    }

    #[test]
    fn pixel_netcdf_allows_pixel_coordinates_but_geotiff_requires_geometry() {
        assert!(
            preflight_raster_output(
                RasterOutputProfile::Pixel,
                RasterOutputFormat::NetCdf,
                None,
                GeographicOutputRequest::default(),
            )
            .is_ok()
        );
        assert!(matches!(
            preflight_raster_output(
                RasterOutputProfile::Pixel,
                RasterOutputFormat::GeoTiff,
                None,
                GeographicOutputRequest::default(),
            ),
            Err(CoreError::Provider(ProviderError::InvalidGrid))
        ));
    }

    #[test]
    fn pixel_geographic_operations_require_a_complete_mapping() {
        let incomplete = GeometryEvidence {
            source: "fixture".into(),
            crs: Some("EPSG:4326".into()),
            x: vec![],
            y: vec![],
            affine: None,
            mapping_complete: false,
        };
        assert!(matches!(
            preflight_raster_output(
                RasterOutputProfile::Pixel,
                RasterOutputFormat::Png,
                Some(&incomplete),
                GeographicOutputRequest { bbox: true, ..Default::default() },
            ),
            Err(CoreError::Provider(ProviderError::InvalidGrid))
        ));
    }

    #[test]
    fn numeric_reader_rejects_content_changed_after_decode_for_all_file_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let limits = Limits::default();

        let netcdf_path = temp.path().join("pixel.nc");
        netcdf::write_pixel_dbz(&mutation_test_field(false), &netcdf_path, &limits).unwrap();
        let netcdf_error = read_raster_result_with_hook(&netcdf_path, None, None, &limits, || {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(&netcdf_path)
                .unwrap()
                .write_all(b"changed while reading")
                .unwrap();
        })
        .unwrap_err();

        let zarr_path = temp.path().join("pixel.zarr");
        zarr::write_pixel_dbz(&mutation_test_field(false), &zarr_path, &limits).unwrap();
        let zarr_error = read_raster_result_with_hook(&zarr_path, None, None, &limits, || {
            std::fs::write(zarr_path.join("late-component"), b"changed while reading").unwrap()
        })
        .unwrap_err();

        let geotiff_path = temp.path().join("pixel.tif");
        geotiff::write_pixel_dbz(&mutation_test_field(true), &geotiff_path, &limits).unwrap();
        let geotiff_error =
            read_raster_result_with_hook(&geotiff_path, None, None, &limits, || {
                std::fs::write(temp.path().join("pixel_quality.tif"), b"changed while reading")
                    .unwrap();
            })
            .unwrap_err();

        for error in [netcdf_error, zarr_error, geotiff_error] {
            assert!(
                error.to_string().contains("numeric input changed while it was being read"),
                "{error}"
            );
        }
    }
}

/// Encode only; callers own staging, identity, naming and transactional publication.
pub(crate) fn encode_science_files(
    field: &RadarField,
    path: &Path,
    format: &str,
    options: &Value,
    limits: &Limits,
) -> CoreResult<Vec<(String, PathBuf)>> {
    let paths = match format {
        "png" => png::write_png(field, path, options)?,
        "netcdf" => vec![netcdf::write_field(field, path, limits)?],
        "geotiff" => geotiff::write_field(field, path, limits)?,
        "zarr" => {
            zarr::write_field(field, path, limits)?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| CoreError::Storage("Zarr artifact path is invalid".into()))?;
            return Ok(zarr::collect_output_files(path)?
                .into_iter()
                .map(|(relative, path)| (format!("{name}/{relative}"), path))
                .collect());
        }
        _ => return Err(CoreError::Storage("unsupported science output format".into())),
    };
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| CoreError::Storage("encoded artifact path is invalid".into()))?;
            Ok((name.to_owned(), path))
        })
        .collect()
}
