use crate::errors::{CoreError, CoreResult};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

const MAX_TRACKED_HOSTS: usize = 4096;

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

    pub fn raster_memory_budget(&self) -> RasterMemoryBudget {
        RasterMemoryBudget::new(self.max_temp_bytes)
    }
}

/// Shared accounting for simultaneously-live raster buffers in one operation.
#[derive(Clone)]
pub struct RasterMemoryBudget {
    limit: u64,
    reserved: Arc<AtomicU64>,
}

pub struct RasterBufferLease {
    budget: RasterMemoryBudget,
    bytes: u64,
}

impl RasterMemoryBudget {
    pub fn new(limit: u64) -> Self {
        Self { limit, reserved: Arc::new(AtomicU64::new(0)) }
    }

    pub fn reserved_bytes(&self) -> u64 {
        self.reserved.load(Ordering::Acquire)
    }

    pub fn reserve(&self, bytes: u64) -> CoreResult<RasterBufferLease> {
        let mut current = self.reserved.load(Ordering::Acquire);
        loop {
            let next = current
                .checked_add(bytes)
                .ok_or_else(|| CoreError::ResourceLimit("raster buffer size overflows".into()))?;
            if next > self.limit {
                return Err(CoreError::ResourceLimit(format!(
                    "raster buffers {next} > {}",
                    self.limit
                )));
            }
            match self.reserved.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(RasterBufferLease { budget: self.clone(), bytes }),
                Err(actual) => current = actual,
            }
        }
    }

    /// Reserve every buffer that will coexist for a raster of this shape.
    pub fn reserve_shape(
        &self,
        height: u64,
        width: u64,
        bytes_per_pixel: &[u64],
    ) -> CoreResult<RasterBufferLease> {
        if height == 0 || width == 0 || bytes_per_pixel.is_empty() {
            return Err(CoreError::ResourceLimit("raster buffer shape is invalid".into()));
        }
        let pixels = height
            .checked_mul(width)
            .ok_or_else(|| CoreError::ResourceLimit("raster dimensions overflow".into()))?;
        let per_pixel = bytes_per_pixel.iter().try_fold(0_u64, |sum, value| {
            sum.checked_add(*value)
                .ok_or_else(|| CoreError::ResourceLimit("raster plane size overflows".into()))
        })?;
        let bytes = pixels
            .checked_mul(per_pixel)
            .ok_or_else(|| CoreError::ResourceLimit("raster buffer size overflows".into()))?;
        self.reserve(bytes)
    }
}

impl RasterBufferLease {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for RasterBufferLease {
    fn drop(&mut self) {
        self.budget.reserved.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[derive(Clone)]
pub struct RequestBudget {
    requests: Arc<Semaphore>,
    frames: Arc<Semaphore>,
    hosts: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    overflow_hosts: Arc<Semaphore>,
    host_limit: usize,
    pub cancellation: CancellationToken,
}

impl RequestBudget {
    pub fn new(limits: &Limits) -> Self {
        Self {
            requests: Arc::new(Semaphore::new(limits.request_concurrency)),
            frames: Arc::new(Semaphore::new(limits.frame_concurrency)),
            hosts: Arc::new(Mutex::new(HashMap::new())),
            overflow_hosts: Arc::new(Semaphore::new(limits.host_concurrency)),
            host_limit: limits.host_concurrency,
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

    pub async fn acquire_host(&self, host: &str) -> CoreResult<OwnedSemaphorePermit> {
        let host = host.to_ascii_lowercase();
        let semaphore = {
            let mut hosts = self.hosts.lock();
            if let Some(semaphore) = hosts.get(&host) {
                semaphore.clone()
            } else if hosts.len() < MAX_TRACKED_HOSTS {
                let semaphore = Arc::new(Semaphore::new(self.host_limit));
                hosts.insert(host, semaphore.clone());
                semaphore
            } else {
                self.overflow_hosts.clone()
            }
        };
        tokio::select! {
            permit = semaphore.acquire_owned() => permit.map_err(|_| CoreError::Cancelled),
            _ = self.cancellation.cancelled() => Err(CoreError::Cancelled),
        }
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod raster_memory_tests {
    use super::*;

    #[test]
    fn raster_buffer_reservations_track_overlapping_lifetimes() {
        let budget = RasterMemoryBudget::new(32);
        let first = budget.reserve_shape(2, 2, &[4, 2]).unwrap();
        assert_eq!(first.bytes(), 24);
        assert_eq!(budget.reserved_bytes(), 24);
        assert!(matches!(budget.reserve(9), Err(CoreError::ResourceLimit(_))));
        drop(first);
        assert_eq!(budget.reserved_bytes(), 0);
        let second = budget.reserve_shape(2, 2, &[4, 2]).unwrap();
        assert_eq!(second.bytes(), 24);
    }

    #[test]
    fn raster_buffer_reservations_reject_overflow_and_empty_shapes() {
        let budget = RasterMemoryBudget::new(u64::MAX);
        assert!(matches!(
            budget.reserve_shape(u64::MAX, 2, &[1]),
            Err(CoreError::ResourceLimit(_))
        ));
        assert!(matches!(budget.reserve_shape(1, 0, &[1]), Err(CoreError::ResourceLimit(_))));
    }
}
