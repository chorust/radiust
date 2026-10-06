use super::Context;
use crate::{emit_cli, load_config, with_progress};
use radiust_core::engine::{Engine, EngineError};
use radiust_core::error_contract::{ErrorCode, ErrorReport, ErrorStage};
use radiust_core::model::RadarField;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

const FORMATS: [&str; 4] = ["png", "netcdf", "geotiff", "zarr"];

pub(crate) struct Args {
    pub manifest: PathBuf,
    pub output: Option<PathBuf>,
    pub formats: Vec<String>,
    pub overwrite: bool,
    pub dbz: bool,
}

pub(crate) fn run(args: Args, context: &Context<'_>) -> Result<u8, String> {
    if args.formats.is_empty()
        || args.formats.iter().any(|format| !FORMATS.contains(&format.as_str()))
        || args.formats.iter().collect::<BTreeSet<_>>().len() != args.formats.len()
    {
        return Err("replay formats must be unique values from png, netcdf, geotiff, zarr".into());
    }

    let mut config = load_config(context.config_path)?;
    if let Some(output) = args.output {
        config.storage.output = output;
    }
    // Replay is an explicitly local operation, even when the user's general
    // configuration permits network access for other commands.
    config.runtime.allow_network = false;
    config.validate().map_err(|error| error.to_string())?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.runtime.decode_workers.max(1))
        .enable_all()
        .build()
        .map_err(|_| "native runtime could not be initialized".to_owned())?;
    let engine = Engine::new(config, radiust_core::source::SourceRegistry::default())
        .map_err(|_| "native replay engine could not be initialized".to_owned())?;
    let (payload, exit_code, mode_info) = runtime.block_on(with_progress(
        &engine,
        replay_cancellable(&engine, args.manifest, args.formats, args.overwrite, args.dbz),
        context.progress_enabled,
    ));
    let payload = crate::report::with_mode_info(payload, mode_info);
    emit_cli(&payload, context.json, context.quiet, context.verbose, Some("replay"))?;
    Ok(exit_code)
}

async fn replay_cancellable(
    engine: &Engine,
    manifest: PathBuf,
    formats: Vec<String>,
    overwrite: bool,
    dbz: bool,
) -> (Value, u8, Value) {
    let operation = replay(engine, manifest, formats, overwrite, dbz);
    tokio::pin!(operation);
    let cancellation = tokio::signal::ctrl_c();
    tokio::pin!(cancellation);
    tokio::select! {
        biased;
        signal = &mut cancellation => {
            if signal.is_err() {
                return error_result(&EngineError::RuntimeUnavailable, ErrorStage::Validate, dbz);
            }
            engine.cancel();
            let (mut payload, _, mode_info) = operation.await.unwrap_or_else(|error| {
                error_result(&error, ErrorStage::Decode, dbz)
            });
            payload["interrupted"] = json!(true);
            payload["error"] = json!({
                "code": "cancelled",
                "message": "operation cancelled",
                "stage": "replay",
                "retryable": false,
            });
            (payload, 130, mode_info)
        }
        result = &mut operation => result.unwrap_or_else(|error| error_result(&error, ErrorStage::Decode, dbz)),
    }
}

async fn replay(
    engine: &Engine,
    manifest: PathBuf,
    formats: Vec<String>,
    overwrite: bool,
    dbz: bool,
) -> Result<(Value, u8, Value), EngineError> {
    if dbz {
        let replayed = engine.replay_raw_manifest_mode(manifest, Some("dbz")).await?;
        let radiust_core::engine::ReplayRawResult::Dbz(result) = replayed else {
            return Err(EngineError::InvalidQuery("replay dbz returned an incompatible result"));
        };
        return replay_dbz(engine, result, formats, overwrite).await;
    }

    let raw = engine.load_raw_manifest(manifest).await?;
    let frame = raw.frame.clone();
    let field: RadarField = engine.decode_science(Arc::new(raw)).await?;
    let mode_info = crate::report::native_mode_info(
        "scientific",
        "scientific",
        &field.name,
        field.units.as_deref(),
        "known",
        if field.grid.crs.is_some()
            && (field.grid.affine.is_some()
                || (!field.grid.x.is_empty() && !field.grid.y.is_empty()))
        {
            "known"
        } else {
            "unknown"
        },
    );
    let output_root = engine.config().storage.output.clone();
    let format_count = formats.len();
    let mut items = Vec::with_capacity(formats.len());
    let mut written = 0_u64;
    let mut skipped = 0_u64;
    let mut failed = 0_u64;

    for format in formats {
        let batch = engine
            .write_science_to(
                frame.clone(),
                field.clone(),
                output_root.clone(),
                &format,
                overwrite,
                json!({}),
                None,
            )
            .await?;
        let Some(item) = batch.items.first() else {
            failed += 1;
            items.push(json!({
                "source": frame.source,
                "product": frame.product,
                "station": frame.station,
                "valid_time": frame.valid_time,
                "logical_id": frame.logical_id,
                "format": format,
                "status": "failed",
                "output_uri": null,
                "error": {"code": "internal", "message": "writer returned no result", "stage": "commit", "retryable": false},
            }));
            continue;
        };
        let status = serde_json::to_value(item.status).unwrap_or(Value::Null);
        match item.status {
            radiust_core::download::DownloadStatus::Written => written += 1,
            radiust_core::download::DownloadStatus::Skipped => skipped += 1,
            _ => failed += 1,
        }
        items.push(json!({
            "source": item.frame.source,
            "product": item.frame.product,
            "station": item.frame.station,
            "valid_time": item.frame.valid_time,
            "logical_id": item.frame.logical_id,
            "format": format,
            "status": status,
            "output_uri": item.output_uri,
            "error": item.error,
        }));
    }

    let mut payload = crate::report::envelope("replay", items, None);
    payload["query"] = json!({
        "source": frame.source,
        "product": frame.product,
        "station": frame.station,
        "valid_time": frame.valid_time,
        "logical_id": frame.logical_id,
    });
    payload["counts"] = json!({
        "items": format_count,
        "written": written,
        "skipped": skipped,
        "failed": failed,
    });
    Ok((payload, if failed == 0 { 0 } else { 5 }, mode_info))
}

async fn replay_dbz(
    engine: &Engine,
    result: radiust_core::raster::RasterResult,
    formats: Vec<String>,
    overwrite: bool,
) -> Result<(Value, u8, Value), EngineError> {
    let frame = match &result.input {
        radiust_core::raster::RasterInput::Source { frame, .. } => frame.clone(),
        _ => return Err(EngineError::InvalidFrame),
    };
    let mode_info = serde_json::to_value(&result.mode_info).unwrap_or(Value::Null);
    let output_root = engine.config().storage.output.clone();
    let format_count = formats.len();
    let result = Arc::new(result);
    let mut items = Vec::with_capacity(format_count);
    let mut written = 0_u64;
    let mut skipped = 0_u64;
    let mut failed = 0_u64;

    for format in formats {
        let extension = match format.as_str() {
            "png" => "png",
            "netcdf" => "nc",
            "geotiff" => "tif",
            "zarr" => "zarr",
            _ => unreachable!("format parser validates replay formats"),
        };
        let output_name = format!("frames/{}/reflectivity.{extension}", frame.logical_id);
        let committed = engine
            .write_raster_result_to(
                result.clone(),
                output_root.clone(),
                output_name.clone(),
                format.clone(),
                overwrite,
            )
            .await;
        match committed {
            Ok(committed) => {
                let status = match committed.status {
                    radiust_core::storage::LocalCommitStatus::Written => {
                        written += 1;
                        radiust_core::download::DownloadStatus::Written
                    }
                    radiust_core::storage::LocalCommitStatus::Skipped => {
                        skipped += 1;
                        radiust_core::download::DownloadStatus::Skipped
                    }
                };
                items.push(json!({
                    "source": frame.source,
                    "product": frame.product,
                    "station": frame.station,
                    "valid_time": frame.valid_time,
                    "logical_id": frame.logical_id,
                    "format": format,
                    "status": status,
                    "output_uri": output_root.join(output_name).display().to_string(),
                    "error": null,
                }));
            }
            Err(error) => {
                failed += 1;
                let status = if matches!(error, radiust_core::errors::CoreError::Cancelled) {
                    radiust_core::download::DownloadStatus::Cancelled
                } else {
                    radiust_core::download::DownloadStatus::Failed
                };
                let report = ErrorReport::from_core(&error, ErrorStage::Commit);
                items.push(json!({
                    "source": frame.source,
                    "product": frame.product,
                    "station": frame.station,
                    "valid_time": frame.valid_time,
                    "logical_id": frame.logical_id,
                    "format": format,
                    "status": status,
                    "output_uri": null,
                    "error": report,
                }));
            }
        }
    }

    let mut payload = crate::report::envelope("replay", items, None);
    payload["query"] = json!({
        "source": frame.source,
        "product": frame.product,
        "station": frame.station,
        "valid_time": frame.valid_time,
        "logical_id": frame.logical_id,
    });
    payload["counts"] = json!({
        "items": format_count,
        "written": written,
        "skipped": skipped,
        "failed": failed,
    });
    Ok((payload, if failed == 0 { 0 } else { 5 }, mode_info))
}

fn error_result(error: &EngineError, stage: ErrorStage, dbz: bool) -> (Value, u8, Value) {
    let report = match error {
        EngineError::Core(error) => ErrorReport::from_core(error, stage),
        EngineError::UnsupportedScience(_) | EngineError::UnsupportedSource(_) => ErrorReport {
            code: ErrorCode::Unsupported,
            message: "validated scientific decoding is unavailable for this source/product".into(),
            stage,
            retryable: false,
        },
        EngineError::InvalidFrame => ErrorReport {
            code: ErrorCode::Integrity,
            message: "raw manifest frame identity is invalid".into(),
            stage: ErrorStage::Validate,
            retryable: false,
        },
        _ => ErrorReport {
            code: ErrorCode::Internal,
            message: "offline raw manifest replay failed".into(),
            stage,
            retryable: false,
        },
    };
    let mut payload = crate::report::envelope("replay", Vec::new(), None);
    payload["error"] = serde_json::to_value(report).unwrap_or(Value::Null);
    (payload, 2, crate::report::failed_mode_info(if dbz { "dbz" } else { "scientific" }))
}
