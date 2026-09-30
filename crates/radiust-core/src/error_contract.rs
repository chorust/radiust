//! Stable and redacted error representation shared by the CLI and bindings.

use crate::errors::CoreError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorStage {
    Validate,
    Discover,
    Acquire,
    Decode,
    Regrid,
    Stage,
    Commit,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidQuery,
    NetworkRestricted,
    ResourceLimit,
    Transport,
    Cancelled,
    CommitOutcomeUnknown,
    Integrity,
    Cache,
    OutputConflict,
    Storage,
    Unsupported,
    Internal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErrorReport {
    pub code: ErrorCode,
    pub message: String,
    pub stage: ErrorStage,
    pub retryable: bool,
}

impl ErrorReport {
    /// Keep an ambiguous remote publish distinct from both success and a
    /// safely retryable failure until the destination has been read back.
    pub fn commit_outcome_unknown() -> Self {
        Self {
            code: ErrorCode::CommitOutcomeUnknown,
            message: "commit outcome is unknown; read the destination before retrying".to_owned(),
            stage: ErrorStage::Commit,
            retryable: false,
        }
    }

    pub fn from_core(error: &CoreError, stage: ErrorStage) -> Self {
        let (code, message, retryable) = match error {
            // Do not copy the URL: it can contain signed query parameters.
            CoreError::NetworkDisabled(_) => (
                ErrorCode::NetworkRestricted,
                "public network access is disabled".to_owned(),
                false,
            ),
            CoreError::ResourceLimit(_) => {
                (ErrorCode::ResourceLimit, "configured resource limit exceeded".to_owned(), false)
            }
            CoreError::Transport(message) if message == "source ph returned a placeholder data image" => {
                (ErrorCode::Transport, message.clone(), true)
            }
            CoreError::Transport(_) => {
                (ErrorCode::Transport, "transport request failed".to_owned(), true)
            }
            CoreError::Temporary(_) => {
                (ErrorCode::Storage, "temporary data operation failed".to_owned(), false)
            }
            CoreError::Cache(_) => (ErrorCode::Cache, "cache operation failed".to_owned(), false),
            CoreError::OutputConflict => (
                ErrorCode::OutputConflict,
                "complete output already exists; overwrite is required".to_owned(),
                false,
            ),
            CoreError::Storage(_) => {
                (ErrorCode::Storage, "output operation failed".to_owned(), false)
            }
            CoreError::CommitOutcomeUnknown => return Self::commit_outcome_unknown(),
            CoreError::Cancelled => (ErrorCode::Cancelled, "operation cancelled".to_owned(), false),
        };
        Self { code, message, stage, retryable }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_error_does_not_expose_signed_url() {
        let error = CoreError::NetworkDisabled("https://example.invalid/?token=secret".into());
        let report = ErrorReport::from_core(&error, ErrorStage::Acquire);
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(!serialized.contains("example.invalid"));
        assert!(!serialized.contains("secret"));
        assert_eq!(report.code, ErrorCode::NetworkRestricted);
    }

    #[test]
    fn unknown_remote_commit_is_not_retryable_or_reported_as_success() {
        let report = ErrorReport::commit_outcome_unknown();
        assert_eq!(report.code, ErrorCode::CommitOutcomeUnknown);
        assert_eq!(report.stage, ErrorStage::Commit);
        assert!(!report.retryable);
        assert!(report.message.contains("read the destination"));
        assert_eq!(
            ErrorReport::from_core(&CoreError::CommitOutcomeUnknown, ErrorStage::Commit),
            report
        );
    }

    #[test]
    fn complete_output_conflict_has_a_stable_actionable_error_code() {
        let report = ErrorReport::from_core(&CoreError::OutputConflict, ErrorStage::Commit);
        assert_eq!(report.code, ErrorCode::OutputConflict);
        assert_eq!(report.message, "complete output already exists; overwrite is required");
        assert_eq!(report.stage, ErrorStage::Commit);
        assert!(!report.retryable);
    }
}
