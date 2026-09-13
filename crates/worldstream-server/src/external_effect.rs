//! Application pattern for durable external effects.
//!
//! This module deliberately stops at a small target port. It does not add a
//! workflow engine or a real connector. Room code creates a request only from
//! a committed transition, and the reconciler keeps ambiguous target outcomes
//! unknown until an explicit probe resolves them.

use std::{collections::BTreeMap, fmt};

use blake3::Hasher;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_EFFECT_ID_BYTES: usize = 128;
pub const MAX_EFFECT_PAYLOAD_BYTES: usize = 64 * 1024;

fn bounded_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_EFFECT_ID_BYTES
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EffectProvenance {
    pub room_id: String,
    pub committed_room_seq: u64,
    pub committed_head_hash: String,
    pub pack_digest: String,
    pub transition_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EffectPreconditions {
    pub room_id: String,
    pub room_seq: u64,
    pub head_hash: String,
    pub integrity_generation: u64,
    pub work_revision: u64,
    /// Stable owner/claim identity, distinct from effect and attempt IDs.
    pub owner_id: String,
    pub activation_id: Option<String>,
    pub activation_lease_generation: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CurrentEffectFence {
    pub room_id: String,
    pub room_seq: u64,
    pub head_hash: String,
    pub integrity_generation: u64,
    pub work_revision: u64,
    pub owner_id: String,
    pub activation_id: Option<String>,
    pub activation_lease_generation: Option<u64>,
    /// Supersession and cancellation are checked at both reconciliation cuts.
    pub eligible: bool,
    pub cancelled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExternalEffectRequest {
    business_operation_id: String,
    work_revision: u64,
    target_id: String,
    provenance: EffectProvenance,
    preconditions: EffectPreconditions,
    payload: Vec<u8>,
    request_digest: String,
}

impl ExternalEffectRequest {
    /// Builds work only from a committed Room transition. The business ID and
    /// revision are distinct from every Action, Activation, and attempt ID.
    pub fn from_committed_room(
        business_operation_id: String,
        work_revision: u64,
        target_id: String,
        provenance: EffectProvenance,
        preconditions: EffectPreconditions,
        payload: Vec<u8>,
    ) -> Result<Self, EffectError> {
        if !bounded_id(&business_operation_id)
            || work_revision == 0
            || !bounded_id(&target_id)
            || !bounded_id(&provenance.room_id)
            || provenance.committed_room_seq == 0
            || !bounded_id(&provenance.committed_head_hash)
            || !bounded_id(&provenance.pack_digest)
            || !bounded_id(&provenance.transition_id)
            || !bounded_id(&preconditions.room_id)
            || !bounded_id(&preconditions.head_hash)
            || !bounded_id(&preconditions.owner_id)
            || preconditions
                .activation_id
                .as_deref()
                .is_some_and(|id| !bounded_id(id))
            || preconditions.room_id != provenance.room_id
            || preconditions.room_seq != provenance.committed_room_seq
            || preconditions.head_hash != provenance.committed_head_hash
            || preconditions.work_revision != work_revision
            || payload.is_empty()
            || payload.len() > MAX_EFFECT_PAYLOAD_BYTES
        {
            return Err(EffectError::InvalidRequest);
        }
        if preconditions.activation_id.is_some()
            != preconditions.activation_lease_generation.is_some()
        {
            return Err(EffectError::InvalidRequest);
        }
        let request_digest = digest_request(
            &business_operation_id,
            work_revision,
            &target_id,
            &provenance,
            &preconditions,
            &payload,
        );
        Ok(Self {
            business_operation_id,
            work_revision,
            target_id,
            provenance,
            preconditions,
            payload,
            request_digest,
        })
    }

    #[must_use]
    pub fn business_operation_id(&self) -> &str {
        &self.business_operation_id
    }
    #[must_use]
    pub const fn work_revision(&self) -> u64 {
        self.work_revision
    }
    #[must_use]
    pub fn target_id(&self) -> &str {
        &self.target_id
    }
    #[must_use]
    pub const fn provenance(&self) -> &EffectProvenance {
        &self.provenance
    }
    #[must_use]
    pub const fn preconditions(&self) -> &EffectPreconditions {
        &self.preconditions
    }
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    #[must_use]
    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EffectStatus {
    Pending,
    Applied,
    Failed,
    Unknown,
    Reconciling,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetApplyOutcome {
    Applied,
    Duplicate,
    LostBeforeApply,
    LostAfterApply,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetProbeOutcome {
    Applied,
    NotApplied,
    Unknown,
    TargetMismatch,
}

pub trait ExternalEffectTarget {
    fn target_id(&self) -> &str;
    fn apply(
        &mut self,
        request: &ExternalEffectRequest,
        attempt_id: &str,
    ) -> Result<TargetApplyOutcome, TargetError>;
    fn probe(&mut self, request: &ExternalEffectRequest)
    -> Result<TargetProbeOutcome, TargetError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectReceipt {
    pub business_operation_id: String,
    pub work_revision: u64,
    pub attempt_id: String,
    pub status: EffectStatus,
    pub request_digest: String,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum EffectError {
    #[error("effect request is invalid")]
    InvalidRequest,
    #[error("effect identity conflicts with an existing work revision")]
    IdentityConflict,
    #[error("effect work revision is stale")]
    StaleRevision,
    #[error("effect is not dispatchable in its current state")]
    NotDispatchable,
    #[error("effect current Room or Activation fence is stale")]
    StaleFence,
    #[error("effect target identity does not match")]
    TargetMismatch,
    #[error("effect target probe is unavailable")]
    ProbeUnavailable,
    #[error("effect owner is no longer eligible")]
    Ineligible,
    #[error("effect was cancelled or superseded")]
    Cancelled,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum TargetError {
    #[error("target rejected the effect")]
    Rejected,
    #[error("target identity mismatch")]
    TargetMismatch,
    #[error("target outcome is ambiguous")]
    Unknown,
}

#[derive(Clone, Debug)]
struct EffectRecord {
    request: ExternalEffectRequest,
    status: EffectStatus,
    attempts: u64,
}

/// Durable journal shape. A storage adapter can persist this value and use it
/// to reconstruct a reconciler after a process restart. `Reconciling` is
/// restored as `Unknown`: a crash during a probe must never be interpreted as
/// proof that the target did or did not apply the effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EffectRecordSnapshot {
    pub request: ExternalEffectRequest,
    pub status: EffectStatus,
    pub attempts: u64,
}

/// In-memory application journal used by the deterministic prototype. A
/// production implementation persists this same record and rehydrates it on
/// restart before dispatching or probing.
pub struct EffectReconciler<T> {
    target: T,
    records: BTreeMap<(String, u64), EffectRecord>,
}

impl<T: ExternalEffectTarget> EffectReconciler<T> {
    #[must_use]
    pub fn new(target: T) -> Self {
        Self {
            target,
            records: BTreeMap::new(),
        }
    }

    /// Rehydrates the application journal without contacting the target.
    pub fn from_snapshot(
        target: T,
        snapshots: impl IntoIterator<Item = EffectRecordSnapshot>,
    ) -> Result<Self, EffectError> {
        let mut reconciler = Self::new(target);
        for snapshot in snapshots {
            let key = (
                snapshot.request.business_operation_id.clone(),
                snapshot.request.work_revision,
            );
            if reconciler
                .records
                .insert(
                    key,
                    EffectRecord {
                        request: snapshot.request,
                        status: if snapshot.status == EffectStatus::Reconciling {
                            EffectStatus::Unknown
                        } else {
                            snapshot.status
                        },
                        attempts: snapshot.attempts,
                    },
                )
                .is_some()
            {
                return Err(EffectError::IdentityConflict);
            }
        }
        Ok(reconciler)
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<EffectRecordSnapshot> {
        self.records
            .values()
            .map(|record| EffectRecordSnapshot {
                request: record.request.clone(),
                status: record.status,
                attempts: record.attempts,
            })
            .collect()
    }

    pub fn enqueue(
        &mut self,
        request: ExternalEffectRequest,
    ) -> Result<EffectReceipt, EffectError> {
        let key = (request.business_operation_id.clone(), request.work_revision);
        if let Some(existing) = self.records.get(&key) {
            if existing.request.request_digest == request.request_digest {
                return Ok(Self::receipt(existing));
            }
            return Err(EffectError::IdentityConflict);
        }
        if self.records.keys().any(|(operation, revision)| {
            operation == &request.business_operation_id && *revision > request.work_revision
        }) {
            return Err(EffectError::StaleRevision);
        }
        let record = EffectRecord {
            request,
            status: EffectStatus::Pending,
            attempts: 0,
        };
        let receipt = Self::receipt(&record);
        self.records.insert(key, record);
        Ok(receipt)
    }

    pub fn dispatch(
        &mut self,
        business_operation_id: &str,
        work_revision: u64,
        fence: &CurrentEffectFence,
    ) -> Result<EffectReceipt, EffectError> {
        let key = (business_operation_id.to_owned(), work_revision);
        let request = self
            .records
            .get(&key)
            .ok_or(EffectError::InvalidRequest)?
            .request
            .clone();
        if self.records.get(&key).map(|record| record.status) != Some(EffectStatus::Pending) {
            return Err(EffectError::NotDispatchable);
        }
        validate_fence(&request, fence)?;
        if self.target.target_id() != request.target_id {
            self.records
                .get_mut(&key)
                .expect("record was just read")
                .status = EffectStatus::Failed;
            return Err(EffectError::TargetMismatch);
        }
        let attempts = {
            let record = self.records.get_mut(&key).expect("record was just read");
            record.attempts = record.attempts.saturating_add(1);
            record.attempts
        };
        let attempt_id = format!("{business_operation_id}/r{work_revision}/a{attempts}");
        let outcome = self.target.apply(&request, &attempt_id);
        let status = match outcome {
            Ok(TargetApplyOutcome::Applied | TargetApplyOutcome::Duplicate) => {
                EffectStatus::Applied
            }
            Ok(TargetApplyOutcome::LostBeforeApply | TargetApplyOutcome::LostAfterApply)
            | Err(TargetError::Unknown) => EffectStatus::Unknown,
            Ok(TargetApplyOutcome::Failed)
            | Err(TargetError::Rejected | TargetError::TargetMismatch) => EffectStatus::Failed,
        };
        let record = self.records.get_mut(&key).expect("record was just read");
        record.status = status;
        Ok(Self::receipt(record))
    }

    pub fn reconcile(
        &mut self,
        business_operation_id: &str,
        work_revision: u64,
        before_probe: &CurrentEffectFence,
        after_probe: &CurrentEffectFence,
    ) -> Result<EffectReceipt, EffectError> {
        let key = (business_operation_id.to_owned(), work_revision);
        let request = self
            .records
            .get(&key)
            .ok_or(EffectError::InvalidRequest)?
            .request
            .clone();
        if self.records.get(&key).map(|record| record.status) != Some(EffectStatus::Unknown) {
            return Err(EffectError::NotDispatchable);
        }
        validate_fence(&request, before_probe)?;
        self.records
            .get_mut(&key)
            .expect("record was just read")
            .status = EffectStatus::Reconciling;
        let status = match self.target.probe(&request) {
            Ok(TargetProbeOutcome::Applied) => EffectStatus::Applied,
            Ok(TargetProbeOutcome::NotApplied) => EffectStatus::Failed,
            Ok(TargetProbeOutcome::Unknown) | Err(TargetError::Unknown) => EffectStatus::Unknown,
            Ok(TargetProbeOutcome::TargetMismatch) | Err(TargetError::TargetMismatch) => {
                EffectStatus::Failed
            }
            Err(TargetError::Rejected) => {
                self.records
                    .get_mut(&key)
                    .expect("record was just read")
                    .status = EffectStatus::Unknown;
                return Err(EffectError::ProbeUnavailable);
            }
        };
        // A Room update, cancellation, lease loss, or supersession may have
        // happened while the target was being probed. Never install a probe
        // answer across that second fence.
        if let Err(error) = validate_fence(&request, after_probe) {
            self.records
                .get_mut(&key)
                .expect("record was just read")
                .status = EffectStatus::Unknown;
            return Err(error);
        }
        self.records
            .get_mut(&key)
            .expect("record was just read")
            .status = status;
        let record = self.records.get(&key).expect("record was just read");
        Ok(Self::receipt(record))
    }

    pub fn retry_failed(
        &mut self,
        business_operation_id: &str,
        work_revision: u64,
    ) -> Result<EffectReceipt, EffectError> {
        let key = (business_operation_id.to_owned(), work_revision);
        let record = self
            .records
            .get_mut(&key)
            .ok_or(EffectError::InvalidRequest)?;
        if record.status != EffectStatus::Failed {
            return Err(EffectError::NotDispatchable);
        }
        record.status = EffectStatus::Pending;
        let record = self.records.get(&key).expect("record was just read");
        Ok(Self::receipt(record))
    }

    #[must_use]
    pub fn status(&self, business_operation_id: &str, work_revision: u64) -> Option<EffectStatus> {
        self.records
            .get(&(business_operation_id.to_owned(), work_revision))
            .map(|record| record.status)
    }

    fn receipt(record: &EffectRecord) -> EffectReceipt {
        EffectReceipt {
            business_operation_id: record.request.business_operation_id.clone(),
            work_revision: record.request.work_revision,
            attempt_id: format!(
                "{}/r{}/a{}",
                record.request.business_operation_id, record.request.work_revision, record.attempts
            ),
            status: record.status,
            request_digest: record.request.request_digest.clone(),
        }
    }
}

fn validate_fence(
    request: &ExternalEffectRequest,
    fence: &CurrentEffectFence,
) -> Result<(), EffectError> {
    let p = request.preconditions();
    if fence.cancelled {
        return Err(EffectError::Cancelled);
    }
    if !fence.eligible {
        return Err(EffectError::Ineligible);
    }
    if p.room_id != fence.room_id
        || p.room_seq != fence.room_seq
        || p.head_hash != fence.head_hash
        || p.integrity_generation != fence.integrity_generation
        || p.work_revision != fence.work_revision
        || p.owner_id != fence.owner_id
        || p.activation_id != fence.activation_id
        || p.activation_lease_generation != fence.activation_lease_generation
    {
        return Err(EffectError::StaleFence);
    }
    Ok(())
}

fn digest_request(
    operation: &str,
    revision: u64,
    target: &str,
    provenance: &EffectProvenance,
    preconditions: &EffectPreconditions,
    payload: &[u8],
) -> String {
    let mut hasher = Hasher::new();
    for value in [
        operation.as_bytes(),
        target.as_bytes(),
        provenance.room_id.as_bytes(),
        provenance.committed_head_hash.as_bytes(),
        provenance.pack_digest.as_bytes(),
        provenance.transition_id.as_bytes(),
        preconditions.room_id.as_bytes(),
        preconditions.head_hash.as_bytes(),
        preconditions.owner_id.as_bytes(),
        payload,
    ] {
        hasher.update(&(value.len() as u64).to_be_bytes());
        hasher.update(value);
    }
    for value in [preconditions
        .activation_id
        .as_deref()
        .unwrap_or("")
        .as_bytes()]
    {
        hasher.update(&(value.len() as u64).to_be_bytes());
        hasher.update(value);
    }
    hasher.update(&revision.to_be_bytes());
    hasher.update(&provenance.committed_room_seq.to_be_bytes());
    hasher.update(&preconditions.room_seq.to_be_bytes());
    hasher.update(&preconditions.integrity_generation.to_be_bytes());
    hasher.update(&preconditions.work_revision.to_be_bytes());
    hasher.update(
        &preconditions
            .activation_lease_generation
            .unwrap_or(0)
            .to_be_bytes(),
    );
    format!("blake3:{}", hasher.finalize().to_hex())
}

/// Deterministic target used by tests and examples. It models target-side
/// idempotency separately from the reconciler's attempt identity.
#[derive(Clone, Debug)]
pub struct FakeEffectTarget {
    target_id: String,
    next_apply: TargetApplyOutcome,
    next_probe: TargetProbeOutcome,
    applied_keys: BTreeMap<String, String>,
}

impl FakeEffectTarget {
    #[must_use]
    pub fn new(target_id: impl Into<String>) -> Self {
        Self {
            target_id: target_id.into(),
            next_apply: TargetApplyOutcome::Applied,
            next_probe: TargetProbeOutcome::Unknown,
            applied_keys: BTreeMap::new(),
        }
    }
    pub fn set_apply_outcome(&mut self, outcome: TargetApplyOutcome) {
        self.next_apply = outcome;
    }
    pub fn set_probe_outcome(&mut self, outcome: TargetProbeOutcome) {
        self.next_probe = outcome;
    }
    #[must_use]
    pub fn applied_count(&self) -> usize {
        self.applied_keys.len()
    }
}

impl ExternalEffectTarget for FakeEffectTarget {
    fn target_id(&self) -> &str {
        &self.target_id
    }
    fn apply(
        &mut self,
        request: &ExternalEffectRequest,
        _attempt_id: &str,
    ) -> Result<TargetApplyOutcome, TargetError> {
        if self.target_id != request.target_id {
            return Err(TargetError::TargetMismatch);
        }
        let key = format!(
            "{}/r{}",
            request.business_operation_id, request.work_revision
        );
        let outcome = self.next_apply;
        let already_applied = self.applied_keys.contains_key(&key);
        if matches!(
            outcome,
            TargetApplyOutcome::Applied
                | TargetApplyOutcome::Duplicate
                | TargetApplyOutcome::LostAfterApply
        ) {
            self.applied_keys
                .entry(key)
                .or_insert_with(|| request.request_digest.clone());
        }
        Ok(
            if already_applied && outcome == TargetApplyOutcome::Applied {
                TargetApplyOutcome::Duplicate
            } else {
                outcome
            },
        )
    }
    fn probe(
        &mut self,
        request: &ExternalEffectRequest,
    ) -> Result<TargetProbeOutcome, TargetError> {
        if self.target_id != request.target_id {
            return Err(TargetError::TargetMismatch);
        }
        Ok(
            if self.applied_keys.contains_key(&format!(
                "{}/r{}",
                request.business_operation_id, request.work_revision
            )) {
                TargetProbeOutcome::Applied
            } else {
                self.next_probe
            },
        )
    }
}

impl fmt::Display for EffectStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            Self::Reconciling => "reconciling",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ExternalEffectRequest {
        ExternalEffectRequest::from_committed_room(
            "business-1".to_owned(),
            1,
            "email-sink".to_owned(),
            EffectProvenance {
                room_id: "room-1".to_owned(),
                committed_room_seq: 42,
                committed_head_hash: "head-42".to_owned(),
                pack_digest: "pack-v1".to_owned(),
                transition_id: "transition-42".to_owned(),
            },
            EffectPreconditions {
                room_id: "room-1".to_owned(),
                room_seq: 42,
                head_hash: "head-42".to_owned(),
                integrity_generation: 3,
                work_revision: 1,
                owner_id: "runner-1".to_owned(),
                activation_id: Some("activation-1".to_owned()),
                activation_lease_generation: Some(7),
            },
            br#"{"message":"hello"}"#.to_vec(),
        )
        .unwrap_or_else(|_| unreachable!("valid effect request"))
    }

    fn fence() -> CurrentEffectFence {
        CurrentEffectFence {
            room_id: "room-1".to_owned(),
            room_seq: 42,
            head_hash: "head-42".to_owned(),
            integrity_generation: 3,
            work_revision: 1,
            owner_id: "runner-1".to_owned(),
            activation_id: Some("activation-1".to_owned()),
            activation_lease_generation: Some(7),
            eligible: true,
            cancelled: false,
        }
    }

    #[test]
    fn success_and_target_duplicate_are_distinct_from_attempt_identity() {
        let mut target = FakeEffectTarget::new("email-sink");
        let req = request();
        assert_eq!(
            target
                .apply(&req, "business-1/r1/a1")
                .unwrap_or_else(|_| unreachable!()),
            TargetApplyOutcome::Applied
        );
        assert_eq!(
            target
                .apply(&req, "business-1/r1/a2")
                .unwrap_or_else(|_| unreachable!()),
            TargetApplyOutcome::Duplicate
        );
        let mut reconciler = EffectReconciler::new(target);
        let first = reconciler
            .enqueue(request())
            .unwrap_or_else(|_| unreachable!());
        let applied = reconciler
            .dispatch("business-1", 1, &fence())
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(first.attempt_id, "business-1/r1/a0");
        assert_eq!(applied.status, EffectStatus::Applied);
        assert_eq!(
            reconciler
                .enqueue(request())
                .unwrap_or_else(|_| unreachable!())
                .status,
            EffectStatus::Applied
        );
    }

    #[test]
    fn lost_reply_before_and_after_apply_require_probe() {
        let mut before = EffectReconciler::new(FakeEffectTarget::new("email-sink"));
        before.enqueue(request()).unwrap_or_else(|_| unreachable!());
        before
            .target
            .set_apply_outcome(TargetApplyOutcome::LostBeforeApply);
        assert_eq!(
            before
                .dispatch("business-1", 1, &fence())
                .unwrap_or_else(|_| unreachable!())
                .status,
            EffectStatus::Unknown
        );
        before
            .target
            .set_probe_outcome(TargetProbeOutcome::NotApplied);
        assert_eq!(
            before
                .reconcile("business-1", 1, &fence(), &fence())
                .unwrap_or_else(|_| unreachable!())
                .status,
            EffectStatus::Failed
        );

        let mut after = EffectReconciler::new(FakeEffectTarget::new("email-sink"));
        after.enqueue(request()).unwrap_or_else(|_| unreachable!());
        after
            .target
            .set_apply_outcome(TargetApplyOutcome::LostAfterApply);
        assert_eq!(
            after
                .dispatch("business-1", 1, &fence())
                .unwrap_or_else(|_| unreachable!())
                .status,
            EffectStatus::Unknown
        );
        assert_eq!(
            after
                .reconcile("business-1", 1, &fence(), &fence())
                .unwrap_or_else(|_| unreachable!())
                .status,
            EffectStatus::Applied
        );
        assert_eq!(after.target.applied_count(), 1);
    }

    #[test]
    fn stale_revision_fence_target_mismatch_restart_and_irreconcilable_unknown() {
        let req = request();
        let revision_two = ExternalEffectRequest::from_committed_room(
            "business-1".to_owned(),
            2,
            "email-sink".to_owned(),
            req.provenance.clone(),
            EffectPreconditions {
                work_revision: 2,
                ..req.preconditions.clone()
            },
            req.payload.clone(),
        )
        .unwrap_or_else(|_| unreachable!());
        let mut revisions = EffectReconciler::new(FakeEffectTarget::new("email-sink"));
        revisions
            .enqueue(revision_two)
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(
            revisions.enqueue(req.clone()),
            Err(EffectError::StaleRevision)
        );

        let mut reconciler = EffectReconciler::new(FakeEffectTarget::new("other-target"));
        reconciler
            .enqueue(req.clone())
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(
            reconciler.dispatch(
                "business-1",
                1,
                &CurrentEffectFence {
                    room_seq: 41,
                    ..fence()
                }
            ),
            Err(EffectError::StaleFence)
        );
        assert_eq!(
            reconciler.dispatch("business-1", 1, &fence()),
            Err(EffectError::TargetMismatch)
        );

        let mut stale_revision = EffectReconciler::new(FakeEffectTarget::new("email-sink"));
        stale_revision
            .enqueue(request())
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(
            stale_revision.dispatch(
                "business-1",
                1,
                &CurrentEffectFence {
                    work_revision: 2,
                    ..fence()
                }
            ),
            Err(EffectError::StaleFence)
        );

        let mut unknown = EffectReconciler::new(FakeEffectTarget::new("email-sink"));
        unknown.enqueue(req).unwrap_or_else(|_| unreachable!());
        unknown
            .target
            .set_apply_outcome(TargetApplyOutcome::LostBeforeApply);
        unknown
            .dispatch("business-1", 1, &fence())
            .unwrap_or_else(|_| unreachable!());
        unknown
            .target
            .set_probe_outcome(TargetProbeOutcome::Unknown);
        assert_eq!(
            unknown
                .reconcile("business-1", 1, &fence(), &fence())
                .unwrap_or_else(|_| unreachable!())
                .status,
            EffectStatus::Unknown
        );
        assert_eq!(unknown.status("business-1", 1), Some(EffectStatus::Unknown));

        let snapshots = unknown.snapshot();
        let mut restarted =
            EffectReconciler::from_snapshot(FakeEffectTarget::new("email-sink"), snapshots)
                .unwrap_or_else(|_| unreachable!());
        assert_eq!(
            restarted.status("business-1", 1),
            Some(EffectStatus::Unknown)
        );
        restarted
            .target
            .set_probe_outcome(TargetProbeOutcome::NotApplied);
        assert_eq!(
            restarted
                .reconcile("business-1", 1, &fence(), &fence())
                .unwrap_or_else(|_| unreachable!())
                .status,
            EffectStatus::Failed
        );

        let mut cancelled = EffectReconciler::new(FakeEffectTarget::new("email-sink"));
        cancelled
            .enqueue(request())
            .unwrap_or_else(|_| unreachable!());
        cancelled
            .target
            .set_apply_outcome(TargetApplyOutcome::LostAfterApply);
        cancelled
            .dispatch("business-1", 1, &fence())
            .unwrap_or_else(|_| unreachable!());
        let cancelled_fence = CurrentEffectFence {
            cancelled: true,
            ..fence()
        };
        assert_eq!(
            cancelled.reconcile("business-1", 1, &fence(), &cancelled_fence),
            Err(EffectError::Cancelled)
        );
        assert_eq!(
            cancelled.status("business-1", 1),
            Some(EffectStatus::Unknown)
        );

        let mut stale_executor = EffectReconciler::new(FakeEffectTarget::new("email-sink"));
        stale_executor
            .enqueue(request())
            .unwrap_or_else(|_| unreachable!());
        stale_executor
            .target
            .set_apply_outcome(TargetApplyOutcome::LostAfterApply);
        stale_executor
            .dispatch("business-1", 1, &fence())
            .unwrap_or_else(|_| unreachable!());
        let superseded = CurrentEffectFence {
            work_revision: 2,
            ..fence()
        };
        assert_eq!(
            stale_executor.reconcile("business-1", 1, &fence(), &superseded),
            Err(EffectError::StaleFence)
        );
        assert_eq!(
            stale_executor.status("business-1", 1),
            Some(EffectStatus::Unknown)
        );
        let stale_owner = CurrentEffectFence {
            owner_id: "runner-2".to_owned(),
            ..fence()
        };
        assert_eq!(
            stale_executor.reconcile("business-1", 1, &fence(), &stale_owner),
            Err(EffectError::StaleFence)
        );
    }
}
