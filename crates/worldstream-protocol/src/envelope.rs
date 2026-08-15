use serde::{Deserialize, Serialize};

use crate::RequestId;

/// Common strict envelope for future versioned client/server messages.
///
/// The shell does not expose Room commands yet; this primitive prevents probe
/// responses from becoming an accidental public Room protocol.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionedEnvelope<T> {
    /// Wire contract version declared by the embedded compatibility manifest.
    pub wire_version: String,
    /// Stable, schema-defined message discriminator.
    pub message_type: String,
    /// Optional transport correlation ID; never an idempotency identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    /// Typed message content.
    pub body: T,
}
