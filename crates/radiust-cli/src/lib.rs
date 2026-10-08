use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
mod commands;
mod human;
mod progress;
mod report;
mod terminal;

use clap::{Parser, Subcommand};
use radiust_core::cache::Cache;
use radiust_core::config::CoreConfig;
use radiust_core::download::{DecodedGrid, DecodedProcessing, FetchErrorPolicy};
use radiust_core::engine::{Engine, EngineError};
use radiust_core::grid::Resampling;
use radiust_core::limits::Limits;
use radiust_core::model::{DiscoveryReport, DiscoveryStatus, PreviewMode, Query, TimeSelector};
use radiust_core::preview::preview_file;
use radiust_core::raster::RasterResultData;
use radiust_core::source::SourceRegistry;
use radiust_core::source::catalog::SourceCatalog;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;

#[derive(Debug, Parser)]
#[command(name = "radiust", version, about = "Acquire and inspect radar data")]
struct Cli {
    /// Read project YAML over ~/.config/radiust/config.yaml (default: ./config.yaml).
    #[arg(long = "conf", global = true)]
    config_path: Option<PathBuf>,
    /// Emit one JSON report.
    #[arg(long, global = true)]
    json: bool,
    /// Suppress successful reports while preserving previews, errors, and JSON.
    #[arg(long, global = true)]
    quiet: bool,
    /// Show individual frames and safe runtime diagnostics in human reports.
    #[arg(long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect built-in sources, products, or stations.
    List {
        #[arg(default_value = "sources")]
        kind: String,
        source: Option<String>,
    },
    /// Find the newest or selected radar frames.
    Discover {
        #[arg(required = true, num_args = 1..)]
        sources: Vec<String>,
        #[arg(long)]
        product: Option<String>,
        #[arg(long = "station")]
        stations: Vec<String>,
        #[arg(long)]
        latest: bool,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
        #[arg(long = "base-time")]
        base_time: Option<String>,
        #[arg(long = "max-age")]
        max_age_secs: Option<f64>,
    },
    /// Plan or acquire selected frames.
    Download {
        #[arg(required_unless_present = "file", conflicts_with = "file")]
        source: Option<String>,
        /// Decode and save a local gray image or numeric reflectivity file.
        #[arg(long, requires = "dbz")]
        file: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long = "raw-only", conflicts_with = "raw")]
        raw_only: bool,
        /// Include the verified source artifacts alongside decoded output.
        #[arg(long)]
        raw: bool,
        /// Require direct native reflectivity values with dBZ units.
        #[arg(long, conflicts_with = "raw_only")]
        dbz: bool,
        #[arg(long)]
        overwrite: bool,
        #[arg(long, value_parser = ["collect", "continue", "stop", "raise"], default_value = "collect")]
        on_error: String,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long = "output-template")]
        output_template: Option<String>,
        #[arg(long = "cache-dir")]
        cache_dir: Option<PathBuf>,
        #[arg(long = "access-key")]
        access_key: Option<String>,
        #[arg(long = "secret-key")]
        secret_key: Option<String>,
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long = "no-cache")]
        no_cache: bool,
        #[arg(long)]
        variable: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        bbox: Option<String>,
        #[arg(long, value_parser = ["native", "geographic"], default_value = "native")]
        grid: String,
        #[arg(long)]
        resolution: Option<f64>,
        #[arg(long, value_parser = ["nearest", "bilinear"], default_value = "nearest")]
        resampling: String,
        #[arg(long, value_parser = ["netcdf", "geotiff", "png", "zarr"])]
        format: Option<String>,
        #[arg(long)]
        product: Option<String>,
        #[arg(long = "station")]
        stations: Vec<String>,
        #[arg(long)]
        latest: bool,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
        #[arg(long = "base-time")]
        base_time: Option<String>,
        #[arg(long = "max-age")]
        max_age_secs: Option<f64>,
    },
    /// Replay a committed raw manifest offline through the native decoder and writers.
    Replay {
        manifest: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(
            long = "format",
            value_delimiter = ',',
            value_parser = ["netcdf", "geotiff", "png", "zarr"],
            default_value = "png,netcdf,geotiff,zarr"
        )]
        formats: Vec<String>,
        #[arg(long)]
        overwrite: bool,
        /// Require direct native reflectivity values with dBZ units.
        #[arg(long)]
        dbz: bool,
    },
    /// Preview a local image or a selected NetCDF field.
    Cat {
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        variable: Option<String>,
        #[arg(long)]
        palette: Option<String>,
        #[arg(long)]
        vmin: Option<f64>,
        #[arg(long)]
        vmax: Option<f64>,
        #[arg(long)]
        width: Option<usize>,
        #[arg(long)]
        height: Option<usize>,
        /// Preview the source image without scientific decoding (the default).
        #[arg(long, conflicts_with_all = ["decoded", "gray", "dbz", "legacy_display"])]
        raw: bool,
        /// Decode a source frame using a validated native scientific decoder.
        #[arg(long, conflicts_with_all = ["raw", "gray", "dbz", "legacy_display"])]
        decoded: bool,
        /// Show the source image using gray-code display semantics.
        #[arg(long, conflicts_with_all = ["raw", "decoded", "dbz", "legacy_display"])]
        gray: bool,
        /// Decode a declared local gray image or native reflectivity to dBZ.
        #[arg(long, conflicts_with_all = ["raw", "decoded", "gray", "legacy_display"])]
        dbz: bool,
        /// Select an explicit frame from a multi-frame local image.
        #[arg(long = "frame-index")]
        frame_index: Option<u32>,
        /// Compatibility alias for --gray; use --gray for gray-code display.
        #[arg(long = "legacy-display", conflicts_with_all = ["raw", "decoded", "gray", "dbz"])]
        legacy_display: bool,
        source: Option<String>,
        #[arg(long)]
        product: Option<String>,
        #[arg(long = "station")]
        stations: Vec<String>,
        #[arg(long)]
        latest: bool,
        #[arg(long)]
        at: Option<String>,
        #[arg(long = "base-time")]
        base_time: Option<String>,
        #[arg(long, value_parser = ["auto", "kitty", "iterm2", "ansi", "text"], default_value = "auto")]
        renderer: String,
    },
    /// Inspect runtime and local storage readiness.
    Doctor {
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        network: bool,
    },
    /// Inspect effective configuration.
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommand>,
    },
    /// Inspect or maintain the acquisition cache.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Show,
}

#[derive(Debug, Subcommand)]
enum CacheCommand {
    Status {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        output_root: Option<PathBuf>,
    },
    Gc {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        output_root: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    Clear {
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        output_root: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        /// Confirm removal of cache-owned entries.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Debug, Default)]
struct CatRenderOptions {
    palette: Option<String>,
    vmin: Option<f64>,
    vmax: Option<f64>,
    width: Option<usize>,
    height: Option<usize>,
}

impl CatRenderOptions {
    fn validate(&self) -> Result<(), String> {
        if self.width == Some(0) || self.height == Some(0) {
            return Err("--width and --height must be positive".into());
        }
        if self.width.is_some_and(|width| width > 1024)
            || self.height.is_some_and(|height| height > 512)
        {
            return Err("--width cannot exceed 1024 and --height cannot exceed 512".into());
        }
        if self.vmin.is_some_and(|value| !value.is_finite())
            || self.vmax.is_some_and(|value| !value.is_finite())
        {
            return Err("--vmin and --vmax must be finite".into());
        }
        if self.vmin.zip(self.vmax).is_some_and(|(vmin, vmax)| vmax <= vmin) {
            return Err("--vmax must be greater than --vmin".into());
        }
        if self.palette.as_deref().is_some_and(|palette| palette != "default") {
            return Err(format!("unknown palette: {}", self.palette.as_deref().unwrap()));
        }
        Ok(())
    }

    fn as_png_options(&self) -> Value {
        json!({
            "palette": self.palette,
            "vmin": self.vmin,
            "vmax": self.vmax,
        })
    }

    fn has_science_options(&self) -> bool {
        self.palette.is_some() || self.vmin.is_some() || self.vmax.is_some()
    }
}

fn environment() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

fn load_config(path: Option<&std::path::Path>) -> Result<CoreConfig, String> {
    let env = environment();
    CoreConfig::load(path, &env, None).map_err(|error| error.to_string())
}

fn emit(value: &Value, as_json: bool) -> Result<(), String> {
    if as_json {
        println!("{}", serde_json::to_string(value).map_err(|error| error.to_string())?);
    } else {
        println!("{}", report::render_human(value, None, false));
    }
    Ok(())
}

fn emit_cli(
    value: &Value,
    as_json: bool,
    quiet: bool,
    verbose: bool,
    command_hint: Option<&str>,
) -> Result<(), String> {
    if quiet && !as_json {
        return Ok(());
    }
    if as_json {
        emit(value, true)
    } else {
        println!("{}", report::render_human(value, command_hint, verbose));
        Ok(())
    }
}

fn build_query(
    sources: Vec<String>,
    product: Option<String>,
    stations: Vec<String>,
    latest: bool,
    at: Option<String>,
    start: Option<String>,
    end: Option<String>,
    base_time: Option<String>,
    max_age_secs: Option<f64>,
) -> Result<Query, String> {
    if sources.is_empty() {
        return Err("at least one source is required".into());
    }
    if at.is_some() && (start.is_some() || end.is_some()) {
        return Err("choose either --at or --start/--end".into());
    }
    if start.is_some() != end.is_some() {
        return Err("--start and --end must be supplied together".into());
    }
    let selector = match (at, start, end) {
        (Some(time), None, None) => TimeSelector::At { time },
        (None, Some(start), Some(end)) => TimeSelector::Range { start, end },
        _ => TimeSelector::Latest,
    };
    if latest && !matches!(&selector, TimeSelector::Latest) {
        return Err("--latest cannot be combined with --at or --start/--end".into());
    }
    if max_age_secs.is_some_and(|value| !value.is_finite() || value <= 0.0) {
        return Err("--max-age must be finite and positive".into());
    }
    if max_age_secs.is_some() && !matches!(&selector, TimeSelector::Latest) {
        return Err("--max-age is only valid with latest".into());
    }
    let query = Query {
        source: (sources.len() == 1).then(|| sources[0].clone()),
        sources: if sources.len() > 1 { sources } else { Vec::new() },
        product,
        stations,
        selector,
        base_time,
        max_age_secs,
    };
    let multi_source = query.source.as_deref() == Some("all") || !query.sources.is_empty();
    query.validate(multi_source).map_err(|error| error.to_string())?;
    Ok(query)
}

fn supports_native_decoded(query: &Query) -> bool {
    matches!(
        (query.source.as_deref(), query.product.as_deref()),
        (Some("rainviewer"), None | Some("composite"))
            | (Some("tw"), Some("grid"))
            | (Some("rdcap"), None | Some("reflectivity"))
    )
}

fn supports_decoded_frame(frame: &radiust_core::model::FrameRef) -> bool {
    matches!(
        (frame.source.as_str(), frame.product.as_str()),
        ("rainviewer", "composite") | ("tw", "grid") | ("rdcap", "reflectivity")
    )
}

fn parse_bbox(value: &str) -> Result<[f64; 4], String> {
    let parts = value
        .split(',')
        .map(|part| part.trim().parse::<f64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "bbox must be west,south,east,north with finite numbers".to_owned())?;
    let bbox: [f64; 4] =
        parts.try_into().map_err(|_| "bbox must be west,south,east,north".to_owned())?;
    let [west, south, east, north] = bbox;
    if bbox.iter().any(|coordinate| !coordinate.is_finite()) {
        return Err("bbox coordinates must be finite".into());
    }
    if !(-180.0..=180.0).contains(&west)
        || !(-180.0..=180.0).contains(&east)
        || !(-90.0..=90.0).contains(&south)
        || !(-90.0..=90.0).contains(&north)
    {
        return Err("bbox must be within longitude [-180, 180] and latitude [-90, 90]".into());
    }
    if west >= east || south >= north {
        return Err("bbox must have positive area and cannot cross the date line".into());
    }
    Ok(bbox)
}

fn validate_download_processing(
    raw_only: bool,
    variable: Option<String>,
    grid: &str,
    bbox: Option<&str>,
    resolution: Option<f64>,
    resampling: &str,
) -> Result<DecodedProcessing, String> {
    let bbox = bbox.map(parse_bbox).transpose()?;
    match grid {
        "native" if bbox.is_some() || resolution.is_some() => {
            return Err("--bbox and --resolution require --grid geographic".into());
        }
        "geographic" if bbox.is_none() || resolution.is_none() => {
            return Err("--grid geographic requires --bbox and --resolution".into());
        }
        "native" | "geographic" => {}
        _ => return Err("--grid must be native or geographic".into()),
    }
    let resampling = Resampling::parse(resampling)
        .ok_or_else(|| "--resampling must be nearest or bilinear".to_owned())?;
    if raw_only
        && (variable.is_some()
            || bbox.is_some()
            || resolution.is_some()
            || grid != "native"
            || resampling != Resampling::Nearest)
    {
        return Err("--raw-only cannot be combined with decoded processing options".into());
    }
    if variable.as_deref().is_some_and(|value| value.trim().is_empty()) {
        return Err("--variable must not be empty".into());
    }
    if resolution.is_some_and(|value| !value.is_finite() || value <= 0.0) {
        return Err("--resolution must be finite and positive".into());
    }
    let grid = match (grid, bbox, resolution) {
        ("native", None, None) => DecodedGrid::Native,
        ("geographic", Some(bbox), Some(resolution)) => {
            DecodedGrid::Geographic { bbox, resolution }
        }
        _ => return Err("invalid decoded grid request".into()),
    };
    Ok(DecodedProcessing { variable, grid, resampling })
}

fn validate_output_template(template: &str) -> Result<(), String> {
    const ALLOWED: &[&str] = &[
        "source",
        "product",
        "station",
        "valid_time",
        "base_time",
        "date",
        "hour",
        "variant_id",
        "ext",
    ];
    let path = std::path::Path::new(template);
    if template.is_empty()
        || path.is_absolute()
        || path.components().any(|component| component == std::path::Component::ParentDir)
    {
        return Err("output template must stay within output root".into());
    }
    let mut chars = template.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '{' => {
                let mut field = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    if next == '{' {
                        return Err("invalid nested field in output template".into());
                    }
                    field.push(next);
                }
                if !closed {
                    return Err("unclosed field in output template".into());
                }
                if !ALLOWED.contains(&field.as_str()) {
                    return Err(format!("unsupported output template field: {field}"));
                }
            }
            '}' => return Err("unmatched closing brace in output template".into()),
            _ => {}
        }
    }
    Ok(())
}

fn discovery_exit_code(report: &DiscoveryReport) -> u8 {
    let counts = &report.counts;
    if report.interrupted {
        130
    } else if counts.total == 0 {
        3
    } else if counts.success == counts.total {
        0
    } else if counts.success > 0 {
        4
    } else if counts.no_data + counts.stale == counts.total {
        3
    } else {
        5
    }
}

fn single_discovery_payload(report: &DiscoveryReport) -> Result<(Value, u8), String> {
    if report.interrupted {
        return Ok((report::error_envelope("cancelled", "operation cancelled", "discover"), 130));
    }
    let successful_frames = report
        .items
        .iter()
        .filter(|item| item.status == DiscoveryStatus::Success)
        .map(|item| {
            let frame =
                item.frame.as_ref().ok_or("successful discovery is missing frame identity")?;
            Ok(json!({
                "source": frame.source,
                "product": frame.product,
                "station": frame.station,
                "valid_time": frame.valid_time,
                "logical_id": frame.logical_id,
                "source_urls": public_source_urls(frame),
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    if !successful_frames.is_empty() {
        let exit_code = if report.counts.success == report.counts.total { 0 } else { 4 };
        return Ok((report::envelope("discover", successful_frames, None), exit_code));
    }
    let Some(item) = report.items.first() else {
        return Ok((report::envelope("discover", Vec::new(), None), 3));
    };
    let error = item.error.as_ref();
    let code = error.map(|value| value.code.as_str()).unwrap_or("no_data");
    let message =
        error.map(|value| value.message.as_str()).unwrap_or("no matching frame is available");
    if item.status == DiscoveryStatus::NetworkRestricted {
        return Ok((
            report::error_envelope("error", "Unexpected operation failure", "validate"),
            2,
        ));
    }
    let status = if item.status == DiscoveryStatus::NoData { 3 } else { 2 };
    Ok((report::error_envelope(code, message, "discover"), status))
}

fn discover(
    config: CoreConfig,
    query: Query,
    progress_enabled: bool,
) -> Result<DiscoveryReport, EngineError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.runtime.decode_workers.max(1))
        .enable_all()
        .build()
        .map_err(|_| EngineError::InvalidConfiguration)?;
    let engine = Engine::new(config, SourceRegistry::default())?;
    runtime.block_on(with_progress(
        &engine,
        async {
            let discovery = engine.discover(query);
            tokio::pin!(discovery);
            let cancellation = tokio::signal::ctrl_c();
            tokio::pin!(cancellation);
            tokio::select! {
                biased;
                signal = &mut cancellation => {
                    signal.map_err(|_| EngineError::RuntimeUnavailable)?;
                    engine.cancel();
                    discovery.await
                }
                result = &mut discovery => result,
            }
        },
        progress_enabled,
    ))
}

/// Drain sanitized Engine events while a command runs. Human progress stays on
/// stderr, leaving JSON output on stdout as one complete report.
async fn with_progress<T, F>(engine: &Engine, future: F, enabled: bool) -> T
where
    F: Future<Output = T>,
{
    if !enabled {
        return future.await;
    }
    let mut events = engine.subscribe_events();
    tokio::pin!(future);
    let mut progress = progress::Progress::new();
    loop {
        tokio::select! {
            biased;
            event = events.recv() => match event {
                Ok(event) => progress.event(&event),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => progress.lagged(),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return future.await,
            },
            result = &mut future => {
                loop {
                    match events.try_recv() {
                        Ok(event) => progress.event(&event),
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => progress.lagged(),
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                        | Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
                    }
                }
                return result;
            }
        }
    }
}

fn download_raw_command(
    config: CoreConfig,
    query: Query,
    policy: FetchErrorPolicy,
    overwrite: bool,
    progress_enabled: bool,
) -> Result<(Value, u8), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.runtime.decode_workers.max(1))
        .enable_all()
        .build()
        .map_err(|_| "native runtime could not be initialized".to_owned())?;
    let engine =
        Engine::new(config, SourceRegistry::default()).map_err(|error| error.to_string())?;
    runtime
        .block_on(with_progress(
            &engine,
            download_raw_cancellable(&engine, query, policy, overwrite),
            progress_enabled,
        ))
        .map_err(|error| error.to_string())
}

fn download_decoded_command(
    config: CoreConfig,
    query: Query,
    policy: FetchErrorPolicy,
    overwrite: bool,
    include_raw: bool,
    format: String,
    output_template: Option<String>,
    processing: DecodedProcessing,
    progress_enabled: bool,
) -> Result<(Value, u8), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.runtime.decode_workers.max(1))
        .enable_all()
        .build()
        .map_err(|_| "native runtime could not be initialized".to_owned())?;
    let engine =
        Engine::new(config, SourceRegistry::default()).map_err(|error| error.to_string())?;
    runtime
        .block_on(with_progress(
            &engine,
            download_decoded_cancellable(
                &engine,
                query,
                policy,
                overwrite,
                include_raw,
                format,
                output_template,
                processing,
            ),
            progress_enabled,
        ))
        .map_err(|error| error.to_string())
}

fn download_dbz_mode_command(
    config: CoreConfig,
    query: Query,
    policy: FetchErrorPolicy,
    overwrite: bool,
    include_raw: bool,
    format: String,
    progress_enabled: bool,
) -> Result<(Value, u8), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.runtime.decode_workers.max(1))
        .enable_all()
        .build()
        .map_err(|_| "native runtime could not be initialized".to_owned())?;
    let engine =
        Engine::new(config, SourceRegistry::default()).map_err(|error| error.to_string())?;
    runtime
        .block_on(with_progress(
            &engine,
            download_dbz_mode_cancellable(&engine, query, policy, overwrite, include_raw, format),
            progress_enabled,
        ))
        .map_err(|error| error.to_string())
}

struct SourceDbzOutput {
    input_index: usize,
    frame: radiust_core::model::FrameRef,
    status: radiust_core::download::FetchStatus,
    result: Option<radiust_core::raster::RasterResult>,
    raw: Option<Arc<radiust_core::model::RawFrame>>,
    error: Option<radiust_core::error_contract::ErrorReport>,
}

fn source_dbz_error_report(error: &EngineError) -> radiust_core::error_contract::ErrorReport {
    use radiust_core::error_contract::{ErrorCode, ErrorReport, ErrorStage};
    match error {
        EngineError::Core(error) => ErrorReport::from_core(error, ErrorStage::Decode),
        EngineError::UnsupportedScience(_) | EngineError::UnsupportedSource(_) => ErrorReport {
            code: ErrorCode::Unsupported,
            message: "source does not have a validated dBZ decoder".into(),
            stage: ErrorStage::Decode,
            retryable: false,
        },
        EngineError::UnsupportedVariable { .. } => ErrorReport {
            code: ErrorCode::Unsupported,
            message: "source dBZ decoder does not provide the requested variable".into(),
            stage: ErrorStage::Decode,
            retryable: false,
        },
        EngineError::InvalidQuery(_) => ErrorReport {
            code: ErrorCode::InvalidQuery,
            message: "source dBZ request is invalid".into(),
            stage: ErrorStage::Validate,
            retryable: false,
        },
        _ => ErrorReport {
            code: ErrorCode::Internal,
            message: "source dBZ decoding failed".into(),
            stage: ErrorStage::Decode,
            retryable: false,
        },
    }
}

async fn download_dbz_mode_cancellable(
    engine: &Engine,
    query: Query,
    policy: FetchErrorPolicy,
    overwrite: bool,
    include_raw: bool,
    format: String,
) -> Result<(Value, u8), EngineError> {
    let report_query = query.clone();
    let operation = async {
        let discovery = engine.discover(query).await?;
        if discovery.interrupted {
            return Ok((report::download_interrupted(&report_query), 130));
        }
        let frames = discovery
            .items
            .iter()
            .filter(|item| item.status == DiscoveryStatus::Success)
            .filter_map(|item| item.frame.clone())
            .collect::<Vec<_>>();
        let (outputs, batch_cancelled) = if include_raw {
            let fetched = engine.fetch_many_raw(frames, policy, false).await;
            let mut outputs = Vec::with_capacity(fetched.items.len());
            for item in fetched.items {
                if item.status == radiust_core::download::FetchStatus::Success {
                    if let Some(raw) = item.raw {
                        let raw = Arc::new(raw);
                        match engine.decode_dbz(raw.clone()).await {
                            Ok(result) => outputs.push(SourceDbzOutput {
                                input_index: item.input_index,
                                frame: item.frame,
                                status: radiust_core::download::FetchStatus::Success,
                                result: Some(result),
                                raw: Some(raw),
                                error: None,
                            }),
                            Err(error) => outputs.push(SourceDbzOutput {
                                input_index: item.input_index,
                                frame: item.frame,
                                status: radiust_core::download::FetchStatus::Failed,
                                result: None,
                                raw: None,
                                error: Some(source_dbz_error_report(&error)),
                            }),
                        }
                    } else {
                        outputs.push(SourceDbzOutput {
                            input_index: item.input_index,
                            frame: item.frame,
                            status: radiust_core::download::FetchStatus::Failed,
                            result: None,
                            raw: None,
                            error: Some(radiust_core::error_contract::ErrorReport {
                                code: radiust_core::error_contract::ErrorCode::Internal,
                                message: "raw acquisition omitted its artifacts".into(),
                                stage: radiust_core::error_contract::ErrorStage::Acquire,
                                retryable: false,
                            }),
                        });
                    }
                } else {
                    outputs.push(SourceDbzOutput {
                        input_index: item.input_index,
                        frame: item.frame,
                        status: item.status,
                        result: None,
                        raw: None,
                        error: item.error_details,
                    });
                }
            }
            (outputs, fetched.cancelled > 0 && fetched.failed == 0)
        } else {
            let decoded = engine.fetch_many_mode(frames, "dbz", policy, false, None).await;
            let outputs = decoded
                .items
                .into_iter()
                .map(|item| SourceDbzOutput {
                    input_index: item.input_index,
                    frame: item.frame,
                    status: item.status,
                    result: item.dbz,
                    raw: None,
                    error: item.error_details,
                })
                .collect();
            (outputs, decoded.cancelled > 0 && decoded.failed == 0)
        };
        let mut downloaded = radiust_core::download::DownloadBatchReport::default();
        let mut decoded_mode_info = Vec::with_capacity(outputs.len());
        let extension = match format.as_str() {
            "png" => "png",
            "netcdf" => "nc",
            "geotiff" => "tif",
            "zarr" => "zarr",
            _ => "nc",
        };
        let mut stop_after_failure = false;
        for item in outputs {
            let mut status = match item.status {
                radiust_core::download::FetchStatus::Planned => {
                    radiust_core::download::DownloadStatus::Planned
                }
                radiust_core::download::FetchStatus::Success => {
                    radiust_core::download::DownloadStatus::Failed
                }
                radiust_core::download::FetchStatus::Failed => {
                    radiust_core::download::DownloadStatus::Failed
                }
                radiust_core::download::FetchStatus::Cancelled => {
                    radiust_core::download::DownloadStatus::Cancelled
                }
                radiust_core::download::FetchStatus::NotStarted => {
                    radiust_core::download::DownloadStatus::NotStarted
                }
            };
            let mut output_uri = None;
            let mut error = item.error.clone();
            let mut mode_info = report::failed_mode_info("dbz");
            if item.status == radiust_core::download::FetchStatus::Success {
                if stop_after_failure {
                    status = radiust_core::download::DownloadStatus::Cancelled;
                    error = Some(radiust_core::error_contract::ErrorReport {
                        code: radiust_core::error_contract::ErrorCode::Cancelled,
                        message: "cancelled after an earlier output failed".into(),
                        stage: radiust_core::error_contract::ErrorStage::Commit,
                        retryable: false,
                    });
                } else if let Some(result) = item.result {
                    mode_info = serde_json::to_value(&result.mode_info).unwrap_or(Value::Null);
                    mode_info["requested"] = json!("dbz");
                    let logical_id = match radiust_core::identity::logical_id(&item.frame) {
                        Ok(value) => value,
                        Err(identity_error) => {
                            status = radiust_core::download::DownloadStatus::Failed;
                            error = Some(radiust_core::error_contract::ErrorReport {
                                code: radiust_core::error_contract::ErrorCode::Integrity,
                                message: identity_error.to_string(),
                                stage: radiust_core::error_contract::ErrorStage::Validate,
                                retryable: false,
                            });
                            String::new()
                        }
                    };
                    if !logical_id.is_empty() {
                        let output_name = format!("frames/{logical_id}/reflectivity.{extension}");
                        let output_root = engine.config().storage.output.clone();
                        let result = Arc::new(result);
                        let committed = match item.raw {
                            Some(raw) => {
                                engine
                                    .write_raster_result_to_with_raw(
                                        result,
                                        raw,
                                        output_root.clone(),
                                        output_name.clone(),
                                        format.clone(),
                                        overwrite,
                                    )
                                    .await
                            }
                            None => {
                                engine
                                    .write_raster_result_to(
                                        result,
                                        output_root.clone(),
                                        output_name.clone(),
                                        format.clone(),
                                        overwrite,
                                    )
                                    .await
                            }
                        };
                        match committed {
                            Ok(committed) => {
                                status = match committed.status {
                                    radiust_core::storage::LocalCommitStatus::Written => {
                                        radiust_core::download::DownloadStatus::Written
                                    }
                                    radiust_core::storage::LocalCommitStatus::Skipped => {
                                        radiust_core::download::DownloadStatus::Skipped
                                    }
                                };
                                output_uri =
                                    Some(output_root.join(output_name).display().to_string());
                                error = None;
                            }
                            Err(commit_error) => {
                                status = radiust_core::download::DownloadStatus::Failed;
                                error = Some(radiust_core::error_contract::ErrorReport::from_core(
                                    &commit_error,
                                    radiust_core::error_contract::ErrorStage::Commit,
                                ));
                            }
                        }
                    }
                } else {
                    status = radiust_core::download::DownloadStatus::Failed;
                    error = Some(radiust_core::error_contract::ErrorReport {
                        code: radiust_core::error_contract::ErrorCode::Internal,
                        message: "dBZ decoder omitted the raster result".into(),
                        stage: radiust_core::error_contract::ErrorStage::Decode,
                        retryable: false,
                    });
                }
                if !matches!(
                    status,
                    radiust_core::download::DownloadStatus::Written
                        | radiust_core::download::DownloadStatus::Skipped
                ) {
                    mode_info["actual"] = Value::Null;
                    if policy == FetchErrorPolicy::Stop {
                        stop_after_failure = true;
                    }
                }
            }
            decoded_mode_info.push(mode_info);
            downloaded.items.push(radiust_core::download::DownloadItem {
                input_index: item.input_index,
                frame: item.frame,
                status,
                output_uri,
                error,
            });
        }
        downloaded.interrupted = batch_cancelled;
        let (mut payload, exit_code) = report::download_execution(&discovery, &downloaded);
        let mut item_mode_infos = Vec::with_capacity(discovery.items.len());
        let mut success_index = 0;
        for item in &discovery.items {
            if item.status == DiscoveryStatus::Success {
                item_mode_infos.push(
                    decoded_mode_info
                        .get(success_index)
                        .cloned()
                        .unwrap_or_else(|| report::failed_mode_info("dbz")),
                );
                success_index += 1;
            } else {
                item_mode_infos.push(report::failed_mode_info("dbz"));
            }
        }
        let summary = item_mode_infos
            .iter()
            .find(|mode| mode["actual"] == "dbz")
            .cloned()
            .unwrap_or_else(|| report::failed_mode_info("dbz"));
        payload = report::with_mode_info(payload, summary);
        if let Some(items) = payload.get_mut("items").and_then(Value::as_array_mut) {
            for (item, mode_info) in items.iter_mut().zip(item_mode_infos) {
                if let Some(object) = item.as_object_mut() {
                    object.insert("mode_info".into(), mode_info);
                }
            }
        }
        Ok((payload, exit_code))
    };
    tokio::pin!(operation);
    let cancellation = tokio::signal::ctrl_c();
    tokio::pin!(cancellation);
    tokio::select! {
        biased;
        signal = &mut cancellation => {
            signal.map_err(|_| EngineError::RuntimeUnavailable)?;
            engine.cancel();
            let mut result = operation.await.unwrap_or_else(|_| {
                (report::download_interrupted(&report_query), 130)
            });
            result.0["interrupted"] = json!(true);
            result.0["error"] = json!({
                "code": "cancelled",
                "message": "operation cancelled",
                "stage": "download",
                "retryable": false,
            });
            Ok((result.0, 130))
        }
        result = &mut operation => result,
    }
}

async fn download_raw_cancellable(
    engine: &Engine,
    query: Query,
    policy: FetchErrorPolicy,
    overwrite: bool,
) -> Result<(Value, u8), EngineError> {
    let report_query = query.clone();
    let operation = async {
        let discovery = engine.discover(query).await?;
        if discovery.interrupted {
            return Ok((report::download_interrupted(&report_query), 130));
        }
        let frames = discovery
            .items
            .iter()
            .filter(|item| item.status == DiscoveryStatus::Success)
            .filter_map(|item| item.frame.clone())
            .collect::<Vec<_>>();
        let downloaded = engine.download_raw_only(frames, policy, false, overwrite).await;
        Ok(report::download_execution(&discovery, &downloaded))
    };
    tokio::pin!(operation);
    let cancellation = tokio::signal::ctrl_c();
    tokio::pin!(cancellation);
    tokio::select! {
        biased;
        signal = &mut cancellation => {
            signal.map_err(|_| EngineError::RuntimeUnavailable)?;
            engine.cancel();
            let mut result = operation.await.unwrap_or_else(|_| {
                (report::download_interrupted(&report_query), 130)
            });
            result.0["interrupted"] = json!(true);
            result.0["error"] = json!({
                "code": "cancelled",
                "message": "operation cancelled",
                "stage": "download",
                "retryable": false,
            });
            Ok((result.0, 130))
        }
        result = &mut operation => result,
    }
}

async fn download_decoded_cancellable(
    engine: &Engine,
    query: Query,
    policy: FetchErrorPolicy,
    overwrite: bool,
    include_raw: bool,
    format: String,
    output_template: Option<String>,
    processing: DecodedProcessing,
) -> Result<(Value, u8), EngineError> {
    let report_query = query.clone();
    let operation = async {
        let discovery = engine.discover(query).await?;
        if discovery.interrupted {
            return Ok((report::download_interrupted(&report_query), 130));
        }
        let frames = discovery
            .items
            .iter()
            .filter(|item| item.status == DiscoveryStatus::Success)
            .filter_map(|item| item.frame.clone())
            .collect::<Vec<_>>();
        let downloaded = engine
            .download_decoded_to_with_processing_and_raw(
                frames,
                policy,
                false,
                overwrite,
                engine.config().storage.output.clone(),
                &format,
                output_template,
                processing,
                include_raw,
            )
            .await?;
        Ok(report::download_execution(&discovery, &downloaded))
    };
    tokio::pin!(operation);
    let cancellation = tokio::signal::ctrl_c();
    tokio::pin!(cancellation);
    tokio::select! {
        biased;
        signal = &mut cancellation => {
            signal.map_err(|_| EngineError::RuntimeUnavailable)?;
            engine.cancel();
            let mut result = operation.await.unwrap_or_else(|_| {
                (report::download_interrupted(&report_query), 130)
            });
            result.0["interrupted"] = json!(true);
            result.0["error"] = json!({
                "code": "cancelled",
                "message": "operation cancelled",
                "stage": "download",
                "retryable": false,
            });
            Ok((result.0, 130))
        }
        result = &mut operation => result,
    }
}

pub(crate) fn public_source_urls(frame: &radiust_core::model::FrameRef) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(url) = frame.locator.get("url").and_then(Value::as_str) {
        candidates.push(url);
    }
    if let Some(artifacts) = frame.locator.get("artifacts").and_then(Value::as_array) {
        candidates.extend(
            artifacts.iter().filter_map(|artifact| artifact.get("url").and_then(Value::as_str)),
        );
    }

    let mut safe_urls = Vec::new();
    for value in candidates {
        let Ok(mut url) = Url::parse(value) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https" | "ftp") {
            continue;
        }
        // Source locators can contain credentials or signed query strings.
        // Keep the useful host/path while ensuring JSON reports cannot leak them.
        if url.set_username("").is_err() || url.set_password(None).is_err() {
            continue;
        }
        url.set_query(None);
        url.set_fragment(None);
        let safe = url.to_string();
        if !safe_urls.contains(&safe) {
            safe_urls.push(safe);
        }
    }
    safe_urls
}

fn cat_file(
    path: &std::path::Path,
    variable: Option<&str>,
    valid_time: Option<&str>,
    renderer: &str,
    config: &CoreConfig,
    as_json: bool,
    options: &CatRenderOptions,
    mode: &str,
    frame_index: Option<u32>,
) -> Result<u8, String> {
    let renderer = resolve_preview_renderer(renderer, std::io::stdout().is_terminal(), as_json)?;
    let extension =
        path.extension().and_then(|value| value.to_str()).unwrap_or_default().to_ascii_lowercase();
    let format = match extension.as_str() {
        "nc" | "nc4" | "cdf" | "netcdf" => Some(("NetCDF4", true)),
        "tif" | "tiff" => Some(("GeoTIFF", false)),
        "zarr" => Some(("Zarr v2", false)),
        _ if variable.is_some() => Some(("NetCDF4", true)),
        _ => None,
    };
    let mode = if mode == "default" && format.is_some() {
        "scientific"
    } else if mode == "default" {
        "raw"
    } else {
        mode
    };
    if format.is_none() && mode == "scientific" {
        return Err("--decoded requires a scientific --file or SOURCE".into());
    }
    if format.is_none() && mode != "dbz" && (variable.is_some() || options.has_science_options()) {
        return Err("raw image preview does not accept scientific decoding options".into());
    }
    options.validate()?;
    if let Some((format_name, is_netcdf)) = format {
        if mode == "gray" || mode == "raw" {
            return Err("--raw and --gray apply only to local images".into());
        }
        if frame_index.is_some() {
            return Err("--frame-index applies only to local multi-frame images".into());
        }
        if let Some(valid_time) = valid_time {
            radiust_core::model::parse_utc_time(valid_time)
                .map_err(|_| "--at time must be ISO-8601 with a timezone".to_owned())?;
        }
        let limits = Limits {
            max_artifact_bytes: config.runtime.max_artifact_bytes,
            max_frame_bytes: config.runtime.max_frame_bytes,
            max_pixels: config.runtime.max_pixels,
            max_temp_bytes: config.runtime.max_temp_bytes,
            ..Limits::default()
        };
        let field = if is_netcdf {
            let variable = variable.ok_or_else(|| {
                "NetCDF preview requires --variable NAME; multi-time NetCDF requires --at TIME"
                    .to_owned()
            })?;
            radiust_core::output::netcdf::read_selected_field(path, variable, valid_time, &limits)
                .map_err(|error| {
                let message = error.to_string();
                match message.as_str() {
                    "storage error: NetCDF variable was not found" => {
                        format!("NetCDF variable {variable} was not found")
                    }
                    "storage error: requested time is absent from the NetCDF variable" => {
                        format!(
                            "NetCDF file has no frame at {}",
                            valid_time.unwrap_or("the requested time")
                        )
                    }
                    _ => message,
                }
            })?
        } else if format_name == "GeoTIFF" {
            let field = radiust_core::output::geotiff::read_selected_field(path, &limits)
                .map_err(|error| error.to_string())?;
            validate_local_field_selection(&field, variable, valid_time)?;
            field
        } else {
            let field = radiust_core::output::zarr::read_selected_field(path, variable, &limits)
                .map_err(|error| error.to_string())?;
            validate_local_field_selection(&field, variable, valid_time)?;
            field
        };
        if mode == "dbz" && (field.name != "reflectivity" || field.units.as_deref() != Some("dBZ"))
        {
            let message = format!(
                "--dbz requires reflectivity in dBZ; selected {} [{}]",
                field.name,
                field.units.as_deref().unwrap_or("unknown")
            );
            if as_json {
                let error = report::with_mode_info(
                    report::error_envelope("unit_mismatch", &message, "decode"),
                    json!({
                        "requested":"dbz", "actual":null,
                        "time_status":"unknown", "geolocation":"unknown"
                    }),
                );
                emit(&error, true)?;
                return Ok(5);
            }
            return Err(message);
        }
        let preview = radiust_core::output::png::preview_field_with_options(
            &field,
            &limits,
            &options.as_png_options(),
        )
        .map_err(|error| error.to_string())?;
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("scientific-data");
        let result = json!({
            "path": name,
            "format": format_name,
            "variable": field.name,
            "valid_time": field.valid_time,
            "units": field.units,
            "width": preview.width,
            "height": preview.height,
            "crs": field.grid.crs,
            "display_mode": if mode == "dbz" { "dbz" } else { "decoded" },
        });
        let text = format!(
            "field path={name} format={format_name} variable={} time={} units={} size={}x{} crs={} display={}",
            field.name,
            field.valid_time,
            field.units.as_deref().unwrap_or("unknown"),
            preview.width,
            preview.height,
            field.grid.crs.as_deref().unwrap_or("unknown"),
            if mode == "dbz" { "dbz" } else { "decoded" },
        );
        let report_value = if mode == "dbz" {
            report::with_mode_info(
                report::envelope("cat", vec![result.clone()], Some(result.clone())),
                json!({
                    "requested":"dbz", "actual":"dbz", "variable":field.name,
                    "units":field.units, "method":"native_numeric", "encoding":null,
                    "range_policy":null, "time_status":"known", "geolocation":
                        if field.grid.crs.is_some() { "known" } else { "unknown" },
                    "limitations":[]
                }),
            )
        } else {
            report::with_mode_info(
                report::envelope("cat", vec![result.clone()], Some(result.clone())),
                json!({
                    "requested":"scientific", "actual":"scientific", "variable":field.name,
                    "units":field.units, "method":"native_numeric", "encoding":null,
                    "time_status":"known", "geolocation":if field.grid.crs.is_some() { "known" } else { "unknown" },
                    "limitations":[]
                }),
            )
        };
        if as_json {
            emit(&report_value, true)?;
            Ok(0)
        } else {
            render_cat_preview(&preview, &text, renderer, options.width, options.height)?;
            Ok(0)
        }
    } else {
        if valid_time.is_some() {
            return Err("--at requires a scientific --file".into());
        }
        let limits = Limits {
            max_artifact_bytes: config.runtime.max_artifact_bytes,
            max_pixels: config.runtime.max_pixels,
            max_temp_bytes: config.runtime.max_temp_bytes,
            ..Limits::default()
        };
        if mode == "dbz" {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(config.runtime.decode_workers.max(1))
                .enable_all()
                .build()
                .map_err(|_| "native runtime could not be initialized".to_owned())?;
            let engine = Engine::new(config.clone(), SourceRegistry::default())
                .map_err(|error| error.to_string())?;
            let result =
                match runtime.block_on(engine.decode_gray_file(path.to_path_buf(), frame_index)) {
                    Ok(result) => result,
                    Err(error) => {
                        let code = local_dbz_error_code(&error);
                        if as_json {
                            emit(&report::local_mode_error(code, &error.to_string(), "dbz"), true)?;
                            return Ok(5);
                        }
                        return Err(error.to_string());
                    }
                };
            let RasterResultData::Pixel(field) = &result.data else {
                return Err("local gray decoding did not produce a pixel field".into());
            };
            let preview = radiust_core::output::png::preview_pixel_dbz_with_options(
                field,
                &limits,
                &options.as_png_options(),
            )
            .map_err(|error| error.to_string())?;
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("image");
            let text = format!(
                "field path={name} variable=reflectivity units=dBZ size={}x{} display=dbz encoding=gray-dbz-v1 range=strict-v1",
                preview.width, preview.height,
            );
            if as_json {
                let envelope = report::with_mode_info(
                    report::envelope("cat", Vec::new(), Some(json!(text))),
                    serde_json::to_value(&result.mode_info).unwrap_or(Value::Null),
                );
                emit(&envelope, true)?;
            } else {
                render_cat_preview(&preview, &text, renderer, options.width, options.height)?;
            }
            return Ok(0);
        }
        if frame_index.is_some() {
            return Err("--frame-index currently applies to --dbz image decoding".into());
        }
        let preview = preview_file(path, &limits).map_err(|error| error.to_string())?;
        if mode == "gray" && !preview_is_gray(&preview.preview.rgba) {
            let message = "visible image pixels are not grayscale";
            if as_json {
                emit(&report::local_mode_error("invalid_gray_encoding", message, "gray"), true)?;
                return Ok(5);
            }
            return Err(message.into());
        }
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("image");
        let text = format!(
            "image path={name} {} source=unknown product=unknown station=unknown time=unknown units={} format={} size={}x{} sha256={} display=original rule=unknown; original source pixels preserved",
            mode,
            if mode == "gray" { "gray_code" } else { "unknown" },
            preview.format,
            preview.preview.width,
            preview.preview.height,
            preview.sha256,
        );
        if as_json {
            let envelope = report::with_mode_info(
                report::envelope("cat", Vec::new(), Some(json!(text))),
                report::local_mode_info(mode, Some(mode)),
            );
            emit(&envelope, true)?;
            Ok(0)
        } else {
            render_cat_preview(&preview.preview, &text, renderer, options.width, options.height)?;
            Ok(0)
        }
    }
}

fn preview_is_gray(rgba: &[u8]) -> bool {
    rgba.len().is_multiple_of(4)
        && rgba
            .chunks_exact(4)
            .all(|pixel| pixel[3] == 0 || (pixel[0] == pixel[1] && pixel[1] == pixel[2]))
}

fn local_dbz_error_code(error: &EngineError) -> &'static str {
    match error {
        EngineError::Core(radiust_core::errors::CoreError::InvalidGrayEncoding { .. }) => {
            "invalid_gray_encoding"
        }
        EngineError::Core(radiust_core::errors::CoreError::ResourceLimit(_)) => "resource_limit",
        EngineError::Core(radiust_core::errors::CoreError::Cancelled) => "cancelled",
        EngineError::Core(radiust_core::errors::CoreError::UnitMismatch { .. }) => "unit_mismatch",
        _ => "decode",
    }
}

fn validate_local_field_selection(
    field: &radiust_core::model::RadarField,
    variable: Option<&str>,
    valid_time: Option<&str>,
) -> Result<(), String> {
    if let Some(requested) = variable
        && requested != field.name
    {
        return Err(format!(
            "selected variable {} does not match requested variable {requested}",
            field.name
        ));
    }
    if let Some(requested) = valid_time {
        let requested_time = radiust_core::model::parse_utc_time(requested)
            .map_err(|_| "--at time must be ISO-8601 with a timezone".to_owned())?;
        let field_time = radiust_core::model::parse_utc_time(&field.valid_time)
            .map_err(|_| "selected field has an invalid valid_time".to_owned())?;
        if requested_time != field_time {
            return Err(format!("file has no frame at {requested}"));
        }
    }
    Ok(())
}

struct CatSourcePreview {
    preview: radiust_core::model::Preview,
    result: Value,
    mode_info: Value,
    text: String,
}

enum CatSourceOutcome {
    Preview(CatSourcePreview),
    Interrupted,
}

impl CatSourceOutcome {
    fn exit_code(&self) -> u8 {
        match self {
            Self::Preview(_) => 0,
            Self::Interrupted => 130,
        }
    }
}

#[cfg(test)]
async fn cat_source_operation<F>(
    engine: &Engine,
    config: &CoreConfig,
    query: Query,
    gray_mode: bool,
    after_raw_acquisition: F,
) -> Result<CatSourceOutcome, String>
where
    F: Future<Output = ()>,
{
    cat_source_operation_with_mode(
        engine,
        config,
        query,
        gray_mode,
        gray_mode,
        None,
        None,
        CatRenderOptions::default(),
        after_raw_acquisition,
    )
    .await
}

async fn cat_source_operation_with_mode<F>(
    engine: &Engine,
    config: &CoreConfig,
    query: Query,
    gray_mode: bool,
    compatibility_display_mode: bool,
    science_mode: Option<&str>,
    variable: Option<String>,
    options: CatRenderOptions,
    after_raw_acquisition: F,
) -> Result<CatSourceOutcome, String>
where
    F: Future<Output = ()>,
{
    if science_mode.is_some() && gray_mode {
        return Err("--gray cannot be combined with --raw, --decoded, or --dbz".into());
    }
    if science_mode.is_none() && options.has_science_options() {
        return Err("raw image preview does not accept scientific decoding options".into());
    }
    options.validate()?;
    let report = engine.discover(query).await.map_err(|error| error.to_string())?;
    if report.interrupted {
        return Ok(CatSourceOutcome::Interrupted);
    }
    let candidates = report.items.iter().filter_map(|item| item.frame.as_ref()).collect::<Vec<_>>();
    if candidates.len() != 1 {
        let message = if candidates.len() > 1 {
            format!(
                "raw preview requires a unique frame; found {}; select --product/--station/--at",
                candidates.len()
            )
        } else {
            report
                .items
                .iter()
                .find_map(|item| item.error.as_ref().map(|error| error.message.clone()))
                .unwrap_or_else(|| "source returned no matching frame".into())
        };
        return Err(message);
    }

    let frame = candidates[0].clone();
    if science_mode == Some("scientific") && !supports_decoded_frame(&frame) {
        return Err(format!(
            "native --decoded preview supports only rainviewer/composite or tw/grid; selected {}/{}",
            frame.source, frame.product
        ));
    }
    let raw = match engine.fetch_raw(frame.clone()).await {
        Ok(raw) => raw,
        Err(EngineError::Core(radiust_core::errors::CoreError::Cancelled)) => {
            return Ok(CatSourceOutcome::Interrupted);
        }
        Err(error) => return Err(error.to_string()),
    };
    after_raw_acquisition.await;
    if science_mode == Some("dbz") {
        let raster = engine.decode_dbz(Arc::new(raw)).await.map_err(|error| error.to_string())?;
        let limits = Limits {
            max_artifact_bytes: config.runtime.max_artifact_bytes,
            max_frame_bytes: config.runtime.max_frame_bytes,
            max_pixels: config.runtime.max_pixels,
            max_temp_bytes: config.runtime.max_temp_bytes,
            ..Limits::default()
        };
        let mut preview = match &raster.data {
            radiust_core::raster::RasterResultData::Native(field) => {
                if variable.as_deref().is_some_and(|name| name != field.name) {
                    return Err(format!(
                        "native {} decoder provides only variable {}; requested {}",
                        frame.source,
                        field.name,
                        variable.as_deref().unwrap_or_default()
                    ));
                }
                radiust_core::output::png::preview_field_with_options(
                    field,
                    &limits,
                    &options.as_png_options(),
                )
            }
            radiust_core::raster::RasterResultData::Pixel(field) => {
                if variable.as_deref().is_some_and(|name| name != "reflectivity") {
                    return Err("source gray decoding provides only variable reflectivity".into());
                }
                radiust_core::output::png::preview_pixel_dbz_with_options(
                    field,
                    &limits,
                    &options.as_png_options(),
                )
            }
            radiust_core::raster::RasterResultData::NativeDataset { .. } => {
                return Err("dbz preview requires a single reflectivity field".into());
            }
        }
        .map_err(|error| error.to_string())?;
        preview.frame = Some(frame.clone());
        preview.mode = PreviewMode::Decoded;
        let mode_info = serde_json::to_value(&raster.mode_info)
            .map_err(|error| format!("dBZ mode information could not be serialized: {error}"))?;
        let (width, height, crs, valid_time) = match &raster.data {
            radiust_core::raster::RasterResultData::Native(field) => (
                preview.width,
                preview.height,
                field.grid.crs.clone(),
                Some(field.valid_time.clone()),
            ),
            radiust_core::raster::RasterResultData::Pixel(field) => {
                (field.width as u32, field.height as u32, None, field.valid_time.clone())
            }
            radiust_core::raster::RasterResultData::NativeDataset { .. } => unreachable!(),
        };
        let mut result = json!({
            "source": frame.source,
            "product": frame.product,
            "station": frame.station,
            "valid_time": valid_time,
            "logical_id": frame.logical_id,
            "variable": "reflectivity",
            "units": "dBZ",
            "width": width,
            "height": height,
            "crs": crs,
            "display_mode": "dbz",
            "source_urls": public_source_urls(&frame),
        });
        result["mode_info"] = mode_info.clone();
        let text = format!(
            "field source={} product={} station={} variable=reflectivity time={} units=dBZ size={}x{} crs={} display=dbz",
            safe_summary_text(&frame.source),
            safe_summary_text(&frame.product),
            safe_summary_text(frame.station.as_deref().unwrap_or("unknown")),
            safe_summary_text(valid_time.as_deref().unwrap_or("unknown")),
            width,
            height,
            safe_summary_text(crs.as_deref().unwrap_or("unknown")),
        );
        return Ok(CatSourceOutcome::Preview(CatSourcePreview {
            preview,
            result,
            mode_info,
            text,
        }));
    }
    if science_mode.is_some() {
        let field =
            engine.decode_science(Arc::new(raw)).await.map_err(|error| error.to_string())?;
        if science_mode == Some("dbz")
            && (field.name != "reflectivity" || field.units.as_deref() != Some("dBZ"))
        {
            return Err(format!(
                "--dbz requires reflectivity in dBZ; selected {} [{}]",
                field.name,
                field.units.as_deref().unwrap_or("unknown")
            ));
        }
        if variable.as_deref().is_some_and(|name| name != field.name) {
            return Err(format!(
                "native {} decoder provides only variable {}; requested {}",
                frame.source,
                field.name,
                variable.as_deref().unwrap_or_default()
            ));
        }
        let limits = Limits {
            max_artifact_bytes: config.runtime.max_artifact_bytes,
            max_frame_bytes: config.runtime.max_frame_bytes,
            max_pixels: config.runtime.max_pixels,
            max_temp_bytes: config.runtime.max_temp_bytes,
            ..Limits::default()
        };
        let mut preview = radiust_core::output::png::preview_field_with_options(
            &field,
            &limits,
            &options.as_png_options(),
        )
        .map_err(|error| error.to_string())?;
        preview.frame = Some(frame.clone());
        let name = safe_summary_text(&field.name);
        let source = safe_summary_text(&frame.source);
        let product = safe_summary_text(&frame.product);
        let station = safe_summary_text(frame.station.as_deref().unwrap_or("unknown"));
        let time = safe_summary_text(&field.valid_time);
        let units = safe_summary_text(field.units.as_deref().unwrap_or("unknown"));
        let crs = safe_summary_text(field.grid.crs.as_deref().unwrap_or("unknown"));
        let mut result = json!({
            "source": frame.source,
            "product": frame.product,
            "station": frame.station,
            "valid_time": field.valid_time,
            "logical_id": frame.logical_id,
            "variable": field.name,
            "units": field.units,
            "width": preview.width,
            "height": preview.height,
            "crs": field.grid.crs,
            "display_mode": if science_mode == Some("dbz") { "dbz" } else { "decoded" },
            "rule_version": preview.rule_version,
            "source_urls": public_source_urls(&frame),
        });
        let text = format!(
            "field source={source} product={product} station={station} variable={name} time={time} units={units} size={}x{} crs={crs} display={}",
            preview.width,
            preview.height,
            science_mode.unwrap_or("scientific"),
        );
        let mode_info = report::native_mode_info(
            science_mode.unwrap_or("scientific"),
            science_mode.unwrap_or("scientific"),
            &field.name,
            field.units.as_deref(),
            "known",
            if field.grid.crs.is_some() { "known" } else { "unknown" },
        );
        result["mode_info"] = mode_info.clone();
        return Ok(CatSourceOutcome::Preview(CatSourcePreview {
            preview,
            result,
            mode_info,
            text,
        }));
    }
    let image_artifacts = raw
        .artifacts
        .iter()
        .filter(|artifact| artifact.receipt.media_type.starts_with("image/"))
        .collect::<Vec<_>>();
    if image_artifacts.is_empty() {
        return Err("selected frame has no image artifact for raw preview".into());
    }
    let limits = Limits {
        max_artifact_bytes: config.runtime.max_artifact_bytes,
        max_pixels: config.runtime.max_pixels,
        max_temp_bytes: config.runtime.max_temp_bytes,
        ..Limits::default()
    };
    let mut preview = if image_artifacts.len() == 1 {
        radiust_core::preview::preview_artifact(image_artifacts[0], &limits)
            .map_err(|error| error.to_string())?
    } else {
        let tiles = radiust_core::source::tiles::preview_source_tiles(&raw, &limits)
            .map_err(|error| error.to_string())?;
        radiust_core::preview::RawImagePreview {
            preview: tiles.preview,
            format: tiles.format,
            size_bytes: tiles.size_bytes,
            sha256: tiles.sha256,
        }
    };
    preview.preview.frame = Some(frame.clone());
    preview.preview.mode = PreviewMode::Raw;
    let mut gray_result = if gray_mode {
        Some(
            radiust_core::gray::apply_for_source(
                &frame.source,
                &frame.product,
                frame.station.as_deref(),
                &preview.format,
                preview.preview.width,
                preview.preview.height,
                &preview.preview.rgba,
                &limits,
            )
            .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    if let Some(gray) = &mut gray_result {
        preview.preview.width = gray.width;
        preview.preview.height = gray.height;
        preview.preview.rgba = std::mem::take(&mut gray.rgba);
        preview.preview.mode = if gray.applied {
            if compatibility_display_mode { PreviewMode::LegacyDisplay } else { PreviewMode::Gray }
        } else {
            PreviewMode::Raw
        };
        preview.preview.rule_version = gray.rule_version.clone();
    }
    let display_mode = gray_result
        .as_ref()
        .map(|gray| {
            if gray.applied {
                if compatibility_display_mode { "legacy" } else { "gray" }
            } else if compatibility_display_mode {
                "original"
            } else {
                "raw"
            }
        })
        .unwrap_or("raw");
    let mut result = json!({
        "source": frame.source,
        "product": frame.product,
        "station": frame.station,
        "valid_time": frame.valid_time,
        "logical_id": frame.logical_id,
        "format": preview.format,
        "size_bytes": preview.size_bytes,
        "width": preview.preview.width,
        "height": preview.preview.height,
        "sha256": preview.sha256,
        "display_mode": display_mode,
        "source_urls": public_source_urls(&frame),
    });
    if let Some(gray) = &gray_result {
        result["rule_version"] = json!(gray.rule_version.clone());
        result["display_reason"] = json!(gray.reason.clone());
    }
    let text = if let Some(gray) = &gray_result {
        format!(
            "image source={} product={} station={} time={} format={} size={}x{} sha256={} display={} rule={}; {}",
            safe_summary_text(&frame.source),
            safe_summary_text(&frame.product),
            safe_summary_text(frame.station.as_deref().unwrap_or("unknown")),
            safe_summary_text(&frame.valid_time),
            preview.format,
            preview.preview.width,
            preview.preview.height,
            preview.sha256,
            display_mode,
            gray.rule_version.as_deref().unwrap_or("unknown"),
            safe_summary_text(gray.reason.as_deref().unwrap_or("original source pixels preserved"),),
        )
    } else {
        format!(
            "image source={} product={} station={} time={} format={} size={}x{} sha256={} display=raw",
            safe_summary_text(&frame.source),
            safe_summary_text(&frame.product),
            safe_summary_text(frame.station.as_deref().unwrap_or("unknown")),
            safe_summary_text(&frame.valid_time),
            preview.format,
            preview.preview.width,
            preview.preview.height,
            preview.sha256,
        )
    };
    let mode_info = if let Some(gray) = &gray_result {
        report::gray_source_mode_info(
            if gray.applied { "gray" } else { "raw" },
            gray.rule_version.as_deref(),
            gray.reason.as_deref(),
        )
    } else {
        report::native_mode_info("raw", "raw", "", None, "known", "unknown")
    };
    result["mode_info"] = mode_info.clone();
    Ok(CatSourceOutcome::Preview(CatSourcePreview {
        preview: preview.preview,
        result,
        mode_info,
        text,
    }))
}

#[cfg(test)]
async fn cat_source_cancellable<F, H>(
    engine: &Engine,
    config: &CoreConfig,
    query: Query,
    gray_mode: bool,
    cancellation: F,
    after_raw_acquisition: H,
) -> Result<CatSourceOutcome, String>
where
    F: Future<Output = std::io::Result<()>>,
    H: Future<Output = ()>,
{
    cat_source_cancellable_with_mode(
        engine,
        config,
        query,
        gray_mode,
        gray_mode,
        None,
        None,
        CatRenderOptions::default(),
        cancellation,
        after_raw_acquisition,
    )
    .await
}

async fn cat_source_cancellable_with_mode<F, H>(
    engine: &Engine,
    config: &CoreConfig,
    query: Query,
    gray_mode: bool,
    compatibility_display_mode: bool,
    science_mode: Option<&str>,
    variable: Option<String>,
    options: CatRenderOptions,
    cancellation: F,
    after_raw_acquisition: H,
) -> Result<CatSourceOutcome, String>
where
    F: Future<Output = std::io::Result<()>>,
    H: Future<Output = ()>,
{
    let operation = cat_source_operation_with_mode(
        engine,
        config,
        query,
        gray_mode,
        compatibility_display_mode,
        science_mode,
        variable,
        options,
        after_raw_acquisition,
    );
    tokio::pin!(operation);
    tokio::select! {
        biased;
        signal = cancellation => {
            signal.map_err(|_| "native runtime could not be initialized".to_owned())?;
            engine.cancel();
            // Keep polling the operation until the Engine has unwound its active
            // work. Any RawFrame it produced is then dropped before we return.
            let _ = operation.await;
            Ok(CatSourceOutcome::Interrupted)
        }
        result = &mut operation => result,
    }
}

fn cat_interrupted_envelope() -> Value {
    let mut payload = report::envelope("cat", Vec::new(), None);
    payload["error"] = json!({
        "code": "cancelled",
        "message": "operation cancelled",
        "stage": "cat",
        "retryable": false,
    });
    payload["interrupted"] = json!(true);
    payload
}

fn cat_source(
    config: CoreConfig,
    query: Query,
    gray_mode: bool,
    compatibility_display_mode: bool,
    science_mode: Option<&str>,
    variable: Option<String>,
    options: CatRenderOptions,
    renderer: &str,
    as_json: bool,
    progress_enabled: bool,
) -> Result<u8, String> {
    let renderer = resolve_preview_renderer(renderer, std::io::stdout().is_terminal(), as_json)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.runtime.decode_workers.max(1))
        .enable_all()
        .build()
        .map_err(|_| "native runtime could not be initialized".to_owned())?;
    let engine = Engine::new(config.clone(), SourceRegistry::default())
        .map_err(|error| error.to_string())?;
    let outcome = runtime.block_on(with_progress(
        &engine,
        cat_source_cancellable_with_mode(
            &engine,
            &config,
            query,
            gray_mode,
            compatibility_display_mode,
            science_mode,
            variable,
            options.clone(),
            tokio::signal::ctrl_c(),
            std::future::ready(()),
        ),
        progress_enabled,
    ));
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) if as_json => {
            let network_preflight = error.contains("public network access is disabled");
            let code = if error.contains("requires reflectivity in dBZ") {
                "unit_mismatch"
            } else if error.contains("supports only") {
                "unsupported"
            } else {
                "decode"
            };
            let stage =
                if code == "unsupported" || network_preflight { "validate" } else { "decode" };
            let requested = science_mode.unwrap_or(if gray_mode { "gray" } else { "raw" });
            let report = report::with_mode_info(
                report::error_envelope(code, &error, stage),
                report::failed_mode_info(requested),
            );
            emit(&report, true)?;
            return Ok(if code == "unsupported" || network_preflight { 2 } else { 5 });
        }
        Err(error) => return Err(error),
    };
    let exit_code = outcome.exit_code();
    match outcome {
        CatSourceOutcome::Interrupted => {
            if as_json {
                emit(&cat_interrupted_envelope(), true)?;
            }
            Ok(exit_code)
        }
        CatSourceOutcome::Preview(preview) => {
            if as_json {
                emit(
                    &report::with_mode_info(
                        report::envelope("cat", vec![preview.result.clone()], Some(preview.result)),
                        preview.mode_info,
                    ),
                    true,
                )?;
            } else {
                render_cat_preview(
                    &preview.preview,
                    &preview.text,
                    renderer,
                    options.width,
                    options.height,
                )?;
            }
            Ok(exit_code)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreviewRenderer {
    Text,
    Ansi,
    Kitty,
    Iterm2,
}

fn terminal_color_disabled() -> bool {
    std::env::var_os("NO_COLOR").is_some()
        || std::env::var("TERM").is_ok_and(|term| term == "dumb")
        || std::env::var("COLORTERM").is_ok_and(|value| value == "0")
}

fn resolve_preview_renderer(
    requested: &str,
    is_terminal: bool,
    as_json: bool,
) -> Result<PreviewRenderer, String> {
    let color_disabled = terminal_color_disabled();
    let kitty = std::env::var_os("KITTY_WINDOW_ID").is_some();
    let iterm2 = std::env::var("TERM_PROGRAM").is_ok_and(|value| value == "iTerm.app");
    resolve_preview_renderer_with_capabilities(
        requested,
        is_terminal,
        as_json,
        color_disabled,
        kitty,
        iterm2,
    )
}

fn resolve_preview_renderer_with_capabilities(
    requested: &str,
    is_terminal: bool,
    as_json: bool,
    color_disabled: bool,
    kitty_capable: bool,
    iterm2_capable: bool,
) -> Result<PreviewRenderer, String> {
    if as_json {
        return Ok(PreviewRenderer::Text);
    }
    let kitty = is_terminal && !color_disabled && kitty_capable;
    let iterm2 = is_terminal && !color_disabled && iterm2_capable;
    match requested {
        "auto" => Ok(if !is_terminal || color_disabled {
            PreviewRenderer::Text
        } else if kitty {
            PreviewRenderer::Kitty
        } else if iterm2 {
            PreviewRenderer::Iterm2
        } else {
            PreviewRenderer::Ansi
        }),
        "text" => Ok(PreviewRenderer::Text),
        "ansi" if !is_terminal => {
            Err("renderer ansi requires a TTY; use --renderer text for redirected output".into())
        }
        "ansi" if color_disabled => Ok(PreviewRenderer::Text),
        "ansi" => Ok(PreviewRenderer::Ansi),
        "kitty" if !is_terminal => {
            Err("renderer kitty requires a TTY; use --renderer text for redirected output".into())
        }
        "kitty" if !kitty => Err("Kitty graphics capability was not confirmed".into()),
        "kitty" => Ok(PreviewRenderer::Kitty),
        "iterm2" if !is_terminal => {
            Err("renderer iterm2 requires a TTY; use --renderer text for redirected output".into())
        }
        "iterm2" if !iterm2 => Err("iTerm2 inline-image capability was not confirmed".into()),
        "iterm2" => Ok(PreviewRenderer::Iterm2),
        _ => Err("renderer must be auto, kitty, iterm2, ansi, or text".into()),
    }
}

fn render_cat_preview(
    preview: &radiust_core::model::Preview,
    text: &str,
    renderer: PreviewRenderer,
    width: Option<usize>,
    height: Option<usize>,
) -> Result<(), String> {
    match renderer {
        PreviewRenderer::Text => println!("{text}"),
        PreviewRenderer::Ansi => print!("{}", render_preview_ansi(preview, width, height)?),
        PreviewRenderer::Kitty => {
            print!("{}", render_preview_kitty(preview)?);
            println!();
        }
        PreviewRenderer::Iterm2 => {
            print!("{}", render_preview_iterm2(preview, width, height)?);
            println!();
        }
    }
    Ok(())
}

fn render_preview_ansi(
    preview: &radiust_core::model::Preview,
    width: Option<usize>,
    height: Option<usize>,
) -> Result<String, String> {
    let columns = width.unwrap_or_else(|| {
        std::env::var("COLUMNS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(120)
            .min(240)
    });
    let rows = height.unwrap_or_else(|| {
        std::env::var("LINES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 1)
            .map(|value| value - 1)
            .unwrap_or(40)
            .min(120)
    });
    terminal::render_ansi(preview, columns, rows)
}

fn render_preview_kitty(preview: &radiust_core::model::Preview) -> Result<String, String> {
    let encoded = BASE64.encode(&encode_preview_png(preview)?);
    let chunks = encoded.as_bytes().chunks(4096).collect::<Vec<_>>();
    let mut output = String::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let payload = std::str::from_utf8(chunk)
            .map_err(|_| "PNG base64 encoding produced invalid text".to_owned())?;
        output.push_str(&format!(
            "\x1b_Ga=T,f=100,m={};{}\x1b\\",
            usize::from(index + 1 < chunks.len()),
            payload
        ));
    }
    Ok(output)
}

fn render_preview_iterm2(
    preview: &radiust_core::model::Preview,
    width: Option<usize>,
    height: Option<usize>,
) -> Result<String, String> {
    let encoded = BASE64.encode(&encode_preview_png(preview)?);
    let mut attributes = vec!["inline=1".to_owned()];
    if let Some(width) = width {
        attributes.push(format!("width={width}"));
    }
    if let Some(height) = height {
        attributes.push(format!("height={height}"));
    }
    Ok(format!("\x1b]1337;File={}:{}\x07", attributes.join(";"), encoded))
}

fn encode_preview_png(preview: &radiust_core::model::Preview) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    preview.validate().map_err(|_| "preview pixels are invalid".to_owned())?;
    let mut png = Vec::new();
    png.try_reserve(preview.rgba.len())
        .map_err(|_| "preview PNG exceeds available memory".to_owned())?;
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&preview.rgba, preview.width, preview.height, image::ExtendedColorType::Rgba8)
        .map_err(|error| format!("preview PNG encoding failed: {error}"))?;
    Ok(png)
}

fn safe_summary_text(value: &str) -> String {
    value.chars().map(|character| if character.is_control() { ' ' } else { character }).collect()
}

fn cache_status(
    config: &mut CoreConfig,
    cache_dir: Option<PathBuf>,
    output_root: Option<PathBuf>,
) -> Result<Value, String> {
    if let Some(path) = cache_dir {
        config.cache.dir = path;
    }
    if let Some(path) = output_root {
        config.storage.output = path;
    }
    let cache = Cache::open(&config.cache.dir).map_err(|error| error.to_string())?;
    let entries = cache.index.entries().map_err(|error| error.to_string())?;
    let count_files = |directory: &str| -> usize {
        std::fs::read_dir(cache.root.join(directory))
            .map(|items| items.filter_map(Result::ok).filter(|item| item.path().is_file()).count())
            .unwrap_or_default()
    };
    Ok(json!({
        "schema_version": 1,
        "enabled": config.cache.enabled,
        "root": cache.root,
        "entries": entries.len(),
        "bytes": entries.iter().map(|entry| entry.size_bytes).sum::<u64>(),
        "objects": entries.iter().filter(|entry| entry.path.starts_with("objects/")).count(),
        "mosaics": entries.iter().filter(|entry| entry.path.starts_with("mosaics/")).count(),
        "leases": count_files("leases"),
        "tmp": count_files("tmp"),
    }))
}

fn cache_gc(
    root: &std::path::Path,
    max_bytes: u64,
    dry_run: bool,
) -> Result<radiust_core::cache::GcReport, String> {
    if dry_run {
        let Some(cache) = Cache::open_for_preview(root).map_err(|error| error.to_string())? else {
            return Ok(radiust_core::cache::GcReport::default());
        };
        cache.gc_preview(max_bytes).map_err(|error| error.to_string())
    } else {
        Cache::open(root).and_then(|cache| cache.gc(max_bytes)).map_err(|error| error.to_string())
    }
}

fn doctor(
    catalog: &SourceCatalog,
    config: &CoreConfig,
    source: Option<&str>,
    network: bool,
) -> Result<Value, String> {
    if let Some(source) = source {
        if catalog.source(source).is_none() {
            return Err(format!("unknown source: {source}"));
        }
    }
    let cache_parent = config.cache.dir.parent().unwrap_or(config.cache.dir.as_path());
    let output_parent = config.storage.output.parent().unwrap_or(config.storage.output.as_path());
    let cache_writable = cache_parent.exists();
    let output_writable = output_parent.exists();
    let mut checks = json!({
        "native_runtime": true,
        "source_catalog": true,
        "cache_writable": cache_writable,
        "output_writable": output_writable,
        "network_probe": "not_requested",
        "source": source,
        "ok": cache_writable && output_writable,
    });
    if network {
        checks["network_probe"] = json!("requested_but_source_probe_is_adapter_specific");
        if let Some(source) = source {
            checks["source_availability"] = json!(catalog.source(source).unwrap().availability);
        }
    }
    if let Some(source) = source {
        if let Some(capabilities) = source_optional_capabilities(
            source,
            config,
            std::env::var_os("PATH").as_deref().unwrap_or_default(),
        ) {
            checks["source_capabilities"] = capabilities;
        }
    }
    Ok(json!({
        "schema_version": 1,
        "command": "doctor",
        "checks": checks
    }))
}

fn source_optional_capabilities(
    source: &str,
    config: &CoreConfig,
    path: &std::ffi::OsStr,
) -> Option<Value> {
    match source {
        "th" => {
            let available = executable_on_path("tesseract", path);
            Some(json!({
                "tesseract": {
                    "status": if available { "available" } else { "missing" },
                    "purpose": "timestamp extraction for TH radar frames",
                    "effect": if available {
                        "timestamp extraction can use the system OCR tool"
                    } else {
                        "timestamp extraction that requires OCR will fail"
                    }
                }
            }))
        }
        "ph" => {
            let chromium_detected =
                radiust_core::source::browser::find_chromium_executable().is_ok();
            Some(json!({
                "timeline_token": {
                    "status": "automatic_session",
                    "value_exposed": false
                },
                "browser_fallback": {
                    "status": if chromium_detected { "chromium_detected" } else { "chromium_missing" },
                    "effect": "PH discovery and data-image acquisition require isolated system Chromium and network opt-in"
                }
            }))
        }
        "windy" => {
            let chromium_detected =
                radiust_core::source::browser::find_chromium_executable().is_ok();
            let requested = config
                .sources
                .get("windy")
                .and_then(|options| options.get("use_playwright"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            Some(json!({
                "http_acquisition": "supported",
                "playwright_acquisition": {
                    "status": if chromium_detected { "chromium_detected" } else { "chromium_missing" },
                    "requested": requested,
                    "effect": "sources.windy.use_playwright=true captures original PNG bytes through isolated Chromium; network opt-in is required"
                }
            }))
        }
        _ => None,
    }
}

fn executable_on_path(name: &str, path: &std::ffi::OsStr) -> bool {
    std::env::split_paths(path).any(|directory| {
        let candidate = directory.join(name);
        let Ok(metadata) = std::fs::metadata(candidate) else {
            return false;
        };
        if !metadata.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    })
}

fn run_cli(cli: Cli) -> Result<u8, String> {
    let progress_enabled = std::io::stderr().is_terminal() && !cli.quiet;
    let catalog = SourceCatalog::builtin().map_err(|error| error.to_string())?;
    let context = commands::Context {
        config_path: cli.config_path.as_deref(),
        json: cli.json,
        quiet: cli.quiet,
        verbose: cli.verbose,
        progress_enabled,
    };
    match cli.command {
        Command::List { kind, source } => {
            emit_cli(
                &commands::list::list(&catalog, &kind, source.as_deref())?,
                cli.json,
                cli.quiet,
                cli.verbose,
                Some("list"),
            )?;
            Ok(0)
        }
        Command::Discover {
            sources,
            product,
            stations,
            latest,
            at,
            start,
            end,
            base_time,
            max_age_secs,
        } => commands::discover::run(
            commands::discover::Args {
                sources,
                product,
                stations,
                latest,
                at,
                start,
                end,
                base_time,
                max_age_secs,
            },
            &context,
        ),
        Command::Download {
            source,
            file,
            dry_run,
            raw_only,
            raw,
            dbz,
            overwrite,
            on_error,
            output,
            output_template,
            cache_dir,
            access_key,
            secret_key,
            endpoint,
            no_cache,
            variable,
            bbox,
            grid,
            resolution,
            resampling,
            format,
            product,
            stations,
            latest,
            at,
            start,
            end,
            base_time,
            max_age_secs,
        } => commands::download::run(
            commands::download::Args {
                source,
                file,
                dry_run,
                raw_only,
                raw,
                dbz,
                overwrite,
                on_error,
                output,
                output_template,
                cache_dir,
                access_key,
                secret_key,
                endpoint,
                no_cache,
                variable,
                bbox,
                grid,
                resolution,
                resampling,
                format,
                product,
                stations,
                latest,
                at,
                start,
                end,
                base_time,
                max_age_secs,
            },
            &context,
        ),
        Command::Replay { manifest, output, formats, overwrite, dbz } => commands::replay::run(
            commands::replay::Args { manifest, output, formats, overwrite, dbz },
            &context,
        ),
        Command::Cat {
            file,
            variable,
            palette,
            vmin,
            vmax,
            width,
            height,
            raw,
            decoded,
            gray,
            dbz,
            frame_index,
            legacy_display,
            source,
            product,
            stations,
            latest,
            at,
            base_time,
            renderer,
        } => {
            let options = CatRenderOptions { palette, vmin, vmax, width, height };
            if legacy_display && (raw || decoded || gray || dbz) {
                return Err(
                    "--legacy-display cannot be combined with --raw, --decoded, --gray, or --dbz"
                        .into(),
                );
            }
            if source.is_some() && !decoded && !dbz && options.has_science_options() {
                return Err("raw image preview does not accept scientific decoding options".into());
            }
            options.validate()?;
            if legacy_display && !cli.json {
                eprintln!("warning: --legacy-display is deprecated; use --gray");
            }
            let gray_requested = gray || legacy_display;
            match (file, source) {
                (Some(path), None) => {
                    if legacy_display {
                        return Err("--legacy-display requires SOURCE".into());
                    }
                    if product.is_some() || !stations.is_empty() || latest || base_time.is_some() {
                        return Err("source query options require SOURCE; local NetCDF selection uses --variable and --at".into());
                    }
                    let config = load_config(cli.config_path.as_deref())?;
                    let mode = if dbz {
                        "dbz"
                    } else if gray_requested {
                        "gray"
                    } else if raw {
                        "raw"
                    } else if decoded {
                        "scientific"
                    } else {
                        "default"
                    };
                    cat_file(
                        &path,
                        variable.as_deref(),
                        at.as_deref(),
                        &renderer,
                        &config,
                        cli.json,
                        &options,
                        mode,
                        frame_index,
                    )
                }
                (None, Some(source)) => {
                    if frame_index.is_some() {
                        return Err("--frame-index applies only to a local --file image".into());
                    }
                    if variable.is_some() && !decoded && !dbz {
                        return Err(
                            "--variable requires --decoded or --dbz for SOURCE previews".into()
                        );
                    }
                    let query = build_query(
                        vec![source],
                        product,
                        stations,
                        latest,
                        at,
                        None,
                        None,
                        base_time,
                        None,
                    )?;
                    if decoded && !supports_native_decoded(&query) {
                        return Err(format!(
                            "native scientific preview supports only rainviewer/composite, tw/grid, or rdcap/reflectivity; selected {}{}",
                            query.source.as_deref().unwrap_or("unknown"),
                            query
                                .product
                                .as_deref()
                                .map_or(String::new(), |product| format!("/{product}")),
                        ));
                    }
                    let config = load_config(cli.config_path.as_deref())?;
                    cat_source(
                        config,
                        query,
                        gray_requested,
                        legacy_display,
                        if dbz {
                            Some("dbz")
                        } else if decoded {
                            Some("scientific")
                        } else {
                            None
                        },
                        variable,
                        options,
                        &renderer,
                        cli.json,
                        progress_enabled,
                    )
                }
                _ => Err("provide exactly one of SOURCE or --file PATH".into()),
            }
        }
        Command::Doctor { source, network } => {
            let config = load_config(cli.config_path.as_deref())?;
            emit_cli(
                &doctor(&catalog, &config, source.as_deref(), network)?,
                cli.json,
                cli.quiet,
                cli.verbose,
                Some("doctor"),
            )?;
            Ok(0)
        }
        Command::Config { command: None | Some(ConfigCommand::Show) } => {
            let config = load_config(cli.config_path.as_deref())?;
            let mut result =
                serde_json::to_value(config.redacted()).map_err(|error| error.to_string())?;
            replace_configured_marker(&mut result);
            emit_cli(
                &report::envelope("config", Vec::new(), Some(result)),
                cli.json,
                cli.quiet,
                cli.verbose,
                Some("config"),
            )?;
            Ok(0)
        }
        Command::Cache { command: CacheCommand::Status { cache_dir, output_root } } => {
            let mut config = load_config(cli.config_path.as_deref())?;
            let result = cache_status(&mut config, cache_dir, output_root)?;
            emit_cli(&result, cli.json, cli.quiet, cli.verbose, Some("cache"))?;
            Ok(0)
        }
        Command::Cache { command: CacheCommand::Gc { cache_dir, output_root, dry_run } } => {
            let mut config = load_config(cli.config_path.as_deref())?;
            if let Some(path) = cache_dir {
                config.cache.dir = path;
            }
            if let Some(path) = output_root {
                config.storage.output = path;
            }
            let report = cache_gc(&config.cache.dir, config.cache.max_bytes, dry_run)?;
            let result = json!({
                "schema_version": 1,
                "operation": "gc",
                "dry_run": dry_run,
                "removed": report.removed,
                "stale_tmp": report.stale_tmp,
                "bytes_before": report.bytes_before,
                "bytes_after": report.bytes_after,
            });
            emit_cli(&result, cli.json, cli.quiet, cli.verbose, Some("cache"))?;
            Ok(0)
        }
        Command::Cache {
            command: CacheCommand::Clear { cache_dir, output_root, dry_run, yes },
        } => {
            if !dry_run && !yes {
                if !std::io::stdout().is_terminal() {
                    return Err("cache clear requires --yes in non-interactive use".into());
                }
                return Err("cache clear requires --yes".into());
            }
            let mut config = load_config(cli.config_path.as_deref())?;
            if let Some(path) = cache_dir {
                config.cache.dir = path;
            }
            if let Some(path) = output_root {
                config.storage.output = path;
            }
            let report = cache_gc(&config.cache.dir, 0, dry_run)?;
            let result = json!({
                "schema_version": 1,
                "operation": "clear",
                "dry_run": dry_run,
                "removed": report.removed,
                "stale_tmp": report.stale_tmp,
                "bytes_before": report.bytes_before,
                "bytes_after": report.bytes_after,
            });
            emit_cli(&result, cli.json, cli.quiet, cli.verbose, Some("cache"))?;
            Ok(0)
        }
    }
}

fn replace_configured_marker(value: &mut Value) {
    match value {
        Value::String(text) if text == "<configured>" => *text = "[REDACTED]".into(),
        Value::Object(values) => {
            for child in values.values_mut() {
                replace_configured_marker(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                replace_configured_marker(child);
            }
        }
        _ => {}
    }
}

pub fn run_args(args: Vec<String>) -> u8 {
    let as_json = args.iter().any(|argument| argument == "--json");
    let local_file_cat = args.iter().any(|argument| argument == "cat")
        && args.iter().any(|argument| argument == "--file");
    let local_file_download = args.iter().any(|argument| argument == "download")
        && args.iter().any(|argument| argument == "--file");
    let requested_local_mode = if args.iter().any(|argument| argument == "--dbz") {
        "dbz"
    } else if args.iter().any(|argument| argument == "--gray") {
        "gray"
    } else {
        "raw"
    };
    let argv = std::iter::once("radiust".to_owned()).chain(args);
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                let _ = error.print();
                return 0;
            }
            if as_json {
                let report = if local_file_cat || local_file_download {
                    report::with_mode_info(
                        report::error_envelope("mode_conflict", &error.to_string(), "validate"),
                        if local_file_download {
                            report::failed_mode_info("dbz")
                        } else {
                            report::local_mode_info(requested_local_mode, None)
                        },
                    )
                } else {
                    report::error_envelope("error", &error.to_string(), "validate")
                };
                let _ = emit(&report, true);
            } else {
                let _ = error.print();
            }
            return 2;
        }
    };
    if cli.quiet && cli.verbose {
        let message = "--quiet and --verbose are mutually exclusive";
        if cli.json {
            let report = report::error_envelope("error", message, "validate");
            let _ = emit(&report, true);
        } else {
            eprintln!("{}", human::error(message));
        }
        return 2;
    }
    match run_cli(cli) {
        Ok(code) => code,
        Err(error) => {
            if as_json {
                let report = report::error_envelope("error", &error, "validate");
                let _ = emit(&report, true);
            } else {
                eprintln!("{}", human::error(&error));
            }
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use radiust_core::errors::CoreError;
    use radiust_core::identity::logical_id;
    use radiust_core::model::{
        ArtifactReceipt, DiscoveryCounts, DiscoveryItem, DiscoveryTarget, FrameRef, RawArtifact,
        RawFrame,
    };
    use radiust_core::source::{SourceAdapter, SourceContext, SourceRegistry};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::{Notify, oneshot};

    #[test]
    fn human_summary_replaces_terminal_control_characters() {
        assert_eq!(safe_summary_text("station\n\u{1b}[31mred\u{1b}[0m\r"), "station  [31mred [0m ");
    }

    #[test]
    fn cat_renderer_choices_keep_tty_capability_json_and_no_color_boundaries() {
        assert_eq!(
            resolve_preview_renderer_with_capabilities("auto", false, false, false, false, false)
                .unwrap(),
            PreviewRenderer::Text
        );
        assert_eq!(
            resolve_preview_renderer_with_capabilities("auto", true, false, true, true, true)
                .unwrap(),
            PreviewRenderer::Text
        );
        assert_eq!(
            resolve_preview_renderer_with_capabilities("auto", true, false, false, true, false)
                .unwrap(),
            PreviewRenderer::Kitty
        );
        assert_eq!(
            resolve_preview_renderer_with_capabilities("auto", true, false, false, false, true)
                .unwrap(),
            PreviewRenderer::Iterm2
        );
        assert_eq!(
            resolve_preview_renderer_with_capabilities("auto", true, false, false, false, false)
                .unwrap(),
            PreviewRenderer::Ansi
        );
        assert_eq!(
            resolve_preview_renderer_with_capabilities("kitty", true, true, false, false, false)
                .unwrap(),
            PreviewRenderer::Text
        );
        assert!(
            resolve_preview_renderer_with_capabilities("ansi", false, false, false, false, false)
                .unwrap_err()
                .contains("requires a TTY")
        );
        assert!(
            resolve_preview_renderer_with_capabilities("kitty", true, false, false, false, false)
                .unwrap_err()
                .contains("capability was not confirmed")
        );
    }

    #[test]
    fn quiet_flag_is_available_before_or_after_the_command() {
        for args in [
            vec!["radiust", "--quiet", "list", "sources"],
            vec!["radiust", "list", "sources", "--quiet"],
        ] {
            assert!(Cli::try_parse_from(args).unwrap().quiet);
        }
    }

    #[test]
    fn query_builder_defaults_to_latest_and_preserves_product_and_station_filters() {
        let implicit = build_query(
            vec!["my".into()],
            Some("composite".into()),
            vec!["east".into(), "peninsular".into()],
            false,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let explicit = build_query(
            vec!["my".into()],
            Some("composite".into()),
            vec!["east".into(), "peninsular".into()],
            true,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(implicit.selector, TimeSelector::Latest);
        assert_eq!(implicit.selector, explicit.selector);
        assert_eq!(implicit.product.as_deref(), Some("composite"));
        assert_eq!(implicit.stations, ["east", "peninsular"]);
        assert_eq!(implicit, explicit);
    }

    #[test]
    fn query_builder_preserves_explicit_time_base_time_range_and_max_age() {
        let at = build_query(
            vec!["my".into()],
            Some("composite".into()),
            vec!["east".into()],
            false,
            Some("2026-09-22T00:00:00Z".into()),
            None,
            None,
            Some("2026-09-21T18:00:00Z".into()),
            None,
        )
        .unwrap();
        assert_eq!(at.selector, TimeSelector::At { time: "2026-09-22T00:00:00Z".into() });
        assert_eq!(at.base_time.as_deref(), Some("2026-09-21T18:00:00Z"));
        assert_eq!(at.stations, ["east"]);

        let latest = build_query(
            vec!["my".into()],
            None,
            Vec::new(),
            false,
            None,
            None,
            None,
            None,
            Some(600.0),
        )
        .unwrap();
        assert_eq!(latest.selector, TimeSelector::Latest);
        assert_eq!(latest.max_age_secs, Some(600.0));

        let range = build_query(
            vec!["my".into()],
            None,
            Vec::new(),
            false,
            None,
            Some("2026-09-22T00:00:00Z".into()),
            Some("2026-09-23T00:00:00Z".into()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            range.selector,
            TimeSelector::Range {
                start: "2026-09-22T00:00:00Z".into(),
                end: "2026-09-23T00:00:00Z".into(),
            }
        );
    }

    #[test]
    fn query_builder_rejects_conflicting_or_malformed_time_selectors() {
        assert!(
            build_query(
                vec!["my".into()],
                None,
                Vec::new(),
                true,
                Some("2026-09-22T00:00:00Z".into()),
                None,
                None,
                None,
                None,
            )
            .unwrap_err()
            .contains("--latest cannot be combined")
        );
        assert!(
            build_query(
                vec!["my".into()],
                None,
                Vec::new(),
                false,
                Some("not-a-time".into()),
                None,
                None,
                None,
                None,
            )
            .unwrap_err()
            .contains("ISO-8601")
        );
        assert!(
            build_query(
                vec!["my".into()],
                None,
                Vec::new(),
                false,
                None,
                Some("2026-09-22T00:00:00Z".into()),
                Some("2026-09-23T00:00:00Z".into()),
                None,
                Some(60.0),
            )
            .unwrap_err()
            .contains("--max-age is only valid with latest")
        );
    }

    #[test]
    fn cat_parser_accepts_variable_and_time_for_local_netcdf() {
        let cli = Cli::try_parse_from([
            "radiust",
            "cat",
            "--file",
            "series.nc",
            "--variable",
            "reflectivity",
            "--at",
            "2026-09-26T02:00:00Z",
            "--palette",
            "default",
            "--vmin",
            "0",
            "--vmax",
            "50",
            "--width",
            "80",
            "--height",
            "24",
            "--renderer",
            "text",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Cat {
                variable: Some(variable),
                at: Some(time),
                palette: Some(palette),
                vmin: Some(vmin),
                vmax: Some(vmax),
                width: Some(width),
                height: Some(height),
                ..
            } if variable == "reflectivity"
                && time == "2026-09-26T02:00:00Z"
                && palette == "default"
                && vmin == 0.0
                && vmax == 50.0
                && width == 80
                && height == 24
        ));
    }

    #[test]
    fn cat_render_options_are_bounded_and_keep_default_palette_semantics() {
        assert!(
            CatRenderOptions {
                palette: Some("default".into()),
                vmin: Some(0.0),
                vmax: Some(50.0),
                width: Some(80),
                height: Some(24)
            }
            .validate()
            .is_ok()
        );
        assert_eq!(
            CatRenderOptions { width: Some(0), ..CatRenderOptions::default() }
                .validate()
                .unwrap_err(),
            "--width and --height must be positive"
        );
        assert!(
            CatRenderOptions { palette: Some("rainbow".into()), ..CatRenderOptions::default() }
                .validate()
                .is_err()
        );
        assert!(
            CatRenderOptions { vmin: Some(10.0), vmax: Some(1.0), ..CatRenderOptions::default() }
                .validate()
                .is_err()
        );
    }

    #[test]
    fn cat_parser_restores_raw_decoded_and_legacy_renderer_choices() {
        for (mode_args, expected_decoded) in [(&["--raw"][..], false), (&["--decoded"][..], true)] {
            let mut args = vec!["radiust", "cat", "rainviewer"];
            args.extend_from_slice(mode_args);
            args.extend(["--renderer", "kitty"]);
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(matches!(
                cli.command,
                Command::Cat { decoded, .. } if decoded == expected_decoded
            ));
        }
        assert!(
            Cli::try_parse_from(["radiust", "cat", "rainviewer", "--raw", "--decoded"]).is_err()
        );
        assert!(
            Cli::try_parse_from(["radiust", "cat", "rainviewer", "--renderer", "iterm2"]).is_ok()
        );
        assert!(
            Cli::try_parse_from(["radiust", "cat", "rainviewer", "--renderer", "ansi"]).is_ok()
        );
        assert!(
            Cli::try_parse_from(["radiust", "cat", "rainviewer", "--renderer", "invalid"]).is_err()
        );
    }

    #[test]
    fn inline_preview_png_round_trips_and_base64_matches_standard_vectors() {
        let preview = radiust_core::model::Preview {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 128],
            frame: None,
            mode: PreviewMode::Raw,
            rule_version: None,
        };
        let encoded = encode_preview_png(&preview).unwrap();
        let decoded = radiust_core::preview::preview_bytes(&encoded, &Limits::default()).unwrap();
        assert_eq!(decoded.preview.rgba, preview.rgba);
        assert_eq!(BASE64.encode(b""), "");
        assert_eq!(BASE64.encode(b"f"), "Zg==");
        assert_eq!(BASE64.encode(b"fo"), "Zm8=");
        assert_eq!(BASE64.encode(b"foo"), "Zm9v");
    }

    #[test]
    fn kitty_chunks_reassemble_to_the_original_rgba() {
        let mut value = 1_u32;
        let preview = radiust_core::model::Preview {
            width: 128,
            height: 128,
            rgba: (0..128 * 128 * 4)
                .map(|_| {
                    value ^= value << 13;
                    value ^= value >> 17;
                    value ^= value << 5;
                    value as u8
                })
                .collect(),
            frame: None,
            mode: PreviewMode::Raw,
            rule_version: None,
        };
        let rendered = render_preview_kitty(&preview).unwrap();
        let chunks = rendered.split("\x1b\\").filter(|chunk| !chunk.is_empty()).collect::<Vec<_>>();
        assert!(chunks.len() > 1);
        let mut encoded = String::new();
        for (index, chunk) in chunks.iter().enumerate() {
            let (header, payload) = chunk.split_once(';').unwrap();
            assert_eq!(
                header,
                format!("\x1b_Ga=T,f=100,m={}", usize::from(index + 1 < chunks.len()))
            );
            assert!(payload.len() <= 4096);
            encoded.push_str(payload);
        }
        let bytes = BASE64.decode(encoded).unwrap();
        let restored = radiust_core::preview::preview_bytes(&bytes, &Limits::default()).unwrap();
        assert_eq!(restored.preview.rgba, preview.rgba);
    }

    #[test]
    fn preview_renderer_honors_explicit_terminal_dimensions() {
        let preview = radiust_core::model::Preview {
            width: 8,
            height: 8,
            rgba: vec![255; 8 * 8 * 4],
            frame: None,
            mode: PreviewMode::Raw,
            rule_version: None,
        };
        let ansi = render_preview_ansi(&preview, Some(3), Some(2)).unwrap();
        assert_eq!(ansi.matches('▀').count(), 6);
        assert_eq!(ansi.lines().count(), 2);

        let iterm = render_preview_iterm2(&preview, Some(80), Some(24)).unwrap();
        assert!(iterm.starts_with("\x1b]1337;File=inline=1;width=80;height=24:"));
        let default_iterm = render_preview_iterm2(&preview, None, None).unwrap();
        assert!(default_iterm.starts_with("\x1b]1337;File=inline=1:"));
    }

    #[test]
    fn download_parser_restores_decoded_processing_defaults_and_choices() {
        let cli = Cli::try_parse_from(["radiust", "download", "rainviewer", "--latest"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Download { grid, resampling, .. }
                if grid == "native" && resampling == "nearest"
        ));

        let cli = Cli::try_parse_from([
            "radiust",
            "download",
            "rainviewer",
            "--raw",
            "--access-key",
            "access",
            "--secret-key",
            "secret",
            "--endpoint",
            "https://objects.invalid",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Download {
                raw: true,
                access_key: Some(access_key),
                secret_key: Some(secret_key),
                endpoint: Some(endpoint),
                ..
            } if access_key == "access" && secret_key == "secret" && endpoint == "https://objects.invalid"
        ));

        assert!(
            Cli::try_parse_from([
                "radiust",
                "download",
                "rainviewer",
                "--grid",
                "geographic",
                "--bbox",
                "-1,-1,1,1",
                "--resolution",
                "0.5",
                "--resampling",
                "bilinear",
                "--variable",
                "reflectivity",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from(["radiust", "download", "rainviewer", "--grid", "mercator",])
                .is_err()
        );
    }

    #[test]
    fn decoded_processing_validation_enforces_legacy_grid_and_raw_only_rules() {
        assert!(matches!(
            validate_download_processing(false, None, "native", None, None, "nearest")
                .unwrap()
                .grid,
            DecodedGrid::Native
        ));
        assert_eq!(
            validate_download_processing(false, None, "native", Some("0,0,1,1"), None, "nearest",)
                .unwrap_err(),
            "--bbox and --resolution require --grid geographic"
        );
        assert_eq!(
            validate_download_processing(false, None, "geographic", None, Some(1.0), "nearest")
                .unwrap_err(),
            "--grid geographic requires --bbox and --resolution"
        );
        assert_eq!(
            validate_download_processing(
                true,
                Some("reflectivity".into()),
                "native",
                None,
                None,
                "nearest",
            )
            .unwrap_err(),
            "--raw-only cannot be combined with decoded processing options"
        );
        assert_eq!(
            validate_download_processing(
                true,
                None,
                "geographic",
                Some("0,0,1,1"),
                Some(0.5),
                "nearest",
            )
            .unwrap_err(),
            "--raw-only cannot be combined with decoded processing options"
        );
        assert!(
            validate_download_processing(
                false,
                None,
                "geographic",
                Some("-1,-1,1,1"),
                Some(0.5),
                "bilinear",
            )
            .is_ok()
        );
        assert!(
            validate_download_processing(
                false,
                None,
                "geographic",
                Some("0,0,181,1"),
                Some(0.5),
                "nearest",
            )
            .is_err()
        );
        assert!(
            validate_download_processing(
                false,
                None,
                "geographic",
                Some("0,0,1,1"),
                Some(f64::INFINITY),
                "nearest",
            )
            .is_err()
        );
    }

    #[test]
    fn cat_parser_accepts_opt_in_legacy_display_for_a_source() {
        let cli = Cli::try_parse_from([
            "radiust",
            "cat",
            "fr",
            "--legacy-display",
            "--product",
            "composite",
            "--station",
            "FRCOMP",
            "--renderer",
            "text",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::Cat { legacy_display: true, .. }));
    }

    #[test]
    fn cat_parser_accepts_canonical_gray_alongside_the_compatibility_flag() {
        let canonical = Cli::try_parse_from(["radiust", "cat", "fr", "--gray"]).unwrap();
        assert!(matches!(
            canonical.command,
            Command::Cat { gray: true, legacy_display: false, .. }
        ));
        let compatibility =
            Cli::try_parse_from(["radiust", "cat", "fr", "--legacy-display"]).unwrap();
        assert!(matches!(
            compatibility.command,
            Command::Cat { gray: false, legacy_display: true, .. }
        ));
    }

    #[test]
    fn download_raw_is_an_attachment_flag_and_conflicts_with_raw_only() {
        let attached = Cli::try_parse_from(["radiust", "download", "rainviewer", "--raw"]).unwrap();
        assert!(matches!(attached.command, Command::Download { raw: true, raw_only: false, .. }));
        assert!(
            Cli::try_parse_from(["radiust", "download", "rainviewer", "--raw", "--raw-only",])
                .is_err()
        );
    }

    #[test]
    fn local_netcdf_cat_requires_time_disambiguation_and_previews_selected_field() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("series.nc");
        let mut file = netcdf::create(&path).unwrap();
        file.add_dimension("time", 2).unwrap();
        file.add_dimension("y", 2).unwrap();
        file.add_dimension("x", 2).unwrap();
        {
            let mut time = file.add_variable::<i64>("time", &["time"]).unwrap();
            time.put_attribute("units", "seconds since 1970-01-01 00:00:00 UTC").unwrap();
            time.put_values(&[1_790_380_800_i64, 1_790_388_000_i64], ..).unwrap();
        }
        {
            let mut data = file.add_variable::<f32>("reflectivity", &["time", "y", "x"]).unwrap();
            data.put_attribute("units", "dBZ").unwrap();
            data.put_values(&[1.0_f32, 2.0, 3.0, 4.0, 11.0, 12.0, 13.0, 14.0], ..).unwrap();
        }
        file.close().unwrap();
        let config = CoreConfig::default();

        let options = CatRenderOptions {
            palette: Some("default".into()),
            vmin: Some(0.0),
            vmax: Some(20.0),
            width: Some(80),
            height: Some(24),
        };
        let ambiguity = cat_file(
            &path,
            Some("reflectivity"),
            None,
            "text",
            &config,
            true,
            &options,
            "scientific",
            None,
        )
        .unwrap_err();
        assert!(ambiguity.contains("time selection is ambiguous"));
        cat_file(
            &path,
            Some("reflectivity"),
            Some("2026-09-26T02:00:00Z"),
            "text",
            &config,
            true,
            &options,
            "scientific",
            None,
        )
        .unwrap();
    }

    #[test]
    fn local_image_cat_honors_the_configured_temporary_memory_limit() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/sources/au/raw/IDR021.T.202609180511.png"
        );
        let mut config = CoreConfig::default();
        config.runtime.max_temp_bytes = 1;

        let error = cat_file(
            std::path::Path::new(path),
            None,
            None,
            "text",
            &config,
            false,
            &CatRenderOptions::default(),
            "raw",
            None,
        )
        .unwrap_err();

        assert!(error.contains("image decode temporary bytes"));
    }

    #[test]
    fn decoded_download_preflight_allows_only_verified_scientific_products() {
        let query = |source: &str, product: Option<&str>| Query {
            source: Some(source.into()),
            product: product.map(str::to_owned),
            ..Query::default()
        };
        assert!(supports_native_decoded(&query("rainviewer", None)));
        assert!(supports_native_decoded(&query("rainviewer", Some("composite"))));
        assert!(supports_native_decoded(&query("tw", Some("grid"))));
        assert!(supports_native_decoded(&query("rdcap", Some("reflectivity"))));
        assert!(!supports_native_decoded(&query("tw", None)));
        assert!(!supports_native_decoded(&query("rdcap", Some("composite"))));
        assert!(!supports_native_decoded(&query("au", Some("composite"))));
    }

    struct CatTestAdapter {
        source_id: &'static str,
        frame: FrameRef,
        additional_frames: Vec<FrameRef>,
        discovery_started: Option<Arc<Notify>>,
        wait_during_discovery: bool,
        fetch_attempts: Option<Arc<AtomicUsize>>,
        raw_fixtures: Option<Vec<CatRawArtifactFixture>>,
    }

    #[derive(Clone)]
    struct CatRawArtifactFixture {
        receipt: ArtifactReceipt,
        bytes: Vec<u8>,
    }

    impl SourceAdapter for CatTestAdapter {
        fn source_id(&self) -> &'static str {
            self.source_id
        }

        fn allows_artifact_host(&self, host: &str) -> bool {
            host == "127.0.0.1"
        }

        fn discover(
            self: Arc<Self>,
            _target: DiscoveryTarget,
            context: SourceContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<FrameRef>, CoreError>> + Send>> {
            if let Some(started) = &self.discovery_started {
                started.notify_one();
            }
            let frames = std::iter::once(self.frame.clone())
                .chain(self.additional_frames.iter().cloned())
                .collect::<Vec<_>>();
            if self.wait_during_discovery {
                Box::pin(async move {
                    tokio::select! {
                        _ = context.request_budget.cancellation.cancelled() => Err(CoreError::Cancelled),
                        _ = std::future::pending::<()>() => Ok(frames),
                    }
                })
            } else {
                Box::pin(async move { Ok(frames) })
            }
        }

        fn fetch_raw(
            self: Arc<Self>,
            frame: FrameRef,
            _context: SourceContext,
            temp_root: PathBuf,
        ) -> Option<Pin<Box<dyn Future<Output = Result<RawFrame, CoreError>> + Send>>> {
            if let Some(fixtures) = self.raw_fixtures.clone() {
                return Some(Box::pin(async move {
                    tokio::fs::create_dir_all(&temp_root)
                        .await
                        .map_err(|_| CoreError::Temporary("test raw directory failed".into()))?;
                    let mut artifacts = Vec::with_capacity(fixtures.len());
                    for fixture in fixtures {
                        let mut file = tempfile::NamedTempFile::new_in(&temp_root)
                            .map_err(|_| CoreError::Temporary("test raw file failed".into()))?;
                        std::io::Write::write_all(&mut file, &fixture.bytes)
                            .map_err(|_| CoreError::Temporary("test raw write failed".into()))?;
                        artifacts.push(RawArtifact {
                            receipt: fixture.receipt,
                            path: file.into_temp_path(),
                        });
                    }
                    Ok(RawFrame { frame, artifacts, private_locator: None })
                }));
            }
            let attempts = self.fetch_attempts.clone()?;
            attempts.fetch_add(1, Ordering::SeqCst);
            Some(Box::pin(async { Err(CoreError::Transport("unexpected preview fetch".into())) }))
        }
    }

    fn cat_test_frame(url: String) -> FrameRef {
        let mut frame = FrameRef {
            source: "au".into(),
            product: "composite".into(),
            station: None,
            valid_time: "2099-01-01T00:00:00.000000Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: Some("cat-cancellation-test".into()),
            locator_version: "cat-test-v1".into(),
            locator: json!({"url": url, "name": "frame.png", "media_type": "image/png"}),
        };
        frame.logical_id = logical_id(&frame).unwrap();
        frame
    }

    fn cat_test_engine(
        config: &CoreConfig,
        frame: FrameRef,
        discovery_started: Option<Arc<Notify>>,
        wait_during_discovery: bool,
    ) -> Engine {
        cat_test_engine_with_frames(
            config,
            vec![frame],
            discovery_started,
            wait_during_discovery,
            None,
            "au",
        )
    }

    fn cat_test_engine_with_frames(
        config: &CoreConfig,
        mut frames: Vec<FrameRef>,
        discovery_started: Option<Arc<Notify>>,
        wait_during_discovery: bool,
        fetch_attempts: Option<Arc<AtomicUsize>>,
        source_id: &'static str,
    ) -> Engine {
        let frame = frames.remove(0);
        let mut overrides = SourceRegistry::default();
        overrides
            .register(Arc::new(CatTestAdapter {
                source_id,
                frame,
                additional_frames: frames,
                discovery_started,
                wait_during_discovery,
                fetch_attempts,
                raw_fixtures: None,
            }))
            .unwrap();
        let mut isolated_config = config.clone();
        isolated_config.cache.enabled = false;
        Engine::new(isolated_config, overrides).unwrap()
    }

    fn rainviewer_cat_fixture() -> (FrameRef, Vec<CatRawArtifactFixture>) {
        let fixture_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/sources/rainviewer");
        let manifest: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/sources/rainviewer/fixture.json"
        ))
        .unwrap();
        let frame_data = &manifest["frames"][0];
        let mut frame = FrameRef {
            source: "rainviewer".into(),
            product: "composite".into(),
            station: None,
            valid_time: frame_data["valid_time"].as_str().unwrap().into(),
            base_time: None,
            logical_id: String::new(),
            revision: frame_data["revision"].as_str().map(str::to_owned),
            locator_version: frame_data["locator_version"].as_str().unwrap().into(),
            locator: frame_data["locator"].clone(),
        };
        frame.logical_id = logical_id(&frame).unwrap();
        let artifacts = frame_data["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|artifact| {
                let path = fixture_root.join(artifact["path"].as_str().unwrap());
                let bytes = std::fs::read(path).unwrap();
                CatRawArtifactFixture {
                    receipt: ArtifactReceipt {
                        name: artifact["name"].as_str().unwrap().into(),
                        media_type: artifact["media_type"].as_str().unwrap().into(),
                        size_bytes: bytes.len() as u64,
                        sha256: artifact["sha256"].as_str().unwrap().into(),
                    },
                    bytes,
                }
            })
            .collect();
        (frame, artifacts)
    }

    fn cat_tile_engine(
        config: &CoreConfig,
        frame: FrameRef,
        raw_fixtures: Vec<CatRawArtifactFixture>,
    ) -> Engine {
        let mut overrides = SourceRegistry::default();
        overrides
            .register(Arc::new(CatTestAdapter {
                source_id: "rainviewer",
                frame,
                additional_frames: Vec::new(),
                discovery_started: None,
                wait_during_discovery: false,
                fetch_attempts: None,
                raw_fixtures: Some(raw_fixtures),
            }))
            .unwrap();
        let mut isolated_config = config.clone();
        isolated_config.cache.enabled = false;
        Engine::new(isolated_config, overrides).unwrap()
    }

    fn cat_source_fixture_engine(
        config: &CoreConfig,
        source_id: &'static str,
        frame: FrameRef,
        raw_fixtures: Vec<CatRawArtifactFixture>,
    ) -> Engine {
        let mut overrides = SourceRegistry::default();
        overrides
            .register(Arc::new(CatTestAdapter {
                source_id,
                frame,
                additional_frames: Vec::new(),
                discovery_started: None,
                wait_during_discovery: false,
                fetch_attempts: None,
                raw_fixtures: Some(raw_fixtures),
            }))
            .unwrap();
        let mut isolated_config = config.clone();
        isolated_config.cache.enabled = false;
        Engine::new(isolated_config, overrides).unwrap()
    }

    fn fr_cat_fixture() -> (FrameRef, Vec<CatRawArtifactFixture>) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/sources/fr");
        let manifest: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/sources/fr/fixture.json"))
                .unwrap();
        let frame_data = &manifest["frames"][0];
        let mut frame = FrameRef {
            source: "fr".into(),
            product: frame_data["product"].as_str().unwrap().into(),
            station: frame_data["station"].as_str().map(str::to_owned),
            valid_time: frame_data["valid_time"].as_str().unwrap().into(),
            base_time: None,
            logical_id: String::new(),
            revision: frame_data["revision"].as_str().map(str::to_owned),
            locator_version: frame_data["locator_version"].as_str().unwrap().into(),
            locator: frame_data["locator"].clone(),
        };
        frame.logical_id = logical_id(&frame).unwrap();
        let fixtures = frame_data["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|artifact| {
                let bytes = std::fs::read(root.join(artifact["path"].as_str().unwrap())).unwrap();
                CatRawArtifactFixture {
                    receipt: ArtifactReceipt {
                        name: artifact["name"].as_str().unwrap().into(),
                        media_type: artifact["media_type"].as_str().unwrap().into(),
                        size_bytes: bytes.len() as u64,
                        sha256: artifact["sha256"].as_str().unwrap().into(),
                    },
                    bytes,
                }
            })
            .collect();
        (frame, fixtures)
    }

    fn cat_test_query() -> Query {
        Query { source: Some("au".into()), product: Some("composite".into()), ..Query::default() }
    }

    fn unique_cat_temp_root() -> std::path::PathBuf {
        static NEXT_ROOT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let sequence = NEXT_ROOT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("radiust-cat-cancel-{}-{nonce}-{sequence}", std::process::id()))
    }

    #[tokio::test]
    async fn source_cat_rejects_multiple_frames_before_raw_acquisition() {
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        let first = cat_test_frame("http://127.0.0.1/first.png".into());
        let mut second = cat_test_frame("http://127.0.0.1/second.png".into());
        second.station = Some("other-station".into());
        second.logical_id = logical_id(&second).unwrap();
        let fetch_attempts = Arc::new(AtomicUsize::new(0));
        let engine = cat_test_engine_with_frames(
            &config,
            vec![first, second],
            None,
            false,
            Some(fetch_attempts.clone()),
            "au",
        );

        let result =
            cat_source_operation(&engine, &config, cat_test_query(), false, std::future::ready(()))
                .await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("ambiguous frames unexpectedly produced a preview"),
        };

        assert!(error.contains("requires a unique frame; found 2"), "{error}");
        assert_eq!(fetch_attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn source_cat_composes_verified_rainviewer_tiles_into_one_raw_preview() {
        let temp_root = unique_cat_temp_root();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.temp_root = Some(temp_root.clone());
        let (frame, raw_fixtures) = rainviewer_cat_fixture();
        let expected_logical_id = frame.logical_id.clone();
        let expected_size_bytes =
            raw_fixtures.iter().map(|fixture| fixture.receipt.size_bytes).sum::<u64>();
        let engine = cat_tile_engine(&config, frame, raw_fixtures);
        let query = Query {
            source: Some("rainviewer".into()),
            product: Some("composite".into()),
            ..Query::default()
        };

        let outcome = cat_source_operation(&engine, &config, query, false, std::future::ready(()))
            .await
            .unwrap();
        let CatSourceOutcome::Preview(preview) = outcome else {
            panic!("verified source tiles should produce a raw preview");
        };

        assert_eq!(preview.result["source"], "rainviewer");
        assert_eq!(preview.result["logical_id"], expected_logical_id);
        assert_eq!(preview.result["format"], "PNG");
        assert_eq!(preview.result["size_bytes"], expected_size_bytes);
        assert_eq!((preview.preview.width, preview.preview.height), (1024, 1024));
        assert_eq!(preview.preview.mode, PreviewMode::Raw);
        assert_eq!(preview.preview.frame.as_ref().unwrap().logical_id, expected_logical_id);
        assert_eq!(preview.preview.rgba.len(), 1024 * 1024 * 4);
        assert_eq!(std::fs::read_dir(&temp_root).unwrap().count(), 0);
        std::fs::remove_dir_all(temp_root).unwrap();
    }

    #[tokio::test]
    async fn source_cat_fetches_and_previews_only_the_unique_raw_frame() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                assert_ne!(read, 0, "client closed before sending the request");
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let png =
                include_bytes!("../../../tests/fixtures/sources/au/raw/IDR021.T.202609180511.png");
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                png.len()
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(png).await.unwrap();
            1
        });

        let temp_root = unique_cat_temp_root();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.temp_root = Some(temp_root.clone());
        let frame = cat_test_frame(format!("http://{address}/selected.png"));
        let expected_logical_id = frame.logical_id.clone();
        let engine = cat_test_engine(&config, frame, None, false);
        let outcome =
            cat_source_operation(&engine, &config, cat_test_query(), false, std::future::ready(()))
                .await
                .unwrap();

        let CatSourceOutcome::Preview(preview) = outcome else {
            panic!("selected raw frame should produce a preview");
        };
        assert_eq!(preview.result["logical_id"], expected_logical_id);
        assert_eq!(preview.result["source"], "au");
        assert_eq!(preview.result["product"], "composite");
        assert_eq!(preview.preview.mode, PreviewMode::Raw);
        assert_eq!(preview.preview.frame.as_ref().unwrap().logical_id, expected_logical_id);
        assert_eq!(&preview.preview.rgba[..4], &[192, 192, 192, 255]);
        assert_eq!(server.await.unwrap(), 1, "cat must not prefetch unrelated frames");
        assert_eq!(std::fs::read_dir(&temp_root).unwrap().count(), 0);
        std::fs::remove_dir_all(temp_root).unwrap();
    }

    #[tokio::test]
    async fn source_cat_applies_only_the_matched_evidence_bound_gray_rule() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                assert_ne!(read, 0, "client closed before sending the request");
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let png = include_bytes!("../../../tests/fixtures/sources/fr/raw/FRCOMP.png");
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                png.len()
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(png).await.unwrap();
        });

        let temp_root = unique_cat_temp_root();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.temp_root = Some(temp_root.clone());
        let mut frame = cat_test_frame(format!("http://{address}/selected.png"));
        frame.source = "fr".into();
        frame.station = Some("FRCOMP".into());
        frame.logical_id = logical_id(&frame).unwrap();
        let expected_logical_id = frame.logical_id.clone();
        let engine = cat_test_engine_with_frames(&config, vec![frame], None, false, None, "fr");
        let query = Query {
            source: Some("fr".into()),
            product: Some("composite".into()),
            stations: vec!["FRCOMP".into()],
            ..Query::default()
        };
        let outcome = cat_source_operation(&engine, &config, query, true, std::future::ready(()))
            .await
            .unwrap();
        let CatSourceOutcome::Preview(preview) = outcome else {
            panic!("verified legacy preview should complete");
        };

        let expected_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/legacy-display/fr/old-gray.png"
        );
        let expected = preview_file(expected_path, &Limits::default()).unwrap();
        assert_eq!(preview.result["logical_id"], expected_logical_id);
        assert_eq!(preview.result["display_mode"], "legacy");
        assert_eq!(preview.result["rule_version"], "old-8d251601-fr-frcomp-replay-v1");
        assert_eq!(preview.result["mode_info"]["requested"], "gray");
        assert_eq!(preview.result["mode_info"]["actual"], "gray");
        assert_eq!(preview.mode_info["actual"], "gray");
        assert_eq!(preview.preview.mode, PreviewMode::LegacyDisplay);
        assert_eq!(preview.preview.width, 700);
        assert_eq!(preview.preview.height, 600);
        assert_eq!(preview.preview.rgba, expected.preview.rgba);
        assert_eq!(std::fs::read_dir(&temp_root).unwrap().count(), 0);
        server.await.unwrap();
        std::fs::remove_dir_all(temp_root).unwrap();
    }

    #[tokio::test]
    async fn source_cat_dbz_uses_the_same_acquired_raw_frame_and_gray_rule() {
        let temp_root = unique_cat_temp_root();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.temp_root = Some(temp_root.clone());
        let (frame, raw_fixtures) = fr_cat_fixture();
        let expected_logical_id = frame.logical_id.clone();
        let engine = cat_source_fixture_engine(&config, "fr", frame, raw_fixtures);
        let query = Query {
            source: Some("fr".into()),
            product: Some("composite".into()),
            stations: vec!["FRCOMP".into()],
            ..Query::default()
        };

        let outcome = cat_source_operation_with_mode(
            &engine,
            &config,
            query,
            false,
            false,
            Some("dbz"),
            Some("reflectivity".into()),
            CatRenderOptions::default(),
            std::future::ready(()),
        )
        .await
        .unwrap();
        let CatSourceOutcome::Preview(preview) = outcome else {
            panic!("verified gray source should produce a dBZ preview");
        };
        assert_eq!(preview.result["logical_id"], expected_logical_id);
        assert_eq!(preview.result["variable"], "reflectivity");
        assert_eq!(preview.result["units"], "dBZ");
        assert_eq!(preview.result["mode_info"]["actual"], "dbz");
        assert_eq!(preview.result["mode_info"]["method"], "verified_source_gray_dbz");
        assert_eq!((preview.preview.width, preview.preview.height), (700, 600));
        assert_eq!(preview.preview.frame.as_ref().unwrap().logical_id, expected_logical_id);
        assert_eq!(std::fs::read_dir(&temp_root).unwrap().count(), 0);
        std::fs::remove_dir_all(temp_root).unwrap();
    }

    fn report_with_counts(counts: DiscoveryCounts, interrupted: bool) -> DiscoveryReport {
        DiscoveryReport {
            schema_version: 1,
            query: Query::default(),
            counts,
            items: Vec::new(),
            interrupted,
        }
    }

    #[test]
    fn aggregate_exit_codes_cover_complete_empty_partial_failure_and_interrupt() {
        let complete = DiscoveryCounts { total: 2, success: 2, ..DiscoveryCounts::default() };
        let empty =
            DiscoveryCounts { total: 2, no_data: 1, stale: 1, ..DiscoveryCounts::default() };
        let partial = DiscoveryCounts {
            total: 2,
            success: 1,
            upstream_failed: 1,
            ..DiscoveryCounts::default()
        };
        let failed =
            DiscoveryCounts { total: 1, network_restricted: 1, ..DiscoveryCounts::default() };

        assert_eq!(discovery_exit_code(&report_with_counts(complete, false)), 0);
        assert_eq!(discovery_exit_code(&report_with_counts(empty, false)), 3);
        assert_eq!(discovery_exit_code(&report_with_counts(DiscoveryCounts::default(), false)), 3);
        assert_eq!(discovery_exit_code(&report_with_counts(partial, false)), 4);
        assert_eq!(discovery_exit_code(&report_with_counts(failed, false)), 5);
        assert_eq!(discovery_exit_code(&report_with_counts(DiscoveryCounts::default(), true)), 130);
    }

    #[test]
    fn single_source_discovery_keeps_all_successful_stations_in_the_legacy_list() {
        let items = ["peninsular", "east"]
            .into_iter()
            .map(|station| {
                let mut frame = FrameRef {
                    source: "my".into(),
                    product: "composite".into(),
                    station: Some(station.into()),
                    valid_time: "2026-09-24T00:00:00.000000Z".into(),
                    base_time: None,
                    logical_id: String::new(),
                    revision: None,
                    locator_version: "my-legacy-v1".into(),
                    locator: Value::Null,
                };
                frame.logical_id = logical_id(&frame).unwrap();
                DiscoveryItem {
                    target: DiscoveryTarget {
                        source: "my".into(),
                        product: Some("composite".into()),
                        station: Some(station.into()),
                    },
                    status: DiscoveryStatus::Success,
                    valid_time: Some(frame.valid_time.clone()),
                    frame: Some(frame),
                    error: None,
                }
            })
            .collect();
        let report = DiscoveryReport::from_items(
            Query { source: Some("my".into()), ..Query::default() },
            items,
            false,
        )
        .unwrap();

        let (payload, status) = single_discovery_payload(&report).unwrap();
        assert_eq!(status, 0);
        assert_eq!(payload["items"].as_array().unwrap().len(), 2);
        assert_eq!(payload["items"][0]["station"], "east");
        assert_eq!(payload["items"][1]["station"], "peninsular");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cat_cancellation_during_discovery_returns_v1_interrupted_status() {
        let started = Arc::new(Notify::new());
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        let engine = cat_test_engine(
            &config,
            cat_test_frame("http://127.0.0.1/unused.png".into()),
            Some(started.clone()),
            true,
        );
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let cancellation = async move {
            let _ = cancel_rx.await;
            Ok(())
        };
        let operation = cat_source_cancellable(
            &engine,
            &config,
            cat_test_query(),
            false,
            cancellation,
            std::future::ready(()),
        );
        tokio::pin!(operation);
        tokio::select! {
            _ = &mut operation => panic!("cat unexpectedly completed"),
            _ = started.notified() => {}
        }
        cancel_tx.send(()).unwrap();
        let outcome = operation.await.unwrap();
        assert!(matches!(&outcome, CatSourceOutcome::Interrupted));
        assert_eq!(outcome.exit_code(), 130);
        assert_eq!(
            cat_interrupted_envelope(),
            json!({
                "schema_version": 1,
                "command": "cat",
                "run_id": null,
                "query": null,
                "counts": {"items": 0},
                "items": [],
                "result": null,
                "error": {
                    "code": "cancelled",
                    "message": "operation cancelled",
                    "stage": "cat",
                    "retryable": false,
                },
                "interrupted": true,
            })
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cat_cancellation_during_raw_acquisition_cleans_staging_and_returns_no_preview() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (response_started_tx, response_started_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                assert_ne!(read, 0, "client closed before sending the request");
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 1024\r\n\r\npartial",
                )
                .await
                .unwrap();
            let _ = response_started_tx.send(());
            let _ = socket.read(&mut buffer).await;
        });

        let temp_root = unique_cat_temp_root();
        std::fs::create_dir_all(&temp_root).unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.temp_root = Some(temp_root.clone());
        let engine = cat_test_engine(
            &config,
            cat_test_frame(format!("http://{address}/frame.png")),
            None,
            false,
        );
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let cancellation = async move {
            let _ = cancel_rx.await;
            Ok(())
        };
        let operation = cat_source_cancellable(
            &engine,
            &config,
            cat_test_query(),
            false,
            cancellation,
            std::future::ready(()),
        );
        tokio::pin!(operation);
        tokio::select! {
            _ = &mut operation => panic!("cat unexpectedly completed"),
            result = response_started_rx => result.unwrap(),
        }
        cancel_tx.send(()).unwrap();
        let outcome = operation.await.unwrap();
        assert!(matches!(&outcome, CatSourceOutcome::Interrupted));
        assert_eq!(std::fs::read_dir(&temp_root).unwrap().count(), 0);
        server.await.unwrap();
        std::fs::remove_dir_all(temp_root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_after_raw_frame_acquisition_drops_its_temp_artifact() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                assert_ne!(read, 0, "client closed before sending the request");
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let png =
                include_bytes!("../../../tests/fixtures/sources/au/raw/IDR021.T.202609180511.png");
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
                png.len()
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(png).await.unwrap();
        });

        let temp_root = unique_cat_temp_root();
        std::fs::create_dir_all(&temp_root).unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.temp_root = Some(temp_root.clone());
        let engine = cat_test_engine(
            &config,
            cat_test_frame(format!("http://{address}/frame.png")),
            None,
            false,
        );
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (raw_ready_tx, raw_ready_rx) = oneshot::channel();
        let (resume_tx, resume_rx) = oneshot::channel();
        let cancellation = async move {
            let _ = cancel_rx.await;
            let _ = resume_tx.send(());
            Ok(())
        };
        let after_raw_acquisition = async move {
            let _ = raw_ready_tx.send(());
            let _ = resume_rx.await;
        };
        let operation = cat_source_cancellable(
            &engine,
            &config,
            cat_test_query(),
            false,
            cancellation,
            after_raw_acquisition,
        );
        tokio::pin!(operation);
        tokio::select! {
            _ = &mut operation => panic!("cat unexpectedly completed"),
            result = raw_ready_rx => result.unwrap(),
        }
        cancel_tx.send(()).unwrap();
        let outcome = operation.await.unwrap();
        assert!(matches!(&outcome, CatSourceOutcome::Interrupted));
        assert_eq!(outcome.exit_code(), 130);
        assert_eq!(std::fs::read_dir(&temp_root).unwrap().count(), 0);
        server.await.unwrap();
        std::fs::remove_dir_all(temp_root).unwrap();
    }
}
