//! Bounded integration from the durable `SQLite` gateway seams to the server.
//!
//! This adapter deliberately exposes only operations whose protocol shape can
//! be produced from the current public Core/SQLite APIs. In particular, it
//! keeps Core synchronization tokens private and does not manufacture a Core
//! action result.

#![cfg_attr(test, allow(clippy::panic, clippy::unwrap_used))]

use std::{
    collections::BTreeMap,
    fmt::{self, Write},
    str::FromStr,
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use worldstream_core::{
    AccessModeV1, ActionAdmittedAt, ActionOfferWitnessV1, ActivationIntentStateV1,
    ActivationOperationRequestV1, ActivationResultCodeV1, AuthorityChangeId, AuthorityChangeV1,
    AuthorityCheckedAt, AuthorityErrorV1, AuthorityV1, AuthorizedRunnerControlV1,
    AuthorizedTimerFiredV1, CREATE_ROOM_OPERATION_KIND, CanonicalJsonV1, CapabilityBearerV1,
    CapabilityExpiresAt, CapabilityId, CapabilityProfileV1, CapabilityScopeSetV1,
    CreationRecordedAt, HistoricalReplayErrorV1, HostClockErrorV1, HostClockSampleV1, HostClockV1,
    InitialMembershipProposalV1, MemberReadOperationV1, MembershipStandingV1, MembershipV1,
    NewCapabilityV1, PackDigestV1, PackGenesisRequestV1, PackRegistryV1, PackViewerV1,
    ParticipantActionIngressErrorV1, ParticipantActionIngressV1, ParticipantActionRequestV1,
    PreparedRoomCreationV1, PrincipalKindV1, ReceiptSemanticInputV1, ReceiptSemanticTimeV1,
    ReplayProjectionKindV1, RoomCommitResolutionV1, RoomCommitStorageV1, RoomCreationIngressV1,
    RoomCreationRequestV1, RoomId, RoomMembershipKeyV1, RoomSeedV1, RoomSequenceV1,
    RunnerControlOperationV1, RunnerId, RunnerMembershipSetV1, SemanticResultV1, SessionErrorV1,
    SessionFrameV1, SessionSyncTokenV1, SessionV1, StoredSemanticResultV1, TimerFiredRequestV1,
    TimerGenerationV1, TimerId, TransitionId, authorize_participant_action_operation,
    authorize_room_creation_operation, commit_room_creation,
};
use worldstream_protocol::{
    AccessMode, ActionAccepted, ActionOffer, ActionRejected, ActionSubmit, ActivationClaim,
    ActivationDelivery, ActivationFrame, ActivationIntentState, ActivationLeaseOperation,
    ActivationOffer, ActivationOfferRequest, ActivationOffers, ActivationOperationReply,
    ActivationResultCode, BearerWireV1, ClientHello, CreateRoomRequest, CreateRoomResponse,
    MAX_MESSAGE_BYTES, ObservationAck, ObservationDeliver, PROTOCOL_VERSION, Principal,
    PrincipalKind, Projection, ProjectionReset, ProjectionResponse, ReplayResponse, RoomAttach,
    RoomAttached, RoomHead, RoomSyncAck, RunnerHello, RunnerReady, ServerWelcome, SyncBranch,
    TimerFireRequest, TimerFireResponse,
};
use worldstream_sqlite::{
    SqliteActivationErrorV1, SqliteAuthenticatedCapabilityV1, SqliteAuthorizedReplayErrorV1,
    SqliteAuthorizedReplayOutcomeV1, SqliteGatewayErrorV1, SqliteObservationDeliveryV1,
    SqliteObservationErrorV1, SqliteObservationFrameV1, SqliteObservationResetReasonV1,
    SqliteParticipantActionErrorV1, SqliteRoomRecoveryV1, SqliteRoomRuntimeStateV1,
    SqliteRoomStore, SqliteRoomSupervisorErrorV1, SqliteRoomSupervisorLeaseV1,
    SqliteRoomSupervisorOperationV1, SqliteRoomSupervisorV1, SqliteTimerCommitErrorV1,
    SqliteTimerStateV1,
};

use crate::{
    ActionReply, AttachReply, BackendError, GatewayBackend, GatewaySession,
    MemberCapabilityIssueRequest, MemberCapabilityIssueResponse, RunnerCapabilityIssueRequest,
    RunnerCapabilityIssueResponse, RunnerMembershipTarget, fill_random_bytes,
};

struct ActivationRequestParts {
    operation_kind: String,
    operation_id: String,
    activation_id: Option<String>,
    claim_id: Option<String>,
    runner_id: String,
    lease_generation: Option<u64>,
    requested_lease_ms: Option<u64>,
    disposition: Option<String>,
}

/// A SQLite-backed gateway with explicit session/capability binding.
pub struct SqliteGatewayBackend {
    store: SqliteRoomStore,
    registry: Arc<PackRegistryV1>,
    bindings: SessionBindings,
    supervisor: Arc<SqliteRoomSupervisorV1>,
    recoveries: Mutex<BTreeMap<String, CatchingUpRoom>>,
}

struct CatchingUpRoom {
    lease: SqliteRoomSupervisorLeaseV1,
    recovery: SqliteRoomRecoveryV1,
}

struct RecoveryWallClock;

impl HostClockV1 for RecoveryWallClock {
    fn sample(&self) -> Result<HostClockSampleV1, HostClockErrorV1> {
        let value = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| HostClockErrorV1::Unavailable)?;
        HostClockSampleV1::new(value).map_err(|_| HostClockErrorV1::Unavailable)
    }
}

impl fmt::Debug for SqliteGatewayBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteGatewayBackend")
            .field("store", &"[OPAQUE]")
            .field("registry", &"[OPAQUE]")
            .field("bindings", &self.bindings)
            .field("supervisor", &self.supervisor)
            .field("recoveries", &"[OPAQUE]")
            .finish()
    }
}

impl SqliteGatewayBackend {
    /// Wires an already-open `SQLite` store and an already-validated pack
    /// registry. The server owns neither Core Genesis preparation nor action
    /// coordination, so those operations remain fail closed below.
    #[must_use]
    pub fn new(store: SqliteRoomStore, registry: Arc<PackRegistryV1>) -> Self {
        Self {
            store,
            registry,
            bindings: SessionBindings::default(),
            supervisor: Arc::new(SqliteRoomSupervisorV1::default()),
            recoveries: Mutex::new(BTreeMap::new()),
        }
    }

    fn map_supervisor_error(error: SqliteRoomSupervisorErrorV1) -> BackendError {
        match error {
            SqliteRoomSupervisorErrorV1::Unavailable => BackendError::StorageUnavailable,
            SqliteRoomSupervisorErrorV1::StaleGeneration
            | SqliteRoomSupervisorErrorV1::InvalidTransition
            | SqliteRoomSupervisorErrorV1::NotActive
            | SqliteRoomSupervisorErrorV1::Busy => BackendError::Busy,
        }
    }

    /// Starts a new actor/runtime generation. The returned lease must move
    /// through Loading -> `CatchingUp` -> `Active` before it may publish.
    ///
    /// # Errors
    ///
    /// Returns `Busy` when another lifecycle generation or passivation barrier
    /// is in progress, or a closed storage/rejection error for invalid input.
    pub fn begin_room_reload(
        &self,
        room_id: &str,
    ) -> Result<SqliteRoomSupervisorLeaseV1, BackendError> {
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        self.supervisor
            .begin_reload(&room_id)
            .map_err(Self::map_supervisor_error)
    }

    /// Moves a current Loading generation into fixed-cutoff recovery.
    ///
    /// # Errors
    ///
    /// Returns `Busy` for a stale lease or invalid lifecycle transition.
    pub fn mark_room_catching_up(
        &self,
        room_id: &str,
        lease: SqliteRoomSupervisorLeaseV1,
    ) -> Result<(), BackendError> {
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        self.supervisor
            .set_catching_up(&room_id, lease)
            .map_err(Self::map_supervisor_error)
    }

    /// Publishes a current `CatchingUp` generation as Active.
    ///
    /// # Errors
    ///
    /// Returns `Busy` for a stale lease or invalid lifecycle transition.
    pub fn mark_room_active(
        &self,
        room_id: &str,
        lease: SqliteRoomSupervisorLeaseV1,
    ) -> Result<(), BackendError> {
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        self.supervisor
            .set_active(&room_id, lease)
            .map_err(Self::map_supervisor_error)
    }

    /// Stops new ordinary operations and opens the per-Room drain barrier.
    ///
    /// # Errors
    ///
    /// Returns `Busy` for a stale lease or invalid lifecycle transition.
    pub fn begin_room_passivation(
        &self,
        room_id: &str,
        lease: SqliteRoomSupervisorLeaseV1,
    ) -> Result<SqliteRoomSupervisorLeaseV1, BackendError> {
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        self.supervisor
            .begin_passivation(&room_id, lease)
            .map_err(Self::map_supervisor_error)
    }

    /// Finishes passivation after all operations admitted by the old
    /// generation have released their guards.
    ///
    /// # Errors
    ///
    /// Returns `Busy` until the in-flight operation count reaches zero.
    pub fn complete_room_passivation(
        &self,
        room_id: &str,
        barrier: SqliteRoomSupervisorLeaseV1,
    ) -> Result<(), BackendError> {
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        self.supervisor
            .finish_passivation(&room_id, barrier)
            .map_err(Self::map_supervisor_error)
    }

    #[must_use]
    pub fn room_supervisor_state(&self, room_id: &str) -> Option<SqliteRoomRuntimeStateV1> {
        let room_id = RoomId::from_str(room_id).ok()?;
        self.supervisor.state(&room_id)
    }

    fn verified_room_lifecycle(
        &self,
        room_id: &RoomId,
    ) -> Result<SqliteRoomRuntimeStateV1, BackendError> {
        // Durable integrity is an earlier fence than the in-process lifecycle.
        // In particular, an already-Active supervisor entry must not allow a
        // Faulted/Quarantined Room to mutate or trigger a healing read.
        if let Some(integrity) = self
            .store
            .room_integrity_state(room_id)
            .map_err(|_| BackendError::StorageUnavailable)?
        {
            match integrity.status() {
                worldstream_core::RoomIntegrityStatusV1::Healthy => {}
                worldstream_core::RoomIntegrityStatusV1::Faulted => {
                    return Err(BackendError::RoomFaulted);
                }
                worldstream_core::RoomIntegrityStatusV1::Quarantined => {
                    return Err(BackendError::RoomQuarantined);
                }
            }
        }
        match self.supervisor.state(room_id) {
            Some(SqliteRoomRuntimeStateV1::Active) => {
                return Ok(SqliteRoomRuntimeStateV1::Active);
            }
            Some(SqliteRoomRuntimeStateV1::Loading | SqliteRoomRuntimeStateV1::Passivating) => {
                return Err(BackendError::Busy);
            }
            Some(SqliteRoomRuntimeStateV1::CatchingUp) => {
                return Ok(SqliteRoomRuntimeStateV1::CatchingUp);
            }
            Some(SqliteRoomRuntimeStateV1::Inactive) | None => {}
        }

        // A backend created after startup enumeration may have no supervisor
        // entry yet. It must establish Loading and verify the durable Room
        // outside the short supervisor lock before publishing Active.
        let lease = self
            .supervisor
            .begin_reload(room_id)
            .map_err(Self::map_supervisor_error)?;
        let Ok(recovery) =
            self.store
                .recover_room_catching_up(&self.registry, room_id, &RecoveryWallClock)
        else {
            let _ = self.supervisor.abandon_loading(room_id, lease);
            return Err(BackendError::StorageUnavailable);
        };
        match recovery {
            Some(recovery) if recovery.state() == SqliteRoomRuntimeStateV1::Active => {
                self.supervisor
                    .set_catching_up(room_id, lease)
                    .map_err(Self::map_supervisor_error)?;
                self.supervisor
                    .set_active(room_id, lease)
                    .map_err(Self::map_supervisor_error)?;
                Ok(SqliteRoomRuntimeStateV1::Active)
            }
            Some(recovery) if recovery.state() == SqliteRoomRuntimeStateV1::CatchingUp => {
                self.supervisor
                    .set_catching_up(room_id, lease)
                    .map_err(Self::map_supervisor_error)?;
                self.recoveries
                    .lock()
                    .map_err(|_| BackendError::StorageUnavailable)?
                    .insert(room_id.to_string(), CatchingUpRoom { lease, recovery });
                Ok(SqliteRoomRuntimeStateV1::CatchingUp)
            }
            Some(_) | None => {
                self.supervisor
                    .abandon_loading(room_id, lease)
                    .map_err(Self::map_supervisor_error)?;
                Err(BackendError::NotFound)
            }
        }
    }

    fn ensure_verified_active(&self, room_id: &RoomId) -> Result<(), BackendError> {
        if self.verified_room_lifecycle(room_id)? == SqliteRoomRuntimeStateV1::Active {
            Ok(())
        } else {
            Err(BackendError::Busy)
        }
    }

    fn commit_catching_up_timer(
        &self,
        room_id: &RoomId,
        authority: AuthorizedTimerFiredV1,
        request: &TimerFiredRequestV1,
    ) -> Result<TimerFireResponse, BackendError> {
        let mut recoveries = self
            .recoveries
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?;
        let (response, lease, drained) = {
            let entry = recoveries
                .get_mut(room_id.as_str())
                .ok_or(BackendError::Busy)?;
            let expected = entry
                .recovery
                .next_due_timer(&self.store)
                .map_err(|_| BackendError::StorageUnavailable)?
                .ok_or(BackendError::Busy)?;
            let expected_hash = expected
                .canonical_request_hash()
                .map_err(|_| BackendError::InvalidResult)?;
            let request_hash = request
                .canonical_request_hash()
                .map_err(|_| BackendError::InvalidResult)?;
            if expected.operation_identity() != request.operation_identity()
                || expected_hash != request_hash
            {
                return Err(BackendError::Busy);
            }
            let transition_id = next_core_id::<TransitionId>()?;
            let resolution = self
                .store
                .commit_authorized_timer_fired(&self.registry, authority, request, transition_id)
                .map_err(|error| map_timer_commit_error(&error))?;
            let response = timer_response_from_resolution(request, &resolution)?;
            let drained = entry
                .recovery
                .next_due_timer(&self.store)
                .map_err(|_| BackendError::StorageUnavailable)?
                .is_none();
            (response, entry.lease, drained)
        };
        if drained {
            self.supervisor
                .set_active(room_id, lease)
                .map_err(Self::map_supervisor_error)?;
            recoveries.remove(room_id.as_str());
        }
        Ok(response)
    }

    fn acquire_room_operation(
        &self,
        room_id: &RoomId,
    ) -> Result<SqliteRoomSupervisorOperationV1, BackendError> {
        self.supervisor
            .acquire_active(room_id)
            .map_err(Self::map_supervisor_error)
    }

    fn supervised_room_snapshot(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<worldstream_sqlite::SqliteGatewayRoomSnapshotV1>, BackendError> {
        self.ensure_verified_active(room_id)?;
        let operation = self.acquire_room_operation(room_id)?;
        let result = self.store.gateway_room_snapshot(&self.registry, room_id);
        operation.publish().map_err(Self::map_supervisor_error)?;
        result.map_err(|error| map_gateway_error(&error))
    }

    fn readable_room_snapshot(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<worldstream_sqlite::SqliteGatewayRoomSnapshotV1>, BackendError> {
        match self
            .store
            .room_integrity_state(room_id)
            .map_err(|_| BackendError::StorageUnavailable)?
            .map(|integrity| integrity.status())
        {
            Some(worldstream_core::RoomIntegrityStatusV1::Quarantined) => {
                Err(BackendError::RoomQuarantined)
            }
            Some(worldstream_core::RoomIntegrityStatusV1::Faulted) => self
                .store
                .gateway_room_snapshot(&self.registry, room_id)
                .map_err(|error| map_gateway_error(&error)),
            Some(worldstream_core::RoomIntegrityStatusV1::Healthy) | None => {
                self.supervised_room_snapshot(room_id)
            }
        }
    }

    fn authorize_runner_target(
        &self,
        session: &GatewaySession,
        runner_id: &str,
        operation: RunnerControlOperationV1,
        room_id: RoomId,
        member_id: worldstream_core::MemberId,
    ) -> Result<AuthorizedRunnerControlV1, BackendError> {
        let runner_id = runner_id
            .parse::<RunnerId>()
            .map_err(|_| BackendError::Rejected)?;
        let authenticated = self.authenticate(session)?;
        self.ensure_verified_active(&room_id)?;
        self.authority()
            .authorize_runner_control(
                &authenticated.into_presented(),
                runner_id,
                operation,
                RoomMembershipKeyV1 { room_id, member_id },
                Self::checked_at()?,
            )
            .map_err(map_authority_error)
    }

    fn activation_request(parts: ActivationRequestParts) -> ActivationOperationRequestV1 {
        ActivationOperationRequestV1 {
            operation_kind: parts.operation_kind,
            operation_id: parts.operation_id,
            activation_id: parts.activation_id,
            claim_id: parts.claim_id,
            runner_id: parts.runner_id,
            lease_generation: parts.lease_generation,
            requested_lease_ms: parts.requested_lease_ms,
            disposition: parts.disposition,
        }
    }

    fn activation_reply(
        result: worldstream_core::ActivationOperationResultV1,
    ) -> Result<ActivationOperationReply, BackendError> {
        Ok(ActivationOperationReply {
            operation_id: result.operation_id,
            activation_id: result.activation_id,
            claim_id: result.claim_id,
            runner_id: result.runner_id,
            code: activation_result_code(result.code),
            state: result.state.map(activation_state),
            lease_generation: result.lease_generation,
            context_hash: result.context_hash.map(|value| value.to_string()),
            context: result.context.map(activation_context).transpose()?,
        })
    }

    fn authenticate(
        &self,
        session: &GatewaySession,
    ) -> Result<SqliteAuthenticatedCapabilityV1, BackendError> {
        let bearer = session
            .owned_bearer()
            .ok_or(BackendError::StorageUnavailable)?;
        let authenticated = self
            .store
            .authenticate_bearer(bearer)
            .map_err(|error| map_gateway_error(&error))?;
        Ok(authenticated)
    }

    fn authority(&self) -> AuthorityV1 {
        AuthorityV1::new(Arc::new(self.store.clone()))
    }

    #[allow(clippy::too_many_lines)]
    fn issue_member_capability_inner(
        &self,
        session: &GatewaySession,
        request: MemberCapabilityIssueRequest,
    ) -> Result<MemberCapabilityIssueResponse, BackendError> {
        // Authenticate before parsing target data so the route never becomes
        // a room or membership oracle for unauthenticated callers.
        let authenticated = self.authenticate(session)?;
        let room_id = RoomId::from_str(&request.room_id).map_err(|_| BackendError::Rejected)?;
        let member_id = request
            .member_id
            .parse::<worldstream_core::MemberId>()
            .map_err(|_| BackendError::Rejected)?;
        let principal_id = request
            .principal_id
            .parse::<worldstream_core::PrincipalId>()
            .map_err(|_| BackendError::Rejected)?;
        let change_id = AuthorityChangeId::from_str(&request.idempotency_key)
            .map_err(|_| BackendError::Rejected)?;
        let scopes =
            CapabilityScopeSetV1::new(request.scopes).map_err(|_| BackendError::Rejected)?;
        let expires_at = request
            .expires_at
            .as_deref()
            .map(CapabilityExpiresAt::from_str)
            .transpose()
            .map_err(|_| BackendError::Rejected)?;

        let snapshot = self
            .supervised_room_snapshot(&room_id)?
            .ok_or(BackendError::NotFound)?;
        let membership = snapshot
            .trace()
            .core_state()
            .membership(&member_id)
            .filter(|membership| membership.standing() == MembershipStandingV1::Enabled)
            .ok_or(BackendError::Forbidden)?;
        if membership.principal_id() != &principal_id {
            return Err(BackendError::Forbidden);
        }

        // A Room can be created with an Agent Membership before the
        // operational authority row exists. Provision that row through the
        // same HostOperator-gated authority seam before registering the
        // Room-member Capability. The Principal ID is itself a canonical
        // ULID, so using it as the stable Principal change ID keeps this
        // compatible with the existing request shape while making retries and
        // concurrent requests converge on one authority change. A caller
        // cannot use the same ID for both changes because each durable receipt
        // has exactly one AuthorityChange target.
        let principal_change_id = AuthorityChangeId::from_str(principal_id.as_str())
            .map_err(|_| BackendError::Rejected)?;
        if principal_change_id == change_id {
            return Err(BackendError::Rejected);
        }
        let presented = authenticated.into_presented();
        let authority = self.authority();
        let principal_result = authority.change(
            &presented,
            AuthorityChangeV1::CreatePrincipal {
                change_id: principal_change_id,
                principal_id: principal_id.clone(),
                kind: membership.principal_kind(),
            },
            Self::checked_at()?,
        );
        match principal_result {
            Ok(receipt)
                if receipt.result()
                    == worldstream_core::AuthorityChangeResultV1::PrincipalCreated
                    && receipt.resulting_generation() == 1
                    && matches!(
                        receipt.target(),
                        worldstream_core::AuthorityChangeTargetV1::Principal(id)
                            if id == &principal_id
                    ) => {}
            Err(AuthorityErrorV1::Conflict | AuthorityErrorV1::InvalidAuthorityRequest) => {
                // The Principal already exists, or its canonical change ID
                // was already applied. The subsequent capability change
                // revalidates the stored Principal kind, status, and exact
                // Room/member binding, so this branch cannot escalate a
                // mismatched or disabled Principal. SQLite may surface the
                // already-applied Principal replay as InvalidAuthorityRequest
                // after the independent Capability commit; it is still not
                // sufficient to issue a bearer without the next validation.
            }
            Ok(_) => return Err(BackendError::InvalidResult),
            Err(error) => return Err(map_authority_error(error)),
        }

        // The plaintext exists only in this stack frame. AuthorityV1::change
        // stores the token hash and applies the typed idempotent change under
        // the SQLite writer transaction; it never receives this bearer.
        let mut bearer_bytes = random_bearer_bytes()?;
        let capability_bearer = CapabilityBearerV1::from_bytes(bearer_bytes);
        let capability_id = next_core_id::<CapabilityId>()?;
        let capability = NewCapabilityV1::new(
            capability_id.clone(),
            capability_bearer.token_hash(),
            principal_id.clone(),
            CapabilityProfileV1::RoomMember {
                room_id: room_id.clone(),
                member_id: member_id.clone(),
            },
            scopes.clone(),
            expires_at,
        )
        .map_err(|_| BackendError::Rejected)?;
        let capability_result = authority.change(
            &presented,
            AuthorityChangeV1::RegisterCapability {
                change_id,
                capability,
            },
            Self::checked_at()?,
        );
        let receipt = capability_result.map_err(map_authority_error)?;
        if receipt.result() != worldstream_core::AuthorityChangeResultV1::CapabilityRegistered
            || receipt.resulting_generation() != 1
            || !matches!(
                receipt.target(),
                worldstream_core::AuthorityChangeTargetV1::Capability(id)
                    if id == &capability_id
            )
        {
            return Err(BackendError::InvalidResult);
        }
        let bearer = BearerWireV1::from_bytes(bearer_bytes).to_wire();
        bearer_bytes.fill(0);
        drop(capability_bearer);
        Ok(MemberCapabilityIssueResponse {
            capability_id: capability_id.to_string(),
            room_id: room_id.to_string(),
            member_id: member_id.to_string(),
            principal_id: principal_id.to_string(),
            scopes,
            bearer,
        })
    }

    // This operation intentionally keeps the three authority commits and
    // their postconditions together so a one-time bearer is never returned
    // before the complete Runner control chain is durably installed.
    #[allow(clippy::too_many_lines)]
    fn issue_runner_capability_inner(
        &self,
        session: &GatewaySession,
        request: RunnerCapabilityIssueRequest,
    ) -> Result<RunnerCapabilityIssueResponse, BackendError> {
        let authenticated = self.authenticate(session)?;
        let runner_id = request
            .runner_id
            .parse::<RunnerId>()
            .map_err(|_| BackendError::Rejected)?;
        let owner_principal_id = request
            .owner_principal_id
            .parse::<worldstream_core::PrincipalId>()
            .map_err(|_| BackendError::Rejected)?;
        let principal_change_id = AuthorityChangeId::from_str(&request.principal_idempotency_key)
            .map_err(|_| BackendError::Rejected)?;
        let runner_change_id = AuthorityChangeId::from_str(&request.runner_idempotency_key)
            .map_err(|_| BackendError::Rejected)?;
        let capability_change_id = AuthorityChangeId::from_str(&request.capability_idempotency_key)
            .map_err(|_| BackendError::Rejected)?;
        if principal_change_id == runner_change_id
            || principal_change_id == capability_change_id
            || runner_change_id == capability_change_id
        {
            return Err(BackendError::Rejected);
        }
        let scopes =
            CapabilityScopeSetV1::new(request.scopes).map_err(|_| BackendError::Rejected)?;
        let expires_at = request
            .expires_at
            .as_deref()
            .map(CapabilityExpiresAt::from_str)
            .transpose()
            .map_err(|_| BackendError::Rejected)?;
        let target_keys = request
            .permitted_memberships
            .iter()
            .map(|target| {
                Ok(RoomMembershipKeyV1 {
                    room_id: target.room_id.parse().map_err(|_| BackendError::Rejected)?,
                    member_id: target
                        .member_id
                        .parse()
                        .map_err(|_| BackendError::Rejected)?,
                })
            })
            .collect::<Result<Vec<_>, BackendError>>()?;
        let permitted_memberships =
            RunnerMembershipSetV1::new(target_keys).map_err(|_| BackendError::Rejected)?;

        for target in permitted_memberships.iter() {
            let snapshot = self
                .supervised_room_snapshot(&target.room_id)?
                .ok_or(BackendError::NotFound)?;
            let membership = snapshot
                .trace()
                .core_state()
                .membership(&target.member_id)
                .ok_or(BackendError::Forbidden)?;
            if membership.principal_id() != &owner_principal_id
                || membership.principal_kind() != PrincipalKindV1::Agent
                || membership.access_mode() != AccessModeV1::Participant
                || membership.standing() != MembershipStandingV1::Enabled
                || membership.role().is_none()
            {
                return Err(BackendError::Forbidden);
            }
        }

        let presented = authenticated.into_presented();
        let authority = self.authority();
        let principal_receipt = authority
            .change(
                &presented,
                AuthorityChangeV1::CreatePrincipal {
                    change_id: principal_change_id,
                    principal_id: owner_principal_id.clone(),
                    kind: PrincipalKindV1::Agent,
                },
                Self::checked_at()?,
            )
            .map_err(map_authority_error)?;
        if principal_receipt.result() != worldstream_core::AuthorityChangeResultV1::PrincipalCreated
            || principal_receipt.resulting_generation() != 1
            || !matches!(
                principal_receipt.target(),
                worldstream_core::AuthorityChangeTargetV1::Principal(id)
                    if id == &owner_principal_id
            )
        {
            return Err(BackendError::InvalidResult);
        }

        let runner_receipt = authority
            .change(
                &presented,
                AuthorityChangeV1::RegisterRunner {
                    change_id: runner_change_id,
                    runner_id: runner_id.clone(),
                    owner_principal_id: owner_principal_id.clone(),
                },
                Self::checked_at()?,
            )
            .map_err(map_authority_error)?;
        if runner_receipt.result() != worldstream_core::AuthorityChangeResultV1::RunnerRegistered
            || runner_receipt.resulting_generation() != 1
            || !matches!(
                runner_receipt.target(),
                worldstream_core::AuthorityChangeTargetV1::Runner(id) if id == &runner_id
            )
        {
            return Err(BackendError::InvalidResult);
        }

        let mut bearer_bytes = random_bearer_bytes()?;
        let capability_bearer = CapabilityBearerV1::from_bytes(bearer_bytes);
        let capability_id = next_core_id::<CapabilityId>()?;
        let capability = NewCapabilityV1::new(
            capability_id.clone(),
            capability_bearer.token_hash(),
            owner_principal_id.clone(),
            CapabilityProfileV1::RunnerControl {
                runner_id: runner_id.clone(),
                permitted_memberships: permitted_memberships.clone(),
            },
            scopes.clone(),
            expires_at,
        )
        .map_err(|_| BackendError::Rejected)?;
        let capability_receipt = authority
            .change(
                &presented,
                AuthorityChangeV1::RegisterCapability {
                    change_id: capability_change_id,
                    capability,
                },
                Self::checked_at()?,
            )
            .map_err(map_authority_error)?;
        if capability_receipt.result()
            != worldstream_core::AuthorityChangeResultV1::CapabilityRegistered
            || capability_receipt.resulting_generation() != 1
            || !matches!(
                capability_receipt.target(),
                worldstream_core::AuthorityChangeTargetV1::Capability(id)
                    if id == &capability_id
            )
        {
            return Err(BackendError::InvalidResult);
        }
        let bearer = BearerWireV1::from_bytes(bearer_bytes).to_wire();
        bearer_bytes.fill(0);
        drop(capability_bearer);
        let permitted_memberships = permitted_memberships
            .iter()
            .map(|target| RunnerMembershipTarget {
                room_id: target.room_id.to_string(),
                member_id: target.member_id.to_string(),
            })
            .collect();
        Ok(RunnerCapabilityIssueResponse {
            capability_id: capability_id.to_string(),
            runner_id: runner_id.to_string(),
            owner_principal_id: owner_principal_id.to_string(),
            permitted_memberships,
            scopes,
            bearer,
        })
    }

    fn checked_at() -> Result<AuthorityCheckedAt, BackendError> {
        let value = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| BackendError::StorageUnavailable)?;
        AuthorityCheckedAt::from_str(&value).map_err(|_| BackendError::StorageUnavailable)
    }

    fn replay_response(
        &self,
        session: &GatewaySession,
        room_id: &str,
        at_room_seq: u64,
    ) -> Result<ReplayResponse, BackendError> {
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::NotFound)?;
        let at_room_seq = RoomSequenceV1::new(at_room_seq).map_err(|_| BackendError::Rejected)?;
        let authenticated = self.authenticate(session)?;
        let snapshot = self
            .readable_room_snapshot(&room_id)?
            .ok_or(BackendError::NotFound)?;
        let member_id = snapshot
            .trace()
            .core_state()
            .memberships()
            .values()
            .find(|membership| {
                membership.principal_id() == authenticated.principal_id()
                    && membership.standing() == MembershipStandingV1::Enabled
            })
            .map(|membership| membership.member_id().clone())
            .ok_or(BackendError::Forbidden)?;
        let authority = self
            .authority()
            .authorize_replay(
                &authenticated.into_presented(),
                room_id.clone(),
                member_id,
                at_room_seq,
                ReplayProjectionKindV1::HistoricalMembership,
                Self::checked_at()?,
            )
            .map_err(map_authority_error)?;
        let mut outcome = self
            .store
            .replay_authorized(&self.registry, authority)
            .map_err(map_replay_error)?;
        let replay = loop {
            match outcome {
                SqliteAuthorizedReplayOutcomeV1::Complete(projection) => break *projection,
                SqliteAuthorizedReplayOutcomeV1::Deferred(continuation) => {
                    outcome = self
                        .store
                        .resume_replay_authorized(&self.registry, continuation)
                        .map_err(map_replay_error)?;
                }
            }
        };
        let view_bytes = replay_view_bytes(replay.canonical_envelope())?;
        let projection = projection_from_canonical_bytes(&view_bytes)?;
        let projection_hash = worldstream_core::projection_hash_for_canonical_bytes(&view_bytes)
            .map_err(|_| BackendError::InvalidResult)?
            .to_string();
        let descriptor = self
            .registry
            .load_retained(replay.verified_head().pack_digest())
            .map_err(|_| BackendError::InvalidResult)?
            .descriptor();
        Ok(ReplayResponse {
            room_id: room_id.to_string(),
            pack: worldstream_protocol::PackReference {
                id: descriptor.pack_id.clone(),
                version: descriptor.explanatory_version.clone(),
                digest: replay.verified_head().pack_digest().to_string(),
            },
            requested_room_seq: at_room_seq.get(),
            room_head: room_head(replay.verified_head()),
            projection,
            projection_hash,
            verification: "verified".to_owned(),
            room_health: integrity_status(replay.integrity().status()),
            integrity_generation: replay.integrity().generation().get(),
        })
    }

    fn projection_response(
        &self,
        session: &GatewaySession,
        room_id: &str,
    ) -> Result<ProjectionResponse, BackendError> {
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::NotFound)?;
        let authenticated = self.authenticate(session)?;
        let snapshot = self
            .readable_room_snapshot(&room_id)?
            .ok_or(BackendError::NotFound)?;
        let principal_id = authenticated.principal_id().clone();
        let member_id = snapshot
            .trace()
            .core_state()
            .memberships()
            .values()
            .find(|membership| {
                membership.principal_id() == &principal_id
                    && membership.standing() == worldstream_core::MembershipStandingV1::Enabled
            })
            .map(|membership| membership.member_id().clone())
            .ok_or(BackendError::Forbidden)?;
        let presented = authenticated.into_presented();
        self.authority()
            .authorize_member_read(
                &presented,
                room_id.clone(),
                member_id.clone(),
                MemberReadOperationV1::CurrentProjection,
                Self::checked_at()?,
            )
            .map_err(map_authority_error)?;

        let trace = snapshot.trace();
        let host = self
            .registry
            .load_retained(trace.head().pack_digest())
            .map_err(|_| BackendError::InvalidResult)?
            .host();
        let view = host
            .view(&worldstream_core::ViewInputV1 {
                core: trace.core_state(),
                activity_state: trace.activity_state(),
                complete_head: trace.head(),
                viewer: &viewer_for(
                    trace
                        .core_state()
                        .membership(&member_id)
                        .ok_or(BackendError::InvalidResult)?,
                ),
            })
            .map_err(|_| BackendError::InvalidResult)?;

        // Re-read the verified snapshot after authorization and view
        // construction. A changed Head is not released as a stale response.
        let current = self
            .readable_room_snapshot(&room_id)?
            .ok_or(BackendError::NotFound)?;
        if current.trace().head() != trace.head() || current.integrity() != snapshot.integrity() {
            return Err(BackendError::Busy);
        }

        let projection = projection_from_view(&view)?;
        let projection_hash = view
            .projection_hash()
            .map_err(|_| BackendError::InvalidResult)?
            .to_string();
        Ok(ProjectionResponse {
            room_id: room_id.to_string(),
            room_head: room_head(trace.head()),
            // Faulted reads are released only after exact retained-history
            // verification; the envelope makes that operational state
            // explicit to the caller.
            room_health: integrity_status(snapshot.integrity().status()),
            integrity_generation: snapshot.integrity_generation().get(),
            projection_schema: view.projection_schema().to_owned(),
            projection,
            projection_hash,
        })
    }

    fn attach_view(
        &self,
        session: &GatewaySession,
        room_id: &RoomId,
        member_id: &worldstream_core::MemberId,
    ) -> Result<AttachContext, BackendError> {
        let authenticated = self.authenticate(session)?;
        let snapshot = self
            .readable_room_snapshot(room_id)?
            .ok_or(BackendError::NotFound)?;
        let membership = snapshot
            .trace()
            .core_state()
            .membership(member_id)
            .filter(|membership| {
                membership.principal_id() == authenticated.principal_id()
                    && membership.standing() == worldstream_core::MembershipStandingV1::Enabled
            })
            .ok_or(BackendError::Forbidden)?
            .clone();
        let presented = authenticated.into_presented();
        self.authority()
            .authorize_member_read(
                &presented,
                room_id.clone(),
                member_id.clone(),
                MemberReadOperationV1::Attach,
                Self::checked_at()?,
            )
            .map_err(map_authority_error)?;
        let trace = snapshot.trace();
        let host = self
            .registry
            .load_retained(trace.head().pack_digest())
            .map_err(|_| BackendError::InvalidResult)?
            .host();
        let view = host
            .view(&worldstream_core::ViewInputV1 {
                core: trace.core_state(),
                activity_state: trace.activity_state(),
                complete_head: trace.head(),
                viewer: &viewer_for(&membership),
            })
            .map_err(|_| BackendError::InvalidResult)?;
        Ok(AttachContext {
            snapshot,
            view,
            membership,
            capability_id: presented.capability_id().clone(),
        })
    }

    fn issue_sync_token(
        &self,
        session: &GatewaySession,
        capability_id: &worldstream_core::CapabilityId,
        room_id: &RoomId,
        member_id: &worldstream_core::MemberId,
        captured: &worldstream_sqlite::SqliteObservationAttachV1,
    ) -> Result<String, BackendError> {
        let barrier = captured
            .session_barrier()
            .map_err(|_| BackendError::InvalidResult)?;
        let mut core_session = SessionV1::new(crate::MAX_OUTBOUND_FRAME_BURST)
            .map_err(|_| BackendError::InvalidResult)?;
        let captured_session = core_session
            .capture_barrier(barrier)
            .map_err(|_| BackendError::InvalidResult)?;
        let token = crate::next_ulid()
            .ok_or(BackendError::Indeterminate)?
            .to_string();
        self.bindings.issue(
            session.session_id(),
            capability_id,
            SyncBinding {
                token: token.clone(),
                room_id: room_id.clone(),
                member_id: member_id.clone(),
                baseline_frame_head: captured_session.barrier().frame_head(),
                session: core_session,
                core_token: captured_session.sync_token().clone(),
            },
        )?;
        Ok(token)
    }
    fn create_room_attempt(
        &self,
        session: &GatewaySession,
        request: &CreateRoomRequest,
    ) -> Result<CreateAttempt, BackendError> {
        let authenticated = self.authenticate(session)?;
        let (core_request, identity) = creation_request(&authenticated, request)?;
        let presented = authenticated.into_presented();
        let ingress = authorize_room_creation_operation(
            &self.authority(),
            &self.store,
            &presented,
            &identity,
            &core_request,
            Self::checked_at()?,
        )
        .map_err(map_room_operation_error)?;
        let grant = match ingress {
            RoomCreationIngressV1::Existing(result) => {
                return Ok(CreateAttempt::Response(Box::new(
                    create_response_from_result(&result)?,
                )));
            }
            RoomCreationIngressV1::Conflict { .. } => return Err(BackendError::Conflict),
            RoomCreationIngressV1::Authorized(grant) => *grant,
        };

        let selected = self
            .registry
            .select_for_new_room(core_request.pack_digest())
            .map_err(|_| BackendError::Rejected)?;
        if selected.descriptor().pack_id != request.pack.id
            || selected.descriptor().explanatory_version != request.pack.version
        {
            return Err(BackendError::Rejected);
        }
        let room_id = next_core_id::<RoomId>()?;
        let seed = random_room_seed()?;
        let created_at = creation_time()?;
        let memberships = request
            .members
            .iter()
            .map(|member| {
                let principal_id = member
                    .principal_id
                    .parse()
                    .map_err(|_| BackendError::Rejected)?;
                MembershipV1::new(
                    next_core_id::<worldstream_core::MemberId>()?,
                    principal_id,
                    core_principal_kind(member.principal_kind),
                    MembershipStandingV1::Enabled,
                    core_access_mode(member.access_mode),
                    member.role.clone(),
                )
                .map_err(|_| BackendError::Rejected)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let genesis = self
            .registry
            .prepare_genesis_for_new_room(&PackGenesisRequestV1 {
                room_id,
                pack_digest: core_request.pack_digest().clone(),
                configuration: core_request.configuration().clone(),
                room_seed: seed,
                created_at,
                initial_core_state: worldstream_core::CoreRoomStateV1::active(memberships)
                    .map_err(|_| BackendError::Rejected)?,
            })
            .map_err(|_| BackendError::Rejected)?;
        let prepared =
            PreparedRoomCreationV1::from_registry_genesis(identity, &core_request, grant, genesis)
                .map_err(|_| BackendError::InvalidResult)?;
        let (resolution, _, _) = commit_room_creation(&self.store, prepared).into_parts();
        match resolution {
            RoomCommitResolutionV1::GenesisCreated { result, .. } => Ok(CreateAttempt::Response(
                Box::new(create_response_from_result(&result)?),
            )),
            RoomCommitResolutionV1::Reprepare | RoomCommitResolutionV1::RetryableKnownAbsent => {
                Ok(CreateAttempt::Retry)
            }
            RoomCommitResolutionV1::Indeterminate => Err(BackendError::Indeterminate),
            RoomCommitResolutionV1::Conflict { .. } => Err(BackendError::Conflict),
            RoomCommitResolutionV1::Fenced => Err(BackendError::Busy),
            RoomCommitResolutionV1::Fault
            | RoomCommitResolutionV1::NotApplicable
            | RoomCommitResolutionV1::TransitionCommitted { .. }
            | RoomCommitResolutionV1::RejectionRecorded { .. }
            | RoomCommitResolutionV1::NoChangeRecorded { .. } => Err(BackendError::InvalidResult),
        }
    }

    fn activation_lease_operation(
        &self,
        session: &GatewaySession,
        request: ActivationLeaseOperation,
        operation: RunnerControlOperationV1,
        operation_kind: &str,
    ) -> Result<ActivationOperationReply, BackendError> {
        let (room_id, member_id) = self
            .store
            .activation_target(&request.activation_id)
            .map_err(map_activation_error)?
            .ok_or(BackendError::NotFound)?;
        let authority = self.authorize_runner_target(
            session,
            &request.runner_id,
            operation,
            room_id,
            member_id,
        )?;
        if operation_kind == "complete"
            && !matches!(
                request.disposition.as_deref(),
                Some("handled" | "declined" | "failed")
            )
        {
            return Err(BackendError::Rejected);
        }
        let core_request = Self::activation_request(ActivationRequestParts {
            operation_kind: operation_kind.to_owned(),
            operation_id: request.operation_id,
            activation_id: Some(request.activation_id),
            claim_id: Some(request.claim_id),
            runner_id: request.runner_id,
            lease_generation: Some(request.lease_generation),
            requested_lease_ms: request.requested_lease_ms,
            disposition: request.disposition,
        });
        let result = match operation {
            RunnerControlOperationV1::Renew => self.store.renew_activation(authority, core_request),
            RunnerControlOperationV1::Release => {
                self.store.release_activation(authority, core_request)
            }
            RunnerControlOperationV1::Complete => {
                self.store.complete_activation(authority, core_request)
            }
            RunnerControlOperationV1::ReceiveOffer | RunnerControlOperationV1::Claim => {
                return Err(BackendError::InvalidResult);
            }
        }
        .map_err(map_activation_error)?;
        Self::activation_reply(result)
    }
}

enum CreateAttempt {
    Response(Box<CreateRoomResponse>),
    Retry,
}

fn creation_request(
    authenticated: &SqliteAuthenticatedCapabilityV1,
    request: &CreateRoomRequest,
) -> Result<
    (
        RoomCreationRequestV1,
        worldstream_core::AdministrationOperationIdentityV1,
    ),
    BackendError,
> {
    if request.members.is_empty() {
        return Err(BackendError::Rejected);
    }
    let pack_digest =
        PackDigestV1::from_str(&request.pack.digest).map_err(|_| BackendError::Rejected)?;
    let configuration = canonical_json(&request.configuration)?;
    let proposals = request
        .members
        .iter()
        .map(|member| {
            let principal_id = member
                .principal_id
                .parse()
                .map_err(|_| BackendError::Rejected)?;
            InitialMembershipProposalV1::new(
                principal_id,
                core_principal_kind(member.principal_kind),
                MembershipStandingV1::Enabled,
                core_access_mode(member.access_mode),
                member.role.clone(),
            )
            .map_err(|_| BackendError::Rejected)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let core_request = RoomCreationRequestV1::new(pack_digest, configuration, proposals);
    core_request
        .canonical_request_hash()
        .map_err(|_| BackendError::Rejected)?;
    Ok((
        core_request,
        worldstream_core::AdministrationOperationIdentityV1 {
            authenticated_principal: authenticated.principal_id().clone(),
            versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
            idempotency_key: request.idempotency_key.clone(),
        },
    ))
}

fn canonical_json(value: &Value) -> Result<CanonicalJsonV1, BackendError> {
    let bytes = serde_json::to_vec(value).map_err(|_| BackendError::Rejected)?;
    CanonicalJsonV1::parse(&bytes).map_err(|_| BackendError::Rejected)
}

fn participant_action_request(
    request: &ActionSubmit,
) -> Result<ParticipantActionRequestV1, BackendError> {
    let room_id = RoomId::from_str(&request.room_id).map_err(|_| BackendError::Rejected)?;
    let member_id = request
        .member_id
        .parse()
        .map_err(|_| BackendError::Rejected)?;
    let action_id = request
        .action_id
        .parse()
        .map_err(|_| BackendError::Rejected)?;
    let based_on_room_seq =
        RoomSequenceV1::new(request.based_on_room_seq).map_err(|_| BackendError::Rejected)?;
    Ok(ParticipantActionRequestV1::new(
        room_id,
        member_id,
        action_id,
        based_on_room_seq,
        request.action_type.clone(),
        canonical_json(&request.payload)?,
    ))
}

fn map_participant_action_ingress_error(error: &ParticipantActionIngressErrorV1) -> BackendError {
    match error {
        ParticipantActionIngressErrorV1::Authority(error) => map_authority_error(*error),
        ParticipantActionIngressErrorV1::ResolutionUnavailable => BackendError::Indeterminate,
        ParticipantActionIngressErrorV1::InvalidStoredResult => BackendError::InvalidResult,
    }
}

fn map_participant_action_error(error: &SqliteParticipantActionErrorV1) -> BackendError {
    match error {
        SqliteParticipantActionErrorV1::StorageUnavailable => BackendError::StorageUnavailable,
        SqliteParticipantActionErrorV1::RoomUnavailable => BackendError::NotFound,
        SqliteParticipantActionErrorV1::IntegrityUnavailable
        | SqliteParticipantActionErrorV1::InvalidResult => BackendError::InvalidResult,
        SqliteParticipantActionErrorV1::RoomFaulted => BackendError::RoomFaulted,
        SqliteParticipantActionErrorV1::ConcurrentChange => BackendError::Busy,
        SqliteParticipantActionErrorV1::Rejected => BackendError::Rejected,
    }
}

fn map_timer_commit_error(error: &SqliteTimerCommitErrorV1) -> BackendError {
    match error {
        SqliteTimerCommitErrorV1::StorageUnavailable => BackendError::StorageUnavailable,
        SqliteTimerCommitErrorV1::RoomUnavailable => BackendError::NotFound,
        SqliteTimerCommitErrorV1::IntegrityUnavailable
        | SqliteTimerCommitErrorV1::InvalidResult => BackendError::InvalidResult,
        SqliteTimerCommitErrorV1::RoomFaulted => BackendError::RoomFaulted,
        SqliteTimerCommitErrorV1::ConcurrentChange => BackendError::Busy,
        SqliteTimerCommitErrorV1::Rejected => BackendError::Rejected,
    }
}

fn timer_response_from_resolution(
    request: &TimerFiredRequestV1,
    resolution: &RoomCommitResolutionV1,
) -> Result<TimerFireResponse, BackendError> {
    if let Some(result) = resolution.stored_result() {
        if result.operation_identity() != &request.operation_identity()
            || result.canonical_request_hash()
                != &request
                    .canonical_request_hash()
                    .map_err(|_| BackendError::InvalidResult)?
        {
            return Err(BackendError::InvalidResult);
        }
        let SemanticResultV1::TransitionCommitted {
            transition_id,
            complete_head,
            ..
        } = result.result()
        else {
            return Err(BackendError::InvalidResult);
        };
        return Ok(TimerFireResponse {
            room_id: request.room_id().to_string(),
            timer_id: request.timer_id().to_string(),
            generation: request.generation().get(),
            transition_id: transition_id.to_string(),
            room_head: room_head(complete_head),
            duplicate: resolution.duplicate(),
        });
    }
    match resolution {
        RoomCommitResolutionV1::Conflict { .. } => Err(BackendError::Conflict),
        RoomCommitResolutionV1::Fenced
        | RoomCommitResolutionV1::Reprepare
        | RoomCommitResolutionV1::RetryableKnownAbsent
        | RoomCommitResolutionV1::NotApplicable => Err(BackendError::Busy),
        RoomCommitResolutionV1::Indeterminate => Err(BackendError::Indeterminate),
        RoomCommitResolutionV1::Fault
        | RoomCommitResolutionV1::GenesisCreated { .. }
        | RoomCommitResolutionV1::TransitionCommitted { .. }
        | RoomCommitResolutionV1::RejectionRecorded { .. }
        | RoomCommitResolutionV1::NoChangeRecorded { .. } => Err(BackendError::InvalidResult),
    }
}

fn action_reply_from_resolution(
    request: &ActionSubmit,
    resolution: &RoomCommitResolutionV1,
) -> Result<ActionReply, BackendError> {
    if let Some(result) = resolution.stored_result() {
        return action_reply_from_result(request, result, resolution.duplicate());
    }
    match resolution {
        RoomCommitResolutionV1::Conflict { .. } => Err(BackendError::Conflict),
        RoomCommitResolutionV1::Fenced
        | RoomCommitResolutionV1::Reprepare
        | RoomCommitResolutionV1::RetryableKnownAbsent => Err(BackendError::Busy),
        RoomCommitResolutionV1::Indeterminate => Err(BackendError::Indeterminate),
        RoomCommitResolutionV1::Fault | RoomCommitResolutionV1::NotApplicable => {
            Err(BackendError::InvalidResult)
        }
        RoomCommitResolutionV1::GenesisCreated { .. }
        | RoomCommitResolutionV1::TransitionCommitted { .. }
        | RoomCommitResolutionV1::RejectionRecorded { .. }
        | RoomCommitResolutionV1::NoChangeRecorded { .. } => Err(BackendError::InvalidResult),
    }
}

fn action_reply_from_result(
    request: &ActionSubmit,
    result: &StoredSemanticResultV1,
    duplicate: bool,
) -> Result<ActionReply, BackendError> {
    let admitted_at = match result.semantic_time() {
        ReceiptSemanticTimeV1::ActionAdmitted(value) => value.as_str().to_owned(),
        ReceiptSemanticTimeV1::Creation(_)
        | ReceiptSemanticTimeV1::TimerScheduled(_)
        | ReceiptSemanticTimeV1::CoreRecorded(_)
        | ReceiptSemanticTimeV1::ExternalInputRecorded(_) => {
            return Err(BackendError::InvalidResult);
        }
    };
    match result.result() {
        SemanticResultV1::TransitionCommitted {
            transition_id,
            complete_head,
            ..
        } => Ok(ActionReply::Accepted(ActionAccepted {
            room_id: request.room_id.clone(),
            member_id: request.member_id.clone(),
            action_id: request.action_id.clone(),
            transition_id: transition_id.to_string(),
            admitted_at,
            room_head: room_head(complete_head),
            duplicate,
        })),
        SemanticResultV1::RejectionRecorded { code, safe_details }
        | SemanticResultV1::NoChangeRecorded { code, safe_details } => {
            let current_room_seq = result
                .basis_complete_head()
                .ok_or(BackendError::InvalidResult)?
                .room_seq()
                .get();
            Ok(ActionReply::Rejected(ActionRejected {
                room_id: request.room_id.clone(),
                member_id: request.member_id.clone(),
                action_id: request.action_id.clone(),
                admitted_at,
                code: code.clone(),
                message: action_rejection_message(code).to_owned(),
                current_room_seq,
                action_offers: action_offers_from_result(result)?,
                retryable_with_same_action_id: false,
                may_submit_revised_action: true,
                duplicate,
                details: canonical_value(safe_details)?,
            }))
        }
        SemanticResultV1::GenesisCreated { .. } => Err(BackendError::InvalidResult),
    }
}

fn action_offers_from_result(
    result: &StoredSemanticResultV1,
) -> Result<Vec<ActionOffer>, BackendError> {
    let ReceiptSemanticInputV1::ParticipantAction {
        action_offer_witness,
        ..
    } = result.semantic_input()
    else {
        return Err(BackendError::InvalidResult);
    };
    match action_offer_witness {
        ActionOfferWitnessV1::Current {
            canonical_action_offers,
        } => serde_json::from_slice(
            &canonical_action_offers
                .to_bytes()
                .map_err(|_| BackendError::InvalidResult)?,
        )
        .map_err(|_| BackendError::InvalidResult),
        ActionOfferWitnessV1::Unavailable { .. } => Ok(Vec::new()),
    }
}

fn canonical_value(value: &CanonicalJsonV1) -> Result<Value, BackendError> {
    serde_json::from_slice(&value.to_bytes().map_err(|_| BackendError::InvalidResult)?)
        .map_err(|_| BackendError::InvalidResult)
}

fn action_rejection_message(code: &str) -> &'static str {
    match code {
        "membership_not_enabled" => "the membership is not enabled",
        "room_archived" => "the room is archived",
        "action_not_allowed" => "the action is not currently offered",
        "stale_room_state" => "the action was based on stale room state",
        "deadline_passed" => "the action deadline has passed",
        "activity_domain_rejection" => "the activity rejected the action",
        _ => "the action was rejected",
    }
}

fn next_core_id<T>() -> Result<T, BackendError>
where
    T: FromStr,
{
    crate::next_ulid()
        .ok_or(BackendError::Indeterminate)?
        .to_string()
        .parse()
        .map_err(|_| BackendError::Indeterminate)
}

fn random_room_seed() -> Result<RoomSeedV1, BackendError> {
    let mut bytes = [0_u8; 32];
    fill_random_bytes(&mut bytes).map_err(|_| BackendError::StorageUnavailable)?;
    let mut text = String::from("hex:");
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    RoomSeedV1::from_str(&text).map_err(|_| BackendError::InvalidResult)
}

fn random_bearer_bytes() -> Result<[u8; 32], BackendError> {
    let mut bytes = [0_u8; 32];
    fill_random_bytes(&mut bytes).map_err(|_| BackendError::StorageUnavailable)?;
    Ok(bytes)
}

fn now_text() -> Result<String, BackendError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| BackendError::StorageUnavailable)
}

fn creation_time() -> Result<CreationRecordedAt, BackendError> {
    // Pack timer arithmetic preserves whole-second values exactly. Sampling
    // Genesis at that precision therefore cannot reintroduce non-canonical
    // fractional padding when the frozen pack derives its first deadline.
    let value = OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .map_err(|_| BackendError::StorageUnavailable)?
        .format(&Rfc3339)
        .map_err(|_| BackendError::StorageUnavailable)?;
    CreationRecordedAt::from_str(&value).map_err(|_| BackendError::StorageUnavailable)
}

fn core_principal_kind(kind: PrincipalKind) -> PrincipalKindV1 {
    match kind {
        PrincipalKind::Human => PrincipalKindV1::Human,
        PrincipalKind::Agent => PrincipalKindV1::Agent,
    }
}

fn core_access_mode(mode: AccessMode) -> AccessModeV1 {
    match mode {
        AccessMode::Participant => AccessModeV1::Participant,
        AccessMode::Spectator => AccessModeV1::Spectator,
        AccessMode::Operator => AccessModeV1::Operator,
    }
}

fn create_response_from_result(
    result: &StoredSemanticResultV1,
) -> Result<CreateRoomResponse, BackendError> {
    match result.result() {
        SemanticResultV1::GenesisCreated {
            room_id,
            initial_member_ids,
            complete_head,
        } if complete_head.room_id() == room_id && complete_head.room_seq().get() == 0 => {
            Ok(CreateRoomResponse {
                room_id: room_id.to_string(),
                member_ids: initial_member_ids.iter().map(ToString::to_string).collect(),
                room_head: room_head(complete_head),
            })
        }
        _ => Err(BackendError::InvalidResult),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_room_operation_error(error: worldstream_core::RoomOperationIngressErrorV1) -> BackendError {
    match error {
        worldstream_core::RoomOperationIngressErrorV1::Authority(error) => {
            map_authority_error(error)
        }
        worldstream_core::RoomOperationIngressErrorV1::ResolutionUnavailable => {
            BackendError::Indeterminate
        }
        worldstream_core::RoomOperationIngressErrorV1::InvalidStoredResult => {
            BackendError::InvalidResult
        }
    }
}

struct AttachContext {
    snapshot: worldstream_sqlite::SqliteGatewayRoomSnapshotV1,
    view: worldstream_core::ValidatedPackViewV1,
    membership: worldstream_core::MembershipV1,
    capability_id: worldstream_core::CapabilityId,
}

impl GatewayBackend for SqliteGatewayBackend {
    fn admission_principal(&self, session: &GatewaySession) -> Result<String, BackendError> {
        Ok(self.authenticate(session)?.principal_id().to_string())
    }

    fn hello(
        &self,
        session: &GatewaySession,
        hello: &ClientHello,
    ) -> Result<ServerWelcome, BackendError> {
        if !hello
            .supported_protocols
            .iter()
            .any(|protocol| protocol == PROTOCOL_VERSION)
        {
            return Err(BackendError::Rejected);
        }
        let authenticated = self.authenticate(session)?;
        Ok(ServerWelcome {
            session_id: session.session_id().clone(),
            selected_protocol: PROTOCOL_VERSION.to_owned(),
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
            heartbeat_interval_ms: 30_000,
            maximum_message_bytes: MAX_MESSAGE_BYTES,
            authenticated_principal: Principal {
                principal_id: authenticated.principal_id().to_string(),
                kind: protocol_principal_kind(authenticated.principal_kind()),
            },
        })
    }

    fn retire_session(&self, session_id: &worldstream_protocol::UlidString) {
        self.bindings.retire(session_id);
    }

    fn create_room(
        &self,
        session: &GatewaySession,
        request: CreateRoomRequest,
    ) -> Result<CreateRoomResponse, BackendError> {
        for _ in 0..3 {
            match self.create_room_attempt(session, &request)? {
                CreateAttempt::Response(response) => return Ok(*response),
                CreateAttempt::Retry => {}
            }
        }
        Err(BackendError::Busy)
    }

    fn projection(
        &self,
        session: &GatewaySession,
        room_id: &str,
    ) -> Result<ProjectionResponse, BackendError> {
        self.projection_response(session, room_id)
    }

    fn replay(
        &self,
        session: &GatewaySession,
        room_id: &str,
        at_room_seq: u64,
    ) -> Result<ReplayResponse, BackendError> {
        self.replay_response(session, room_id, at_room_seq)
    }

    fn attach(
        &self,
        session: &GatewaySession,
        request: RoomAttach,
    ) -> Result<AttachReply, BackendError> {
        let room_id = RoomId::from_str(&request.room_id).map_err(|_| BackendError::Rejected)?;
        // Authentication remains the first stateful boundary: an unknown or
        // passivating Room must not be distinguishable to an unauthenticated
        // transport session.
        let _authenticated = self.authenticate(session)?;
        let member_id = request
            .member_id
            .parse()
            .map_err(|_| BackendError::Rejected)?;
        let context = self.attach_view(session, &room_id, &member_id)?;
        let attach_grant = self
            .authority()
            .authorize_member_read(
                &self
                    .store
                    .authenticate_bearer(
                        session
                            .owned_bearer()
                            .ok_or(BackendError::StorageUnavailable)?,
                    )
                    .map_err(|error| map_gateway_error(&error))?
                    .into_presented(),
                room_id.clone(),
                member_id.clone(),
                MemberReadOperationV1::Attach,
                Self::checked_at()?,
            )
            .map_err(map_authority_error)?;
        let captured = self
            .store
            .attach_observations(attach_grant, context.view.clone())
            .map_err(map_observation_error)?;
        if captured.room_head() != context.snapshot.trace().head() {
            return Err(BackendError::Busy);
        }
        if request.after_frame_seq != captured.cursor() {
            return Err(BackendError::Rejected);
        }
        let sync_token = self.issue_sync_token(
            session,
            &context.capability_id,
            &room_id,
            &member_id,
            &captured,
        )?;
        let current = self
            .readable_room_snapshot(&room_id)?
            .ok_or(BackendError::NotFound)?;
        if current.trace().head() != captured.room_head()
            || current.integrity() != context.snapshot.integrity()
        {
            return Err(BackendError::Busy);
        }
        let (sync, reset, frames) = protocol_delivery(
            &captured,
            &room_id,
            &member_id,
            current.integrity_generation().get(),
            integrity_status(current.integrity().status()),
        )?;
        let trace = current.trace();
        let descriptor = self
            .registry
            .load_retained(trace.head().pack_digest())
            .map_err(|_| BackendError::InvalidResult)?
            .descriptor();
        Ok(AttachReply {
            attached: RoomAttached {
                room_id: room_id.to_string(),
                member_id: member_id.to_string(),
                principal_kind: protocol_principal_kind(context.membership.principal_kind()),
                access_mode: protocol_access_mode(context.membership.access_mode()),
                role: context.membership.role().map(str::to_owned),
                membership_status: membership_status(context.membership.standing()),
                room_status: room_status(trace.core_state().room_status()),
                room_health: integrity_status(current.integrity().status()),
                integrity_generation: current.integrity_generation().get(),
                room_head: room_head(captured.room_head()),
                cursor: captured.cursor(),
                frame_head: captured.frame_head(),
                retained_floor: captured.retained_floor(),
                sync_token,
                sync,
                pack: worldstream_protocol::PackReference {
                    id: descriptor.pack_id.clone(),
                    version: descriptor.explanatory_version.clone(),
                    digest: captured.room_head().pack_digest().to_string(),
                },
            },
            reset,
            frames,
        })
    }

    fn sync_ack(
        &self,
        session: &GatewaySession,
        request: RoomSyncAck,
    ) -> Result<Vec<ObservationDeliver>, BackendError> {
        let room_id = RoomId::from_str(&request.room_id).map_err(|_| BackendError::Rejected)?;
        let member_id = request
            .member_id
            .parse()
            .map_err(|_| BackendError::Rejected)?;
        let authenticated = self.authenticate(session)?;
        let capability_id = authenticated.presented().capability_id().clone();
        let mut binding = self.bindings.take(
            session.session_id(),
            &capability_id,
            &room_id,
            &member_id,
            &request.sync_token,
        )?;
        let result = (|| {
            let grant = self
                .authority()
                .authorize_member_read(
                    &authenticated.into_presented(),
                    room_id.clone(),
                    member_id.clone(),
                    MemberReadOperationV1::CatchUp,
                    Self::checked_at()?,
                )
                .map_err(map_authority_error)?;
            let frames = self
                .store
                .read_observation_suffix(grant, binding.baseline_frame_head)
                .map_err(map_observation_error)?;
            let mut by_sequence = BTreeMap::new();
            for frame in frames {
                let sequence = frame.frame_seq();
                binding
                    .session
                    .publish(
                        SessionFrameV1::new(sequence).map_err(|_| BackendError::InvalidResult)?,
                    )
                    .map_err(map_session_error)?;
                by_sequence.insert(sequence, frame);
            }
            let flushed = binding
                .session
                .sync_ack(&binding.core_token, request.through_frame_head)
                .map_err(map_session_error)?;
            flushed
                .into_iter()
                .map(|frame| {
                    by_sequence
                        .remove(&frame.frame_seq())
                        .ok_or(BackendError::InvalidResult)
                        .and_then(|frame| observation_deliver(&frame, &room_id, &member_id))
                })
                .collect()
        })();
        match result {
            Ok(frames) => Ok(frames),
            Err(error) => {
                self.bindings
                    .restore(session.session_id(), &capability_id, binding)?;
                Err(error)
            }
        }
    }

    fn live_observation_suffix(
        &self,
        session: &GatewaySession,
        room_id: &str,
        member_id: &str,
        after_frame_seq: u64,
    ) -> Result<Vec<ObservationDeliver>, BackendError> {
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        let member_id = member_id
            .parse::<worldstream_core::MemberId>()
            .map_err(|_| BackendError::Rejected)?;
        let authenticated = self.authenticate(session)?;
        let grant = self
            .authority()
            .authorize_member_read(
                &authenticated.into_presented(),
                room_id.clone(),
                member_id.clone(),
                MemberReadOperationV1::CatchUp,
                Self::checked_at()?,
            )
            .map_err(map_authority_error)?;
        self.store
            .read_observation_suffix(grant, after_frame_seq)
            .map_err(map_observation_error)?
            .into_iter()
            .map(|frame| observation_deliver(&frame, &room_id, &member_id))
            .collect()
    }

    fn observation_ack(
        &self,
        session: &GatewaySession,
        request: ObservationAck,
    ) -> Result<Option<u64>, BackendError> {
        let room_id = RoomId::from_str(&request.room_id).map_err(|_| BackendError::Rejected)?;
        let member_id = request
            .member_id
            .parse()
            .map_err(|_| BackendError::Rejected)?;
        let authenticated = self.authenticate(session)?;
        let presented = authenticated.into_presented();
        self.ensure_verified_active(&room_id)?;
        let grant = self
            .authority()
            .authorize_member_read(
                &presented,
                room_id,
                member_id,
                MemberReadOperationV1::AcknowledgeObservation,
                Self::checked_at()?,
            )
            .map_err(map_authority_error)?;
        self.store
            .acknowledge_observation(grant, request.through_frame_seq)
            .map_err(map_observation_error)
    }

    fn action(
        &self,
        session: &GatewaySession,
        request: ActionSubmit,
    ) -> Result<ActionReply, BackendError> {
        request
            .validate_bounds()
            .map_err(|_| BackendError::Rejected)?;
        let action_room_id =
            RoomId::from_str(&request.room_id).map_err(|_| BackendError::Rejected)?;
        let core_request = participant_action_request(&request)?;
        let authenticated = self.authenticate(session)?;
        self.ensure_verified_active(&action_room_id)?;
        let operation = self.acquire_room_operation(&action_room_id)?;
        let ingress = authorize_participant_action_operation(
            &self.authority(),
            &self.store,
            &authenticated.into_presented(),
            &core_request,
            Self::checked_at()?,
        )
        .map_err(|error| map_participant_action_ingress_error(&error))?;

        match ingress {
            ParticipantActionIngressV1::Existing(result) => {
                operation.publish().map_err(Self::map_supervisor_error)?;
                action_reply_from_result(&request, &result, true)
            }
            ParticipantActionIngressV1::Conflict { .. } => Err(BackendError::Conflict),
            ParticipantActionIngressV1::Authorized(grant) => {
                let admitted_at = ActionAdmittedAt::from_str(&now_text()?)
                    .map_err(|_| BackendError::StorageUnavailable)?;
                let transition_id = next_core_id::<TransitionId>()?;
                let resolution = self
                    .store
                    .commit_authorized_participant_action(
                        &self.registry,
                        *grant,
                        &core_request,
                        admitted_at,
                        transition_id,
                    )
                    .map_err(|error| map_participant_action_error(&error))?;
                operation.publish().map_err(Self::map_supervisor_error)?;
                action_reply_from_resolution(&request, &resolution)
            }
        }
    }

    fn fire_timer(
        &self,
        session: &GatewaySession,
        room_id: &str,
        request: TimerFireRequest,
    ) -> Result<TimerFireResponse, BackendError> {
        request
            .validate_bounds()
            .map_err(|_| BackendError::Rejected)?;
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        let authenticated = self.authenticate(session)?;
        let lifecycle = self.verified_room_lifecycle(&room_id)?;
        if lifecycle == SqliteRoomRuntimeStateV1::CatchingUp
            && !self
                .recoveries
                .lock()
                .map_err(|_| BackendError::StorageUnavailable)?
                .contains_key(room_id.as_str())
        {
            return Err(BackendError::Busy);
        }
        let operation = if lifecycle == SqliteRoomRuntimeStateV1::Active {
            Some(self.acquire_room_operation(&room_id)?)
        } else {
            None
        };
        let timer_id = TimerId::from_str(&request.timer_id).map_err(|_| BackendError::Rejected)?;
        let generation =
            TimerGenerationV1::new(request.generation).map_err(|_| BackendError::Rejected)?;
        let candidate = self
            .store
            .timer_candidate(&room_id, &timer_id, generation)
            .map_err(|_| BackendError::StorageUnavailable)?
            .ok_or(BackendError::NotFound)?;
        let timer_request = candidate.request();
        let authority = self
            .authority()
            .authorize_timer_fired(
                &authenticated.into_presented(),
                timer_request,
                Self::checked_at()?,
            )
            .map_err(map_authority_error)?;

        if candidate.state() == SqliteTimerStateV1::Fired {
            let operation = operation.ok_or(BackendError::Busy)?;
            let identity = timer_request.operation_identity();
            let request_hash = timer_request
                .canonical_request_hash()
                .map_err(|_| BackendError::InvalidResult)?;
            let response = match RoomCommitStorageV1::resolve(&self.store, &identity, &request_hash)
            {
                worldstream_core::ResolveOutcomeV1::StoredResolution(result) => {
                    timer_response_from_resolution(
                        timer_request,
                        &RoomCommitResolutionV1::resolved(
                            worldstream_core::ResolutionStatusV1::Existing,
                            *result,
                        ),
                    )
                }
                worldstream_core::ResolveOutcomeV1::Conflict { .. } => Err(BackendError::Conflict),
                worldstream_core::ResolveOutcomeV1::KnownAbsent => Err(BackendError::Busy),
                worldstream_core::ResolveOutcomeV1::ResolutionUnavailable => {
                    Err(BackendError::Indeterminate)
                }
            };
            let response = response?;
            operation.publish().map_err(Self::map_supervisor_error)?;
            return Ok(response);
        }
        if candidate.state() != SqliteTimerStateV1::Scheduled
            || !self
                .store
                .timer_candidate_is_due(&candidate)
                .map_err(|_| BackendError::StorageUnavailable)?
        {
            return Err(BackendError::Busy);
        }
        if lifecycle == SqliteRoomRuntimeStateV1::CatchingUp {
            return self.commit_catching_up_timer(&room_id, authority, timer_request);
        }
        let operation = operation.ok_or(BackendError::Busy)?;
        let transition_id = next_core_id::<TransitionId>()?;
        let resolution = self
            .store
            .commit_authorized_timer_fired(&self.registry, authority, timer_request, transition_id)
            .map_err(|error| map_timer_commit_error(&error))?;
        operation.publish().map_err(Self::map_supervisor_error)?;
        timer_response_from_resolution(timer_request, &resolution)
    }

    fn issue_member_capability(
        &self,
        session: &GatewaySession,
        request: MemberCapabilityIssueRequest,
    ) -> Result<MemberCapabilityIssueResponse, BackendError> {
        self.issue_member_capability_inner(session, request)
    }

    fn issue_runner_capability(
        &self,
        session: &GatewaySession,
        request: RunnerCapabilityIssueRequest,
    ) -> Result<RunnerCapabilityIssueResponse, BackendError> {
        self.issue_runner_capability_inner(session, request)
    }

    fn runner_hello(
        &self,
        session: &GatewaySession,
        request: RunnerHello,
    ) -> Result<RunnerReady, BackendError> {
        let _ = self.authenticate(session)?;
        request
            .runner_id
            .parse::<RunnerId>()
            .map_err(|_| BackendError::Rejected)?;
        if request.maximum_concurrent_activations == 0
            || request.maximum_concurrent_activations > 64
            || request.supported_pack_ids.len() > 64
            || request
                .supported_pack_ids
                .iter()
                .any(|id| id.is_empty() || id.len() > 256)
        {
            return Err(BackendError::Rejected);
        }
        Ok(RunnerReady {
            runner_id: request.runner_id,
        })
    }

    fn activation_offers(
        &self,
        session: &GatewaySession,
        request: ActivationOfferRequest,
    ) -> Result<ActivationOffers, BackendError> {
        let room_id = request
            .room_id
            .parse()
            .map_err(|_| BackendError::Rejected)?;
        let member_id = request
            .member_id
            .parse()
            .map_err(|_| BackendError::Rejected)?;
        let authority = self.authorize_runner_target(
            session,
            &request.runner_id,
            RunnerControlOperationV1::ReceiveOffer,
            room_id,
            member_id,
        )?;
        let operation = Self::activation_request(ActivationRequestParts {
            operation_kind: "offer".to_owned(),
            operation_id: request.operation_id.clone(),
            activation_id: None,
            claim_id: None,
            runner_id: request.runner_id.clone(),
            lease_generation: None,
            requested_lease_ms: None,
            disposition: None,
        });
        let result = self
            .store
            .offer_activations(authority, operation)
            .map_err(map_activation_error)?;
        Ok(ActivationOffers {
            operation_id: result.operation.operation_id,
            runner_id: result.operation.runner_id,
            offers: result
                .offers
                .into_iter()
                .map(|offer| ActivationOffer {
                    activation_id: offer.activation_id,
                    room_id: offer.room_id.to_string(),
                    member_id: offer.member_id.to_string(),
                    cause_room_seq: offer.cause_room_seq.get(),
                    reason_code: offer.reason_code,
                    priority: offer.priority,
                    deadline: offer.semantic_deadline,
                    lease_duration_ms: offer.maximum_lease_ms,
                })
                .collect(),
        })
    }

    fn activation_claim(
        &self,
        session: &GatewaySession,
        request: ActivationClaim,
    ) -> Result<ActivationOperationReply, BackendError> {
        // Authenticate before looking up the activation target; target
        // existence is private to the authenticated runner authority path.
        let _ = self.authenticate(session)?;
        let (room_id, member_id) = self
            .store
            .activation_target(&request.activation_id)
            .map_err(map_activation_error)?
            .ok_or(BackendError::NotFound)?;
        let authority = self.authorize_runner_target(
            session,
            &request.runner_id,
            RunnerControlOperationV1::Claim,
            room_id,
            member_id,
        )?;
        let operation = Self::activation_request(ActivationRequestParts {
            operation_kind: "claim".to_owned(),
            operation_id: request.claim_id.clone(),
            activation_id: Some(request.activation_id.clone()),
            claim_id: Some(request.claim_id.clone()),
            runner_id: request.runner_id,
            lease_generation: None,
            requested_lease_ms: Some(request.requested_lease_ms),
            disposition: None,
        });
        if let Some(result) = self
            .store
            .activation_claim_receipt(&authority.target().room_id, &operation)
            .map_err(map_activation_error)?
        {
            return Self::activation_reply(result);
        }
        let claim = self
            .store
            .prepare_activation_claim(&self.registry, &authority, operation)
            .map_err(map_activation_error)?;
        let runtime_state = self
            .store
            .room_runtime_state(&self.registry, &authority.target().room_id)
            .map_err(|_| BackendError::StorageUnavailable)?
            .ok_or(BackendError::NotFound)?;
        let result = self
            .store
            .claim_activation(authority, claim, runtime_state)
            .map_err(map_activation_error)?;
        Self::activation_reply(result)
    }

    fn activation_renew(
        &self,
        session: &GatewaySession,
        request: ActivationLeaseOperation,
    ) -> Result<ActivationOperationReply, BackendError> {
        self.activation_lease_operation(session, request, RunnerControlOperationV1::Renew, "renew")
    }

    fn activation_release(
        &self,
        session: &GatewaySession,
        request: ActivationLeaseOperation,
    ) -> Result<ActivationOperationReply, BackendError> {
        self.activation_lease_operation(
            session,
            request,
            RunnerControlOperationV1::Release,
            "release",
        )
    }

    fn activation_complete(
        &self,
        session: &GatewaySession,
        request: ActivationLeaseOperation,
    ) -> Result<ActivationOperationReply, BackendError> {
        self.activation_lease_operation(
            session,
            request,
            RunnerControlOperationV1::Complete,
            "complete",
        )
    }

    fn scheduler_tick(&self) -> Result<(), BackendError> {
        for room_id in self.store.room_ids().map_err(map_activation_error)? {
            if self.verified_room_lifecycle(&room_id)? == SqliteRoomRuntimeStateV1::CatchingUp {
                // Readiness covers the scheduler process, not ordinary Room
                // admission. An overdue Room remains gated until the exact
                // HostOperator Timer request drains its fixed-cutoff recovery.
                continue;
            }
            self.store
                .reclaim_activation_leases(room_id, None)
                .map_err(map_activation_error)?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct SessionBindings(Mutex<BTreeMap<worldstream_protocol::UlidString, SessionBinding>>);

const MAX_SESSION_BINDINGS: usize = 1_024;

impl fmt::Debug for SessionBindings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionBindings([REDACTED])")
    }
}

impl SessionBindings {
    fn issue(
        &self,
        session_id: &worldstream_protocol::UlidString,
        capability_id: &worldstream_core::CapabilityId,
        sync: SyncBinding,
    ) -> Result<(), BackendError> {
        let mut bindings = self
            .0
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?;
        if !bindings.contains_key(session_id) {
            if bindings.len() >= MAX_SESSION_BINDINGS {
                return Err(BackendError::StorageUnavailable);
            }
            bindings.insert(
                session_id.clone(),
                SessionBinding {
                    capability_id: capability_id.clone(),
                    sync: None,
                },
            );
        }
        let binding = bindings
            .get_mut(session_id)
            .ok_or(BackendError::StorageUnavailable)?;
        if binding.capability_id != *capability_id {
            return Err(BackendError::Forbidden);
        }
        if binding.sync.is_some() {
            return Err(BackendError::StorageUnavailable);
        }
        binding.sync = Some(sync);
        Ok(())
    }

    fn retire(&self, session_id: &worldstream_protocol::UlidString) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id);
    }

    fn take(
        &self,
        session_id: &worldstream_protocol::UlidString,
        capability_id: &worldstream_core::CapabilityId,
        room_id: &RoomId,
        member_id: &worldstream_core::MemberId,
        token: &str,
    ) -> Result<SyncBinding, BackendError> {
        let mut bindings = self
            .0
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?;
        let binding = bindings
            .get_mut(session_id)
            .ok_or(BackendError::Forbidden)?;
        if binding.capability_id != *capability_id {
            return Err(BackendError::Forbidden);
        }
        let sync = binding.sync.take().ok_or(BackendError::Rejected)?;
        if sync.token != token || sync.room_id != *room_id || sync.member_id != *member_id {
            binding.sync = Some(sync);
            return Err(BackendError::Rejected);
        }
        Ok(sync)
    }

    fn restore(
        &self,
        session_id: &worldstream_protocol::UlidString,
        capability_id: &worldstream_core::CapabilityId,
        sync: SyncBinding,
    ) -> Result<(), BackendError> {
        let mut bindings = self
            .0
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?;
        let binding = bindings
            .get_mut(session_id)
            .ok_or(BackendError::Forbidden)?;
        if binding.capability_id != *capability_id {
            return Err(BackendError::Forbidden);
        }
        if binding.sync.is_some() {
            return Err(BackendError::StorageUnavailable);
        }
        binding.sync = Some(sync);
        Ok(())
    }
}

struct SessionBinding {
    capability_id: worldstream_core::CapabilityId,
    sync: Option<SyncBinding>,
}

struct SyncBinding {
    token: String,
    room_id: RoomId,
    member_id: worldstream_core::MemberId,
    baseline_frame_head: u64,
    session: SessionV1,
    core_token: SessionSyncTokenV1,
}

fn viewer_for(membership: &worldstream_core::MembershipV1) -> PackViewerV1 {
    match membership.access_mode() {
        AccessModeV1::Participant => PackViewerV1::Participant(membership.member_id().clone()),
        AccessModeV1::Spectator => PackViewerV1::Public(membership.member_id().clone()),
        AccessModeV1::Operator => PackViewerV1::Operator(membership.member_id().clone()),
    }
}

fn protocol_access_mode(mode: AccessModeV1) -> AccessMode {
    match mode {
        AccessModeV1::Participant => AccessMode::Participant,
        AccessModeV1::Spectator => AccessMode::Spectator,
        AccessModeV1::Operator => AccessMode::Operator,
    }
}

fn membership_status(standing: worldstream_core::MembershipStandingV1) -> String {
    match standing {
        worldstream_core::MembershipStandingV1::Enabled => "enabled",
        worldstream_core::MembershipStandingV1::Suspended => "suspended",
        worldstream_core::MembershipStandingV1::Departed => "departed",
    }
    .to_owned()
}

fn room_status(status: worldstream_core::RoomStatusV1) -> String {
    match status {
        worldstream_core::RoomStatusV1::Active => "active",
        worldstream_core::RoomStatusV1::Archived => "archived",
    }
    .to_owned()
}

fn integrity_status(status: worldstream_core::RoomIntegrityStatusV1) -> String {
    match status {
        worldstream_core::RoomIntegrityStatusV1::Healthy => "healthy",
        worldstream_core::RoomIntegrityStatusV1::Faulted => "faulted",
        worldstream_core::RoomIntegrityStatusV1::Quarantined => "quarantined",
    }
    .to_owned()
}

fn protocol_delivery(
    captured: &worldstream_sqlite::SqliteObservationAttachV1,
    room_id: &RoomId,
    member_id: &worldstream_core::MemberId,
    integrity_generation: u64,
    room_health: String,
) -> Result<(SyncBranch, Option<ProjectionReset>, Vec<ObservationDeliver>), BackendError> {
    match captured.delivery() {
        SqliteObservationDeliveryV1::Retained {
            cursor_exclusive,
            through_frame_head,
            frames,
        } => Ok((
            SyncBranch::RetainedFrames {
                cursor_exclusive: *cursor_exclusive,
                through_frame_head: *through_frame_head,
            },
            None,
            frames
                .iter()
                .map(|frame| observation_deliver(frame, room_id, member_id))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        SqliteObservationDeliveryV1::Reset {
            baseline_frame_head,
            reason,
            projection_reset,
        } => {
            let projection = projection_from_canonical_bytes(projection_reset.canonical_bytes())?;
            let reset = ProjectionReset {
                room_id: room_id.to_string(),
                member_id: member_id.to_string(),
                room_head: room_head(projection_reset.complete_head()),
                room_health,
                integrity_generation,
                baseline_frame_head: *baseline_frame_head,
                reset_reason: reset_reason(*reason).to_owned(),
                projection_schema: projection_reset.projection_schema().to_owned(),
                projection,
                projection_hash: projection_reset.projection_hash().to_string(),
            };
            Ok((
                SyncBranch::ProjectionReset {
                    baseline_frame_head: *baseline_frame_head,
                    reason: reset_reason(*reason).to_owned(),
                },
                Some(reset),
                Vec::new(),
            ))
        }
    }
}

fn reset_reason(reason: SqliteObservationResetReasonV1) -> &'static str {
    match reason {
        SqliteObservationResetReasonV1::FirstAttach => "first_attach",
        SqliteObservationResetReasonV1::RetainedRangeUnavailable => "retained_range_unavailable",
        SqliteObservationResetReasonV1::ResetMarked => "reset_marked",
    }
}

fn observation_deliver(
    frame: &SqliteObservationFrameV1,
    room_id: &RoomId,
    member_id: &worldstream_core::MemberId,
) -> Result<ObservationDeliver, BackendError> {
    let (observation_schema, observation) = observation_from_payload(frame.payload_bytes())?;
    Ok(ObservationDeliver {
        room_id: room_id.to_string(),
        member_id: member_id.to_string(),
        frame_seq: frame.frame_seq(),
        cause_room_seq: frame.cause_room_seq().get(),
        frame_kind: "delta".to_owned(),
        observation_schema: observation_schema.clone(),
        observation,
        frame_payload_hash: frame.payload_hash().to_string(),
    })
}

fn observation_from_payload(bytes: &[u8]) -> Result<(String, Value), BackendError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| BackendError::InvalidResult)?;
    let object = value.as_object().ok_or(BackendError::InvalidResult)?;
    let observation_schema = object
        .get("observation_schema")
        .and_then(Value::as_str)
        .ok_or(BackendError::InvalidResult)?
        .to_owned();
    let mut observation = object
        .get("observation")
        .cloned()
        .ok_or(BackendError::InvalidResult)?;
    let Some(action_offers) = object.get("action_offers") else {
        return Ok((observation_schema, observation));
    };
    if action_offers.is_null() {
        return Ok((observation_schema, observation));
    }
    if !action_offers.is_array() {
        return Err(BackendError::InvalidResult);
    }
    let observation_object = observation
        .as_object_mut()
        .ok_or(BackendError::InvalidResult)?;
    if observation_object.contains_key("action_offers") {
        return Err(BackendError::InvalidResult);
    }
    observation_object.insert("action_offers".to_owned(), action_offers.clone());
    Ok((observation_schema, observation))
}

fn map_session_error(error: SessionErrorV1) -> BackendError {
    match error {
        SessionErrorV1::SlowConsumer => BackendError::Busy,
        SessionErrorV1::ZeroBufferCapacity
        | SessionErrorV1::CursorAheadOfFrameHead
        | SessionErrorV1::InvalidRetainedFloor
        | SessionErrorV1::InvalidFrameSequence
        | SessionErrorV1::InvalidState
        | SessionErrorV1::FrameNotPostBarrier
        | SessionErrorV1::DuplicateBufferedFrame
        | SessionErrorV1::FrameSequenceGap
        | SessionErrorV1::FrameSequenceStale
        | SessionErrorV1::SyncTokenMismatch
        | SessionErrorV1::SyncBaselineMismatch
        | SessionErrorV1::SyncAlreadyAcknowledged
        | SessionErrorV1::SessionNonceExhausted
        | SessionErrorV1::TokenIssuanceFailed => BackendError::Rejected,
    }
}

fn projection_from_view(
    view: &worldstream_core::ValidatedPackViewV1,
) -> Result<Projection, BackendError> {
    projection_from_canonical_bytes(view.canonical_bytes())
}

fn projection_from_canonical_bytes(bytes: &[u8]) -> Result<Projection, BackendError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| BackendError::InvalidResult)?;
    let object = value.as_object().ok_or(BackendError::InvalidResult)?;
    serde_json::from_value(json!({
        "core": object.get("authorized_core").cloned().ok_or(BackendError::InvalidResult)?,
        "activity": object.get("projection").cloned().ok_or(BackendError::InvalidResult)?,
        "action_offers": object.get("action_offers").cloned().ok_or(BackendError::InvalidResult)?,
    }))
    .map_err(|_| BackendError::InvalidResult)
}

fn replay_view_bytes(envelope: &CanonicalJsonV1) -> Result<Vec<u8>, BackendError> {
    let bytes = envelope
        .to_bytes()
        .map_err(|_| BackendError::InvalidResult)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| BackendError::InvalidResult)?;
    let activity_projection = value
        .get("historical_projection")
        .and_then(Value::as_object)
        .and_then(|historical| historical.get("activity_projection"))
        .ok_or(BackendError::InvalidResult)?;
    let view_bytes =
        serde_json::to_vec(activity_projection).map_err(|_| BackendError::InvalidResult)?;
    CanonicalJsonV1::from_canonical_bytes(&view_bytes)
        .map_err(|_| BackendError::InvalidResult)?
        .to_bytes()
        .map_err(|_| BackendError::InvalidResult)
}

fn room_head(head: &worldstream_core::CompleteHeadV1) -> RoomHead {
    RoomHead {
        room_id: head.room_id().to_string(),
        room_seq: head.room_seq().get(),
        genesis_or_transition_hash: head.genesis_or_transition_hash().to_string(),
        core_schema_version: head.core_schema_version().to_owned(),
        pack_digest: head.pack_digest().to_string(),
        core_state_hash: head.core_state_hash().to_string(),
        activity_state_hash: head.activity_state_hash().to_string(),
        authoritative_state_hash: head.authoritative_state_hash().to_string(),
    }
}

fn protocol_principal_kind(kind: PrincipalKindV1) -> PrincipalKind {
    match kind {
        PrincipalKindV1::Human => PrincipalKind::Human,
        PrincipalKindV1::Agent => PrincipalKind::Agent,
    }
}

fn map_gateway_error(error: &SqliteGatewayErrorV1) -> BackendError {
    match error {
        SqliteGatewayErrorV1::StorageUnavailable | SqliteGatewayErrorV1::Corrupt => {
            BackendError::StorageUnavailable
        }
        SqliteGatewayErrorV1::Unauthenticated => BackendError::Forbidden,
        SqliteGatewayErrorV1::RoomUnavailable => BackendError::NotFound,
        SqliteGatewayErrorV1::IntegrityUnavailable => BackendError::RoomQuarantined,
        SqliteGatewayErrorV1::ConcurrentChange => BackendError::Busy,
    }
}

fn map_activation_error(error: SqliteActivationErrorV1) -> BackendError {
    match error {
        SqliteActivationErrorV1::Authority(error) => map_authority_error(error),
        SqliteActivationErrorV1::StorageUnavailable | SqliteActivationErrorV1::Corrupt => {
            BackendError::StorageUnavailable
        }
        SqliteActivationErrorV1::IdempotencyConflict => BackendError::Conflict,
        SqliteActivationErrorV1::Forbidden => BackendError::Forbidden,
        SqliteActivationErrorV1::Fenced
        | SqliteActivationErrorV1::StaleLease
        | SqliteActivationErrorV1::StaleContext => BackendError::Busy,
        SqliteActivationErrorV1::InvalidRequest => BackendError::Rejected,
    }
}

fn activation_result_code(value: ActivationResultCodeV1) -> ActivationResultCode {
    match value {
        ActivationResultCodeV1::Granted => ActivationResultCode::Granted,
        ActivationResultCodeV1::Renewed => ActivationResultCode::Renewed,
        ActivationResultCodeV1::Released => ActivationResultCode::Released,
        ActivationResultCodeV1::Completed => ActivationResultCode::Completed,
        ActivationResultCodeV1::NotAvailable => ActivationResultCode::NotAvailable,
        ActivationResultCodeV1::Expired => ActivationResultCode::Expired,
        ActivationResultCodeV1::Cancelled => ActivationResultCode::Cancelled,
        ActivationResultCodeV1::Fenced => ActivationResultCode::Fenced,
        ActivationResultCodeV1::StaleLease => ActivationResultCode::StaleLease,
        ActivationResultCodeV1::IdempotencyConflict => ActivationResultCode::IdempotencyConflict,
        ActivationResultCodeV1::ResultRetired => ActivationResultCode::ResultRetired,
    }
}

fn activation_state(value: ActivationIntentStateV1) -> ActivationIntentState {
    match value {
        ActivationIntentStateV1::Pending => ActivationIntentState::Pending,
        ActivationIntentStateV1::Leased => ActivationIntentState::Leased,
        ActivationIntentStateV1::Completed => ActivationIntentState::Completed,
        ActivationIntentStateV1::Expired => ActivationIntentState::Expired,
        ActivationIntentStateV1::Cancelled => ActivationIntentState::Cancelled,
    }
}

fn activation_context(
    value: worldstream_core::ActivationInvocationContextV1,
) -> Result<worldstream_protocol::ActivationInvocationContext, BackendError> {
    let projection = projection_from_canonical_bytes(&value.projection_bytes)?;
    let runner_budget = serde_json::from_slice(&value.runner_budget_bytes)
        .map_err(|_| BackendError::InvalidResult)?;
    let runner_limits = serde_json::from_slice(&value.runner_limits_bytes)
        .map_err(|_| BackendError::InvalidResult)?;
    let artifact_references = value
        .artifact_references
        .iter()
        .map(|reference| {
            serde_json::from_slice(
                &reference
                    .to_bytes()
                    .map_err(|_| BackendError::InvalidResult)?,
            )
            .map_err(|_| BackendError::InvalidResult)
        })
        .collect::<Result<Vec<Value>, _>>()?;
    let delivery = match value.delivery {
        worldstream_core::ActivationDeliveryV1::RetainedFrames {
            cursor_exclusive,
            through_frame_head,
            frames,
        } => ActivationDelivery::RetainedFrames {
            cursor_exclusive,
            through_frame_head,
            frames: frames
                .into_iter()
                .map(|frame| {
                    let payload = serde_json::from_slice(&frame.payload_bytes)
                        .map_err(|_| BackendError::InvalidResult)?;
                    Ok(ActivationFrame {
                        frame_seq: frame.frame_seq,
                        cause_room_seq: frame.cause_room_seq.get(),
                        payload_hash: frame.payload_hash.to_string(),
                        payload,
                    })
                })
                .collect::<Result<Vec<_>, BackendError>>()?,
        },
        worldstream_core::ActivationDeliveryV1::ProjectionReset {
            baseline_frame_head,
            reason,
        } => ActivationDelivery::ProjectionReset {
            baseline_frame_head,
            reason,
        },
    };
    let action_offers = projection.action_offers.clone();
    let projection = serde_json::to_value(projection).map_err(|_| BackendError::InvalidResult)?;
    Ok(worldstream_protocol::ActivationInvocationContext {
        activation_id: value.activation_id,
        claim_id: value.claim_id,
        cause_room_seq: value.cause_room_seq.get(),
        reason_code: value.reason_code,
        lease_generation: value.lease_generation,
        lease_until: value.lease_until,
        deadline: value.semantic_deadline.map(|deadline| deadline.to_string()),
        room_head: room_head(&value.room_head),
        integrity_generation: value.integrity_generation,
        policy_revision: value.policy_revision,
        authority_generation: value.authority_generation,
        membership_generation: value.membership_generation,
        frame_head: value.frame_head,
        retained_floor: value.retained_floor,
        cursor: value.cursor,
        projection_schema: value.projection_schema,
        projection,
        action_offers,
        runner_budget,
        runner_limits,
        artifact_references,
        delivery,
    })
}

fn map_authority_error(error: AuthorityErrorV1) -> BackendError {
    match error {
        AuthorityErrorV1::Unauthenticated
        | AuthorityErrorV1::Forbidden
        | AuthorityErrorV1::MembershipNotEnabled => BackendError::Forbidden,
        AuthorityErrorV1::InvalidAuthorityRequest => BackendError::Rejected,
        AuthorityErrorV1::StaleAuthorityGeneration => BackendError::Busy,
        AuthorityErrorV1::Conflict => BackendError::Conflict,
        AuthorityErrorV1::Unavailable => BackendError::StorageUnavailable,
    }
}

fn map_observation_error(error: SqliteObservationErrorV1) -> BackendError {
    match error {
        SqliteObservationErrorV1::Authority(error) => map_authority_error(error),
        SqliteObservationErrorV1::StorageUnavailable | SqliteObservationErrorV1::Corrupt => {
            BackendError::StorageUnavailable
        }
        SqliteObservationErrorV1::CursorAhead => BackendError::Rejected,
        SqliteObservationErrorV1::MembershipUnavailable => BackendError::NotFound,
        SqliteObservationErrorV1::RoomQuarantined => BackendError::RoomQuarantined,
        SqliteObservationErrorV1::WrongOperation | SqliteObservationErrorV1::StaleView => {
            BackendError::Busy
        }
    }
}

fn map_replay_error(error: SqliteAuthorizedReplayErrorV1) -> BackendError {
    match error {
        SqliteAuthorizedReplayErrorV1::Authority(error) => map_authority_error(error),
        SqliteAuthorizedReplayErrorV1::StorageUnavailable
        | SqliteAuthorizedReplayErrorV1::Corrupt
        | SqliteAuthorizedReplayErrorV1::Replay(
            HistoricalReplayErrorV1::AddressMismatch
            | HistoricalReplayErrorV1::SequenceUnavailable
            | HistoricalReplayErrorV1::ReplayFailed(_)
            | HistoricalReplayErrorV1::ProjectionUnavailable,
        ) => BackendError::StorageUnavailable,
        SqliteAuthorizedReplayErrorV1::Replay(HistoricalReplayErrorV1::IntegrityUnavailable) => {
            BackendError::RoomQuarantined
        }
        SqliteAuthorizedReplayErrorV1::Replay(
            HistoricalReplayErrorV1::HistoricalMembershipUnavailable,
        ) => BackendError::Forbidden,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use worldstream_core::{
        AuthorityBootstrapV1, AuthorityChangeV1, AuthorityCheckedAt, AuthorityV1,
        CapabilityBearerV1, CapabilityId, CapabilityProfileV1, CapabilityScopeSetV1,
        CapabilityScopeV1, NewCapabilityV1, PresentedCapabilityV1, PrincipalKindV1,
        builtin_counter_registry, counter_v2_digest,
    };
    use worldstream_protocol::{
        AccessMode, BearerWireV1, CreateMember, PackReference, PrincipalKind,
    };

    fn session(value: u8, id: &str) -> GatewaySession {
        let id = id.parse().unwrap_or_else(|_| panic!("test session id"));
        let wire = BearerWireV1::from_bytes([value; 32]);
        let bearer = CapabilityBearerV1::from_bytes(
            BearerWireV1::parse(&wire.to_wire())
                .unwrap_or_else(|_| panic!("test bearer"))
                .into_bytes(),
        );
        GatewaySession::new_with_wire(id, bearer, wire)
    }

    #[test]
    fn genesis_creation_time_is_canonical_at_pack_safe_precision() {
        let value = creation_time().unwrap_or_else(|_| panic!("Genesis creation clock"));
        assert_eq!(value.as_str().len(), 20);
        assert!(value.as_str().ends_with('Z'));
        assert!(!value.as_str().contains('.'));
    }

    #[allow(clippy::too_many_lines)]
    #[test]
    fn sqlite_gateway_create_is_authorized_idempotent_and_conflict_safe() {
        let file = NamedTempFile::new().unwrap_or_else(|_| panic!("temp db"));
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        let principal = "01ARZ3NDEKTSV4RRFFQ69G5FC2"
            .parse::<worldstream_core::PrincipalId>()
            .unwrap_or_else(|_| panic!("principal"));
        let capability = "01ARZ3NDEKTSV4RRFFQ69G5FC3"
            .parse::<worldstream_core::CapabilityId>()
            .unwrap_or_else(|_| panic!("capability"));
        let bootstrap = AuthorityBootstrapV1::new(
            "01ARZ3NDEKTSV4RRFFQ69G5FC4"
                .parse()
                .unwrap_or_else(|_| panic!("change")),
            principal.clone(),
            PrincipalKindV1::Human,
            capability,
            bearer.token_hash(),
            None,
        )
        .unwrap_or_else(|_| panic!("bootstrap"));
        authority
            .bootstrap(
                bootstrap,
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|_| panic!("checked at")),
            )
            .unwrap_or_else(|_| panic!("apply bootstrap"));

        let registry =
            Arc::new(builtin_counter_registry().unwrap_or_else(|_| panic!("counter registry")));
        let descriptor = registry
            .load_retained(&counter_v2_digest())
            .unwrap_or_else(|_| panic!("counter pack"))
            .descriptor()
            .clone();
        let request = CreateRoomRequest {
            pack: PackReference {
                id: descriptor.pack_id,
                version: descriptor.explanatory_version,
                digest: counter_v2_digest().to_string(),
            },
            configuration: json!({"initial_value": 0, "maximum_value": 16}),
            members: vec![CreateMember {
                principal_id: principal.to_string(),
                principal_kind: PrincipalKind::Human,
                role: Some("counter".to_owned()),
                access_mode: AccessMode::Participant,
            }],
            idempotency_key: "gateway-create-1".to_owned(),
        };
        let backend = SqliteGatewayBackend::new(store, registry);
        let gateway_session = session(0xa9, "01ARZ3NDEKTSV4RRFFQ69G5FC5");
        let first = backend
            .create_room(&gateway_session, request.clone())
            .unwrap_or_else(|error| panic!("create Room: {error:?}"));
        assert_eq!(first.member_ids.len(), 1);
        assert_eq!(first.room_head.room_seq, 0);

        let duplicate = backend
            .create_room(&gateway_session, request.clone())
            .unwrap_or_else(|error| panic!("duplicate create: {error:?}"));
        assert_eq!(duplicate, first);

        let mut conflict = request;
        conflict.configuration = json!({"initial_value": 1, "maximum_value": 16});
        assert!(matches!(
            backend.create_room(&gateway_session, conflict),
            Err(BackendError::Conflict)
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn sqlite_gateway_action_is_authorized_duplicate_conflict_and_stale_safe() {
        let file = NamedTempFile::new().unwrap_or_else(|_| panic!("temp db"));
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let host_bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        let principal = "01ARZ3NDEKTSV4RRFFQ69G5FC2"
            .parse::<worldstream_core::PrincipalId>()
            .unwrap_or_else(|_| panic!("principal"));
        let host_capability = CapabilityId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FC3")
            .unwrap_or_else(|_| panic!("host capability"));
        authority
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FC4"
                        .parse()
                        .unwrap_or_else(|_| panic!("bootstrap change")),
                    principal.clone(),
                    PrincipalKindV1::Human,
                    host_capability.clone(),
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|_| panic!("bootstrap")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|_| panic!("bootstrap time")),
            )
            .unwrap_or_else(|_| panic!("apply bootstrap"));

        let registry =
            Arc::new(builtin_counter_registry().unwrap_or_else(|_| panic!("counter registry")));
        let descriptor = registry
            .load_retained(&counter_v2_digest())
            .unwrap_or_else(|_| panic!("counter pack"))
            .descriptor()
            .clone();
        let create = CreateRoomRequest {
            pack: PackReference {
                id: descriptor.pack_id,
                version: descriptor.explanatory_version,
                digest: counter_v2_digest().to_string(),
            },
            configuration: json!({"initial_value": 0, "maximum_value": 16}),
            members: vec![CreateMember {
                principal_id: principal.to_string(),
                principal_kind: PrincipalKind::Human,
                role: Some("counter".to_owned()),
                access_mode: AccessMode::Participant,
            }],
            idempotency_key: "gateway-action-create".to_owned(),
        };
        let backend = SqliteGatewayBackend::new(store, registry);
        let host_session = session(0xa9, "01ARZ3NDEKTSV4RRFFQ69G5FC5");
        let created = backend
            .create_room(&host_session, create)
            .unwrap_or_else(|error| panic!("create Room: {error:?}"));
        let room_id = created.room_id.clone();
        let member_id = created.member_ids[0].clone();
        let room_id_typed = room_id.parse().unwrap_or_else(|_| panic!("Room ID"));
        let member_id_typed = member_id.parse().unwrap_or_else(|_| panic!("Member ID"));
        let host_presented = PresentedCapabilityV1::new(host_capability, host_bearer);

        let member_capability = |capability_id: &str,
                                 token: [u8; 32],
                                 scopes: Vec<CapabilityScopeV1>,
                                 change_id: &str| {
            let capability = NewCapabilityV1::new(
                capability_id
                    .parse()
                    .unwrap_or_else(|_| panic!("member capability ID")),
                CapabilityBearerV1::from_bytes(token).token_hash(),
                principal.clone(),
                CapabilityProfileV1::RoomMember {
                    room_id: room_id_typed,
                    member_id: member_id_typed,
                },
                CapabilityScopeSetV1::new(scopes)
                    .unwrap_or_else(|_| panic!("member capability scopes")),
                None,
            )
            .unwrap_or_else(|_| panic!("member capability"));
            authority
                .change(
                    &host_presented,
                    AuthorityChangeV1::RegisterCapability {
                        change_id: change_id
                            .parse()
                            .unwrap_or_else(|_| panic!("capability change ID")),
                        capability,
                    },
                    "2026-08-15T12:00:01Z"
                        .parse()
                        .unwrap_or_else(|_| panic!("capability change time")),
                )
                .unwrap_or_else(|_| panic!("register member capability"));
        };
        member_capability(
            "01ARZ3NDEKTSV4RRFFQ69G5FC6",
            [0xb8; 32],
            vec![CapabilityScopeV1::RoomAct],
            "01ARZ3NDEKTSV4RRFFQ69G5FC7",
        );

        let action = |action_id: &str, based_on_room_seq: u64, payload: Value| ActionSubmit {
            room_id: room_id.clone(),
            member_id: member_id.clone(),
            action_id: action_id.to_owned(),
            based_on_room_seq,
            action_type: "increment".to_owned(),
            payload,
        };
        let member_session = session(0xb8, "01ARZ3NDEKTSV4RRFFQ69G5FC8");
        let first = match backend
            .action(
                &member_session,
                action("01ARZ3NDEKTSV4RRFFQ69G5FD0", 0, json!({})),
            )
            .unwrap_or_else(|error| panic!("authorized Action: {error:?}"))
        {
            ActionReply::Accepted(reply) => reply,
            ActionReply::Rejected(reply) => panic!("authorized Action rejected: {reply:?}"),
        };
        assert_eq!(first.room_head.room_seq, 1);
        assert!(!first.duplicate);

        let duplicate = match backend
            .action(
                &member_session,
                action("01ARZ3NDEKTSV4RRFFQ69G5FD0", 0, json!({})),
            )
            .unwrap_or_else(|error| panic!("duplicate Action: {error:?}"))
        {
            ActionReply::Accepted(reply) => reply,
            ActionReply::Rejected(reply) => panic!("duplicate Action rejected: {reply:?}"),
        };
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.transition_id, first.transition_id);
        assert!(matches!(
            backend.action(
                &member_session,
                action("01ARZ3NDEKTSV4RRFFQ69G5FD0", 0, json!({"changed": true})),
            ),
            Err(BackendError::Conflict)
        ));

        let stale = match backend
            .action(
                &member_session,
                action("01ARZ3NDEKTSV4RRFFQ69G5FD1", 0, json!({})),
            )
            .unwrap_or_else(|error| panic!("stale Action: {error:?}"))
        {
            ActionReply::Rejected(reply) => reply,
            ActionReply::Accepted(reply) => panic!("stale Action accepted: {reply:?}"),
        };
        assert_eq!(stale.code, "stale_room_state");
        assert_eq!(stale.current_room_seq, 1);
        assert!(!stale.duplicate);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn sqlite_gateway_action_denies_capability_without_room_act_scope() {
        let file = NamedTempFile::new().unwrap_or_else(|_| panic!("temp db"));
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        let principal = "01ARZ3NDEKTSV4RRFFQ69G5FC2"
            .parse::<worldstream_core::PrincipalId>()
            .unwrap_or_else(|_| panic!("principal"));
        let capability = CapabilityId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FC3")
            .unwrap_or_else(|_| panic!("capability"));
        authority
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FC4"
                        .parse()
                        .unwrap_or_else(|_| panic!("bootstrap change")),
                    principal.clone(),
                    PrincipalKindV1::Human,
                    capability,
                    bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|_| panic!("bootstrap")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|_| panic!("bootstrap time")),
            )
            .unwrap_or_else(|_| panic!("apply bootstrap"));
        let registry =
            Arc::new(builtin_counter_registry().unwrap_or_else(|_| panic!("counter registry")));
        let descriptor = registry
            .load_retained(&counter_v2_digest())
            .unwrap_or_else(|_| panic!("counter pack"))
            .descriptor()
            .clone();
        let backend = SqliteGatewayBackend::new(store.clone(), registry);
        let host_session = session(0xa9, "01ARZ3NDEKTSV4RRFFQ69G5FC5");
        let created = backend
            .create_room(
                &host_session,
                CreateRoomRequest {
                    pack: PackReference {
                        id: descriptor.pack_id,
                        version: descriptor.explanatory_version,
                        digest: counter_v2_digest().to_string(),
                    },
                    configuration: json!({"initial_value": 0, "maximum_value": 16}),
                    members: vec![CreateMember {
                        principal_id: principal.to_string(),
                        principal_kind: PrincipalKind::Human,
                        role: Some("counter".to_owned()),
                        access_mode: AccessMode::Participant,
                    }],
                    idempotency_key: "gateway-action-scope-create".to_owned(),
                },
            )
            .unwrap_or_else(|error| panic!("create Room: {error:?}"));
        let room_id = created
            .room_id
            .parse()
            .unwrap_or_else(|_| panic!("Room ID"));
        let member_id = created.member_ids[0]
            .parse()
            .unwrap_or_else(|_| panic!("Member ID"));
        let host_presented = PresentedCapabilityV1::new(
            CapabilityId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FC3")
                .unwrap_or_else(|_| panic!("host capability")),
            bearer,
        );
        let limited = NewCapabilityV1::new(
            "01ARZ3NDEKTSV4RRFFQ69G5FC6"
                .parse()
                .unwrap_or_else(|_| panic!("limited capability")),
            CapabilityBearerV1::from_bytes([0xc7; 32]).token_hash(),
            principal,
            CapabilityProfileV1::RoomMember { room_id, member_id },
            CapabilityScopeSetV1::new([CapabilityScopeV1::RoomAttach])
                .unwrap_or_else(|_| panic!("limited scopes")),
            None,
        )
        .unwrap_or_else(|_| panic!("limited capability"));
        authority
            .change(
                &host_presented,
                AuthorityChangeV1::RegisterCapability {
                    change_id: "01ARZ3NDEKTSV4RRFFQ69G5FC7"
                        .parse()
                        .unwrap_or_else(|_| panic!("limited capability change")),
                    capability: limited,
                },
                "2026-08-15T12:00:01Z"
                    .parse()
                    .unwrap_or_else(|_| panic!("limited capability time")),
            )
            .unwrap_or_else(|_| panic!("register limited capability"));
        assert!(matches!(
            backend.action(
                &session(0xc7, "01ARZ3NDEKTSV4RRFFQ69G5FC8"),
                ActionSubmit {
                    room_id: created.room_id,
                    member_id: created.member_ids[0].clone(),
                    action_id: "01ARZ3NDEKTSV4RRFFQ69G5FD0".to_owned(),
                    based_on_room_seq: 0,
                    action_type: "increment".to_owned(),
                    payload: json!({}),
                },
            ),
            Err(BackendError::Forbidden)
        ));
    }

    #[test]
    fn session_binding_retirement_removes_only_the_closed_session() {
        let bindings = SessionBindings::default();
        let first_session = "01ARZ3NDEKTSV4RRFFQ69G5FBD"
            .parse::<worldstream_protocol::UlidString>()
            .unwrap_or_else(|_| panic!("test session id"));
        let second_session = "01ARZ3NDEKTSV4RRFFQ69G5FBE"
            .parse::<worldstream_protocol::UlidString>()
            .unwrap_or_else(|_| panic!("test session id"));
        let first_capability = CapabilityId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FBF")
            .unwrap_or_else(|_| panic!("test capability id"));
        let second_capability = CapabilityId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FBG")
            .unwrap_or_else(|_| panic!("test capability id"));
        {
            let mut rows = bindings.0.lock().unwrap_or_else(|_| panic!("bindings"));
            rows.insert(
                first_session.clone(),
                SessionBinding {
                    capability_id: first_capability,
                    sync: None,
                },
            );
            rows.insert(
                second_session.clone(),
                SessionBinding {
                    capability_id: second_capability,
                    sync: None,
                },
            );
        }

        bindings.retire(&first_session);
        let rows = bindings.0.lock().unwrap_or_else(|_| panic!("bindings"));
        assert!(!rows.contains_key(&first_session));
        assert!(rows.contains_key(&second_session));
    }

    #[allow(clippy::too_many_lines)]
    #[test]
    fn supervisor_lifecycle_blocks_snapshot_work_until_current_generation_is_active() {
        let file = NamedTempFile::new().unwrap_or_else(|_| panic!("temp db"));
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        authority
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FC4"
                        .parse()
                        .unwrap_or_else(|_| panic!("bootstrap change")),
                    "01ARZ3NDEKTSV4RRFFQ69G5FC2"
                        .parse()
                        .unwrap_or_else(|_| panic!("bootstrap principal")),
                    PrincipalKindV1::Human,
                    "01ARZ3NDEKTSV4RRFFQ69G5FC3"
                        .parse()
                        .unwrap_or_else(|_| panic!("bootstrap capability")),
                    bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|_| panic!("bootstrap request")),
                "2026-08-15T12:00:00Z"
                    .parse()
                    .unwrap_or_else(|_| panic!("bootstrap time")),
            )
            .unwrap_or_else(|_| panic!("apply bootstrap"));
        let registry =
            Arc::new(builtin_counter_registry().unwrap_or_else(|_| panic!("counter registry")));
        let backend = SqliteGatewayBackend::new(store, registry);
        let authenticated_session = session(0xa9, "01ARZ3NDEKTSV4RRFFQ69G5FBE");
        let room_id =
            RoomId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FBD").unwrap_or_else(|_| panic!("room id"));

        assert!(matches!(
            backend.supervisor.require_active(&room_id),
            Err(SqliteRoomSupervisorErrorV1::NotActive)
        ));
        assert!(matches!(
            backend.supervised_room_snapshot(&room_id),
            Err(BackendError::NotFound)
        ));
        assert_eq!(
            backend.room_supervisor_state(room_id.as_str()),
            Some(SqliteRoomRuntimeStateV1::Inactive)
        );

        let first = backend
            .begin_room_reload(room_id.as_str())
            .unwrap_or_else(|error| panic!("begin Loading generation: {error}"));
        assert_eq!(
            backend.room_supervisor_state(room_id.as_str()),
            Some(SqliteRoomRuntimeStateV1::Loading)
        );
        assert!(matches!(
            backend.supervised_room_snapshot(&room_id),
            Err(BackendError::Busy)
        ));
        assert!(matches!(
            backend.fire_timer(
                &authenticated_session,
                room_id.as_str(),
                TimerFireRequest {
                    timer_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0".to_owned(),
                    generation: 1,
                },
            ),
            Err(BackendError::Busy)
        ));
        assert!(matches!(
            backend.attach(
                &authenticated_session,
                RoomAttach {
                    room_id: room_id.to_string(),
                    member_id: "01ARZ3NDEKTSV4RRFFQ69G5FBE".to_owned(),
                    after_frame_seq: None,
                },
            ),
            Err(BackendError::Busy)
        ));
        assert!(matches!(
            backend.action(
                &authenticated_session,
                ActionSubmit {
                    room_id: room_id.to_string(),
                    member_id: "01ARZ3NDEKTSV4RRFFQ69G5FBE".to_owned(),
                    action_id: "01ARZ3NDEKTSV4RRFFQ69G5FBF".to_owned(),
                    based_on_room_seq: 0,
                    action_type: "increment".to_owned(),
                    payload: json!({}),
                },
            ),
            Err(BackendError::Busy)
        ));
        backend
            .mark_room_catching_up(room_id.as_str(), first)
            .unwrap_or_else(|error| panic!("enter CatchingUp: {error}"));
        assert!(matches!(
            backend.supervised_room_snapshot(&room_id),
            Err(BackendError::Busy)
        ));
        assert!(matches!(
            backend.fire_timer(
                &authenticated_session,
                room_id.as_str(),
                TimerFireRequest {
                    timer_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0".to_owned(),
                    generation: 1,
                },
            ),
            Err(BackendError::Busy)
        ));
        backend
            .mark_room_active(room_id.as_str(), first)
            .unwrap_or_else(|error| panic!("enter Active: {error}"));
        assert!(matches!(
            backend.supervised_room_snapshot(&room_id),
            Ok(None)
        ));

        let operation = backend
            .acquire_room_operation(&room_id)
            .unwrap_or_else(|error| panic!("admit in-flight Room operation: {error}"));
        let barrier = backend
            .begin_room_passivation(room_id.as_str(), first)
            .unwrap_or_else(|error| panic!("enter Passivating: {error}"));
        assert!(matches!(
            backend.supervised_room_snapshot(&room_id),
            Err(BackendError::Busy)
        ));
        assert!(matches!(
            backend.fire_timer(
                &authenticated_session,
                room_id.as_str(),
                TimerFireRequest {
                    timer_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0".to_owned(),
                    generation: 1,
                },
            ),
            Err(BackendError::Busy)
        ));
        assert!(matches!(
            backend.begin_room_reload(room_id.as_str()),
            Err(BackendError::Busy)
        ));
        assert!(matches!(
            backend.complete_room_passivation(room_id.as_str(), barrier),
            Err(BackendError::Busy)
        ));
        assert!(matches!(operation.publish(), Ok(())));
        drop(operation);
        backend
            .complete_room_passivation(room_id.as_str(), barrier)
            .unwrap_or_else(|error| panic!("enter Inactive: {error}"));
        assert_eq!(
            backend.room_supervisor_state(room_id.as_str()),
            Some(SqliteRoomRuntimeStateV1::Inactive)
        );
        assert!(matches!(
            backend.supervised_room_snapshot(&room_id),
            Err(BackendError::NotFound)
        ));

        let second = backend
            .begin_room_reload(room_id.as_str())
            .unwrap_or_else(|error| panic!("begin replacement generation: {error}"));
        assert!(second.generation() > first.generation());
        assert!(matches!(
            backend.complete_room_passivation(room_id.as_str(), barrier),
            Err(BackendError::Busy)
        ));
        backend
            .mark_room_catching_up(room_id.as_str(), second)
            .unwrap_or_else(|error| panic!("replacement CatchingUp: {error}"));
        backend
            .mark_room_active(room_id.as_str(), second)
            .unwrap_or_else(|error| panic!("replacement Active: {error}"));
        assert!(matches!(
            backend.supervised_room_snapshot(&room_id),
            Ok(None)
        ));
    }

    #[test]
    fn unknown_bearer_is_forbidden_without_secret_echo() {
        let file = NamedTempFile::new().unwrap_or_else(|_| panic!("temp db"));
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let registry = worldstream_core::builtin_counter_registry()
            .unwrap_or_else(|_| panic!("counter registry"));
        let backend = SqliteGatewayBackend::new(store, Arc::new(registry));
        let hello = ClientHello {
            client_name: "test".to_owned(),
            client_version: "test".to_owned(),
            mode: worldstream_protocol::ClientMode::Participant,
            supported_protocols: vec![PROTOCOL_VERSION.to_owned()],
            capabilities: vec!["cursor_ack".to_owned(), "projection_reset".to_owned()],
        };
        assert!(matches!(
            backend.hello(&session(0x7a, "01ARZ3NDEKTSV4RRFFQ69G5FBD"), &hello),
            Err(BackendError::Forbidden)
        ));
        assert!(!format!("{backend:?}").contains("7a7a"));
    }

    #[test]
    fn unauthenticated_session_operations_are_forbidden() {
        let file = NamedTempFile::new().unwrap_or_else(|_| panic!("temp db"));
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let registry = worldstream_core::builtin_counter_registry()
            .unwrap_or_else(|_| panic!("counter registry"));
        let backend = SqliteGatewayBackend::new(store, Arc::new(registry));
        let gateway_session = session(0x7a, "01ARZ3NDEKTSV4RRFFQ69G5FBD");
        assert!(matches!(
            backend.attach(
                &gateway_session,
                RoomAttach {
                    room_id: "01ARZ3NDEKTSV4RRFFQ69G5FBD".to_owned(),
                    member_id: "01ARZ3NDEKTSV4RRFFQ69G5FBE".to_owned(),
                    after_frame_seq: None,
                }
            ),
            Err(BackendError::Forbidden)
        ));
        assert!(matches!(
            backend.sync_ack(
                &gateway_session,
                RoomSyncAck {
                    room_id: "01ARZ3NDEKTSV4RRFFQ69G5FBD".to_owned(),
                    member_id: "01ARZ3NDEKTSV4RRFFQ69G5FBE".to_owned(),
                    through_frame_head: 0,
                    sync_token: "opaque".to_owned(),
                }
            ),
            Err(BackendError::Forbidden)
        ));
    }

    #[test]
    fn sqlite_error_mapping_is_closed() {
        assert!(matches!(
            map_gateway_error(&SqliteGatewayErrorV1::Unauthenticated),
            BackendError::Forbidden
        ));
        assert!(matches!(
            map_gateway_error(&SqliteGatewayErrorV1::Corrupt),
            BackendError::StorageUnavailable
        ));
        assert!(matches!(
            map_observation_error(SqliteObservationErrorV1::CursorAhead),
            BackendError::Rejected
        ));
        assert!(matches!(
            map_gateway_error(&SqliteGatewayErrorV1::IntegrityUnavailable),
            BackendError::RoomQuarantined
        ));
        assert!(matches!(
            map_observation_error(SqliteObservationErrorV1::RoomQuarantined),
            BackendError::RoomQuarantined
        ));
        assert!(matches!(
            map_participant_action_error(&SqliteParticipantActionErrorV1::RoomFaulted),
            BackendError::RoomFaulted
        ));
    }

    #[test]
    fn observation_payload_moves_action_offers_into_protocol_observation() {
        let (schema, observation) = observation_from_payload(
            br#"{"action_offers":[{"action_type":"commit_move"}],"observation":{"reason":"opened"},"observation_schema":"fixture/observation/v1"}"#,
        )
        .unwrap_or_else(|_| panic!("observation payload"));
        assert_eq!(schema, "fixture/observation/v1");
        assert_eq!(
            observation
                .get("action_offers")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            observation.get("reason").and_then(Value::as_str),
            Some("opened")
        );
    }

    #[test]
    fn observation_payload_rejects_offers_for_non_object_observation() {
        assert!(matches!(
            observation_from_payload(
                br#"{"action_offers":[],"observation":[],"observation_schema":"fixture/observation/v1"}"#,
            ),
            Err(BackendError::InvalidResult)
        ));
    }
}
