use super::Context;
use crate::report;
use radiust_core::download::FetchErrorPolicy;
use radiust_core::engine::{Engine, EngineError};
use radiust_core::error_contract::ErrorReport;
use radiust_core::errors::CoreError;
use radiust_core::output::{GeographicOutputRequest, RasterOutputFormat, RasterOutputProfile};
use radiust_core::raster::{RasterInput, RasterResultData};
use radiust_core::source::SourceRegistry;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) struct Args {
    pub source: Option<String>,
    pub file: Option<PathBuf>,
    pub dry_run: bool,
    pub raw_only: bool,
    pub raw: bool,
    pub dbz: bool,
    pub overwrite: bool,
    pub on_error: String,
    pub output: Option<PathBuf>,
    pub output_template: Option<String>,
    pub cache_dir: Option<PathBuf>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub endpoint: Option<String>,
    pub no_cache: bool,
    pub variable: Option<String>,
    pub bbox: Option<String>,
    pub grid: String,
    pub resolution: Option<f64>,
    pub resampling: String,
    pub format: Option<String>,
    pub product: Option<String>,
    pub stations: Vec<String>,
    pub latest: bool,
    pub at: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub base_time: Option<String>,
    pub max_age_secs: Option<f64>,
}

pub(crate) fn run(mut args: Args, context: &Context<'_>) -> Result<u8, String> {
    if let Some(file) = args.file.take() {
        return run_local_dbz(args, file, context);
    }
    let Some(source) = args.source.take() else {
        return local_error(context, "invalid_query", "provide SOURCE or --file PATH", "validate");
    };
    let query = crate::build_query(
        vec![source],
        args.product,
        args.stations,
        args.latest,
        args.at,
        args.start,
        args.end,
        args.base_time,
        args.max_age_secs,
    )?;
    if args.dbz && args.variable.as_deref().is_some_and(|variable| variable != "reflectivity") {
        let message = "--dbz requires the reflectivity variable";
        crate::emit(
            &report::with_mode_info(
                report::error_envelope("unit_mismatch", message, "validate"),
                report::failed_mode_info("dbz"),
            ),
            context.json,
        )?;
        return Ok(2);
    }
    let processing = crate::validate_download_processing(
        args.raw_only,
        args.variable,
        &args.grid,
        args.bbox.as_deref(),
        args.resolution,
        &args.resampling,
    )?;
    let mut config = crate::load_config(context.config_path)?;
    if let Some(output) = args.output {
        config.storage.output = output;
    }
    if let Some(cache_dir) = args.cache_dir {
        config.cache.dir = cache_dir;
    }
    if let Some(access_key) = args.access_key {
        config.storage.access_key = Some(access_key);
    }
    if let Some(secret_key) = args.secret_key {
        config.storage.secret_key = Some(secret_key);
    }
    if let Some(endpoint) = args.endpoint {
        config.storage.endpoint = Some(endpoint);
    }
    if args.no_cache {
        config.cache.enabled = false;
    }
    if let Some(template) = args.output_template.as_deref() {
        crate::validate_output_template(template)?;
    }
    if args.raw_only && args.output_template.is_some() {
        return Err("--output-template currently applies to decoded outputs only".into());
    }
    if let Some(format) = args.format {
        config.output.format = format;
    }
    config.validate().map_err(|error| error.to_string())?;
    if !args.dry_run
        && !args.raw_only
        && !matches!(config.output.format.as_str(), "png" | "netcdf" | "geotiff" | "zarr")
    {
        crate::emit(
            &report::error_envelope(
                "unsupported",
                "Native decoded downloads currently support --format png, netcdf, geotiff, or zarr for validated scientific sources",
                "validate",
            ),
            context.json,
        )?;
        return Ok(2);
    }
    if !args.dry_run && !args.raw_only && !crate::supports_native_decoded(&query) && !args.dbz {
        let message = match config.output.format.as_str() {
            "png" => {
                "Native PNG download requires rainviewer composite, tw grid, or rdcap reflectivity"
            }
            "netcdf" => {
                "Native decoded downloads support --format png, netcdf, geotiff, or zarr only for rainviewer composite, tw grid, or rdcap reflectivity"
            }
            "geotiff" => {
                "Native GeoTIFF downloads support only rainviewer composite, tw grid, or rdcap reflectivity"
            }
            "zarr" => {
                "Native Zarr downloads support only rainviewer composite, tw grid, or rdcap reflectivity"
            }
            _ => "Native decoded downloads do not support this output format",
        };
        crate::emit(&report::error_envelope("unsupported", message, "validate"), context.json)?;
        return Ok(2);
    }
    let decoded_format = config.output.format.clone();
    let (mut payload, exit_code) = if args.dry_run {
        let discovery =
            crate::discover(config, query, context.progress_enabled).map_err(|e| e.to_string())?;
        report::download_dry_run(&discovery)
    } else {
        let policy = FetchErrorPolicy::parse(&args.on_error)
            .ok_or_else(|| "--on-error must be collect, continue, stop, or raise".to_owned())?;
        if args.raw_only {
            crate::download_raw_command(
                config,
                query,
                policy,
                args.overwrite,
                context.progress_enabled,
            )?
        } else if args.dbz && !crate::supports_native_decoded(&query) {
            if processing.grid != radiust_core::download::DecodedGrid::Native {
                return local_error(
                    context,
                    "invalid_grid",
                    "source gray dBZ output requires the native pixel grid",
                    "validate",
                );
            }
            crate::download_dbz_mode_command(
                config,
                query,
                policy,
                args.overwrite,
                args.raw,
                decoded_format,
                context.progress_enabled,
            )?
        } else {
            crate::download_decoded_command(
                config,
                query,
                policy,
                args.overwrite,
                args.raw,
                decoded_format,
                args.output_template,
                processing,
                context.progress_enabled,
            )?
        }
    };
    if args.dbz {
        let mut mode_info = report::native_mode_info(
            "dbz",
            if args.dry_run { "" } else { "dbz" },
            "reflectivity",
            Some("dBZ"),
            if args.dry_run { "unknown" } else { "known" },
            "unknown",
        );
        if args.dry_run {
            mode_info["actual"] = serde_json::Value::Null;
        }
        payload = report::with_mode_info(payload, mode_info);
    }
    crate::emit_cli(&payload, context.json, context.quiet, context.verbose, Some("download"))?;
    Ok(exit_code)
}

fn run_local_dbz(args: Args, file: PathBuf, context: &Context<'_>) -> Result<u8, String> {
    if !args.dbz {
        return local_error(context, "invalid_query", "--file requires --dbz", "validate");
    }
    if args.raw || args.raw_only {
        return local_error(
            context,
            "invalid_query",
            "--raw and --raw-only apply only to source downloads",
            "validate",
        );
    }
    if args.output_template.is_some() {
        return local_error(
            context,
            "invalid_query",
            "--output-template applies only to source downloads",
            "validate",
        );
    }
    if args.cache_dir.is_some()
        || args.access_key.is_some()
        || args.secret_key.is_some()
        || args.endpoint.is_some()
        || args.no_cache
    {
        return local_error(
            context,
            "invalid_query",
            "cache and remote-storage options apply only to source downloads",
            "validate",
        );
    }
    if args.product.is_some()
        || !args.stations.is_empty()
        || args.latest
        || args.start.is_some()
        || args.end.is_some()
        || args.base_time.is_some()
        || args.max_age_secs.is_some()
    {
        return local_error(
            context,
            "invalid_query",
            "source selection flags cannot be used with --file",
            "validate",
        );
    }
    if args.variable.as_deref().is_some_and(|variable| variable != "reflectivity") {
        return local_error(
            context,
            "unit_mismatch",
            "--dbz requires the reflectivity variable",
            "validate",
        );
    }
    if args.bbox.is_some()
        || args.grid != "native"
        || args.resolution.is_some()
        || args.resampling != "nearest"
    {
        return local_error(
            context,
            "invalid_grid",
            "geographic processing is unavailable for local dBZ files",
            "validate",
        );
    }

    let mut config = crate::load_config(context.config_path)?;
    if let Some(output) = args.output {
        config.storage.output = output;
    }
    if let Some(format) = args.format {
        config.output.format = format;
    }
    config.validate().map_err(|error| error.to_string())?;
    let format = config.output.format.as_str();
    let output_format = match format {
        "png" => RasterOutputFormat::Png,
        "netcdf" => RasterOutputFormat::NetCdf,
        "geotiff" => RasterOutputFormat::GeoTiff,
        "zarr" => RasterOutputFormat::Zarr,
        _ => {
            return local_error(
                context,
                "unsupported",
                "local dBZ downloads support --format png, netcdf, geotiff, or zarr",
                "validate",
            );
        }
    };
    let numeric_input = is_numeric_file(&file);
    if args.at.is_some() && !numeric_input {
        return local_error(
            context,
            "invalid_query",
            "--at requires a numeric file with a known time coordinate",
            "validate",
        );
    }

    let engine = Engine::new(config.clone(), SourceRegistry::default())
        .map_err(|error| error.to_string())?;
    let result = if numeric_input {
        engine.read_dbz_file(&file, args.variable.as_deref(), args.at.as_deref())
    } else {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.runtime.decode_workers.max(1))
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => {
                return local_error(
                    context,
                    "internal",
                    "native runtime could not be initialized",
                    "decode",
                );
            }
        };
        match runtime.block_on(engine.decode_gray_file(file.clone(), None)) {
            Ok(result) => Ok(result),
            Err(error) => {
                return engine_local_error(context, &error, "decode");
            }
        }
    };
    let result = match result {
        Ok(result) => result,
        Err(error) => return core_local_error(context, &error, "decode"),
    };

    let profile = match &result.data {
        RasterResultData::Pixel(_) => RasterOutputProfile::Pixel,
        RasterResultData::Native(_) | RasterResultData::NativeDataset { .. } => {
            RasterOutputProfile::Native
        }
    };
    if profile == RasterOutputProfile::Pixel && format == "geotiff" {
        return core_local_error(
            context,
            &CoreError::Provider(radiust_core::errors::ProviderError::InvalidGrid),
            "validate",
        );
    }
    let geometry = match &result.data {
        RasterResultData::Pixel(field) => field.geometry.clone(),
        RasterResultData::Native(field) => radiust_core::output::geometry_from_field(field),
        RasterResultData::NativeDataset { owner, index } => {
            owner.fields.get(*index).and_then(radiust_core::output::geometry_from_field)
        }
    };
    if let Err(error) = radiust_core::output::preflight_raster_output(
        profile,
        output_format,
        geometry.as_ref(),
        GeographicOutputRequest::default(),
    ) {
        return core_local_error(context, &error, "validate");
    }

    let input_key = match &result.input {
        RasterInput::Local { identity, read_receipt } => json!({
            "kind": identity.kind,
            "content_digest": identity.content_sha256,
            "frame_index": read_receipt.frame_index,
        }),
        RasterInput::NumericFile { identity, .. } => json!({
            "kind": identity.kind,
            "content_digest": identity.content_digest,
            "variable": identity.variable,
            "selection": identity.selection,
            "valid_time": identity.valid_time,
        }),
        RasterInput::Source { .. } => {
            return local_error(
                context,
                "internal",
                "local download unexpectedly produced a source identity",
                "validate",
            );
        }
    };
    let output_key = radiust_core::identity::digest(&json!({
        "input": input_key,
        "variable": "reflectivity",
        "format": format,
        "writer_options": {},
    }))
    .map_err(|error| error.to_string())?;
    let extension = match format {
        "png" => "png",
        "netcdf" => "nc",
        "geotiff" => "tif",
        "zarr" => "zarr",
        _ => unreachable!("format was validated above"),
    };
    let output_name = format!("dbz/{}.{}", &output_key[..24], extension);
    let output_uri = config.storage.output.join(&output_name).display().to_string();
    let mut mode_info = serde_json::to_value(&result.mode_info).unwrap_or(Value::Null);
    mode_info["requested"] = json!("dbz");
    let variable = match &result.data {
        RasterResultData::Pixel(_) => "reflectivity".to_owned(),
        RasterResultData::Native(field) => field.name.clone(),
        RasterResultData::NativeDataset { owner, index } => owner
            .fields
            .get(*index)
            .map(|field| field.name.clone())
            .unwrap_or_else(|| "reflectivity".to_owned()),
    };
    if args.dry_run {
        let mut payload = local_report(&file, &variable, "planned", None, None);
        mode_info["actual"] = Value::Null;
        payload = report::with_mode_info(payload, mode_info);
        crate::emit_cli(&payload, context.json, context.quiet, context.verbose, Some("download"))?;
        return Ok(0);
    }

    let commit = match engine.write_raster_to(
        &result,
        &config.storage.output,
        &output_name,
        format,
        &json!({}),
        args.overwrite,
    ) {
        Ok(commit) => commit,
        Err(error) => return core_local_error(context, &error, "commit"),
    };
    let status = match commit.status {
        radiust_core::storage::LocalCommitStatus::Written => "written",
        radiust_core::storage::LocalCommitStatus::Skipped => "skipped",
    };
    let mut payload = local_report(
        &file,
        &variable,
        status,
        Some(output_uri),
        Some(json!({
            "logical_id": commit.manifest.logical_id,
            "revision": commit.manifest.revision,
            "output_id": commit.manifest.output_id,
            "raw_complete": commit.manifest.raw_complete,
            "schema_version": commit.manifest.schema_version,
        })),
    );
    if status == "skipped" {
        payload["counts"]["written"] = json!(0);
        payload["counts"]["skipped"] = json!(1);
    }
    payload = report::with_mode_info(payload, mode_info);
    crate::emit_cli(&payload, context.json, context.quiet, context.verbose, Some("download"))?;
    Ok(0)
}

fn is_numeric_file(path: &Path) -> bool {
    if path.is_dir() {
        return true;
    }
    matches!(
        path.extension().and_then(|value| value.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("nc" | "netcdf" | "zarr" | "tif" | "tiff")
    )
}

fn local_report(
    file: &Path,
    variable: &str,
    status: &str,
    output_uri: Option<String>,
    result: Option<Value>,
) -> Value {
    let mut counts = json!({
        "written": 0,
        "skipped": 0,
        "failed": 0,
        "cancelled": 0,
        "not_started": 0,
        "planned": 0,
    });
    counts[status] = json!(1);
    json!({
        "schema_version": 1,
        "command": "download",
        "run_id": null,
        "query": {"file": file.display().to_string(), "variable": variable},
        "counts": counts,
        "items": [{
            "source": "local",
            "product": "numeric_or_gray_file",
            "station": null,
            "valid_time": null,
            "logical_id": null,
            "status": status,
            "output_uri": output_uri,
            "error": null,
        }],
        "result": result,
        "error": null,
        "interrupted": false,
    })
}

fn local_error(
    context: &Context<'_>,
    code: &str,
    message: &str,
    stage: &str,
) -> Result<u8, String> {
    let payload = report::with_mode_info(
        report::error_envelope(code, message, stage),
        report::failed_mode_info("dbz"),
    );
    crate::emit_cli(&payload, context.json, context.quiet, context.verbose, Some("download"))?;
    Ok(2)
}

fn core_local_error(context: &Context<'_>, error: &CoreError, stage: &str) -> Result<u8, String> {
    let stage_label = stage;
    let error_stage = match stage {
        "commit" => radiust_core::error_contract::ErrorStage::Commit,
        "validate" => radiust_core::error_contract::ErrorStage::Validate,
        _ => radiust_core::error_contract::ErrorStage::Decode,
    };
    let report_value = ErrorReport::from_core(error, error_stage);
    let code = serde_json::to_value(report_value.code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "error".to_owned());
    let payload = report::with_mode_info(
        report::error_envelope(&code, &report_value.message, stage_label),
        report::failed_mode_info("dbz"),
    );
    crate::emit_cli(&payload, context.json, context.quiet, context.verbose, Some("download"))?;
    Ok(if code == "cancelled" { 130 } else { 5 })
}

fn engine_local_error(
    context: &Context<'_>,
    error: &EngineError,
    stage: &str,
) -> Result<u8, String> {
    match error {
        EngineError::Core(error) => core_local_error(context, error, stage),
        _ => local_error(context, crate::local_dbz_error_code(error), &error.to_string(), stage),
    }
}
