//! Exact-byte boundary for `WorldStream`'s pinned A202 compatibility profile.
//!
//! This crate is an Application Integrator seam. It owns no Room authority,
//! performs no network access, and holds no private key. It verifies opaque
//! A202 bytes and authenticated resolver observations before a caller submits
//! a typed Action to `WorldStream`.

mod adapter;
mod canonical;
mod crypto;
mod model;
mod proof;

pub use adapter::A202AdapterV1;
pub use canonical::{CanonicalError, canonical_bytes, canonical_content_bytes};
pub use crypto::{
    a202_content_hash, a202_signature_message, decode_base64url, encode_base64url,
    exact_blake3_digest, resolver_attestation_message, verify_es256,
};
pub use model::{
    A202_COMMERCIAL_MEDIA_TYPE, A202_PINNED_REVISION, A202_RULES_VERSION, A202_SPEC_VERSION,
    AdapterError, DetachedEs256SignatureV1, ExactA202ObjectV1, ExpectedA202ObjectV1,
    ExpectedSignatureV1, HostValidityResultV1, KeyStatusV1, LogicalA202HeadV1, OpaqueA202InputV1,
    OperatedSessionBindingV1, PreparedA202SubmissionV1, ProfilePinsV1, ResolvedPublicKeyV1,
    ResolverEvidencePolicyV1, RetryDecisionV1, RoomHeadWitnessV1, SignatureVerificationV1,
    SubmissionBasisV1, UnsignedResolverEvidenceV1, VerificationOutcomeV1,
    VerifiedResolverEvidenceV1,
};
pub use proof::{
    CrossIndexEntryV1, NegotiationProofPackageV1, PackBundleReferenceV1, PartyProtocolProofV1,
    ProofError, ReplayEvidenceV1, VenueRecordV1, VenueRuntimeProofV1,
};
