use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("network access to {0} is disabled")]
    NetworkDisabled(String),
    #[error("resource limit exceeded: {0}")]
    ResourceLimit(String),
    #[error("transport failed: {0}")]
    Transport(String),
    #[error("temporary file error: {0}")]
    Temporary(String),
    #[error("cache error: {0}")]
    Cache(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("operation cancelled")]
    Cancelled,
}

pub type CoreResult<T> = Result<T, CoreError>;
