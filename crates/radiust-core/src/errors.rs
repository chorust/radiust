use thiserror::Error;

/// Stable provider failures whose public classification must not depend on
/// parsing an error string.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ProviderError {
    #[error("station is not in the provider catalog")]
    UnknownStation,
    #[error("provider catalog is unavailable")]
    CatalogUnavailable,
    #[error("provider has no matching observation time")]
    NoMatchingTime,
    #[error("provider index contains ambiguous candidates")]
    AmbiguousIndex,
    #[error("provider file tickets were exhausted")]
    TicketExhausted,
    #[error("the selected provider frame is no longer available")]
    SelectedFrameDisappeared,
    #[error("provider returned an unexpected body")]
    UnexpectedBody,
    #[error("provider response format has not been verified")]
    DecodeUnverified,
    #[error("provider grid is invalid")]
    InvalidGrid,
    #[error("provider denied access")]
    AccessDenied,
    #[error("provider request timed out")]
    Timeout,
}

#[derive(Clone, Debug, Error)]
pub enum CoreError {
    #[error("gray encoding is invalid: {reason}")]
    InvalidGrayEncoding {
        reason: String,
        row: Option<usize>,
        column: Option<usize>,
        value: Option<String>,
    },
    #[error("requested dBZ values have incompatible variable or units")]
    UnitMismatch { variable: String, units: Option<String> },
    #[error("network access to {0} is disabled")]
    NetworkDisabled(String),
    #[error("resource limit exceeded: {0}")]
    ResourceLimit(String),
    #[error("transport failed: {0}")]
    Transport(String),
    #[error("HTTP request returned status {status}")]
    HttpStatus { status: u16, retryable: bool },
    #[error(transparent)]
    Provider(ProviderError),
    #[error("temporary file error: {0}")]
    Temporary(String),
    #[error("cache error: {0}")]
    Cache(String),
    #[error("input integrity check failed: {0}")]
    Integrity(String),
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
