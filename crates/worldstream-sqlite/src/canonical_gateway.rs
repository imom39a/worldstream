//! Production canonical serving executors and commit preparation.

use super::{
    MAX_GATEWAY_SNAPSHOT_RECOVERY_RETRIES, RoomHistoryInspectionV1, SqliteActivationClaimV1,
    SqliteActivationErrorV1, SqliteGatewayErrorV1, SqliteParticipantActionErrorV1,
    SqlitePreparedWrite, SqliteRecoveryPhaseV1, SqliteRoomRecoveryErrorV1,
    SqliteRoomRuntimeStateV1, SqliteRoomStore, SqliteTelemetryEventV1, SqliteTimerCommitErrorV1,
    SqliteTimerErrorV1, WriterCommand, map_gateway_inspection_error, map_gateway_recovery_error,
    map_participant_action_gateway_error, map_participant_action_preparation_error,
    map_participant_action_trace_error, map_timer_commit_gateway_error,
    map_timer_commit_preparation_error, map_timer_commit_trace_error,
    participant_action_request_parts, prepare_activation_claim_readonly_from_trace,
};
use std::{collections::BTreeMap, sync::mpsc};
use worldstream_core::{
    ActionAdmittedAt, ActivationOperationRequestV1, AuthorizedCoreAdministrationV1,
    AuthorizedExternalInputV1, AuthorizedRunnerControlV1, AuthorizedTimerFiredV1, CompleteHeadV1,
    CoreAdministrationRequestV1, CoreRecordedAt, CoreRoomStateV1, CoreTraceV1, ExternalInputV1,
    HostClockSampleV1, HostClockV1, IntegrityGenerationV1, MemberId, PackRegistryV1, PackViewerV1,
    ParticipantActionAuthorityV1, ParticipantActionRequestV1, ParticipantActionV1,
    PreparedAuthorityChangeV1, RecordedStimulusV1, ReplayFailureClassV1, RoomCommitResolutionV1,
    RoomId, RoomIntegrityStateV1, RoomIntegrityStatusV1, RoomRecoveryErrorV1, RoomSequenceV1,
    TimerFiredRequestV1, TimerFiredV1, TraceErrorV1, TransitionId, ValidatedPackViewV1,
};
use worldstream_core::{
    AuthorityErrorV1, AuthorizedReceiptReadV1, CanonicalRoomTrace, PreparedCanonicalRoomCommit,
    PreparedCanonicalRoomWrite, ResolutionStatusV1, ResolveOutcomeV1,
    commit_canonical_existing_room, recover_canonical_room_from_storage,
};

pub(super) trait SqliteServingTrace {
    fn head(&self) -> &CompleteHeadV1;
    fn core_state(&self) -> &CoreRoomStateV1;
    fn view(&self, viewer: &PackViewerV1) -> Result<ValidatedPackViewV1, TraceErrorV1>;
}

impl SqliteServingTrace for CoreTraceV1 {
    fn head(&self) -> &CompleteHeadV1 {
        self.head()
    }
    fn core_state(&self) -> &CoreRoomStateV1 {
        self.core_state()
    }
    fn view(&self, viewer: &PackViewerV1) -> Result<ValidatedPackViewV1, TraceErrorV1> {
        let retained = self
            .retained_pack()
            .ok_or(TraceErrorV1::InvalidPreparedAdvance)?;
        retained
            .host()
            .view(&worldstream_core::ViewInputV1 {
                core: self.core_state(),
                activity_state: self.activity_state(),
                complete_head: self.head(),
                viewer,
            })
            .map_err(|_| TraceErrorV1::InvalidPreparedAdvance)
    }
}

impl SqliteServingTrace for CanonicalRoomTrace {
    fn head(&self) -> &CompleteHeadV1 {
        self.head()
    }
    fn core_state(&self) -> &CoreRoomStateV1 {
        self.core_state()
    }
    fn view(&self, viewer: &PackViewerV1) -> Result<ValidatedPackViewV1, TraceErrorV1> {
        self.view(viewer)
    }
}

/// An exact recovered executor and durable current observation barrier.
pub struct SqliteCanonicalGatewayRoomSnapshot {
    pub(super) trace: CanonicalRoomTrace,
    pub(super) integrity: RoomIntegrityStateV1,
    pub(super) frame_heads: BTreeMap<MemberId, u64>,
}

impl SqliteCanonicalGatewayRoomSnapshot {
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        CanonicalRoomTrace,
        RoomIntegrityStateV1,
        BTreeMap<MemberId, u64>,
    ) {
        (self.trace, self.integrity, self.frame_heads)
    }
    #[must_use]
    pub const fn trace(&self) -> &CanonicalRoomTrace {
        &self.trace
    }
    #[must_use]
    pub const fn integrity(&self) -> &RoomIntegrityStateV1 {
        &self.integrity
    }
    #[must_use]
    pub fn frame_heads(&self) -> &BTreeMap<MemberId, u64> {
        &self.frame_heads
    }
    #[must_use]
    pub fn integrity_generation(&self) -> IntegrityGenerationV1 {
        self.integrity.generation()
    }
}

/// A verified canonical executor gated by one fixed Timer cutoff.
pub struct SqliteCanonicalRoomRecovery {
    trace: CanonicalRoomTrace,
    cutoff: HostClockSampleV1,
    state: SqliteRoomRuntimeStateV1,
}

impl SqliteCanonicalRoomRecovery {
    #[must_use]
    pub const fn state(&self) -> SqliteRoomRuntimeStateV1 {
        self.state
    }
    #[must_use]
    pub const fn cutoff(&self) -> &HostClockSampleV1 {
        &self.cutoff
    }
    #[must_use]
    pub const fn trace(&self) -> &CanonicalRoomTrace {
        &self.trace
    }
    #[must_use]
    pub const fn trace_mut(&mut self) -> &mut CanonicalRoomTrace {
        &mut self.trace
    }
    /// Reads the next due obligation under the original recovery cutoff.
    /// # Errors
    /// Returns the existing closed Timer storage failure.
    pub fn next_due_timer(
        &mut self,
        store: &SqliteRoomStore,
    ) -> Result<Option<TimerFiredRequestV1>, SqliteTimerErrorV1> {
        if self.state != SqliteRoomRuntimeStateV1::CatchingUp {
            return Ok(None);
        }
        let candidate = store.next_due_timer(self.trace.head().room_id(), &self.cutoff)?;
        if candidate.is_none() {
            self.state = SqliteRoomRuntimeStateV1::Active;
        }
        Ok(candidate)
    }
}

impl SqliteRoomStore {
    fn resolve_canonical_admission_receipt(
        &self,
        authority: AuthorizedReceiptReadV1,
    ) -> Result<Option<RoomCommitResolutionV1>, AuthorityErrorV1> {
        Ok(match self.resolve_authorized(authority)? {
            ResolveOutcomeV1::StoredResolution(result) => Some(RoomCommitResolutionV1::resolved(
                ResolutionStatusV1::Existing,
                *result,
            )),
            ResolveOutcomeV1::Conflict {
                existing_request_hash,
            } => Some(RoomCommitResolutionV1::Conflict {
                existing_request_hash,
            }),
            ResolveOutcomeV1::KnownAbsent => None,
            ResolveOutcomeV1::ResolutionUnavailable => Some(RoomCommitResolutionV1::Indeterminate),
        })
    }

    /// Prepares exact private Activation context from the canonical executor.
    /// # Errors
    /// Returns closed preparation, authority, integrity or context-bound failures.
    pub fn prepare_canonical_activation_claim(
        &self,
        registry: &PackRegistryV1,
        authority: &AuthorizedRunnerControlV1,
        request: ActivationOperationRequestV1,
    ) -> Result<SqliteActivationClaimV1, SqliteActivationErrorV1> {
        let snapshot = self
            .gateway_canonical_room_snapshot(registry, &authority.target().room_id)
            .map_err(|_| SqliteActivationErrorV1::StaleContext)?
            .ok_or(SqliteActivationErrorV1::Fenced)?;
        self.prepare_activation_claim_from_canonical_serving_trace(
            registry,
            authority,
            request,
            snapshot.trace(),
            snapshot.integrity(),
        )
    }

    /// Prepares context while borrowing the uniquely owned canonical executor.
    /// # Errors
    /// Returns the existing closed preparation, authority or context-bound failure.
    pub fn prepare_activation_claim_from_canonical_serving_trace(
        &self,
        registry: &PackRegistryV1,
        authority: &AuthorizedRunnerControlV1,
        request: ActivationOperationRequestV1,
        trace: &CanonicalRoomTrace,
        integrity: &RoomIntegrityStateV1,
    ) -> Result<SqliteActivationClaimV1, SqliteActivationErrorV1> {
        if trace.head().room_id() != &authority.target().room_id {
            return Err(SqliteActivationErrorV1::Fenced);
        }
        prepare_activation_claim_readonly_from_trace(
            &self.writer.database_file,
            &self.writer.path,
            registry,
            authority,
            request,
            trace,
            integrity.generation(),
        )
    }

    /// Commits canonical creation and approved authority changes atomically.
    #[must_use]
    pub fn commit_canonical_creation_with_authority(
        &self,
        prepared: &PreparedCanonicalRoomWrite,
        changes: Vec<PreparedAuthorityChangeV1>,
    ) -> RoomCommitResolutionV1 {
        let Ok(prepared) = SqlitePreparedWrite::from_canonical(prepared) else {
            return RoomCommitResolutionV1::Fault;
        };
        let (reply, receive) = mpsc::channel();
        if self
            .writer
            .commands
            .send(WriterCommand::CommitCreationWithAuthority {
                prepared: Box::new(prepared),
                changes,
                reply,
            })
            .is_err()
        {
            return RoomCommitResolutionV1::Indeterminate;
        }
        receive
            .recv()
            .unwrap_or(RoomCommitResolutionV1::Indeterminate)
    }

    /// Resolves the original durable receipt before preparing Core administration.
    /// Fresh work uses the owned current canonical executor.
    /// # Errors
    /// Returns the existing preparation, integrity, or storage failure.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_authorized_core_administration_from_canonical_serving_trace(
        &self,
        authority: AuthorizedCoreAdministrationV1,
        request: &CoreAdministrationRequestV1,
        recorded_at: CoreRecordedAt,
        transition_id: TransitionId,
        trace: &mut CanonicalRoomTrace,
        integrity: &RoomIntegrityStateV1,
        frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<RoomCommitResolutionV1, SqliteTimerCommitErrorV1> {
        let receipt_authority =
            AuthorizedReceiptReadV1::from_core_administration_authority(&authority, request)
                .map_err(|_| SqliteTimerCommitErrorV1::Rejected)?;
        match self.resolve_canonical_admission_receipt(receipt_authority) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {}
            Err(AuthorityErrorV1::Unavailable) => {
                return Err(SqliteTimerCommitErrorV1::StorageUnavailable);
            }
            Err(_) => return Ok(RoomCommitResolutionV1::Fenced),
        }
        if trace.head().room_id() != request.room_id() {
            return Err(SqliteTimerCommitErrorV1::RoomUnavailable);
        }
        if integrity.status() == RoomIntegrityStatusV1::Faulted {
            return Err(SqliteTimerCommitErrorV1::RoomFaulted);
        }
        if integrity.status() == RoomIntegrityStatusV1::Quarantined {
            return Err(SqliteTimerCommitErrorV1::IntegrityUnavailable);
        }
        let prepared = PreparedCanonicalRoomCommit::for_authorized_core_administration(
            trace,
            request,
            recorded_at,
            transition_id,
            integrity.generation(),
            authority,
            frame_heads,
        )
        .map_err(map_timer_commit_preparation_error)?;
        Ok(commit_canonical_existing_room(self, trace, prepared)
            .into_parts()
            .0)
    }

    /// Recovers a canonical executor through exact full or checkpoint Replay.
    /// # Errors
    /// Returns the existing closed storage, runtime, integrity or fence failure.
    pub fn recover_canonical_room(
        &self,
        registry: &PackRegistryV1,
        room_id: &RoomId,
    ) -> Result<Option<CanonicalRoomTrace>, RoomRecoveryErrorV1> {
        self.emit_telemetry(SqliteTelemetryEventV1::Recovery {
            phase: SqliteRecoveryPhaseV1::Started,
        });
        let result = recover_canonical_room_from_storage(self, registry, room_id);
        self.emit_telemetry(SqliteTelemetryEventV1::Recovery {
            phase: if result.is_ok() {
                SqliteRecoveryPhaseV1::Completed
            } else {
                SqliteRecoveryPhaseV1::Failed
            },
        });
        result
    }

    /// Captures one cutoff and retains the same fixed-cutoff Timer drain gate.
    /// # Errors
    /// Returns recovery, clock, or durable Timer failures.
    pub fn recover_canonical_room_catching_up(
        &self,
        registry: &PackRegistryV1,
        room_id: &RoomId,
        clock: &dyn HostClockV1,
    ) -> Result<Option<SqliteCanonicalRoomRecovery>, SqliteRoomRecoveryErrorV1> {
        let Some(trace) = self.recover_canonical_room(registry, room_id)? else {
            return Ok(None);
        };
        let cutoff = clock.sample().map_err(SqliteRoomRecoveryErrorV1::Clock)?;
        let state = if self.next_due_timer(room_id, &cutoff)?.is_some() {
            SqliteRoomRuntimeStateV1::CatchingUp
        } else {
            SqliteRoomRuntimeStateV1::Active
        };
        Ok(Some(SqliteCanonicalRoomRecovery {
            trace,
            cutoff,
            state,
        }))
    }

    /// Returns a uniquely owned canonical executor under the current serving fence.
    /// Faulted reads use exact read-only Replay and do not repair integrity.
    /// # Errors
    /// Returns closed recovery, integrity, corruption, or concurrent-change failure.
    pub fn gateway_canonical_room_snapshot(
        &self,
        registry: &PackRegistryV1,
        room_id: &RoomId,
    ) -> Result<Option<SqliteCanonicalGatewayRoomSnapshot>, SqliteGatewayErrorV1> {
        if self
            .room_integrity_state(room_id)?
            .is_some_and(|integrity| integrity.status() == RoomIntegrityStatusV1::Faulted)
        {
            let Some(inspection) = self
                .inspect_room(room_id)
                .map_err(|error| map_gateway_inspection_error(&error))?
            else {
                return Ok(None);
            };
            let trace = replay_canonical_inspection_read_only(&inspection, registry)
                .map_err(map_gateway_recovery_error)?;
            return Ok(Some(SqliteCanonicalGatewayRoomSnapshot {
                trace,
                integrity: RoomIntegrityStateV1::new(
                    inspection.integrity_status,
                    inspection.integrity_generation,
                ),
                frame_heads: inspection.frame_heads,
            }));
        }
        let mut retries = 0;
        let trace = loop {
            match self.recover_canonical_room(registry, room_id) {
                Ok(trace) => break trace,
                Err(RoomRecoveryErrorV1::ConcurrentChange)
                    if retries < MAX_GATEWAY_SNAPSHOT_RECOVERY_RETRIES =>
                {
                    retries += 1;
                }
                Err(error) => return Err(map_gateway_recovery_error(error)),
            }
        };
        let Some(trace) = trace else {
            return Ok(None);
        };
        let fence = self
            .current_room_serving_fence(room_id)?
            .ok_or(SqliteGatewayErrorV1::RoomUnavailable)?;
        if fence.integrity().status() == RoomIntegrityStatusV1::Quarantined {
            return Err(SqliteGatewayErrorV1::IntegrityUnavailable);
        }
        if fence.head() != trace.head() {
            return Err(SqliteGatewayErrorV1::ConcurrentChange);
        }
        Ok(Some(SqliteCanonicalGatewayRoomSnapshot {
            trace,
            integrity: fence.integrity().clone(),
            frame_heads: fence.frame_heads().clone(),
        }))
    }

    /// Resolves the original durable receipt before recovering or preparing work.
    /// Fresh work uses the exact canonical serving executor.
    /// # Errors
    /// Returns the existing closed preparation, runtime, authority or storage failure.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_authorized_canonical_participant_action(
        &self,
        registry: &PackRegistryV1,
        authority: ParticipantActionAuthorityV1,
        request: &ParticipantActionRequestV1,
        admitted_at: ActionAdmittedAt,
        transition_id: TransitionId,
    ) -> Result<RoomCommitResolutionV1, SqliteParticipantActionErrorV1> {
        let receipt_authority =
            AuthorizedReceiptReadV1::from_participant_action_authority(&authority, request)
                .map_err(|_| SqliteParticipantActionErrorV1::Rejected)?;
        match self.resolve_canonical_admission_receipt(receipt_authority) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {}
            Err(AuthorityErrorV1::Unavailable) => {
                return Err(SqliteParticipantActionErrorV1::StorageUnavailable);
            }
            Err(_) => return Ok(RoomCommitResolutionV1::Fenced),
        }
        let parts = participant_action_request_parts(request)?;
        let snapshot = self
            .gateway_canonical_room_snapshot(registry, &parts.room_id)
            .map_err(|error| map_participant_action_gateway_error(&error))?
            .ok_or(SqliteParticipantActionErrorV1::RoomUnavailable)?;
        let SqliteCanonicalGatewayRoomSnapshot {
            mut trace,
            integrity,
            frame_heads,
        } = snapshot;
        self.commit_authorized_participant_action_from_canonical_serving_trace(
            authority,
            request,
            admitted_at,
            transition_id,
            &mut trace,
            &integrity,
            &frame_heads,
        )
    }

    /// Resolves the original durable receipt before recovering or preparing work.
    /// Fresh work uses the exact canonical serving executor.
    /// # Errors
    /// Returns the existing closed preparation, runtime, authority or storage failure.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_authorized_participant_action_from_canonical_serving_trace(
        &self,
        authority: ParticipantActionAuthorityV1,
        request: &ParticipantActionRequestV1,
        admitted_at: ActionAdmittedAt,
        transition_id: TransitionId,
        trace: &mut CanonicalRoomTrace,
        integrity: &RoomIntegrityStateV1,
        frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<RoomCommitResolutionV1, SqliteParticipantActionErrorV1> {
        let receipt_authority =
            AuthorizedReceiptReadV1::from_participant_action_authority(&authority, request)
                .map_err(|_| SqliteParticipantActionErrorV1::Rejected)?;
        match self.resolve_canonical_admission_receipt(receipt_authority) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {}
            Err(AuthorityErrorV1::Unavailable) => {
                return Err(SqliteParticipantActionErrorV1::StorageUnavailable);
            }
            Err(_) => return Ok(RoomCommitResolutionV1::Fenced),
        }
        let parts = participant_action_request_parts(request)?;
        if trace.head().room_id() != &parts.room_id {
            return Err(SqliteParticipantActionErrorV1::RoomUnavailable);
        }
        if integrity.status() == RoomIntegrityStatusV1::Faulted {
            return Err(SqliteParticipantActionErrorV1::RoomFaulted);
        }
        if integrity.status() == RoomIntegrityStatusV1::Quarantined {
            return Err(SqliteParticipantActionErrorV1::IntegrityUnavailable);
        }
        let payload_schema_digest = trace
            .retained_pack()
            .descriptor()
            .actions
            .iter()
            .find(|definition| definition.action_type == parts.action_type.as_str())
            .map(|definition| definition.payload_schema.schema_digest.clone())
            .ok_or(SqliteParticipantActionErrorV1::Rejected)?;
        let stimulus = RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: parts.member_id.clone(),
            action_id: parts.action_id.clone(),
            action_type: parts.action_type.clone(),
            payload_schema_digest,
            canonical_payload: parts.payload,
            exact_basis_head: trace.head().clone(),
            admitted_at: admitted_at.clone(),
        });
        let prepared = if parts.based_on_room_seq == trace.head().room_seq() {
            match trace.prepare(stimulus) {
                Ok(transition) => match authority {
                    ParticipantActionAuthorityV1::EnabledParticipant(authority) => {
                        PreparedCanonicalRoomCommit::for_authorized_action(
                            trace,
                            request,
                            transition,
                            transition_id,
                            integrity.generation(),
                            authority,
                            frame_heads,
                        )
                        .map_err(map_participant_action_preparation_error)?
                    }
                    ParticipantActionAuthorityV1::StableMembershipNotEnabled(authority) => {
                        PreparedCanonicalRoomCommit::for_authorized_stable_action_disposition(
                            trace,
                            request,
                            admitted_at,
                            integrity.generation(),
                            ParticipantActionAuthorityV1::StableMembershipNotEnabled(authority),
                        )
                        .map_err(map_participant_action_preparation_error)?
                    }
                },
                Err(
                    TraceErrorV1::ActionAdmission(_)
                    | TraceErrorV1::ArchivedStimulusForbidden
                    | TraceErrorV1::CompleteHeadMismatch,
                ) => PreparedCanonicalRoomCommit::for_authorized_stable_action_disposition(
                    trace,
                    request,
                    admitted_at,
                    integrity.generation(),
                    authority,
                )
                .map_err(map_participant_action_preparation_error)?,
                Err(error) => return Err(map_participant_action_trace_error(&error)),
            }
        } else {
            PreparedCanonicalRoomCommit::for_authorized_stable_action_disposition(
                trace,
                request,
                admitted_at,
                integrity.generation(),
                authority,
            )
            .map_err(map_participant_action_preparation_error)?
        };
        Ok(commit_canonical_existing_room(self, trace, prepared)
            .into_parts()
            .0)
    }

    /// Resolves the original durable receipt before recovering or preparing work.
    /// Fresh work uses the exact canonical serving executor.
    /// # Errors
    /// Returns the existing closed preparation, runtime, authority or storage failure.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_authorized_canonical_timer_fired(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedTimerFiredV1,
        request: &TimerFiredRequestV1,
        transition_id: TransitionId,
    ) -> Result<RoomCommitResolutionV1, SqliteTimerCommitErrorV1> {
        let receipt_authority =
            AuthorizedReceiptReadV1::from_timer_fired_authority(&authority, request)
                .map_err(|_| SqliteTimerCommitErrorV1::Rejected)?;
        match self.resolve_canonical_admission_receipt(receipt_authority) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {}
            Err(AuthorityErrorV1::Unavailable) => {
                return Err(SqliteTimerCommitErrorV1::StorageUnavailable);
            }
            Err(_) => return Ok(RoomCommitResolutionV1::Fenced),
        }
        let snapshot = self
            .gateway_canonical_room_snapshot(registry, request.room_id())
            .map_err(|error| map_timer_commit_gateway_error(&error))?
            .ok_or(SqliteTimerCommitErrorV1::RoomUnavailable)?;
        let SqliteCanonicalGatewayRoomSnapshot {
            mut trace,
            integrity,
            frame_heads,
        } = snapshot;
        self.commit_authorized_timer_fired_from_canonical_serving_trace(
            authority,
            request,
            transition_id,
            &mut trace,
            &integrity,
            &frame_heads,
        )
    }

    /// Resolves the original durable receipt before recovering or preparing work.
    /// Fresh work uses the exact canonical serving executor.
    /// # Errors
    /// Returns the existing closed preparation, runtime, authority or storage failure.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_authorized_timer_fired_from_canonical_serving_trace(
        &self,
        authority: AuthorizedTimerFiredV1,
        request: &TimerFiredRequestV1,
        transition_id: TransitionId,
        trace: &mut CanonicalRoomTrace,
        integrity: &RoomIntegrityStateV1,
        frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<RoomCommitResolutionV1, SqliteTimerCommitErrorV1> {
        let receipt_authority =
            AuthorizedReceiptReadV1::from_timer_fired_authority(&authority, request)
                .map_err(|_| SqliteTimerCommitErrorV1::Rejected)?;
        match self.resolve_canonical_admission_receipt(receipt_authority) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {}
            Err(AuthorityErrorV1::Unavailable) => {
                return Err(SqliteTimerCommitErrorV1::StorageUnavailable);
            }
            Err(_) => return Ok(RoomCommitResolutionV1::Fenced),
        }
        if trace.head().room_id() != request.room_id() {
            return Err(SqliteTimerCommitErrorV1::RoomUnavailable);
        }
        if integrity.status() == RoomIntegrityStatusV1::Faulted {
            return Err(SqliteTimerCommitErrorV1::RoomFaulted);
        }
        if integrity.status() == RoomIntegrityStatusV1::Quarantined {
            return Err(SqliteTimerCommitErrorV1::IntegrityUnavailable);
        }
        let integrity_generation = integrity.generation();
        let stimulus = RecordedStimulusV1::TimerFired(TimerFiredV1 {
            timer_id: request.timer_id().clone(),
            generation: request.generation(),
            scheduled_for: request.scheduled_for().clone(),
            canonical_payload: request.canonical_payload().clone(),
        });
        let prepared_transition = trace
            .prepare(stimulus)
            .map_err(|error| map_timer_commit_trace_error(&error))?;
        let prepared = PreparedCanonicalRoomCommit::for_authorized_timer_fired(
            trace,
            request,
            prepared_transition,
            transition_id,
            integrity_generation,
            authority,
            frame_heads,
        )
        .map_err(map_timer_commit_preparation_error)?;
        Ok(commit_canonical_existing_room(self, trace, prepared)
            .into_parts()
            .0)
    }

    /// Resolves the original durable receipt before recovering or preparing work.
    /// Fresh work uses the exact canonical serving executor.
    /// # Errors
    /// Returns the existing closed preparation, runtime, authority or storage failure.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_authorized_canonical_core_administration(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedCoreAdministrationV1,
        request: &CoreAdministrationRequestV1,
        recorded_at: CoreRecordedAt,
        transition_id: TransitionId,
    ) -> Result<RoomCommitResolutionV1, SqliteTimerCommitErrorV1> {
        let receipt_authority =
            AuthorizedReceiptReadV1::from_core_administration_authority(&authority, request)
                .map_err(|_| SqliteTimerCommitErrorV1::Rejected)?;
        match self.resolve_canonical_admission_receipt(receipt_authority) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {}
            Err(AuthorityErrorV1::Unavailable) => {
                return Err(SqliteTimerCommitErrorV1::StorageUnavailable);
            }
            Err(_) => return Ok(RoomCommitResolutionV1::Fenced),
        }
        let snapshot = self
            .gateway_canonical_room_snapshot(registry, request.room_id())
            .map_err(|error| map_timer_commit_gateway_error(&error))?
            .ok_or(SqliteTimerCommitErrorV1::RoomUnavailable)?;
        let SqliteCanonicalGatewayRoomSnapshot {
            mut trace,
            integrity,
            frame_heads,
        } = snapshot;
        if integrity.status() == RoomIntegrityStatusV1::Faulted {
            return Err(SqliteTimerCommitErrorV1::RoomFaulted);
        }
        if integrity.status() == RoomIntegrityStatusV1::Quarantined {
            return Err(SqliteTimerCommitErrorV1::IntegrityUnavailable);
        }
        let prepared = PreparedCanonicalRoomCommit::for_authorized_core_administration(
            &trace,
            request,
            recorded_at,
            transition_id,
            integrity.generation(),
            authority,
            &frame_heads,
        )
        .map_err(map_timer_commit_preparation_error)?;
        Ok(commit_canonical_existing_room(self, &mut trace, prepared)
            .into_parts()
            .0)
    }

    /// Resolves the original durable receipt before recovering or preparing work.
    /// Fresh work uses the exact canonical serving executor.
    /// # Errors
    /// Returns the existing closed preparation, runtime, authority or storage failure.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_authorized_canonical_external_input(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedExternalInputV1,
        room_id: &RoomId,
        based_on_room_seq: RoomSequenceV1,
        input: &ExternalInputV1,
        transition_id: TransitionId,
    ) -> Result<RoomCommitResolutionV1, SqliteTimerCommitErrorV1> {
        let receipt_authority = AuthorizedReceiptReadV1::from_external_input_authority(
            &authority,
            room_id,
            based_on_room_seq,
            input,
        )
        .map_err(|_| SqliteTimerCommitErrorV1::Rejected)?;
        match self.resolve_canonical_admission_receipt(receipt_authority) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {}
            Err(AuthorityErrorV1::Unavailable) => {
                return Err(SqliteTimerCommitErrorV1::StorageUnavailable);
            }
            Err(_) => return Ok(RoomCommitResolutionV1::Fenced),
        }
        let snapshot = self
            .gateway_canonical_room_snapshot(registry, room_id)
            .map_err(|error| map_timer_commit_gateway_error(&error))?
            .ok_or(SqliteTimerCommitErrorV1::RoomUnavailable)?;
        let SqliteCanonicalGatewayRoomSnapshot {
            mut trace,
            integrity,
            frame_heads,
        } = snapshot;
        self.commit_authorized_external_input_from_canonical_serving_trace(
            authority,
            room_id,
            based_on_room_seq,
            input,
            transition_id,
            &mut trace,
            &integrity,
            &frame_heads,
        )
    }

    /// Resolves the original durable receipt before recovering or preparing work.
    /// Fresh work uses the exact canonical serving executor.
    /// # Errors
    /// Returns the existing closed preparation, runtime, authority or storage failure.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_authorized_external_input_from_canonical_serving_trace(
        &self,
        authority: AuthorizedExternalInputV1,
        room_id: &RoomId,
        based_on_room_seq: RoomSequenceV1,
        input: &ExternalInputV1,
        transition_id: TransitionId,
        trace: &mut CanonicalRoomTrace,
        integrity: &RoomIntegrityStateV1,
        frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<RoomCommitResolutionV1, SqliteTimerCommitErrorV1> {
        let receipt_authority = AuthorizedReceiptReadV1::from_external_input_authority(
            &authority,
            room_id,
            based_on_room_seq,
            input,
        )
        .map_err(|_| SqliteTimerCommitErrorV1::Rejected)?;
        match self.resolve_canonical_admission_receipt(receipt_authority) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {}
            Err(AuthorityErrorV1::Unavailable) => {
                return Err(SqliteTimerCommitErrorV1::StorageUnavailable);
            }
            Err(_) => return Ok(RoomCommitResolutionV1::Fenced),
        }
        if trace.head().room_id() != room_id {
            return Err(SqliteTimerCommitErrorV1::RoomUnavailable);
        }
        if integrity.status() == RoomIntegrityStatusV1::Faulted {
            return Err(SqliteTimerCommitErrorV1::RoomFaulted);
        }
        if integrity.status() == RoomIntegrityStatusV1::Quarantined {
            return Err(SqliteTimerCommitErrorV1::IntegrityUnavailable);
        }
        let prepared_transition = trace
            .prepare(RecordedStimulusV1::ExternalInput(input.clone()))
            .map_err(|error| map_timer_commit_trace_error(&error))?;
        let prepared = PreparedCanonicalRoomCommit::for_authorized_external_input(
            trace,
            room_id,
            based_on_room_seq,
            input,
            prepared_transition,
            transition_id,
            integrity.generation(),
            authority,
            frame_heads,
        )
        .map_err(map_timer_commit_preparation_error)?;
        Ok(commit_canonical_existing_room(self, trace, prepared)
            .into_parts()
            .0)
    }
}

pub(super) fn replay_canonical_inspection_read_only(
    inspection: &RoomHistoryInspectionV1,
    registry: &PackRegistryV1,
) -> Result<CanonicalRoomTrace, RoomRecoveryErrorV1> {
    let report = CanonicalRoomTrace::replay(
        registry,
        &inspection.canonical_genesis_bytes,
        &inspection.canonical_transition_bytes,
    )
    .map_err(|failure| match failure.class {
        ReplayFailureClassV1::RuntimeUnavailable => RoomRecoveryErrorV1::RuntimeUnavailable,
        ReplayFailureClassV1::RuntimeFault => RoomRecoveryErrorV1::RuntimeFault,
        _ => RoomRecoveryErrorV1::Corrupt,
    })?;
    if report.final_head != inspection.head {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(report.into_trace())
}
