use crate::limits::{Limits, RequestBudget};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::broadcast;

/// Default bound for operation event delivery. Slow subscribers can detect
/// dropped events through `broadcast::Receiver::recv` returning `Lagged`.
pub const DEFAULT_EVENT_CAPACITY: usize = 256;

static NEXT_OPERATION_ID: AtomicU64 = AtomicU64::new(1);

/// Stable operation names emitted by an Engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Discover,
    FetchRaw,
    DecodeScience,
    Regrid,
    DownloadRawOnly,
    DownloadPng,
    DownloadNetcdf,
    DownloadZarr,
    DownloadGeotiff,
}

impl OperationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::FetchRaw => "fetch_raw",
            Self::DecodeScience => "decode_science",
            Self::Regrid => "regrid",
            Self::DownloadRawOnly => "download_raw_only",
            Self::DownloadPng => "download_png",
            Self::DownloadNetcdf => "download_netcdf",
            Self::DownloadZarr => "download_zarr",
            Self::DownloadGeotiff => "download_geotiff",
        }
    }
}

/// Process-unique identifier for one Engine operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct OperationId(u64);

impl OperationId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Lifecycle phase carried by an operation event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStage {
    Started,
    Progress,
    Completed,
    Cancelled,
    Failed,
}

/// Optional count-based progress. `total` is absent when the work size is
/// not known in advance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct OperationProgress {
    pub completed: u64,
    pub total: Option<u64>,
}

/// Safe event payload: it deliberately has no input, locator, error, or
/// artifact fields.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OperationEvent {
    pub operation_id: OperationId,
    pub operation: OperationKind,
    pub stage: OperationStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<OperationProgress>,
}

/// Cloneable bounded publisher and subscription source for operation events.
#[derive(Clone)]
pub struct RuntimeEvents {
    sender: broadcast::Sender<OperationEvent>,
}

impl Default for RuntimeEvents {
    fn default() -> Self {
        Self::new(DEFAULT_EVENT_CAPACITY)
    }
}

impl RuntimeEvents {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender }
    }

    /// Subscribe before starting operations to observe their initial events.
    /// The bounded channel reports missed events as `RecvError::Lagged`.
    pub fn subscribe(&self) -> RuntimeEventReceiver {
        self.sender.subscribe()
    }

    /// Begin an operation and immediately publish its `started` event.
    pub fn begin(&self, operation: OperationKind) -> OperationContext {
        let context = OperationContext {
            sender: self.sender.clone(),
            id: OperationId(NEXT_OPERATION_ID.fetch_add(1, Ordering::Relaxed)),
            operation,
            finished: false,
        };
        context.emit(OperationStage::Started, None);
        context
    }
}

pub type RuntimeEventReceiver = broadcast::Receiver<OperationEvent>;

/// RAII lifecycle handle. Dropping an unfinished handle reports cancellation,
/// which covers a caller dropping the future that owns an Engine operation.
pub struct OperationContext {
    sender: broadcast::Sender<OperationEvent>,
    id: OperationId,
    operation: OperationKind,
    finished: bool,
}

impl OperationContext {
    pub const fn id(&self) -> OperationId {
        self.id
    }

    pub fn progress(&self, completed: u64, total: Option<u64>) {
        if !self.finished {
            self.emit(OperationStage::Progress, Some(OperationProgress { completed, total }));
        }
    }

    pub fn complete(self) {
        self.finish(OperationStage::Completed);
    }

    pub fn cancel(self) {
        self.finish(OperationStage::Cancelled);
    }

    pub fn fail(self) {
        self.finish(OperationStage::Failed);
    }

    fn finish(mut self, stage: OperationStage) {
        self.finished = true;
        self.emit(stage, None);
    }

    fn emit(&self, stage: OperationStage, progress: Option<OperationProgress>) {
        // Broadcast send errors mean there are no subscribers. They must never
        // affect the Engine operation itself.
        let _ = self.sender.send(OperationEvent {
            operation_id: self.id,
            operation: self.operation,
            stage,
            progress,
        });
    }
}

impl Drop for OperationContext {
    fn drop(&mut self) {
        if !self.finished {
            self.finished = true;
            self.emit(OperationStage::Cancelled, None);
        }
    }
}

pub struct RuntimeManager {
    runtime: Runtime,
    pub budget: Arc<RequestBudget>,
}

impl RuntimeManager {
    pub fn new(limits: Limits) -> std::io::Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(limits.decode_workers.max(1))
            .enable_all()
            .build()?;
        Ok(Self { runtime, budget: Arc::new(RequestBudget::new(&limits)) })
    }

    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    pub fn cancel(&self) {
        self.budget.cancel();
    }
}
