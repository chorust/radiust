//! Stable and redacted error representation shared by the CLI and bindings.

use crate::errors::{CoreError, ProviderError};
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
    InvalidGrayEncoding,
    UnitMismatch,
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
    UnknownStation,
    CatalogUnavailable,
    NoMatchingTime,
    AmbiguousIndex,
    TicketExhausted,
    SelectedFrameDisappeared,
    UnexpectedBody,
    DecodeUnverified,
    InvalidGrid,
    AccessDenied,
    Timeout,
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
            CoreError::InvalidGrayEncoding { reason, .. } => {
                (ErrorCode::InvalidGrayEncoding, format!("gray input is invalid: {reason}"), false)
            }
            CoreError::UnitMismatch { .. } => (
                ErrorCode::UnitMismatch,
                "dBZ decoding requires a reflectivity variable with dBZ units".to_owned(),
                false,
            ),
            // Do not copy the URL: it can contain signed query parameters.
            CoreError::NetworkDisabled(_) => (
                ErrorCode::NetworkRestricted,
                "public network access is disabled".to_owned(),
                false,
            ),
            CoreError::ResourceLimit(_) => {
                (ErrorCode::ResourceLimit, "configured resource limit exceeded".to_owned(), false)
            }
            CoreError::Transport(message)
                if message == "source ph returned a placeholder data image" =>
            {
                (ErrorCode::Transport, message.clone(), true)
            }
            CoreError::Transport(_) => {
                (ErrorCode::Transport, "transport request failed".to_owned(), true)
            }
            CoreError::HttpStatus { status, retryable } => {
                (ErrorCode::Transport, format!("HTTP request returned status {status}"), *retryable)
            }
            CoreError::Provider(error) => match error {
                ProviderError::UnknownStation => (
                    ErrorCode::UnknownStation,
                    "station is not in the provider catalog".to_owned(),
                    false,
                ),
                ProviderError::CatalogUnavailable => (
                    ErrorCode::CatalogUnavailable,
                    "provider catalog is unavailable".to_owned(),
                    true,
                ),
                ProviderError::NoMatchingTime => (
                    ErrorCode::NoMatchingTime,
                    "provider has no matching observation time".to_owned(),
                    false,
                ),
                ProviderError::AmbiguousIndex => (
                    ErrorCode::AmbiguousIndex,
                    "provider index contains ambiguous candidates".to_owned(),
                    false,
                ),
                ProviderError::TicketExhausted => (
                    ErrorCode::TicketExhausted,
                    "provider file tickets were exhausted".to_owned(),
                    true,
                ),
                ProviderError::SelectedFrameDisappeared => (
                    ErrorCode::SelectedFrameDisappeared,
                    "the selected provider frame is no longer available".to_owned(),
                    false,
                ),
                ProviderError::UnexpectedBody => (
                    ErrorCode::UnexpectedBody,
                    "provider returned an unexpected body".to_owned(),
                    false,
                ),
                ProviderError::DecodeUnverified => (
                    ErrorCode::DecodeUnverified,
                    "provider response format has not been verified".to_owned(),
                    false,
                ),
                ProviderError::InvalidGrid => {
                    (ErrorCode::InvalidGrid, "provider grid is invalid".to_owned(), false)
                }
                ProviderError::AccessDenied => {
                    (ErrorCode::AccessDenied, "provider denied access".to_owned(), false)
                }
                ProviderError::Timeout => {
                    (ErrorCode::Timeout, "provider request timed out".to_owned(), true)
                }
            },
            CoreError::Temporary(_) => {
                (ErrorCode::Storage, "temporary data operation failed".to_owned(), false)
            }
            CoreError::Cache(_) => (ErrorCode::Cache, "cache operation failed".to_owned(), false),
            CoreError::Integrity(_) => (
                ErrorCode::Integrity,
                "raw manifest or retained artifacts failed integrity validation".to_owned(),
                false,
            ),
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

    #[test]
    fn provider_errors_have_typed_safe_codes_and_retryability() {
        let report = ErrorReport::from_core(
            &CoreError::Provider(ProviderError::TicketExhausted),
            ErrorStage::Acquire,
        );
        assert_eq!(report.code, ErrorCode::TicketExhausted);
        assert_eq!(report.stage, ErrorStage::Acquire);
        assert!(report.retryable);
        assert_eq!(report.message, "provider file tickets were exhausted");

        let report = ErrorReport::from_core(
            &CoreError::Provider(ProviderError::InvalidGrid),
            ErrorStage::Decode,
        );
        assert_eq!(report.code, ErrorCode::InvalidGrid);
        assert!(!report.retryable);
    }

    #[test]
    fn provider_error_reports_never_expose_request_details() {
        let report = ErrorReport::from_core(
            &CoreError::Provider(ProviderError::AccessDenied),
            ErrorStage::Acquire,
        );
        let serialized = serde_json::to_string(&report).unwrap();
        assert_eq!(report.code, ErrorCode::AccessDenied);
        assert!(!serialized.contains("https://"));
        assert!(!serialized.contains("ticket"));
        assert!(!serialized.contains("response body"));
    }

    #[test]
    fn http_status_preserves_status_and_retryability_without_provider_details() {
        let report = ErrorReport::from_core(
            &CoreError::HttpStatus { status: 503, retryable: true },
            ErrorStage::Acquire,
        );
        assert_eq!(report.code, ErrorCode::Transport);
        assert_eq!(report.message, "HTTP request returned status 503");
        assert!(report.retryable);
    }

    #[test]
    fn gray_encoding_and_unit_errors_have_stable_non_retryable_codes() {
        let invalid = ErrorReport::from_core(
            &CoreError::InvalidGrayEncoding {
                reason: "visible pixel is outside the declared range".into(),
                row: Some(4),
                column: Some(7),
                value: Some("225".into()),
            },
            ErrorStage::Decode,
        );
        assert_eq!(invalid.code, ErrorCode::InvalidGrayEncoding);
        assert_eq!(invalid.stage, ErrorStage::Decode);
        assert!(!invalid.retryable);

        let units = ErrorReport::from_core(
            &CoreError::UnitMismatch { variable: "rain_rate".into(), units: Some("mm/h".into()) },
            ErrorStage::Decode,
        );
        assert_eq!(units.code, ErrorCode::UnitMismatch);
        assert_eq!(units.message, "dBZ decoding requires a reflectivity variable with dBZ units");
        assert!(!units.retryable);
    }
}
