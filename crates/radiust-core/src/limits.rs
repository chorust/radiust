use crate::errors::{CoreError, CoreResult};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct Limits {
    pub frame_concurrency: usize,
    pub request_concurrency: usize,
    pub host_concurrency: usize,
    pub decode_workers: usize,
    pub max_artifact_bytes: u64,
    pub max_frame_bytes: u64,
    pub max_pixels: u64,
    pub max_temp_bytes: u64,
    pub request_timeout_secs: u64,
    pub frame_deadline_secs: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            frame_concurrency: 2,
            request_concurrency: 16,
            host_concurrency: 4,
            decode_workers: 2,
            max_artifact_bytes: 512 * 1024 * 1024,
            max_frame_bytes: 2 * 1024 * 1024 * 1024,
            max_pixels: 100_000_000,
            max_temp_bytes: 10 * 1024 * 1024 * 1024,
            request_timeout_secs: 30,
            frame_deadline_secs: 300,
        }
    }
}

impl Limits {
    pub fn validate_bytes(&self, size: u64, frame_total: u64) -> CoreResult<()> {
        if size > self.max_artifact_bytes {
            return Err(CoreError::ResourceLimit(format!(
                "artifact {size} > {}",
                self.max_artifact_bytes
            )));
        }
        if frame_total > self.max_frame_bytes {
            return Err(CoreError::ResourceLimit(format!(
                "frame {frame_total} > {}",
                self.max_frame_bytes
            )));
        }
        Ok(())
    }

    pub fn validate_pixels(&self, pixels: u64) -> CoreResult<()> {
        if pixels > self.max_pixels {
            return Err(CoreError::ResourceLimit(format!("pixels {pixels} > {}", self.max_pixels)));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct RequestBudget {
    requests: Arc<Semaphore>,
    frames: Arc<Semaphore>,
    pub cancellation: CancellationToken,
}

impl RequestBudget {
    pub fn new(limits: &Limits) -> Self {
        Self {
            requests: Arc::new(Semaphore::new(limits.request_concurrency)),
            frames: Arc::new(Semaphore::new(limits.frame_concurrency)),
            cancellation: CancellationToken::new(),
        }
    }

    pub async fn acquire_request(&self) -> CoreResult<OwnedSemaphorePermit> {
        tokio::select! {
            permit = self.requests.clone().acquire_owned() => permit.map_err(|_| CoreError::Cancelled),
            _ = self.cancellation.cancelled() => Err(CoreError::Cancelled),
        }
    }

    pub async fn acquire_frame(&self) -> CoreResult<OwnedSemaphorePermit> {
        tokio::select! {
            permit = self.frames.clone().acquire_owned() => permit.map_err(|_| CoreError::Cancelled),
            _ = self.cancellation.cancelled() => Err(CoreError::Cancelled),
        }
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
}
