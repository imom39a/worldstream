use serde::{Deserialize, Serialize};

use crate::UlidString;

/// The only wire version implemented by this workspace.
pub const PROTOCOL_VERSION: &str = "0.1";
/// Maximum encoded WebSocket message accepted by the gateway.
pub const MAX_MESSAGE_BYTES: usize = 512 * 1024;
/// Maximum encoded Action payload accepted before admission.
pub const MAX_ACTION_PAYLOAD_BYTES: usize = 256 * 1024;

/// Common strict envelope for versioned client/server messages.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionedEnvelope<T> {
    /// Wire contract version declared by the embedded compatibility manifest.
    pub protocol: String,
    /// Stable, schema-defined message discriminator.
    #[serde(rename = "type")]
    pub message_type: String,
    /// Unique identifier for this transmitted envelope.
    pub message_id: UlidString,
    /// Optional transport correlation ID; never an idempotency identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<UlidString>,
    /// Typed message content.
    pub body: T,
}

/// Alias used by transport code and SDK documentation.
pub type ProtocolEnvelope<T> = VersionedEnvelope<T>;

/// Decodes one bounded, version-checked JSON envelope. Type-specific bodies
/// use `deny_unknown_fields`, so a caller cannot smuggle a second schema past
/// the transport boundary.
///
/// # Errors
///
/// Returns [`EnvelopeError::TooLarge`] when the encoded message exceeds
/// [`MAX_MESSAGE_BYTES`], [`EnvelopeError::Malformed`] for invalid JSON,
/// unknown fields, invalid identifiers, or an invalid body, and
/// [`EnvelopeError::UnsupportedVersion`] when the protocol is not `0.1`.
pub fn decode_envelope<T>(raw: &[u8]) -> Result<VersionedEnvelope<T>, EnvelopeError>
where
    T: serde::de::DeserializeOwned,
{
    if raw.len() > MAX_MESSAGE_BYTES {
        return Err(EnvelopeError::TooLarge);
    }
    let untyped =
        serde_json::from_slice::<serde_json::Value>(raw).map_err(|_| EnvelopeError::Malformed)?;
    let protocol = untyped
        .get("protocol")
        .and_then(serde_json::Value::as_str)
        .ok_or(EnvelopeError::Malformed)?;
    if protocol != PROTOCOL_VERSION {
        return Err(EnvelopeError::UnsupportedVersion);
    }
    let envelope = serde_json::from_slice::<VersionedEnvelope<T>>(raw)
        .map_err(|_| EnvelopeError::Malformed)?;
    if envelope.message_type.is_empty() || envelope.message_type.len() > 128 {
        return Err(EnvelopeError::Malformed);
    }
    Ok(envelope)
}

/// Closed transport parsing failures; callers map these to safe protocol
/// errors without returning serde internals or bearer values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EnvelopeError {
    #[error("message exceeds the configured bound")]
    TooLarge,
    #[error("message is not a valid protocol envelope")]
    Malformed,
    #[error("message uses an unsupported protocol version")]
    UnsupportedVersion,
}

#[cfg(test)]
mod tests {
    use super::{EnvelopeError, decode_envelope};
    use crate::ClientHello;

    #[test]
    fn envelope_rejects_unknown_fields_and_wrong_versions() {
        let unknown = br#"{"protocol":"0.1","type":"client.hello","message_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","body":{},"extra":true}"#;
        assert_eq!(
            decode_envelope::<ClientHello>(unknown),
            Err(EnvelopeError::Malformed)
        );
        let wrong = br#"{"protocol":"0.2","type":"client.hello","message_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","body":{}}"#;
        assert_eq!(
            decode_envelope::<ClientHello>(wrong),
            Err(EnvelopeError::UnsupportedVersion)
        );
    }
}
