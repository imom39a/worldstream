//! Provider-neutral black-box vectors for the storage commit seam.
//!
//! The vector deliberately observes only the public `RoomCommitStorageV1`
//! contract: resolution class, duplicate status, canonical receipt bytes, and
//! guarded resolution.  It does not inspect provider tables or infer a live
//! PostgreSQL result from the in-process fixture.

use worldstream_core::{
    CanonicalRequestHashV1, OperationIdentityV1, PreparedRoomWriteV1, ResolveOutcomeV1,
    RoomCommitResolutionV1, RoomCommitStorageV1,
};

/// Stable, provider-neutral resolution labels suitable for transcript
/// comparisons. Receipt bytes are kept separately and never decoded or
/// re-encoded by the harness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConformanceResolutionKind {
    GenesisCreated,
    TransitionCommitted,
    RejectionRecorded,
    NoChangeRecorded,
    NotApplicable,
    Reprepare,
    Fenced,
    Conflict,
    RetryableKnownAbsent,
    Indeterminate,
    Fault,
}

impl ConformanceResolutionKind {
    fn from_resolution(resolution: &RoomCommitResolutionV1) -> Self {
        match resolution {
            RoomCommitResolutionV1::GenesisCreated { .. } => Self::GenesisCreated,
            RoomCommitResolutionV1::TransitionCommitted { .. } => Self::TransitionCommitted,
            RoomCommitResolutionV1::RejectionRecorded { .. } => Self::RejectionRecorded,
            RoomCommitResolutionV1::NoChangeRecorded { .. } => Self::NoChangeRecorded,
            RoomCommitResolutionV1::NotApplicable => Self::NotApplicable,
            RoomCommitResolutionV1::Reprepare => Self::Reprepare,
            RoomCommitResolutionV1::Fenced => Self::Fenced,
            RoomCommitResolutionV1::Conflict { .. } => Self::Conflict,
            RoomCommitResolutionV1::RetryableKnownAbsent => Self::RetryableKnownAbsent,
            RoomCommitResolutionV1::Indeterminate => Self::Indeterminate,
            RoomCommitResolutionV1::Fault => Self::Fault,
        }
    }
}

/// Stable guarded-resolution labels for transcript comparisons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConformanceResolveKind {
    StoredResolution,
    Conflict,
    KnownAbsent,
    ResolutionUnavailable,
}

impl ConformanceResolveKind {
    fn from_outcome(outcome: &ResolveOutcomeV1) -> Self {
        match outcome {
            ResolveOutcomeV1::StoredResolution(_) => Self::StoredResolution,
            ResolveOutcomeV1::Conflict { .. } => Self::Conflict,
            ResolveOutcomeV1::KnownAbsent => Self::KnownAbsent,
            ResolveOutcomeV1::ResolutionUnavailable => Self::ResolutionUnavailable,
        }
    }
}

/// One observed operation in a provider-neutral transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConformanceObservation {
    /// Canonical Operation Identity bytes for stable cross-provider matching.
    pub identity_bytes: Option<Vec<u8>>,
    /// Canonical Request Hash bytes supplied to the storage port.
    pub request_hash_bytes: Vec<u8>,
    /// Resolution class returned by `commit`.
    pub resolution: ConformanceResolutionKind,
    /// Whether the returned durable result was an idempotent duplicate.
    pub duplicate: bool,
    /// Exact canonical receipt bytes, when a durable result was returned.
    pub receipt_bytes: Option<Vec<u8>>,
    /// Resolution class returned by the identity/hash lookup.
    pub resolved: ConformanceResolveKind,
    /// Exact canonical receipt bytes returned by guarded resolution, if any.
    pub resolved_receipt_bytes: Option<Vec<u8>>,
}

/// Runs one prepared vector through any storage provider.
///
/// The vector is owned because prepared writes are intentionally opaque and
/// are not required to be cloneable. Callers create the same vector twice to
/// compare two providers.
pub fn run_vector(
    storage: &dyn RoomCommitStorageV1,
    vector: impl IntoIterator<Item = PreparedRoomWriteV1>,
) -> Vec<ConformanceObservation> {
    vector
        .into_iter()
        .map(|prepared| {
            let identity: OperationIdentityV1 = prepared.identity().clone();
            let request_hash: CanonicalRequestHashV1 = prepared.request_hash().clone();
            let identity_bytes = identity.canonical_bytes().ok();
            let resolution = storage.commit(&prepared);
            let receipt_bytes = resolution
                .stored_result()
                .map(|result| result.canonical_receipt_bytes().to_vec());
            let duplicate = resolution.duplicate();
            let resolved = storage.resolve(&identity, &request_hash);
            let resolved_receipt_bytes = match &resolved {
                ResolveOutcomeV1::StoredResolution(result) => {
                    Some(result.canonical_receipt_bytes().to_vec())
                }
                ResolveOutcomeV1::Conflict { .. }
                | ResolveOutcomeV1::KnownAbsent
                | ResolveOutcomeV1::ResolutionUnavailable => None,
            };
            ConformanceObservation {
                identity_bytes,
                request_hash_bytes: request_hash.as_bytes().to_vec(),
                resolution: ConformanceResolutionKind::from_resolution(&resolution),
                duplicate,
                receipt_bytes,
                resolved: ConformanceResolveKind::from_outcome(&resolved),
                resolved_receipt_bytes,
            }
        })
        .collect()
}
