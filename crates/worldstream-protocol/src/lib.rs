//! Minimal protocol primitives shared by the `WorldStream` process shell.
//!
//! Room commands and domain payloads intentionally do not live here yet. They
//! are introduced only with their canonical schemas and conformance fixtures.

mod bearer;
mod envelope;
mod error;
mod id;
mod messages;

pub use bearer::{
    BEARER_BYTES, BEARER_HEX_LENGTH, BEARER_WIRE_LENGTH, BEARER_WIRE_PREFIX, BearerWireError,
    BearerWireV1,
};
pub use envelope::{
    EnvelopeError, MAX_ACTION_PAYLOAD_BYTES, MAX_MESSAGE_BYTES, PROTOCOL_VERSION, ProtocolEnvelope,
    VersionedEnvelope, WEBSOCKET_SUBPROTOCOL, decode_envelope,
};
pub use error::{ErrorBody, ErrorCode, ErrorEnvelope, ProtocolErrorBody};
pub use id::{IdParseError, RequestId, UlidString};
pub use messages::*;
