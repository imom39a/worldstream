//! Additive current verification and guarded recovery for Genesis-selected history.
use super::*;
use crate::{CanonicalRoomTrace, GenesisRecord, TransitionRecord};

/// Strict immutable Genesis evidence required by bounded record verification.
#[derive(Clone)]
pub struct VerifiedCanonicalGenesis {
    record: GenesisRecord,
    bytes: Vec<u8>,
}
redacted_debug!(VerifiedCanonicalGenesis);
impl VerifiedCanonicalGenesis {
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, RoomRecoveryErrorV1> {
        let record =
            GenesisRecord::from_canonical_bytes(bytes).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        record
            .check_payload_budget()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        Ok(Self {
            record,
            bytes: bytes.to_vec(),
        })
    }
    #[must_use]
    pub const fn record(&self) -> &GenesisRecord {
        &self.record
    }
    #[must_use]
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.bytes
    }
}
/// Record-only commitment evidence. It does not carry opaque Activity or
/// establish historical semantic execution.
#[derive(Clone)]
pub struct VerifiedCanonicalLineageRecord {
    head: CompleteHeadV1,
    previous: Option<Blake3DigestV1>,
}
redacted_debug!(VerifiedCanonicalLineageRecord);
impl VerifiedCanonicalLineageRecord {
    pub fn verify_for_storage(
        genesis: &VerifiedCanonicalGenesis,
        expected_head: &CompleteHeadV1,
        bytes: &[u8],
    ) -> Result<Self, RoomRecoveryErrorV1> {
        if genesis.record.room_id() != expected_head.room_id()
            || genesis.record.pack_digest() != expected_head.pack_digest()
            || genesis.record.core_schema_version() != expected_head.core_schema_version()
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let (head, previous) = if expected_head.room_seq().get() == 0 {
            if bytes != genesis.bytes {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            (genesis.record.complete_head(), None)
        } else {
            let record = genesis
                .record
                .decode_transition(bytes)
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
            genesis
                .record
                .check_transition_payload_budget(&record, bytes)
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
            (
                record.complete_head(),
                Some(record.previous_lineage_hash().clone()),
            )
        };
        if &head != expected_head {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        Ok(Self { head, previous })
    }
    #[must_use]
    pub const fn head(&self) -> &CompleteHeadV1 {
        &self.head
    }
    #[must_use]
    pub const fn previous_lineage_hash(&self) -> Option<&Blake3DigestV1> {
        self.previous.as_ref()
    }
    pub fn verify_successor(
        &self,
        predecessor: &CompleteHeadV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        if self.head.room_id() != predecessor.room_id()
            || self.head.pack_digest() != predecessor.pack_digest()
            || self.head.core_schema_version() != predecessor.core_schema_version()
            || predecessor.room_seq().checked_successor().ok() != Some(self.head.room_seq())
            || self.previous.as_ref() != Some(predecessor.genesis_or_transition_hash())
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        Ok(())
    }
}
/// Current complete serving materializations checked against one immutable
/// record. Verification has no historical reduction or Pack callback.
#[derive(Clone)]
pub struct VerifiedCanonicalCurrentRoomMaterialization {
    core: CoreRoomStateV1,
    activity: CanonicalJsonV1,
    memberships: Vec<PreparedMembershipMaterializationV1>,
    record: VerifiedCanonicalLineageRecord,
}
redacted_debug!(VerifiedCanonicalCurrentRoomMaterialization);
impl VerifiedCanonicalCurrentRoomMaterialization {
    pub fn verify_for_storage(
        genesis: &VerifiedCanonicalGenesis,
        expected_head: &CompleteHeadV1,
        current_record_bytes: &[u8],
        core_bytes: &[u8],
        activity_bytes: &[u8],
    ) -> Result<Self, RoomRecoveryErrorV1> {
        let record = VerifiedCanonicalLineageRecord::verify_for_storage(
            genesis,
            expected_head,
            current_record_bytes,
        )?;
        let core = CanonicalJsonV1::decode_canonical::<CoreRoomStateV1>(core_bytes)
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        let checked = crate::CoreReducerV1::new(|_| Ok(()))
            .validate_state(core.clone())
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        let activity = CanonicalJsonV1::from_canonical_bytes(activity_bytes)
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if checked.core_state_hash() != expected_head.core_state_hash()
            || crate::lineage::hash_activity_state(expected_head.pack_digest(), &activity)
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
                != *expected_head.activity_state_hash()
            || crate::lineage::hash_authoritative_state(
                expected_head.pack_digest(),
                expected_head.core_state_hash(),
                expected_head.activity_state_hash(),
            )
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
                != *expected_head.authoritative_state_hash()
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        if genesis.record.format() == crate::CanonicalHistoryFormat::V2 {
            crate::PAYLOAD_BUDGET_V1
                .check_authoritative_state(&core, &activity)
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        }
        let memberships = core
            .memberships()
            .values()
            .map(|membership| {
                Ok(PreparedMembershipMaterializationV1 {
                    membership: membership.clone(),
                    canonical_membership_bytes: encode(membership)
                        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?,
                })
            })
            .collect::<Result<Vec<_>, RoomRecoveryErrorV1>>()?;
        Ok(Self {
            core,
            activity,
            memberships,
            record,
        })
    }
    #[must_use]
    pub fn room_status(&self) -> RoomStatusV1 {
        self.core.room_status()
    }
    #[must_use]
    pub const fn core_state(&self) -> &CoreRoomStateV1 {
        &self.core
    }
    #[must_use]
    pub const fn activity_state(&self) -> &CanonicalJsonV1 {
        &self.activity
    }
    #[must_use]
    pub fn memberships(&self) -> &[PreparedMembershipMaterializationV1] {
        &self.memberships
    }
    #[must_use]
    pub const fn previous_lineage_hash(&self) -> Option<&Blake3DigestV1> {
        self.record.previous_lineage_hash()
    }
    #[must_use]
    pub const fn head(&self) -> &CompleteHeadV1 {
        self.record.head()
    }
}

/// Byte-neutral candidate/checkpoint containers retain the existing storage
/// capture semantics. This separate port accepts either Genesis format.
pub trait CanonicalRoomRecoveryStorage: Send + Sync {
    fn inspect_recovery_candidate(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<RoomRecoveryCandidateV1>, RoomRecoveryErrorV1>;
    fn inspect_full_recovery_candidate(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<RoomRecoveryCandidateV1>, RoomRecoveryErrorV1>;
    fn guard_recovery_install(
        &self,
        room_id: &RoomId,
        expected_head: &CompleteHeadV1,
        expected_integrity_generation: IntegrityGenerationV1,
        recovered: &RecoveredRoomMaterializationsV1,
    ) -> Result<(), RoomRecoveryErrorV1>;
    fn record_recovery_failure(
        &self,
        room_id: &RoomId,
        expected_head: &CompleteHeadV1,
        expected_integrity_generation: IntegrityGenerationV1,
        disposition: RecoveryIntegrityDispositionV1,
    ) -> Result<(), RoomRecoveryErrorV1>;
}
pub struct RecoveredCanonicalRoomExecution {
    trace: CanonicalRoomTrace,
    receipt: RoomRecoveryExecutionReceiptV1,
}
impl RecoveredCanonicalRoomExecution {
    #[must_use]
    pub const fn trace(&self) -> &CanonicalRoomTrace {
        &self.trace
    }
    #[must_use]
    pub const fn receipt(&self) -> RoomRecoveryExecutionReceiptV1 {
        self.receipt
    }
    #[must_use]
    pub fn into_trace(self) -> CanonicalRoomTrace {
        self.trace
    }
}

/// Timer-generation witness fold shared by full and paginated canonical Replay.
pub(crate) struct CanonicalTimerLedger {
    rows: BTreeMap<(TimerId, TimerGenerationV1), RecoveredTimerMaterializationV1>,
    scheduled: BTreeMap<TimerId, TimerGenerationV1>,
    last_generation: BTreeMap<TimerId, TimerGenerationV1>,
}
impl CanonicalTimerLedger {
    pub(crate) fn begin(
        genesis: &GenesisRecord,
        checkpoint: Option<&RoomRecoveryCheckpointV1>,
    ) -> Result<Self, RoomRecoveryErrorV1> {
        let mut ledger = Self {
            rows: BTreeMap::new(),
            scheduled: BTreeMap::new(),
            last_generation: BTreeMap::new(),
        };
        let rows = if let Some(checkpoint) = checkpoint {
            checkpoint.timers().to_vec()
        } else {
            genesis
                .initial_timers()
                .iter()
                .map(|timer| {
                    Ok(RecoveredTimerMaterializationV1::new(
                        timer.timer_id.clone(),
                        timer.generation,
                        timer.scheduled_for.clone(),
                        timer
                            .canonical_payload
                            .to_bytes()
                            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?,
                        RecoveredTimerStateV1::Scheduled,
                    ))
                })
                .collect::<Result<Vec<_>, RoomRecoveryErrorV1>>()?
        };
        for row in rows {
            if ledger
                .rows
                .insert((row.timer_id().clone(), row.generation()), row.clone())
                .is_some()
                || ledger
                    .last_generation
                    .insert(row.timer_id().clone(), row.generation())
                    .is_some_and(|generation| generation >= row.generation())
                || (row.state() == RecoveredTimerStateV1::Scheduled
                    && ledger
                        .scheduled
                        .insert(row.timer_id().clone(), row.generation())
                        .is_some())
            {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
        }
        Ok(ledger)
    }
    pub(crate) fn consume(&mut self, record: &TransitionRecord) -> Result<(), RoomRecoveryErrorV1> {
        // Stage only touched generation rows. Historical Timer generations must
        // not be copied for every Transition in a long paginated history.
        let mut keys = std::collections::BTreeSet::new();
        if let RecordedStimulusV1::TimerFired(fired) = record.recorded_stimulus() {
            keys.insert((fired.timer_id.clone(), fired.generation));
        }
        for change in record.ordered_timer_changes() {
            match change {
                TimerChangeV1::Schedule {
                    timer_id,
                    generation,
                    ..
                }
                | TimerChangeV1::Cancel {
                    timer_id,
                    generation,
                } => {
                    keys.insert((timer_id.clone(), *generation));
                }
                TimerChangeV1::Reschedule {
                    timer_id,
                    previous_generation,
                    generation,
                    ..
                } => {
                    keys.insert((timer_id.clone(), *previous_generation));
                    keys.insert((timer_id.clone(), *generation));
                }
            }
        }
        let mut staged = Self {
            rows: keys
                .into_iter()
                .filter_map(|key| self.rows.get(&key).cloned().map(|row| (key, row)))
                .collect(),
            scheduled: self.scheduled.clone(),
            last_generation: self.last_generation.clone(),
        };
        staged.consume_in_place(record)?;
        self.rows.extend(staged.rows);
        self.scheduled = staged.scheduled;
        self.last_generation = staged.last_generation;
        Ok(())
    }
    fn consume_in_place(&mut self, record: &TransitionRecord) -> Result<(), RoomRecoveryErrorV1> {
        if let RecordedStimulusV1::TimerFired(fired) = record.recorded_stimulus() {
            if self.scheduled.get(&fired.timer_id) != Some(&fired.generation) {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            let row = self
                .rows
                .get_mut(&(fired.timer_id.clone(), fired.generation))
                .ok_or(RoomRecoveryErrorV1::Corrupt)?;
            if row.state() != RecoveredTimerStateV1::Scheduled
                || row.scheduled_for() != &fired.scheduled_for
                || row.canonical_payload_bytes()
                    != fired
                        .canonical_payload
                        .to_bytes()
                        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            *row = RecoveredTimerMaterializationV1::new(
                row.timer_id().clone(),
                row.generation(),
                row.scheduled_for().clone(),
                row.canonical_payload_bytes().to_vec(),
                RecoveredTimerStateV1::Fired,
            );
            self.scheduled.remove(&fired.timer_id);
        }
        for change in record.ordered_timer_changes() {
            apply_recovered_timer_change(
                &mut self.rows,
                &mut self.scheduled,
                &mut self.last_generation,
                record.recorded_stimulus(),
                change,
            )?;
        }
        Ok(())
    }
    pub(crate) fn materializations(&self) -> Vec<RecoveredTimerMaterializationV1> {
        self.rows.values().cloned().collect()
    }
    pub(crate) fn finish(self) -> Vec<RecoveredTimerMaterializationV1> {
        self.rows.into_values().collect()
    }
}

fn canonical_records(
    candidate: &RoomRecoveryCandidateV1,
) -> Result<(GenesisRecord, Vec<TransitionRecord>), RoomRecoveryErrorV1> {
    let genesis = GenesisRecord::from_canonical_bytes(&candidate.canonical_genesis_bytes)
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let records = candidate
        .canonical_transition_bytes
        .iter()
        .map(|bytes| {
            genesis
                .decode_transition(bytes)
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((genesis, records))
}
fn preflight_canonical_candidate(
    candidate: &RoomRecoveryCandidateV1,
) -> Result<(), RoomRecoveryErrorV1> {
    let mut preflight =
        crate::CanonicalStorageHistoryPreflight::begin(&candidate.canonical_genesis_bytes)
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    preflight
        .consume_transition_page(&candidate.canonical_transition_bytes)
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    if preflight.final_head() != &candidate.head
        || candidate
            .canonical_core_state_bytes
            .as_ref()
            .is_some_and(|bytes| {
                preflight
                    .structural_state()
                    .core_state()
                    .canonical_bytes()
                    .ok()
                    .as_ref()
                    != Some(bytes)
            })
    {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    if let Some(bytes) = candidate.canonical_activity_state_bytes.as_deref() {
        preflight
            .finish()
            .with_activity_materialization(bytes)
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    }
    Ok(())
}
fn recover_canonical_materializations(
    candidate: &RoomRecoveryCandidateV1,
    report: &crate::CanonicalReplayReport,
) -> Result<RecoveredRoomMaterializationsV1, RoomRecoveryErrorV1> {
    let (genesis, records) = canonical_records(candidate)?;
    let mut ledger = CanonicalTimerLedger::begin(&genesis, candidate.checkpoint())?;
    let mut decisions = Vec::new();
    for record in records {
        ledger.consume(&record)?;
        for decision in prepare_activation_decisions_from_effects(
            record.room_seq(),
            record.ordered_attention_signals(),
        )
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
        {
            decisions.push(RecoveredActivationDecisionV1::new(
                record.room_seq(),
                decision.decision_id().to_owned(),
                decision.target_member_id().cloned(),
                decision.canonical_decision_bytes().to_vec(),
            ));
        }
    }
    recover_materializations_from_facts(
        candidate,
        report.final_state(),
        report.observation_consequences(),
        ledger.finish(),
        Some(report.membership_generations().clone()),
        decisions,
    )
}

pub fn recover_canonical_room_from_storage(
    storage: &dyn CanonicalRoomRecoveryStorage,
    registry: &PackRegistryV1,
    room_id: &RoomId,
) -> Result<Option<CanonicalRoomTrace>, RoomRecoveryErrorV1> {
    recover_canonical_room_from_storage_with_receipt(storage, registry, room_id)
        .map(|execution| execution.map(RecoveredCanonicalRoomExecution::into_trace))
}

/// Recovers one Room through the ordinary trusted storage SPI and returns a
/// receipt for the path that actually completed. This is a qualification seam;
/// serving callers should use [`recover_canonical_room_from_storage`].
///
/// # Errors
///
/// Returns the same closed recovery errors as [`recover_canonical_room_from_storage`].
pub fn recover_canonical_room_from_storage_with_receipt(
    storage: &dyn CanonicalRoomRecoveryStorage,
    registry: &PackRegistryV1,
    room_id: &RoomId,
) -> Result<Option<RecoveredCanonicalRoomExecution>, RoomRecoveryErrorV1> {
    let Some(candidate) = storage.inspect_recovery_candidate(room_id)? else {
        return Ok(None);
    };
    let used_checkpoint = candidate.has_checkpoint();
    match recover_canonical_room_candidate(storage, registry, room_id, &candidate, !used_checkpoint)
    {
        Ok(execution) => Ok(Some(execution)),
        Err(
            error @ (RoomRecoveryErrorV1::Corrupt
            | RoomRecoveryErrorV1::RuntimeUnavailable
            | RoomRecoveryErrorV1::RuntimeFault),
        ) if used_checkpoint => {
            let Some(fallback) = storage.inspect_full_recovery_candidate(room_id)? else {
                return Err(RoomRecoveryErrorV1::ConcurrentChange);
            };
            if fallback.has_checkpoint() {
                return Err(error);
            }
            recover_canonical_room_candidate(storage, registry, room_id, &fallback, true).map(Some)
        }
        Err(error) => Err(error),
    }
}

/// Replays the canonical Genesis-to-Head candidate even when a disposable
/// checkpoint is available. Trusted storage maintenance uses this to create a
/// new checkpoint only after the entire retained lineage and the current
/// materializations have passed the ordinary guarded recovery fence.
///
/// # Errors
///
/// Returns a closed recovery error if the canonical candidate cannot be read,
/// replayed, or installed at its exact durable fence.
pub fn recover_canonical_room_from_full_storage(
    storage: &dyn CanonicalRoomRecoveryStorage,
    registry: &PackRegistryV1,
    room_id: &RoomId,
) -> Result<Option<CanonicalRoomTrace>, RoomRecoveryErrorV1> {
    let Some(candidate) = storage.inspect_full_recovery_candidate(room_id)? else {
        return Ok(None);
    };
    if candidate.has_checkpoint() {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    recover_canonical_room_candidate(storage, registry, room_id, &candidate, true)
        .map(|execution| Some(execution.into_trace()))
}

#[allow(clippy::too_many_lines)]
fn recover_canonical_room_candidate(
    storage: &dyn CanonicalRoomRecoveryStorage,
    registry: &PackRegistryV1,
    room_id: &RoomId,
    candidate: &RoomRecoveryCandidateV1,
    record_failure: bool,
) -> Result<RecoveredCanonicalRoomExecution, RoomRecoveryErrorV1> {
    if candidate.head.room_id() != room_id {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    let verified = (|| {
        PackRevisionLockV1::from_canonical_bytes(
            &candidate.canonical_pack_revision_lock_bytes,
            candidate.head.pack_digest(),
        )
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if candidate
            .head
            .canonical_bytes()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            != candidate.canonical_head_bytes
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let report = if let Some(checkpoint) = candidate.checkpoint() {
            CanonicalRoomTrace::replay_checkpoint_for_recovery(
                registry,
                &candidate.canonical_genesis_bytes,
                checkpoint,
                &candidate.canonical_transition_bytes,
            )
        } else {
            preflight_canonical_candidate(candidate)?;
            CanonicalRoomTrace::replay_registry_canonical(
                registry,
                &candidate.canonical_genesis_bytes,
                &candidate.canonical_transition_bytes,
                true,
            )
        }
        .map_err(|failure| {
            if failure.class == ReplayFailureClassV1::RuntimeUnavailable
                && failure.last_verified_head.as_deref() == Some(&candidate.head)
            {
                RoomRecoveryErrorV1::RuntimeUnavailable
            } else if failure.class == ReplayFailureClassV1::RuntimeFault {
                RoomRecoveryErrorV1::RuntimeFault
            } else {
                RoomRecoveryErrorV1::Corrupt
            }
        })?;
        let replayed_revision_lock_bytes = report
            .retained_pack_revision_lock()
            .canonical_bytes()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if report.final_head != candidate.head
            || report
                .final_head
                .canonical_bytes()
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
                != candidate.canonical_head_bytes
            || replayed_revision_lock_bytes != candidate.canonical_pack_revision_lock_bytes
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let recovered_materializations = recover_canonical_materializations(candidate, &report)?;
        Ok((report, recovered_materializations))
    })();
    let (report, recovered_materializations) = match verified {
        Ok(value) => value,
        Err(RoomRecoveryErrorV1::Corrupt) => {
            if !record_failure {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            storage.record_recovery_failure(
                room_id,
                &candidate.head,
                candidate.integrity_generation,
                RecoveryIntegrityDispositionV1::Quarantined,
            )?;
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        Err(RoomRecoveryErrorV1::RuntimeUnavailable) => {
            if !record_failure {
                return Err(RoomRecoveryErrorV1::RuntimeUnavailable);
            }
            storage.record_recovery_failure(
                room_id,
                &candidate.head,
                candidate.integrity_generation,
                RecoveryIntegrityDispositionV1::Faulted,
            )?;
            return Err(RoomRecoveryErrorV1::RuntimeUnavailable);
        }
        Err(RoomRecoveryErrorV1::RuntimeFault) => {
            if !record_failure {
                return Err(RoomRecoveryErrorV1::RuntimeFault);
            }
            storage.record_recovery_failure(
                room_id,
                &candidate.head,
                candidate.integrity_generation,
                RecoveryIntegrityDispositionV1::Faulted,
            )?;
            return Err(RoomRecoveryErrorV1::RuntimeFault);
        }
        Err(error) => return Err(error),
    };
    storage.guard_recovery_install(
        room_id,
        &candidate.head,
        candidate.integrity_generation,
        &recovered_materializations,
    )?;
    Ok(RecoveredCanonicalRoomExecution {
        trace: report.into_trace(),
        receipt: RoomRecoveryExecutionReceiptV1::for_candidate(candidate)?,
    })
}

#[cfg(test)]
#[path = "canonical_recovery_tests.rs"]
mod tests;
