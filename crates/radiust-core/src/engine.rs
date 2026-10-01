//! Reusable business-operation entry point used by CLI and language bindings.

use crate::config::{CoreConfig, RuntimeConfig};
use crate::discovery::{DiscoveryOutcome, run_source_fair_with_deadline};
#[cfg(feature = "extension-module")]
use crate::download::DecodedFetchStream;
use crate::download::{
    DecodedFetchBatchReport, FetchBatchReport, FetchErrorPolicy, fetch_many_raw,
};
use crate::error_contract::{ErrorReport, ErrorStage};
use crate::errors::{CoreError, ProviderError};
use crate::grid::{Resampling, regrid_regular, supports_regrid_crs};
use crate::identity::{ProcessingSpec, apply_science_versions, cache_key, logical_id, safe_ref};
use crate::limits::{Limits, RequestBudget};
use crate::model::{
    ArtifactReceipt, DiscoveryItem, DiscoveryReport, DiscoveryStatus, DiscoveryTarget, FrameRef,
    Grid, Query, RadarField, RawArtifact, RawFrame, SafeError, TimeSelector, parse_utc_time,
};
use crate::runtime::{OperationContext, OperationKind, RuntimeEventReceiver, RuntimeEvents};
use crate::source::catalog::{
    CatalogCapabilityStatus, CatalogError, CatalogMetadata, CatalogStation, SourceCatalog,
    StationCatalogUpdate,
};
use crate::source::rdcap::{is_valid_station_id, resolve_station_selection};
use crate::source::{SourceContext, SourceRegistry};
use crate::storage::{LocalCommitRequest, LocalCommitStatus, LocalStore, StagedArtifact};
use crate::transport::ftp::FtpTransport;
use crate::transport::http::{HttpRequestCoalescer, HttpTransport};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

pub struct Engine {
    config: Arc<CoreConfig>,
    catalog: SourceCatalog,
    sources: SourceRegistry,
    request_budget: Arc<RequestBudget>,
    decode_workers: Arc<Semaphore>,
    commit_workers: Arc<Semaphore>,
    http_transport: Arc<HttpTransport>,
    ftp_transport: Arc<FtpTransport>,
    runtime_events: RuntimeEvents,
    raw_cache: tokio::sync::OnceCell<Option<Arc<Mutex<crate::cache::Cache>>>>,
    raw_fetch_locks: Mutex<BTreeMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
}

impl Engine {
    pub fn new(config: CoreConfig, sources: SourceRegistry) -> Result<Self, EngineError> {
        config.validate().map_err(|_| EngineError::InvalidConfiguration)?;
        let limits = limits_from_config(&config.runtime);
        let request_budget = Arc::new(RequestBudget::new(&limits));
        let http_transport = Arc::new(
            HttpTransport::with_budget(
                limits.clone(),
                config.runtime.allow_network,
                request_budget.clone(),
            )
            .map_err(|_| EngineError::InvalidConfiguration)?,
        );
        let mut ftp_limits = limits.clone();
        ftp_limits.max_artifact_bytes =
            ftp_limits.max_artifact_bytes.min(ftp_limits.max_temp_bytes);
        let ftp_transport = Arc::new(FtpTransport::with_budget(
            ftp_limits,
            config.runtime.allow_network,
            request_budget.clone(),
        ));
        Ok(Self {
            catalog: SourceCatalog::builtin()?,
            sources: SourceRegistry::with_builtins().with_overrides(sources),
            request_budget,
            decode_workers: Arc::new(Semaphore::new(limits.decode_workers)),
            // LocalStore currently uses a non-blocking root lock. Keep every
            // commit issued by this Engine instance behind one shared permit.
            commit_workers: Arc::new(Semaphore::new(1)),
            http_transport,
            ftp_transport,
            runtime_events: RuntimeEvents::default(),
            raw_cache: tokio::sync::OnceCell::const_new(),
            raw_fetch_locks: Mutex::new(BTreeMap::new()),
            config: Arc::new(config),
        })
    }

    fn default_temp_root(&self, purpose: &str) -> PathBuf {
        if self.config.cache.enabled {
            self.config.cache.dir.join("tmp").join(purpose)
        } else {
            std::env::temp_dir().join("radiust").join(purpose)
        }
    }

    pub fn cancel(&self) {
        self.request_budget.cancel();
        self.ftp_transport.cancel();
    }

    pub fn config(&self) -> &CoreConfig {
        &self.config
    }

    /// Subscribe to bounded, sanitized lifecycle events for this Engine.
    pub fn subscribe_events(&self) -> RuntimeEventReceiver {
        self.runtime_events.subscribe()
    }

    pub(crate) fn cancellation_token(&self) -> tokio_util::sync::CancellationToken {
        self.request_budget.cancellation.clone()
    }

    pub(crate) fn resource_limits(&self) -> Limits {
        limits_from_config(&self.config.runtime)
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.request_budget.cancellation.is_cancelled()
    }

    /// Acquire one bounded CPU slot shared by scientific decoding and output
    /// encoding. Keeping both stages on the same pool prevents a batch from
    /// multiplying its configured worker budget.
    pub(crate) async fn acquire_decode_worker(&self) -> Result<OwnedSemaphorePermit, CoreError> {
        tokio::select! {
            permit = self.decode_workers.clone().acquire_owned() => {
                permit.map_err(|_| CoreError::Cancelled)
            }
            _ = self.request_budget.cancellation.cancelled() => Err(CoreError::Cancelled),
        }
    }

    /// Run a local commit off the async executor and serialize commits made
    /// through this Engine so concurrent SDK calls do not collide on its
    /// non-blocking output-root lock.
    pub(crate) async fn run_commit<T, F>(&self, commit: F) -> Result<T, CoreError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, CoreError> + Send + 'static,
    {
        let permit = tokio::select! {
            permit = self.commit_workers.clone().acquire_owned() => {
                permit.map_err(|_| CoreError::Cancelled)?
            }
            _ = self.request_budget.cancellation.cancelled() => {
                return Err(CoreError::Cancelled);
            }
        };
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            commit()
        })
        .await
        .map_err(|_| CoreError::Temporary("commit worker failed".into()))?
    }

    /// Serialize asynchronous object-store commits through the same Engine
    /// commit budget used by local manifest publication.
    pub(crate) async fn run_remote_commit<T, F, Fut>(&self, commit: F) -> Result<T, CoreError>
    where
        T: Send,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T, CoreError>> + Send,
    {
        let permit = tokio::select! {
            permit = self.commit_workers.clone().acquire_owned() => {
                permit.map_err(|_| CoreError::Cancelled)?
            }
            _ = self.request_budget.cancellation.cancelled() => {
                return Err(CoreError::Cancelled);
            }
        };
        let result = commit().await;
        drop(permit);
        result
    }

    pub async fn discover(&self, query: Query) -> Result<DiscoveryReport, EngineError> {
        let seed =
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
        self.discover_seeded(query, seed).await
    }

    /// Fetch a discovered frame's raw artifacts into bounded temporary
    /// files. Dropping the returned frame removes those files automatically.
    pub async fn fetch_raw(&self, frame: FrameRef) -> Result<RawFrame, EngineError> {
        let operation = self.runtime_events.begin(OperationKind::FetchRaw);
        let result = async {
            let _frame_permit = self.request_budget.acquire_frame().await?;
            tokio::time::timeout(
                Duration::from_secs_f64(self.config.runtime.frame_deadline),
                self.fetch_raw_inner(frame, &operation),
            )
            .await
            .map_err(|_| {
                EngineError::Core(CoreError::Transport("frame deadline exceeded".into()))
            })?
        }
        .await;
        finish_engine_operation(operation, &result, false);
        result
    }

    /// Decode a raw frame with the source-specific Rust decoder, bounded by
    /// `runtime.decode_workers` and the configured pixel/byte limits.
    pub async fn decode_science(&self, raw: Arc<RawFrame>) -> Result<RadarField, EngineError> {
        let operation = self.runtime_events.begin(OperationKind::DecodeScience);
        let result = self.decode_science_inner(raw).await;
        finish_engine_operation(operation, &result, false);
        result
    }

    /// Verify a committed raw manifest and stage its local artifacts without
    /// decoding them. The returned frame owns temporary copies and removes
    /// them when dropped.
    pub async fn load_raw_manifest(&self, manifest_path: PathBuf) -> Result<RawFrame, EngineError> {
        let temp_root = self
            .config
            .runtime
            .temp_root
            .clone()
            .unwrap_or_else(|| self.default_temp_root("replay"));
        let limits = self.resource_limits();
        let raw = tokio::task::spawn_blocking(move || {
            crate::raw_manifest::load(&manifest_path, &temp_root, &limits)
        })
        .await
        .map_err(|_| {
            EngineError::Core(crate::errors::CoreError::Temporary(
                "raw replay worker failed".into(),
            ))
        })??;
        Ok(raw)
    }

    /// Verify a committed raw manifest and replay its local artifacts through
    /// the same bounded Rust decoder used by online acquisition.
    pub async fn replay_raw_manifest(
        &self,
        manifest_path: PathBuf,
    ) -> Result<RadarField, EngineError> {
        let raw = self.load_raw_manifest(manifest_path).await?;
        self.decode_science(Arc::new(raw)).await
    }

    async fn decode_science_inner(&self, raw: Arc<RawFrame>) -> Result<RadarField, EngineError> {
        if self.is_cancelled() {
            return Err(CoreError::Cancelled.into());
        }
        if !matches!(
            (raw.frame.source.as_str(), raw.frame.product.as_str()),
            ("rainviewer", "composite") | ("tw", "grid") | ("rdcap", "reflectivity")
        ) {
            return Err(EngineError::UnsupportedScience(format!(
                "{}/{}",
                raw.frame.source, raw.frame.product
            )));
        }
        let limits = limits_from_config(&self.config.runtime);
        let permit = self.acquire_decode_worker().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            match (raw.frame.source.as_str(), raw.frame.product.as_str()) {
                ("rainviewer", "composite") => crate::science::decode_rainviewer(&raw, &limits),
                ("tw", "grid") => crate::science::decode_tw_grid(&raw, &limits),
                ("rdcap", "reflectivity") => crate::science::decode_rdcap(&raw, &limits),
                _ => unreachable!("source/product was checked before dispatch"),
            }
        })
        .await
        .map_err(|_| EngineError::Core(CoreError::Transport("science worker failed".into())))?
        .map_err(EngineError::Core)
    }

    /// Regrid a regular scientific field using a bounded CPU worker.
    pub async fn regrid(
        &self,
        field: RadarField,
        target: Grid,
        method: Resampling,
    ) -> Result<RadarField, EngineError> {
        let operation = self.runtime_events.begin(OperationKind::Regrid);
        let result = self.regrid_inner(field, target, method).await;
        finish_engine_operation(operation, &result, false);
        result
    }

    async fn regrid_inner(
        &self,
        field: RadarField,
        target: Grid,
        method: Resampling,
    ) -> Result<RadarField, EngineError> {
        if self.is_cancelled() {
            return Err(CoreError::Cancelled.into());
        }
        if !supports_regrid_crs(field.grid.crs.as_deref(), target.crs.as_deref()) {
            return Err(EngineError::UnsupportedRegrid {
                source_crs: field.grid.crs.unwrap_or_else(|| "unknown".into()),
                target_crs: target.crs.unwrap_or_else(|| "unknown".into()),
            });
        }
        let limits = limits_from_config(&self.config.runtime);
        let permit = tokio::select! {
            permit = self.decode_workers.clone().acquire_owned() => {
                permit.map_err(|_| EngineError::Core(CoreError::Cancelled))?
            }
            _ = self.request_budget.cancellation.cancelled() => {
                return Err(CoreError::Cancelled.into());
            }
        };
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            regrid_regular(&field, target, method, &limits)
        })
        .await
        .map_err(|_| EngineError::Core(CoreError::Transport("regrid worker failed".into())))?
        .map_err(EngineError::Core)
    }

    /// Fetch several raw frames with bounded concurrency and ordered results.
    /// In stop mode, active futures are dropped and queued frames are marked
    /// not-started after the first failed acquisition.
    pub async fn fetch_many_raw(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
    ) -> FetchBatchReport {
        fetch_many_raw(self, frames, self.config.runtime.frame_concurrency, policy, dry_run).await
    }

    pub async fn fetch_many_raw_with_concurrency(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        concurrency: usize,
    ) -> FetchBatchReport {
        fetch_many_raw(self, frames, concurrency, policy, dry_run).await
    }

    /// Acquire and scientifically decode an ordered batch under the Engine's
    /// shared request and CPU worker limits.
    pub async fn fetch_many_decoded(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
    ) -> DecodedFetchBatchReport {
        self.fetch_many_decoded_with_concurrency(
            frames,
            policy,
            dry_run,
            self.config.runtime.frame_concurrency,
        )
        .await
    }

    pub async fn fetch_many_decoded_with_concurrency(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        concurrency: usize,
    ) -> DecodedFetchBatchReport {
        crate::download::fetch_many_decoded(self, frames, concurrency, policy, dry_run).await
    }

    /// Create a completion-ordered decoded stream with bounded prefetch.
    #[cfg(feature = "extension-module")]
    pub fn fetch_decoded_stream(
        self: &Arc<Self>,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        concurrency: usize,
    ) -> DecodedFetchStream {
        DecodedFetchStream::new(self.clone(), frames, concurrency, policy)
    }

    /// Acquire, commit, and report raw-only outputs through the local v1
    /// manifest store. Decoded formats remain the responsibility of a
    /// validated output encoder.
    pub async fn download_raw_only(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
    ) -> crate::download::DownloadBatchReport {
        self.download_raw_only_to(
            frames,
            policy,
            dry_run,
            overwrite,
            self.config.storage.output.clone(),
        )
        .await
    }

    pub async fn download_raw_only_to(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
        output_root: PathBuf,
    ) -> crate::download::DownloadBatchReport {
        let operation = self.runtime_events.begin(OperationKind::DownloadRawOnly);
        let report = crate::download::download_raw_only_to(
            self,
            frames,
            policy,
            dry_run,
            overwrite,
            output_root,
        )
        .await;
        finish_download_operation(operation, &report);
        report
    }

    /// Decode verified scientific frames, render PNG plus sidecar, and commit
    /// each result through the local manifest-last store.
    pub async fn download_png(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
    ) -> crate::download::DownloadBatchReport {
        self.download_png_to(frames, policy, dry_run, overwrite, self.config.storage.output.clone())
            .await
    }

    pub async fn download_png_to(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
        output_root: PathBuf,
    ) -> crate::download::DownloadBatchReport {
        let operation = self.runtime_events.begin(OperationKind::DownloadPng);
        let report =
            crate::download::download_png_to(self, frames, policy, dry_run, overwrite, output_root)
                .await;
        finish_download_operation(operation, &report);
        report
    }

    /// Decode verified scientific frames, write CF-1.8 NetCDF4, and commit
    /// each result through the local manifest-last store.
    pub async fn download_netcdf(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
    ) -> crate::download::DownloadBatchReport {
        self.download_netcdf_to(
            frames,
            policy,
            dry_run,
            overwrite,
            self.config.storage.output.clone(),
        )
        .await
    }

    pub async fn download_netcdf_to(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
        output_root: PathBuf,
    ) -> crate::download::DownloadBatchReport {
        let operation = self.runtime_events.begin(OperationKind::DownloadNetcdf);
        let report = crate::download::download_netcdf_to(
            self,
            frames,
            policy,
            dry_run,
            overwrite,
            output_root,
        )
        .await;
        finish_download_operation(operation, &report);
        report
    }

    /// Decode verified scientific frames, write consolidated Zarr v2 stores,
    /// and commit their files through the local manifest-last store.
    pub async fn download_zarr(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
    ) -> crate::download::DownloadBatchReport {
        self.download_zarr_to(
            frames,
            policy,
            dry_run,
            overwrite,
            self.config.storage.output.clone(),
        )
        .await
    }

    pub async fn download_zarr_to(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
        output_root: PathBuf,
    ) -> crate::download::DownloadBatchReport {
        let operation = self.runtime_events.begin(OperationKind::DownloadZarr);
        let report = crate::download::download_zarr_to(
            self,
            frames,
            policy,
            dry_run,
            overwrite,
            output_root,
        )
        .await;
        finish_download_operation(operation, &report);
        report
    }

    /// Decode verified scientific frames, write GeoTIFF data/quality/provenance
    /// groups, and commit them through the local manifest-last store.
    pub async fn download_geotiff(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
    ) -> crate::download::DownloadBatchReport {
        self.download_geotiff_to(
            frames,
            policy,
            dry_run,
            overwrite,
            self.config.storage.output.clone(),
        )
        .await
    }

    pub async fn download_geotiff_to(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
        output_root: PathBuf,
    ) -> crate::download::DownloadBatchReport {
        let operation = self.runtime_events.begin(OperationKind::DownloadGeotiff);
        let report = crate::download::download_geotiff_to(
            self,
            frames,
            policy,
            dry_run,
            overwrite,
            output_root,
        )
        .await;
        finish_download_operation(operation, &report);
        report
    }

    pub async fn download_decoded_to_with_template(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
        output_root: PathBuf,
        format: &str,
        output_template: Option<String>,
    ) -> Result<crate::download::DownloadBatchReport, EngineError> {
        self.download_decoded_to_with_template_and_raw(
            frames,
            policy,
            dry_run,
            overwrite,
            output_root,
            format,
            output_template,
            false,
        )
        .await
    }

    pub async fn download_decoded_to_with_template_and_raw(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
        output_root: PathBuf,
        format: &str,
        output_template: Option<String>,
        include_raw: bool,
    ) -> Result<crate::download::DownloadBatchReport, EngineError> {
        self.download_decoded_to_with_processing_and_raw(
            frames,
            policy,
            dry_run,
            overwrite,
            output_root,
            format,
            output_template,
            crate::download::DecodedProcessing::default(),
            include_raw,
        )
        .await
    }

    pub async fn download_decoded_to_with_processing(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
        output_root: PathBuf,
        format: &str,
        output_template: Option<String>,
        processing: crate::download::DecodedProcessing,
    ) -> Result<crate::download::DownloadBatchReport, EngineError> {
        self.download_decoded_to_with_processing_and_raw(
            frames,
            policy,
            dry_run,
            overwrite,
            output_root,
            format,
            output_template,
            processing,
            false,
        )
        .await
    }

    pub async fn download_decoded_to_with_processing_and_raw(
        &self,
        frames: Vec<FrameRef>,
        policy: FetchErrorPolicy,
        dry_run: bool,
        overwrite: bool,
        output_root: PathBuf,
        format: &str,
        output_template: Option<String>,
        processing: crate::download::DecodedProcessing,
        include_raw: bool,
    ) -> Result<crate::download::DownloadBatchReport, EngineError> {
        let (kind, format) = match format {
            "png" => (OperationKind::DownloadPng, crate::download::DecodedOutputFormat::Png),
            "netcdf" => {
                (OperationKind::DownloadNetcdf, crate::download::DecodedOutputFormat::Netcdf)
            }
            "geotiff" => {
                (OperationKind::DownloadGeotiff, crate::download::DecodedOutputFormat::Geotiff)
            }
            "zarr" => (OperationKind::DownloadZarr, crate::download::DecodedOutputFormat::Zarr),
            _ => return Err(EngineError::InvalidConfiguration),
        };
        let operation = self.runtime_events.begin(kind);
        let report = crate::download::download_decoded_to_with_processing_and_raw(
            self,
            frames,
            policy,
            dry_run,
            overwrite,
            output_root,
            format,
            output_template,
            processing,
            include_raw,
        )
        .await;
        finish_download_operation(operation, &report);
        Ok(report)
    }

    /// Encode an in-memory field and publish it through the local manifest-last
    /// store. The caller supplies the frame reference so persisted output keeps
    /// the acquisition identity even when no raw artifacts are available.
    pub async fn write_science_to(
        &self,
        frame: FrameRef,
        field: RadarField,
        output_root: PathBuf,
        format: &str,
        overwrite: bool,
        options: Value,
        variable: Option<String>,
    ) -> Result<crate::download::DownloadBatchReport, EngineError> {
        frame.validate_identity().map_err(|_| EngineError::InvalidFrame)?;
        field.validate()?;
        let frame_time =
            parse_utc_time(&frame.valid_time).map_err(|_| EngineError::InvalidFrame)?;
        let field_time =
            parse_utc_time(&field.valid_time).map_err(|_| EngineError::InvalidFrame)?;
        if frame_time != field_time {
            return Err(EngineError::InvalidQuery("frame and field valid_time differ"));
        }
        if variable.as_deref().is_some_and(|name| name != field.name) {
            return Err(EngineError::InvalidConfiguration);
        }
        let kind = match format {
            "png" => OperationKind::DownloadPng,
            "netcdf" => OperationKind::DownloadNetcdf,
            "geotiff" => OperationKind::DownloadGeotiff,
            "zarr" => OperationKind::DownloadZarr,
            _ => return Err(EngineError::InvalidConfiguration),
        };
        let limits = self.resource_limits();
        let pixels = field
            .shape
            .iter()
            .try_fold(1_u64, |total, size| total.checked_mul(*size as u64))
            .ok_or_else(|| CoreError::ResourceLimit("field dimensions overflow".into()))?;
        limits.validate_pixels(pixels)?;

        let operation = self.runtime_events.begin(kind);
        let result = self
            .write_science_inner(
                frame.clone(),
                field,
                output_root,
                format,
                overwrite,
                options,
                variable,
            )
            .await;
        let (status, output_uri, error) = match result {
            Ok((LocalCommitStatus::Written, output_uri)) => {
                (crate::download::DownloadStatus::Written, Some(output_uri), None)
            }
            Ok((LocalCommitStatus::Skipped, output_uri)) => {
                (crate::download::DownloadStatus::Skipped, Some(output_uri), None)
            }
            Err((error, _stage)) if matches!(error, CoreError::Cancelled) => (
                crate::download::DownloadStatus::Cancelled,
                None,
                Some(ErrorReport::from_core(&error, ErrorStage::Commit)),
            ),
            Err((error, stage)) => (
                crate::download::DownloadStatus::Failed,
                None,
                Some(ErrorReport::from_core(&error, stage)),
            ),
        };
        let mut report = crate::download::DownloadBatchReport {
            items: vec![crate::download::DownloadItem {
                input_index: 0,
                frame,
                status,
                output_uri,
                error,
            }],
            written: if status == crate::download::DownloadStatus::Written { 1 } else { 0 },
            skipped: if status == crate::download::DownloadStatus::Skipped { 1 } else { 0 },
            failed: if status == crate::download::DownloadStatus::Failed { 1 } else { 0 },
            cancelled: if status == crate::download::DownloadStatus::Cancelled { 1 } else { 0 },
            ..crate::download::DownloadBatchReport::default()
        };
        report.interrupted = status == crate::download::DownloadStatus::Cancelled;
        finish_download_operation(operation, &report);
        Ok(report)
    }

    async fn write_science_inner(
        &self,
        frame: FrameRef,
        field: RadarField,
        output_root: PathBuf,
        format: &str,
        overwrite: bool,
        options: Value,
        variable: Option<String>,
    ) -> Result<(LocalCommitStatus, String), (CoreError, ErrorStage)> {
        let revision =
            science_revision(&frame, &field).map_err(|error| (error, ErrorStage::Stage))?;
        let mut spec = ProcessingSpec {
            format: format.to_owned(),
            variable,
            options,
            ..ProcessingSpec::default()
        };
        apply_science_versions(&mut spec, &field);
        let limits = self.resource_limits();
        let permit = tokio::select! {
            permit = self.decode_workers.clone().acquire_owned() => {
                permit.map_err(|_| (CoreError::Cancelled, ErrorStage::Stage))?
            }
            _ = self.request_budget.cancellation.cancelled() => {
                return Err((CoreError::Cancelled, ErrorStage::Stage));
            }
        };
        let format = format.to_owned();
        let prepared = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            prepare_science_output(
                output_root,
                frame,
                field,
                revision,
                spec,
                &format,
                overwrite,
                limits,
            )
        })
        .await
        .map_err(|_| {
            (CoreError::Temporary("science output worker failed".into()), ErrorStage::Stage)
        })?
        .map_err(|error| (error, ErrorStage::Stage))?;

        let cancellation = self.cancellation_token();
        self.run_commit(move || {
            let PreparedScienceOutput { _workspace, store, request, output_uri } = prepared;
            let result = store.commit_cancellable(request, &cancellation)?;
            drop(_workspace);
            Ok((result.status, output_uri))
        })
        .await
        .map_err(|error| (error, ErrorStage::Commit))
    }

    async fn fetch_raw_inner(
        &self,
        frame: FrameRef,
        operation: &OperationContext,
    ) -> Result<RawFrame, EngineError> {
        if self.is_cancelled() {
            return Err(CoreError::Cancelled.into());
        }
        if self.catalog.source(&frame.source).is_none() {
            return Err(EngineError::InvalidFrame);
        }
        if self.catalog.source(&frame.source).is_some_and(|source| source.availability == "retired")
        {
            return Err(EngineError::InvalidFrame);
        }
        let adapter = self
            .sources
            .get(&frame.source)
            .ok_or_else(|| EngineError::UnsupportedSource(frame.source.clone()))?;
        let identity_validation = frame.validate_identity();
        if !self.config.runtime.allow_network && identity_validation.is_err() {
            return Err(EngineError::Core(CoreError::NetworkDisabled(
                "raw acquisition requires network access".into(),
            )));
        }
        identity_validation.map_err(|_| EngineError::InvalidFrame)?;

        let _raw_fetch_guard = if frame.source == "rdcap" {
            let lock = self.raw_fetch_lock(&frame.logical_id);
            Some(tokio::select! {
                guard = lock.lock_owned() => guard,
                _ = self.request_budget.cancellation.cancelled() => {
                    return Err(CoreError::Cancelled.into());
                }
            })
        } else {
            None
        };

        let temp_root =
            self.config.runtime.temp_root.clone().unwrap_or_else(|| self.default_temp_root("raw"));
        let fetch_query = Query {
            source: Some(frame.source.clone()),
            product: Some(frame.product.clone()),
            stations: frame.station.clone().into_iter().collect(),
            selector: TimeSelector::At { time: frame.valid_time.clone() },
            base_time: frame.base_time.clone(),
            ..Query::default()
        };
        let source_fetch = adapter.clone().fetch_raw(
            frame.clone(),
            self.source_context(
                fetch_query,
                &frame.source,
                Arc::new(HttpRequestCoalescer::default()),
            ),
            temp_root.clone(),
        );

        if raw_frame_cache_key(&frame).is_some() {
            if let Some(cache) = self.raw_cache().await {
                if let Some(raw) =
                    self.load_cached_raw(cache, frame.clone(), temp_root.clone()).await?
                {
                    operation
                        .progress(raw.artifacts.len() as u64, Some(raw.artifacts.len() as u64));
                    return Ok(raw);
                }
            }
        }

        if !self.config.runtime.allow_network {
            return Err(EngineError::Core(CoreError::NetworkDisabled(
                "raw acquisition requires network access".into(),
            )));
        }

        if let Some(fetch) = source_fetch {
            let raw = fetch.await.map_err(|error| sanitize_fetch_error(error, &frame.source))?;
            if raw.frame.logical_id != frame.logical_id {
                return Err(EngineError::InvalidFrame);
            }
            if raw.frame.source == "rdcap" {
                crate::source::rdcap::validate_binding(&raw.frame, &raw.artifacts)?;
            }
            operation.progress(1, Some(1));
            self.store_cached_raw(&raw).await;
            return Ok(raw);
        }

        let requests = artifact_requests(&frame)?;
        let request_count = requests.len() as u64;
        let headers = artifact_headers(&frame)?;
        let header_refs =
            headers.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect::<Vec<_>>();
        let mut artifacts = Vec::with_capacity(requests.len());
        let mut total_bytes = 0_u64;
        for (index, request) in requests.into_iter().enumerate() {
            let artifact_url =
                url::Url::parse(&request.url).map_err(|_| EngineError::InvalidFrame)?;
            if !adapter.allows_artifact_url(&frame, &artifact_url) {
                return Err(EngineError::UnsupportedSource(frame.source.clone()));
            }
            let remaining_frame_bytes =
                self.config.runtime.max_frame_bytes.saturating_sub(total_bytes);
            let destination = temp_root.join(format!("{}.{}.bin", uuid::Uuid::new_v4(), index));
            let receipt = self
                .http_transport
                .get_to_path_same_origin_limited_with_headers(
                    &request.url,
                    &header_refs,
                    &destination,
                    remaining_frame_bytes,
                )
                .await
                .map_err(|error| sanitize_fetch_error(error, &frame.source))?;
            total_bytes = total_bytes.saturating_add(receipt.size_bytes);
            if total_bytes > self.config.runtime.max_frame_bytes {
                let _ = std::fs::remove_file(&destination);
                return Err(EngineError::Core(CoreError::ResourceLimit(
                    "frame artifacts exceed configured limit".into(),
                )));
            }
            let path = tempfile::TempPath::try_from_path(destination).map_err(|_| {
                EngineError::Core(CoreError::Temporary(
                    "downloaded artifact could not be retained".into(),
                ))
            })?;
            artifacts.push(RawArtifact {
                receipt: ArtifactReceipt {
                    name: request.name,
                    media_type: request.media_type,
                    size_bytes: receipt.size_bytes,
                    sha256: receipt.sha256,
                },
                path,
            });
            operation.progress((index + 1) as u64, Some(request_count));
        }
        let raw = RawFrame { frame, artifacts, private_locator: None };
        self.store_cached_raw(&raw).await;
        Ok(raw)
    }

    async fn raw_cache(&self) -> Option<Arc<Mutex<crate::cache::Cache>>> {
        if !self.config.cache.enabled {
            return None;
        }
        self.raw_cache
            .get_or_init(|| async {
                let root = self.config.cache.dir.clone();
                tokio::task::spawn_blocking(move || {
                    crate::cache::Cache::open(root).ok().map(|cache| Arc::new(Mutex::new(cache)))
                })
                .await
                .unwrap_or(None)
            })
            .await
            .clone()
    }

    fn raw_fetch_lock(&self, logical_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks =
            self.raw_fetch_locks.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(logical_id).and_then(std::sync::Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(logical_id.to_owned(), Arc::downgrade(&lock));
        lock
    }

    async fn load_cached_raw(
        &self,
        cache: Arc<Mutex<crate::cache::Cache>>,
        mut frame: FrameRef,
        temp_root: PathBuf,
    ) -> Result<Option<RawFrame>, CoreError> {
        let Some(frame_key) = raw_frame_cache_key(&frame) else {
            return Ok(None);
        };
        let manifest_key = format!("raw:{frame_key}:manifest");
        let limits = self.resource_limits();
        let loaded = tokio::task::spawn_blocking(move || {
            let cache = cache.lock().ok()?;
            let entry = cache.index.get_entry(&manifest_key).ok()??;
            if entry.size_bytes > 1024 * 1024 {
                let _ = cache.index.remove(&manifest_key);
                return None;
            }
            let bytes = cache.get_bytes(&manifest_key).ok()??;
            let document: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            if document.get("schema_version")?.as_u64()? != 1
                || document.get("logical_id")?.as_str()? != frame.logical_id
            {
                let _ = cache.index.remove(&manifest_key);
                return None;
            }
            frame.revision =
                document.get("revision").and_then(serde_json::Value::as_str).map(str::to_owned);
            if frame.source == "rdcap" {
                frame.locator = crate::identity::safe_locator(&frame.locator);
            }
            let descriptors = document.get("artifacts")?.as_array()?;
            if descriptors.is_empty() {
                let _ = cache.index.remove(&manifest_key);
                return None;
            }
            // Cache-hit acquisition may be the first operation in a fresh
            // cache root, so create the operation stage directory before
            // get_to_path attempts its exclusive file create.
            std::fs::create_dir_all(&temp_root).ok()?;

            let mut artifacts = Vec::with_capacity(descriptors.len());
            let mut total_bytes = 0_u64;
            for (index, descriptor) in descriptors.iter().enumerate() {
                let name = descriptor.get("name")?.as_str()?;
                if !valid_cached_artifact_name(name) {
                    return None;
                }
                let media_type = descriptor.get("media_type")?.as_str()?.to_owned();
                let size_bytes = descriptor.get("size_bytes")?.as_u64()?;
                let sha256 = descriptor.get("sha256")?.as_str()?.to_owned();
                if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return None;
                }
                if size_bytes > limits.max_artifact_bytes {
                    return Some(Err(CoreError::ResourceLimit(format!(
                        "cached artifact {size_bytes} > {}",
                        limits.max_artifact_bytes
                    ))));
                }
                let Some(frame_total) = total_bytes.checked_add(size_bytes) else {
                    return Some(Err(CoreError::ResourceLimit(
                        "cached frame size overflow".into(),
                    )));
                };
                if frame_total > limits.max_frame_bytes {
                    return Some(Err(CoreError::ResourceLimit(format!(
                        "cached frame {frame_total} > {}",
                        limits.max_frame_bytes
                    ))));
                }
                if frame_total > limits.max_temp_bytes {
                    return Some(Err(CoreError::ResourceLimit(format!(
                        "cached temporary data {frame_total} > {}",
                        limits.max_temp_bytes
                    ))));
                }
                let remaining_frame = limits.max_frame_bytes - total_bytes;
                let remaining_temp = limits.max_temp_bytes - total_bytes;
                let artifact_limit =
                    remaining_frame.min(remaining_temp).min(limits.max_artifact_bytes);
                let artifact_key = format!("raw:{frame_key}:artifact:{name}");
                let destination =
                    temp_root.join(format!("{}.cache-{index}.bin", uuid::Uuid::new_v4()));
                let Some((copied_size, copied_sha256)) =
                    cache.get_to_path(&artifact_key, &destination, artifact_limit).ok()?
                else {
                    return None;
                };
                if copied_size != size_bytes || copied_sha256 != sha256 {
                    let _ = std::fs::remove_file(&destination);
                    return None;
                }
                let path = match tempfile::TempPath::try_from_path(destination.clone()) {
                    Ok(path) => path,
                    Err(_) => {
                        let _ = std::fs::remove_file(&destination);
                        return None;
                    }
                };
                total_bytes = total_bytes.saturating_add(copied_size);
                artifacts.push(RawArtifact {
                    receipt: ArtifactReceipt {
                        name: name.to_owned(),
                        media_type,
                        size_bytes,
                        sha256,
                    },
                    path,
                });
            }
            let raw = RawFrame { frame, artifacts, private_locator: None };
            if raw.frame.source == "rdcap"
                && crate::source::rdcap::validate_binding(&raw.frame, &raw.artifacts).is_err()
            {
                return None;
            }
            Some(Ok(raw))
        })
        .await
        .map_err(|_| CoreError::Temporary("cached artifact worker failed".into()))?;
        match loaded {
            Some(Ok(raw)) => Ok(Some(raw)),
            Some(Err(error)) => Err(error),
            None => Ok(None),
        }
    }

    async fn store_cached_raw(&self, raw: &RawFrame) {
        let Some(frame_key) = raw_frame_cache_key(&raw.frame) else { return };
        let mut names = std::collections::HashSet::with_capacity(raw.artifacts.len());
        if raw.artifacts.iter().any(|artifact| !names.insert(artifact.receipt.name.as_str())) {
            return;
        }
        let Some(cache) = self.raw_cache().await else { return };
        let artifacts = raw
            .artifacts
            .iter()
            .map(|artifact| {
                (
                    format!("raw:{frame_key}:artifact:{}", artifact.receipt.name),
                    artifact.path.to_path_buf(),
                    artifact.receipt.clone(),
                )
            })
            .collect::<Vec<_>>();
        if artifacts.is_empty() {
            return;
        }
        let frame = raw.frame.clone();
        let max_artifact_bytes = self.config.runtime.max_artifact_bytes;
        let max_cache_bytes = self.config.cache.max_bytes;
        let max_age_days = self.config.cache.max_age_days;
        let _ = tokio::task::spawn_blocking(move || {
            let Ok(cache) = cache.lock() else { return };
            let now =
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
            let expires_at = now.saturating_add((max_age_days.saturating_mul(86_400)) as i64);
            let mut descriptors = Vec::with_capacity(artifacts.len());
            for (key, path, receipt) in artifacts {
                let Ok((size_bytes, sha256, _)) =
                    cache.put_file(&key, path, "object", Some(expires_at), max_artifact_bytes)
                else {
                    return;
                };
                if size_bytes != receipt.size_bytes || sha256 != receipt.sha256 {
                    return;
                }
                descriptors.push(serde_json::json!({
                    "name": receipt.name,
                    "media_type": receipt.media_type,
                    "size_bytes": size_bytes,
                    "sha256": sha256,
                }));
            }
            let Ok(safe_ref) = safe_ref(&frame) else { return };
            let manifest = serde_json::json!({
                "schema_version": 1,
                "logical_id": frame.logical_id,
                "revision": frame.revision,
                "ref": safe_ref,
                "artifacts": descriptors,
            });
            let Ok(bytes) = serde_json::to_vec(&manifest) else { return };
            if bytes.len() > 1024 * 1024 {
                return;
            }
            let manifest_key = format!("raw:{frame_key}:manifest");
            if cache.put_bytes(&manifest_key, &bytes, "object", Some(expires_at)).is_ok() {
                let _ = cache.gc(max_cache_bytes);
            }
        })
        .await;
    }

    /// Seeded form keeps replay fixtures deterministic while production calls
    /// shuffle target dispatch on each run.
    pub async fn discover_seeded(
        &self,
        query: Query,
        seed: u64,
    ) -> Result<DiscoveryReport, EngineError> {
        let operation = self.runtime_events.begin(OperationKind::Discover);
        let result = self.discover_seeded_inner(query, seed, &operation).await;
        let interrupted = matches!(&result, Ok(report) if report.interrupted);
        finish_engine_operation(operation, &result, interrupted);
        result
    }

    async fn discover_seeded_inner(
        &self,
        query: Query,
        seed: u64,
        operation: &OperationContext,
    ) -> Result<DiscoveryReport, EngineError> {
        let multi_source = query.source.as_deref() == Some("all") || !query.sources.is_empty();
        query.validate(multi_source)?;
        if query.source.is_some() && !query.sources.is_empty() {
            return Err(EngineError::InvalidQuery("source and sources cannot be combined"));
        }
        let source_ids = if !query.sources.is_empty() {
            Some(query.sources.clone())
        } else {
            match query.source.as_deref() {
                Some("all") => None,
                Some(source) => Some(vec![source.to_owned()]),
                None => return Err(EngineError::InvalidQuery("a source must be selected")),
            }
        };
        let discovery_deadline = Duration::from_secs_f64(self.config.runtime.discovery_deadline);
        let deadline_at = Instant::now() + discovery_deadline;
        let request_coalescer = Arc::new(HttpRequestCoalescer::default());
        let mut catalog = self.catalog.clone();
        let rdcap_selected =
            source_ids.as_ref().is_none_or(|ids| ids.iter().any(|source| source == "rdcap"));
        let mut catalog_refresh_error = None;
        let mut catalog_refreshed = false;

        // Refresh once before expansion/filtering. The refresh uses the same
        // Engine request budget and its elapsed time is deducted from the
        // discovery deadline below.
        if rdcap_selected {
            let refresh_result = if !self.config.runtime.allow_network {
                Err(CoreError::NetworkDisabled(
                    "RDCAP catalog refresh requires network access".into(),
                ))
            } else if let Some(adapter) = self.sources.get("rdcap") {
                let remaining = deadline_at.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    Err(CoreError::Provider(ProviderError::Timeout))
                } else {
                    let context =
                        self.source_context(query.clone(), "rdcap", request_coalescer.clone());
                    match tokio::time::timeout(remaining, adapter.refresh_station_catalog(context))
                        .await
                    {
                        Ok(result) => result,
                        Err(_) => Err(CoreError::Provider(ProviderError::Timeout)),
                    }
                }
            } else {
                Err(CoreError::Provider(ProviderError::CatalogUnavailable))
            };

            match refresh_result {
                Ok(Some(update)) => {
                    merge_station_catalog_update(&mut catalog, update)?;
                    catalog_refreshed = true;
                }
                Ok(None) => {
                    catalog_refresh_error =
                        Some(CoreError::Provider(ProviderError::CatalogUnavailable));
                }
                Err(error) => catalog_refresh_error = Some(error),
            }
        }

        let mut dispatch_query = query.clone();
        let mut preflight_items = Vec::new();
        if query.source.as_deref() == Some("rdcap") && !query.stations.is_empty() {
            let station_ids = catalog
                .source("rdcap")
                .ok_or(EngineError::InvalidQuery("RDCAP catalog is unavailable"))?
                .stations
                .iter()
                .map(|station| station.id.clone())
                .collect::<Vec<_>>();
            let mut normalized = Vec::with_capacity(query.stations.len());
            let mut seen = HashSet::new();
            let product = query
                .product
                .clone()
                .or_else(|| catalog.default_product("rdcap").map(str::to_owned));
            for selector in &query.stations {
                match resolve_station_selection(selector, &station_ids) {
                    Ok(station_id) => {
                        if !seen.insert(station_id.clone()) {
                            return Err(EngineError::InvalidQuery(
                                "station list contains duplicates after RDCAP normalization",
                            ));
                        }
                        normalized.push(station_id);
                    }
                    Err(provider_error) => {
                        let error = if provider_error == ProviderError::AmbiguousIndex {
                            CoreError::Provider(provider_error)
                        } else if catalog_refreshed {
                            CoreError::Provider(ProviderError::UnknownStation)
                        } else {
                            match catalog_refresh_error.as_ref() {
                                Some(CoreError::Cancelled) => CoreError::Cancelled,
                                Some(CoreError::Provider(ProviderError::Timeout)) => {
                                    CoreError::Provider(ProviderError::Timeout)
                                }
                                _ => CoreError::Provider(ProviderError::CatalogUnavailable),
                            }
                        };
                        preflight_items.push(core_error_item(
                            DiscoveryTarget {
                                source: "rdcap".into(),
                                product: product.clone(),
                                station: Some(selector.clone()),
                            },
                            error,
                        ));
                    }
                }
            }
            dispatch_query.stations = normalized;
        }

        let mut targets = catalog.expand_targets(source_ids.as_deref())?;
        if query.product.is_none()
            && let Some([source]) = source_ids.as_deref()
            && let Some(default_product) = catalog.default_product(source)
        {
            targets.retain(|target| target.product.as_deref() == Some(default_product));
        }
        if let Some(product) = query.product.as_deref() {
            targets.retain(|target| target.product.as_deref() == Some(product));
        }
        if !dispatch_query.stations.is_empty() {
            let mut expanded = Vec::new();
            for target in targets {
                if target.station.is_some() {
                    if dispatch_query
                        .stations
                        .iter()
                        .any(|station| Some(station.as_str()) == target.station.as_deref())
                    {
                        expanded.push(target);
                    }
                } else {
                    expanded.extend(dispatch_query.stations.iter().map(|station| {
                        DiscoveryTarget { station: Some(station.clone()), ..target.clone() }
                    }));
                }
            }
            targets = expanded;
            targets.sort();
            targets.dedup();
        } else if !query.stations.is_empty() {
            // Every explicit station selector failed preflight. Do not let an
            // empty normalized list turn into an unfiltered 48-station scan.
            targets.retain(|target| target.source != "rdcap");
        }

        let target_count = targets.len() as u64 + preflight_items.len() as u64;
        let mut completed_targets = preflight_items.len() as u64;
        let mut items = preflight_items;
        let mut runnable = Vec::new();
        if completed_targets > 0 {
            operation.progress(completed_targets, Some(target_count));
        }
        for target in targets {
            let runnable_before = runnable.len();
            if catalog.source(&target.source).is_some_and(|source| source.availability == "retired")
            {
                items.push(status_item(
                    target,
                    DiscoveryStatus::Retired,
                    "retired",
                    "source retired",
                ));
            } else if let Some(field) = required_credential(&target.source)
                .filter(|field| !self.has_credential(&target.source, field))
            {
                let message = format!("source requires sources.{}.{}", target.source, field);
                items.push(status_item(
                    target,
                    DiscoveryStatus::MissingCredentials,
                    "missing_credentials",
                    &message,
                ));
            } else if !self.config.runtime.allow_network {
                items.push(status_item(
                    target,
                    DiscoveryStatus::NetworkRestricted,
                    "network_restricted",
                    "public network access is disabled",
                ));
            } else if let Some(adapter) = self.sources.get(&target.source) {
                runnable.push((target, adapter));
            } else {
                items.push(status_item(
                    target,
                    DiscoveryStatus::UpstreamFailed,
                    "unsupported",
                    "native source adapter is not available",
                ));
            }
            if runnable.len() == runnable_before {
                completed_targets += 1;
                operation.progress(completed_targets, Some(target_count));
            }
        }

        let query_for_adapters = dispatch_query.clone();
        let adapters = runnable
            .into_iter()
            .map(|(target, adapter)| (target, adapter))
            .collect::<std::collections::BTreeMap<_, _>>();
        let runnable_targets = adapters.keys().cloned().collect::<Vec<_>>();
        let remaining = deadline_at.saturating_duration_since(Instant::now());
        let dispatched: Vec<(DiscoveryTarget, DiscoveryOutcome<Vec<FrameRef>, CoreError>)> = if self
            .request_budget
            .cancellation
            .is_cancelled()
        {
            runnable_targets
                .into_iter()
                .map(|target| (target, DiscoveryOutcome::Cancelled))
                .collect()
        } else if remaining.is_zero() {
            runnable_targets.into_iter().map(|target| (target, DiscoveryOutcome::Timeout)).collect()
        } else {
            run_source_fair_with_deadline(
                runnable_targets,
                self.config.runtime.discovery_workers,
                seed,
                self.request_budget.cancellation.clone(),
                Some(remaining),
                |target| target.source.clone(),
                {
                    let adapters = &adapters;
                    move |target| {
                        let adapter = adapters[&target].clone();
                        let context = self.source_context(
                            query_for_adapters.clone(),
                            &target.source,
                            request_coalescer.clone(),
                        );
                        async move { adapter.discover(target, context).await }
                    }
                },
            )
            .await
        };

        for (target, outcome) in dispatched {
            match outcome {
                DiscoveryOutcome::Completed(Ok(frames)) => {
                    items.extend(select_target_frames(
                        &dispatch_query,
                        target,
                        frames,
                        multi_source,
                        Utc::now(),
                    ));
                }
                DiscoveryOutcome::Completed(Err(error)) => {
                    items.push(core_error_item(target, error));
                }
                DiscoveryOutcome::Cancelled => {
                    items.push(status_item(
                        target,
                        DiscoveryStatus::Cancelled,
                        "cancelled",
                        "operation cancelled",
                    ));
                }
                DiscoveryOutcome::NotStarted => {
                    items.push(status_item(
                        target,
                        DiscoveryStatus::NotStarted,
                        "not_started",
                        "not started before cancellation",
                    ));
                }
                DiscoveryOutcome::Timeout => {
                    items.push(status_item(
                        target,
                        DiscoveryStatus::Timeout,
                        "timeout",
                        "discovery deadline exceeded",
                    ));
                }
            }
            completed_targets += 1;
            operation.progress(completed_targets, Some(target_count));
        }

        let interrupted = self.request_budget.cancellation.is_cancelled();
        Ok(DiscoveryReport::from_items(query, items, interrupted)?)
    }

    fn has_credential(&self, source: &str, field: &str) -> bool {
        self.config
            .sources
            .get(source)
            .and_then(|values| values.get(field))
            .and_then(serde_yaml_ng::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    }

    fn source_context(
        &self,
        query: Query,
        source_id: &str,
        request_coalescer: Arc<HttpRequestCoalescer>,
    ) -> SourceContext {
        let source_options = self.config.sources.get(source_id).cloned().unwrap_or_default();
        SourceContext {
            query,
            allow_network: self.config.runtime.allow_network,
            discovery_workers: self.config.runtime.discovery_workers,
            source_options: Arc::new(source_options),
            request_budget: self.request_budget.clone(),
            limits: limits_from_config(&self.config.runtime),
            http_transport: self.http_transport.clone(),
            ftp_transport: self.ftp_transport.clone(),
            request_coalescer,
        }
    }
}

fn merge_station_catalog_update(
    catalog: &mut SourceCatalog,
    update: StationCatalogUpdate,
) -> Result<(), EngineError> {
    if update.source_id != "rdcap" || update.stations.is_empty() {
        return Err(CoreError::Provider(ProviderError::CatalogUnavailable).into());
    }
    let source_index = catalog
        .sources
        .iter()
        .position(|source| source.id == update.source_id)
        .ok_or(CoreError::Provider(ProviderError::CatalogUnavailable))?;
    let valid_products = catalog.sources[source_index]
        .products
        .iter()
        .map(|product| product.id.as_str())
        .collect::<HashSet<_>>();
    let default_product = catalog.default_product("rdcap").map(str::to_owned);
    let mut live_stations = BTreeMap::new();
    for mut station in update.stations {
        if !is_valid_station_id(&station.id)
            || station.product_ids.iter().any(|product| !valid_products.contains(product.as_str()))
        {
            return Err(CoreError::Provider(ProviderError::UnexpectedBody).into());
        }
        if station.product_ids.is_empty()
            && let Some(default_product) = default_product.as_ref()
        {
            station.product_ids.push(default_product.clone());
        }
        if live_stations.insert(station.id.clone(), station).is_some() {
            return Err(CoreError::Provider(ProviderError::UnexpectedBody).into());
        }
    }

    let source = &mut catalog.sources[source_index];
    let mut merged = BTreeMap::new();
    for mut snapshot_station in std::mem::take(&mut source.stations) {
        if let Some(live_station) = live_stations.remove(&snapshot_station.id) {
            snapshot_station = merge_station(snapshot_station, live_station, "present");
        } else {
            snapshot_station.metadata = merge_catalog_metadata(
                snapshot_station.metadata.take(),
                None,
                "not_seen_in_refresh",
            );
        }
        merged.insert(snapshot_station.id.clone(), snapshot_station);
    }
    for (station_id, mut live_station) in live_stations {
        live_station.metadata =
            merge_catalog_metadata(None, live_station.metadata.take(), "new_from_live_directory");
        merged.insert(station_id, live_station);
    }
    source.stations = merged.into_values().collect();
    source.metadata = merge_catalog_metadata(source.metadata.take(), update.metadata, "refreshed");
    Ok(())
}

fn merge_station(
    snapshot: CatalogStation,
    mut live: CatalogStation,
    state: &str,
) -> CatalogStation {
    let coordinate_conflict = live.metadata.as_ref().is_some_and(|metadata| {
        metadata.directory_conflicts.iter().any(|conflict| conflict.field == "Longitude/Latitude")
    });
    let station_code = snapshot.id.rsplit('/').next().unwrap_or_default();
    if live.name.trim().is_empty() || live.name.eq_ignore_ascii_case(station_code) {
        live.name = snapshot.name.clone();
    }
    live.longitude = if coordinate_conflict { None } else { live.longitude.or(snapshot.longitude) };
    live.latitude = if coordinate_conflict { None } else { live.latitude.or(snapshot.latitude) };
    for product in snapshot.product_ids {
        if !live.product_ids.contains(&product) {
            live.product_ids.push(product);
        }
    }
    live.product_ids.sort();
    live.metadata = merge_catalog_metadata(snapshot.metadata, live.metadata.take(), state);
    live
}

fn merge_catalog_metadata(
    snapshot: Option<CatalogMetadata>,
    live: Option<CatalogMetadata>,
    live_state: &str,
) -> Option<CatalogMetadata> {
    let snapshot_present = snapshot.is_some();
    if snapshot.is_none() && live.is_none() {
        let mut metadata = CatalogMetadata::default();
        metadata.extensions.insert("snapshot_catalog_state".into(), json!("absent"));
        metadata.extensions.insert("live_catalog_state".into(), json!(live_state));
        return Some(metadata);
    }
    let mut merged = snapshot.unwrap_or_default();
    let snapshot_extensions = merged.extensions.clone();
    if let Some(live) = live {
        merged.country = live.country.or(merged.country);
        merged.recent_query = live.recent_query.or(merged.recent_query);
        for conflict in live.directory_conflicts {
            let already_present = merged.directory_conflicts.iter().any(|existing| {
                existing.station_id == conflict.station_id
                    && existing.field == conflict.field
                    && existing.values == conflict.values
            });
            if !already_present {
                merged.directory_conflicts.push(conflict);
            }
        }
        for provenance in live.provenance {
            if !merged.provenance.iter().any(|existing| {
                existing.source == provenance.source
                    && existing.reference == provenance.reference
                    && existing.observed_at == provenance.observed_at
            }) {
                merged.provenance.push(provenance);
            }
        }
        for (country, live_capabilities) in live.country_capabilities {
            let capabilities = merged.country_capabilities.entry(country).or_default();
            merge_capability(&mut capabilities.discovery, live_capabilities.discovery);
            merge_capability(&mut capabilities.raw_acquisition, live_capabilities.raw_acquisition);
            merge_capability(&mut capabilities.science, live_capabilities.science);
            merge_capability(&mut capabilities.readback, live_capabilities.readback);
        }
        for (key, value) in live.extensions {
            if snapshot_extensions.get(&key).is_some_and(|snapshot| snapshot != &value) {
                merged
                    .extensions
                    .insert(format!("snapshot_{key}"), snapshot_extensions[&key].clone());
            }
            merged.extensions.insert(key, value);
        }
    }
    merged.extensions.insert(
        "snapshot_catalog_state".into(),
        json!(if snapshot_present { "present" } else { "absent" }),
    );
    merged.extensions.insert("live_catalog_state".into(), json!(live_state));
    Some(merged)
}

fn merge_capability(target: &mut CatalogCapabilityStatus, live: CatalogCapabilityStatus) {
    if live != CatalogCapabilityStatus::Unverified || *target == CatalogCapabilityStatus::Unverified
    {
        *target = live;
    }
}

struct PreparedScienceOutput {
    _workspace: tempfile::TempDir,
    store: LocalStore,
    request: LocalCommitRequest,
    output_uri: String,
}

fn science_revision(frame: &FrameRef, field: &RadarField) -> crate::errors::CoreResult<String> {
    let metadata = serde_json::to_vec(&json!({
        "name": field.name,
        "shape": field.shape,
        "units": field.units,
        "valid_time": field.valid_time,
        "grid": field.grid,
        "provenance": field.provenance,
        "source_revision": frame.revision,
    }))
    .map_err(|_| CoreError::Storage("science identity could not be serialized".into()))?;
    let mut digest = Sha256::new();
    digest.update(b"radiust-memory-science-v1\0");
    digest.update(metadata);
    for value in &field.values {
        digest.update(value.to_le_bytes());
    }
    for quality in &field.quality {
        digest.update(quality.to_le_bytes());
    }
    Ok(hex::encode(digest.finalize()))
}

#[allow(clippy::too_many_arguments)]
fn prepare_science_output(
    output_root: PathBuf,
    frame: FrameRef,
    field: RadarField,
    revision: String,
    processing_spec: ProcessingSpec,
    format: &str,
    overwrite: bool,
    limits: Limits,
) -> crate::errors::CoreResult<PreparedScienceOutput> {
    if !processing_spec.options.is_object() {
        return Err(CoreError::Storage("writer options must be a JSON object".into()));
    }
    let (extension, format_name) = match format {
        "png" => ("png", "PNG"),
        "netcdf" => ("nc", "NetCDF4"),
        "geotiff" => ("tif", "GeoTIFF"),
        "zarr" => ("zarr", "Zarr"),
        _ => return Err(CoreError::Storage("unsupported science output format".into())),
    };
    let frame_id = logical_id(&frame).map_err(|error| CoreError::Storage(error.to_string()))?;
    let group = format!("frames/{frame_id}");
    let output_leaf = format!("decoded.{extension}");
    let output_name = format!("{group}/{output_leaf}");
    let store = LocalStore::new(&output_root, limits.clone())?;
    let workspace = tempfile::Builder::new()
        .prefix(".radiust-render-")
        .tempdir_in(store.root())
        .map_err(|error| CoreError::Temporary(format!("{format_name} staging failed: {error}")))?;
    let encoded_path = workspace.path().join(&output_leaf);
    let encoded = match format {
        "png" => crate::output::png::write_png(&field, &encoded_path, &processing_spec.options)?
            .into_iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| CoreError::Storage("PNG artifact path is invalid".into()))?;
                Ok((name.to_owned(), path))
            })
            .collect::<crate::errors::CoreResult<Vec<_>>>()?,
        "netcdf" => vec![(
            output_leaf.clone(),
            crate::output::netcdf::write_field(&field, &encoded_path, &limits)?,
        )],
        "geotiff" => crate::output::geotiff::write_field(&field, &encoded_path, &limits)?
            .into_iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| CoreError::Storage("GeoTIFF artifact path is invalid".into()))?;
                Ok((name.to_owned(), path))
            })
            .collect::<crate::errors::CoreResult<Vec<_>>>()?,
        "zarr" => {
            crate::output::zarr::write_field(&field, &encoded_path, &limits)?;
            collect_science_zarr_files(&encoded_path)?
                .into_iter()
                .map(|(relative, path)| (format!("{output_leaf}/{relative}"), path))
                .collect()
        }
        _ => unreachable!("output format was validated above"),
    };

    let mut total_bytes = 0_u64;
    let mut artifacts = Vec::with_capacity(encoded.len());
    for (name, path) in encoded {
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            CoreError::Temporary(format!("{format_name} output could not be inspected"))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(CoreError::Storage(format!("{format_name} output is not a regular file")));
        }
        total_bytes = total_bytes.checked_add(metadata.len()).ok_or_else(|| {
            CoreError::ResourceLimit(format!("{format_name} output size overflow"))
        })?;
        limits.validate_bytes(metadata.len(), total_bytes)?;
        if total_bytes > limits.max_temp_bytes / 3 {
            return Err(CoreError::ResourceLimit(format!(
                "{format_name} output exceeds the transactional temporary-storage budget"
            )));
        }
        let (role, media_type) = match format {
            "png" if name.ends_with(".json") => ("metadata", "application/json"),
            "png" => ("data", "image/png"),
            "netcdf" => ("data", "application/x-netcdf"),
            "geotiff" if name.ends_with("_provenance.json") => ("metadata", "application/json"),
            "geotiff" => ("data", "image/tiff"),
            "zarr" if name.rsplit('/').next().is_some_and(|part| part.starts_with(".z")) => {
                ("metadata", "application/json")
            }
            "zarr" => ("data", "application/octet-stream"),
            _ => return Err(CoreError::Storage("writer returned an unsupported artifact".into())),
        };
        artifacts.push(StagedArtifact {
            name: name.clone(),
            relative_uri: format!("{group}/{name}"),
            role: role.into(),
            media_type: media_type.into(),
            source: path,
        });
    }

    let request = LocalCommitRequest {
        frame,
        revision,
        processing_spec: processing_spec.clone(),
        output_name: output_name.clone(),
        artifacts,
        raw_complete: false,
        overwrite,
    };
    Ok(PreparedScienceOutput {
        _workspace: workspace,
        output_uri: store.root().join(output_name).display().to_string(),
        store,
        request,
    })
}

fn collect_science_zarr_files(root: &Path) -> crate::errors::CoreResult<Vec<(String, PathBuf)>> {
    fn visit(
        root: &Path,
        directory: &Path,
        files: &mut Vec<(String, PathBuf)>,
    ) -> crate::errors::CoreResult<()> {
        let entries = fs::read_dir(directory)
            .map_err(|_| CoreError::Storage("Zarr store could not be enumerated".into()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| CoreError::Storage("Zarr store could not be enumerated".into()))?;
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| CoreError::Storage("Zarr artifact could not be inspected".into()))?;
            if metadata.file_type().is_symlink() {
                return Err(CoreError::Storage("Zarr store contains a symbolic link".into()));
            }
            if metadata.is_dir() {
                visit(root, &path, files)?;
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| CoreError::Storage("Zarr artifact escaped its store".into()))?
                    .components()
                    .map(|component| match component {
                        std::path::Component::Normal(value) => {
                            value.to_str().map(str::to_owned).ok_or_else(|| {
                                CoreError::Storage("Zarr artifact path is invalid".into())
                            })
                        }
                        _ => Err(CoreError::Storage("Zarr artifact path is invalid".into())),
                    })
                    .collect::<crate::errors::CoreResult<Vec<_>>>()?
                    .join("/");
                files.push((relative, path));
            } else {
                return Err(CoreError::Storage("Zarr store contains a non-file artifact".into()));
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    if files.is_empty() {
        return Err(CoreError::Storage("Zarr writer produced an empty store".into()));
    }
    Ok(files)
}

fn finish_engine_operation<T>(
    operation: OperationContext,
    result: &Result<T, EngineError>,
    interrupted: bool,
) {
    if interrupted || matches!(result, Err(EngineError::Core(CoreError::Cancelled))) {
        operation.cancel();
    } else if result.is_err() {
        operation.fail();
    } else {
        operation.complete();
    }
}

fn finish_download_operation(
    operation: OperationContext,
    report: &crate::download::DownloadBatchReport,
) {
    if report.failed > 0 {
        operation.fail();
    } else if report.interrupted || report.cancelled > 0 {
        operation.cancel();
    } else {
        operation.complete();
    }
}

struct ArtifactRequest {
    url: String,
    name: String,
    media_type: String,
}

fn artifact_requests(frame: &FrameRef) -> Result<Vec<ArtifactRequest>, EngineError> {
    let locator = frame.locator.as_object().ok_or(EngineError::InvalidFrame)?;
    let primary_url =
        locator.get("url").and_then(serde_json::Value::as_str).ok_or(EngineError::InvalidFrame)?;
    let mut requests = vec![make_artifact_request(
        primary_url,
        locator.get("name").and_then(serde_json::Value::as_str),
        locator.get("media_type").and_then(serde_json::Value::as_str),
    )?];
    if let Some(extras) = locator.get("artifacts") {
        let extras = extras.as_array().ok_or(EngineError::InvalidFrame)?;
        for artifact in extras {
            let artifact = artifact.as_object().ok_or(EngineError::InvalidFrame)?;
            let url = artifact
                .get("url")
                .and_then(serde_json::Value::as_str)
                .ok_or(EngineError::InvalidFrame)?;
            requests.push(make_artifact_request(
                url,
                artifact.get("name").and_then(serde_json::Value::as_str),
                artifact.get("media_type").and_then(serde_json::Value::as_str),
            )?);
        }
    }
    Ok(requests)
}

fn raw_frame_cache_key(frame: &FrameRef) -> Option<String> {
    if frame.source == "rdcap" {
        return cache_key(frame, Some("rdcap-raw-v1"), "1", "1").ok();
    }
    let revision = frame.revision.as_deref()?;
    if matches!(frame.source.as_str(), "bmkg" | "opensnow" | "rainviewer" | "windy")
        || frame.locator.get("artifacts").and_then(serde_json::Value::as_array).is_some_and(
            |artifacts| {
                artifacts.iter().any(|artifact| {
                    artifact.get("role").and_then(serde_json::Value::as_str) == Some("tile")
                })
            },
        )
    {
        return None;
    }
    cache_key(frame, Some(revision), "1", "1").ok()
}

fn valid_cached_artifact_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':' | '\0'))
}

fn artifact_headers(frame: &FrameRef) -> Result<Vec<(String, String)>, EngineError> {
    let Some(value) = frame.locator.get("headers") else {
        return Ok(Vec::new());
    };
    let headers = value.as_object().ok_or(EngineError::InvalidFrame)?;
    headers
        .iter()
        .map(|(name, value)| {
            let value = value.as_str().ok_or(EngineError::InvalidFrame)?;
            Ok((name.clone(), value.to_owned()))
        })
        .collect()
}

fn make_artifact_request(
    url: &str,
    name: Option<&str>,
    media_type: Option<&str>,
) -> Result<ArtifactRequest, EngineError> {
    let parsed = url::Url::parse(url).map_err(|_| EngineError::InvalidFrame)?;
    let name = name
        .map(str::to_owned)
        .or_else(|| {
            parsed
                .path_segments()
                .and_then(Iterator::last)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "frame.bin".into());
    if name == "."
        || name == ".."
        || name.is_empty()
        || name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':' | '\0'))
    {
        return Err(EngineError::InvalidFrame);
    }
    let media_type = media_type.map(str::to_owned).unwrap_or_else(|| {
        match name.rsplit('.').next().unwrap_or_default().to_ascii_lowercase().as_str() {
            "png" => "image/png",
            "gif" => "image/gif",
            "jpg" | "jpeg" => "image/jpeg",
            "json" => "application/json",
            _ => "application/octet-stream",
        }
        .to_owned()
    });
    Ok(ArtifactRequest { url: url.to_owned(), name, media_type })
}

fn sanitize_fetch_error(error: CoreError, source: &str) -> CoreError {
    match error {
        CoreError::Transport(message)
            if source == "ph" && message == "source ph returned a placeholder data image" =>
        {
            CoreError::Transport(message)
        }
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled(format!("source {source} artifact acquisition is disabled"))
        }
        CoreError::HttpStatus { status, retryable } => CoreError::HttpStatus { status, retryable },
        CoreError::Provider(error) => CoreError::Provider(error),
        CoreError::ResourceLimit(message) => CoreError::ResourceLimit(message),
        CoreError::Temporary(message) => CoreError::Temporary(message),
        _ => CoreError::Transport(format!("source {source} artifact request failed")),
    }
}

fn limits_from_config(runtime: &RuntimeConfig) -> Limits {
    // A concurrently acquired batch keeps raw files alive while one frame is
    // copied into its transactional output stage. Reserve space for both the
    // retained batch and that commit copy inside the configured temp budget.
    let per_frame_temp_budget = runtime
        .max_temp_bytes
        .checked_div(runtime.frame_concurrency.max(1) as u64)
        .unwrap_or_default()
        .checked_div(2)
        .unwrap_or_default()
        .max(1);
    Limits {
        frame_concurrency: runtime.frame_concurrency,
        request_concurrency: runtime.request_concurrency,
        host_concurrency: runtime.host_concurrency,
        decode_workers: runtime.decode_workers,
        max_artifact_bytes: runtime.max_artifact_bytes,
        max_frame_bytes: runtime.max_frame_bytes.min(per_frame_temp_budget),
        max_pixels: runtime.max_pixels,
        max_temp_bytes: runtime.max_temp_bytes,
        request_timeout_secs: runtime.request_timeout.ceil() as u64,
        frame_deadline_secs: runtime.frame_deadline.ceil() as u64,
    }
}

fn required_credential(source: &str) -> Option<&'static str> {
    match source {
        "id" => Some("token"),
        "id_sidarma" | "wunderground" => Some("api_key"),
        _ => None,
    }
}

fn status_item(
    target: DiscoveryTarget,
    status: DiscoveryStatus,
    code: &str,
    message: &str,
) -> DiscoveryItem {
    DiscoveryItem {
        target,
        status,
        valid_time: None,
        frame: None,
        error: Some(SafeError {
            code: code.to_owned(),
            message: message.to_owned(),
            stage: "discover".to_owned(),
            retryable: false,
        }),
    }
}

fn core_error_item(target: DiscoveryTarget, error: CoreError) -> DiscoveryItem {
    let error = ErrorReport::from_core(&error, ErrorStage::Discover);
    let code = serde_json::to_value(error.code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "internal".to_owned());
    let status = match code.as_str() {
        "network_restricted" => DiscoveryStatus::NetworkRestricted,
        "cancelled" => DiscoveryStatus::Cancelled,
        "timeout" => DiscoveryStatus::Timeout,
        "ambiguous_index" => DiscoveryStatus::Ambiguous,
        _ => DiscoveryStatus::UpstreamFailed,
    };
    DiscoveryItem {
        target,
        status,
        valid_time: None,
        frame: None,
        error: Some(SafeError {
            code,
            message: error.message,
            stage: "discover".into(),
            retryable: error.retryable,
        }),
    }
}

enum FrameSelection {
    Selected(Vec<FrameRef>),
    NoData,
    Stale,
    Ambiguous(usize),
}

fn select_frame(
    query: &Query,
    frames: Vec<FrameRef>,
    multi_source: bool,
    now: DateTime<Utc>,
) -> FrameSelection {
    let mut matching = frames
        .into_iter()
        .filter(|frame| {
            query.base_time.as_deref().is_none_or(|time| {
                frame.base_time.as_deref().and_then(|frame_time| parse_utc_time(frame_time).ok())
                    == parse_utc_time(time).ok()
            })
        })
        .filter(|frame| match &query.selector {
            TimeSelector::Latest => true,
            TimeSelector::At { time } => {
                parse_utc_time(&frame.valid_time).ok() == parse_utc_time(time).ok()
            }
            TimeSelector::Range { start, end } => {
                let Ok(frame_time) = parse_utc_time(&frame.valid_time) else {
                    return false;
                };
                let (Ok(start), Ok(end)) = (parse_utc_time(start), parse_utc_time(end)) else {
                    return false;
                };
                frame_time >= start && frame_time < end
            }
        })
        .collect::<Vec<_>>();
    let mut identities = HashSet::with_capacity(matching.len());
    matching.retain(|frame| identities.insert((frame.logical_id.clone(), frame.revision.clone())));
    if matching.is_empty() {
        return FrameSelection::NoData;
    }
    if query.max_age_secs.is_some()
        && matching.iter().all(|frame| is_stale(frame, query.max_age_secs, now))
    {
        return FrameSelection::Stale;
    }
    matching.sort_by_key(|frame| parse_utc_time(&frame.valid_time).ok());
    if matches!(query.selector, TimeSelector::Latest) {
        let latest = matching.last().and_then(|frame| parse_utc_time(&frame.valid_time).ok());
        matching.retain(|frame| parse_utc_time(&frame.valid_time).ok() == latest);
    }
    if multi_source && !matches!(query.selector, TimeSelector::Range { .. }) && matching.len() > 1 {
        return FrameSelection::Ambiguous(matching.len());
    }
    if matches!(query.selector, TimeSelector::Range { .. }) {
        FrameSelection::Selected(matching)
    } else {
        FrameSelection::Selected(matching.into_iter().rev().take(1).collect())
    }
}

/// A source-level catalogue target can yield frames for several stations even
/// when the built-in catalogue does not enumerate those stations. Keep each
/// station's terminal state separate so single-source discovery preserves the
/// legacy per-station result set and multi-source reports remain unambiguous.
fn select_target_frames(
    query: &Query,
    target: DiscoveryTarget,
    frames: Vec<FrameRef>,
    multi_source: bool,
    now: DateTime<Utc>,
) -> Vec<DiscoveryItem> {
    if frames.is_empty() {
        return vec![status_item(
            target,
            DiscoveryStatus::NoData,
            "no_data",
            "no matching frame is available",
        )];
    }

    let mut by_station = BTreeMap::<(String, Option<String>), Vec<FrameRef>>::new();
    for frame in frames {
        if frame.source != target.source
            || target.product.as_deref().is_some_and(|product| product != frame.product)
            || target
                .station
                .as_ref()
                .is_some_and(|station| frame.station.as_ref() != Some(station))
        {
            return vec![status_item(
                target,
                DiscoveryStatus::UpstreamFailed,
                "invalid_frame",
                "source adapter returned a frame outside its discovery target",
            )];
        }
        let product = target.product.clone().unwrap_or_else(|| frame.product.clone());
        let station = target.station.clone().or_else(|| frame.station.clone());
        by_station.entry((product, station)).or_default().push(frame);
    }

    by_station
        .into_iter()
        .flat_map(|((product, station), candidates)| {
            let target =
                DiscoveryTarget { source: target.source.clone(), product: Some(product), station };
            let item = match select_frame(query, candidates, multi_source, now) {
                FrameSelection::Selected(frames) => {
                    return frames
                        .into_iter()
                        .map(|frame| DiscoveryItem {
                            target: target.clone(),
                            status: DiscoveryStatus::Success,
                            valid_time: Some(frame.valid_time.clone()),
                            frame: Some(frame),
                            error: None,
                        })
                        .collect::<Vec<_>>();
                }
                FrameSelection::NoData => {
                    let (code, message) = if target.source == "rdcap" {
                        ("no_matching_time", "no RDCAP frame matches the requested time")
                    } else {
                        ("no_data", "no matching frame is available")
                    };
                    status_item(target, DiscoveryStatus::NoData, code, message)
                }
                FrameSelection::Stale => status_item(
                    target,
                    DiscoveryStatus::Stale,
                    "stale",
                    "latest frame is older than --max-age",
                ),
                FrameSelection::Ambiguous(count) => {
                    let code =
                        if target.source == "rdcap" { "ambiguous_index" } else { "ambiguous" };
                    status_item(
                        target,
                        DiscoveryStatus::Ambiguous,
                        code,
                        &format!(
                            "{count} candidates for source/product/station; select a single source"
                        ),
                    )
                }
            };
            vec![item]
        })
        .collect()
}

fn is_stale(frame: &FrameRef, max_age_secs: Option<f64>, now: DateTime<Utc>) -> bool {
    let Some(max_age_secs) = max_age_secs else {
        return false;
    };
    let Ok(valid_time) = DateTime::parse_from_rfc3339(&frame.valid_time) else {
        return false;
    };
    let valid_time = valid_time.with_timezone(&Utc);
    valid_time <= now
        && now
            .signed_duration_since(valid_time)
            .to_std()
            .is_ok_and(|age| age.as_secs_f64() > max_age_secs)
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Query(#[from] crate::model::ModelError),
    #[error("frame identity or locator is invalid")]
    InvalidFrame,
    #[error("native acquisition is not available for source {0}")]
    UnsupportedSource(String),
    #[error("native scientific decoding is not available for source {0}")]
    UnsupportedScience(String),
    #[error("scientific variable {requested} is unavailable; decoded field is {available}")]
    UnsupportedVariable { requested: String, available: String },
    #[error(
        "coordinate transformation from {source_crs} to {target_crs} is unsupported without a verified transform"
    )]
    UnsupportedRegrid { source_crs: String, target_crs: String },
    #[error("invalid core configuration")]
    InvalidConfiguration,
    #[error("native operation runtime could not be initialized")]
    RuntimeUnavailable,
    #[error("invalid query: {0}")]
    InvalidQuery(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{SourceAdapter, SourceContext, SourceRegistry};
    use futures_util::future::BoxFuture;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn offline_discovery_reports_all_catalog_targets_without_network() {
        let engine = Engine::new(CoreConfig::default(), SourceRegistry::default()).unwrap();
        let report = engine
            .discover_seeded(Query { source: Some("all".into()), ..Query::default() }, 7)
            .await
            .unwrap();
        assert_eq!(report.counts.total, 74);
        assert_eq!(report.counts.missing_credentials, 3);
        assert_eq!(report.counts.retired, 1);
        assert_eq!(report.counts.network_restricted, 70);
    }

    #[test]
    fn directory_merge_separates_live_state_from_retained_snapshot_provenance() {
        let mut catalog = SourceCatalog::builtin().unwrap();
        let live_bale = CatalogStation {
            id: "PHL/BALE".into(),
            name: "Baler".into(),
            longitude: Some(121.6331),
            latitude: Some(15.7502),
            product_ids: vec!["reflectivity".into()],
            metadata: Some(CatalogMetadata {
                extensions: BTreeMap::from([
                    ("directory_record_ids".into(), json!(["5031"])),
                    ("directory_statuses".into(), json!(["Active"])),
                ]),
                ..CatalogMetadata::default()
            }),
        };
        let added = CatalogStation {
            id: "PHL/NEW1".into(),
            name: "New Radar".into(),
            longitude: None,
            latitude: None,
            product_ids: vec!["reflectivity".into()],
            metadata: None,
        };
        merge_station_catalog_update(
            &mut catalog,
            StationCatalogUpdate {
                source_id: "rdcap".into(),
                stations: vec![live_bale, added],
                metadata: None,
            },
        )
        .unwrap();

        let source = catalog.source("rdcap").unwrap();
        assert_eq!(source.stations.len(), 49);
        let bale = source.stations.iter().find(|station| station.id == "PHL/BALE").unwrap();
        let bale_metadata = bale.metadata.as_ref().unwrap();
        assert_eq!(bale_metadata.extensions["snapshot_catalog_state"], "present");
        assert_eq!(bale_metadata.extensions["live_catalog_state"], "present");
        assert_eq!(
            bale_metadata.extensions["snapshot_directory_statuses"],
            json!(["Inactive", "Active"])
        );
        assert_eq!(bale_metadata.extensions["directory_statuses"], json!(["Active"]));

        let retained = source.stations.iter().find(|station| station.id == "TWN/RCHL").unwrap();
        assert_eq!(
            retained.metadata.as_ref().unwrap().extensions["live_catalog_state"],
            "not_seen_in_refresh"
        );
        let added = source.stations.iter().find(|station| station.id == "PHL/NEW1").unwrap();
        assert_eq!(added.metadata.as_ref().unwrap().extensions["snapshot_catalog_state"], "absent");
        assert_eq!(
            added.metadata.as_ref().unwrap().extensions["live_catalog_state"],
            "new_from_live_directory"
        );
    }

    #[tokio::test]
    async fn scientific_decode_and_output_encoding_share_the_cpu_worker_budget() {
        let mut config = CoreConfig::default();
        config.runtime.decode_workers = 1;
        let engine = Engine::new(config, SourceRegistry::default()).unwrap();
        let held = engine.acquire_decode_worker().await.unwrap();

        assert!(
            tokio::time::timeout(Duration::from_millis(20), engine.acquire_decode_worker())
                .await
                .is_err(),
            "a second decode or encode worker exceeded the configured limit"
        );

        drop(held);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), engine.acquire_decode_worker())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn commits_from_one_engine_are_serialized_behind_the_shared_root_lock_worker() {
        let engine = Engine::new(CoreConfig::default(), SourceRegistry::default()).unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let job = || {
            let active = active.clone();
            let maximum = maximum.clone();
            move || {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(current, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok::<_, CoreError>(())
            }
        };

        let (first, second) = tokio::join!(engine.run_commit(job()), engine.run_commit(job()));

        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn source_context_only_exposes_options_for_the_selected_adapter() {
        let mut config = CoreConfig::default();
        config.sources.insert(
            "id_sidarma".into(),
            BTreeMap::from([(
                "api_key".into(),
                serde_yaml_ng::Value::String("external-test-key".into()),
            )]),
        );
        config.sources.insert(
            "vn".into(),
            BTreeMap::from([(
                "private_token".into(),
                serde_yaml_ng::Value::String("unrelated-secret".into()),
            )]),
        );
        let engine = Engine::new(config, SourceRegistry::default()).unwrap();

        let context = engine.source_context(
            Query::default(),
            "id_sidarma",
            Arc::new(HttpRequestCoalescer::default()),
        );

        assert_eq!(context.source_options.len(), 1);
        assert_eq!(
            context.source_options.get("api_key").and_then(serde_yaml_ng::Value::as_str),
            Some("external-test-key")
        );
        assert!(!context.source_options.contains_key("private_token"));
    }

    #[test]
    fn max_age_marks_old_latest_frames_stale_but_keeps_future_frames() {
        let now = DateTime::parse_from_rfc3339("2026-09-24T00:10:00Z").unwrap().to_utc();
        let old = FrameRef {
            source: "my".into(),
            product: "composite".into(),
            station: Some("east".into()),
            valid_time: "2026-09-24T00:00:00Z".into(),
            base_time: None,
            logical_id: "old".into(),
            revision: None,
            locator_version: "1".into(),
            locator: serde_json::Value::Null,
        };
        let future = FrameRef { valid_time: "2026-09-24T00:11:00Z".into(), ..old.clone() };
        assert!(is_stale(&old, Some(60.0), now));
        assert!(!is_stale(&old, None, now));
        assert!(!is_stale(&future, Some(60.0), now));
    }

    #[test]
    fn multi_source_vn_latest_selects_each_station_without_historical_ambiguity() {
        let query = Query { source: Some("all".into()), ..Query::default() };
        let target =
            DiscoveryTarget { source: "vn".into(), product: Some("cmax".into()), station: None };
        let stations = ["DHA", "NHB", "NHT", "PHA", "PLE", "PLI", "QNH", "TKY", "VIN", "VTR"];
        let latest = "2026-09-24T00:10:00Z";
        let frames = stations
            .into_iter()
            .flat_map(|station| {
                ["2026-09-24T00:00:00Z", latest].into_iter().map(move |time| {
                    let mut frame = FrameRef {
                        source: "vn".into(),
                        product: "cmax".into(),
                        station: Some(station.into()),
                        valid_time: time.into(),
                        base_time: None,
                        logical_id: String::new(),
                        revision: Some(format!("{station}-{time}")),
                        locator_version: "1".into(),
                        locator: serde_json::Value::Null,
                    };
                    frame.logical_id = crate::identity::logical_id(&frame).unwrap();
                    frame
                })
            })
            .collect();
        let items = select_target_frames(&query, target, frames, true, Utc::now());
        let report = DiscoveryReport::from_items(query, items, false).unwrap();
        assert_eq!(report.counts.success, stations.len());
        assert_eq!(report.counts.ambiguous, 0);
        for (item, station) in report.items.iter().zip(stations) {
            assert_eq!(item.target.station.as_deref(), Some(station));
            assert_eq!(item.valid_time.as_deref(), Some(latest));
        }
    }

    #[test]
    fn multi_source_target_with_multiple_latest_candidates_is_ambiguous() {
        let query = Query { source: Some("all".into()), ..Query::default() };
        let first = FrameRef {
            source: "ca".into(),
            product: "rain".into(),
            station: Some("CA1".into()),
            valid_time: "2026-09-24T00:00:00Z".into(),
            base_time: None,
            logical_id: "first".into(),
            revision: None,
            locator_version: "1".into(),
            locator: serde_json::Value::Null,
        };
        let second =
            FrameRef { station: Some("CA2".into()), logical_id: "second".into(), ..first.clone() };

        assert!(matches!(
            select_frame(&query, vec![first, second], true, Utc::now()),
            FrameSelection::Ambiguous(2)
        ));
    }

    #[test]
    fn range_discovery_keeps_all_station_frames_and_excludes_end() {
        let query = Query {
            source: Some("ca".into()),
            selector: TimeSelector::Range {
                start: "2026-09-24T00:00:00Z".into(),
                end: "2026-09-24T00:10:00Z".into(),
            },
            ..Query::default()
        };
        let target = DiscoveryTarget {
            source: "ca".into(),
            product: Some("rain".into()),
            station: Some("CA1".into()),
        };
        let frames = [
            "2026-09-24T00:10:00Z",
            "2026-09-24T00:05:00Z",
            "2026-09-23T23:55:00Z",
            "2026-09-24T00:00:00Z",
        ]
        .into_iter()
        .map(|time| {
            let mut frame = FrameRef {
                source: "ca".into(),
                product: "rain".into(),
                station: Some("CA1".into()),
                valid_time: time.into(),
                base_time: None,
                logical_id: String::new(),
                revision: None,
                locator_version: "1".into(),
                locator: serde_json::Value::Null,
            };
            frame.logical_id = crate::identity::logical_id(&frame).unwrap();
            frame
        })
        .collect();
        let items = select_target_frames(&query, target, frames, true, Utc::now());
        let report = DiscoveryReport::from_items(query.clone(), items, false).unwrap();
        assert_eq!(report.counts.success, 2);
        assert_eq!(
            report.items.iter().map(|item| item.valid_time.as_deref().unwrap()).collect::<Vec<_>>(),
            vec!["2026-09-24T00:00:00Z", "2026-09-24T00:05:00Z"]
        );
        let duplicate = vec![report.items[0].clone(), report.items[0].clone()];
        assert!(DiscoveryReport::from_items(query.clone(), duplicate, false).is_err());
        let latest = Query { selector: TimeSelector::Latest, ..query };
        assert!(DiscoveryReport::from_items(latest, report.items, false).is_err());
    }

    struct FixtureAdapter {
        id: &'static str,
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
    }

    impl SourceAdapter for FixtureAdapter {
        fn source_id(&self) -> &'static str {
            self.id
        }

        fn allows_artifact_host(&self, host: &str) -> bool {
            host.eq_ignore_ascii_case("127.0.0.1")
        }

        fn discover(
            self: Arc<Self>,
            target: DiscoveryTarget,
            _context: SourceContext,
        ) -> BoxFuture<'static, crate::errors::CoreResult<Vec<FrameRef>>> {
            Box::pin(async move {
                let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                self.active.fetch_sub(1, Ordering::SeqCst);
                let mut frame = FrameRef {
                    source: target.source,
                    product: target.product.unwrap_or_else(|| "default".into()),
                    station: target.station,
                    valid_time: "2026-09-24T00:00:00Z".into(),
                    base_time: None,
                    logical_id: String::new(),
                    revision: None,
                    locator_version: "1".into(),
                    locator: serde_json::Value::Null,
                };
                frame.logical_id = crate::identity::logical_id(&frame).map_err(|_| {
                    CoreError::Transport("fixture identity could not be computed".into())
                })?;
                Ok(vec![frame])
            })
        }
    }

    struct CachedRdcapAdapter {
        fetches: Arc<AtomicUsize>,
    }

    impl SourceAdapter for CachedRdcapAdapter {
        fn source_id(&self) -> &'static str {
            "rdcap"
        }

        fn discover(
            self: Arc<Self>,
            _target: DiscoveryTarget,
            _context: SourceContext,
        ) -> BoxFuture<'static, crate::errors::CoreResult<Vec<FrameRef>>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn fetch_raw(
            self: Arc<Self>,
            mut frame: FrameRef,
            _context: SourceContext,
            temp_root: PathBuf,
        ) -> Option<BoxFuture<'static, crate::errors::CoreResult<RawFrame>>> {
            let fetches = self.fetches.clone();
            Some(Box::pin(async move {
                use std::io::Write;

                fetches.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(40)).await;
                std::fs::create_dir_all(&temp_root)
                    .map_err(|_| CoreError::Temporary("test staging is unavailable".into()))?;
                let content = serde_json::to_vec(&"cached rdcap payload").unwrap();
                let content_sha256 = hex::encode(Sha256::digest(&content));
                frame.revision = Some(content_sha256.clone());
                let response = tempfile::Builder::new()
                    .prefix("rdcap-cache-response-")
                    .tempfile_in(&temp_root)
                    .map_err(|_| CoreError::Temporary("test response staging failed".into()))?;
                let mut response = response;
                response
                    .write_all(&content)
                    .and_then(|()| response.as_file().sync_all())
                    .map_err(|_| CoreError::Temporary("test response staging failed".into()))?;
                let response_path = response.into_temp_path();
                let response_receipt = crate::transport::http::HttpBodyReceipt {
                    size_bytes: content.len() as u64,
                    sha256: content_sha256,
                };
                let binding = crate::source::rdcap::binding_bytes(&frame, &response_receipt)?;
                let binding_digest = hex::encode(Sha256::digest(&binding));
                let mut binding_file = tempfile::Builder::new()
                    .prefix("rdcap-cache-binding-")
                    .tempfile_in(&temp_root)
                    .map_err(|_| CoreError::Temporary("test binding staging failed".into()))?;
                binding_file
                    .write_all(&binding)
                    .and_then(|()| binding_file.as_file().sync_all())
                    .map_err(|_| CoreError::Temporary("test binding staging failed".into()))?;
                let binding_path = binding_file.into_temp_path();
                Ok(RawFrame {
                    frame,
                    artifacts: vec![
                        RawArtifact {
                            receipt: ArtifactReceipt {
                                name: "file-response.json".into(),
                                media_type: "application/json".into(),
                                size_bytes: response_receipt.size_bytes,
                                sha256: response_receipt.sha256,
                            },
                            path: response_path,
                        },
                        RawArtifact {
                            receipt: ArtifactReceipt {
                                name: "binding.json".into(),
                                media_type: "application/json".into(),
                                size_bytes: binding.len() as u64,
                                sha256: binding_digest,
                            },
                            path: binding_path,
                        },
                    ],
                    private_locator: None,
                })
            }))
        }
    }

    fn cached_rdcap_frame(ticket: &str) -> FrameRef {
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
                "key": "1790834708000",
                "url": format!("https://rdcap.cwa.gov.tw/file?ft={ticket}"),
                "headers": {"Referer": "https://rdcap.cwa.gov.tw/data_access/radar_display/TWN/RCHL"},
            }),
        };
        frame.logical_id = crate::identity::logical_id(&frame).unwrap();
        frame
    }

    struct DynamicStationAdapter;

    impl SourceAdapter for DynamicStationAdapter {
        fn source_id(&self) -> &'static str {
            "ca"
        }

        fn discover(
            self: Arc<Self>,
            target: DiscoveryTarget,
            _context: SourceContext,
        ) -> BoxFuture<'static, crate::errors::CoreResult<Vec<FrameRef>>> {
            Box::pin(async move {
                let stations = target
                    .station
                    .clone()
                    .map(|station| vec![station])
                    .unwrap_or_else(|| vec!["CA01".into(), "CA02".into()]);
                stations
                    .into_iter()
                    .map(|station| {
                        let mut frame = FrameRef {
                            source: target.source.clone(),
                            product: target.product.clone().unwrap_or_else(|| "rain".into()),
                            station: Some(station),
                            valid_time: "2026-09-24T00:00:00Z".into(),
                            base_time: None,
                            logical_id: String::new(),
                            revision: None,
                            locator_version: "fixture-v1".into(),
                            locator: serde_json::Value::Null,
                        };
                        frame.logical_id = crate::identity::logical_id(&frame).map_err(|_| {
                            CoreError::Transport("fixture identity could not be computed".into())
                        })?;
                        Ok(frame)
                    })
                    .collect()
            })
        }
    }

    #[tokio::test]
    async fn engine_dispatches_fixture_sources_concurrently_with_configured_bound() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let mut registry = SourceRegistry::default();
        // Shadow every provider adapter with a fixture so this concurrency
        // test cannot reach the public network. Catalog preflight still
        // reports credential/retirement gates for a subset of targets.
        for id in [
            "au",
            "bmkg",
            "ca",
            "cam",
            "es",
            "fr",
            "id",
            "id_sidarma",
            "kr",
            "my",
            "nz",
            "opensnow",
            "ph",
            "pt",
            "rdcap",
            "rainviewer",
            "sg",
            "th",
            "th_royalrain",
            "tw",
            "tw-http",
            "uk",
            "vn",
            "windy",
            "wunderground",
        ] {
            registry
                .register(Arc::new(FixtureAdapter {
                    id,
                    active: active.clone(),
                    maximum: maximum.clone(),
                }))
                .unwrap();
        }
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.discovery_workers = 2;
        let engine = Engine::new(config, registry).unwrap();
        let report = engine
            .discover_seeded(Query { source: Some("all".into()), ..Query::default() }, 91)
            .await
            .unwrap();

        assert_eq!(maximum.load(Ordering::SeqCst), 2);
        assert_eq!(report.counts.success, 70);
        assert_eq!(report.items.len(), 74);
        assert_eq!(report.items.first().unwrap().target.source, "au");
    }

    #[tokio::test]
    async fn one_catalog_target_reports_each_dynamic_station_separately() {
        let mut registry = SourceRegistry::default();
        registry.register(Arc::new(DynamicStationAdapter)).unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        let engine = Engine::new(config, registry).unwrap();

        let report = engine
            .discover_seeded(Query { source: Some("ca".into()), ..Query::default() }, 8)
            .await
            .unwrap();

        assert_eq!(report.counts.total, 2);
        assert_eq!(report.counts.success, 2);
        assert_eq!(
            report.items.iter().map(|item| item.target.station.as_deref()).collect::<Vec<_>>(),
            vec![Some("CA01"), Some("CA02")]
        );
        assert!(report.items.iter().all(|item| {
            item.frame.as_ref().is_some_and(|frame| frame.station == item.target.station)
        }));
    }

    #[tokio::test]
    async fn engine_fetches_raw_artifacts_into_owned_temporary_files() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 256];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            assert!(request.starts_with(b"GET /frame.png HTTP/1.1\r\n"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload",
                )
                .await
                .unwrap();
        });

        let cache = tempfile::tempdir().unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.temp_root = Some(cache.path().to_path_buf());
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let mut sources = SourceRegistry::default();
        sources.register(Arc::new(FixtureAdapter { id: "au", active, maximum })).unwrap();
        let engine = Engine::new(config, sources).unwrap();
        let mut frame = FrameRef {
            source: "au".into(),
            product: "composite".into(),
            station: None,
            valid_time: "2026-09-24T00:00:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "1".into(),
            locator: serde_json::json!({
                "url": format!("http://{address}/frame.png"),
                "name": "frame.png",
                "media_type": "image/png",
                "artifacts": [],
            }),
        };
        frame.logical_id = crate::identity::logical_id(&frame).unwrap();

        let raw = engine.fetch_raw(frame).await.unwrap();
        server.await.unwrap();
        assert_eq!(raw.artifacts.len(), 1);
        assert_eq!(raw.artifacts[0].receipt.name, "frame.png");
        assert_eq!(raw.artifacts[0].receipt.size_bytes, 7);
        assert_eq!(std::fs::read(&raw.artifacts[0].path).unwrap(), b"payload");
        let path = raw.artifacts[0].path.to_path_buf();
        drop(raw);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn raw_artifact_cache_is_reused_by_a_network_disabled_engine() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 256];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            assert!(request.starts_with(b"GET /cached.png HTTP/1.1\r\n"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload",
                )
                .await
                .unwrap();
        });

        let workspace = tempfile::tempdir().unwrap();
        let mut online_config = CoreConfig::default();
        online_config.runtime.allow_network = true;
        online_config.runtime.temp_root = Some(workspace.path().join("runtime-temp"));
        online_config.cache.dir = workspace.path().join("cache");
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let mut sources = SourceRegistry::default();
        sources.register(Arc::new(FixtureAdapter { id: "au", active, maximum })).unwrap();
        let online = Engine::new(online_config.clone(), sources.clone()).unwrap();

        let mut frame = FrameRef {
            source: "au".into(),
            product: "composite".into(),
            station: None,
            valid_time: "2026-09-24T00:00:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: Some("provider-revision-1".into()),
            locator_version: "fixture-v1".into(),
            locator: serde_json::json!({
                "url": format!("http://{address}/cached.png"),
                "name": "cached.png",
                "media_type": "image/png",
                "artifacts": [],
            }),
        };
        frame.logical_id = crate::identity::logical_id(&frame).unwrap();

        let first = online.fetch_raw(frame.clone()).await.unwrap();
        assert_eq!(std::fs::read(&first.artifacts[0].path).unwrap(), b"payload");
        drop(first);
        server.await.unwrap();

        let mut offline_config = online_config;
        offline_config.runtime.allow_network = false;
        let offline = Engine::new(offline_config, sources).unwrap();
        let cached = offline.fetch_raw(frame).await.unwrap();
        assert_eq!(cached.artifacts[0].receipt.name, "cached.png");
        assert_eq!(cached.artifacts[0].receipt.size_bytes, 7);
        assert_eq!(std::fs::read(&cached.artifacts[0].path).unwrap(), b"payload");
    }

    #[tokio::test]
    async fn concurrent_rdcap_raw_fetches_share_one_result_across_ticket_rotation() {
        let workspace = tempfile::tempdir().unwrap();
        let fetches = Arc::new(AtomicUsize::new(0));
        let mut overrides = SourceRegistry::default();
        overrides.register(Arc::new(CachedRdcapAdapter { fetches: fetches.clone() })).unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.temp_root = Some(workspace.path().join("staging"));
        config.cache.dir = workspace.path().join("cache");
        let engine = Engine::new(config, overrides).unwrap();
        let first_frame = cached_rdcap_frame("ticket-one");
        let second_frame = cached_rdcap_frame("ticket-two");
        assert_eq!(first_frame.logical_id, second_frame.logical_id);

        let (first, second) =
            tokio::join!(engine.fetch_raw(first_frame), engine.fetch_raw(second_frame));
        let first = first.unwrap();
        let second = second.unwrap();

        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        assert_eq!(first.frame.revision, second.frame.revision);
        assert_eq!(first.artifacts.len(), 2);
        assert_eq!(second.artifacts.len(), 2);
        assert_eq!(
            std::fs::read(&first.artifacts[0].path).unwrap(),
            std::fs::read(&second.artifacts[0].path).unwrap()
        );
        assert!(!serde_json::to_string(&second.public_receipt()).unwrap().contains("ticket-two"));
    }

    #[tokio::test]
    async fn rdcap_raw_only_commit_is_offline_loadable_and_idempotent() {
        let workspace = tempfile::tempdir().unwrap();
        let fetches = Arc::new(AtomicUsize::new(0));
        let mut overrides = SourceRegistry::default();
        overrides.register(Arc::new(CachedRdcapAdapter { fetches: fetches.clone() })).unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.temp_root = Some(workspace.path().join("staging"));
        config.storage.output = workspace.path().join("output");
        config.cache.dir = workspace.path().join("cache");
        let engine = Engine::new(config, overrides).unwrap();
        let frame = cached_rdcap_frame("private-raw-only-ticket");

        let first = engine
            .download_raw_only(vec![frame.clone()], FetchErrorPolicy::Collect, false, false)
            .await;
        assert_eq!(first.written, 1);
        assert_eq!(first.failed, 0);
        let manifest =
            workspace.path().join(format!("output/frames/{}/raw-manifest.json", frame.logical_id));
        let manifest_text = std::fs::read_to_string(&manifest).unwrap();
        assert!(!manifest_text.contains("private-raw-only-ticket"));
        assert!(!manifest_text.contains("Referer"));
        let raw = crate::raw_manifest::load(
            &manifest,
            &workspace.path().join("offline-staging"),
            &limits_from_config(&engine.config.runtime),
        )
        .unwrap();
        assert_eq!(raw.artifacts.len(), 2);
        assert_eq!(
            raw.frame.revision.as_deref(),
            Some(
                hex::encode(Sha256::digest(serde_json::to_vec(&"cached rdcap payload").unwrap()))
                    .as_str()
            )
        );

        let second =
            engine.download_raw_only(vec![frame], FetchErrorPolicy::Collect, false, false).await;
        assert_eq!(second.skipped, 1);
        assert_eq!(second.written, 0);
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn disabling_the_raw_cache_keeps_each_acquisition_on_the_source_path() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 256];
                loop {
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                assert!(request.starts_with(b"GET /uncached.png HTTP/1.1\r\n"));
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload",
                    )
                    .await
                    .unwrap();
            }
        });

        let workspace = tempfile::tempdir().unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.cache.dir = workspace.path().join("cache-is-a-file");
        std::fs::write(&config.cache.dir, b"not a directory").unwrap();
        config.cache.enabled = false;
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let mut sources = SourceRegistry::default();
        sources.register(Arc::new(FixtureAdapter { id: "au", active, maximum })).unwrap();
        let engine = Engine::new(config, sources).unwrap();
        let mut frame = FrameRef {
            source: "au".into(),
            product: "composite".into(),
            station: None,
            valid_time: "2026-09-24T00:00:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: Some("provider-revision-2".into()),
            locator_version: "fixture-v1".into(),
            locator: serde_json::json!({
                "url": format!("http://{address}/uncached.png"),
                "name": "uncached.png",
                "media_type": "image/png",
                "artifacts": [],
            }),
        };
        frame.logical_id = crate::identity::logical_id(&frame).unwrap();

        for _ in 0..2 {
            let raw = engine.fetch_raw(frame.clone()).await.unwrap();
            assert_eq!(std::fs::read(&raw.artifacts[0].path).unwrap(), b"payload");
        }
        server.await.unwrap();
        assert_eq!(std::fs::read(&engine.config.cache.dir).unwrap(), b"not a directory");
    }

    #[tokio::test]
    async fn cache_gc_does_not_remove_an_active_raw_artifact_stage() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::oneshot;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (partial_sent, partial_received) = oneshot::channel();
        let (resume_server, resume_server_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 256];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npay")
                .await
                .unwrap();
            let _ = partial_sent.send(());
            let _ = resume_server_rx.await;
            stream.write_all(b"load").await.unwrap();
        });

        let workspace = tempfile::tempdir().unwrap();
        let cache_root = workspace.path().join("cache");
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.cache.dir = cache_root.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let mut sources = SourceRegistry::default();
        sources.register(Arc::new(FixtureAdapter { id: "au", active, maximum })).unwrap();
        let engine = Arc::new(Engine::new(config, sources).unwrap());
        let mut frame = FrameRef {
            source: "au".into(),
            product: "composite".into(),
            station: None,
            valid_time: "2026-09-24T00:00:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "fixture-v1".into(),
            locator: serde_json::json!({
                "url": format!("http://{address}/frame.png"),
                "name": "frame.png",
                "artifacts": [],
            }),
        };
        frame.logical_id = crate::identity::logical_id(&frame).unwrap();
        let fetch = tokio::spawn({
            let engine = engine.clone();
            async move { engine.fetch_raw(frame).await }
        });
        partial_received.await.unwrap();

        let staging_root = cache_root.join("tmp/raw");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if std::fs::read_dir(&staging_root).is_ok_and(|entries| {
                    entries.filter_map(Result::ok).any(|entry| entry.path().is_file())
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("raw response staging file is created");

        let cache = crate::cache::Cache::open(&cache_root).unwrap();
        cache.gc(u64::MAX).unwrap();
        assert!(
            std::fs::read_dir(&staging_root)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.path().is_file()),
            "cache GC must leave the nested active raw stage intact"
        );

        let _ = resume_server.send(());
        let raw = fetch.await.unwrap().unwrap();
        server.await.unwrap();
        assert_eq!(raw.artifacts[0].receipt.size_bytes, 7);
        let artifact_path = raw.artifacts[0].path.to_path_buf();
        drop(raw);
        assert!(!artifact_path.exists());
    }

    #[tokio::test]
    async fn raw_fetch_enforces_the_remaining_frame_budget_during_each_artifact_stream() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for expected_path in ["/one.png", "/two.png"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 256];
                loop {
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                assert!(
                    request.starts_with(format!("GET {expected_path} HTTP/1.1\r\n").as_bytes())
                );
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nframe",
                    )
                    .await
                    .unwrap();
            }
        });

        let temp_root = tempfile::tempdir().unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.max_artifact_bytes = 5;
        config.runtime.max_frame_bytes = 8;
        config.runtime.temp_root = Some(temp_root.path().to_path_buf());
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let mut sources = SourceRegistry::default();
        sources.register(Arc::new(FixtureAdapter { id: "au", active, maximum })).unwrap();
        let engine = Engine::new(config, sources).unwrap();
        let mut frame = FrameRef {
            source: "au".into(),
            product: "composite".into(),
            station: None,
            valid_time: "2026-09-24T00:00:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "1".into(),
            locator: serde_json::json!({
                "url": format!("http://{address}/one.png"),
                "name": "one.png",
                "artifacts": [{
                    "url": format!("http://{address}/two.png"),
                    "name": "two.png",
                }],
            }),
        };
        frame.logical_id = crate::identity::logical_id(&frame).unwrap();

        let error = engine.fetch_raw(frame).await.unwrap_err();
        server.await.unwrap();

        assert!(matches!(error, EngineError::Core(CoreError::ResourceLimit(_))));
        assert_eq!(std::fs::read_dir(temp_root.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn raw_fetch_rejects_hosts_outside_the_source_adapter_allow_list() {
        let cache = tempfile::tempdir().unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.temp_root = Some(cache.path().to_path_buf());
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let mut sources = SourceRegistry::default();
        sources.register(Arc::new(FixtureAdapter { id: "au", active, maximum })).unwrap();
        let engine = Engine::new(config, sources).unwrap();
        let mut frame = FrameRef {
            source: "au".into(),
            product: "composite".into(),
            station: None,
            valid_time: "2026-09-24T00:00:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "1".into(),
            locator: serde_json::json!({
                "url": "https://example.com/frame.png?token=private-marker",
                "name": "frame.png",
                "artifacts": [],
            }),
        };
        frame.logical_id = crate::identity::logical_id(&frame).unwrap();

        let error = engine.fetch_raw(frame).await.unwrap_err();
        assert!(matches!(&error, EngineError::UnsupportedSource(_)));
        assert!(!error.to_string().contains("private-marker"));
    }

    #[tokio::test]
    async fn raw_fetch_rejects_a_same_host_locator_that_does_not_match_sg_frame_identity() {
        let cache = tempfile::tempdir().unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.temp_root = Some(cache.path().to_path_buf());
        let engine = Engine::new(config, SourceRegistry::default()).unwrap();
        let mut frame = FrameRef {
            source: "sg".into(),
            product: "composite".into(),
            station: Some("SGCOMP".into()),
            valid_time: "2026-09-18T02:45:00.000000Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: Some("2026091810450000".into()),
            locator_version: "sg-legacy-v1".into(),
            locator: serde_json::json!({
                "url": "https://www.weather.gov.sg/files/rainarea/240km/dpsri_240km_2026091811000000dBR.dpsri.png",
                "station": "SGCOMP",
                "revision": "2026091810450000",
            }),
        };
        frame.logical_id = crate::identity::logical_id(&frame).unwrap();

        let error = engine.fetch_raw(frame).await.unwrap_err();

        assert!(matches!(error, EngineError::UnsupportedSource(source) if source == "sg"));
        assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
    }
}
