//! Ordered, bounded raw-frame acquisition with deterministic partial results.

use crate::engine::{Engine, EngineError};
use crate::error_contract::{ErrorCode, ErrorReport, ErrorStage};
use crate::errors::CoreError;
use crate::grid::Resampling;
use crate::identity::{ProcessingSpec, digest, logical_id, output_id, safe_ref, variant_id};
use crate::limits::Limits;
use crate::model::{FrameRef, Grid, RadarField, RawFrame, parse_utc_time};
use crate::storage::manifest::is_safe_relative_path;
use crate::storage::object::{ObjectStore, ObjectStoreConfig};
use crate::storage::{
    LocalCommitRequest, LocalCommitStatus, LocalStore, RemoteStore, StagedArtifact,
};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::future::join_all;
use futures_util::stream::FuturesUnordered;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use url::Url;

fn schedule_fetch<'a>(
    index: usize,
    frame: FrameRef,
    engine: &'a Engine,
    cancellation: CancellationToken,
    in_flight: &mut FuturesUnordered<
        BoxFuture<'a, (usize, FrameRef, Result<RawFrame, EngineError>)>,
    >,
    running: &mut BTreeMap<usize, (FrameRef, CancellationToken)>,
) {
    let frame_for_fetch = frame.clone();
    let frame_for_running = frame.clone();
    let cancellation_for_fetch = cancellation.clone();
    in_flight.push(Box::pin(async move {
        let result = tokio::select! {
            biased;
            _ = cancellation_for_fetch.cancelled() => Err(EngineError::Core(CoreError::Cancelled)),
            result = engine.fetch_raw(frame_for_fetch) => result,
        };
        (index, frame, result)
    }));
    running.insert(index, (frame_for_running, cancellation));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FetchErrorPolicy {
    Collect,
    Stop,
}

impl FetchErrorPolicy {
    /// Normalize the public batch aliases while keeping execution policy explicit.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "collect" | "continue" => Some(Self::Collect),
            "stop" | "raise" => Some(Self::Stop),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FetchStatus {
    Planned,
    Success,
    Failed,
    Cancelled,
    NotStarted,
}

#[derive(Debug)]
pub struct FetchItem {
    pub error_details: Option<ErrorReport>,
    pub input_index: usize,
    pub frame: FrameRef,
    pub status: FetchStatus,
    pub raw: Option<RawFrame>,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct FetchBatchReport {
    pub items: Vec<FetchItem>,
    pub planned: usize,
    pub success: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub not_started: usize,
}

impl FetchBatchReport {
    fn from_items(items: Vec<FetchItem>) -> Self {
        let mut report = Self { items, ..Self::default() };
        for item in &report.items {
            match item.status {
                FetchStatus::Planned => report.planned += 1,
                FetchStatus::Success => report.success += 1,
                FetchStatus::Failed => report.failed += 1,
                FetchStatus::Cancelled => report.cancelled += 1,
                FetchStatus::NotStarted => report.not_started += 1,
            }
        }
        report
    }
}

/// Ordered result of acquiring and scientifically decoding a batch.
#[derive(Debug)]
pub struct DecodedFetchItem {
    pub input_index: usize,
    pub frame: FrameRef,
    pub status: FetchStatus,
    pub data: Option<RadarField>,
    pub error: Option<String>,
    pub error_details: Option<ErrorReport>,
}

#[derive(Debug, Default)]
pub struct DecodedFetchBatchReport {
    pub items: Vec<DecodedFetchItem>,
    pub planned: usize,
    pub success: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub not_started: usize,
}

impl DecodedFetchBatchReport {
    fn from_items(items: Vec<DecodedFetchItem>) -> Self {
        let mut report = Self { items, ..Self::default() };
        for item in &report.items {
            match item.status {
                FetchStatus::Planned => report.planned += 1,
                FetchStatus::Success => report.success += 1,
                FetchStatus::Failed => report.failed += 1,
                FetchStatus::Cancelled => report.cancelled += 1,
                FetchStatus::NotStarted => report.not_started += 1,
            }
        }
        report
    }
}

/// Completion-ordered decoded fetch stream retained across binding calls.
/// Completed values remain in `in_flight` until the caller asks for them, so
/// they continue to count against the configured prefetch bound.
#[cfg(any(feature = "extension-module", test))]
pub struct DecodedFetchStream {
    engine: Arc<Engine>,
    frames: Vec<FrameRef>,
    concurrency: usize,
    policy: FetchErrorPolicy,
    next_index: usize,
    in_flight: FuturesUnordered<BoxFuture<'static, DecodedFetchItem>>,
    cancellation: CancellationToken,
    emitted: Vec<Option<DecodedFetchItem>>,
    closed: bool,
}

#[derive(Debug)]
#[cfg(any(feature = "extension-module", test))]
pub struct DecodedFetchStreamFailure {
    pub partial_result: DecodedFetchBatchReport,
    pub cause: String,
}

#[cfg(any(feature = "extension-module", test))]
impl DecodedFetchStream {
    pub(crate) fn new(
        engine: Arc<Engine>,
        frames: Vec<FrameRef>,
        concurrency: usize,
        policy: FetchErrorPolicy,
    ) -> Self {
        let count = frames.len();
        Self {
            engine,
            frames,
            concurrency: concurrency.max(1),
            policy,
            next_index: 0,
            in_flight: FuturesUnordered::new(),
            cancellation: CancellationToken::new(),
            emitted: std::iter::repeat_with(|| None).take(count).collect(),
            closed: false,
        }
    }

    pub(crate) fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub(crate) async fn next(
        &mut self,
    ) -> Result<Option<DecodedFetchItem>, DecodedFetchStreamFailure> {
        if self.closed {
            return Ok(None);
        }
        self.fill();
        let completed = tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => None,
            item = self.in_flight.next() => item,
        };
        let Some(item) = completed else {
            self.close();
            return Ok(None);
        };

        if self.policy == FetchErrorPolicy::Stop {
            let index = item.input_index;
            self.emitted[index] = Some(decoded_item_summary(&item));
            if item.status == FetchStatus::Failed {
                let cause = item.error.clone().unwrap_or_else(|| "native fetch failed".into());
                let partial_result = self.partial_result();
                self.close();
                return Err(DecodedFetchStreamFailure { partial_result, cause });
            }
        }
        self.fill();
        Ok(Some(item))
    }

    pub(crate) fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.cancellation.cancel();
        self.in_flight = FuturesUnordered::new();
        self.emitted.clear();
    }

    fn fill(&mut self) {
        while self.in_flight.len() < self.concurrency && self.next_index < self.frames.len() {
            let index = self.next_index;
            self.next_index += 1;
            let frame = self.frames[index].clone();
            let join_frame = frame.clone();
            let engine = self.engine.clone();
            let cancellation = self.cancellation.child_token();
            let worker = tokio::spawn(async move {
                let raw = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => Err(EngineError::Core(CoreError::Cancelled)),
                    result = engine.fetch_raw(frame.clone()) => result,
                };
                match raw {
                    Ok(raw) => {
                        let decoded = tokio::select! {
                            biased;
                            _ = cancellation.cancelled() => {
                                Err(EngineError::Core(CoreError::Cancelled))
                            }
                            result = engine.decode_science(Arc::new(raw)) => result,
                        };
                        match decoded {
                            Ok(data) => DecodedFetchItem {
                                input_index: index,
                                frame,
                                status: FetchStatus::Success,
                                data: Some(data),
                                error: None,
                                error_details: None,
                            },
                            Err(EngineError::Core(CoreError::Cancelled)) => DecodedFetchItem {
                                input_index: index,
                                frame,
                                status: FetchStatus::Cancelled,
                                data: None,
                                error: Some("operation cancelled".into()),
                                error_details: None,
                            },
                            Err(error) => {
                                let error_details = engine_error_report(&error, ErrorStage::Decode);
                                DecodedFetchItem {
                                    input_index: index,
                                    frame,
                                    status: FetchStatus::Failed,
                                    data: None,
                                    error: Some(error_details.message.clone()),
                                    error_details: Some(error_details),
                                }
                            }
                        }
                    }
                    Err(EngineError::Core(CoreError::Cancelled)) => DecodedFetchItem {
                        input_index: index,
                        frame,
                        status: FetchStatus::Cancelled,
                        data: None,
                        error: Some("operation cancelled".into()),
                        error_details: None,
                    },
                    Err(error) => {
                        let error_details = engine_error_report(&error, ErrorStage::Acquire);
                        DecodedFetchItem {
                            input_index: index,
                            frame,
                            status: FetchStatus::Failed,
                            data: None,
                            error: Some(error_details.message.clone()),
                            error_details: Some(error_details),
                        }
                    }
                }
            });
            self.in_flight.push(Box::pin(async move {
                worker.await.unwrap_or_else(|error| {
                    let cancelled = error.is_cancelled();
                    DecodedFetchItem {
                        input_index: index,
                        frame: join_frame,
                        status: if cancelled {
                            FetchStatus::Cancelled
                        } else {
                            FetchStatus::Failed
                        },
                        data: None,
                        error: Some(if cancelled {
                            "operation cancelled".into()
                        } else {
                            "native fetch worker panicked".into()
                        }),
                        error_details: None,
                    }
                })
            }));
        }
    }

    fn partial_result(&self) -> DecodedFetchBatchReport {
        let items = self
            .frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                self.emitted[index].as_ref().map(decoded_item_summary).unwrap_or_else(|| {
                    if index < self.next_index {
                        DecodedFetchItem {
                            input_index: index,
                            frame: frame.clone(),
                            status: FetchStatus::Cancelled,
                            data: None,
                            error: Some("operation cancelled".into()),
                            error_details: None,
                        }
                    } else {
                        DecodedFetchItem {
                            input_index: index,
                            frame: frame.clone(),
                            status: FetchStatus::NotStarted,
                            data: None,
                            error: Some("not started after batch failure".into()),
                            error_details: None,
                        }
                    }
                })
            })
            .collect();
        DecodedFetchBatchReport::from_items(items)
    }
}

#[cfg(any(feature = "extension-module", test))]
fn decoded_item_summary(item: &DecodedFetchItem) -> DecodedFetchItem {
    DecodedFetchItem {
        input_index: item.input_index,
        frame: item.frame.clone(),
        status: item.status,
        data: None,
        error: item.error.clone(),
        error_details: item.error_details.clone(),
    }
}

/// Decode successful acquisitions concurrently under the Engine's shared CPU
/// worker limit while preserving their original input order and per-frame errors.
pub async fn decode_fetch_batch(
    engine: &Engine,
    report: FetchBatchReport,
) -> DecodedFetchBatchReport {
    let items = join_all(report.items.into_iter().map(|item| async move {
        let FetchItem { input_index, frame, status, raw, error, error_details } = item;
        if status != FetchStatus::Success {
            return DecodedFetchItem {
                input_index,
                frame,
                status,
                data: None,
                error,
                error_details,
            };
        }
        let Some(raw) = raw else {
            return DecodedFetchItem {
                input_index,
                frame,
                status: FetchStatus::Failed,
                data: None,
                error: Some("native fetch report omitted a successful raw frame".into()),
                error_details: Some(ErrorReport {
                    code: ErrorCode::Internal,
                    message: "native fetch report omitted a successful raw frame".into(),
                    stage: ErrorStage::Decode,
                    retryable: false,
                }),
            };
        };
        match engine.decode_science(Arc::new(raw)).await {
            Ok(data) => DecodedFetchItem {
                input_index,
                frame,
                status: FetchStatus::Success,
                data: Some(data),
                error: None,
                error_details: None,
            },
            Err(error) => {
                let error_details = engine_error_report(&error, ErrorStage::Decode);
                DecodedFetchItem {
                    input_index,
                    frame,
                    status: FetchStatus::Failed,
                    data: None,
                    error: Some(error_details.message.clone()),
                    error_details: Some(error_details),
                }
            }
        }
    }))
    .await;
    DecodedFetchBatchReport::from_items(items)
}

fn schedule_decoded_fetch<'a>(
    index: usize,
    frame: FrameRef,
    engine: &'a Engine,
    cancellation: CancellationToken,
    in_flight: &mut FuturesUnordered<
        BoxFuture<'a, (usize, FrameRef, Result<RadarField, (EngineError, ErrorStage)>)>,
    >,
    running: &mut BTreeMap<usize, (FrameRef, CancellationToken)>,
) {
    let frame_for_fetch = frame.clone();
    let cancellation_for_fetch = cancellation.clone();
    in_flight.push(Box::pin(async move {
        let result = tokio::select! {
            biased;
            _ = cancellation_for_fetch.cancelled() => {
                Err((EngineError::Core(CoreError::Cancelled), ErrorStage::Acquire))
            }
            result = async {
                let raw = engine
                    .fetch_raw(frame_for_fetch)
                    .await
                    .map_err(|error| (error, ErrorStage::Acquire))?;
                engine
                    .decode_science(Arc::new(raw))
                    .await
                    .map_err(|error| (error, ErrorStage::Decode))
            } => result,
        };
        (index, frame, result)
    }));
    running.insert(index, (frame.clone(), cancellation));
}

/// Acquire and decode each frame in one bounded task so a decode failure can
/// stop the scheduler before it starts later frames.
pub async fn fetch_many_decoded(
    engine: &Engine,
    frames: Vec<FrameRef>,
    concurrency: usize,
    policy: FetchErrorPolicy,
    dry_run: bool,
) -> DecodedFetchBatchReport {
    if dry_run {
        return DecodedFetchBatchReport::from_items(
            frames
                .into_iter()
                .enumerate()
                .map(|(input_index, frame)| DecodedFetchItem {
                    input_index,
                    frame,
                    status: FetchStatus::Planned,
                    data: None,
                    error: None,
                    error_details: None,
                })
                .collect(),
        );
    }

    let count = frames.len();
    if engine.is_cancelled() {
        return DecodedFetchBatchReport::from_items(
            frames
                .into_iter()
                .enumerate()
                .map(|(input_index, frame)| DecodedFetchItem {
                    input_index,
                    frame,
                    status: FetchStatus::NotStarted,
                    data: None,
                    error: Some("not started after cancellation".into()),
                    error_details: None,
                })
                .collect(),
        );
    }

    let mut items: Vec<Option<DecodedFetchItem>> =
        std::iter::repeat_with(|| None).take(count).collect();
    let mut in_flight: FuturesUnordered<
        BoxFuture<'_, (usize, FrameRef, Result<RadarField, (EngineError, ErrorStage)>)>,
    > = FuturesUnordered::new();
    let mut running = BTreeMap::<usize, (FrameRef, CancellationToken)>::new();
    let mut next = 0;
    let concurrency = concurrency.max(1);
    let mut stop_after = None;

    while next < count && in_flight.len() < concurrency {
        schedule_decoded_fetch(
            next,
            frames[next].clone(),
            engine,
            CancellationToken::new(),
            &mut in_flight,
            &mut running,
        );
        next += 1;
    }

    while let Some((index, frame, result)) = in_flight.next().await {
        let cancellation = running
            .remove(&index)
            .map(|(_, cancellation)| cancellation)
            .unwrap_or_else(CancellationToken::new);
        let cancelled = matches!(
            &result,
            Err((EngineError::Core(CoreError::Cancelled), _))
        );
        let after_failure = stop_after.is_some_and(|failed_index| index > failed_index);
        let failed = result.is_err() && !cancelled && !after_failure;
        let cancelled_message = if after_failure || cancellation.is_cancelled() {
            "cancelled after another frame failed"
        } else {
            "operation cancelled"
        };
        items[index] = Some(if after_failure {
            DecodedFetchItem {
                input_index: index,
                frame,
                status: FetchStatus::Cancelled,
                data: None,
                error: Some(cancelled_message.into()),
                error_details: None,
            }
        } else {
            match result {
                Ok(data) => DecodedFetchItem {
                    input_index: index,
                    frame,
                    status: FetchStatus::Success,
                    data: Some(data),
                    error: None,
                    error_details: None,
                },
                Err((EngineError::Core(CoreError::Cancelled), _)) => DecodedFetchItem {
                    input_index: index,
                    frame,
                    status: FetchStatus::Cancelled,
                    data: None,
                    error: Some(cancelled_message.into()),
                    error_details: None,
                },
                Err((error, stage)) => {
                    let error_details = engine_error_report(&error, stage);
                    let message = if stage == ErrorStage::Acquire {
                        error.to_string()
                    } else {
                        error_details.message.clone()
                    };
                    DecodedFetchItem {
                        input_index: index,
                        frame,
                        status: FetchStatus::Failed,
                        data: None,
                        error: Some(message),
                        error_details: Some(error_details),
                    }
                }
            }
        });

        if engine.is_cancelled() || (cancelled && !after_failure) {
            for (index, (frame, cancellation)) in std::mem::take(&mut running) {
                cancellation.cancel();
                items[index] = Some(DecodedFetchItem {
                    input_index: index,
                    frame,
                    status: FetchStatus::Cancelled,
                    data: None,
                    error: Some("operation cancelled".into()),
                    error_details: None,
                });
            }
            drop(in_flight);
            for index in next..count {
                items[index] = Some(DecodedFetchItem {
                    input_index: index,
                    frame: frames[index].clone(),
                    status: FetchStatus::NotStarted,
                    data: None,
                    error: Some("not started after cancellation".into()),
                    error_details: None,
                });
            }
            break;
        }

        if failed && policy == FetchErrorPolicy::Stop {
            let failed_index = stop_after.map_or(index, |earlier| earlier.min(index));
            stop_after = Some(failed_index);
            for (completed_index, item) in items.iter_mut().enumerate() {
                if completed_index > failed_index && item.is_some() {
                    *item = Some(DecodedFetchItem {
                        input_index: completed_index,
                        frame: frames[completed_index].clone(),
                        status: FetchStatus::Cancelled,
                        data: None,
                        error: Some("cancelled after another frame failed".into()),
                        error_details: None,
                    });
                }
            }
            for (running_index, (_, cancellation)) in &running {
                if *running_index > failed_index {
                    cancellation.cancel();
                }
            }
        }

        if stop_after.is_none() && next < count {
            schedule_decoded_fetch(
                next,
                frames[next].clone(),
                engine,
                CancellationToken::new(),
                &mut in_flight,
                &mut running,
            );
            next += 1;
        }
    }

    let items = items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            item.unwrap_or_else(|| DecodedFetchItem {
                input_index: index,
                frame: frames[index].clone(),
                status: FetchStatus::NotStarted,
                data: None,
                error: Some(if stop_after.is_some() {
                    "not started after an earlier frame failed".into()
                } else {
                    "not started".into()
                }),
                error_details: None,
            })
        })
        .collect();
    DecodedFetchBatchReport::from_items(items)
}

pub async fn fetch_many_raw(
    engine: &Engine,
    frames: Vec<FrameRef>,
    concurrency: usize,
    policy: FetchErrorPolicy,
    dry_run: bool,
) -> FetchBatchReport {
    if dry_run {
        return FetchBatchReport::from_items(
            frames
                .into_iter()
                .enumerate()
                .map(|(input_index, frame)| FetchItem {
                    error_details: None,
                    input_index,
                    frame,
                    status: FetchStatus::Planned,
                    raw: None,
                    error: None,
                })
                .collect(),
        );
    }

    let count = frames.len();
    if engine.is_cancelled() {
        return FetchBatchReport::from_items(
            frames
                .into_iter()
                .enumerate()
                .map(|(input_index, frame)| FetchItem {
                    error_details: None,
                    input_index,
                    frame,
                    status: FetchStatus::NotStarted,
                    raw: None,
                    error: Some("not started after cancellation".into()),
                })
                .collect(),
        );
    }
    let mut items: Vec<Option<FetchItem>> = std::iter::repeat_with(|| None).take(count).collect();
    let mut in_flight: FuturesUnordered<
        BoxFuture<'_, (usize, FrameRef, Result<RawFrame, EngineError>)>,
    > = FuturesUnordered::new();
    let mut running = BTreeMap::<usize, (FrameRef, CancellationToken)>::new();
    let mut next = 0;
    let concurrency = concurrency.max(1);
    let mut stop_after = None;

    while next < count && in_flight.len() < concurrency {
        let index = next;
        let frame = frames[index].clone();
        schedule_fetch(
            index,
            frame,
            engine,
            CancellationToken::new(),
            &mut in_flight,
            &mut running,
        );
        next += 1;
    }

    while let Some((index, frame, result)) = in_flight.next().await {
        let cancellation = running
            .remove(&index)
            .map(|(_, cancellation)| cancellation)
            .unwrap_or_else(CancellationToken::new);
        let cancelled = matches!(&result, Err(EngineError::Core(CoreError::Cancelled)));
        let after_failure = stop_after.is_some_and(|failed_index| index > failed_index);
        let failed = result.is_err() && !cancelled && !after_failure;
        let cancelled_message = if after_failure || cancellation.is_cancelled() {
            "cancelled after another frame failed"
        } else {
            "operation cancelled"
        };
        items[index] = Some(if after_failure {
            cancelled_fetch_item(index, frame, cancelled_message)
        } else {
            match result {
                Ok(raw) => FetchItem {
                    error_details: None,
                    input_index: index,
                    frame,
                    status: FetchStatus::Success,
                    raw: Some(raw),
                    error: None,
                },
                Err(EngineError::Core(CoreError::Cancelled)) => {
                    cancelled_fetch_item(index, frame, cancelled_message)
                }
                Err(error) => FetchItem {
                    error_details: Some(engine_error_report(&error, ErrorStage::Acquire)),
                    input_index: index,
                    frame,
                    status: FetchStatus::Failed,
                    raw: None,
                    error: Some(error.to_string()),
                },
            }
        });

        if engine.is_cancelled() || (cancelled && !after_failure) {
            for (index, (frame, cancellation)) in std::mem::take(&mut running) {
                cancellation.cancel();
                items[index] = Some(cancelled_fetch_item(index, frame, "operation cancelled"));
            }
            drop(in_flight);
            for index in next..count {
                items[index] = Some(not_started_fetch_item(
                    index,
                    frames[index].clone(),
                    "not started after cancellation",
                ));
            }
            break;
        }

        if failed && policy == FetchErrorPolicy::Stop {
            let failed_index = stop_after.map_or(index, |earlier| earlier.min(index));
            stop_after = Some(failed_index);
            for (completed_index, item) in items.iter_mut().enumerate() {
                if completed_index > failed_index && item.is_some() {
                    *item = Some(cancelled_fetch_item(
                        completed_index,
                        frames[completed_index].clone(),
                        "cancelled after another frame failed",
                    ));
                }
            }
            for (running_index, (_, cancellation)) in &running {
                if *running_index > failed_index {
                    cancellation.cancel();
                }
            }
        }

        if stop_after.is_none() && next < count {
            let frame = frames[next].clone();
            schedule_fetch(
                next,
                frame,
                engine,
                CancellationToken::new(),
                &mut in_flight,
                &mut running,
            );
            next += 1;
        }
    }

    let items = items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            item.unwrap_or_else(|| {
                not_started_fetch_item(
                    index,
                    frames[index].clone(),
                    if stop_after.is_some() {
                        "not started after an earlier frame failed"
                    } else {
                        "not started"
                    },
                )
            })
        })
        .collect();
    FetchBatchReport::from_items(items)
}

fn cancelled_fetch_item(input_index: usize, frame: FrameRef, message: &str) -> FetchItem {
    FetchItem {
        error_details: None,
        input_index,
        frame,
        status: FetchStatus::Cancelled,
        raw: None,
        error: Some(message.into()),
    }
}

fn not_started_fetch_item(input_index: usize, frame: FrameRef, message: &str) -> FetchItem {
    FetchItem {
        error_details: None,
        input_index,
        frame,
        status: FetchStatus::NotStarted,
        raw: None,
        error: Some(message.into()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Planned,
    Written,
    Skipped,
    Failed,
    Cancelled,
    NotStarted,
}

#[derive(Clone, Debug, Serialize)]
pub struct DownloadItem {
    pub input_index: usize,
    pub frame: FrameRef,
    pub status: DownloadStatus,
    pub output_uri: Option<String>,
    pub error: Option<ErrorReport>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DownloadBatchReport {
    pub items: Vec<DownloadItem>,
    pub interrupted: bool,
    pub planned: usize,
    pub written: usize,
    pub skipped: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub not_started: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DecodedGrid {
    Native,
    Geographic { bbox: [f64; 4], resolution: f64 },
}

impl Default for DecodedGrid {
    fn default() -> Self {
        Self::Native
    }
}

impl DecodedGrid {
    fn name(&self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Geographic { .. } => "geographic",
        }
    }

    fn bbox(&self) -> Option<[f64; 4]> {
        match self {
            Self::Native => None,
            Self::Geographic { bbox, .. } => Some(*bbox),
        }
    }

    fn resolution(&self) -> Option<f64> {
        match self {
            Self::Native => None,
            Self::Geographic { resolution, .. } => Some(*resolution),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedProcessing {
    pub variable: Option<String>,
    pub grid: DecodedGrid,
    pub resampling: Resampling,
}

impl Default for DecodedProcessing {
    fn default() -> Self {
        Self { variable: None, grid: DecodedGrid::Native, resampling: Resampling::Nearest }
    }
}

fn geographic_target_grid(
    bbox: [f64; 4],
    resolution: f64,
    limits: &Limits,
) -> crate::errors::CoreResult<Grid> {
    let [west, south, east, north] = bbox;
    if bbox.iter().any(|value| !value.is_finite())
        || !resolution.is_finite()
        || resolution <= 0.0
        || !(-180.0..=180.0).contains(&west)
        || !(-180.0..=180.0).contains(&east)
        || !(-90.0..=90.0).contains(&south)
        || !(-90.0..=90.0).contains(&north)
        || west >= east
        || south >= north
    {
        return Err(CoreError::Storage("geographic bbox or resolution is invalid".into()));
    }

    let axis_len = |start: f64, end: f64| -> crate::errors::CoreResult<usize> {
        // Match the legacy np.arange(start, end + resolution / 2, resolution)
        // endpoint convention while checking the budget before allocating axes.
        let count = ((end - start) / resolution + 0.5).ceil();
        if !count.is_finite() || count < 1.0 || count > limits.max_pixels as f64 {
            return Err(CoreError::ResourceLimit(
                "target grid dimensions exceed the pixel limit".into(),
            ));
        }
        usize::try_from(count as u64)
            .map_err(|_| CoreError::ResourceLimit("target grid dimensions overflow".into()))
    };
    let width = axis_len(west, east)?;
    let height = axis_len(south, north)?;
    let cells = width
        .checked_mul(height)
        .ok_or_else(|| CoreError::ResourceLimit("target grid dimensions overflow".into()))?;
    limits.validate_pixels(cells as u64)?;

    let coordinates = |start: f64, count: usize| -> crate::errors::CoreResult<Vec<f64>> {
        let mut values = Vec::new();
        values.try_reserve_exact(count).map_err(|_| {
            CoreError::ResourceLimit("target grid coordinate allocation failed".into())
        })?;
        values.extend((0..count).map(|index| start + index as f64 * resolution));
        Ok(values)
    };
    let x = coordinates(west, width)?;
    let y = coordinates(south, height)?;
    let shape = vec![height, width];
    Ok(Grid { shape, crs: Some("EPSG:4326".into()), x, y, affine: None })
}

#[derive(Clone)]
enum CommitBackend {
    Local(LocalStore),
    Remote { store: RemoteStore, uri: String },
}

impl CommitBackend {
    fn staging_root(&self, engine: &Engine) -> crate::errors::CoreResult<PathBuf> {
        match self {
            Self::Local(store) => Ok(store.root().to_path_buf()),
            Self::Remote { .. } => {
                let root =
                    engine.config().runtime.temp_root.clone().unwrap_or_else(std::env::temp_dir);
                fs::create_dir_all(&root).map_err(|_| {
                    CoreError::Temporary("remote output staging root could not be prepared".into())
                })?;
                Ok(root)
            }
        }
    }
}

fn remote_pointer_uri(remote_uri: &str, output_name: &str) -> String {
    format!("{}/{}.manifest.json", remote_uri.trim_end_matches('/'), output_name)
}

#[derive(Debug)]
struct RemoteOutputTarget {
    provider: String,
    bucket: String,
    prefix: String,
    uri: String,
}

fn parse_remote_output_target(
    value: &str,
) -> crate::errors::CoreResult<Option<RemoteOutputTarget>> {
    if !value.contains("://") {
        return Ok(None);
    }
    let url =
        Url::parse(value).map_err(|_| CoreError::Storage("remote output URI is invalid".into()))?;
    let provider = url.scheme().to_ascii_lowercase();
    if !matches!(provider.as_str(), "s3" | "oss") {
        return Err(CoreError::Storage("remote output must use s3:// or oss://".into()));
    }
    let bucket = url
        .host_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CoreError::Storage("remote output bucket is missing".into()))?
        .to_owned();
    if !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CoreError::Storage(
            "remote output URI must not contain credentials, ports, query, or fragment".into(),
        ));
    }
    let prefix = url.path().trim_matches('/').to_owned();
    let uri = if prefix.is_empty() {
        format!("{provider}://{bucket}")
    } else {
        format!("{provider}://{bucket}/{prefix}")
    };
    Ok(Some(RemoteOutputTarget { provider, bucket, prefix, uri }))
}

fn create_commit_backend(engine: &Engine, root: &Path) -> crate::errors::CoreResult<CommitBackend> {
    let output = root.to_string_lossy();
    if let Some(target) = parse_remote_output_target(&output)? {
        if !engine.config().runtime.allow_network {
            return Err(CoreError::NetworkDisabled("remote object storage".into()));
        }
        let storage = &engine.config().storage;
        let objects = ObjectStore::from_config(ObjectStoreConfig {
            provider: target.provider,
            bucket: target.bucket,
            prefix: target.prefix,
            endpoint: storage.endpoint.clone(),
            region: storage.region.clone(),
            access_key_id: storage.access_key.clone(),
            secret_access_key: storage.secret_key.clone(),
            anonymous: storage.anonymous,
        })?;
        return Ok(CommitBackend::Remote {
            store: RemoteStore::new(
                objects,
                engine.config().runtime.max_artifact_bytes,
                engine.config().runtime.max_frame_bytes,
            ),
            uri: target.uri,
        });
    }
    LocalStore::new(root, engine.resource_limits()).map(CommitBackend::Local)
}

impl DownloadBatchReport {
    fn from_items(items: Vec<DownloadItem>) -> Self {
        let mut report = Self { items, ..Self::default() };
        for item in &report.items {
            match item.status {
                DownloadStatus::Planned => report.planned += 1,
                DownloadStatus::Written => report.written += 1,
                DownloadStatus::Skipped => report.skipped += 1,
                DownloadStatus::Failed => report.failed += 1,
                DownloadStatus::Cancelled => report.cancelled += 1,
                DownloadStatus::NotStarted => report.not_started += 1,
            }
        }
        report
    }
}

/// Download source bytes and publish a raw-only group using the same v1
/// manifest protocol as Python. Fetches are bounded; outputs are committed in
/// input order so stop mode preserves only the ordered partial result.
pub async fn download_raw_only(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
) -> DownloadBatchReport {
    download_raw_only_to(
        engine,
        frames,
        policy,
        dry_run,
        overwrite,
        engine.config().storage.output.clone(),
    )
    .await
}

pub async fn download_raw_only_to(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
    output_root: PathBuf,
) -> DownloadBatchReport {
    if dry_run {
        return DownloadBatchReport::from_items(
            frames
                .into_iter()
                .enumerate()
                .map(|(input_index, frame)| DownloadItem {
                    input_index,
                    frame,
                    status: DownloadStatus::Planned,
                    output_uri: None,
                    error: None,
                })
                .collect(),
        );
    }
    if frames.is_empty() {
        return DownloadBatchReport::default();
    }

    let config = engine.config().clone();
    let runtime = &config.runtime;
    let backend = match create_commit_backend(engine, &output_root) {
        Ok(backend) => backend,
        Err(error) => {
            let report = ErrorReport::from_core(&error, ErrorStage::Commit);
            return DownloadBatchReport::from_items(
                frames
                    .into_iter()
                    .enumerate()
                    .map(|(input_index, frame)| DownloadItem {
                        input_index,
                        frame,
                        status: DownloadStatus::Failed,
                        output_uri: None,
                        error: Some(report.clone()),
                    })
                    .collect(),
            );
        }
    };

    let concurrency = runtime.frame_concurrency.max(1);
    let mut output_items = Vec::with_capacity(frames.len());
    let mut offset = 0;
    let mut stop = false;
    while offset < frames.len() && !stop {
        if engine.is_cancelled() {
            break;
        }
        let end = (offset + concurrency).min(frames.len());
        let fetched =
            fetch_many_raw(engine, frames[offset..end].to_vec(), concurrency, policy, false).await;
        let mut failed_in_order = false;
        for fetched_item in fetched.items {
            let input_index = offset + fetched_item.input_index;
            let frame = fetched_item.frame;
            if failed_in_order {
                output_items.push(DownloadItem {
                    input_index,
                    frame,
                    status: DownloadStatus::Cancelled,
                    output_uri: None,
                    error: Some(cancelled_after_failure()),
                });
                continue;
            }

            match fetched_item.status {
                FetchStatus::Planned => output_items.push(DownloadItem {
                    input_index,
                    frame,
                    status: DownloadStatus::Planned,
                    output_uri: None,
                    error: None,
                }),
                FetchStatus::Failed => {
                    output_items.push(DownloadItem {
                        input_index,
                        frame,
                        status: DownloadStatus::Failed,
                        output_uri: None,
                        error: fetched_item.error_details,
                    });
                    failed_in_order = policy == FetchErrorPolicy::Stop;
                }
                FetchStatus::Cancelled => {
                    output_items.push(DownloadItem {
                        input_index,
                        frame,
                        status: DownloadStatus::Cancelled,
                        output_uri: None,
                        error: Some(cancelled_error()),
                    });
                    failed_in_order = policy == FetchErrorPolicy::Stop;
                }
                FetchStatus::NotStarted => {
                    output_items.push(DownloadItem {
                        input_index,
                        frame,
                        status: DownloadStatus::NotStarted,
                        output_uri: None,
                        error: Some(not_started_error()),
                    });
                    failed_in_order = policy == FetchErrorPolicy::Stop;
                }
                FetchStatus::Success => {
                    let Some(raw) = fetched_item.raw else {
                        output_items.push(DownloadItem {
                            input_index,
                            frame,
                            status: DownloadStatus::Failed,
                            output_uri: None,
                            error: Some(internal_error()),
                        });
                        failed_in_order = policy == FetchErrorPolicy::Stop;
                        continue;
                    };
                    let frame = raw.frame.clone();
                    let backend = backend.clone();
                    let config = config.clone();
                    let cancellation = engine.cancellation_token();
                    let commit_result = match backend {
                        CommitBackend::Local(store) => {
                            engine
                                .run_commit(move || {
                                    commit_raw_frame(&store, raw, &config, overwrite, &cancellation)
                                })
                                .await
                        }
                        CommitBackend::Remote { store, uri } => {
                            engine
                                .run_remote_commit(move || async move {
                                    commit_raw_frame_remote(
                                        &store,
                                        raw,
                                        &config,
                                        overwrite,
                                        &cancellation,
                                        &uri,
                                    )
                                    .await
                                })
                                .await
                        }
                    };
                    match commit_result {
                        Ok((status, output_uri)) => output_items.push(DownloadItem {
                            input_index,
                            frame,
                            status: match status {
                                LocalCommitStatus::Written => DownloadStatus::Written,
                                LocalCommitStatus::Skipped => DownloadStatus::Skipped,
                            },
                            output_uri: Some(output_uri),
                            error: None,
                        }),
                        Err(CoreError::Cancelled) => {
                            output_items.push(DownloadItem {
                                input_index,
                                frame,
                                status: DownloadStatus::Cancelled,
                                output_uri: None,
                                error: Some(cancelled_error()),
                            });
                            failed_in_order = true;
                        }
                        Err(error) => {
                            output_items.push(DownloadItem {
                                input_index,
                                frame,
                                status: DownloadStatus::Failed,
                                output_uri: None,
                                error: Some(ErrorReport::from_core(&error, ErrorStage::Commit)),
                            });
                            failed_in_order = policy == FetchErrorPolicy::Stop;
                        }
                    }
                }
            }
        }
        offset = end;
        stop = failed_in_order || engine.is_cancelled();
    }

    if offset < frames.len() {
        for (relative, frame) in frames[offset..].iter().cloned().enumerate() {
            output_items.push(DownloadItem {
                input_index: offset + relative,
                frame,
                status: DownloadStatus::NotStarted,
                output_uri: None,
                error: Some(not_started_error()),
            });
        }
    }
    output_items.sort_by_key(|item| item.input_index);
    let mut report = DownloadBatchReport::from_items(output_items);
    report.interrupted = engine.is_cancelled();
    report
}

/// Decoded formats share one bounded prepare/commit path so stop, collect,
/// cancellation, and manifest publication keep the same ordering rules.
#[derive(Clone, Copy)]
pub enum DecodedOutputFormat {
    Png,
    Netcdf,
    Geotiff,
    Zarr,
}

impl DecodedOutputFormat {
    fn file_name(self) -> &'static str {
        match self {
            Self::Png => "decoded.png",
            Self::Netcdf => "decoded.nc",
            Self::Geotiff => "decoded.tif",
            Self::Zarr => "decoded.zarr",
        }
    }

    fn format_name(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Netcdf => "netcdf",
            Self::Geotiff => "geotiff",
            Self::Zarr => "zarr",
        }
    }
}

enum PreparedDecodedFrame {
    Ready(Box<ReadyDecodedFrame>),
    Finished(Box<DownloadItem>),
}

enum PreparedDecodedOutput {
    Ready(Box<RenderedDecodedFrame>),
    Finished(Box<DownloadItem>),
}

impl PreparedDecodedFrame {
    fn finished(item: DownloadItem) -> Self {
        Self::Finished(Box::new(item))
    }
}

struct ReadyDecodedFrame {
    input_index: usize,
    frame: FrameRef,
    revision: String,
    field: RadarField,
    raw: Option<Arc<RawFrame>>,
}

struct RenderedDecodedFrame {
    input_index: usize,
    frame: FrameRef,
    output: PreparedDecodedOutputFiles,
}

struct PreparedDecodedOutputFiles {
    // Keep the encoded files alive until LocalStore has copied them into its
    // transactional stage.
    _workspace: tempfile::TempDir,
    // Keep temporary raw artifacts alive until the atomic commit completes.
    _raw: Option<Arc<RawFrame>>,
    request: LocalCommitRequest,
    output_uri: String,
}

async fn prepare_decoded_frame(
    engine: &Engine,
    input_index: usize,
    fetched: FetchItem,
    processing: DecodedProcessing,
    include_raw: bool,
    cancellation: CancellationToken,
) -> PreparedDecodedFrame {
    let frame = fetched.frame;
    if cancellation.is_cancelled() {
        return PreparedDecodedFrame::finished(DownloadItem {
            input_index,
            frame,
            status: DownloadStatus::Cancelled,
            output_uri: None,
            error: Some(cancelled_after_failure()),
        });
    }
    match fetched.status {
        FetchStatus::Planned => PreparedDecodedFrame::finished(DownloadItem {
            input_index,
            frame,
            status: DownloadStatus::Planned,
            output_uri: None,
            error: None,
        }),
        FetchStatus::Failed => PreparedDecodedFrame::finished(DownloadItem {
            input_index,
            frame,
            status: DownloadStatus::Failed,
            output_uri: None,
            error: fetched.error_details,
        }),
        FetchStatus::Cancelled => PreparedDecodedFrame::finished(DownloadItem {
            input_index,
            frame,
            status: DownloadStatus::Cancelled,
            output_uri: None,
            error: Some(cancelled_error()),
        }),
        FetchStatus::NotStarted => PreparedDecodedFrame::finished(DownloadItem {
            input_index,
            frame,
            status: DownloadStatus::NotStarted,
            output_uri: None,
            error: Some(not_started_error()),
        }),
        FetchStatus::Success => {
            let Some(raw) = fetched.raw else {
                return PreparedDecodedFrame::finished(DownloadItem {
                    input_index,
                    frame,
                    status: DownloadStatus::Failed,
                    output_uri: None,
                    error: Some(internal_error()),
                });
            };
            let raw = Arc::new(raw);
            let frame = raw.frame.clone();
            let revision = match raw_revision(&raw) {
                Ok(revision) => revision,
                Err(error) => {
                    return PreparedDecodedFrame::finished(DownloadItem {
                        input_index,
                        frame,
                        status: DownloadStatus::Failed,
                        output_uri: None,
                        error: Some(ErrorReport::from_core(&error, ErrorStage::Decode)),
                    });
                }
            };
            let decoded = tokio::select! {
                biased;
                _ = cancellation.cancelled() => Err(EngineError::Core(CoreError::Cancelled)),
                result = engine.decode_science(raw.clone()) => result,
            };
            match decoded {
                Ok(mut field) => {
                    if let Some(requested) = processing.variable.as_deref()
                        && requested != field.name
                    {
                        let error = EngineError::UnsupportedVariable {
                            requested: requested.to_owned(),
                            available: field.name.clone(),
                        };
                        return PreparedDecodedFrame::finished(DownloadItem {
                            input_index,
                            frame,
                            status: DownloadStatus::Failed,
                            output_uri: None,
                            error: Some(engine_error_report(&error, ErrorStage::Decode)),
                        });
                    }
                    if let DecodedGrid::Geographic { bbox, resolution } = processing.grid {
                        let target = match geographic_target_grid(
                            bbox,
                            resolution,
                            &engine.resource_limits(),
                        ) {
                            Ok(target) => target,
                            Err(error) => {
                                return PreparedDecodedFrame::finished(DownloadItem {
                                    input_index,
                                    frame,
                                    status: DownloadStatus::Failed,
                                    output_uri: None,
                                    error: Some(ErrorReport::from_core(&error, ErrorStage::Regrid)),
                                });
                            }
                        };
                        let regridded = tokio::select! {
                            biased;
                            _ = cancellation.cancelled() => {
                                Err(EngineError::Core(CoreError::Cancelled))
                            }
                            result = engine.regrid(field, target, processing.resampling) => result,
                        };
                        field = match regridded {
                            Ok(field) => field,
                            Err(EngineError::Core(CoreError::Cancelled)) => {
                                return PreparedDecodedFrame::finished(DownloadItem {
                                    input_index,
                                    frame,
                                    status: DownloadStatus::Cancelled,
                                    output_uri: None,
                                    error: Some(cancelled_error()),
                                });
                            }
                            Err(error) => {
                                return PreparedDecodedFrame::finished(DownloadItem {
                                    input_index,
                                    frame,
                                    status: DownloadStatus::Failed,
                                    output_uri: None,
                                    error: Some(engine_error_report(&error, ErrorStage::Regrid)),
                                });
                            }
                        };
                    }
                    PreparedDecodedFrame::Ready(Box::new(ReadyDecodedFrame {
                        input_index,
                        frame,
                        revision,
                        field,
                        raw: include_raw.then_some(raw),
                    }))
                }
                Err(EngineError::Core(CoreError::Cancelled)) => {
                    PreparedDecodedFrame::finished(DownloadItem {
                        input_index,
                        frame,
                        status: DownloadStatus::Cancelled,
                        output_uri: None,
                        error: Some(cancelled_error()),
                    })
                }
                Err(error) => PreparedDecodedFrame::finished(DownloadItem {
                    input_index,
                    frame,
                    status: DownloadStatus::Failed,
                    output_uri: None,
                    error: Some(engine_error_report(&error, ErrorStage::Decode)),
                }),
            }
        }
    }
}

async fn prepare_decoded_output(
    engine: &Engine,
    prepared: PreparedDecodedFrame,
    output_root: std::path::PathBuf,
    options: DecodedCommitOptions,
    cancellation: CancellationToken,
) -> PreparedDecodedOutput {
    let prepared = match prepared {
        PreparedDecodedFrame::Ready(prepared) => prepared,
        PreparedDecodedFrame::Finished(item) => {
            return PreparedDecodedOutput::Finished(item);
        }
    };
    let ReadyDecodedFrame { input_index, frame, revision, field, raw } = *prepared;
    let worker = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(CoreError::Cancelled),
        permit = engine.acquire_decode_worker() => permit,
    };
    let permit = match worker {
        Ok(permit) => permit,
        Err(CoreError::Cancelled) => {
            return PreparedDecodedOutput::Finished(Box::new(DownloadItem {
                input_index,
                frame,
                status: DownloadStatus::Cancelled,
                output_uri: None,
                error: Some(cancelled_error()),
            }));
        }
        Err(error) => {
            return PreparedDecodedOutput::Finished(Box::new(DownloadItem {
                input_index,
                frame,
                status: DownloadStatus::Failed,
                output_uri: None,
                error: Some(ErrorReport::from_core(&error, ErrorStage::Commit)),
            }));
        }
    };
    let limits = engine.resource_limits();
    let frame_for_output = frame.clone();
    let output = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        prepare_decoded_output_files(
            &output_root,
            frame_for_output,
            &revision,
            &field,
            &limits,
            options,
            raw,
        )
    })
    .await;
    match output {
        Ok(Ok(output)) => PreparedDecodedOutput::Ready(Box::new(RenderedDecodedFrame {
            input_index,
            frame,
            output,
        })),
        Ok(Err(CoreError::Cancelled)) => PreparedDecodedOutput::Finished(Box::new(DownloadItem {
            input_index,
            frame,
            status: DownloadStatus::Cancelled,
            output_uri: None,
            error: Some(cancelled_error()),
        })),
        Ok(Err(error)) => PreparedDecodedOutput::Finished(Box::new(DownloadItem {
            input_index,
            frame,
            status: DownloadStatus::Failed,
            output_uri: None,
            error: Some(ErrorReport::from_core(&error, ErrorStage::Commit)),
        })),
        Err(_) => PreparedDecodedOutput::Finished(Box::new(DownloadItem {
            input_index,
            frame,
            status: DownloadStatus::Failed,
            output_uri: None,
            error: Some(internal_error()),
        })),
    }
}

/// Acquire validated scientific frames, render PNG plus sidecar, and commit
/// in input order through the manifest-last local store.
pub async fn download_png(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
) -> DownloadBatchReport {
    download_png_to(
        engine,
        frames,
        policy,
        dry_run,
        overwrite,
        engine.config().storage.output.clone(),
    )
    .await
}

pub async fn download_png_to(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
    output_root: PathBuf,
) -> DownloadBatchReport {
    download_decoded(
        engine,
        frames,
        policy,
        dry_run,
        overwrite,
        DecodedOutputFormat::Png,
        &output_root,
        None,
        DecodedProcessing::default(),
        false,
    )
    .await
}

/// Acquire, decode, encode, and commit NetCDF4 for validated scientific frames.
pub async fn download_netcdf(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
) -> DownloadBatchReport {
    download_netcdf_to(
        engine,
        frames,
        policy,
        dry_run,
        overwrite,
        engine.config().storage.output.clone(),
    )
    .await
}

pub async fn download_netcdf_to(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
    output_root: PathBuf,
) -> DownloadBatchReport {
    download_decoded(
        engine,
        frames,
        policy,
        dry_run,
        overwrite,
        DecodedOutputFormat::Netcdf,
        &output_root,
        None,
        DecodedProcessing::default(),
        false,
    )
    .await
}

/// Acquire, decode, encode, and commit consolidated Zarr v2 stores for
/// validated scientific frames.
pub async fn download_zarr(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
) -> DownloadBatchReport {
    download_zarr_to(
        engine,
        frames,
        policy,
        dry_run,
        overwrite,
        engine.config().storage.output.clone(),
    )
    .await
}

pub async fn download_zarr_to(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
    output_root: PathBuf,
) -> DownloadBatchReport {
    download_decoded(
        engine,
        frames,
        policy,
        dry_run,
        overwrite,
        DecodedOutputFormat::Zarr,
        &output_root,
        None,
        DecodedProcessing::default(),
        false,
    )
    .await
}

/// Acquire, decode, and commit float32/quality/provenance GeoTIFF groups for
/// validated scientific frames.
pub async fn download_geotiff(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
) -> DownloadBatchReport {
    download_geotiff_to(
        engine,
        frames,
        policy,
        dry_run,
        overwrite,
        engine.config().storage.output.clone(),
    )
    .await
}

pub async fn download_geotiff_to(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
    output_root: PathBuf,
) -> DownloadBatchReport {
    download_decoded(
        engine,
        frames,
        policy,
        dry_run,
        overwrite,
        DecodedOutputFormat::Geotiff,
        &output_root,
        None,
        DecodedProcessing::default(),
        false,
    )
    .await
}

pub async fn download_decoded_to_with_template(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
    output_root: PathBuf,
    format: DecodedOutputFormat,
    output_template: Option<String>,
) -> DownloadBatchReport {
    download_decoded_to_with_template_and_raw(
        engine,
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
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
    output_root: PathBuf,
    format: DecodedOutputFormat,
    output_template: Option<String>,
    include_raw: bool,
) -> DownloadBatchReport {
    download_decoded_to_with_processing_and_raw(
        engine,
        frames,
        policy,
        dry_run,
        overwrite,
        output_root,
        format,
        output_template,
        DecodedProcessing::default(),
        include_raw,
    )
    .await
}

pub async fn download_decoded_to_with_processing(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
    output_root: PathBuf,
    format: DecodedOutputFormat,
    output_template: Option<String>,
    processing: DecodedProcessing,
) -> DownloadBatchReport {
    download_decoded_to_with_processing_and_raw(
        engine,
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
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
    output_root: PathBuf,
    format: DecodedOutputFormat,
    output_template: Option<String>,
    processing: DecodedProcessing,
    include_raw: bool,
) -> DownloadBatchReport {
    download_decoded(
        engine,
        frames,
        policy,
        dry_run,
        overwrite,
        format,
        &output_root,
        output_template,
        processing,
        include_raw,
    )
    .await
}

async fn download_decoded(
    engine: &Engine,
    frames: Vec<FrameRef>,
    policy: FetchErrorPolicy,
    dry_run: bool,
    overwrite: bool,
    format: DecodedOutputFormat,
    output_root: &Path,
    output_template: Option<String>,
    processing: DecodedProcessing,
    include_raw: bool,
) -> DownloadBatchReport {
    if dry_run {
        return DownloadBatchReport::from_items(
            frames
                .into_iter()
                .enumerate()
                .map(|(input_index, frame)| DownloadItem {
                    input_index,
                    frame,
                    status: DownloadStatus::Planned,
                    output_uri: None,
                    error: None,
                })
                .collect(),
        );
    }
    if frames.is_empty() {
        return DownloadBatchReport::default();
    }

    let config = engine.config().clone();
    let backend = match create_commit_backend(engine, output_root) {
        Ok(backend) => backend,
        Err(error) => {
            let report = ErrorReport::from_core(&error, ErrorStage::Commit);
            return DownloadBatchReport::from_items(
                frames
                    .into_iter()
                    .enumerate()
                    .map(|(input_index, frame)| DownloadItem {
                        input_index,
                        frame,
                        status: DownloadStatus::Failed,
                        output_uri: None,
                        error: Some(report.clone()),
                    })
                    .collect(),
            );
        }
    };
    let staging_root = match backend.staging_root(engine) {
        Ok(root) => root,
        Err(error) => {
            let report = ErrorReport::from_core(&error, ErrorStage::Commit);
            return DownloadBatchReport::from_items(
                frames
                    .into_iter()
                    .enumerate()
                    .map(|(input_index, frame)| DownloadItem {
                        input_index,
                        frame,
                        status: DownloadStatus::Failed,
                        output_uri: None,
                        error: Some(report.clone()),
                    })
                    .collect(),
            );
        }
    };

    let concurrency = config.runtime.frame_concurrency.max(1);
    let mut output_items = Vec::with_capacity(frames.len());
    let mut offset = 0;
    let mut stop = false;
    while offset < frames.len() && !stop {
        if engine.is_cancelled() {
            break;
        }
        let end = (offset + concurrency).min(frames.len());
        let fetched =
            fetch_many_raw(engine, frames[offset..end].to_vec(), concurrency, policy, false).await;
        let mut failed_in_order = false;
        let preparation_cancellation = CancellationToken::new();
        let batch_offset = offset;
        let output_root = staging_root.clone();
        let commit_options = DecodedCommitOptions {
            format,
            overwrite,
            output_template: output_template.clone(),
            processing: processing.clone(),
            include_raw,
        };
        let mut prepared = futures_util::stream::iter(fetched.items.into_iter().enumerate().map(
            |(relative_index, item)| {
                let input_index = batch_offset + relative_index;
                let cancellation = preparation_cancellation.clone();
                let output_root = output_root.clone();
                let commit_options = commit_options.clone();
                let processing = processing.clone();
                async move {
                    let decoded = prepare_decoded_frame(
                        engine,
                        input_index,
                        item,
                        processing,
                        commit_options.include_raw,
                        cancellation.clone(),
                    )
                    .await;
                    prepare_decoded_output(
                        engine,
                        decoded,
                        output_root,
                        commit_options,
                        cancellation,
                    )
                    .await
                }
            },
        ))
        .buffered(concurrency);
        while let Some(prepared_output) = prepared.next().await {
            let (input_index, frame, output) = match prepared_output {
                PreparedDecodedOutput::Finished(item) => {
                    let item = *item;
                    if failed_in_order {
                        output_items.push(DownloadItem {
                            input_index: item.input_index,
                            frame: item.frame,
                            status: DownloadStatus::Cancelled,
                            output_uri: None,
                            error: Some(cancelled_after_failure()),
                        });
                        continue;
                    }
                    // Once an ordered stop condition is known, cancel the remaining
                    // preparations in this batch and publish only the ordered prefix.
                    failed_in_order = match item.status {
                        DownloadStatus::Failed => policy == FetchErrorPolicy::Stop,
                        DownloadStatus::Cancelled | DownloadStatus::NotStarted => true,
                        DownloadStatus::Planned
                        | DownloadStatus::Written
                        | DownloadStatus::Skipped => false,
                    };
                    if failed_in_order {
                        preparation_cancellation.cancel();
                    }
                    output_items.push(item);
                    continue;
                }
                PreparedDecodedOutput::Ready(prepared) => {
                    if failed_in_order && policy == FetchErrorPolicy::Stop {
                        output_items.push(DownloadItem {
                            input_index: prepared.input_index,
                            frame: prepared.frame,
                            status: DownloadStatus::Cancelled,
                            output_uri: None,
                            error: Some(cancelled_after_failure()),
                        });
                        continue;
                    }
                    (prepared.input_index, prepared.frame, prepared.output)
                }
            };
            let backend = backend.clone();
            let cancellation = engine.cancellation_token();
            let commit_result = match backend {
                CommitBackend::Local(store) => {
                    engine
                        .run_commit(move || {
                            commit_prepared_decoded_output(&store, output, &cancellation)
                        })
                        .await
                }
                CommitBackend::Remote { store, uri } => {
                    engine
                        .run_remote_commit(move || async move {
                            commit_prepared_decoded_output_remote(
                                &store,
                                output,
                                &cancellation,
                                &uri,
                            )
                            .await
                        })
                        .await
                }
            };
            let mut stop_preparing = false;
            match commit_result {
                Ok((status, output_uri)) => output_items.push(DownloadItem {
                    input_index,
                    frame,
                    status: match status {
                        LocalCommitStatus::Written => DownloadStatus::Written,
                        LocalCommitStatus::Skipped => DownloadStatus::Skipped,
                    },
                    output_uri: Some(output_uri),
                    error: None,
                }),
                Err(CoreError::Cancelled) => {
                    output_items.push(DownloadItem {
                        input_index,
                        frame,
                        status: DownloadStatus::Cancelled,
                        output_uri: None,
                        error: Some(cancelled_error()),
                    });
                    failed_in_order = true;
                    stop_preparing = true;
                }
                Err(error) => {
                    output_items.push(DownloadItem {
                        input_index,
                        frame,
                        status: DownloadStatus::Failed,
                        output_uri: None,
                        error: Some(ErrorReport::from_core(&error, ErrorStage::Commit)),
                    });
                    failed_in_order = policy == FetchErrorPolicy::Stop;
                    stop_preparing = failed_in_order;
                }
            }
            if stop_preparing {
                preparation_cancellation.cancel();
            }
        }
        drop(prepared);
        offset = end;
        stop = failed_in_order || engine.is_cancelled();
    }

    if offset < frames.len() {
        for (relative, frame) in frames[offset..].iter().cloned().enumerate() {
            output_items.push(DownloadItem {
                input_index: offset + relative,
                frame,
                status: DownloadStatus::NotStarted,
                output_uri: None,
                error: Some(not_started_error()),
            });
        }
    }
    output_items.sort_by_key(|item| item.input_index);
    let mut report = DownloadBatchReport::from_items(output_items);
    report.interrupted = engine.is_cancelled();
    report
}

#[derive(Clone)]
struct DecodedCommitOptions {
    format: DecodedOutputFormat,
    overwrite: bool,
    output_template: Option<String>,
    processing: DecodedProcessing,
    include_raw: bool,
}

fn render_output_template(
    template: &str,
    frame: &FrameRef,
    resolved_output_id: &str,
    spec: &ProcessingSpec,
) -> crate::errors::CoreResult<String> {
    let valid_time = parse_utc_time(&frame.valid_time)
        .map_err(|_| CoreError::Storage("output template frame time is invalid".into()))?;
    let mut valid_value = valid_time.format("%Y%m%dT%H%M%S").to_string();
    if valid_time.timestamp_subsec_micros() > 0 {
        valid_value.push_str(&format!("{:06}", valid_time.timestamp_subsec_micros()));
    }
    valid_value.push('Z');
    let base_value = frame
        .base_time
        .as_deref()
        .map(parse_utc_time)
        .transpose()
        .map_err(|_| CoreError::Storage("output template base time is invalid".into()))?
        .map(|time| format!("{}Z", time.format("%Y%m%dT%H%M%S")))
        .unwrap_or_default();
    let values = BTreeMap::from([
        ("source", frame.source.as_str().to_owned()),
        ("product", frame.product.as_str().to_owned()),
        ("station", frame.station.as_deref().unwrap_or("composite").to_owned()),
        ("valid_time", valid_value),
        ("base_time", base_value),
        ("date", valid_time.format("%Y-%m-%d").to_string()),
        ("hour", valid_time.format("%H").to_string()),
        ("variant_id", variant_id(resolved_output_id).to_owned()),
        (
            "ext",
            if spec.format == "netcdf" {
                "nc".to_owned()
            } else if spec.format == "geotiff" {
                "tif".to_owned()
            } else {
                spec.format.clone()
            },
        ),
    ]);
    let mut rendered = String::new();
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
                    field.push(next);
                }
                if !closed {
                    return Err(CoreError::Storage("unclosed output template field".into()));
                }
                let value = values.get(field.as_str()).ok_or_else(|| {
                    CoreError::Storage("unsupported output template field".into())
                })?;
                rendered.push_str(value);
            }
            '}' => return Err(CoreError::Storage("unmatched output template brace".into())),
            _ => rendered.push(character),
        }
    }
    if !is_safe_relative_path(&rendered) {
        return Err(CoreError::Storage("output template must stay within output root".into()));
    }
    Ok(rendered)
}

fn template_artifact_name(
    format: DecodedOutputFormat,
    output_leaf: &str,
    artifact_name: &str,
) -> String {
    let output_stem =
        Path::new(output_leaf).file_stem().and_then(|stem| stem.to_str()).unwrap_or(output_leaf);
    match format {
        DecodedOutputFormat::Png if artifact_name.ends_with(".render.json") => {
            format!("{output_stem}.render.json")
        }
        DecodedOutputFormat::Png | DecodedOutputFormat::Netcdf => output_leaf.to_owned(),
        DecodedOutputFormat::Geotiff => artifact_name
            .strip_prefix("decoded")
            .map(|suffix| format!("{output_stem}{suffix}"))
            .unwrap_or_else(|| artifact_name.to_owned()),
        DecodedOutputFormat::Zarr => artifact_name
            .strip_prefix("decoded.zarr")
            .map(|suffix| format!("{output_leaf}{suffix}"))
            .unwrap_or_else(|| artifact_name.to_owned()),
    }
}

#[cfg(test)]
fn commit_decoded_field(
    store: &LocalStore,
    frame: FrameRef,
    revision: &str,
    field: &RadarField,
    limits: &Limits,
    options: DecodedCommitOptions,
    cancellation: &tokio_util::sync::CancellationToken,
) -> crate::errors::CoreResult<(LocalCommitStatus, String)> {
    let prepared =
        prepare_decoded_output_files(store.root(), frame, revision, field, limits, options, None)?;
    commit_prepared_decoded_output(store, prepared, cancellation)
}

fn prepare_decoded_output_files(
    output_root: &std::path::Path,
    frame: FrameRef,
    revision: &str,
    field: &RadarField,
    limits: &Limits,
    options: DecodedCommitOptions,
    raw: Option<Arc<RawFrame>>,
) -> crate::errors::CoreResult<PreparedDecodedOutputFiles> {
    let DecodedCommitOptions { format, overwrite, output_template, processing, include_raw } =
        options;
    let frame_id = logical_id(&frame).map_err(|error| CoreError::Storage(error.to_string()))?;
    let processing_spec = ProcessingSpec {
        format: format.format_name().into(),
        variable: Some(field.name.clone()),
        grid: processing.grid.name().into(),
        bbox: processing.grid.bbox(),
        resolution: processing.grid.resolution(),
        resampling: processing.resampling.as_str().into(),
        ..ProcessingSpec::default()
    };
    let resolved_output_id = output_id(&frame, revision, &processing_spec)
        .map_err(|error| CoreError::Storage(error.to_string()))?;
    let output_name = match output_template.as_deref() {
        Some(template) => {
            render_output_template(template, &frame, &resolved_output_id, &processing_spec)?
        }
        None => format!("frames/{frame_id}/{}", format.file_name()),
    };
    let group = std::path::Path::new(&output_name)
        .parent()
        .map(|parent| parent.to_string_lossy().replace('\\', "/"))
        .filter(|parent| !parent.is_empty())
        .unwrap_or_else(|| {
            if output_template.is_some() { String::new() } else { format!("frames/{frame_id}") }
        });
    let output_leaf =
        std::path::Path::new(&output_name).file_name().and_then(|name| name.to_str()).ok_or_else(
            || CoreError::Storage("output template produced an invalid file name".into()),
        )?;
    let workspace = tempfile::Builder::new()
        .prefix(".radiust-render-")
        .tempdir_in(output_root)
        .map_err(|error| {
            let encoder = match format {
                DecodedOutputFormat::Png => "PNG",
                DecodedOutputFormat::Netcdf => "NetCDF",
                DecodedOutputFormat::Geotiff => "GeoTIFF",
                DecodedOutputFormat::Zarr => "Zarr",
            };
            CoreError::Temporary(format!("{encoder} staging failed: {error}"))
        })?;
    let primary_path = workspace.path().join(format.file_name());
    let paths = match format {
        DecodedOutputFormat::Png => {
            crate::output::png::write_png(field, &primary_path, &json!({}))?
                .into_iter()
                .map(|path| {
                    let name = path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .ok_or_else(|| CoreError::Storage("PNG artifact path is invalid".into()))?;
                    Ok((name.to_owned(), path))
                })
                .collect::<crate::errors::CoreResult<Vec<_>>>()?
        }
        DecodedOutputFormat::Netcdf => vec![(
            format.file_name().to_owned(),
            crate::output::netcdf::write_field(field, &primary_path, limits)?,
        )],
        DecodedOutputFormat::Geotiff => {
            crate::output::geotiff::write_field(field, &primary_path, limits)?
                .into_iter()
                .map(|path| {
                    let name =
                        path.file_name().and_then(|value| value.to_str()).ok_or_else(|| {
                            CoreError::Storage("GeoTIFF artifact path is invalid".into())
                        })?;
                    Ok((name.to_owned(), path))
                })
                .collect::<crate::errors::CoreResult<Vec<_>>>()?
        }
        DecodedOutputFormat::Zarr => {
            crate::output::zarr::write_field(field, &primary_path, limits)?;
            collect_zarr_files(&primary_path)?
                .into_iter()
                .map(|(relative_name, path)| {
                    (format!("{}/{}", format.file_name(), relative_name), path)
                })
                .collect()
        }
    };
    let mut artifacts = Vec::with_capacity(paths.len());
    let mut total_bytes = 0_u64;
    for (source_name, path) in paths {
        let name = if output_template.is_some() {
            template_artifact_name(format, output_leaf, &source_name)
        } else {
            source_name
        };
        let encoder = match format {
            DecodedOutputFormat::Png => "PNG",
            DecodedOutputFormat::Netcdf => "NetCDF",
            DecodedOutputFormat::Geotiff => "GeoTIFF",
            DecodedOutputFormat::Zarr => "Zarr",
        };
        let (role, media_type) = match format {
            DecodedOutputFormat::Png if name.ends_with(".render.json") => {
                ("metadata", "application/json")
            }
            DecodedOutputFormat::Png if name.ends_with(".png") => ("data", "image/png"),
            DecodedOutputFormat::Netcdf if name.ends_with(".nc") => {
                ("data", "application/x-netcdf")
            }
            DecodedOutputFormat::Geotiff if name.ends_with("_provenance.json") => {
                ("metadata", "application/json")
            }
            DecodedOutputFormat::Geotiff if name.ends_with(".tif") => ("data", "image/tiff"),
            DecodedOutputFormat::Geotiff => {
                return Err(CoreError::Storage(
                    "GeoTIFF encoder returned an unsupported artifact".into(),
                ));
            }
            DecodedOutputFormat::Zarr
                if name.rsplit('/').next().is_some_and(|part| part.starts_with(".z")) =>
            {
                ("metadata", "application/json")
            }
            DecodedOutputFormat::Zarr => ("data", "application/octet-stream"),
            DecodedOutputFormat::Png => {
                return Err(CoreError::Storage(
                    "PNG encoder returned an unsupported artifact".into(),
                ));
            }
            DecodedOutputFormat::Netcdf => {
                return Err(CoreError::Storage(
                    "NetCDF encoder returned an unsupported artifact".into(),
                ));
            }
        };
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            CoreError::Temporary(format!("{encoder} encoder output could not be inspected"))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(CoreError::Storage(format!(
                "{encoder} encoder output is not a regular file"
            )));
        }
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| CoreError::ResourceLimit(format!("{encoder} output size overflow")))?;
        limits.validate_bytes(metadata.len(), total_bytes)?;
        // The commit copies encoded files into its own root-local transaction
        // stage while this render workspace is still live.
        if total_bytes > limits.max_temp_bytes / 3 {
            let message = match format {
                DecodedOutputFormat::Png => {
                    "PNG output and transactional copies exceed the temporary-storage budget"
                }
                DecodedOutputFormat::Netcdf => {
                    "NetCDF output and transactional copies exceed the temporary-storage budget"
                }
                DecodedOutputFormat::Geotiff => {
                    "GeoTIFF output and transactional copies exceed the temporary-storage budget"
                }
                DecodedOutputFormat::Zarr => {
                    "Zarr output and transactional copies exceed the temporary-storage budget"
                }
            };
            return Err(CoreError::ResourceLimit(message.into()));
        }
        let relative_uri = if group.is_empty() { name.clone() } else { format!("{group}/{name}") };
        artifacts.push(StagedArtifact {
            name: name.clone(),
            relative_uri,
            role: role.into(),
            media_type: media_type.into(),
            source: path,
        });
    }
    if include_raw {
        let raw_frame = raw.as_deref().ok_or_else(|| {
            CoreError::Storage("raw+decoded output is missing its acquired raw frame".into())
        })?;
        append_raw_artifacts(
            workspace.path(),
            &frame_id,
            raw_frame,
            limits,
            &mut artifacts,
            &mut total_bytes,
        )?;
    }
    let request = LocalCommitRequest {
        frame,
        revision: revision.to_owned(),
        processing_spec,
        output_name: output_name.clone(),
        artifacts,
        raw_complete: include_raw,
        overwrite,
    };
    Ok(PreparedDecodedOutputFiles {
        _workspace: workspace,
        _raw: raw,
        request,
        output_uri: output_root.join(output_name).display().to_string(),
    })
}

fn append_raw_artifacts(
    workspace: &Path,
    frame_id: &str,
    raw: &RawFrame,
    limits: &Limits,
    artifacts: &mut Vec<StagedArtifact>,
    total_bytes: &mut u64,
) -> crate::errors::CoreResult<()> {
    let raw_manifest_path = workspace.join("raw-manifest.json");
    let mut raw_artifacts = raw
        .artifacts
        .iter()
        .map(|artifact| {
            json!({
                "name": artifact.receipt.name,
                "role": "data",
                "media_type": artifact.receipt.media_type,
                "size_bytes": artifact.receipt.size_bytes,
                "sha256": artifact.receipt.sha256,
                "source_revision": null,
            })
        })
        .collect::<Vec<_>>();
    raw_artifacts.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    let raw_manifest = json!({
        "schema_version": 1,
        "ref": safe_ref(&raw.frame).map_err(|error| CoreError::Storage(error.to_string()))?,
        "artifacts": raw_artifacts,
        "metadata": {},
        "raw_complete": true,
    });
    fs::write(
        &raw_manifest_path,
        serde_json::to_vec_pretty(&raw_manifest)
            .map_err(|_| CoreError::Storage("raw manifest could not be serialized".into()))?,
    )
    .map_err(|error| CoreError::Temporary(format!("raw manifest staging failed: {error}")))?;
    let manifest_metadata = fs::symlink_metadata(&raw_manifest_path)
        .map_err(|_| CoreError::Temporary("raw manifest staging could not be inspected".into()))?;
    if !manifest_metadata.is_file() || manifest_metadata.file_type().is_symlink() {
        return Err(CoreError::Storage("raw manifest staging is not a regular file".into()));
    }
    *total_bytes = total_bytes
        .checked_add(manifest_metadata.len())
        .ok_or_else(|| CoreError::ResourceLimit("raw output size overflow".into()))?;
    limits.validate_bytes(manifest_metadata.len(), *total_bytes)?;
    artifacts.push(StagedArtifact {
        name: "raw-manifest.json".into(),
        relative_uri: format!("frames/{frame_id}/raw-manifest.json"),
        role: "metadata".into(),
        media_type: "application/json".into(),
        source: raw_manifest_path,
    });
    for artifact in &raw.artifacts {
        let size = artifact.receipt.size_bytes;
        *total_bytes = total_bytes
            .checked_add(size)
            .ok_or_else(|| CoreError::ResourceLimit("raw output size overflow".into()))?;
        limits.validate_bytes(size, *total_bytes)?;
        artifacts.push(StagedArtifact {
            name: format!("raw/{}", artifact.receipt.name),
            relative_uri: format!("frames/{frame_id}/raw/{}", artifact.receipt.name),
            role: "data".into(),
            media_type: artifact.receipt.media_type.clone(),
            source: artifact.path.to_path_buf(),
        });
    }
    if *total_bytes > limits.max_temp_bytes / 3 {
        return Err(CoreError::ResourceLimit(
            "raw and decoded output plus transactional copies exceed the temporary-storage budget"
                .into(),
        ));
    }
    Ok(())
}

fn commit_prepared_decoded_output(
    store: &LocalStore,
    prepared: PreparedDecodedOutputFiles,
    cancellation: &tokio_util::sync::CancellationToken,
) -> crate::errors::CoreResult<(LocalCommitStatus, String)> {
    let PreparedDecodedOutputFiles { _workspace, _raw, request, output_uri } = prepared;
    let result = store.commit_cancellable(request, cancellation)?;
    drop((_workspace, _raw));
    Ok((result.status, output_uri))
}

async fn commit_prepared_decoded_output_remote(
    store: &RemoteStore,
    prepared: PreparedDecodedOutputFiles,
    cancellation: &CancellationToken,
    remote_uri: &str,
) -> crate::errors::CoreResult<(LocalCommitStatus, String)> {
    let PreparedDecodedOutputFiles { _workspace, _raw, request, .. } = prepared;
    let output_uri = remote_pointer_uri(remote_uri, &request.output_name);
    let result = store.commit_cancellable(request, cancellation.clone()).await?;
    drop((_workspace, _raw));
    Ok((result.status, output_uri))
}

fn collect_zarr_files(
    root: &std::path::Path,
) -> crate::errors::CoreResult<Vec<(String, std::path::PathBuf)>> {
    fn visit(
        root: &std::path::Path,
        directory: &std::path::Path,
        files: &mut Vec<(String, std::path::PathBuf)>,
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
                    .map_err(|_| CoreError::Storage("Zarr artifact is outside its store".into()))?;
                let name = relative
                    .components()
                    .map(|component| component.as_os_str().to_str())
                    .collect::<Option<Vec<_>>>()
                    .filter(|components| !components.is_empty())
                    .map(|components| components.join("/"))
                    .ok_or_else(|| CoreError::Storage("Zarr artifact path is invalid".into()))?;
                files.push((name, path));
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

fn engine_error_report(error: &EngineError, stage: ErrorStage) -> ErrorReport {
    match error {
        EngineError::Core(error) => ErrorReport::from_core(error, stage),
        EngineError::Query(_) | EngineError::InvalidQuery(_) => ErrorReport {
            code: ErrorCode::InvalidQuery,
            message: "query parameters are invalid".into(),
            stage: ErrorStage::Validate,
            retryable: false,
        },
        EngineError::UnsupportedScience(_) | EngineError::UnsupportedSource(_) => ErrorReport {
            code: ErrorCode::Unsupported,
            message: "validated scientific decoding is unavailable for this source/product".into(),
            stage,
            retryable: false,
        },
        EngineError::UnsupportedVariable { .. } => ErrorReport {
            code: ErrorCode::Unsupported,
            message: "requested variable is unavailable for the decoded field".into(),
            stage: ErrorStage::Decode,
            retryable: false,
        },
        EngineError::UnsupportedRegrid { source_crs, target_crs } => ErrorReport {
            code: ErrorCode::Unsupported,
            message: format!(
                "coordinate transformation from {source_crs} to {target_crs} is unsupported; no verified transform is available"
            ),
            stage: ErrorStage::Regrid,
            retryable: false,
        },
        _ => ErrorReport {
            code: ErrorCode::Internal,
            message: "scientific decoding failed".into(),
            stage,
            retryable: false,
        },
    }
}

fn commit_raw_frame(
    store: &LocalStore,
    raw: RawFrame,
    config: &crate::config::CoreConfig,
    overwrite: bool,
    cancellation: &tokio_util::sync::CancellationToken,
) -> crate::errors::CoreResult<(LocalCommitStatus, String)> {
    let PreparedRawCommit { _raw, _workspace, request } =
        prepare_raw_commit(raw, config, overwrite)?;
    let output_name = request.output_name.clone();
    let result = store.commit_cancellable(request, cancellation)?;
    let output_uri = store.root().join(output_name).display().to_string();
    drop((_raw, _workspace));
    Ok((result.status, output_uri))
}

async fn commit_raw_frame_remote(
    store: &RemoteStore,
    raw: RawFrame,
    config: &crate::config::CoreConfig,
    overwrite: bool,
    cancellation: &CancellationToken,
    remote_uri: &str,
) -> crate::errors::CoreResult<(LocalCommitStatus, String)> {
    let PreparedRawCommit { _raw, _workspace, request } =
        prepare_raw_commit(raw, config, overwrite)?;
    let output_name = request.output_name.clone();
    let result = store.commit_cancellable(request, cancellation.clone()).await?;
    let output_uri = remote_pointer_uri(remote_uri, &output_name);
    drop((_raw, _workspace));
    Ok((result.status, output_uri))
}

struct PreparedRawCommit {
    _raw: RawFrame,
    _workspace: tempfile::TempDir,
    request: LocalCommitRequest,
}

fn prepare_raw_commit(
    raw: RawFrame,
    config: &crate::config::CoreConfig,
    overwrite: bool,
) -> crate::errors::CoreResult<PreparedRawCommit> {
    let frame_id = raw.frame.logical_id.clone();
    let group = format!("frames/{frame_id}");
    let output_name = format!("{group}/raw-manifest.json");
    let workspace = tempfile::tempdir()
        .map_err(|error| CoreError::Temporary(format!("raw manifest staging failed: {error}")))?;
    let reference = safe_ref(&raw.frame).map_err(|error| CoreError::Storage(error.to_string()))?;
    let mut raw_artifacts = raw
        .artifacts
        .iter()
        .map(|artifact| {
            json!({
                "name": artifact.receipt.name,
                "role": "data",
                "media_type": artifact.receipt.media_type,
                "size_bytes": artifact.receipt.size_bytes,
                "sha256": artifact.receipt.sha256,
                "source_revision": null,
            })
        })
        .collect::<Vec<_>>();
    raw_artifacts.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    let raw_manifest = json!({
        "schema_version": 1,
        "ref": reference,
        "artifacts": raw_artifacts,
        "metadata": {},
        "raw_complete": true,
    });
    let raw_manifest_path = workspace.path().join("raw-manifest.json");
    fs::write(
        &raw_manifest_path,
        serde_json::to_vec_pretty(&raw_manifest)
            .map_err(|_| CoreError::Storage("raw manifest could not be serialized".into()))?,
    )
    .map_err(|error| CoreError::Temporary(format!("raw manifest staging failed: {error}")))?;

    let mut artifacts = vec![StagedArtifact {
        name: "raw-manifest.json".into(),
        relative_uri: output_name.clone(),
        role: "metadata".into(),
        media_type: "application/json".into(),
        source: raw_manifest_path,
    }];
    for artifact in &raw.artifacts {
        let relative_uri = format!("{group}/raw/{}", artifact.receipt.name);
        artifacts.push(StagedArtifact {
            name: relative_uri.clone(),
            relative_uri,
            role: "data".into(),
            media_type: artifact.receipt.media_type.clone(),
            source: artifact.path.to_path_buf(),
        });
    }
    let revision = raw_revision(&raw)?;
    let spec = ProcessingSpec {
        output_kind: "raw-only".into(),
        format: config.output.format.clone(),
        ..ProcessingSpec::default()
    };
    let request = LocalCommitRequest {
        frame: raw.frame.clone(),
        revision,
        processing_spec: spec,
        output_name: output_name.clone(),
        artifacts,
        raw_complete: true,
        overwrite,
    };
    Ok(PreparedRawCommit { _raw: raw, _workspace: workspace, request })
}

fn raw_revision(raw: &RawFrame) -> crate::errors::CoreResult<String> {
    if let Some(revision) = raw.frame.revision.as_deref().filter(|value| !value.is_empty()) {
        return digest(&json!({"namespace": "upstream", "revision": revision}))
            .map_err(|error| CoreError::Storage(error.to_string()));
    }
    let mut artifacts = raw
        .artifacts
        .iter()
        .map(|artifact| {
            json!({
                "name": artifact.receipt.name,
                "size_bytes": artifact.receipt.size_bytes,
                "sha256": artifact.receipt.sha256,
                "source_revision": null,
            })
        })
        .collect::<Vec<_>>();
    artifacts.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    digest(&json!({"identity_schema": 1, "artifacts": artifacts}))
        .map_err(|error| CoreError::Storage(error.to_string()))
}

fn cancelled_error() -> ErrorReport {
    ErrorReport {
        code: ErrorCode::Cancelled,
        message: "operation cancelled".into(),
        stage: ErrorStage::Acquire,
        retryable: false,
    }
}

fn cancelled_after_failure() -> ErrorReport {
    ErrorReport {
        code: ErrorCode::Cancelled,
        message: "not committed after an earlier failure".into(),
        stage: ErrorStage::Commit,
        retryable: false,
    }
}

fn not_started_error() -> ErrorReport {
    ErrorReport {
        code: ErrorCode::Cancelled,
        message: "not started".into(),
        stage: ErrorStage::Acquire,
        retryable: false,
    }
}

fn internal_error() -> ErrorReport {
    ErrorReport {
        code: ErrorCode::Internal,
        message: "download operation failed".into(),
        stage: ErrorStage::Commit,
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CoreConfig;
    use crate::identity::logical_id;
    use crate::model::{ArtifactReceipt, DiscoveryTarget, RawArtifact};
    use crate::source::{SourceAdapter, SourceContext, SourceRegistry};
    use futures_util::future::BoxFuture;
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    #[test]
    fn remote_output_uris_accept_only_safe_s3_and_oss_targets() {
        let target = parse_remote_output_target("s3://radar-bucket/archive/2026").unwrap().unwrap();
        assert_eq!(target.provider, "s3");
        assert_eq!(target.bucket, "radar-bucket");
        assert_eq!(target.prefix, "archive/2026");
        assert_eq!(target.uri, "s3://radar-bucket/archive/2026");

        let root = parse_remote_output_target("oss://radar-bucket/").unwrap().unwrap();
        assert_eq!(root.provider, "oss");
        assert!(root.prefix.is_empty());
        assert_eq!(root.uri, "oss://radar-bucket");
        assert!(parse_remote_output_target("./local-output").unwrap().is_none());
        for invalid in [
            "https://bucket.example.invalid/output",
            "s3://user:secret@bucket/output",
            "s3://bucket/output?token=secret",
            "s3://bucket/output#fragment",
            "s3:///output",
        ] {
            assert!(parse_remote_output_target(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn invalid_engine_queries_map_to_safe_non_retryable_validation_errors() {
        let error = EngineError::InvalidQuery("private query detail");
        let report = engine_error_report(&error, ErrorStage::Decode);

        assert_eq!(report.code, ErrorCode::InvalidQuery);
        assert_eq!(report.stage, ErrorStage::Validate);
        assert!(!report.retryable);
        assert!(!report.message.contains("private query detail"));
    }

    struct ReplayAdapter;

    impl SourceAdapter for ReplayAdapter {
        fn source_id(&self) -> &'static str {
            "rainviewer"
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
            frame: FrameRef,
            _context: SourceContext,
            temp_root: PathBuf,
        ) -> Option<BoxFuture<'static, crate::errors::CoreResult<RawFrame>>> {
            Some(Box::pin(async move {
                if frame.valid_time.ends_with("01:00:00Z") {
                    return Err(CoreError::Transport("fixture failure".into()));
                }
                fs::create_dir_all(&temp_root)
                    .map_err(|error| CoreError::Temporary(error.to_string()))?;
                let content = format!("raw:{}", frame.valid_time).into_bytes();
                let sha256 = hex::encode(Sha256::digest(&content));
                let path = temp_root.join(format!("{}.bin", frame.logical_id));
                fs::write(&path, &content)
                    .map_err(|error| CoreError::Temporary(error.to_string()))?;
                let path = tempfile::TempPath::try_from_path(path)
                    .map_err(|_| CoreError::Temporary("fixture temp path failed".into()))?;
                Ok(RawFrame {
                    frame,
                    artifacts: vec![RawArtifact {
                        receipt: ArtifactReceipt {
                            name: "source.bin".into(),
                            media_type: "application/octet-stream".into(),
                            size_bytes: content.len() as u64,
                            sha256,
                        },
                        path,
                    }],
                    private_locator: None,
                })
            }))
        }
    }

    struct StreamDropSignal(Arc<std::sync::atomic::AtomicUsize>);

    impl Drop for StreamDropSignal {
        fn drop(&mut self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct BlockingStreamAdapter {
        started: tokio::sync::mpsc::UnboundedSender<()>,
        drops: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl SourceAdapter for BlockingStreamAdapter {
        fn source_id(&self) -> &'static str {
            "rainviewer"
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
            _frame: FrameRef,
            _context: SourceContext,
            _temp_root: PathBuf,
        ) -> Option<BoxFuture<'static, crate::errors::CoreResult<RawFrame>>> {
            let started = self.started.clone();
            let drops = self.drops.clone();
            Some(Box::pin(async move {
                let _signal = StreamDropSignal(drops);
                let _ = started.send(());
                futures_util::future::pending::<crate::errors::CoreResult<RawFrame>>().await
            }))
        }
    }

    struct StreamActivityGuard(Arc<std::sync::atomic::AtomicUsize>);

    impl Drop for StreamActivityGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct ConcurrentStreamAdapter {
        started: tokio::sync::mpsc::UnboundedSender<u8>,
        active: Arc<std::sync::atomic::AtomicUsize>,
        max_active: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl SourceAdapter for ConcurrentStreamAdapter {
        fn source_id(&self) -> &'static str {
            "rainviewer"
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
            frame: FrameRef,
            _context: SourceContext,
            _temp_root: PathBuf,
        ) -> Option<BoxFuture<'static, crate::errors::CoreResult<RawFrame>>> {
            let hour =
                frame.valid_time.get(11..13).and_then(|value| value.parse().ok()).unwrap_or(0);
            let delay_ms = match hour {
                0 => 300,
                1 => 20,
                _ => 60,
            };
            let started = self.started.clone();
            let active = self.active.clone();
            let max_active = self.max_active.clone();
            Some(Box::pin(async move {
                let current = active.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                max_active.fetch_max(current, std::sync::atomic::Ordering::SeqCst);
                let _activity = StreamActivityGuard(active);
                let _ = started.send(hour);
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                Err(CoreError::Transport("stream fixture failure".into()))
            }))
        }
    }

    struct RainViewerFixtureAdapter;

    impl SourceAdapter for RainViewerFixtureAdapter {
        fn source_id(&self) -> &'static str {
            "rainviewer"
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
            frame: FrameRef,
            _context: SourceContext,
            temp_root: PathBuf,
        ) -> Option<BoxFuture<'static, crate::errors::CoreResult<RawFrame>>> {
            Some(Box::pin(async move {
                let corrupt_fixture =
                    frame.locator.get("fixture_corrupt").and_then(serde_json::Value::as_bool)
                        == Some(true);
                const TILES: [(&str, &[u8]); 4] = [
                    (
                        "tile-z1-x0-y0.png",
                        include_bytes!(
                            "../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x0-y0.png"
                        ),
                    ),
                    (
                        "tile-z1-x1-y0.png",
                        include_bytes!(
                            "../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x1-y0.png"
                        ),
                    ),
                    (
                        "tile-z1-x0-y1.png",
                        include_bytes!(
                            "../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x0-y1.png"
                        ),
                    ),
                    (
                        "tile-z1-x1-y1.png",
                        include_bytes!(
                            "../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x1-y1.png"
                        ),
                    ),
                ];
                fs::create_dir_all(&temp_root)
                    .map_err(|error| CoreError::Temporary(error.to_string()))?;
                let mut artifacts = Vec::with_capacity(TILES.len());
                for (name, content) in TILES {
                    let content: &[u8] = if corrupt_fixture { b"invalid tile" } else { content };
                    let path = temp_root.join(format!("{}-{name}", uuid::Uuid::new_v4()));
                    fs::write(&path, content)
                        .map_err(|error| CoreError::Temporary(error.to_string()))?;
                    let path = tempfile::TempPath::try_from_path(path)
                        .map_err(|_| CoreError::Temporary("fixture temp path failed".into()))?;
                    artifacts.push(RawArtifact {
                        receipt: ArtifactReceipt {
                            name: name.into(),
                            media_type: "image/png".into(),
                            size_bytes: content.len() as u64,
                            sha256: hex::encode(Sha256::digest(content)),
                        },
                        path,
                    });
                }
                Ok(RawFrame { frame, artifacts, private_locator: None })
            }))
        }
    }

    fn fixture_frame(hour: u8) -> FrameRef {
        let mut frame = FrameRef {
            source: "rainviewer".into(),
            product: "composite".into(),
            station: None,
            valid_time: format!("2025-01-01T{hour:02}:00:00Z"),
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "fixture-v1".into(),
            locator: json!({"fixture": true}),
        };
        frame.logical_id = logical_id(&frame).unwrap();
        frame
    }

    fn replay_engine(root: &Path, frame_concurrency: usize) -> Engine {
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.frame_concurrency = frame_concurrency;
        config.storage.output = root.join("output");
        config.runtime.temp_root = Some(root.join("temp"));
        config.cache.dir = root.join("cache");
        let mut overrides = SourceRegistry::default();
        overrides.register(Arc::new(ReplayAdapter)).unwrap();
        Engine::new(config, overrides).unwrap()
    }

    #[tokio::test]
    async fn temporary_storage_failure_keeps_its_category_in_raw_and_decoded_downloads() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("temp"), b"not a directory").unwrap();
        let engine = replay_engine(root.path(), 1);
        let frame = fixture_frame(0);
        let raw = engine
            .download_raw_only(vec![frame.clone()], FetchErrorPolicy::Collect, false, false)
            .await;
        let decoded =
            engine.download_png(vec![frame], FetchErrorPolicy::Collect, false, false).await;
        for report in [raw, decoded] {
            assert_eq!(report.failed, 1);
            let error = report.items[0].error.as_ref().unwrap();
            assert_eq!(error.code, ErrorCode::Storage);
            assert_eq!(error.stage, ErrorStage::Acquire);
            assert!(!error.retryable);
            assert_eq!(error.message, "temporary data operation failed");
            assert!(!error.message.contains(&root.path().display().to_string()));
        }
    }

    fn synthetic_fetch_stream(
        count: u8,
        policy: FetchErrorPolicy,
        concurrency: usize,
    ) -> DecodedFetchStream {
        let engine =
            Arc::new(Engine::new(CoreConfig::default(), SourceRegistry::default()).unwrap());
        DecodedFetchStream::new(
            engine,
            (0..count).map(fixture_frame).collect(),
            concurrency,
            policy,
        )
    }

    fn synthetic_decoded_item(
        stream: &DecodedFetchStream,
        index: usize,
        status: FetchStatus,
    ) -> DecodedFetchItem {
        DecodedFetchItem {
            input_index: index,
            frame: stream.frames[index].clone(),
            status,
            data: None,
            error: (status == FetchStatus::Failed).then(|| "fixture failure".into()),
            error_details: None,
        }
    }

    fn push_delayed_item(
        stream: &mut DecodedFetchStream,
        index: usize,
        delay_ms: u64,
        status: FetchStatus,
    ) {
        let item = synthetic_decoded_item(stream, index, status);
        stream.in_flight.push(Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            item
        }));
    }

    #[tokio::test]
    async fn decoded_fetch_stream_yields_in_completion_order() {
        let mut stream = synthetic_fetch_stream(3, FetchErrorPolicy::Collect, 3);
        stream.next_index = 3;
        push_delayed_item(&mut stream, 0, 80, FetchStatus::Success);
        push_delayed_item(&mut stream, 1, 10, FetchStatus::Success);
        push_delayed_item(&mut stream, 2, 40, FetchStatus::Success);

        let mut order = Vec::new();
        while let Some(item) = stream.next().await.unwrap() {
            order.push(item.input_index);
        }

        assert_eq!(order, [1, 2, 0]);
    }

    #[tokio::test]
    async fn decoded_fetch_stream_refills_prefetch_window_before_yielding() {
        let mut stream = synthetic_fetch_stream(3, FetchErrorPolicy::Collect, 2);
        stream.next_index = 2;
        push_delayed_item(&mut stream, 0, 10, FetchStatus::Success);
        push_delayed_item(&mut stream, 1, 100, FetchStatus::Success);

        let item = stream.next().await.unwrap().unwrap();

        assert_eq!(item.input_index, 0);
        assert_eq!(stream.next_index, 3);
        assert_eq!(stream.in_flight.len(), 2);
        stream.close();
    }

    #[tokio::test]
    async fn decoded_fetch_stream_runs_bounded_workers_and_refills_before_returning() {
        let (started, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let max_active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut sources = SourceRegistry::default();
        sources
            .register(Arc::new(ConcurrentStreamAdapter {
                started,
                active: active.clone(),
                max_active: max_active.clone(),
            }))
            .unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.cache.enabled = false;
        let engine = Arc::new(Engine::new(config, sources).unwrap());
        let mut stream = DecodedFetchStream::new(
            engine,
            vec![fixture_frame(0), fixture_frame(1), fixture_frame(2)],
            2,
            FetchErrorPolicy::Collect,
        );

        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.input_index, 1);
        let mut launched = Vec::new();
        for _ in 0..3 {
            launched.push(
                tokio::time::timeout(std::time::Duration::from_secs(1), started_rx.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        launched.sort_unstable();
        assert_eq!(launched, [0, 1, 2]);

        assert_eq!(stream.next().await.unwrap().unwrap().input_index, 2);
        assert_eq!(stream.next().await.unwrap().unwrap().input_index, 0);
        assert_eq!(max_active.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn decoded_fetch_stream_collect_does_not_retain_emitted_items() {
        let mut stream = synthetic_fetch_stream(1, FetchErrorPolicy::Collect, 1);
        stream.next_index = 1;
        push_delayed_item(&mut stream, 0, 10, FetchStatus::Success);

        let item = stream.next().await.unwrap().unwrap();

        assert_eq!(item.status, FetchStatus::Success);
        assert!(stream.emitted.iter().all(Option::is_none));
        stream.close();
    }

    #[tokio::test]
    async fn decoded_fetch_stream_raise_builds_ordered_partial_result() {
        let mut stream = synthetic_fetch_stream(4, FetchErrorPolicy::Stop, 2);
        stream.next_index = 2;
        push_delayed_item(&mut stream, 0, 80, FetchStatus::Success);
        push_delayed_item(&mut stream, 1, 10, FetchStatus::Failed);

        let failure = stream.next().await.unwrap_err();

        assert_eq!(failure.cause, "fixture failure");
        assert_eq!(
            failure.partial_result.items.iter().map(|item| item.status).collect::<Vec<_>>(),
            [
                FetchStatus::Cancelled,
                FetchStatus::Failed,
                FetchStatus::NotStarted,
                FetchStatus::NotStarted,
            ]
        );
        assert!(failure.partial_result.items.iter().all(|item| item.data.is_none()));
    }

    #[tokio::test]
    async fn decoded_fetch_stream_raise_retains_previously_emitted_successes() {
        let mut stream = synthetic_fetch_stream(2, FetchErrorPolicy::Stop, 2);
        stream.next_index = 2;
        push_delayed_item(&mut stream, 0, 10, FetchStatus::Success);
        push_delayed_item(&mut stream, 1, 30, FetchStatus::Failed);

        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.input_index, 0);
        let failure = stream.next().await.unwrap_err();

        assert_eq!(
            failure.partial_result.items.iter().map(|item| item.status).collect::<Vec<_>>(),
            [FetchStatus::Success, FetchStatus::Failed]
        );
        assert!(failure.partial_result.items.iter().all(|item| item.data.is_none()));
    }

    #[tokio::test]
    async fn decoded_fetch_stream_cancellation_drops_pending_work() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (started, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let drops = Arc::new(AtomicUsize::new(0));
        let mut sources = SourceRegistry::default();
        sources
            .register(Arc::new(BlockingStreamAdapter { started, drops: drops.clone() }))
            .unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.cache.enabled = false;
        let engine = Arc::new(Engine::new(config, sources).unwrap());
        let mut stream =
            DecodedFetchStream::new(engine, vec![fixture_frame(0)], 1, FetchErrorPolicy::Collect);
        let cancellation = stream.cancellation_token();
        let next = tokio::spawn(async move { stream.next().await });
        tokio::time::timeout(std::time::Duration::from_secs(1), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        cancellation.cancel();

        assert!(next.await.unwrap().unwrap().is_none());
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while drops.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    fn rainviewer_fixture_frame() -> FrameRef {
        let mut frame = FrameRef {
            source: "rainviewer".into(),
            product: "composite".into(),
            station: None,
            valid_time: "2026-09-18T02:00:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: Some("7cc4a10f8d53".into()),
            locator_version: "rainviewer-v2".into(),
            locator: json!({
                "api_url": "https://api.rainviewer.com/public/weather-maps.json",
                "host": "https://tilecache.rainviewer.com",
                "path": "/v2/radar/7cc4a10f8d53",
                "tile_size": 512,
                "zoom": 1,
                "color": 2,
                "options": "0_0",
            }),
        };
        frame.logical_id = logical_id(&frame).unwrap();
        frame
    }

    fn png_field() -> RadarField {
        RadarField {
            name: "reflectivity".into(),
            values: vec![0.0, 10.0, 20.0, f32::NAN],
            shape: vec![2, 2],
            quality: vec![0, 0, 1, 1],
            units: Some("dBZ".into()),
            valid_time: "2025-01-01T00:00:00Z".into(),
            grid: crate::model::Grid {
                shape: vec![2, 2],
                crs: Some("EPSG:4326".into()),
                x: vec![0.0, 1.0],
                y: vec![-1.0, 1.0],
                affine: None,
            },
            provenance: vec!["source=fixture".into(), "product=composite".into()],
        }
    }

    #[test]
    fn png_commit_publishes_verified_v1_manifest_and_skips_matching_output() {
        let root = tempfile::tempdir().unwrap();
        let store = LocalStore::new(root.path(), Limits::default()).unwrap();
        let frame = fixture_frame(0);
        let field = png_field();
        let revision = hex::encode(Sha256::digest(b"fixture-revision"));
        let cancellation = tokio_util::sync::CancellationToken::new();

        let (first_status, output_uri) = commit_decoded_field(
            &store,
            frame.clone(),
            &revision,
            &field,
            &Limits::default(),
            DecodedCommitOptions {
                format: DecodedOutputFormat::Png,
                overwrite: false,
                output_template: None,
                processing: DecodedProcessing::default(),
                include_raw: false,
            },
            &cancellation,
        )
        .unwrap();
        assert_eq!(first_status, LocalCommitStatus::Written);
        let output = std::path::PathBuf::from(output_uri);
        assert!(output.is_file());
        assert!(output.with_file_name("decoded.render.json").is_file());
        let manifest_path = output.with_file_name("decoded.png.manifest.json");
        let manifest = crate::storage::manifest::read_manifest(&manifest_path).unwrap().unwrap();
        assert!(!manifest.raw_complete);
        assert_eq!(manifest.artifacts.len(), 2);
        assert!(crate::storage::manifest::is_complete(store.root(), &manifest));

        let png = image::open(&output).unwrap().to_rgba8();
        assert_eq!(png.dimensions(), (2, 2));
        assert_eq!(png.get_pixel(0, 0).0[3], 0); // ascending latitude was flipped; quality=1
        assert_eq!(png.get_pixel(1, 0).0[3], 0); // NaN remains transparent

        let (second_status, _) = commit_decoded_field(
            &store,
            frame,
            &revision,
            &field,
            &Limits::default(),
            DecodedCommitOptions {
                format: DecodedOutputFormat::Png,
                overwrite: false,
                output_template: None,
                processing: DecodedProcessing::default(),
                include_raw: false,
            },
            &cancellation,
        )
        .unwrap();
        assert_eq!(second_status, LocalCommitStatus::Skipped);
    }

    #[test]
    fn decoded_output_template_controls_the_committed_path_and_sidecar_names() {
        let root = tempfile::tempdir().unwrap();
        let frame = fixture_frame(0);
        let field = png_field();
        let revision = hex::encode(Sha256::digest(b"fixture-revision"));
        let prepared = prepare_decoded_output_files(
            root.path(),
            frame,
            &revision,
            &field,
            &Limits::default(),
            DecodedCommitOptions {
                format: DecodedOutputFormat::Png,
                overwrite: false,
                output_template: Some("{source}/{product}/{valid_time}.{ext}".into()),
                processing: DecodedProcessing::default(),
                include_raw: false,
            },
            None,
        )
        .unwrap();

        assert_eq!(prepared.request.output_name, "rainviewer/composite/20250101T000000Z.png");
        assert!(prepared.request.artifacts.iter().any(|artifact| {
            artifact.relative_uri == "rainviewer/composite/20250101T000000Z.png"
        }));
        assert!(prepared.request.artifacts.iter().any(|artifact| {
            artifact.relative_uri == "rainviewer/composite/20250101T000000Z.render.json"
        }));
        let output_uri = prepared.output_uri.clone();
        let store = LocalStore::new(root.path(), Limits::default()).unwrap();
        let committed = store.commit(prepared.request.clone()).unwrap();
        assert_eq!(committed.status, LocalCommitStatus::Written);
        assert!(Path::new(&output_uri).is_file());
        assert!(root.path().join("rainviewer/composite/20250101T000000Z.render.json").is_file());
        let manifest = crate::storage::manifest::read_manifest(
            &root.path().join("rainviewer/composite/20250101T000000Z.png.manifest.json"),
        )
        .unwrap()
        .unwrap();
        assert!(crate::storage::manifest::is_complete(root.path(), &manifest));
    }

    #[tokio::test]
    async fn raw_only_download_commits_complete_manifest_and_skips_on_repeat() {
        let root = tempfile::tempdir().unwrap();
        let engine = replay_engine(root.path(), 2);
        let frame = fixture_frame(0);
        let first = engine
            .download_raw_only(vec![frame.clone()], FetchErrorPolicy::Collect, false, false)
            .await;
        assert_eq!(first.written, 1);
        assert_eq!(first.failed, 0);
        let output_root = root.path().join("output");
        let manifest_path = output_root
            .join(format!("frames/{}/raw-manifest.json.manifest.json", frame.logical_id));
        let manifest = crate::storage::manifest::read_manifest(&manifest_path).unwrap().unwrap();
        assert!(crate::storage::manifest::is_complete(&output_root, &manifest));
        assert!(manifest.raw_complete);

        let second =
            engine.download_raw_only(vec![frame], FetchErrorPolicy::Collect, false, false).await;
        assert_eq!(second.skipped, 1);
        assert_eq!(second.written, 0);
    }

    #[tokio::test]
    async fn decoded_png_download_runs_verified_decoder_and_commits_with_sidecar() {
        let root = tempfile::tempdir().unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.frame_concurrency = 2;
        config.runtime.temp_root = Some(root.path().join("temp"));
        config.storage.output = root.path().join("output");
        config.cache.dir = root.path().join("cache");
        let mut overrides = SourceRegistry::default();
        overrides.register(Arc::new(RainViewerFixtureAdapter)).unwrap();
        let engine = Engine::new(config, overrides).unwrap();
        let frame = rainviewer_fixture_frame();

        let first =
            engine.download_png(vec![frame.clone()], FetchErrorPolicy::Collect, false, false).await;
        assert_eq!(first.written, 1, "{:?}", first.items);
        assert_eq!(first.failed, 0);
        let output_root = root.path().join("output");
        let output = output_root.join(format!("frames/{}/decoded.png", frame.logical_id));
        let image = image::open(&output).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (1024, 1024));
        assert!(output.with_file_name("decoded.render.json").is_file());
        let manifest = crate::storage::manifest::read_manifest(
            &output.with_file_name("decoded.png.manifest.json"),
        )
        .unwrap()
        .unwrap();
        assert!(!manifest.raw_complete);
        assert!(crate::storage::manifest::is_complete(&output_root, &manifest));

        let second =
            engine.download_png(vec![frame], FetchErrorPolicy::Collect, false, false).await;
        assert_eq!(second.skipped, 1);
        assert_eq!(second.failed, 0);
    }

    #[tokio::test]
    async fn decoded_zarr_download_commits_nested_store_with_manifest_last_and_skips_repeat() {
        let root = tempfile::tempdir().unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.frame_concurrency = 2;
        config.runtime.temp_root = Some(root.path().join("temp"));
        config.storage.output = root.path().join("output");
        config.cache.dir = root.path().join("cache");
        let mut overrides = SourceRegistry::default();
        overrides.register(Arc::new(RainViewerFixtureAdapter)).unwrap();
        let engine = Engine::new(config, overrides).unwrap();
        let frame = rainviewer_fixture_frame();

        let first = engine
            .download_zarr(vec![frame.clone()], FetchErrorPolicy::Collect, false, false)
            .await;
        assert_eq!(first.written, 1, "{:?}", first.items);
        assert_eq!(first.failed, 0);
        let output_root = root.path().join("output");
        let output = output_root.join(format!("frames/{}/decoded.zarr", frame.logical_id));
        assert!(output.is_dir());
        assert!(output.join(".zmetadata").is_file());
        let manifest_path = output.with_file_name("decoded.zarr.manifest.json");
        let manifest = crate::storage::manifest::read_manifest(&manifest_path).unwrap().unwrap();
        assert!(!manifest.raw_complete);
        assert!(manifest.artifacts.len() > 10);
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.relative_uri.ends_with("decoded.zarr/.zmetadata")
                && artifact.role == "metadata"
                && artifact.media_type == "application/json"
        }));
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.relative_uri.starts_with(&format!("frames/{}/decoded.zarr/", frame.logical_id))
                && artifact.role == "data"
        }));
        assert!(crate::storage::manifest::is_complete(output_root.as_path(), &manifest));

        let second =
            engine.download_zarr(vec![frame], FetchErrorPolicy::Collect, false, false).await;
        assert_eq!(second.skipped, 1);
        assert_eq!(second.failed, 0);
    }

    #[tokio::test]
    async fn decoded_geotiff_download_commits_three_artifacts_and_skips_repeat() {
        let root = tempfile::tempdir().unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.frame_concurrency = 2;
        config.runtime.temp_root = Some(root.path().join("temp"));
        config.storage.output = root.path().join("output");
        config.cache.dir = root.path().join("cache");
        let mut overrides = SourceRegistry::default();
        overrides.register(Arc::new(RainViewerFixtureAdapter)).unwrap();
        let engine = Engine::new(config, overrides).unwrap();
        let frame = rainviewer_fixture_frame();

        let raw = engine.fetch_raw(frame.clone()).await.unwrap();
        let decoded = engine.decode_science(Arc::new(raw)).await.unwrap();
        crate::output::geotiff::write_field(
            &decoded,
            root.path().join("direct/decoded.tif"),
            &engine.resource_limits(),
        )
        .unwrap();

        let first = engine
            .download_geotiff(vec![frame.clone()], FetchErrorPolicy::Collect, false, false)
            .await;
        assert_eq!(first.written, 1, "{:?}", first.items);
        assert_eq!(first.failed, 0);
        let output_root = root.path().join("output");
        let output = output_root.join(format!("frames/{}/decoded.tif", frame.logical_id));
        assert!(output.is_file());
        assert!(output.with_file_name("decoded_quality.tif").is_file());
        assert!(output.with_file_name("decoded_provenance.json").is_file());
        let manifest = crate::storage::manifest::read_manifest(
            &output.with_file_name("decoded.tif.manifest.json"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(manifest.artifacts.len(), 3);
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.relative_uri.ends_with("decoded.tif") && artifact.role == "data"
        }));
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.relative_uri.ends_with("decoded_quality.tif") && artifact.role == "data"
        }));
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.relative_uri.ends_with("decoded_provenance.json")
                && artifact.role == "metadata"
        }));
        assert!(crate::storage::manifest::is_complete(output_root.as_path(), &manifest));

        let second =
            engine.download_geotiff(vec![frame], FetchErrorPolicy::Collect, false, false).await;
        assert_eq!(second.skipped, 1);
        assert_eq!(second.failed, 0);
    }

    #[tokio::test]
    async fn raw_only_dry_run_never_fetches_or_creates_the_output_root() {
        let root = tempfile::tempdir().unwrap();
        let engine = replay_engine(root.path(), 2);
        let report = engine
            .download_raw_only(vec![fixture_frame(0)], FetchErrorPolicy::Collect, true, false)
            .await;
        assert_eq!(report.planned, 1);
        assert_eq!(report.written, 0);
        assert_eq!(report.items[0].status, DownloadStatus::Planned);
        assert!(!root.path().join("output").exists());
    }

    #[tokio::test]
    async fn stop_policy_preserves_committed_prefix_and_does_not_start_later_frames() {
        let root = tempfile::tempdir().unwrap();
        let engine = replay_engine(root.path(), 1);
        let frames = vec![fixture_frame(0), fixture_frame(1), fixture_frame(2)];
        let report = engine.download_raw_only(frames, FetchErrorPolicy::Stop, false, false).await;
        assert_eq!(report.items[0].status, DownloadStatus::Written);
        assert_eq!(report.items[1].status, DownloadStatus::Failed);
        assert_eq!(report.items[2].status, DownloadStatus::NotStarted);
        assert_eq!(report.written, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.not_started, 1);
    }

    #[tokio::test]
    async fn decoded_png_stop_policy_stops_after_first_decode_failure() {
        let root = tempfile::tempdir().unwrap();
        let engine = replay_engine(root.path(), 1);
        let frames = vec![fixture_frame(0), fixture_frame(2), fixture_frame(3)];
        let report = engine.download_png(frames, FetchErrorPolicy::Stop, false, false).await;
        assert_eq!(report.items[0].status, DownloadStatus::Failed);
        assert_eq!(report.items[1].status, DownloadStatus::NotStarted);
        assert_eq!(report.items[2].status, DownloadStatus::NotStarted);
        assert_eq!(report.failed, 1);
        assert_eq!(report.not_started, 2);
        assert!(!root.path().join("output/frames").exists());
    }

    #[tokio::test]
    async fn decoded_png_collect_policy_commits_later_frames_after_decode_failure() {
        let root = tempfile::tempdir().unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.frame_concurrency = 2;
        config.runtime.temp_root = Some(root.path().join("temp"));
        config.storage.output = root.path().join("output");
        config.cache.dir = root.path().join("cache");
        let mut overrides = SourceRegistry::default();
        overrides.register(Arc::new(RainViewerFixtureAdapter)).unwrap();
        let engine = Engine::new(config, overrides).unwrap();

        let mut corrupt = rainviewer_fixture_frame();
        corrupt.valid_time = "2026-09-18T01:00:00Z".into();
        corrupt.locator["fixture_corrupt"] = json!(true);
        corrupt.logical_id = logical_id(&corrupt).unwrap();
        let valid = rainviewer_fixture_frame();
        let report = engine
            .download_png(vec![corrupt, valid.clone()], FetchErrorPolicy::Collect, false, false)
            .await;

        assert_eq!(report.items[0].status, DownloadStatus::Failed);
        assert_eq!(report.items[1].status, DownloadStatus::Written);
        assert_eq!((report.failed, report.written, report.not_started), (1, 1, 0));
        assert!(
            root.path().join(format!("output/frames/{}/decoded.png", valid.logical_id)).is_file()
        );
    }
}
