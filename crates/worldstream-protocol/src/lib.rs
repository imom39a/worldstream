//! Minimal protocol primitives shared by the `WorldStream` process shell.
//!
//! Room commands and domain payloads intentionally do not live here yet. They
//! are introduced only with their canonical schemas and conformance fixtures.

mod envelope;
mod error;
mod id;

pub use envelope::VersionedEnvelope;
pub use error::{ErrorBody, ErrorCode, ErrorEnvelope};
pub use id::{IdParseError, RequestId, UlidString};
