use super::Context;
use crate::report;
use radiust_core::download::FetchErrorPolicy;
use std::path::PathBuf;

pub(crate) struct Args {
    pub source: String,
    pub dry_run: bool,
    pub raw_only: bool,
    pub raw: bool,
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

pub(crate) fn run(args: Args, context: &Context<'_>) -> Result<u8, String> {
    let query = crate::build_query(
        vec![args.source],
        args.product,
        args.stations,
        args.latest,
        args.at,
        args.start,
        args.end,
        args.base_time,
        args.max_age_secs,
    )?;
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
    if !args.dry_run && !args.raw_only && !crate::supports_native_decoded(&query) {
        let message = match config.output.format.as_str() {
            "png" => "Native PNG download requires rainviewer composite or tw grid",
            "netcdf" => {
                "Native decoded downloads support --format png, netcdf, geotiff, or zarr only for rainviewer composite or tw grid"
            }
            "geotiff" => "Native GeoTIFF downloads support only rainviewer composite or tw grid",
            "zarr" => "Native Zarr downloads support only rainviewer composite or tw grid",
            _ => "Native decoded downloads do not support this output format",
        };
        crate::emit(&report::error_envelope("unsupported", message, "validate"), context.json)?;
        return Ok(2);
    }
    let decoded_format = config.output.format.clone();
    let (payload, exit_code) = if args.dry_run {
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
    crate::emit_cli(&payload, context.json, context.quiet, context.verbose, Some("download"))?;
    Ok(exit_code)
}
