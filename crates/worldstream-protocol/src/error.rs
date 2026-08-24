use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable error codes needed by the process shell.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Configuration did not satisfy the versioned contract.
    ConfigInvalid,
    /// The storage layer intentionally does not exist in this scaffold.
    StorageNotInitialized,
    /// An unexpected boundary failure occurred.
    Internal,
    Unauthenticated,
    Forbidden,
    UnsupportedProtocol,
    InvalidEnvelope,
    MessageTooLarge,
    RoomNotFound,
    ActivityPackRevisionUnavailable,
    MembershipNotFound,
    MembershipNotEnabled,
    RoomFaulted,
    RoomQuarantined,
    RoomBusy,
    CursorAhead,
    CursorOutOfRange,
    SyncBarrierMismatch,
    IdempotencyConflict,
    CommitIndeterminate,
    InvalidPayload,
    ActivityFault,
    RateLimited,
    StorageUnavailable,
    SlowConsumer,
}

/// Bounded machine-readable error body.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    /// Stable code for programmatic handling.
    pub code: ErrorCode,
    /// Safe operator-facing explanation without secrets or payload bodies.
    pub message: String,
    /// Whether retrying without a configuration or process-state change can help.
    pub retryable: bool,
    /// Optional bounded structured detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Strict error body carried by a versioned WebSocket `error` message.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolErrorBody {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Top-level JSON error shape used by operator endpoints.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorEnvelope {
    /// Error content.
    pub error: ErrorBody,
}

impl ErrorEnvelope {
    /// Constructs a safe error envelope without unbounded details.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            error: ErrorBody {
                code,
                message: message.into(),
                retryable,
                details: None,
            },
        }
    }
}
