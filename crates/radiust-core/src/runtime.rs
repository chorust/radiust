use crate::limits::{Limits, RequestBudget};
use std::sync::Arc;
use tokio::runtime::{Builder, Runtime};

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
