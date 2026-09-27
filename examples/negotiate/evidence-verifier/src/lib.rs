//! Independent offline verifier for `WorldStream` Negotiate proof packages.
//!
//! This crate intentionally has no dependency on `worldstream-core`, the
//! production A202 adapter, or the Negotiate oracle. Its canonical serializer,
//! digest checks, signature checks, and proof-package parser are independent.

mod canonical;
mod model;
mod verify;

pub use model::{
    CheckResultV1, EvidenceCheckV1, EvidenceScopeV1, EvidenceSectionV1,
    NegotiationVerificationReportV1, TrustedKeyV1, TrustedResolverSourceV1, VerifierTrustV1,
};
pub use verify::{PROOF_PACKAGE_FORMAT_V1, VerificationError, verify_package};
