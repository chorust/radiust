use thiserror::Error;

#[derive(Clone, Debug, Error)]
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
    #[error("complete output already exists; overwrite is required")]
    OutputConflict,
    #[error("storage error: {0}")]
    Storage(String),
    #[error("remote commit outcome is unknown; read the destination before retrying")]
    CommitOutcomeUnknown,
    #[error("operation cancelled")]
    Cancelled,
}

pub type CoreResult<T> = Result<T, CoreError>;
