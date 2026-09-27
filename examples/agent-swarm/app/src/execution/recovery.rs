//! Durable external-effect evidence and conservative reconciliation.

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_ID_BYTES: usize = 256;

/// Immutable identity and fence for one concrete effect attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectIntent {
    pub operation_id: String,
    pub swarm_id: String,
    pub work_id: String,
    pub work_revision: u64,
    pub owner_member_id: String,
    pub invocation_id: String,
    pub attempt_id: String,
    pub configuration_revision: u64,
    pub execution_epoch: u64,
    pub target_id: String,
    pub request_digest: String,
}

impl EffectIntent {
    pub(crate) fn valid(&self) -> bool {
        [
            self.operation_id.as_str(),
            self.swarm_id.as_str(),
            self.work_id.as_str(),
            self.owner_member_id.as_str(),
            self.invocation_id.as_str(),
            self.attempt_id.as_str(),
            self.target_id.as_str(),
        ]
        .into_iter()
        .all(bounded)
            && self.work_revision > 0
            && self.configuration_revision > 0
            && self.execution_epoch > 0
            && self
                .request_digest
                .strip_prefix("blake3:")
                .is_some_and(|digest| {
                    digest.len() == 64
                        && digest
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                })
    }
}

/// Persisted write-ahead phase.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    Prepared,
    Dispatched,
    Applied,
    Failed,
    Unknown,
    Reconciling,
    Acknowledged,
}

/// Durable effect record. `Dispatched` and `Reconciling` restore as `Unknown`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectRecord {
    pub intent: EffectIntent,
    pub state: EffectState,
}

impl EffectRecord {
    /// Creates the write-ahead intent before target contact.
    ///
    /// # Errors
    /// Rejects incomplete or unstable identities.
    pub fn prepare(intent: EffectIntent) -> Result<Self, RecoveryError> {
        if !intent.valid() {
            return Err(RecoveryError::InvalidIntent);
        }
        Ok(Self {
            intent,
            state: EffectState::Prepared,
        })
    }

    /// Marks the last durable cut before contacting the target.
    ///
    /// # Errors
    /// Only a prepared effect may be dispatched.
    pub fn mark_dispatched(&mut self) -> Result<(), RecoveryError> {
        if self.state != EffectState::Prepared {
            return Err(RecoveryError::InvalidPhase);
        }
        self.state = EffectState::Dispatched;
        Ok(())
    }

    /// Records a target response. Lost or ambiguous replies stay unknown.
    ///
    /// # Errors
    /// Requires a dispatched or already-unknown operation.
    pub fn observe(&mut self, outcome: EffectOutcome) -> Result<(), RecoveryError> {
        if !matches!(self.state, EffectState::Dispatched | EffectState::Unknown) {
            return Err(RecoveryError::InvalidPhase);
        }
        self.state = match outcome {
            EffectOutcome::Applied | EffectOutcome::Duplicate => EffectState::Applied,
            EffectOutcome::NotApplied | EffectOutcome::Rejected => EffectState::Failed,
            EffectOutcome::Unknown => EffectState::Unknown,
            EffectOutcome::TargetMismatch => return Err(RecoveryError::TargetMismatch),
        };
        Ok(())
    }

    /// Converts crash-ambiguous phases without contacting the target.
    pub fn restore_after_crash(&mut self) {
        if matches!(
            self.state,
            EffectState::Dispatched | EffectState::Reconciling
        ) {
            self.state = EffectState::Unknown;
        }
    }

    /// Retains local acknowledgement only after a terminal target fact.
    ///
    /// # Errors
    /// Unknown work can never be acknowledged away.
    pub fn acknowledge(&mut self) -> Result<(), RecoveryError> {
        if !matches!(self.state, EffectState::Applied | EffectState::Failed) {
            return Err(RecoveryError::InvalidPhase);
        }
        self.state = EffectState::Acknowledged;
        Ok(())
    }
}

/// Target apply/probe outcome.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectOutcome {
    Applied,
    Duplicate,
    NotApplied,
    Rejected,
    Unknown,
    TargetMismatch,
}

/// Current ownership/configuration fence sampled on both sides of a probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectFence {
    pub swarm_id: String,
    pub work_id: String,
    pub work_revision: u64,
    pub owner_member_id: String,
    pub invocation_id: String,
    pub attempt_id: String,
    pub configuration_revision: u64,
    pub execution_epoch: u64,
    pub eligible: bool,
}

impl EffectFence {
    fn matches(&self, intent: &EffectIntent) -> bool {
        self.eligible
            && self.swarm_id == intent.swarm_id
            && self.work_id == intent.work_id
            && self.work_revision == intent.work_revision
            && self.owner_member_id == intent.owner_member_id
            && self.invocation_id == intent.invocation_id
            && self.attempt_id == intent.attempt_id
            && self.configuration_revision == intent.configuration_revision
            && self.execution_epoch == intent.execution_epoch
    }
}

/// Probe/idempotency boundary for a concrete target.
pub trait EffectProbe {
    fn target_id(&self) -> &str;
    /// Probes the target's idempotency record for this exact intent.
    ///
    /// # Errors
    /// Reports unavailable targets and mismatched target identity without
    /// inferring that an ambiguous effect was absent.
    fn probe(&mut self, intent: &EffectIntent) -> Result<EffectOutcome, RecoveryError>;
}

/// Reconciles an unknown operation under before/after ownership fences.
///
/// Callers using a durable journal must persist `Reconciling` before invoking
/// the target, then persist this function's terminal/unknown result. A crash in
/// between restores `Reconciling` as `Unknown`.
///
/// # Errors
/// Refuses stale ownership, target mismatch, or non-unknown records.
pub fn reconcile_effect(
    record: &mut EffectRecord,
    before: &EffectFence,
    after: &EffectFence,
    probe: &mut impl EffectProbe,
) -> Result<(), RecoveryError> {
    if record.state != EffectState::Unknown {
        return Err(RecoveryError::InvalidPhase);
    }
    if !before.matches(&record.intent) {
        return Err(RecoveryError::StaleFence);
    }
    if probe.target_id() != record.intent.target_id {
        return Err(RecoveryError::TargetMismatch);
    }
    record.state = EffectState::Reconciling;
    let outcome = match probe.probe(&record.intent) {
        Ok(outcome) => outcome,
        Err(RecoveryError::ProbeUnavailable) => EffectOutcome::Unknown,
        Err(error) => {
            record.state = EffectState::Unknown;
            return Err(error);
        }
    };
    if !after.matches(&record.intent) {
        record.state = EffectState::Unknown;
        return Err(RecoveryError::StaleFence);
    }
    record.state = match outcome {
        EffectOutcome::Applied | EffectOutcome::Duplicate => EffectState::Applied,
        EffectOutcome::NotApplied | EffectOutcome::Rejected => EffectState::Failed,
        EffectOutcome::Unknown => EffectState::Unknown,
        EffectOutcome::TargetMismatch => {
            record.state = EffectState::Unknown;
            return Err(RecoveryError::TargetMismatch);
        }
    };
    Ok(())
}

/// Closed recovery failures.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RecoveryError {
    #[error("effect intent is invalid")]
    InvalidIntent,
    #[error("effect is not valid in its current phase")]
    InvalidPhase,
    #[error("effect ownership fence is stale")]
    StaleFence,
    #[error("effect target identity mismatches")]
    TargetMismatch,
    #[error("effect probe is unavailable")]
    ProbeUnavailable,
}

fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_ID_BYTES && !value.contains('\0')
}
