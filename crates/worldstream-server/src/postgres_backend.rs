//! `PostgreSQL` runtime boundary for the operator process.
//!
//! `PostgresRoomStore` owns the provider's verified schema, durable authority,
//! native observation, Activation, and Room-commit primitives. This module
//! binds the production authority handshake and lease scheduler into the
//! server's complete `GatewayBackend` surface without routing production work
//! through conformance-only provider APIs.

#![cfg_attr(test, allow(clippy::panic, clippy::unwrap_used))]

use std::{
    collections::BTreeMap,
    fmt,
    str::FromStr,
    sync::{Arc, Mutex},
};

use anyhow::{Result, bail};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use worldstream_core::{
    AccessModeV1, ActivationIntentStateV1, ActivationOperationRequestV1, ActivationResultCodeV1,
    AdministrationOperationIdentityV1, AdmissionLaneErrorV1, AuthorityChangeId, AuthorityChangeV1,
    AuthorityCheckedAt, AuthorityErrorV1, AuthorityStoreV1, AuthorityV1,
    AuthorizedReceiptResolverV1, AuthorizedRunnerControlV1, CORE_OPERATION_KIND,
    CREATE_ROOM_OPERATION_KIND, CanonicalJsonV1, CanonicalRoomTraceCache, CapabilityBearerV1,
    CapabilityExpiresAt, CapabilityId, CapabilityProfileV1, CapabilityScopeSetV1,
    CoreAdministrationIngressV1, CoreAdministrationRequestV1, CoreChangeSetV1, CoreProposedKindV1,
    CoreRecordedAt, CreationRecordedAt, DiagnosticOperationV1, DiagnosticTargetV1,
    ExternalInputRecordedAt, HistoricalReplayErrorV1, HostClockErrorV1, HostClockSampleV1,
    HostClockV1, InitialMembershipProposalV1, MemberReadOperationV1, MembershipStandingV1,
    MembershipV1, MonotonicHostClockV1, NewCapabilityV1, PackDigestV1, PackGenesisRequestV1,
    PackRegistryV1, PackViewerV1, ParticipantActionIngressErrorV1, ParticipantActionIngressV1,
    ParticipantActionRequestV1, PrincipalKindV1, ReplayProjectionKindV1, RoomAdmissionLanesV1,
    RoomCommitResolutionV1, RoomCommitStorageV1, RoomCreationIngressV1, RoomCreationRequestV1,
    RoomId, RoomIntegrityStateV1, RoomIntegrityStatusV1, RoomMembershipKeyV1, RoomSeedV1,
    RoomSequenceV1, RoomTraceCacheErrorV1, RunnerControlOperationV1, RunnerId,
    RunnerMembershipSetV1, SemanticResultV1, SessionErrorV1, SessionFrameV1, SessionSyncTokenV1,
    SessionV1, StoredSemanticResultV1, TimerFiredRequestV1, TimerGenerationV1, TimerId,
    TransitionId, authorize_core_administration_operation, authorize_participant_action_operation,
};
use worldstream_postgres::{
    PostgresActivationError, PostgresAuthorityAuthenticationError,
    PostgresExternalInputPreparationErrorV1, PostgresFrameEvidenceV1,
    PostgresHistoricalEvidenceErrorV1, PostgresObservationDeliveryV1, PostgresObservationError,
    PostgresObservationRetentionV1, PostgresRoomCommitError, PostgresRoomDiagnosticErrorV1,
    PostgresRoomDiagnosticSummaryV1, PostgresRoomServingFenceV1, PostgresRoomStore,
    PostgresSchemaVerificationError, PostgresTimerStateV1,
};
use worldstream_protocol::{
    AccessMode, ActionAccepted, ActionRejected, ActionSubmit, ActivationClaim, ActivationDelivery,
    ActivationFrame, ActivationIntentState, ActivationLeaseOperation, ActivationOffer,
    ActivationOfferRequest, ActivationOffers, ActivationOperationReply, ActivationResultCode,
    BearerWireV1, ClientHello, CreateRoomRequest, CreateRoomResponse,
    HISTORICAL_EVIDENCE_RESPONSE_VERSION, HistoricalEvidenceOutcomeV1,
    HistoricalEvidenceReferenceV1, LobbyLaunchRequest, LobbyLaunchResponse, MAX_MESSAGE_BYTES,
    MemberCapabilityProvisionRequestV1, MemberCapabilityProvisionResponseV1,
    OPERATOR_ACTIVATION_STATUS_VERSION, ObservationAck, ObservationDeliver,
    OperatorActivationStatusV1, OperatorActivityPhase, OperatorBackupProfileStatus,
    OperatorBackupStorageHealth, OperatorBackupStorageProfile, OperatorBackupVerification,
    OperatorDataFreshness, OperatorLiveBackupPrepareRequest, OperatorLiveBackupStatus,
    OperatorRoomIntegrity, OperatorRoomIntegrityStatus, OperatorRoomInventoryPage,
    OperatorRoomInventoryRequest, OperatorRoomSummary, PROTOCOL_VERSION, PackReference, Principal,
    PrincipalKind, Projection, ProjectionReset, ProjectionResponse,
    ROOM_ARCHIVE_RESPONSE_SCHEMA_V1, ReplayResponse, RoomArchiveRequestV1, RoomArchiveResponseV1,
    RoomAttach, RoomAttached, RoomHead, RoomSyncAck, RunnerCapabilityProvisionRequestV1,
    RunnerCapabilityProvisionResponseV1, RunnerHello, RunnerReady, ServerWelcome, SyncBranch,
    TimerFireRequest, TimerFireResponse,
};
use worldstream_runtime::SecretSource;

use crate::{
    ActionReply, AttachReply, BackendError, GatewayBackend, GatewaySession,
    MemberCapabilityIssueRequest, MemberCapabilityIssueResponse, RunnerCapabilityIssueRequest,
    RunnerCapabilityIssueResponse, RunnerMembershipTarget,
    activity_start::{
        lobby_response_from_resolution, lobby_response_from_result, prepare_activity_start_request,
    },
    external_input::{ingress_response_from_result, prepare_external_input_ingress},
    fill_random_bytes,
};

const MAX_DSN_BYTES: usize = 16 * 1024;

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

type ProtocolDelivery = (
    SyncBranch,
    Option<ProjectionReset>,
    Vec<ObservationDeliver>,
    u64,
    u64,
    Option<u64>,
    u64,
);

/// Reads a `PostgreSQL` DSN from the runtime's owner-only secret-file form.
///
/// The caller receives only bounded, pathless diagnostics. Both owner-only
/// files and duplicated inherited descriptors/handles use the same bounded
/// secret-source read path.
///
/// # Errors
///
/// Returns a redacted error when the source cannot be read within the bound
/// or does not contain one canonical single-line UTF-8 DSN.
pub fn read_postgres_dsn(source: &SecretSource) -> Result<String> {
    let bytes = source
        .read_bounded(MAX_DSN_BYTES)
        .map_err(|_| anyhow::anyhow!("PostgreSQL DSN secret-source read failed"))?;
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow::anyhow!("PostgreSQL DSN secret material is not UTF-8"))?
        .trim();
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte == b'\0' || byte == b'\r' || byte == b'\n')
    {
        bail!("PostgreSQL DSN secret material contains invalid control data");
    }
    Ok(value.to_owned())
}

/// Complete production `PostgreSQL` gateway adapter backed by Core admission
/// and the provider's verified authority, observation, Activation, and commit
/// seams.
pub struct PostgresGatewayBackend {
    store: Arc<PostgresRoomStore>,
    registry: Arc<PackRegistryV1>,
    bindings: Arc<SessionBindings>,
    #[cfg(test)]
    live_payload_page_reads: std::sync::atomic::AtomicUsize,
    traces: CanonicalRoomTraceCache,
    observation_retention: Mutex<PostgresObservationRetentionV1>,
    host_clock: Arc<dyn HostClockV1>,
    admission_lanes: RoomAdmissionLanesV1,
    #[cfg(test)]
    recovery_forbidden: std::sync::atomic::AtomicBool,
}

struct RuntimeWallClock;

impl HostClockV1 for RuntimeWallClock {
    fn sample(&self) -> Result<HostClockSampleV1, HostClockErrorV1> {
        let value = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| HostClockErrorV1::Unavailable)?;
        HostClockSampleV1::new(value).map_err(|_| HostClockErrorV1::Unavailable)
    }
}

impl fmt::Debug for PostgresGatewayBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PostgresGatewayBackend")
            .field("store", &"[OPAQUE]")
            .field("registry", &"[OPAQUE]")
            .field("bindings", &self.bindings)
            .field("traces", &"[OPAQUE]")
            .field("observation_retention", &"[OPAQUE]")
            .field("host_clock", &"[OPAQUE]")
            .field("admission_lanes", &self.admission_lanes)
            .finish()
    }
}

impl PostgresGatewayBackend {
    /// Retains the already-configured least-privilege runtime store and the
    /// process-wide startup registry. No profile-local fallback is constructed.
    #[must_use]
    pub fn new(store: PostgresRoomStore, registry: Arc<PackRegistryV1>) -> Self {
        Self::with_host_clock(
            store,
            registry,
            Arc::new(MonotonicHostClockV1::new(RuntimeWallClock)),
        )
    }

    /// Wires the gateway to one application-owned semantic `HostClock`.
    ///
    /// Callers providing a raw wall source should wrap it in
    /// [`MonotonicHostClockV1`].
    #[must_use]
    pub fn with_host_clock(
        store: PostgresRoomStore,
        registry: Arc<PackRegistryV1>,
        host_clock: Arc<dyn HostClockV1>,
    ) -> Self {
        Self::with_runtime(store, registry, host_clock, RoomAdmissionLanesV1::default())
    }

    #[cfg(test)]
    pub(crate) fn with_test_admission_lanes(
        store: PostgresRoomStore,
        registry: Arc<PackRegistryV1>,
        admission_lanes: RoomAdmissionLanesV1,
    ) -> Self {
        let mut backend = Self::new(store, registry);
        backend.admission_lanes = admission_lanes;
        backend
    }

    #[cfg(test)]
    pub(crate) fn test_admission_lanes(&self) -> &RoomAdmissionLanesV1 {
        &self.admission_lanes
    }

    fn with_runtime(
        store: PostgresRoomStore,
        registry: Arc<PackRegistryV1>,
        host_clock: Arc<dyn HostClockV1>,
        admission_lanes: RoomAdmissionLanesV1,
    ) -> Self {
        Self {
            store: Arc::new(store),
            registry,
            bindings: Arc::new(SessionBindings::default()),
            #[cfg(test)]
            live_payload_page_reads: std::sync::atomic::AtomicUsize::new(0),
            traces: CanonicalRoomTraceCache::default(),
            observation_retention: Mutex::new(PostgresObservationRetentionV1::default()),
            host_clock,
            admission_lanes,
            #[cfg(test)]
            recovery_forbidden: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Performs the read-only startup schema/capability check through the
    /// same store that will be retained by the gateway.
    ///
    /// # Errors
    ///
    /// Returns the provider's closed verification error when schema,
    /// capability, or migration admission fails.
    pub fn verify_schema(&self) -> Result<(), PostgresSchemaVerificationError> {
        self.store.verify_schema()
    }

    fn authority(&self) -> AuthorityV1 {
        AuthorityV1::new(self.store.clone())
    }

    fn checked_at(&self) -> Result<AuthorityCheckedAt, BackendError> {
        self.store
            .authority_checked_at()
            .map_err(|_| BackendError::StorageUnavailable)
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
        self.with_serving_trace(&room_id, |trace, _| {
            let membership = trace
                .core_state()
                .membership(&member_id)
                .ok_or(BackendError::Forbidden)?;
            if trace.core_state().room_status() != worldstream_core::RoomStatusV1::Active
                || membership.standing() != MembershipStandingV1::Enabled
                || membership.access_mode() != AccessModeV1::Participant
                || membership.principal_kind() != PrincipalKindV1::Agent
                || membership.role().is_none()
            {
                return Err(BackendError::Forbidden);
            }
            Ok(())
        })?;
        self.authority()
            .authorize_runner_control(
                &authenticated.into_presented(),
                runner_id,
                operation,
                RoomMembershipKeyV1 { room_id, member_id },
                self.checked_at()?,
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

    fn activation_lease_operation(
        &self,
        session: &GatewaySession,
        request: ActivationLeaseOperation,
        operation: RunnerControlOperationV1,
        operation_kind: &str,
    ) -> Result<ActivationOperationReply, BackendError> {
        // Authenticate before private target lookup.
        let _ = self.authenticate(session)?;
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
        self.store
            .operate_activation_lease_authorized(authority, core_request)
            .map_err(map_activation_error)
            .and_then(Self::activation_reply)
    }

    #[allow(clippy::too_many_lines)]
    fn issue_member_capability_inner(
        &self,
        session: &GatewaySession,
        request: MemberCapabilityIssueRequest,
    ) -> Result<MemberCapabilityIssueResponse, BackendError> {
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
        let (trace, _) = self.verified_trace(&room_id)?;
        let membership = trace
            .core_state()
            .membership(&member_id)
            .filter(|membership| membership.standing() == MembershipStandingV1::Enabled)
            .ok_or(BackendError::Forbidden)?;
        if membership.principal_id() != &principal_id {
            return Err(BackendError::Forbidden);
        }
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
            self.checked_at()?,
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
            Err(AuthorityErrorV1::Conflict | AuthorityErrorV1::InvalidAuthorityRequest) => {}
            Ok(_) => return Err(BackendError::InvalidResult),
            Err(error) => return Err(map_authority_error(error)),
        }
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
        let receipt = authority
            .change(
                &presented,
                AuthorityChangeV1::RegisterCapability {
                    change_id,
                    capability,
                },
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
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
            let (trace, _) = self.verified_trace(&target.room_id)?;
            let membership = trace
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
                self.checked_at()?,
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
                self.checked_at()?,
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
                self.checked_at()?,
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

    #[allow(clippy::too_many_lines)]
    fn provision_member_capability_inner(
        &self,
        session: &GatewaySession,
        request: MemberCapabilityProvisionRequestV1,
    ) -> Result<MemberCapabilityProvisionResponseV1, BackendError> {
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
        let change_id = AuthorityChangeId::from_str(&request.capability.capability_idempotency_key)
            .map_err(|_| BackendError::Rejected)?;
        let capability_id = request
            .capability
            .capability_id
            .parse::<CapabilityId>()
            .map_err(|_| BackendError::Rejected)?;
        let scopes = crate::provisioned_scopes(request.scopes)?;
        let expires_at = request
            .expires_at
            .as_deref()
            .map(CapabilityExpiresAt::from_str)
            .transpose()
            .map_err(|_| BackendError::Rejected)?;
        let bearer = request
            .capability
            .bearer
            .wire()
            .map_err(|_| BackendError::Rejected)?;
        if !crate::member_capability_access_role_valid(request.access_mode, request.role.as_deref())
        {
            return Err(BackendError::Rejected);
        }
        let (trace, _) = self.verified_trace(&room_id)?;
        let membership = trace
            .core_state()
            .membership(&member_id)
            .filter(|membership| membership.standing() == MembershipStandingV1::Enabled)
            .ok_or(BackendError::Forbidden)?;
        if membership.principal_id() != &principal_id
            || membership.principal_kind() != core_principal_kind(request.principal_kind)
            || membership.access_mode() != core_access_mode(request.access_mode)
            || membership.role() != request.role.as_deref()
        {
            return Err(BackendError::Forbidden);
        }
        let principal_change_id = AuthorityChangeId::from_str(principal_id.as_str())
            .map_err(|_| BackendError::Rejected)?;
        if principal_change_id == change_id {
            return Err(BackendError::Rejected);
        }
        let presented = authenticated.into_presented();
        let authority = self.authority();
        match authority.change(
            &presented,
            AuthorityChangeV1::CreatePrincipal {
                change_id: principal_change_id,
                principal_id: principal_id.clone(),
                kind: membership.principal_kind(),
            },
            self.checked_at()?,
        ) {
            Ok(receipt)
                if receipt.result()
                    == worldstream_core::AuthorityChangeResultV1::PrincipalCreated
                    && receipt.resulting_generation() == 1 => {}
            Err(AuthorityErrorV1::Conflict | AuthorityErrorV1::InvalidAuthorityRequest) => {}
            Ok(_) => return Err(BackendError::InvalidResult),
            Err(error) => return Err(map_authority_error(error)),
        }
        let capability_bearer = CapabilityBearerV1::from_bytes(bearer.into_bytes());
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
        let receipt = authority
            .change(
                &presented,
                AuthorityChangeV1::RegisterCapability {
                    change_id,
                    capability,
                },
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
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
        Ok(MemberCapabilityProvisionResponseV1 {
            capability_id: capability_id.to_string(),
            room_id: room_id.to_string(),
            member_id: member_id.to_string(),
            principal_id: principal_id.to_string(),
            scopes: crate::scope_names(&scopes),
        })
    }

    #[allow(clippy::too_many_lines)]
    fn provision_runner_capability_inner(
        &self,
        session: &GatewaySession,
        request: RunnerCapabilityProvisionRequestV1,
    ) -> Result<RunnerCapabilityProvisionResponseV1, BackendError> {
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
        let capability_change_id =
            AuthorityChangeId::from_str(&request.capability.capability_idempotency_key)
                .map_err(|_| BackendError::Rejected)?;
        let capability_id = request
            .capability
            .capability_id
            .parse::<CapabilityId>()
            .map_err(|_| BackendError::Rejected)?;
        if principal_change_id == runner_change_id
            || principal_change_id == capability_change_id
            || runner_change_id == capability_change_id
        {
            return Err(BackendError::Rejected);
        }
        let scopes = crate::provisioned_scopes(request.scopes)?;
        let expires_at = request
            .expires_at
            .as_deref()
            .map(CapabilityExpiresAt::from_str)
            .transpose()
            .map_err(|_| BackendError::Rejected)?;
        let bearer = request
            .capability
            .bearer
            .wire()
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
            let (trace, _) = self.verified_trace(&target.room_id)?;
            let membership = trace
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
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
        if principal_receipt.result() != worldstream_core::AuthorityChangeResultV1::PrincipalCreated
            || principal_receipt.resulting_generation() != 1
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
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
        if runner_receipt.result() != worldstream_core::AuthorityChangeResultV1::RunnerRegistered
            || runner_receipt.resulting_generation() != 1
        {
            return Err(BackendError::InvalidResult);
        }
        let capability_bearer = CapabilityBearerV1::from_bytes(bearer.into_bytes());
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
                self.checked_at()?,
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
        Ok(RunnerCapabilityProvisionResponseV1 {
            capability_id: capability_id.to_string(),
            runner_id: runner_id.to_string(),
            owner_principal_id: owner_principal_id.to_string(),
            permitted_memberships: permitted_memberships
                .iter()
                .map(
                    |target| worldstream_protocol::RunnerMembershipProvisionTargetV1 {
                        room_id: target.room_id.to_string(),
                        member_id: target.member_id.to_string(),
                    },
                )
                .collect(),
            scopes: crate::scope_names(&scopes),
        })
    }

    fn verified_trace(
        &self,
        room_id: &RoomId,
    ) -> Result<
        (
            worldstream_core::CanonicalRoomTrace,
            worldstream_postgres::PostgresRoomVerification,
        ),
        BackendError,
    > {
        #[cfg(test)]
        if self
            .recovery_forbidden
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(BackendError::StorageUnavailable);
        }
        let verification =
            self.store
                .verify_room(room_id.as_ref())
                .map_err(|error| match error {
                    worldstream_postgres::PostgresRoomVerificationError::MissingRoom { .. } => {
                        BackendError::NotFound
                    }
                    _ => BackendError::StorageUnavailable,
                })?;
        match verification.integrity_status.as_str() {
            "healthy" => {}
            "faulted" => return Err(BackendError::RoomFaulted),
            "quarantined" => return Err(BackendError::RoomQuarantined),
            _ => return Err(BackendError::InvalidResult),
        }
        let trace = self
            .store
            .recover_canonical_room(self.registry.as_ref(), room_id.as_ref())
            .map_err(map_recovery_error)?
            .ok_or(BackendError::NotFound)?;
        Ok((trace, verification))
    }

    #[cfg(test)]
    fn forbid_recovery_for_test(&self) {
        self.recovery_forbidden
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Borrows one current executor only after comparing its complete Head
    /// and integrity state with a lightweight durable fence. A mismatch
    /// discards the executor and performs guarded recovery; no hot operation
    /// reads or replays historical transitions.
    fn with_serving_trace<R>(
        &self,
        room_id: &RoomId,
        operation: impl FnOnce(
            &mut worldstream_core::CanonicalRoomTrace,
            &PostgresRoomServingFenceV1,
        ) -> Result<R, BackendError>,
    ) -> Result<R, BackendError> {
        self.traces
            .with_room(room_id, |slot| {
                let mut fence = self
                    .store
                    .current_room_serving_fence(room_id)
                    .map_err(map_serving_fence_error)?
                    .ok_or(BackendError::NotFound)?;
                match fence.integrity().status() {
                    RoomIntegrityStatusV1::Healthy => {}
                    RoomIntegrityStatusV1::Faulted => return Err(BackendError::RoomFaulted),
                    RoomIntegrityStatusV1::Quarantined => {
                        return Err(BackendError::RoomQuarantined);
                    }
                }
                let cache_matches = slot.as_ref().is_some_and(|cached| {
                    cached.trace().head() == fence.head() && cached.integrity() == fence.integrity()
                });
                if !cache_matches {
                    *slot = None;
                    let (trace, verification) = self.verified_trace(room_id)?;
                    let current = self
                        .store
                        .current_room_serving_fence(room_id)
                        .map_err(map_serving_fence_error)?
                        .ok_or(BackendError::NotFound)?;
                    if trace.head() != current.head()
                        || verification.integrity_generation
                            != current.integrity().generation().get()
                        || verification.integrity_status
                            != serving_integrity_status(current.integrity())
                    {
                        return Err(BackendError::Busy);
                    }
                    *slot = Some(worldstream_core::CachedCanonicalRoomTrace::new(
                        trace,
                        current.integrity().clone(),
                    ));
                    fence = current;
                }
                let cached = slot.as_mut().ok_or(BackendError::StorageUnavailable)?;
                let result = operation(cached.trace_mut(), &fence);
                if result.is_err() {
                    *slot = None;
                }
                result
            })
            .map_err(map_trace_cache_error)?
    }

    fn commit_cached_plan(
        &self,
        room_id: &RoomId,
        operation: impl FnOnce(
            &worldstream_core::CanonicalRoomTrace,
            &PostgresRoomServingFenceV1,
        )
            -> Result<worldstream_core::PreparedCanonicalRoomCommit, BackendError>,
    ) -> Result<RoomCommitResolutionV1, BackendError> {
        self.traces
            .with_room(room_id, |slot| {
                let mut fence = self
                    .store
                    .current_room_serving_fence(room_id)
                    .map_err(map_serving_fence_error)?
                    .ok_or(BackendError::NotFound)?;
                match fence.integrity().status() {
                    RoomIntegrityStatusV1::Healthy => {}
                    RoomIntegrityStatusV1::Faulted => return Err(BackendError::RoomFaulted),
                    RoomIntegrityStatusV1::Quarantined => {
                        return Err(BackendError::RoomQuarantined);
                    }
                }
                let cache_matches = slot.as_ref().is_some_and(|cached| {
                    cached.trace().head() == fence.head() && cached.integrity() == fence.integrity()
                });
                if !cache_matches {
                    *slot = None;
                    let (trace, verification) = self.verified_trace(room_id)?;
                    let current = self
                        .store
                        .current_room_serving_fence(room_id)
                        .map_err(map_serving_fence_error)?
                        .ok_or(BackendError::NotFound)?;
                    if trace.head() != current.head()
                        || verification.integrity_generation
                            != current.integrity().generation().get()
                        || verification.integrity_status
                            != serving_integrity_status(current.integrity())
                    {
                        return Err(BackendError::Busy);
                    }
                    *slot = Some(worldstream_core::CachedCanonicalRoomTrace::new(
                        trace,
                        current.integrity().clone(),
                    ));
                    fence = current;
                }
                let cached = slot.as_mut().ok_or(BackendError::StorageUnavailable)?;
                let result = operation(cached.trace(), &fence).map(|prepared| {
                    worldstream_core::commit_canonical_existing_room(
                        self.store.as_ref(),
                        cached.trace_mut(),
                        prepared,
                    )
                    .into_parts()
                    .0
                });
                cached.trace_mut().discard_persisted_history();
                if result.is_err()
                    || matches!(
                        &result,
                        Ok(RoomCommitResolutionV1::Reprepare
                            | RoomCommitResolutionV1::RetryableKnownAbsent
                            | RoomCommitResolutionV1::Fenced
                            | RoomCommitResolutionV1::Indeterminate
                            | RoomCommitResolutionV1::Fault)
                    )
                {
                    *slot = None;
                }
                result
            })
            .map_err(map_trace_cache_error)?
    }

    fn commit_cached_timer(
        &self,
        room: &RoomId,
        authority: worldstream_core::AuthorizedTimerFiredV1,
        request: &TimerFiredRequestV1,
        transition: TransitionId,
    ) -> Result<RoomCommitResolutionV1, BackendError> {
        self.commit_cached_plan(room, |trace, fence| {
            let prepared = trace
                .prepare(worldstream_core::RecordedStimulusV1::TimerFired(
                    worldstream_core::TimerFiredV1 {
                        timer_id: request.timer_id().clone(),
                        generation: request.generation(),
                        scheduled_for: request.scheduled_for().clone(),
                        canonical_payload: request.canonical_payload().clone(),
                    },
                ))
                .map_err(crate::map_canonical_trace_preparation_error)?;
            worldstream_core::PreparedCanonicalRoomCommit::for_authorized_timer_fired(
                trace,
                request,
                prepared,
                transition,
                fence.integrity().generation(),
                authority,
                fence.frame_heads(),
            )
            .map_err(crate::map_canonical_write_preparation_error)
        })
    }

    fn commit_cached_external_input(
        &self,
        room: &RoomId,
        authority: worldstream_core::AuthorizedExternalInputV1,
        based_on: RoomSequenceV1,
        input: &worldstream_core::ExternalInputV1,
        transition: TransitionId,
    ) -> Result<RoomCommitResolutionV1, BackendError> {
        self.commit_cached_plan(room, |trace, fence| {
            let prepared = trace
                .prepare(worldstream_core::RecordedStimulusV1::ExternalInput(
                    input.clone(),
                ))
                .map_err(crate::map_canonical_trace_preparation_error)?;
            worldstream_core::PreparedCanonicalRoomCommit::for_authorized_external_input(
                trace,
                room,
                based_on,
                input,
                prepared,
                transition,
                fence.integrity().generation(),
                authority,
                fence.frame_heads(),
            )
            .map_err(|error| match error {
                worldstream_core::PrepareRoomWriteErrorV1::PreparedBasisMismatch => {
                    BackendError::Rejected
                }
                other => crate::map_canonical_write_preparation_error(other),
            })
        })
    }

    fn commit_cached_core_administration(
        &self,
        room: &RoomId,
        authority: worldstream_core::AuthorizedCoreAdministrationV1,
        request: &CoreAdministrationRequestV1,
        recorded_at: CoreRecordedAt,
        transition: TransitionId,
    ) -> Result<RoomCommitResolutionV1, BackendError> {
        self.commit_cached_plan(room, |trace, fence| {
            worldstream_core::PreparedCanonicalRoomCommit::for_authorized_core_administration(
                trace,
                request,
                recorded_at,
                transition,
                fence.integrity().generation(),
                authority,
                fence.frame_heads(),
            )
            .map_err(crate::map_canonical_write_preparation_error)
        })
    }

    fn commit_cached_participant_action(
        &self,
        room_id: &RoomId,
        authority: worldstream_core::ParticipantActionAuthorityV1,
        request: &ParticipantActionRequestV1,
        action: &ActionSubmit,
        admitted_at: worldstream_core::ActionAdmittedAt,
        transition_id: TransitionId,
    ) -> Result<RoomCommitResolutionV1, BackendError> {
        self.traces
            .with_room(room_id, |slot| {
                let mut fence = self
                    .store
                    .current_room_serving_fence(room_id)
                    .map_err(map_serving_fence_error)?
                    .ok_or(BackendError::NotFound)?;
                match fence.integrity().status() {
                    RoomIntegrityStatusV1::Healthy => {}
                    RoomIntegrityStatusV1::Faulted => return Err(BackendError::RoomFaulted),
                    RoomIntegrityStatusV1::Quarantined => {
                        return Err(BackendError::RoomQuarantined);
                    }
                }
                let cache_matches = slot.as_ref().is_some_and(|cached| {
                    cached.trace().head() == fence.head() && cached.integrity() == fence.integrity()
                });
                if !cache_matches {
                    *slot = None;
                    let (trace, verification) = self.verified_trace(room_id)?;
                    let current = self
                        .store
                        .current_room_serving_fence(room_id)
                        .map_err(map_serving_fence_error)?
                        .ok_or(BackendError::NotFound)?;
                    if trace.head() != current.head()
                        || verification.integrity_generation
                            != current.integrity().generation().get()
                        || verification.integrity_status
                            != serving_integrity_status(current.integrity())
                    {
                        return Err(BackendError::Busy);
                    }
                    *slot = Some(worldstream_core::CachedCanonicalRoomTrace::new(
                        trace,
                        current.integrity().clone(),
                    ));
                    fence = current;
                }
                let cached = slot.as_mut().ok_or(BackendError::StorageUnavailable)?;
                let member_id = action
                    .member_id
                    .parse()
                    .map_err(|_| BackendError::Rejected)?;
                let membership = cached
                    .trace()
                    .core_state()
                    .membership(&member_id)
                    .ok_or(BackendError::Forbidden)?;
                let payload_schema_digest = self
                    .view_for(cached.trace(), membership)?
                    .action_offers()
                    .offers()
                    .iter()
                    .find(|offer| offer.action_type == action.action_type)
                    .map(|offer| offer.payload_schema_digest.clone())
                    .ok_or(BackendError::Rejected)?;
                let stimulus = worldstream_core::ParticipantActionV1 {
                    member_id,
                    action_id: action
                        .action_id
                        .parse()
                        .map_err(|_| BackendError::Rejected)?,
                    action_type: action.action_type.clone(),
                    payload_schema_digest,
                    canonical_payload: canonical_json(&action.payload)?,
                    exact_basis_head: cached.trace().head().clone(),
                    admitted_at,
                };
                let integrity = cached.integrity().clone();
                let resolution = self
                    .store
                    .commit_authorized_participant_action_from_canonical_serving_trace(
                        authority,
                        request,
                        stimulus,
                        transition_id,
                        cached.trace_mut(),
                        integrity.generation(),
                        fence.frame_heads(),
                    )
                    .map_err(map_room_commit_error);
                cached.trace_mut().discard_persisted_history();
                if matches!(
                    &resolution,
                    Ok(RoomCommitResolutionV1::Reprepare
                        | RoomCommitResolutionV1::RetryableKnownAbsent
                        | RoomCommitResolutionV1::Fenced
                        | RoomCommitResolutionV1::Indeterminate
                        | RoomCommitResolutionV1::Fault)
                        | Err(_)
                ) {
                    *slot = None;
                }
                resolution
            })
            .map_err(map_trace_cache_error)?
    }

    fn pack_reference(&self, digest: &PackDigestV1) -> Result<PackReference, BackendError> {
        let retained = self
            .registry
            .load_retained(digest)
            .map_err(|_| BackendError::InvalidResult)?;
        let descriptor = retained.descriptor();
        Ok(PackReference {
            id: descriptor.pack_id.clone(),
            version: descriptor.explanatory_version.clone(),
            digest: digest.to_string(),
        })
    }

    fn member_for_principal(
        trace: &worldstream_core::CanonicalRoomTrace,
        principal_id: &worldstream_core::PrincipalId,
    ) -> Result<MembershipV1, BackendError> {
        trace
            .core_state()
            .memberships()
            .values()
            .find(|membership| {
                membership.principal_id() == principal_id
                    && membership.standing() == MembershipStandingV1::Enabled
            })
            .cloned()
            .ok_or(BackendError::Forbidden)
    }

    fn view_for(
        &self,
        trace: &worldstream_core::CanonicalRoomTrace,
        membership: &MembershipV1,
    ) -> Result<worldstream_core::ValidatedPackViewV1, BackendError> {
        trace
            .view(&viewer_for(membership))
            .map_err(|_| BackendError::InvalidResult)
    }

    fn authenticate(
        &self,
        session: &GatewaySession,
    ) -> Result<worldstream_postgres::PostgresAuthenticatedCapabilityV1, BackendError> {
        let bearer = session
            .owned_bearer()
            .ok_or(BackendError::StorageUnavailable)?;
        self.store
            .authenticate_bearer(bearer)
            .map_err(map_authentication_error)
    }

    fn authorize_activity_pack_catalog(
        &self,
        session: &GatewaySession,
    ) -> Result<(), BackendError> {
        let authenticated = self.authenticate(session)?;
        let grant = self
            .authority()
            .authorize_diagnostic(
                &authenticated.into_presented(),
                worldstream_core::DiagnosticTargetV1::Deployment,
                worldstream_core::DiagnosticOperationV1::ActivityPackCatalog,
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
        let adapter_input = grant.into_adapter_input();
        if adapter_input.target() != &worldstream_core::DiagnosticTargetV1::Deployment
            || adapter_input.operation()
                != worldstream_core::DiagnosticOperationV1::ActivityPackCatalog
        {
            return Err(BackendError::InvalidResult);
        }
        let snapshot = self
            .store
            .snapshot(&adapter_input.authority_snapshot_query())
            .map_err(|_| BackendError::StorageUnavailable)?
            .ok_or(BackendError::Forbidden)?;
        adapter_input
            .revalidate_current(&snapshot, &self.checked_at()?)
            .map_err(map_authority_error)?;
        Ok(())
    }

    fn authorize_runner_presence(&self, session: &GatewaySession) -> Result<(), BackendError> {
        let authenticated = self.authenticate(session)?;
        let checked_at = self.checked_at()?;
        let grant = self
            .authority()
            .authorize_diagnostic(
                &authenticated.into_presented(),
                worldstream_core::DiagnosticTargetV1::Deployment,
                worldstream_core::DiagnosticOperationV1::RunnerPresence,
                checked_at.clone(),
            )
            .map_err(map_authority_error)?;
        let adapter_input = grant.into_adapter_input();
        let snapshot = self
            .store
            .snapshot(&adapter_input.authority_snapshot_query())
            .map_err(|_| BackendError::StorageUnavailable)?
            .ok_or(BackendError::Forbidden)?;
        adapter_input
            .revalidate_current(&snapshot, &checked_at)
            .map_err(map_authority_error)
    }

    fn authorize_backup(&self, session: &GatewaySession) -> Result<(), BackendError> {
        let authenticated = self.authenticate(session)?;
        let checked_at = self.checked_at()?;
        let grant = self
            .authority()
            .authorize_diagnostic(
                &authenticated.into_presented(),
                DiagnosticTargetV1::Deployment,
                DiagnosticOperationV1::Backup,
                checked_at.clone(),
            )
            .map_err(map_authority_error)?;
        let adapter_input = grant.into_adapter_input();
        let snapshot = self
            .store
            .snapshot(&adapter_input.authority_snapshot_query())
            .map_err(|_| BackendError::StorageUnavailable)?
            .ok_or(BackendError::Forbidden)?;
        adapter_input
            .revalidate_current(&snapshot, &checked_at)
            .map_err(map_authority_error)
    }

    fn creation_attempt(
        &self,
        session: &GatewaySession,
        request: &CreateRoomRequest,
        format: worldstream_core::CanonicalHistoryFormat,
    ) -> Result<CreateAttempt, BackendError> {
        let authenticated = self.authenticate(session)?;
        let (legacy_request, identity) = creation_request(&authenticated, request)?;
        let core_request =
            worldstream_core::RoomCreationRequestWithFormat::new(legacy_request, format);
        let grant = match worldstream_core::authorize_canonical_room_creation_operation(
            &self.authority(),
            self.store.as_ref(),
            &authenticated.into_presented(),
            &identity,
            &core_request,
            self.checked_at()?,
        )
        .map_err(map_room_operation_error)?
        {
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
            .as_ref()
            .select_for_new_room(core_request.legacy_request().pack_digest())
            .map_err(|_| BackendError::Rejected)?;
        if selected.descriptor().pack_id != request.pack.id
            || selected.descriptor().explanatory_version != request.pack.version
        {
            return Err(BackendError::Rejected);
        }
        let room_id = next_core_id::<RoomId>()?;
        let genesis = self
            .registry
            .as_ref()
            .prepare_genesis_for_new_room_with_format(
                &PackGenesisRequestV1 {
                    room_id,
                    pack_digest: core_request.legacy_request().pack_digest().clone(),
                    configuration: core_request.legacy_request().configuration().clone(),
                    room_seed: random_room_seed()?,
                    created_at: creation_time()?,
                    initial_core_state: worldstream_core::CoreRoomStateV1::active(
                        request
                            .members
                            .iter()
                            .map(|member| {
                                MembershipV1::new(
                                    next_core_id()?,
                                    member
                                        .principal_id
                                        .parse()
                                        .map_err(|_| BackendError::Rejected)?,
                                    core_principal_kind(member.principal_kind),
                                    MembershipStandingV1::Enabled,
                                    core_access_mode(member.access_mode),
                                    member.role.clone(),
                                )
                                .map_err(|_| BackendError::Rejected)
                            })
                            .collect::<Result<Vec<_>, BackendError>>()?,
                    )
                    .map_err(|_| BackendError::Rejected)?,
                },
                format,
            )
            .map_err(|_| BackendError::Rejected)?;
        let prepared = worldstream_core::PreparedCanonicalRoomCreation::from_registry_genesis(
            identity,
            &core_request,
            grant,
            genesis,
        )
        .map_err(crate::map_canonical_write_preparation_error)?;
        let (resolution, _, _, _) =
            worldstream_core::commit_canonical_room_creation(self.store.as_ref(), prepared)
                .into_parts();
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

    fn projection_response(
        &self,
        session: &GatewaySession,
        room_id: &str,
    ) -> Result<ProjectionResponse, BackendError> {
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::NotFound)?;
        let authenticated = self.authenticate(session)?;
        self.with_serving_trace(&room_id, |trace, fence| {
            let membership = Self::member_for_principal(trace, authenticated.principal_id())?;
            self.authority()
                .authorize_member_read(
                    &authenticated.into_presented(),
                    room_id.clone(),
                    membership.member_id().clone(),
                    MemberReadOperationV1::CurrentProjection,
                    self.checked_at()?,
                )
                .map_err(map_authority_error)?;
            let view = self.view_for(trace, &membership)?;
            Ok(ProjectionResponse {
                room_id: room_id.to_string(),
                room_head: room_head(trace.head()),
                room_health: serving_integrity_status(fence.integrity()),
                integrity_generation: fence.integrity().generation().get(),
                projection_schema: view.projection_schema().to_owned(),
                projection: projection_from_view(&view)?,
                projection_hash: view
                    .projection_hash()
                    .map_err(|_| BackendError::InvalidResult)?
                    .to_string(),
            })
        })
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
        let (trace, verification) = self.verified_trace(&room_id)?;
        let membership = Self::member_for_principal(&trace, authenticated.principal_id())?;
        let replay = self
            .store
            .replay_authorized(
                self.registry.as_ref(),
                self.authority()
                    .authorize_replay(
                        &authenticated.into_presented(),
                        room_id.clone(),
                        membership.member_id().clone(),
                        at_room_seq,
                        ReplayProjectionKindV1::HistoricalMembership,
                        self.checked_at()?,
                    )
                    .map_err(map_authority_error)?,
            )
            .map_err(map_replay_error)?;
        let view_bytes = replay_view_bytes(replay.canonical_envelope())?;
        let projection = projection_from_canonical_bytes(&view_bytes)?;
        let pack = self.pack_reference(replay.verified_head().pack_digest())?;
        Ok(ReplayResponse {
            room_id: room_id.to_string(),
            pack,
            requested_room_seq: at_room_seq.get(),
            room_head: room_head(replay.verified_head()),
            projection,
            projection_hash: worldstream_core::projection_hash_for_canonical_bytes(&view_bytes)
                .map_err(|_| BackendError::InvalidResult)?
                .to_string(),
            verification: "verified".to_owned(),
            room_health: verification.integrity_status,
            integrity_generation: verification.integrity_generation,
        })
    }

    fn attach_response(
        &self,
        session: &GatewaySession,
        request: &RoomAttach,
    ) -> Result<AttachReply, BackendError> {
        let room_id = RoomId::from_str(&request.room_id).map_err(|_| BackendError::Rejected)?;
        let member_id = request
            .member_id
            .parse()
            .map_err(|_| BackendError::Rejected)?;
        let authenticated = self.authenticate(session)?;
        self.with_serving_trace(&room_id, |trace, fence| {
            let membership = trace
                .core_state()
                .membership(&member_id)
                .filter(|membership| {
                    membership.principal_id() == authenticated.principal_id()
                        && membership.standing() == MembershipStandingV1::Enabled
                })
                .cloned()
                .ok_or(BackendError::Forbidden)?;
            let view = self.view_for(trace, &membership)?;
            let authority = self
                .authority()
                .authorize_member_read(
                    &authenticated.into_presented(),
                    room_id.clone(),
                    member_id.clone(),
                    MemberReadOperationV1::Attach,
                    self.checked_at()?,
                )
                .map_err(map_authority_error)?;
            let canonical = CanonicalJsonV1::from_canonical_bytes(view.canonical_bytes())
                .map_err(|_| BackendError::InvalidResult)?;
            let delivery = self
                .store
                .read_observation_at(
                    room_id.as_ref(),
                    member_id.as_ref(),
                    trace.head(),
                    fence.integrity().generation().get(),
                    &authority.into_adapter_input(),
                    request.after_frame_seq,
                    &canonical,
                )
                .map_err(map_observation_error)?;
            let (delivery, recovery_reason) =
                recover_attach_cursor(delivery, request.after_frame_seq, &canonical)?;
            let (sync, reset, frames, frame_head, retained_floor, cursor, reset_generation) =
                Self::protocol_delivery(
                    &delivery,
                    recovery_reason,
                    &room_id,
                    &member_id,
                    trace,
                    fence.integrity(),
                )?;
            let capability = self.authenticate(session)?;
            let token = next_ulid_string()?;
            let mut core_session = SessionV1::new(crate::MAX_OUTBOUND_FRAME_BURST)
                .map_err(|_| BackendError::InvalidResult)?;
            let barrier = core_session
                .capture_barrier(
                    worldstream_core::SessionBarrierV1::new(
                        trace.head().clone(),
                        frame_head,
                        retained_floor,
                        cursor,
                    )
                    .map_err(map_session_error)?,
                )
                .map_err(|_| BackendError::InvalidResult)?;
            self.bindings.issue(
                session.session_id(),
                capability.presented().capability_id(),
                SyncBinding {
                    token: token.clone(),
                    room_id: room_id.clone(),
                    member_id: member_id.clone(),
                    baseline_frame_head: frame_head,
                    installed_reset_generation: reset_generation,
                    session: core_session,
                    core_token: barrier.sync_token().clone(),
                },
            )?;
            let pack = self.pack_reference(trace.head().pack_digest())?;
            Ok(AttachReply {
                attached: RoomAttached {
                    room_id: room_id.to_string(),
                    member_id: member_id.to_string(),
                    principal_kind: protocol_principal_kind(membership.principal_kind()),
                    access_mode: protocol_access_mode(membership.access_mode()),
                    role: membership.role().map(str::to_owned),
                    membership_status: membership_status(membership.standing()),
                    room_status: room_status(trace.core_state().room_status()),
                    room_health: serving_integrity_status(fence.integrity()),
                    integrity_generation: fence.integrity().generation().get(),
                    room_head: room_head(trace.head()),
                    cursor,
                    frame_head,
                    retained_floor,
                    sync_token: token,
                    sync,
                    pack,
                },
                reset,
                frames,
            })
        })
    }

    fn action_response(
        &self,
        session: &GatewaySession,
        request: &ActionSubmit,
    ) -> Result<ActionReply, BackendError> {
        request
            .validate_bounds()
            .map_err(|_| BackendError::Rejected)?;
        let action_room_id =
            RoomId::from_str(&request.room_id).map_err(|_| BackendError::Rejected)?;
        let core_request = participant_action_request(request)?;
        let authenticated = self.authenticate(session)?;
        let ingress = authorize_participant_action_operation(
            &self.authority(),
            self.store.as_ref(),
            &authenticated.into_presented(),
            &core_request,
            self.checked_at()?,
        )
        .map_err(|error| map_participant_action_ingress_error(&error))?;
        match ingress {
            ParticipantActionIngressV1::Existing(result) => {
                action_reply_from_result(request, &result, true)
            }
            ParticipantActionIngressV1::Conflict { .. } => Err(BackendError::Conflict),
            ParticipantActionIngressV1::Authorized(authority) => {
                // The offered action and its exact basis Head come from the
                // same fenced executor used for Core preparation and COMMIT.
                let admission = self
                    .admission_lanes
                    .reserve_action(&action_room_id, self.host_clock.as_ref())
                    .map_err(|error| map_admission_lane_error(&error))?;
                let resolution = self.commit_cached_participant_action(
                    &action_room_id,
                    *authority,
                    &core_request,
                    request,
                    admission.admitted_at().clone(),
                    next_core_id::<TransitionId>()?,
                )?;
                action_reply_from_resolution(request, &resolution)
            }
        }
    }
}

impl GatewayBackend for PostgresGatewayBackend {
    fn room_admission_queue_snapshot(&self) -> worldstream_core::RoomAdmissionQueueSnapshotV1 {
        self.admission_lanes.queue_snapshot()
    }

    fn admission_principal(&self, session: &GatewaySession) -> Result<String, BackendError> {
        Ok(self.authenticate(session)?.principal_id().to_string())
    }

    fn activity_pack_catalog(
        &self,
        session: &GatewaySession,
    ) -> Result<worldstream_protocol::ActivityPackCatalogResponse, BackendError> {
        self.authorize_activity_pack_catalog(session)?;
        Ok(crate::activity_pack_catalog_from_registry(
            self.registry.as_ref(),
        ))
    }

    fn activity_pack_revision(
        &self,
        session: &GatewaySession,
        revision_digest: &str,
    ) -> Result<worldstream_protocol::ActivityPackCatalogRevisionResponse, BackendError> {
        self.authorize_activity_pack_catalog(session)?;
        crate::activity_pack_revision_from_registry(self.registry.as_ref(), revision_digest)
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
        self.create_room_with_format(
            session,
            worldstream_protocol::CreateRoomRequestWithFormat {
                request,
                canonical_history_format: worldstream_protocol::CanonicalHistoryFormatV1::V1,
            },
        )
    }

    fn create_room_with_format(
        &self,
        session: &GatewaySession,
        request: worldstream_protocol::CreateRoomRequestWithFormat,
    ) -> Result<CreateRoomResponse, BackendError> {
        let format = match request.canonical_history_format {
            worldstream_protocol::CanonicalHistoryFormatV1::V1 => {
                worldstream_core::CanonicalHistoryFormat::V1
            }
            worldstream_protocol::CanonicalHistoryFormatV1::V2
                if crate::COMPACT_ROOM_CREATION_ENABLED =>
            {
                worldstream_core::CanonicalHistoryFormat::V2
            }
            worldstream_protocol::CanonicalHistoryFormatV1::V2 => {
                return Err(BackendError::Rejected);
            }
        };
        for _ in 0..3 {
            match self.creation_attempt(session, &request.request, format)? {
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

    fn historical_evidence(
        &self,
        session: &GatewaySession,
        room_id: &str,
        at_room_seq: u64,
        after_room_seq: u64,
    ) -> Result<worldstream_protocol::HistoricalEvidenceResponseV1, BackendError> {
        let replay = self.replay_response(session, room_id, at_room_seq)?;
        let room = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        let page_result = self
            .store
            .historical_evidence_page(&room, after_room_seq, at_room_seq);
        // Recheck current Capability and the requested historical fence after
        // the immutable page read so a racing revocation is fail-closed.
        let fenced = self.replay_response(session, room_id, at_room_seq)?;
        if fenced.room_head != replay.room_head
            || fenced.integrity_generation != replay.integrity_generation
        {
            return Err(BackendError::Busy);
        }
        let page = match page_result {
            Ok(page) => page,
            Err(PostgresHistoricalEvidenceErrorV1::Missing) => {
                return Ok(worldstream_protocol::HistoricalEvidenceResponseV1 {
                    version: HISTORICAL_EVIDENCE_RESPONSE_VERSION.to_owned(),
                    room_id: replay.room_id,
                    cut_room_seq: at_room_seq,
                    after_room_seq,
                    next_after_room_seq: None,
                    outcome: HistoricalEvidenceOutcomeV1::Missing,
                    references: Vec::new(),
                    room_head: replay.room_head,
                    room_health: replay.room_health,
                    integrity_generation: replay.integrity_generation,
                });
            }
            Err(PostgresHistoricalEvidenceErrorV1::Pruned) => {
                return Ok(worldstream_protocol::HistoricalEvidenceResponseV1 {
                    version: HISTORICAL_EVIDENCE_RESPONSE_VERSION.to_owned(),
                    room_id: replay.room_id,
                    cut_room_seq: at_room_seq,
                    after_room_seq,
                    next_after_room_seq: None,
                    outcome: HistoricalEvidenceOutcomeV1::Pruned,
                    references: Vec::new(),
                    room_head: replay.room_head,
                    room_health: replay.room_health,
                    integrity_generation: replay.integrity_generation,
                });
            }
            Err(PostgresHistoricalEvidenceErrorV1::Retired) => {
                return Ok(worldstream_protocol::HistoricalEvidenceResponseV1 {
                    version: HISTORICAL_EVIDENCE_RESPONSE_VERSION.to_owned(),
                    room_id: replay.room_id,
                    cut_room_seq: at_room_seq,
                    after_room_seq,
                    next_after_room_seq: None,
                    outcome: HistoricalEvidenceOutcomeV1::Retired,
                    references: Vec::new(),
                    room_head: replay.room_head,
                    room_health: replay.room_health,
                    integrity_generation: replay.integrity_generation,
                });
            }
            Err(PostgresHistoricalEvidenceErrorV1::BudgetExceeded) => {
                return Err(BackendError::Busy);
            }
            Err(PostgresHistoricalEvidenceErrorV1::StorageUnavailable) => {
                return Err(BackendError::StorageUnavailable);
            }
            Err(PostgresHistoricalEvidenceErrorV1::Corrupt) => {
                return Err(BackendError::InvalidResult);
            }
        };
        let references = page
            .references
            .into_iter()
            .map(|reference| HistoricalEvidenceReferenceV1 {
                room_seq: reference.room_seq(),
                transition_id: reference.transition_id().to_owned(),
                transition_hash: reference.transition_hash().to_owned(),
                previous_lineage_hash: reference.previous_lineage_hash().to_owned(),
                evidence_reference: reference.evidence_reference().to_owned(),
            })
            .collect();
        Ok(worldstream_protocol::HistoricalEvidenceResponseV1 {
            version: HISTORICAL_EVIDENCE_RESPONSE_VERSION.to_owned(),
            room_id: replay.room_id,
            cut_room_seq: at_room_seq,
            after_room_seq,
            next_after_room_seq: page.next_after_room_seq,
            outcome: match page.outcome {
                worldstream_core::HistoricalEvidencePageOutcomeV1::Complete => {
                    HistoricalEvidenceOutcomeV1::Complete
                }
                worldstream_core::HistoricalEvidencePageOutcomeV1::Exhausted => {
                    HistoricalEvidenceOutcomeV1::Exhausted
                }
                worldstream_core::HistoricalEvidencePageOutcomeV1::Missing => {
                    HistoricalEvidenceOutcomeV1::Missing
                }
                worldstream_core::HistoricalEvidencePageOutcomeV1::Pruned => {
                    HistoricalEvidenceOutcomeV1::Pruned
                }
                worldstream_core::HistoricalEvidencePageOutcomeV1::Retired => {
                    HistoricalEvidenceOutcomeV1::Retired
                }
            },
            references,
            room_head: replay.room_head,
            room_health: replay.room_health,
            integrity_generation: replay.integrity_generation,
        })
    }

    fn membership_status(
        &self,
        session: &GatewaySession,
        room_id: &str,
        member_id: &str,
    ) -> Result<worldstream_protocol::MembershipStatusResponse, BackendError> {
        let authenticated = self.authenticate(session)?;
        let room_id: RoomId = room_id.parse().map_err(|_| BackendError::Rejected)?;
        let member_id = member_id.parse().map_err(|_| BackendError::Rejected)?;
        let (trace, verification) = self.verified_trace(&room_id)?;
        let membership = trace
            .core_state()
            .membership(&member_id)
            .filter(|member| {
                member.principal_id() == authenticated.principal_id()
                    && member.standing() == MembershipStandingV1::Enabled
            })
            .ok_or(BackendError::Forbidden)?;
        self.authority()
            .authorize_member_read(
                &authenticated.into_presented(),
                room_id.clone(),
                member_id.clone(),
                MemberReadOperationV1::Attach,
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
        let pack = self.pack_reference(trace.head().pack_digest())?;
        let (current, current_verification) = self.verified_trace(&room_id)?;
        if current.head() != trace.head()
            || current_verification.integrity_generation != verification.integrity_generation
            || current_verification.integrity_status != verification.integrity_status
        {
            return Err(BackendError::Busy);
        }
        Ok(worldstream_protocol::MembershipStatusResponse {
            version: "membership_status.v1".to_owned(),
            room_id: room_id.to_string(),
            member_id: member_id.to_string(),
            principal_kind: protocol_principal_kind(membership.principal_kind()),
            access_mode: protocol_access_mode(membership.access_mode()),
            role: membership.role().map(str::to_owned),
            membership_status: membership_status(membership.standing()),
            pack,
        })
    }

    fn attach(
        &self,
        session: &GatewaySession,
        request: RoomAttach,
    ) -> Result<AttachReply, BackendError> {
        self.attach_response(session, &request)
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
            let authority = self
                .authority()
                .authorize_member_read(
                    &authenticated.into_presented(),
                    room_id.clone(),
                    member_id.clone(),
                    MemberReadOperationV1::CatchUp,
                    self.checked_at()?,
                )
                .map_err(map_authority_error)?;
            let frames = self
                .store
                .read_observation_suffix_bounded(
                    room_id.as_ref(),
                    member_id.as_ref(),
                    &authority.into_adapter_input(),
                    binding.baseline_frame_head,
                    binding.installed_reset_generation,
                )
                .map_err(map_observation_error)?;
            let mut by_sequence = BTreeMap::new();
            for frame in frames {
                let sequence = frame.frame_seq;
                binding
                    .session
                    .publish(
                        SessionFrameV1::new(sequence).map_err(|_| BackendError::InvalidResult)?,
                    )
                    .map_err(map_session_error)?;
                by_sequence.insert(sequence, frame);
            }
            binding
                .session
                .sync_ack(&binding.core_token, request.through_frame_head)
                .map_err(map_session_error)?
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
            Ok(frames) => {
                self.bindings
                    .mark_live(session.session_id(), &capability_id, &binding)?;
                Ok(frames)
            }
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
        let capability_id = authenticated.presented().capability_id().clone();
        let authority = self
            .authority()
            .authorize_member_read(
                &authenticated.into_presented(),
                room_id.clone(),
                member_id.clone(),
                MemberReadOperationV1::CatchUp,
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
        let reset_generation = self.bindings.live_reset_generation(
            session.session_id(),
            &capability_id,
            &room_id,
            &member_id,
        )?;
        self.store
            .read_observation_suffix_bounded(
                room_id.as_ref(),
                member_id.as_ref(),
                &authority.into_adapter_input(),
                after_frame_seq,
                reset_generation,
            )
            .map_err(map_observation_error)?
            .into_iter()
            .map(|frame| observation_deliver(&frame, &room_id, &member_id))
            .collect()
    }

    fn live_observation_page(
        &self,
        session: &GatewaySession,
        room_id: &str,
        member_id: &str,
        after_frame_seq: u64,
        max_frames: usize,
        max_canonical_payload_bytes: usize,
    ) -> Result<crate::LiveObservationPage, BackendError> {
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        let member_id = member_id
            .parse::<worldstream_core::MemberId>()
            .map_err(|_| BackendError::Rejected)?;
        let authenticated = self.authenticate(session)?;
        let capability_id = authenticated.presented().capability_id().clone();
        let authority = self
            .authority()
            .authorize_member_read(
                &authenticated.into_presented(),
                room_id.clone(),
                member_id.clone(),
                MemberReadOperationV1::CatchUp,
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
        let reset_generation = self.bindings.live_reset_generation(
            session.session_id(),
            &capability_id,
            &room_id,
            &member_id,
        )?;
        let (frames, has_more) = self
            .store
            .read_observation_page_bounded(
                room_id.as_ref(),
                member_id.as_ref(),
                &authority.into_adapter_input(),
                after_frame_seq,
                reset_generation,
                max_frames,
                max_canonical_payload_bytes,
            )
            .map_err(map_observation_error)?;
        let frames = frames
            .into_iter()
            .map(|frame| observation_deliver(&frame, &room_id, &member_id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(crate::LiveObservationPage { frames, has_more })
    }

    fn supports_shared_live_observation_cut(&self) -> bool {
        true
    }
    fn prepare_live_observation_batch(
        &self,
        recipients: &[crate::LiveObservationRecipient],
        max_frames: usize,
        max_bytes: usize,
    ) -> Vec<Result<crate::LiveObservationPreparedPage, BackendError>> {
        let addresses: std::collections::BTreeSet<_> = recipients
            .iter()
            .map(|r| (&r.room_id, &r.member_id, r.after_frame_seq))
            .collect();
        if recipients.len() > 16 || addresses.len() > 2 {
            return recipients
                .iter()
                .map(|_| Err(BackendError::InvalidResult))
                .collect();
        }
        let mut results: Vec<Option<Result<crate::LiveObservationPreparedPage, BackendError>>> =
            (0..recipients.len()).map(|_| None).collect();
        let mut groups = BTreeMap::new();
        for (index, recipient) in recipients.iter().enumerate() {
            let prepare = || {
                let room: RoomId = recipient
                    .room_id
                    .parse()
                    .map_err(|_| BackendError::Rejected)?;
                let member: worldstream_core::MemberId = recipient
                    .member_id
                    .parse()
                    .map_err(|_| BackendError::Rejected)?;
                let authenticated = self.authenticate(&recipient.session)?;
                let capability = authenticated.presented().capability_id().clone();
                let grant = self
                    .authority()
                    .authorize_member_read(
                        &authenticated.into_presented(),
                        room.clone(),
                        member.clone(),
                        MemberReadOperationV1::CatchUp,
                        self.checked_at()?,
                    )
                    .map_err(map_authority_error)?;
                let reset = self.bindings.live_reset_generation(
                    recipient.session.session_id(),
                    &capability,
                    &room,
                    &member,
                )?;
                Ok::<_, BackendError>((
                    room,
                    member,
                    Arc::new(grant.into_adapter_input()),
                    reset,
                    capability,
                ))
            };
            match prepare() {
                Ok((room, member, grant, reset, capability)) => {
                    groups
                        .entry((room, member, recipient.after_frame_seq))
                        .or_insert_with(Vec::new)
                        .push((index, grant, reset, capability));
                }
                Err(error) => results[index] = Some(Err(error)),
            }
        }
        for ((room, member, after), group) in groups {
            let reset = group[0].2;
            let grants: Vec<_> = group
                .iter()
                .map(|(_, grant, _, _)| Arc::clone(grant))
                .collect();
            let shared = if group.iter().all(|item| item.2 == reset) {
                #[cfg(test)]
                self.live_payload_page_reads
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.store
                    .read_shared_observation_page(&grants, after, reset, max_frames, max_bytes)
                    .ok()
            } else {
                None
            };
            let mut page = shared.map(|(frames, more, cut)| (frames, more, Arc::new(cut)));
            let mut page_reset = reset;
            for (index, grant, reset, _capability) in &group {
                // A raced invalid grant cannot deny other Sessions. The first
                // valid independent cut becomes the candidate; every later
                // grant must prove compatibility using immutable metadata.
                if page.is_none() {
                    #[cfg(test)]
                    self.live_payload_page_reads
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    match self.store.read_shared_observation_page(
                        &[Arc::clone(grant)],
                        after,
                        *reset,
                        max_frames,
                        max_bytes,
                    ) {
                        Ok((frames, more, cut)) => {
                            page_reset = *reset;
                            page = Some((frames, more, Arc::new(cut)));
                        }
                        Err(error) => {
                            results[*index] = Some(Err(map_observation_error(error)));
                            continue;
                        }
                    }
                }
                if *reset != page_reset {
                    results[*index] = Some(Err(BackendError::ResetRequired));
                    continue;
                }
                let Some((_, _, cut)) = page.as_ref() else {
                    results[*index] = Some(Err(BackendError::InvalidResult));
                    continue;
                };
                if let Err(error) = self.store.revalidate_live_observation(grant, cut) {
                    results[*index] = Some(Err(map_observation_error(error)));
                }
            }
            if let Some((frames, has_more, cut)) = page {
                let converted = frames
                    .iter()
                    .map(|frame| observation_deliver(frame, &room, &member))
                    .collect::<Result<Vec<_>, _>>();
                match converted {
                    Ok(frames) => {
                        let frames: Arc<[ObservationDeliver]> = frames.into();
                        for (index, grant, reset, capability) in group {
                            if results[index].is_some() {
                                continue;
                            }
                            let fence = PostgresLiveFence {
                                store: self.store.clone(),
                                bindings: Arc::clone(&self.bindings),
                                grant,
                                cut: Arc::clone(&cut),
                                session_id: recipients[index].session.session_id().clone(),
                                capability_id: capability,
                                reset,
                            };
                            results[index] = Some(Ok(crate::LiveObservationPreparedPage {
                                frames: Arc::clone(&frames),
                                has_more,
                                fence: Some(Arc::new(fence)),
                            }));
                        }
                    }
                    Err(_) => {
                        for (index, _, _, _) in group {
                            if results[index].is_none() {
                                results[index] = Some(Err(BackendError::InvalidResult));
                            }
                        }
                    }
                }
            }
        }
        results
            .into_iter()
            .map(|result| result.unwrap_or(Err(BackendError::InvalidResult)))
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
            .parse::<worldstream_core::MemberId>()
            .map_err(|_| BackendError::Rejected)?;
        let authenticated = self.authenticate(session)?;
        self.authority()
            .authorize_member_read(
                &authenticated.into_presented(),
                room_id.clone(),
                member_id.clone(),
                MemberReadOperationV1::AcknowledgeObservation,
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
        self.store
            .acknowledge_observation(
                room_id.as_ref(),
                member_id.as_ref(),
                request.through_frame_seq,
            )
            .map_err(map_observation_error)
    }

    fn action(
        &self,
        session: &GatewaySession,
        request: ActionSubmit,
    ) -> Result<ActionReply, BackendError> {
        self.action_response(session, &request)
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
        self.with_serving_trace(&room_id, |_, _| Ok(()))?;
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
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
        if candidate.state() == PostgresTimerStateV1::Fired {
            let identity = timer_request.operation_identity();
            let request_hash = timer_request
                .canonical_request_hash()
                .map_err(|_| BackendError::InvalidResult)?;
            return match RoomCommitStorageV1::resolve(self.store.as_ref(), &identity, &request_hash)
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
        }
        if candidate.state() != PostgresTimerStateV1::Scheduled
            || !self
                .store
                .timer_candidate_is_due(&candidate)
                .map_err(|_| BackendError::StorageUnavailable)?
        {
            return Err(BackendError::Busy);
        }
        let _admission = self
            .admission_lanes
            .reserve_host_stimulus(&room_id)
            .map_err(|error| map_admission_lane_error(&error))?;
        let resolution = self.commit_cached_timer(
            &room_id,
            authority,
            timer_request,
            next_core_id::<TransitionId>()?,
        )?;
        timer_response_from_resolution(timer_request, &resolution)
    }

    #[allow(clippy::too_many_lines)]
    fn ingest_external_input(
        &self,
        session: &GatewaySession,
        room_id: &str,
        request: worldstream_protocol::ExternalInputIngressRequestV1,
    ) -> Result<worldstream_protocol::ExternalInputIngressResponseV1, BackendError> {
        let authenticated = self.authenticate(session)?;
        let checked_at = self.checked_at()?;
        let presented = authenticated.into_presented();
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        self.authority()
            .authorize_diagnostic(
                &presented,
                DiagnosticTargetV1::Room(room_id.clone()),
                DiagnosticOperationV1::SafeRoomSummary,
                checked_at.clone(),
            )
            .map_err(map_authority_error)?;
        let mut plan = prepare_external_input_ingress(room_id, &request, &checked_at)?;
        let receipt_grant = self
            .authority()
            .authorize_receipt_read(
                &presented,
                plan.identity.clone(),
                plan.request_hash.clone(),
                Some(plan.room_id.clone()),
                checked_at.clone(),
            )
            .map_err(map_authority_error)?;
        match self
            .store
            .resolve_authorized(receipt_grant)
            .map_err(map_authority_error)?
        {
            worldstream_core::ResolveOutcomeV1::StoredResolution(result) => {
                return ingress_response_from_result(&plan, &result, true);
            }
            worldstream_core::ResolveOutcomeV1::Conflict { .. } => {
                return Err(BackendError::Conflict);
            }
            worldstream_core::ResolveOutcomeV1::ResolutionUnavailable => {
                return Err(BackendError::Indeterminate);
            }
            worldstream_core::ResolveOutcomeV1::KnownAbsent => {}
        }
        let authority = self
            .authority()
            .authorize_external_input(
                &presented,
                plan.room_id.clone(),
                plan.request_hash.clone(),
                checked_at,
            )
            .map_err(map_authority_error)?;
        self.with_serving_trace(&plan.room_id, |trace, _| {
            if trace.head().pack_digest() != &plan.pack_digest {
                return Err(BackendError::Conflict);
            }
            Ok(())
        })?;
        let _admission = self
            .admission_lanes
            .reserve_host_stimulus(&plan.room_id)
            .map_err(|error| map_admission_lane_error(&error))?;
        plan.input.recorded_at = self
            .store
            .reserve_external_input_recorded_at(
                &plan.identity,
                &plan.request_hash,
                &plan.input.recorded_at,
            )
            .map_err(map_external_input_preparation_error)?;
        let resolution = self.commit_cached_external_input(
            &plan.room_id,
            authority,
            plan.based_on_room_seq,
            &plan.input,
            next_core_id::<TransitionId>()?,
        )?;
        ingress_response_from_result(
            &plan,
            resolution.stored_result().ok_or(BackendError::Busy)?,
            resolution.duplicate(),
        )
    }

    #[allow(clippy::too_many_lines)]
    fn launch_lobby(
        &self,
        session: &GatewaySession,
        room_id: &str,
        request: LobbyLaunchRequest,
    ) -> Result<LobbyLaunchResponse, BackendError> {
        let authenticated = self.authenticate(session)?;
        let checked_at = self.checked_at()?;
        let presented = authenticated.into_presented();
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        self.authority()
            .authorize_diagnostic(
                &presented,
                DiagnosticTargetV1::Room(room_id.clone()),
                DiagnosticOperationV1::SafeRoomSummary,
                checked_at.clone(),
            )
            .map_err(map_authority_error)?;
        let mut plan =
            prepare_activity_start_request(self.registry.as_ref(), room_id, &request, &checked_at)?;
        let receipt_grant = self
            .authority()
            .authorize_receipt_read(
                &presented,
                plan.identity.clone(),
                plan.request_hash.clone(),
                Some(plan.room_id.clone()),
                checked_at.clone(),
            )
            .map_err(map_authority_error)?;
        match self
            .store
            .resolve_authorized(receipt_grant)
            .map_err(map_authority_error)?
        {
            worldstream_core::ResolveOutcomeV1::StoredResolution(result) => {
                return lobby_response_from_result(
                    &plan,
                    self.registry.as_ref(),
                    &request.input_id,
                    &result,
                    true,
                );
            }
            worldstream_core::ResolveOutcomeV1::Conflict { .. } => {
                return Err(BackendError::Conflict);
            }
            worldstream_core::ResolveOutcomeV1::ResolutionUnavailable => {
                return Err(BackendError::Indeterminate);
            }
            worldstream_core::ResolveOutcomeV1::KnownAbsent => {}
        }
        let authority = self
            .authority()
            .authorize_external_input(
                &presented,
                plan.room_id.clone(),
                plan.request_hash.clone(),
                checked_at,
            )
            .map_err(map_authority_error)?;
        let revision_digest = self.with_serving_trace(&plan.room_id, |trace, _| {
            Ok(trace.head().pack_digest().clone())
        })?;
        plan.validate_room_pack(self.registry.as_ref(), &revision_digest)?;
        if !self
            .registry
            .activity_start_is_approved(&revision_digest)
            .map_err(|_| BackendError::InvalidResult)?
        {
            return Err(BackendError::WrongPhase);
        }
        let _admission = self
            .admission_lanes
            .reserve_host_stimulus(&plan.room_id)
            .map_err(|error| map_admission_lane_error(&error))?;
        match RoomCommitStorageV1::resolve(self.store.as_ref(), &plan.identity, &plan.request_hash)
        {
            worldstream_core::ResolveOutcomeV1::StoredResolution(result) => {
                return lobby_response_from_result(
                    &plan,
                    self.registry.as_ref(),
                    &request.input_id,
                    &result,
                    true,
                );
            }
            worldstream_core::ResolveOutcomeV1::Conflict { .. } => {
                return Err(BackendError::Conflict);
            }
            worldstream_core::ResolveOutcomeV1::ResolutionUnavailable => {
                return Err(BackendError::Indeterminate);
            }
            worldstream_core::ResolveOutcomeV1::KnownAbsent => {}
        }
        let contract = self.with_serving_trace(&plan.room_id, |trace, _| {
            plan.validate_room_pack(self.registry.as_ref(), trace.head().pack_digest())?;
            let contract = match self
                .registry
                .activity_start_compatibility(trace.head().pack_digest())
                .map_err(|_| BackendError::InvalidResult)?
            {
                worldstream_core::ActivityStartCompatibilityV1::Supported(contract) => contract,
                _ => return Err(BackendError::WrongPhase),
            };
            if !worldstream_core::activity_start_is_applicable(&contract, trace.activity_state()) {
                return Err(BackendError::WrongPhase);
            }
            Ok(contract)
        })?;
        let proposed_recorded_at = self.checked_at()?;
        plan.input.recorded_at = ExternalInputRecordedAt::from_str(proposed_recorded_at.as_str())
            .map_err(|_| BackendError::StorageUnavailable)?;
        plan.input.recorded_at = self
            .store
            .reserve_external_input_recorded_at(
                &plan.identity,
                &plan.request_hash,
                &plan.input.recorded_at,
            )
            .map_err(map_external_input_preparation_error)?;
        match RoomCommitStorageV1::resolve(self.store.as_ref(), &plan.identity, &plan.request_hash)
        {
            worldstream_core::ResolveOutcomeV1::StoredResolution(result) => {
                return lobby_response_from_result(
                    &plan,
                    self.registry.as_ref(),
                    &request.input_id,
                    &result,
                    true,
                );
            }
            worldstream_core::ResolveOutcomeV1::Conflict { .. } => {
                return Err(BackendError::Conflict);
            }
            worldstream_core::ResolveOutcomeV1::ResolutionUnavailable => {
                return Err(BackendError::Indeterminate);
            }
            worldstream_core::ResolveOutcomeV1::KnownAbsent => {}
        }
        self.with_serving_trace(&plan.room_id, |trace, _| {
            plan.validate_room_pack(self.registry.as_ref(), trace.head().pack_digest())?;
            if !worldstream_core::activity_start_is_applicable(&contract, trace.activity_state()) {
                return Err(BackendError::WrongPhase);
            }
            Ok(())
        })?;
        let resolution = self.commit_cached_external_input(
            &plan.room_id,
            authority,
            plan.based_on_room_seq,
            &plan.input,
            next_core_id::<TransitionId>()?,
        )?;
        lobby_response_from_resolution(
            &plan,
            self.registry.as_ref(),
            &request.input_id,
            &resolution,
        )
    }

    fn resolve_lobby_launch(
        &self,
        session: &GatewaySession,
        room_id: &str,
        request: LobbyLaunchRequest,
    ) -> Result<Option<LobbyLaunchResponse>, BackendError> {
        let authenticated = self.authenticate(session)?;
        let checked_at = self.checked_at()?;
        let presented = authenticated.into_presented();
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        self.authority()
            .authorize_diagnostic(
                &presented,
                DiagnosticTargetV1::Room(room_id.clone()),
                DiagnosticOperationV1::SafeRoomSummary,
                checked_at.clone(),
            )
            .map_err(map_authority_error)?;
        let plan =
            prepare_activity_start_request(self.registry.as_ref(), room_id, &request, &checked_at)?;
        let grant = self
            .authority()
            .authorize_receipt_read(
                &presented,
                plan.identity.clone(),
                plan.request_hash.clone(),
                Some(plan.room_id.clone()),
                checked_at,
            )
            .map_err(map_authority_error)?;
        let outcome = self
            .store
            .resolve_authorized(grant)
            .map_err(map_authority_error)?;
        match outcome {
            worldstream_core::ResolveOutcomeV1::StoredResolution(result) => {
                lobby_response_from_result(
                    &plan,
                    self.registry.as_ref(),
                    &request.input_id,
                    &result,
                    true,
                )
                .map(Some)
            }
            worldstream_core::ResolveOutcomeV1::Conflict { .. } => Err(BackendError::Conflict),
            worldstream_core::ResolveOutcomeV1::ResolutionUnavailable => {
                Err(BackendError::Indeterminate)
            }
            worldstream_core::ResolveOutcomeV1::KnownAbsent => {
                self.with_serving_trace(&plan.room_id, |trace, _| {
                    plan.validate_room_pack(self.registry.as_ref(), trace.head().pack_digest())
                })?;
                Ok(None)
            }
        }
    }

    fn archive_room(
        &self,
        session: &GatewaySession,
        room_id: &str,
        request: RoomArchiveRequestV1,
    ) -> Result<RoomArchiveResponseV1, BackendError> {
        request
            .validate_bounds()
            .map_err(|_| BackendError::Rejected)?;
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        let authenticated = self.authenticate(session)?;
        let principal_id = authenticated.principal_id().clone();
        let presented = authenticated.into_presented();
        let (fallback_head, current_status) = self.with_serving_trace(&room_id, |trace, _| {
            Ok((trace.head().clone(), trace.core_state().room_status()))
        })?;
        let checked_at = self.checked_at()?;
        let recorded_at = CoreRecordedAt::from_str(checked_at.as_str())
            .map_err(|_| BackendError::StorageUnavailable)?;
        let core_request = CoreAdministrationRequestV1::new(
            room_id.clone(),
            AdministrationOperationIdentityV1 {
                authenticated_principal: principal_id,
                versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
                idempotency_key: request.idempotency_key,
            },
            CoreProposedKindV1::Archive,
            fallback_head.room_seq(),
            "hosted_creator_close",
            CoreChangeSetV1::archive(current_status),
        )
        .map_err(|_| BackendError::Rejected)?;
        let ingress = authorize_core_administration_operation(
            &self.authority(),
            self.store.as_ref(),
            &presented,
            &core_request,
            checked_at,
        )
        .map_err(map_room_operation_error)?;
        if current_status == worldstream_core::RoomStatusV1::Archived {
            // Still pass the retry through Core's receipt/admin authority
            // boundary. The original archive request necessarily differs
            // from this current-state proposal, so either an authorized
            // request or the same principal's retained identity conflict is
            // sufficient before reporting the already-achieved outcome.
            return match ingress {
                CoreAdministrationIngressV1::Existing(result) => {
                    archive_response_from_result(&core_request, &result, &fallback_head, true)
                }
                CoreAdministrationIngressV1::Conflict { .. }
                | CoreAdministrationIngressV1::Authorized(_) => Ok(RoomArchiveResponseV1 {
                    schema: ROOM_ARCHIVE_RESPONSE_SCHEMA_V1.to_owned(),
                    room_id: room_id.to_string(),
                    room_head: room_head(&fallback_head),
                    duplicate: true,
                }),
            };
        }
        match ingress {
            CoreAdministrationIngressV1::Existing(result) => {
                archive_response_from_result(&core_request, &result, &fallback_head, true)
            }
            CoreAdministrationIngressV1::Conflict { .. } => Err(BackendError::Conflict),
            CoreAdministrationIngressV1::Authorized(authority) => {
                let _admission = self
                    .admission_lanes
                    .reserve_host_stimulus(&room_id)
                    .map_err(|error| map_admission_lane_error(&error))?;
                let resolution = self.commit_cached_core_administration(
                    &room_id,
                    *authority,
                    &core_request,
                    recorded_at,
                    next_core_id::<TransitionId>()?,
                )?;
                archive_response_from_resolution(&core_request, &resolution, &fallback_head)
            }
        }
    }

    fn operator_room_inventory(
        &self,
        session: &GatewaySession,
        request: OperatorRoomInventoryRequest,
    ) -> Result<OperatorRoomInventoryPage, BackendError> {
        let authenticated = self.authenticate(session)?;
        let checked_at = self.checked_at()?;
        let authority = self
            .authority()
            .authorize_diagnostic(
                &authenticated.into_presented(),
                DiagnosticTargetV1::Deployment,
                DiagnosticOperationV1::SafeRoomSummary,
                checked_at.clone(),
            )
            .map_err(map_authority_error)?;
        let after_room_id = request
            .after_room_id
            .as_deref()
            .map(RoomId::from_str)
            .transpose()
            .map_err(|_| BackendError::Rejected)?;
        let page = self
            .store
            .diagnostic_inventory_page(
                authority,
                &checked_at,
                after_room_id.as_ref(),
                request.limit,
            )
            .map_err(|error| map_postgres_diagnostic_error(&error))?;
        let rooms = page
            .rooms()
            .iter()
            .map(|summary| operator_room_summary(summary, checked_at.as_str()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(OperatorRoomInventoryPage {
            rooms,
            next_after_room_id: page.next_after_room_id().map(ToString::to_string),
        })
    }

    fn operator_room_detail(
        &self,
        session: &GatewaySession,
        room_id: &str,
    ) -> Result<OperatorRoomSummary, BackendError> {
        let authenticated = self.authenticate(session)?;
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::NotFound)?;
        let checked_at = self.checked_at()?;
        let authority = self
            .authority()
            .authorize_diagnostic(
                &authenticated.into_presented(),
                DiagnosticTargetV1::Room(room_id),
                DiagnosticOperationV1::SafeRoomSummary,
                checked_at.clone(),
            )
            .map_err(map_authority_error)?;
        let summary = self
            .store
            .diagnostic_summary(authority, &checked_at)
            .map_err(|error| map_postgres_diagnostic_error(&error))?;
        operator_room_summary(&summary, checked_at.as_str())
    }

    fn operator_activation_status(
        &self,
        session: &GatewaySession,
        room_id: &str,
        member_id: &str,
    ) -> Result<OperatorActivationStatusV1, BackendError> {
        let authenticated = self.authenticate(session)?;
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::NotFound)?;
        let member_id = member_id
            .parse::<worldstream_core::MemberId>()
            .map_err(|_| BackendError::NotFound)?;
        let checked_at = self.checked_at()?;
        let authority = self
            .authority()
            .authorize_diagnostic(
                &authenticated.into_presented(),
                DiagnosticTargetV1::Room(room_id),
                DiagnosticOperationV1::SafeRoomSummary,
                checked_at.clone(),
            )
            .map_err(map_authority_error)?;
        let status = self
            .store
            .diagnostic_activation_status(authority, &checked_at, &member_id)
            .map_err(|error| map_postgres_diagnostic_error(&error))?;
        Ok(OperatorActivationStatusV1 {
            version: OPERATOR_ACTIVATION_STATUS_VERSION.to_owned(),
            waiting: status.waiting(),
            leased: status.leased(),
            observed_at_unix_ms: crate::checked_unix_time_ms()?,
        })
    }

    fn operator_backup_profile(
        &self,
        session: &GatewaySession,
    ) -> Result<OperatorBackupProfileStatus, BackendError> {
        self.authorize_backup(session)?;
        Ok(OperatorBackupProfileStatus {
            storage_profile: OperatorBackupStorageProfile::PostgresPrimary,
            storage_health: OperatorBackupStorageHealth::Healthy,
            live_backup_supported: false,
            verification: OperatorBackupVerification::Unavailable,
            freshness: OperatorDataFreshness::Unavailable {
                reason: "provider_managed_backup_required".to_owned(),
            },
        })
    }

    fn operator_live_backup(
        &self,
        session: &GatewaySession,
        request: OperatorLiveBackupPrepareRequest,
    ) -> Result<OperatorLiveBackupStatus, BackendError> {
        self.authorize_backup(session)?;
        Ok(OperatorLiveBackupStatus {
            operation_id: request.operation_id,
            storage_profile: OperatorBackupStorageProfile::PostgresPrimary,
            storage_health: OperatorBackupStorageHealth::Healthy,
            native_verification: OperatorBackupVerification::Unavailable,
            semantic_verification: OperatorBackupVerification::Unavailable,
            freshness: OperatorDataFreshness::Unavailable {
                reason: "provider_managed_backup_required".to_owned(),
            },
            artifact: None,
            unavailable_reason: Some("provider_managed_backup_required".to_owned()),
        })
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

    fn provision_member_capability(
        &self,
        session: &GatewaySession,
        request: MemberCapabilityProvisionRequestV1,
    ) -> Result<MemberCapabilityProvisionResponseV1, BackendError> {
        self.provision_member_capability_inner(session, request)
    }

    fn provision_runner_capability(
        &self,
        session: &GatewaySession,
        request: RunnerCapabilityProvisionRequestV1,
    ) -> Result<RunnerCapabilityProvisionResponseV1, BackendError> {
        self.provision_runner_capability_inner(session, request)
    }

    fn authorize_operator_runner_presence(
        &self,
        session: &GatewaySession,
    ) -> Result<(), BackendError> {
        self.authorize_runner_presence(session)
    }

    fn runner_hello(
        &self,
        session: &GatewaySession,
        request: RunnerHello,
    ) -> Result<RunnerReady, BackendError> {
        let authenticated = self.authenticate(session)?;
        let runner_id = request
            .runner_id
            .parse::<RunnerId>()
            .map_err(|_| BackendError::Rejected)?;
        if authenticated.runner_id() != Some(&runner_id) {
            return Err(BackendError::Forbidden);
        }
        if !crate::runner_hello_is_bounded(&request) {
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
        let offers = self
            .store
            .offer_activations_authorized(authority, operation)
            .map_err(map_activation_error)?;
        Ok(ActivationOffers {
            operation_id: request.operation_id,
            runner_id: request.runner_id,
            offers: offers
                .into_iter()
                .map(|offer| ActivationOffer {
                    activation_id: offer.activation_id,
                    room_id: offer.room_id,
                    member_id: offer.target_member_id,
                    cause_room_seq: offer.cause_room_seq,
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
            room_id.clone(),
            member_id,
        )?;
        let operation = Self::activation_request(ActivationRequestParts {
            operation_kind: "claim".to_owned(),
            operation_id: request.claim_id.clone(),
            activation_id: Some(request.activation_id),
            claim_id: Some(request.claim_id),
            runner_id: request.runner_id,
            lease_generation: None,
            requested_lease_ms: Some(request.requested_lease_ms),
            disposition: None,
        });
        let preparation = self.with_serving_trace(&room_id, |trace, fence| {
            self.store
                .prepare_activation_claim_from_canonical_serving_trace(
                    self.registry.as_ref(),
                    authority,
                    operation,
                    trace,
                    fence.integrity(),
                )
                .map_err(map_activation_error)
        })?;
        match preparation {
            worldstream_postgres::PostgresActivationClaimPreparationV1::Existing(result) => {
                Self::activation_reply(*result)
            }
            worldstream_postgres::PostgresActivationClaimPreparationV1::Prepared(claim) => self
                .store
                .claim_activation_authorized(*claim)
                .map_err(map_activation_error)
                .and_then(Self::activation_reply),
        }
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

    fn scheduler_tick(&self) -> Result<Vec<String>, BackendError> {
        self.store
            .reclaim_expired_activation_leases()
            .map_err(|_| BackendError::StorageUnavailable)?;
        self.observation_retention
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?
            .tick(&self.store)
            .map_err(|_| BackendError::StorageUnavailable)
    }
}

enum CreateAttempt {
    Response(Box<CreateRoomResponse>),
    Retry,
}

struct SessionBindings(Mutex<BTreeMap<worldstream_protocol::UlidString, SessionBinding>>);

const MAX_SESSION_BINDINGS: usize = 1_024;

impl Default for SessionBindings {
    fn default() -> Self {
        Self(Mutex::new(BTreeMap::new()))
    }
}

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
                    live: None,
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
        binding.live = None;
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
        if binding.capability_id != *capability_id || binding.sync.is_some() {
            return Err(BackendError::StorageUnavailable);
        }
        binding.sync = Some(sync);
        Ok(())
    }

    fn mark_live(
        &self,
        session_id: &worldstream_protocol::UlidString,
        capability_id: &worldstream_core::CapabilityId,
        sync: &SyncBinding,
    ) -> Result<(), BackendError> {
        let mut bindings = self
            .0
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?;
        let binding = bindings
            .get_mut(session_id)
            .ok_or(BackendError::Forbidden)?;
        if binding.capability_id != *capability_id || binding.sync.is_some() {
            return Err(BackendError::StorageUnavailable);
        }
        binding.live = Some(LiveObservationBinding {
            room_id: sync.room_id.clone(),
            member_id: sync.member_id.clone(),
            reset_generation: sync.installed_reset_generation,
        });
        Ok(())
    }

    fn live_reset_generation(
        &self,
        session_id: &worldstream_protocol::UlidString,
        capability_id: &worldstream_core::CapabilityId,
        room_id: &RoomId,
        member_id: &worldstream_core::MemberId,
    ) -> Result<u64, BackendError> {
        let bindings = self
            .0
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?;
        let binding = bindings.get(session_id).ok_or(BackendError::Forbidden)?;
        let live = binding.live.as_ref().ok_or(BackendError::ResetRequired)?;
        if binding.capability_id != *capability_id
            || live.room_id != *room_id
            || live.member_id != *member_id
        {
            return Err(BackendError::ResetRequired);
        }
        Ok(live.reset_generation)
    }
}

struct SessionBinding {
    capability_id: worldstream_core::CapabilityId,
    sync: Option<SyncBinding>,
    live: Option<LiveObservationBinding>,
}

struct LiveObservationBinding {
    room_id: RoomId,
    member_id: worldstream_core::MemberId,
    reset_generation: u64,
}

struct SyncBinding {
    token: String,
    room_id: RoomId,
    member_id: worldstream_core::MemberId,
    baseline_frame_head: u64,
    installed_reset_generation: u64,
    session: SessionV1,
    core_token: SessionSyncTokenV1,
}

impl PostgresGatewayBackend {
    fn protocol_delivery(
        delivery: &PostgresObservationDeliveryV1,
        recovery_reason: Option<&str>,
        room_id: &RoomId,
        member_id: &worldstream_core::MemberId,
        trace: &worldstream_core::CanonicalRoomTrace,
        integrity: &RoomIntegrityStateV1,
    ) -> Result<ProtocolDelivery, BackendError> {
        match delivery {
            PostgresObservationDeliveryV1::Retained {
                cursor,
                cursor_exclusive,
                frame_head,
                retained_floor,
                reset_generation,
                frames,
            } => Ok((
                SyncBranch::RetainedFrames {
                    cursor_exclusive: *cursor_exclusive,
                    through_frame_head: *frame_head,
                },
                None,
                frames
                    .iter()
                    .map(|frame| observation_deliver(frame, room_id, member_id))
                    .collect::<Result<Vec<_>, _>>()?,
                *frame_head,
                *retained_floor,
                *cursor,
                *reset_generation,
            )),
            PostgresObservationDeliveryV1::Reset {
                cursor,
                frame_head,
                retained_floor,
                reset_through,
                reset_generation,
                projection_bytes,
            } => {
                let projection = projection_from_canonical_bytes(projection_bytes)?;
                let projection_schema = projection_schema_from_canonical_bytes(projection_bytes)?;
                let projection_hash =
                    worldstream_core::projection_hash_for_canonical_bytes(projection_bytes)
                        .map_err(|_| BackendError::InvalidResult)?
                        .to_string();
                let reset_reason = if let Some(reason) = recovery_reason {
                    reason
                } else if cursor.is_none() {
                    "first_attach"
                } else if reset_through.is_some() {
                    "reset_marked"
                } else {
                    "retained_range_unavailable"
                };
                Ok((
                    SyncBranch::ProjectionReset {
                        baseline_frame_head: *frame_head,
                        reason: reset_reason.to_owned(),
                    },
                    Some(ProjectionReset {
                        room_id: room_id.to_string(),
                        member_id: member_id.to_string(),
                        room_head: room_head(trace.head()),
                        room_health: serving_integrity_status(integrity),
                        integrity_generation: integrity.generation().get(),
                        baseline_frame_head: *frame_head,
                        reset_reason: reset_reason.to_owned(),
                        projection_schema,
                        projection,
                        projection_hash,
                    }),
                    Vec::new(),
                    *frame_head,
                    *retained_floor,
                    *cursor,
                    *reset_generation,
                ))
            }
        }
    }
}

fn recover_attach_cursor(
    delivery: PostgresObservationDeliveryV1,
    requested_cursor: Option<u64>,
    current_projection: &CanonicalJsonV1,
) -> Result<(PostgresObservationDeliveryV1, Option<&'static str>), BackendError> {
    let cursor = match &delivery {
        PostgresObservationDeliveryV1::Retained { cursor, .. }
        | PostgresObservationDeliveryV1::Reset { cursor, .. } => *cursor,
    };
    // A lost ACK receipt permits a conservative client Cursor, but never a
    // client assertion of progress beyond the durable Membership Cursor.
    if requested_cursor > cursor {
        return Err(BackendError::Rejected);
    }
    let recovery_reason = (requested_cursor < cursor).then_some("client_cursor_behind");
    if recovery_reason.is_some()
        && let PostgresObservationDeliveryV1::Retained {
            cursor,
            frame_head,
            retained_floor,
            reset_generation,
            ..
        } = delivery
    {
        return Ok((
            PostgresObservationDeliveryV1::Reset {
                cursor,
                frame_head,
                retained_floor,
                reset_through: None,
                reset_generation,
                projection_bytes: current_projection
                    .to_bytes()
                    .map_err(|_| BackendError::InvalidResult)?,
            },
            recovery_reason,
        ));
    }
    Ok((delivery, recovery_reason))
}

fn viewer_for(membership: &MembershipV1) -> PackViewerV1 {
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

fn membership_status(standing: MembershipStandingV1) -> String {
    match standing {
        MembershipStandingV1::Enabled => "enabled",
        MembershipStandingV1::Suspended => "suspended",
        MembershipStandingV1::Departed => "departed",
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

fn operator_room_summary(
    summary: &PostgresRoomDiagnosticSummaryV1,
    observed_at: &str,
) -> Result<OperatorRoomSummary, BackendError> {
    if summary.head().room_id() != summary.room_id()
        || summary.head().pack_digest()
            != &summary
                .pack_revision()
                .revision_digest()
                .map_err(|_| BackendError::InvalidResult)?
    {
        return Err(BackendError::InvalidResult);
    }
    let integrity_status = match summary.integrity().status() {
        worldstream_core::RoomIntegrityStatusV1::Healthy => OperatorRoomIntegrityStatus::Healthy,
        worldstream_core::RoomIntegrityStatusV1::Faulted => OperatorRoomIntegrityStatus::Faulted,
        worldstream_core::RoomIntegrityStatusV1::Quarantined => {
            OperatorRoomIntegrityStatus::Quarantined
        }
    };
    Ok(OperatorRoomSummary {
        room_id: summary.room_id().to_string(),
        room_head: room_head(summary.head()),
        pack: PackReference {
            id: summary.pack_revision().pack_id.clone(),
            version: summary.pack_revision().explanatory_version.clone(),
            digest: summary.head().pack_digest().to_string(),
        },
        integrity: OperatorRoomIntegrity {
            status: integrity_status,
            generation: summary.integrity().generation().get(),
        },
        activity_phase: OperatorActivityPhase::Unavailable {
            reason: "operator_membership_required".to_owned(),
        },
        freshness: OperatorDataFreshness::Fresh {
            observed_at: observed_at.to_owned(),
        },
    })
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

fn projection_schema_from_canonical_bytes(bytes: &[u8]) -> Result<String, BackendError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| BackendError::InvalidResult)?;
    value
        .get("projection_schema")
        .and_then(Value::as_str)
        .filter(|schema| !schema.is_empty())
        .map(str::to_owned)
        .ok_or(BackendError::InvalidResult)
}

fn replay_view_bytes(envelope: &CanonicalJsonV1) -> Result<Vec<u8>, BackendError> {
    let bytes = envelope
        .to_bytes()
        .map_err(|_| BackendError::InvalidResult)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| BackendError::InvalidResult)?;
    let projection = value
        .get("activity_projection")
        .ok_or(BackendError::InvalidResult)?;
    CanonicalJsonV1::parse(
        &serde_json::to_vec(projection).map_err(|_| BackendError::InvalidResult)?,
    )
    .map_err(|_| BackendError::InvalidResult)
    .and_then(|value| value.to_bytes().map_err(|_| BackendError::InvalidResult))
}

fn observation_from_payload(bytes: &[u8]) -> Result<(String, Value), BackendError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| BackendError::InvalidResult)?;
    let object = value.as_object().ok_or(BackendError::InvalidResult)?;
    let schema = object
        .get("observation_schema")
        .and_then(Value::as_str)
        .ok_or(BackendError::InvalidResult)?
        .to_owned();
    let mut observation = object
        .get("observation")
        .cloned()
        .ok_or(BackendError::InvalidResult)?;
    if let Some(offers) = object.get("action_offers")
        && !offers.is_null()
    {
        let target = observation
            .as_object_mut()
            .ok_or(BackendError::InvalidResult)?;
        target.insert("action_offers".to_owned(), offers.clone());
    }
    Ok((schema, observation))
}

fn observation_deliver(
    frame: &PostgresFrameEvidenceV1,
    room_id: &RoomId,
    member_id: &worldstream_core::MemberId,
) -> Result<ObservationDeliver, BackendError> {
    let (schema, observation) = observation_from_payload(&frame.payload_bytes)?;
    let mut hash_text = String::from("blake3:");
    for byte in &frame.payload_hash {
        use std::fmt::Write as _;
        let _ = write!(hash_text, "{byte:02x}");
    }
    let hash = worldstream_core::Blake3DigestV1::from_str(&hash_text)
        .map_err(|_| BackendError::InvalidResult)?;
    if hash != worldstream_core::Blake3DigestV1::hash(&frame.payload_bytes) {
        return Err(BackendError::InvalidResult);
    }
    Ok(ObservationDeliver {
        room_id: room_id.to_string(),
        member_id: member_id.to_string(),
        frame_seq: frame.frame_seq,
        cause_room_seq: frame.cause_room_seq,
        frame_kind: "delta".to_owned(),
        observation_schema: schema,
        observation,
        frame_payload_hash: hash.to_string(),
    })
}

fn creation_request(
    authenticated: &worldstream_postgres::PostgresAuthenticatedCapabilityV1,
    request: &CreateRoomRequest,
) -> Result<
    (
        RoomCreationRequestV1,
        worldstream_core::AdministrationOperationIdentityV1,
    ),
    BackendError,
> {
    let pack_digest = request
        .pack
        .digest
        .parse()
        .map_err(|_| BackendError::Rejected)?;
    let configuration = canonical_json(&request.configuration)?;
    let proposals = request
        .members
        .iter()
        .map(|member| {
            InitialMembershipProposalV1::new(
                member
                    .principal_id
                    .parse()
                    .map_err(|_| BackendError::Rejected)?,
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
    CanonicalJsonV1::parse(&serde_json::to_vec(value).map_err(|_| BackendError::Rejected)?)
        .map_err(|_| BackendError::Rejected)
}

fn participant_action_request(
    request: &ActionSubmit,
) -> Result<ParticipantActionRequestV1, BackendError> {
    Ok(ParticipantActionRequestV1::new(
        request
            .room_id
            .parse()
            .map_err(|_| BackendError::Rejected)?,
        request
            .member_id
            .parse()
            .map_err(|_| BackendError::Rejected)?,
        request
            .action_id
            .parse()
            .map_err(|_| BackendError::Rejected)?,
        RoomSequenceV1::new(request.based_on_room_seq).map_err(|_| BackendError::Rejected)?,
        request.action_type.clone(),
        canonical_json(&request.payload)?,
    ))
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
        _ => Err(BackendError::InvalidResult),
    }
}

fn action_reply_from_result(
    request: &ActionSubmit,
    result: &StoredSemanticResultV1,
    duplicate: bool,
) -> Result<ActionReply, BackendError> {
    let admitted_at = match result.semantic_time() {
        worldstream_core::ReceiptSemanticTimeV1::ActionAdmitted(value) => value.as_str().to_owned(),
        _ => return Err(BackendError::InvalidResult),
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
            Ok(ActionReply::Rejected(ActionRejected {
                room_id: request.room_id.clone(),
                member_id: request.member_id.clone(),
                action_id: request.action_id.clone(),
                admitted_at,
                code: code.clone(),
                message: action_rejection_message(code).to_owned(),
                current_room_seq: result
                    .basis_complete_head()
                    .ok_or(BackendError::InvalidResult)?
                    .room_seq()
                    .get(),
                action_offers: Vec::new(),
                retryable_with_same_action_id: false,
                may_submit_revised_action: true,
                duplicate,
                details: serde_json::from_slice(
                    &safe_details
                        .to_bytes()
                        .map_err(|_| BackendError::InvalidResult)?,
                )
                .map_err(|_| BackendError::InvalidResult)?,
            }))
        }
        SemanticResultV1::GenesisCreated { .. } => Err(BackendError::InvalidResult),
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
        _ => Err(BackendError::InvalidResult),
    }
}

fn archive_response_from_result(
    request: &CoreAdministrationRequestV1,
    result: &StoredSemanticResultV1,
    fallback_head: &worldstream_core::CompleteHeadV1,
    duplicate: bool,
) -> Result<RoomArchiveResponseV1, BackendError> {
    let identity = worldstream_core::OperationIdentityV1::Administration(Box::new(
        request.operation_identity().clone(),
    ));
    if result.operation_identity() != &identity
        || result.canonical_request_hash()
            != &request
                .canonical_request_hash()
                .map_err(|_| BackendError::InvalidResult)?
        || result.target_room_id() != request.room_id()
    {
        return Err(BackendError::InvalidResult);
    }
    let head = match result.result() {
        SemanticResultV1::TransitionCommitted { complete_head, .. } => complete_head,
        SemanticResultV1::NoChangeRecorded { .. } => fallback_head,
        _ => return Err(BackendError::InvalidResult),
    };
    Ok(RoomArchiveResponseV1 {
        schema: ROOM_ARCHIVE_RESPONSE_SCHEMA_V1.to_owned(),
        room_id: request.room_id().to_string(),
        room_head: room_head(head),
        duplicate,
    })
}

fn archive_response_from_resolution(
    request: &CoreAdministrationRequestV1,
    resolution: &RoomCommitResolutionV1,
    fallback_head: &worldstream_core::CompleteHeadV1,
) -> Result<RoomArchiveResponseV1, BackendError> {
    if let Some(result) = resolution.stored_result() {
        return archive_response_from_result(
            request,
            result,
            fallback_head,
            resolution.duplicate(),
        );
    }
    match resolution {
        RoomCommitResolutionV1::Conflict { .. } => Err(BackendError::Conflict),
        RoomCommitResolutionV1::Fenced
        | RoomCommitResolutionV1::Reprepare
        | RoomCommitResolutionV1::RetryableKnownAbsent
        | RoomCommitResolutionV1::NotApplicable => Err(BackendError::Busy),
        RoomCommitResolutionV1::Indeterminate => Err(BackendError::Indeterminate),
        _ => Err(BackendError::InvalidResult),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_activation_error(error: PostgresActivationError) -> BackendError {
    match error {
        PostgresActivationError::Authority(error) => map_authority_error(error),
        PostgresActivationError::Connection(_)
        | PostgresActivationError::Sql(_)
        | PostgresActivationError::Corrupt => BackendError::StorageUnavailable,
        PostgresActivationError::InvalidRequest => BackendError::Rejected,
        PostgresActivationError::IdempotencyConflict => BackendError::Conflict,
        PostgresActivationError::Fenced | PostgresActivationError::StaleLease => BackendError::Busy,
        PostgresActivationError::ContextTooLarge => BackendError::Rejected,
    }
}

fn map_postgres_diagnostic_error(error: &PostgresRoomDiagnosticErrorV1) -> BackendError {
    match error {
        PostgresRoomDiagnosticErrorV1::Authority(error) => map_authority_error(*error),
        PostgresRoomDiagnosticErrorV1::RoomUnavailable => BackendError::NotFound,
        PostgresRoomDiagnosticErrorV1::InvalidTarget
        | PostgresRoomDiagnosticErrorV1::InvalidBounds => BackendError::Rejected,
        PostgresRoomDiagnosticErrorV1::StorageUnavailable
        | PostgresRoomDiagnosticErrorV1::Corrupt => BackendError::StorageUnavailable,
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

fn map_participant_action_ingress_error(error: &ParticipantActionIngressErrorV1) -> BackendError {
    match error {
        ParticipantActionIngressErrorV1::Authority(error) => map_authority_error(*error),
        ParticipantActionIngressErrorV1::ResolutionUnavailable => BackendError::Indeterminate,
        ParticipantActionIngressErrorV1::InvalidStoredResult => BackendError::InvalidResult,
    }
}

fn map_admission_lane_error(error: &AdmissionLaneErrorV1) -> BackendError {
    match error {
        AdmissionLaneErrorV1::Full | AdmissionLaneErrorV1::Unavailable => BackendError::Busy,
        AdmissionLaneErrorV1::Clock(_) | AdmissionLaneErrorV1::InvalidCapacity => {
            BackendError::StorageUnavailable
        }
    }
}

fn map_external_input_preparation_error(
    error: PostgresExternalInputPreparationErrorV1,
) -> BackendError {
    match error {
        PostgresExternalInputPreparationErrorV1::Conflict => BackendError::Conflict,
        PostgresExternalInputPreparationErrorV1::StorageUnavailable => {
            BackendError::StorageUnavailable
        }
        PostgresExternalInputPreparationErrorV1::Corrupt => BackendError::InvalidResult,
    }
}

fn map_recovery_error(error: worldstream_core::RoomRecoveryErrorV1) -> BackendError {
    match error {
        worldstream_core::RoomRecoveryErrorV1::IntegrityUnavailable => {
            BackendError::RoomQuarantined
        }
        worldstream_core::RoomRecoveryErrorV1::ConcurrentChange
        | worldstream_core::RoomRecoveryErrorV1::RuntimeUnavailable => BackendError::Busy,
        worldstream_core::RoomRecoveryErrorV1::RuntimeFault
        | worldstream_core::RoomRecoveryErrorV1::Corrupt => BackendError::InvalidResult,
        worldstream_core::RoomRecoveryErrorV1::StorageUnavailable => {
            BackendError::StorageUnavailable
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_replay_error(error: worldstream_postgres::PostgresReplayError) -> BackendError {
    match error {
        worldstream_postgres::PostgresReplayError::Authority
        | worldstream_postgres::PostgresReplayError::Replay(
            HistoricalReplayErrorV1::HistoricalMembershipUnavailable,
        ) => BackendError::Forbidden,
        worldstream_postgres::PostgresReplayError::Replay(
            HistoricalReplayErrorV1::IntegrityUnavailable,
        ) => BackendError::RoomQuarantined,
        worldstream_postgres::PostgresReplayError::Verification
        | worldstream_postgres::PostgresReplayError::Replay(_) => BackendError::StorageUnavailable,
    }
}

fn serving_integrity_status(integrity: &RoomIntegrityStateV1) -> String {
    match integrity.status() {
        RoomIntegrityStatusV1::Healthy => "healthy",
        RoomIntegrityStatusV1::Faulted => "faulted",
        RoomIntegrityStatusV1::Quarantined => "quarantined",
    }
    .to_owned()
}

#[allow(clippy::needless_pass_by_value)]
fn map_serving_fence_error(
    error: worldstream_postgres::PostgresRoomVerificationError,
) -> BackendError {
    match error {
        worldstream_postgres::PostgresRoomVerificationError::MissingRoom { .. } => {
            BackendError::NotFound
        }
        worldstream_postgres::PostgresRoomVerificationError::Connection(_)
        | worldstream_postgres::PostgresRoomVerificationError::Sql(_)
        | worldstream_postgres::PostgresRoomVerificationError::RuntimeUnavailable
        | worldstream_postgres::PostgresRoomVerificationError::RuntimeFault => {
            BackendError::StorageUnavailable
        }
        worldstream_postgres::PostgresRoomVerificationError::InvalidRoomId
        | worldstream_postgres::PostgresRoomVerificationError::Corrupt { .. } => {
            BackendError::InvalidResult
        }
    }
}

fn map_trace_cache_error(error: RoomTraceCacheErrorV1) -> BackendError {
    match error {
        RoomTraceCacheErrorV1::Busy | RoomTraceCacheErrorV1::Poisoned => BackendError::Busy,
        RoomTraceCacheErrorV1::InvalidCapacity => BackendError::StorageUnavailable,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_observation_error(error: PostgresObservationError) -> BackendError {
    match error {
        PostgresObservationError::FutureCursor => BackendError::Rejected,
        PostgresObservationError::Fenced => BackendError::Busy,
        PostgresObservationError::ResetRequired => BackendError::ResetRequired,
        PostgresObservationError::Authority => BackendError::Forbidden,
        PostgresObservationError::Unavailable
        | PostgresObservationError::Corrupt
        | PostgresObservationError::Connection(_)
        | PostgresObservationError::Sql(_)
        | PostgresObservationError::InvalidProjection => BackendError::StorageUnavailable,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_room_commit_error(error: PostgresRoomCommitError) -> BackendError {
    match error {
        PostgresRoomCommitError::Recovery(error) => map_recovery_error(error),
        PostgresRoomCommitError::Preparation => BackendError::InvalidResult,
        PostgresRoomCommitError::Rejected | PostgresRoomCommitError::StaleExternalInputBasis => {
            BackendError::Rejected
        }
    }
}

#[cfg(test)]
#[test]
fn external_input_stale_basis_is_a_refusal_without_masking_invalid_output() {
    assert!(matches!(
        map_room_commit_error(PostgresRoomCommitError::StaleExternalInputBasis),
        BackendError::Rejected
    ));
    assert!(matches!(
        map_room_commit_error(PostgresRoomCommitError::Preparation),
        BackendError::InvalidResult
    ));
}

fn map_session_error(error: SessionErrorV1) -> BackendError {
    match error {
        SessionErrorV1::SlowConsumer => BackendError::Busy,
        _ => BackendError::Rejected,
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

fn next_ulid_string() -> Result<String, BackendError> {
    crate::next_ulid()
        .map(|value| value.to_string())
        .ok_or(BackendError::Indeterminate)
}

fn random_room_seed() -> Result<RoomSeedV1, BackendError> {
    let mut bytes = [0_u8; 32];
    crate::fill_random_bytes(&mut bytes).map_err(|_| BackendError::StorageUnavailable)?;
    let mut text = String::from("hex:");
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
    }
    RoomSeedV1::from_str(&text).map_err(|_| BackendError::InvalidResult)
}

fn random_bearer_bytes() -> Result<[u8; 32], BackendError> {
    let mut bytes = [0_u8; 32];
    fill_random_bytes(&mut bytes).map_err(|_| BackendError::StorageUnavailable)?;
    Ok(bytes)
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

fn protocol_principal_kind(kind: PrincipalKindV1) -> PrincipalKind {
    match kind {
        PrincipalKindV1::Human => PrincipalKind::Human,
        PrincipalKindV1::Agent => PrincipalKind::Agent,
    }
}

fn map_authentication_error(error: PostgresAuthorityAuthenticationError) -> BackendError {
    match error {
        PostgresAuthorityAuthenticationError::Unauthenticated => BackendError::Forbidden,
        PostgresAuthorityAuthenticationError::Unavailable
        | PostgresAuthorityAuthenticationError::Corrupt => BackendError::StorageUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        os::unix::{fs::PermissionsExt, io::AsRawFd},
        sync::Arc,
        thread,
        time::Instant,
    };

    use postgres::{Client, NoTls};
    use serde_json::json;
    use tempfile::tempdir;
    use worldstream_component_host::ComponentPackHostV1;
    use worldstream_pack_bundle::PackBundleVerifierV1;

    use super::{
        ActionReply, ActionSubmit, AttachReply, BackendError, GatewayBackend, GatewaySession,
        MAX_DSN_BYTES, MemberCapabilityIssueRequest, ObservationAck, PostgresGatewayBackend,
        RoomAttach, RoomSyncAck, creation_time, map_admission_lane_error,
        projection_schema_from_canonical_bytes, read_postgres_dsn, recover_attach_cursor,
    };
    use worldstream_core::{
        AdmissionLaneErrorV1, AuthorityBootstrapV1, AuthorityCheckedAt, AuthorityV1,
        CapabilityBearerV1, CapabilityScopeV1, PrincipalKindV1, agent_heist_lobby_digest,
        builtin_agent_heist_registry, builtin_counter_registry, builtin_worldstream_registry,
        counter_v2_digest,
    };
    use worldstream_postgres::{
        PostgresConnectionConfig, PostgresConnectionPath, PostgresObservationDeliveryV1,
        PostgresRoomStore,
    };
    use worldstream_protocol::{
        AccessMode, ActivationClaim, ActivationLeaseOperation, ActivationResultCode, BearerWireV1,
        CreateMember, CreateRoomRequest, LobbyLaunchRequest, MemberCapabilityProvisionRequestV1,
        OperatorActivityPhase, OperatorRoomIntegrityStatus, OperatorRoomInventoryRequest,
        PackReference, PrincipalKind, RunnerCapabilityProvisionRequestV1,
    };
    use worldstream_runtime::SecretSource;

    #[test]
    fn conservative_attach_cursor_installs_current_projection_without_advancing_cursor() {
        let projection = worldstream_core::CanonicalJsonV1::parse(
            br#"{"action_offers":[],"authorized_core":{},"projection":{"value":7},"projection_schema":"counter/projection/v1"}"#,
        )
        .unwrap_or_else(|error| panic!("current projection: {error:?}"));
        let retained = PostgresObservationDeliveryV1::Retained {
            cursor: Some(8),
            cursor_exclusive: 7,
            frame_head: 9,
            retained_floor: 1,
            reset_generation: 3,
            frames: Vec::new(),
        };
        let expected = PostgresObservationDeliveryV1::Reset {
            cursor: Some(8),
            frame_head: 9,
            retained_floor: 1,
            reset_through: None,
            reset_generation: 3,
            projection_bytes: projection
                .to_bytes()
                .unwrap_or_else(|error| panic!("projection bytes: {error:?}")),
        };
        for requested in [None, Some(7)] {
            assert_eq!(
                recover_attach_cursor(retained.clone(), requested, &projection)
                    .unwrap_or_else(|error| panic!("conservative recovery: {error:?}")),
                (expected.clone(), Some("client_cursor_behind")),
            );
        }
        // The storage reader already selects a reset for a missing client Cursor.
        assert_eq!(
            recover_attach_cursor(expected.clone(), None, &projection)
                .unwrap_or_else(|error| panic!("missing cursor recovery: {error:?}")),
            (expected.clone(), Some("client_cursor_behind")),
        );
        let mut first_attach = expected;
        if let PostgresObservationDeliveryV1::Reset { cursor, .. } = &mut first_attach {
            *cursor = None;
        }
        assert_eq!(
            recover_attach_cursor(first_attach.clone(), None, &projection)
                .unwrap_or_else(|error| panic!("first attach: {error:?}")),
            (first_attach, None),
        );
    }

    #[test]
    fn attach_cursor_rejects_unacknowledged_progress_even_below_frame_head() {
        let projection = worldstream_core::CanonicalJsonV1::parse(b"{}")
            .unwrap_or_else(|error| panic!("projection fixture: {error:?}"));
        let retained = PostgresObservationDeliveryV1::Retained {
            cursor: Some(8),
            cursor_exclusive: 8,
            frame_head: 10,
            retained_floor: 1,
            reset_generation: 3,
            frames: Vec::new(),
        };
        assert_eq!(
            recover_attach_cursor(retained.clone(), Some(8), &projection)
                .unwrap_or_else(|error| panic!("exact cursor: {error:?}")),
            (retained.clone(), None),
        );
        for requested in [9, 11] {
            assert!(matches!(
                recover_attach_cursor(retained.clone(), Some(requested), &projection),
                Err(BackendError::Rejected)
            ));
        }
    }

    #[test]
    fn reset_uses_descriptor_projection_schema_from_exact_view() {
        let bytes = br#"{"action_offers":[],"authorized_core":{},"projection":{},"projection_schema":"agent-heist/projection/v1"}"#;
        assert_eq!(
            projection_schema_from_canonical_bytes(bytes)
                .unwrap_or_else(|error| unreachable!("descriptor schema: {error:?}")),
            "agent-heist/projection/v1"
        );
        assert!(matches!(
            projection_schema_from_canonical_bytes(
                br#"{"action_offers":[],"authorized_core":{},"projection":{},"projection_schema":""}"#
            ),
            Err(BackendError::InvalidResult)
        ));
    }
    fn session(value: u8, id: &str) -> GatewaySession {
        let id = id
            .parse()
            .unwrap_or_else(|_| unreachable!("test session id"));
        let wire = BearerWireV1::from_bytes([value; 32]);
        let bearer = CapabilityBearerV1::from_bytes(
            BearerWireV1::parse(&wire.to_wire())
                .unwrap_or_else(|_| unreachable!("test bearer"))
                .into_bytes(),
        );
        GatewaySession::new_with_wire(id, bearer, wire)
    }

    fn archive_registry() -> (
        Arc<worldstream_core::PackRegistryV1>,
        PackReference,
        worldstream_core::PackGoldenCorpusV1,
    ) {
        let path = env::var("WORLDSTREAM_ARCHIVE_CONTRACT_BUNDLE")
            .unwrap_or_else(|error| panic!("archive bundle path: {error}"));
        let bundle = PackBundleVerifierV1
            .inspect(Arc::<[u8]>::from(
                fs::read(&path).unwrap_or_else(|error| panic!("archive bundle: {error}")),
            ))
            .unwrap_or_else(|error| panic!("verify archive bundle: {error}"));
        let corpus = bundle.golden_corpus().clone();
        let pack = PackReference {
            id: bundle.descriptor().pack_id.clone(),
            version: bundle.descriptor().explanatory_version.clone(),
            digest: bundle.revision_digest().to_string(),
        };
        let admission = ComponentPackHostV1::new()
            .and_then(|host| {
                host.admit(
                    bundle,
                    worldstream_core::PackRegistryStatusV1 {
                        selectable_for_new_rooms: true,
                        runnable_for_retained_rooms: true,
                        approved_for_activity_start: true,
                    },
                )
            })
            .unwrap_or_else(|error| panic!("admit archive bundle: {error}"));
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| panic!("counter registry: {error}"))
            .admit_portable([admission])
            .unwrap_or_else(|error| panic!("archive registry: {error}"));
        (Arc::new(registry), pack, corpus)
    }

    #[test]
    fn genesis_creation_time_is_canonical_at_pack_safe_precision() {
        let value = creation_time().unwrap_or_else(|_| unreachable!("Genesis creation clock"));
        assert_eq!(value.as_str().len(), 20);
        assert!(value.as_str().ends_with('Z'));
        assert!(!value.as_str().contains('.'));
    }

    #[test]
    fn runtime_dsn_reader_accepts_bounded_owner_only_file_without_echoing_it() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp directory: {error}"));
        let path = directory.path().join("postgres.dsn");
        let dsn = "host=127.0.0.1 port=55436 user=worldstream password=secret dbname=worldstream";
        fs::write(&path, format!("{dsn}\n"))
            .unwrap_or_else(|error| unreachable!("DSN file: {error}"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| unreachable!("DSN permissions: {error}"));

        let value = read_postgres_dsn(&SecretSource::File(path))
            .unwrap_or_else(|error| unreachable!("owner-only DSN should be readable: {error}"));
        assert_eq!(value, dsn);
    }

    #[test]
    fn runtime_dsn_reader_accepts_duplicated_inherited_descriptor() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp directory: {error}"));
        let path = directory.path().join("postgres-handle.dsn");
        let dsn = "host=127.0.0.1 port=55436 user=worldstream dbname=worldstream";
        fs::write(&path, dsn).unwrap_or_else(|error| unreachable!("DSN file: {error}"));
        let file = fs::File::open(path).unwrap_or_else(|error| unreachable!("DSN open: {error}"));
        let handle = u64::try_from(file.as_raw_fd())
            .unwrap_or_else(|error| unreachable!("descriptor conversion: {error}"));

        let value = read_postgres_dsn(&SecretSource::InheritedHandle(handle))
            .unwrap_or_else(|error| unreachable!("inherited DSN should be readable: {error}"));
        assert_eq!(value, dsn);
    }

    #[test]
    fn facade_retains_runtime_store_and_verification_is_read_only() {
        let config = PostgresConnectionConfig::runtime(
            "host=127.0.0.1 port=1 user=worldstream dbname=worldstream",
            PostgresConnectionPath::Direct,
        )
        .unwrap_or_else(|error| unreachable!("runtime config: {error}"));
        let store = PostgresRoomStore::new(config)
            .unwrap_or_else(|error| unreachable!("runtime store: {error}"));
        let registry = Arc::new(
            builtin_worldstream_registry()
                .unwrap_or_else(|error| unreachable!("test registry: {error}")),
        );
        let backend = PostgresGatewayBackend::new(store, Arc::clone(&registry));
        assert!(Arc::ptr_eq(&registry, &backend.registry));
        assert!(backend.verify_schema().is_err());
        assert_eq!(backend.admission_lanes.capacity(), 256);
        assert!(backend.admission_lanes.host_reserve() > 0);
        assert!(matches!(
            map_admission_lane_error(&AdmissionLaneErrorV1::Full),
            BackendError::Busy
        ));
        assert_eq!(MAX_DSN_BYTES, 16 * 1024);
    }

    #[test]
    fn production_postgres_gateway_has_no_conformance_method_calls() {
        let source = include_str!("postgres_backend.rs");
        for method in [
            "read_observation_conformance",
            "acknowledge_observation_conformance",
            "recover_conformance_trace",
            "commit_conformance_participant_action",
            "commit_conformance_timer_fired",
            "commit_conformance_write_for_conformance",
            "read_activation_receipt_conformance",
        ] {
            assert!(
                !source.contains(&format!(".{method}")),
                "production gateway must not call {method}"
            );
        }
        let unavailable_call = ["Self::", "unavailable()"].concat();
        assert!(!source.contains(&unavailable_call));
        for method in [
            "timer_candidate(",
            "commit_cached_timer(",
            ".reserve_action(&action_room_id, self.host_clock.as_ref())",
            ".reserve_host_stimulus(&room_id)",
            "offer_activations_authorized(",
            "prepare_activation_claim_from_canonical_serving_trace(",
            "claim_activation_authorized(",
            "operate_activation_lease_authorized(",
        ] {
            assert!(
                source.contains(method),
                "production PostgreSQL gateway must call {method}"
            );
        }
        let orphaned_wall_sample = ["ActionAdmittedAt::from_str", "(&now_text()?)"].concat();
        assert!(!source.contains(&orphaned_wall_sample));
    }

    #[test]
    fn counter_canonical_projection_shape_matches_sqlite_gateway_contract() {
        let canonical = br#"{
            "action_offers": [],
            "authorized_core": {"room_status":"active"},
            "projection": {"value": 0}
        }"#;
        let postgres_projection =
            super::projection_from_canonical_bytes(canonical).unwrap_or_else(|error| {
                unreachable!("Counter canonical projection should decode: {error:?}")
            });
        let sqlite_contract = worldstream_protocol::Projection {
            core: serde_json::json!({"room_status":"active"}),
            activity: serde_json::json!({"value": 0}),
            action_offers: Vec::new(),
        };
        assert_eq!(postgres_projection, sqlite_contract);
        assert_eq!(
            serde_json::to_vec(&postgres_projection)
                .unwrap_or_else(|error| { unreachable!("Projection serialization: {error}") }),
            serde_json::to_vec(&sqlite_contract)
                .unwrap_or_else(|error| { unreachable!("Projection serialization: {error}") })
        );
    }

    #[test]
    fn postgres_replay_extracts_core_historical_projection_envelope() {
        let envelope = worldstream_core::CanonicalJsonV1::parse(
            br#"{
                "activity_projection": {
                    "action_offers": [],
                    "authorized_core": {"room_status": "active"},
                    "projection": {"value": 0}
                },
                "envelope": "worldstream/historical-replay-projection/v1"
            }"#,
        )
        .unwrap_or_else(|error| unreachable!("historical envelope: {error}"));
        let extracted = super::replay_view_bytes(&envelope)
            .unwrap_or_else(|error| unreachable!("historical view: {error:?}"));
        assert_eq!(
            super::projection_from_canonical_bytes(&extracted)
                .unwrap_or_else(|error| unreachable!("projection: {error:?}")),
            worldstream_protocol::Projection {
                core: serde_json::json!({"room_status":"active"}),
                activity: serde_json::json!({"value":0}),
                action_offers: Vec::new(),
            }
        );
    }

    #[test]
    #[ignore = "requires WORLDSTREAM_ARCHIVE_CONTRACT_BUNDLE and WORLDSTREAM_POSTGRES_GATEWAY_DSN_FILE"]
    #[allow(clippy::too_many_lines)]
    fn live_postgres_archive_activity_start_is_metadata_derived_and_idempotent() {
        let (Some(bundle_path), Some(dsn_path)) = (
            env::var_os("WORLDSTREAM_ARCHIVE_CONTRACT_BUNDLE"),
            env::var_os("WORLDSTREAM_POSTGRES_GATEWAY_DSN_FILE"),
        ) else {
            return;
        };
        let dsn = read_postgres_dsn(&SecretSource::File(dsn_path.into()))
            .unwrap_or_else(|error| panic!("live runtime DSN file: {error}"));
        let config = PostgresConnectionConfig::runtime(dsn, PostgresConnectionPath::Direct)
            .unwrap_or_else(|error| panic!("runtime config: {error}"));
        let store =
            PostgresRoomStore::new(config).unwrap_or_else(|error| panic!("runtime store: {error}"));
        let (registry, pack, corpus) = archive_registry();
        let backend = Arc::new(PostgresGatewayBackend::new(store, Arc::clone(&registry)));
        backend
            .verify_schema()
            .unwrap_or_else(|error| panic!("verified schema: {error}"));

        let host_bearer = CapabilityBearerV1::from_bytes([0xd1; 32]);
        let host = session(0xd1, "01ARZ3NDEKTSV4RRFFQ69G5H20");
        AuthorityV1::new(backend.store.clone())
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5H21".parse().unwrap(),
                    "01ARZ3NDEKTSV4RRFFQ69G5H22".parse().unwrap(),
                    PrincipalKindV1::Human,
                    "01ARZ3NDEKTSV4RRFFQ69G5H23".parse().unwrap(),
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap(),
                "2026-08-15T12:00:00Z".parse().unwrap(),
            )
            .unwrap_or_else(|error| panic!("bootstrap authority: {error}"));

        let room = backend
            .create_room(
                &host,
                CreateRoomRequest {
                    pack: pack.clone(),
                    configuration: serde_json::to_value(&corpus.genesis.configuration).unwrap(),
                    members: vec![
                        CreateMember {
                            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5H24".to_owned(),
                            principal_kind: PrincipalKind::Human,
                            role: Some("lead".to_owned()),
                            access_mode: AccessMode::Participant,
                        },
                        CreateMember {
                            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5H25".to_owned(),
                            principal_kind: PrincipalKind::Agent,
                            role: Some("mira".to_owned()),
                            access_mode: AccessMode::Participant,
                        },
                        CreateMember {
                            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5H26".to_owned(),
                            principal_kind: PrincipalKind::Agent,
                            role: Some("jonah".to_owned()),
                            access_mode: AccessMode::Participant,
                        },
                    ],
                    idempotency_key: "archive-postgres-activity-start-room".to_owned(),
                },
            )
            .unwrap_or_else(|error| panic!("create archive Room: {error}"));
        let request = LobbyLaunchRequest {
            input_id: "01ARZ3NDEKTSV4RRFFQ69G5H27".to_owned(),
            based_on_room_seq: 0,
            pack_digest: Some(pack.digest.clone()),
        };
        let before_unauthorized = backend
            .operator_room_detail(&host, &room.room_id)
            .unwrap_or_else(|error| {
                panic!("read archive Room before unauthorized request: {error}")
            })
            .room_head;
        let non_operator = session(0xd2, "01ARZ3NDEKTSV4RRFFQ69G5H29");
        assert!(matches!(
            backend.launch_lobby(&non_operator, &room.room_id, request.clone()),
            Err(BackendError::Forbidden)
        ));
        assert!(matches!(
            backend.resolve_lobby_launch(&non_operator, &room.room_id, request.clone()),
            Err(BackendError::Forbidden)
        ));
        let after_unauthorized = backend
            .operator_room_detail(&host, &room.room_id)
            .unwrap_or_else(|error| panic!("read archive Room after unauthorized request: {error}"))
            .room_head;
        assert_eq!(after_unauthorized, before_unauthorized);
        assert!(
            backend
                .resolve_lobby_launch(&host, &room.room_id, request.clone())
                .unwrap()
                .is_none()
        );
        let concurrent = (0..4)
            .map(|index| {
                let backend = Arc::clone(&backend);
                let room_id = room.room_id.clone();
                let request = request.clone();
                thread::spawn(move || {
                    backend.launch_lobby(
                        &session(0xd1, &format!("01ARZ3NDEKTSV4RRFFQ69G5H3{index}")),
                        &room_id,
                        request,
                    )
                })
            })
            .collect::<Vec<_>>();
        let mut successful = Vec::new();
        for result in concurrent {
            match result
                .join()
                .unwrap_or_else(|_| panic!("concurrent PostgreSQL start panicked"))
            {
                Ok(response) => successful.push(response),
                Err(BackendError::Busy | BackendError::WrongPhase) => {}
                Err(error) => panic!("concurrent PostgreSQL start failed unexpectedly: {error}"),
            }
        }
        let first = successful
            .iter()
            .find(|response| !response.duplicate)
            .unwrap_or_else(|| panic!("one PostgreSQL start must commit"));
        assert_eq!(
            successful
                .iter()
                .filter(|response| !response.duplicate)
                .count(),
            1
        );
        assert_eq!(first.room_head.room_seq, 1);
        let duplicate = backend
            .resolve_lobby_launch(&host, &room.room_id, request.clone())
            .unwrap()
            .unwrap();
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.transition_id, first.transition_id);
        assert_eq!(duplicate.room_head, first.room_head);
        let before = backend
            .operator_room_detail(&host, &room.room_id)
            .unwrap()
            .room_head;
        assert!(matches!(
            backend.launch_lobby(
                &host,
                &room.room_id,
                LobbyLaunchRequest {
                    input_id: "01ARZ3NDEKTSV4RRFFQ69G5H28".to_owned(),
                    based_on_room_seq: 1,
                    pack_digest: Some(pack.digest.clone()),
                },
            ),
            Err(BackendError::WrongPhase)
        ));
        let after = backend
            .operator_room_detail(&host, &room.room_id)
            .unwrap()
            .room_head;
        assert_eq!(after, before);
        assert!(
            !bundle_path.is_empty(),
            "bundle path is part of the test contract"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::unwrap_used)]
    fn live_postgres_shared_batch_uses_production_adapter() {
        let Ok(dsn) = env::var("WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN") else {
            return;
        };
        for format in [
            worldstream_core::CanonicalHistoryFormat::V1,
            worldstream_core::CanonicalHistoryFormat::V2,
        ] {
            // Uses the public P2 host installed by the isolated page fixture.
            // Every Room, issued capability, operation, and Session is fresh.
            let store = PostgresRoomStore::new(
                PostgresConnectionConfig::runtime(dsn.clone(), PostgresConnectionPath::Direct)
                    .unwrap(),
            )
            .unwrap();
            let backend = PostgresGatewayBackend::new(
                store,
                Arc::new(builtin_worldstream_registry().unwrap()),
            );
            let host = session(0x72, crate::next_ulid().unwrap().as_str());
            let principal = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
            let counter = backend
                .registry
                .load_retained(&counter_v2_digest())
                .unwrap();
            let request = CreateRoomRequest {
                pack: PackReference {
                    id: counter.descriptor().pack_id.clone(),
                    version: counter.descriptor().explanatory_version.clone(),
                    digest: counter_v2_digest().to_string(),
                },
                configuration: json!({"initial_value":0,"maximum_value":16}),
                members: vec![worldstream_protocol::CreateMember {
                    principal_id: principal.into(),
                    principal_kind: PrincipalKind::Human,
                    role: Some("counter".into()),
                    access_mode: AccessMode::Participant,
                }],
                idempotency_key: crate::next_ulid().unwrap().to_string(),
            };
            let selection = worldstream_protocol::CreateRoomRequestWithFormat {
                request: request.clone(),
                canonical_history_format: match format {
                    worldstream_core::CanonicalHistoryFormat::V1 => {
                        worldstream_protocol::CanonicalHistoryFormatV1::V1
                    }
                    worldstream_core::CanonicalHistoryFormat::V2 => {
                        worldstream_protocol::CanonicalHistoryFormatV1::V2
                    }
                },
            };
            let created = backend
                .create_room_with_format(&host, selection.clone())
                .unwrap();
            let retry = backend.create_room_with_format(&host, selection).unwrap();
            assert_eq!(
                serde_json::to_vec(&created).unwrap(),
                serde_json::to_vec(&retry).unwrap()
            );
            if format == worldstream_core::CanonicalHistoryFormat::V1 {
                assert_eq!(
                    backend.create_room(&host, request.clone()).unwrap(),
                    created
                );
            }
            let other_selection = worldstream_protocol::CreateRoomRequestWithFormat {
                request: request.clone(),
                canonical_history_format: match format {
                    worldstream_core::CanonicalHistoryFormat::V1 => {
                        worldstream_protocol::CanonicalHistoryFormatV1::V2
                    }
                    worldstream_core::CanonicalHistoryFormat::V2 => {
                        worldstream_protocol::CanonicalHistoryFormatV1::V1
                    }
                },
            };
            assert!(matches!(
                backend.create_room_with_format(&host, other_selection),
                Err(BackendError::Conflict)
            ));
            let room = created.room_id;
            let member = created.member_ids[0].clone();
            let mut sessions = Vec::new();
            let mut capabilities = Vec::new();
            for _ in 0..2 {
                let issued = backend
                    .issue_member_capability(
                        &host,
                        MemberCapabilityIssueRequest {
                            room_id: room.clone(),
                            member_id: member.clone(),
                            principal_id: principal.into(),
                            scopes: vec![
                                CapabilityScopeV1::RoomAttach,
                                CapabilityScopeV1::RoomObserveMember,
                                CapabilityScopeV1::RoomAct,
                            ],
                            idempotency_key: crate::next_ulid().unwrap().to_string(),
                            expires_at: None,
                        },
                    )
                    .unwrap();
                let peer = Arc::new(GatewaySession::new_with_wire(
                    crate::next_ulid().unwrap(),
                    CapabilityBearerV1::from_bytes(
                        BearerWireV1::parse(&issued.bearer).unwrap().into_bytes(),
                    ),
                    BearerWireV1::parse(&issued.bearer).unwrap(),
                ));
                let attached = backend
                    .attach(
                        &peer,
                        RoomAttach {
                            room_id: room.clone(),
                            member_id: member.clone(),
                            after_frame_seq: None,
                        },
                    )
                    .unwrap();
                backend
                    .sync_ack(
                        &peer,
                        RoomSyncAck {
                            room_id: room.clone(),
                            member_id: member.clone(),
                            through_frame_head: attached.attached.frame_head,
                            sync_token: attached.attached.sync_token,
                        },
                    )
                    .unwrap();
                capabilities.push(issued.capability_id);
                sessions.push(peer);
            }
            backend.forbid_recovery_for_test();
            backend
                .action(
                    &sessions[0],
                    ActionSubmit {
                        room_id: room.clone(),
                        member_id: member.clone(),
                        action_id: crate::next_ulid().unwrap().to_string(),
                        based_on_room_seq: 0,
                        action_type: "increment".into(),
                        payload: json!({}),
                    },
                )
                .unwrap();
            backend
                .traces
                .with_room(&room.parse().unwrap(), |slot| {
                    assert!(slot.as_ref().unwrap().trace().transitions().is_empty());
                    assert_eq!(slot.as_ref().unwrap().trace().head().room_seq().get(), 1);
                })
                .unwrap();
            let recipients: Vec<_> = sessions
                .iter()
                .map(|session| crate::LiveObservationRecipient {
                    session: Arc::clone(session),
                    room_id: room.clone(),
                    member_id: member.clone(),
                    after_frame_seq: 0,
                })
                .collect();
            let reads = backend
                .live_payload_page_reads
                .load(std::sync::atomic::Ordering::Relaxed);
            let pages = backend.prepare_live_observation_batch(&recipients, 32, 1024 * 1024);
            assert_eq!(
                backend
                    .live_payload_page_reads
                    .load(std::sync::atomic::Ordering::Relaxed)
                    - reads,
                1
            );
            // The actual production batch (including its independent fallback)
            // must terminate while an unrelated transaction retains the Room lock.
            let mut blocker = postgres::Client::connect(&dsn, postgres::NoTls).unwrap();
            let cursor_before: Option<i64> = blocker.query_one(
                "SELECT last_ack_frame_seq FROM worldstream_members WHERE room_id=$1 AND member_id=$2",
                &[&room, &member],
            ).unwrap().get(0);
            let mut held = blocker.transaction().unwrap();
            held.query_one(
                "SELECT room_id FROM worldstream_room_roots WHERE room_id=$1 FOR UPDATE",
                &[&room],
            )
            .unwrap();
            let started = std::time::Instant::now();
            let blocked = worldstream_core::with_storage_read_deadline(
                started + std::time::Duration::from_millis(100),
                || backend.prepare_live_observation_batch(&recipients, 32, 1024 * 1024),
            );
            assert!(blocked.iter().all(Result::is_err));
            assert!(started.elapsed() < std::time::Duration::from_millis(600));
            let fenced = worldstream_core::with_storage_read_deadline(
                std::time::Instant::now() + std::time::Duration::from_millis(100),
                || {
                    pages[0]
                        .as_ref()
                        .unwrap()
                        .fence
                        .as_ref()
                        .unwrap()
                        .revalidate(&sessions[0])
                },
            );
            assert!(matches!(fenced, Err(BackendError::StorageUnavailable)));
            held.rollback().unwrap();
            let mut held_authority = blocker.transaction().unwrap();
            held_authority.query_one(
                "SELECT capability_id FROM worldstream_authority_capabilities WHERE capability_id=$1 FOR UPDATE",
                &[&capabilities[0]],
            ).unwrap();
            let authority_wait = worldstream_core::with_storage_read_deadline(
                std::time::Instant::now() + std::time::Duration::from_millis(100),
                || {
                    pages[0]
                        .as_ref()
                        .unwrap()
                        .fence
                        .as_ref()
                        .unwrap()
                        .revalidate(&sessions[0])
                },
            );
            // A canceled authority SQL read is operational unavailability,
            // never a permanent authority rejection or lineage quarantine.
            assert!(matches!(
                authority_wait,
                Err(BackendError::StorageUnavailable)
            ));
            held_authority.rollback().unwrap();
            let cursor_after: Option<i64> = blocker.query_one(
                "SELECT last_ack_frame_seq FROM worldstream_members WHERE room_id=$1 AND member_id=$2",
                &[&room, &member],
            ).unwrap().get(0);
            assert_eq!(cursor_after, cursor_before);
            let restored = worldstream_core::with_storage_read_deadline(
                std::time::Instant::now() + std::time::Duration::from_millis(750),
                || backend.prepare_live_observation_batch(&recipients, 32, 1024 * 1024),
            );
            assert_eq!(restored[0].as_ref().unwrap().frames.len(), 1);
            assert_eq!(restored[1].as_ref().unwrap().frames.len(), 1);
            eprintln!(
                "LIVE_PUBLICATION_ROOM_LOCK_DEADLINE=PASS format={format:?} blocked_then_retry_cursor_unchanged"
            );
            let first = pages[0].as_ref().unwrap();
            let second = pages[1].as_ref().unwrap();
            assert!(Arc::ptr_eq(&first.frames, &second.frames));
            assert_eq!(first.frames.len(), 1);
            first
                .fence
                .as_ref()
                .unwrap()
                .revalidate(&sessions[0])
                .unwrap();
            second
                .fence
                .as_ref()
                .unwrap()
                .revalidate(&sessions[1])
                .unwrap();
            backend
                .action(
                    &sessions[0],
                    ActionSubmit {
                        room_id: room.clone(),
                        member_id: member.clone(),
                        action_id: crate::next_ulid().unwrap().to_string(),
                        based_on_room_seq: 1,
                        action_type: "increment".into(),
                        payload: json!({}),
                    },
                )
                .unwrap();
            first
                .fence
                .as_ref()
                .unwrap()
                .revalidate(&sessions[0])
                .unwrap();
            second
                .fence
                .as_ref()
                .unwrap()
                .revalidate(&sessions[1])
                .unwrap();
            let positions = [
                recipients[0].clone(),
                crate::LiveObservationRecipient {
                    after_frame_seq: 1,
                    ..recipients[1].clone()
                },
            ];
            let position_pages =
                backend.prepare_live_observation_batch(&positions, 32, 1024 * 1024);
            assert_eq!(position_pages[0].as_ref().unwrap().frames.len(), 2);
            assert_eq!(position_pages[1].as_ref().unwrap().frames.len(), 1);
            assert_eq!(
                backend
                    .observation_ack(
                        &sessions[0],
                        ObservationAck {
                            room_id: room.clone(),
                            member_id: member.clone(),
                            through_frame_seq: 2
                        }
                    )
                    .unwrap(),
                Some(2)
            );
            let reattach = backend
                .attach(
                    &sessions[1],
                    RoomAttach {
                        room_id: room.clone(),
                        member_id: member.clone(),
                        after_frame_seq: Some(2),
                    },
                )
                .unwrap();
            assert_eq!(reattach.attached.cursor, Some(2));
            assert!(
                second
                    .fence
                    .as_ref()
                    .unwrap()
                    .revalidate(&sessions[1])
                    .is_err()
            );
            first
                .fence
                .as_ref()
                .unwrap()
                .revalidate(&sessions[0])
                .unwrap();
            backend
                .sync_ack(
                    &sessions[1],
                    RoomSyncAck {
                        room_id: room.clone(),
                        member_id: member.clone(),
                        through_frame_head: reattach.attached.frame_head,
                        sync_token: reattach.attached.sync_token,
                    },
                )
                .unwrap();
            let authenticated = backend.authenticate(&host).unwrap();
            backend
                .authority()
                .change(
                    &authenticated.into_presented(),
                    worldstream_core::AuthorityChangeV1::RevokeCapability {
                        change_id: crate::next_ulid().unwrap().as_str().parse().unwrap(),
                        capability_id: capabilities[0].parse().unwrap(),
                        expected_generation: worldstream_core::AuthorityGenerationV1::new(1)
                            .unwrap(),
                        reason_code: worldstream_core::AuthorityReasonCodeV1::new(
                            "shared_adapter_test",
                        )
                        .unwrap(),
                    },
                    backend.checked_at().unwrap(),
                )
                .unwrap();
            assert!(
                first
                    .fence
                    .as_ref()
                    .unwrap()
                    .revalidate(&sessions[0])
                    .is_err()
            );
            second
                .fence
                .as_ref()
                .unwrap()
                .revalidate(&sessions[1])
                .unwrap();
            let isolated = backend.prepare_live_observation_batch(&recipients, 32, 1024 * 1024);
            assert!(isolated[0].is_err());
            assert_eq!(isolated[1].as_ref().unwrap().frames.len(), 2);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn live_postgres_gateway_counter_workflow_uses_production_backend() {
        let Some(path) = env::var_os("WORLDSTREAM_POSTGRES_GATEWAY_DSN_FILE") else {
            return;
        };
        let dsn = read_postgres_dsn(&SecretSource::File(path.into()))
            .unwrap_or_else(|error| unreachable!("live runtime DSN file: {error}"));
        let connection_path = match env::var("WORLDSTREAM_POSTGRES_GATEWAY_CONNECTION_PATH")
            .ok()
            .as_deref()
        {
            None | Some("direct") => PostgresConnectionPath::Direct,
            Some("transaction_pool") => PostgresConnectionPath::TransactionPool,
            Some(other) => unreachable!("unsupported live gateway connection path: {other}"),
        };
        let config = PostgresConnectionConfig::runtime(dsn.clone(), connection_path)
            .unwrap_or_else(|error| unreachable!("runtime config: {error}"));
        let store = PostgresRoomStore::new(config)
            .unwrap_or_else(|error| unreachable!("runtime store: {error}"));
        let registry = Arc::new(
            builtin_worldstream_registry()
                .unwrap_or_else(|error| unreachable!("test registry: {error}")),
        );
        // The test later uses a counter-only registry to derive the first Room
        // fixture. Keep the production registry for restart: durable Lobby
        // receipts must be rendered using their retained Agent Heist Pack.
        let restart_registry = Arc::clone(&registry);
        let backend = PostgresGatewayBackend::new(store, Arc::clone(&registry));
        backend
            .verify_schema()
            .unwrap_or_else(|error| unreachable!("verified schema: {error}"));

        let authority = AuthorityV1::new(backend.store.clone());
        let host_bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        let host_principal = "01ARZ3NDEKTSV4RRFFQ69G5FC2"
            .parse::<worldstream_core::PrincipalId>()
            .unwrap_or_else(|_| unreachable!("host principal"));
        let participant_principal = "01ARZ3NDEKTSV4RRFFQ69G5FC6"
            .parse::<worldstream_core::PrincipalId>()
            .unwrap_or_else(|_| unreachable!("participant principal"));
        authority
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FC4"
                        .parse()
                        .unwrap_or_else(|_| unreachable!("bootstrap change")),
                    host_principal.clone(),
                    PrincipalKindV1::Human,
                    "01ARZ3NDEKTSV4RRFFQ69G5FC3"
                        .parse()
                        .unwrap_or_else(|_| unreachable!("host capability")),
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|_| unreachable!("bootstrap request")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|_| unreachable!("bootstrap time")),
            )
            .unwrap_or_else(|error| unreachable!("bootstrap authority: {error}"));

        let registry = Arc::new(
            builtin_counter_registry()
                .unwrap_or_else(|error| unreachable!("Counter registry: {error}")),
        );
        let descriptor = registry
            .load_retained(&counter_v2_digest())
            .unwrap_or_else(|error| unreachable!("Counter pack: {error}"))
            .descriptor()
            .clone();
        let create = worldstream_protocol::CreateRoomRequest {
            pack: PackReference {
                id: descriptor.pack_id,
                version: descriptor.explanatory_version,
                digest: counter_v2_digest().to_string(),
            },
            configuration: json!({"initial_value": 0, "maximum_value": 16}),
            members: vec![
                CreateMember {
                    principal_id: participant_principal.to_string(),
                    principal_kind: PrincipalKind::Agent,
                    role: Some("counter".to_owned()),
                    access_mode: AccessMode::Participant,
                },
                CreateMember {
                    principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC7".to_owned(),
                    principal_kind: PrincipalKind::Human,
                    role: None,
                    access_mode: AccessMode::Spectator,
                },
            ],
            idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FD0".to_owned(), // gitleaks:allow - public fixture ULID
        };
        let host = session(0xa9, "01ARZ3NDEKTSV4RRFFQ69G5FC5");
        let created = backend
            .create_room(&host, create.clone())
            .unwrap_or_else(|error| unreachable!("create Room: {error:?}"));
        let inventory = backend
            .operator_room_inventory(
                &host,
                OperatorRoomInventoryRequest {
                    after_room_id: None,
                    limit: 100,
                },
            )
            .unwrap_or_else(|error| unreachable!("operator inventory: {error:?}"));
        assert!(
            inventory
                .rooms
                .iter()
                .any(|summary| summary.room_id == created.room_id)
        );
        let detail = backend
            .operator_room_detail(&host, &created.room_id)
            .unwrap_or_else(|error| unreachable!("operator detail: {error:?}"));
        assert_eq!(detail.room_head, created.room_head);
        assert_eq!(
            detail.integrity.status,
            OperatorRoomIntegrityStatus::Healthy
        );
        assert!(matches!(
            detail.activity_phase,
            OperatorActivityPhase::Unavailable { .. }
        ));
        assert_eq!(
            backend
                .create_room(&host, create.clone())
                .unwrap_or_else(|error| unreachable!("duplicate create: {error:?}")),
            created
        );
        let mut conflicting_create = create;
        conflicting_create.configuration = json!({"initial_value": 1, "maximum_value": 16});
        assert!(matches!(
            backend.create_room(&host, conflicting_create),
            Err(BackendError::Conflict)
        ));

        let room_id = created.room_id.clone();
        let member_id = created.member_ids[0].clone();
        let other_member_id = created.member_ids[1].clone();
        let issued = backend
            .issue_member_capability(
                &host,
                MemberCapabilityIssueRequest {
                    room_id: room_id.clone(),
                    member_id: member_id.clone(),
                    principal_id: participant_principal.to_string(),
                    scopes: vec![
                        CapabilityScopeV1::RoomAttach,
                        CapabilityScopeV1::RoomObserveMember,
                        CapabilityScopeV1::RoomAct,
                        CapabilityScopeV1::RoomReplay,
                    ],
                    idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FC8".to_owned(), // gitleaks:allow - public fixture ULID
                    expires_at: None,
                },
            )
            .unwrap_or_else(|error| unreachable!("member capability: {error:?}"));
        let sealed_member = json!({
            "room_id": room_id,
            "member_id": member_id,
            "principal_id": participant_principal.to_string(),
            "principal_kind": "agent",
            "role": "counter",
            "access_mode": "participant",
            "scopes": ["room:attach", "room:act", "room:observe_member"],
            "capability": {
                "capability_id": "01ARZ3NDEKTSV4RRFFQ69G5FF0",
                "capability_idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FF1",
                "bearer": BearerWireV1::from_bytes([0xc1; 32]).to_wire()
            }
        });
        let sealed_member_first = backend
            .provision_member_capability(
                &host,
                serde_json::from_value::<MemberCapabilityProvisionRequestV1>(sealed_member.clone())
                    .unwrap_or_else(|error| unreachable!("sealed member request: {error}")),
            )
            .unwrap_or_else(|error| unreachable!("sealed member provision: {error:?}"));
        let sealed_runner = json!({
            "runner_id": "01ARZ3NDEKTSV4RRFFQ69G5FF2",
            "owner_principal_id": participant_principal.to_string(),
            "permitted_memberships": [{ "room_id": room_id, "member_id": member_id }],
            "scopes": ["activation:offer_receive", "activation:claim", "activation:complete"],
            "principal_idempotency_key": participant_principal.to_string(),
            "runner_idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FF3",
            "capability": {
                "capability_id": "01ARZ3NDEKTSV4RRFFQ69G5FF4",
                "capability_idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FF5",
                "bearer": BearerWireV1::from_bytes([0xc2; 32]).to_wire()
            }
        });
        let sealed_runner_first = backend
            .provision_runner_capability(
                &host,
                serde_json::from_value::<RunnerCapabilityProvisionRequestV1>(sealed_runner.clone())
                    .unwrap_or_else(|error| unreachable!("sealed runner request: {error}")),
            )
            .unwrap_or_else(|error| unreachable!("sealed runner provision: {error:?}"));
        let member_wire =
            BearerWireV1::parse(&issued.bearer).unwrap_or_else(|_| unreachable!("issued bearer"));
        let member = GatewaySession::new_with_wire(
            "01ARZ3NDEKTSV4RRFFQ69G5FC7"
                .parse()
                .unwrap_or_else(|_| unreachable!("member session")),
            CapabilityBearerV1::from_bytes(
                BearerWireV1::parse(&issued.bearer)
                    .unwrap_or_else(|_| unreachable!("issued bearer"))
                    .into_bytes(),
            ),
            member_wire,
        );

        let projection_zero = backend
            .projection(&member, &room_id)
            .unwrap_or_else(|error| unreachable!("projection: {error:?}"));
        let replay_zero = backend
            .replay(&member, &room_id, 0)
            .unwrap_or_else(|error| unreachable!("Replay: {error:?}"));
        assert_eq!(projection_zero.room_head, replay_zero.room_head);
        let attached: AttachReply = backend
            .attach(
                &member,
                RoomAttach {
                    room_id: room_id.clone(),
                    member_id: member_id.clone(),
                    after_frame_seq: None,
                },
            )
            .unwrap_or_else(|error| unreachable!("attach: {error:?}"));
        assert_eq!(attached.attached.cursor, None);
        assert!(matches!(
            &attached.attached.sync,
            worldstream_protocol::SyncBranch::ProjectionReset { reason, .. }
                if reason == "first_attach"
        ));
        assert_eq!(
            attached
                .reset
                .as_ref()
                .map(|reset| reset.reset_reason.as_str()),
            Some("first_attach")
        );
        let baseline = attached.attached.frame_head;
        let frames = backend
            .sync_ack(
                &member,
                RoomSyncAck {
                    room_id: room_id.clone(),
                    member_id: member_id.clone(),
                    through_frame_head: baseline,
                    sync_token: attached.attached.sync_token,
                },
            )
            .unwrap_or_else(|error| unreachable!("sync ack: {error:?}"));
        assert!(frames.is_empty());
        assert_eq!(
            backend
                .observation_ack(
                    &member,
                    ObservationAck {
                        room_id: room_id.clone(),
                        member_id: member_id.clone(),
                        through_frame_seq: 0,
                    },
                )
                .unwrap_or_else(|error| unreachable!("zero observation ack: {error:?}")),
            None
        );

        let action = ActionSubmit {
            room_id: room_id.clone(),
            member_id: member_id.clone(),
            action_id: "01ARZ3NDEKTSV4RRFFQ69G5FD1".to_owned(),
            based_on_room_seq: 0,
            action_type: "increment".to_owned(),
            payload: json!({}),
        };
        let ActionReply::Accepted(first) = backend
            .action(&member, action.clone())
            .unwrap_or_else(|error| unreachable!("Action: {error:?}"))
        else {
            unreachable!("authorized Action rejected")
        };
        assert_eq!(first.room_head.room_seq, 1);

        let mut status_client = Client::connect(&dsn, NoTls)
            .unwrap_or_else(|error| unreachable!("activation status fixture connection: {error}"));
        let mut transaction = status_client
            .transaction()
            .unwrap_or_else(|error| unreachable!("activation status fixture transaction: {error}"));
        for (suffix, target, state) in [
            ("waiting-a", member_id.as_str(), "pending"),
            ("waiting-b", member_id.as_str(), "pending"),
            ("leased", member_id.as_str(), "leased"),
            ("completed", member_id.as_str(), "completed"),
            ("expired", member_id.as_str(), "expired"),
            ("cancelled", member_id.as_str(), "cancelled"),
            ("other-member", other_member_id.as_str(), "pending"),
        ] {
            let leased = state == "leased";
            transaction
                .execute(
                    "INSERT INTO worldstream_activation_intents(\
                     activation_id, room_id, cause_room_seq, decision_id, target_member_id,\
                     reason_code, deduplication_key, priority, policy_revision, state,\
                     intent_generation, lease_generation, runner_id, claim_id, lease_until)\
                     VALUES ($1, $2, 1, $3, $4, 'bounded', $5, 1, 1, $6, 1, $7, $8, $9, $10)\
                     ON CONFLICT (activation_id) DO NOTHING",
                    &[
                        &format!("operator-status-{suffix}"),
                        &room_id,
                        &format!("operator-status-decision-{suffix}"),
                        &target,
                        &format!("operator-status-dedup-{suffix}"),
                        &state,
                        &i64::from(leased),
                        &leased.then_some("01ARZ3NDEKTSV4RRFFQ69G5FF2"),
                        &leased.then_some("operator-status-claim"),
                        &leased.then_some("2026-08-15T12:05:00Z"),
                    ],
                )
                .unwrap_or_else(|error| unreachable!("seed {state} Activation: {error}"));
        }
        transaction
            .commit()
            .unwrap_or_else(|error| unreachable!("publish activation status fixture: {error}"));
        let status = backend
            .operator_activation_status(&host, &room_id, &member_id)
            .unwrap_or_else(|error| unreachable!("bounded activation status: {error:?}"));
        assert_eq!((status.waiting, status.leased), (2, 1));
        let isolated = backend
            .operator_activation_status(&host, &room_id, &other_member_id)
            .unwrap_or_else(|error| unreachable!("isolated activation status: {error:?}"));
        assert_eq!((isolated.waiting, isolated.leased), (1, 0));
        assert!(matches!(
            backend.operator_activation_status(&host, &room_id, "01ARZ3NDEKTSV4RRFFQ69G5FQ9"),
            Err(BackendError::NotFound)
        ));
        assert!(matches!(
            backend.operator_activation_status(&member, &room_id, &member_id),
            Err(BackendError::Forbidden)
        ));
        println!(
            "LIVE_POSTGRES=PASS operator_activation_status=pending+leased+terminal-filter+member-isolation+authorization"
        );
        let live = backend
            .live_observation_suffix(&member, &room_id, &member_id, baseline)
            .unwrap_or_else(|error| unreachable!("live suffix: {error:?}"));
        assert_eq!(live.len(), 1);
        let frame_seq = live[0].frame_seq;
        assert_eq!(
            backend
                .observation_ack(
                    &member,
                    ObservationAck {
                        room_id: room_id.clone(),
                        member_id: member_id.clone(),
                        through_frame_seq: frame_seq,
                    },
                )
                .unwrap_or_else(|error| unreachable!("observation ack: {error:?}")),
            Some(frame_seq)
        );
        let reattached = backend
            .attach(
                &member,
                RoomAttach {
                    room_id: room_id.clone(),
                    member_id: member_id.clone(),
                    after_frame_seq: Some(frame_seq),
                },
            )
            .unwrap_or_else(|error| unreachable!("cursor reattach: {error:?}"));
        assert_eq!(reattached.attached.cursor, Some(frame_seq));
        assert!(matches!(
            &reattached.attached.sync,
            worldstream_protocol::SyncBranch::RetainedFrames {
                cursor_exclusive,
                through_frame_head,
            } if *cursor_exclusive == frame_seq && *through_frame_head == frame_seq
        ));
        let reattach_frames = backend
            .sync_ack(
                &member,
                RoomSyncAck {
                    room_id: room_id.clone(),
                    member_id: member_id.clone(),
                    through_frame_head: frame_seq,
                    sync_token: reattached.attached.sync_token,
                },
            )
            .unwrap_or_else(|error| unreachable!("cursor resync: {error:?}"));
        assert!(reattach_frames.is_empty());

        for after_frame_seq in [None, Some(frame_seq.saturating_sub(1))] {
            let recovered = backend
                .attach(
                    &member,
                    RoomAttach {
                        room_id: room_id.clone(),
                        member_id: member_id.clone(),
                        after_frame_seq,
                    },
                )
                .unwrap_or_else(|error| panic!("conservative cursor attach: {error:?}"));
            assert_eq!(recovered.attached.cursor, Some(frame_seq));
            assert_eq!(recovered.attached.room_head, reattached.attached.room_head);
            assert!(matches!(
                &recovered.attached.sync,
                worldstream_protocol::SyncBranch::ProjectionReset {
                    baseline_frame_head,
                    reason,
                } if *baseline_frame_head == frame_seq && reason == "client_cursor_behind"
            ));
            assert!(recovered.frames.is_empty());
            let reset = recovered
                .reset
                .unwrap_or_else(|| panic!("conservative attach requires current Projection"));
            assert_eq!(reset.baseline_frame_head, frame_seq);
            assert_eq!(reset.room_head, recovered.attached.room_head);
            assert_eq!(reset.reset_reason, "client_cursor_behind");
            assert!(
                backend
                    .sync_ack(
                        &member,
                        RoomSyncAck {
                            room_id: room_id.clone(),
                            member_id: member_id.clone(),
                            through_frame_head: frame_seq,
                            sync_token: recovered.attached.sync_token,
                        },
                    )
                    .unwrap_or_else(|error| panic!("conservative cursor sync: {error:?}"))
                    .is_empty()
            );
        }
        assert!(matches!(
            backend.attach(
                &member,
                RoomAttach {
                    room_id: room_id.clone(),
                    member_id: member_id.clone(),
                    after_frame_seq: Some(frame_seq.saturating_add(1)),
                },
            ),
            Err(BackendError::Rejected)
        ));

        let stale_action = ActionSubmit {
            room_id: room_id.clone(),
            member_id: member_id.clone(),
            action_id: "01ARZ3NDEKTSV4RRFFQ69G5FD2".to_owned(), // gitleaks:allow - public fixture ULID
            based_on_room_seq: 0,
            action_type: "increment".to_owned(),
            payload: json!({}),
        };
        let ActionReply::Rejected(stale) = backend
            .action(&member, stale_action.clone())
            .unwrap_or_else(|error| unreachable!("stale Action must be durable: {error:?}"))
        else {
            unreachable!("stale Action was accepted")
        };
        assert_eq!(stale.code, "stale_room_state");
        assert_eq!(stale.current_room_seq, 1);
        assert!(!stale.duplicate);

        if let Some(admin_dsn) = env::var_os("WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN") {
            let mut delivery_admin = Client::connect(&admin_dsn.to_string_lossy(), NoTls)
                .unwrap_or_else(|error| unreachable!("delivery budget admin connection: {error}"));

            // Capture a production Attach authority and barrier basis, then
            // advance only the operational integrity generation. The exact stale
            // head/integrity fence must fail before any retained delivery read;
            // the gateway maps this to a retryable attach and recovers its cache.
            let stale_authenticated = backend
                .authenticate(&member)
                .unwrap_or_else(|error| unreachable!("stale attach authentication: {error:?}"));
            let (stale_head, stale_generation, stale_projection) = backend
                .with_serving_trace(
                    &room_id
                        .parse()
                        .unwrap_or_else(|_| unreachable!("stale attach room")),
                    |trace, fence| {
                        let member = member_id.parse().map_err(|_| BackendError::InvalidResult)?;
                        let membership = trace
                            .core_state()
                            .membership(&member)
                            .ok_or(BackendError::InvalidResult)?;
                        let view = backend.view_for(trace, membership)?;
                        Ok((
                            trace.head().clone(),
                            fence.integrity().generation().get(),
                            worldstream_core::CanonicalJsonV1::from_canonical_bytes(
                                view.canonical_bytes(),
                            )
                            .map_err(|_| BackendError::InvalidResult)?,
                        ))
                    },
                )
                .unwrap_or_else(|error| unreachable!("stale attach serving trace: {error:?}"));
            let stale_authority = backend
                .authority()
                .authorize_member_read(
                    &stale_authenticated.into_presented(),
                    room_id
                        .parse()
                        .unwrap_or_else(|_| unreachable!("stale attach room")),
                    member_id
                        .parse()
                        .unwrap_or_else(|_| unreachable!("stale attach member")),
                    worldstream_core::MemberReadOperationV1::Attach,
                    backend
                        .checked_at()
                        .unwrap_or_else(|error| unreachable!("stale attach clock: {error:?}")),
                )
                .unwrap_or_else(|error| unreachable!("stale attach authorization: {error:?}"))
                .into_adapter_input();
            delivery_admin
            .execute(
                "UPDATE worldstream_room_roots SET integrity_generation = integrity_generation + 1 WHERE room_id = $1",
                &[&room_id],
            )
            .unwrap_or_else(|error| unreachable!("advance stale attach integrity: {error}"));
            assert!(matches!(
                backend.store.read_observation_at(
                    &room_id,
                    &member_id,
                    &stale_head,
                    stale_generation,
                    &stale_authority,
                    Some(frame_seq),
                    &stale_projection,
                ),
                Err(worldstream_postgres::PostgresObservationError::Fenced)
            ));
            assert_eq!(
                backend
                    .projection(&member, &room_id)
                    .unwrap_or_else(|error| unreachable!(
                        "fresh projection after stale fence: {error:?}"
                    ))
                    .integrity_generation,
                stale_generation + 1
            );

            // Delivery is operational state, not canonical history. Force a
            // retained suffix just beyond the read budget through the isolated
            // admin lane, then exercise the public production attach path. The
            // attach must install a coherent ProjectionReset rather than load the
            // 257-frame suffix and repeatedly hit transport backpressure.
            delivery_admin
            .execute(
                "INSERT INTO worldstream_frames(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash) SELECT $1, $2, sequence, sequence, '{}'::bytea, 'hash'::bytea FROM generate_series(2, 258) AS sequence ON CONFLICT (room_id, member_id, frame_seq) DO NOTHING",
                &[&room_id, &member_id],
            )
            .unwrap_or_else(|error| panic!("seed bounded attach frames: {error:?}"));
            delivery_admin
            .execute(
                "UPDATE worldstream_members SET frame_head = 258, retained_frame_floor = 1, reset_required_through = NULL WHERE room_id = $1 AND member_id = $2",
                &[&room_id, &member_id],
            )
            .unwrap_or_else(|error| unreachable!("publish bounded attach head: {error}"));
            let bounded_attach = backend
                .attach(
                    &member,
                    RoomAttach {
                        room_id: room_id.clone(),
                        member_id: member_id.clone(),
                        after_frame_seq: Some(frame_seq),
                    },
                )
                .unwrap_or_else(|error| unreachable!("bounded attach must reset: {error:?}"));
            assert!(matches!(
                &bounded_attach.attached.sync,
                worldstream_protocol::SyncBranch::ProjectionReset {
                    baseline_frame_head,
                    ..
                } if *baseline_frame_head == 258
            ));
            assert!(bounded_attach.reset.is_some());
            assert!(
                backend
                    .sync_ack(
                        &member,
                        RoomSyncAck {
                            room_id: room_id.clone(),
                            member_id: member_id.clone(),
                            through_frame_head: 258,
                            sync_token: bounded_attach.attached.sync_token,
                        },
                    )
                    .unwrap_or_else(|error| unreachable!(
                        "bounded reset sync acknowledgement: {error:?}"
                    ))
                    .is_empty()
            );
            // `sync_ack` activates the transport Session; this durable
            // acknowledgement is what makes 258 a valid reconnect Cursor.
            assert_eq!(
                backend
                    .observation_ack(
                        &member,
                        ObservationAck {
                            room_id: room_id.clone(),
                            member_id: member_id.clone(),
                            through_frame_seq: 258,
                        },
                    )
                    .unwrap_or_else(|error| unreachable!(
                        "persist bounded reset cursor: {error:?}"
                    )),
                Some(258)
            );
            // A missing retained tail must never become an empty live suffix:
            // the captured member head remains 258, while the bounded reader
            // observes that frame 258 is unavailable and fences the Session.
            delivery_admin
                .execute(
                    "DELETE FROM worldstream_frames WHERE room_id = $1 AND member_id = $2 AND frame_seq = 258",
                    &[&room_id, &member_id],
                )
                .unwrap_or_else(|error| unreachable!("delete bounded retained tail: {error}"));
            assert!(matches!(
                backend.live_observation_suffix(&member, &room_id, &member_id, 257),
                Err(BackendError::ResetRequired)
            ));

            // A later reset marker at the same frame head must fence the already
            // live Session by epoch, while a fresh attach at the same Cursor is
            // allowed to install that new reset epoch.
            delivery_admin
            .execute(
                "UPDATE worldstream_members SET reset_required_through = frame_head, reset_generation = reset_generation + 1 WHERE room_id = $1 AND member_id = $2",
                &[&room_id, &member_id],
            )
            .unwrap_or_else(|error| unreachable!("install same-head reset epoch: {error}"));
            assert!(matches!(
                backend.live_observation_suffix(&member, &room_id, &member_id, 258),
                Err(BackendError::ResetRequired)
            ));
            let epoch_reattach = backend
                .attach(
                    &member,
                    RoomAttach {
                        room_id: room_id.clone(),
                        member_id: member_id.clone(),
                        after_frame_seq: Some(258),
                    },
                )
                .unwrap_or_else(|error| unreachable!("same-cursor reset reattach: {error:?}"));
            assert!(matches!(
                &epoch_reattach.attached.sync,
                worldstream_protocol::SyncBranch::ProjectionReset {
                    baseline_frame_head,
                    ..
                } if *baseline_frame_head == 258
            ));
            assert!(
                backend
                    .sync_ack(
                        &member,
                        RoomSyncAck {
                            room_id: room_id.clone(),
                            member_id: member_id.clone(),
                            through_frame_head: 258,
                            sync_token: epoch_reattach.attached.sync_token,
                        },
                    )
                    .unwrap_or_else(|error| unreachable!(
                        "same-cursor reset sync acknowledgement: {error:?}"
                    ))
                    .is_empty()
            );
            delivery_admin
            .execute(
                "DELETE FROM worldstream_frames WHERE room_id = $1 AND member_id = $2 AND frame_seq >= 2",
                &[&room_id, &member_id],
            )
            .unwrap_or_else(|error| unreachable!("clean bounded attach frames: {error}"));
            delivery_admin
            .execute(
                "UPDATE worldstream_members SET frame_head = $3, retained_frame_floor = 1, last_ack_frame_seq = $3, reset_required_through = NULL WHERE room_id = $1 AND member_id = $2",
                &[&room_id, &member_id, &i64::try_from(frame_seq).unwrap_or(0)],
            )
            .unwrap_or_else(|error| unreachable!("restore bounded attach positions: {error}"));
            println!(
                "LIVE_POSTGRES=PASS bounded-attach=257-frame-projection-reset+same-head-reset-epoch"
            );
        }

        let heist_registry = builtin_agent_heist_registry()
            .unwrap_or_else(|error| unreachable!("Agent Heist registry: {error}"));
        let lobby_descriptor = heist_registry
            .load_retained(&agent_heist_lobby_digest())
            .unwrap_or_else(|error| unreachable!("Lobby revision: {error}"))
            .descriptor()
            .clone();
        let lobby_room = backend
            .create_room(
                &host,
                CreateRoomRequest {
                    pack: PackReference {
                        id: lobby_descriptor.pack_id,
                        version: lobby_descriptor.explanatory_version,
                        digest: agent_heist_lobby_digest().to_string(),
                    },
                    configuration: json!({
                        "pack_id":"worldstream.agent-heist","pack_schema":1,
                        "roles":["navigator","insider","broker"],
                        "briefing_duration_seconds":30,"negotiation_duration_seconds":90,
                        "commitment_duration_seconds":30,
                        "commitment_reminder_seconds_before_deadline":10,
                        "result_duration_seconds":20,"maximum_plans":12,
                        "maximum_open_offers_per_role":4
                    }),
                    members: vec![
                        CreateMember {
                            principal_id: host_principal.to_string(),
                            principal_kind: PrincipalKind::Human,
                            role: Some("navigator".to_owned()),
                            access_mode: AccessMode::Participant,
                        },
                        CreateMember {
                            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FE0".to_owned(),
                            principal_kind: PrincipalKind::Human,
                            role: Some("insider".to_owned()),
                            access_mode: AccessMode::Participant,
                        },
                        CreateMember {
                            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FE1".to_owned(),
                            principal_kind: PrincipalKind::Human,
                            role: Some("broker".to_owned()),
                            access_mode: AccessMode::Participant,
                        },
                    ],
                    idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FE2".to_owned(),
                },
            )
            .unwrap_or_else(|error| unreachable!("create Lobby: {error:?}"));
        let lobby_request = LobbyLaunchRequest {
            input_id: "01ARZ3NDEKTSV4RRFFQ69G5FE3".to_owned(),
            based_on_room_seq: 0,
            pack_digest: None,
        };
        let lobby_first = backend
            .launch_lobby(&host, &lobby_room.room_id, lobby_request.clone())
            .unwrap_or_else(|error| unreachable!("launch Lobby: {error:?}"));
        assert_eq!(lobby_first.room_head.room_seq, 1);
        assert!(!lobby_first.duplicate);

        drop(backend);
        let restarted = PostgresGatewayBackend::new(
            PostgresRoomStore::new(
                PostgresConnectionConfig::runtime(dsn.clone(), connection_path)
                    .unwrap_or_else(|error| unreachable!("restart config: {error}")),
            )
            .unwrap_or_else(|error| unreachable!("restart store: {error}")),
            restart_registry,
        );
        let projection_after = restarted
            .projection(&member, &room_id)
            .unwrap_or_else(|error| unreachable!("restart projection: {error:?}"));
        assert_eq!(projection_after.room_head, first.room_head);
        assert_eq!(
            restarted
                .provision_member_capability(
                    &host,
                    serde_json::from_value::<MemberCapabilityProvisionRequestV1>(
                        sealed_member.clone(),
                    )
                    .unwrap_or_else(|error| unreachable!("duplicate sealed member: {error}")),
                )
                .unwrap_or_else(|error| unreachable!("replay sealed member: {error:?}")),
            sealed_member_first
        );
        let mut conflicting_member = sealed_member;
        conflicting_member["capability"]["bearer"] =
            json!(BearerWireV1::from_bytes([0xc3; 32]).to_wire());
        assert!(matches!(
            restarted.provision_member_capability(
                &host,
                serde_json::from_value::<MemberCapabilityProvisionRequestV1>(conflicting_member)
                    .unwrap_or_else(|error| unreachable!("conflicting sealed member: {error}")),
            ),
            Err(BackendError::Conflict)
        ));
        assert_eq!(
            restarted
                .provision_runner_capability(
                    &host,
                    serde_json::from_value::<RunnerCapabilityProvisionRequestV1>(
                        sealed_runner.clone(),
                    )
                    .unwrap_or_else(|error| unreachable!("duplicate sealed runner: {error}")),
                )
                .unwrap_or_else(|error| unreachable!("replay sealed runner: {error:?}")),
            sealed_runner_first
        );
        let mut mismatched_principal = sealed_runner.clone();
        mismatched_principal["owner_principal_id"] = json!("01ARZ3NDEKTSV4RRFFQ69G5FC9");
        assert!(matches!(
            restarted.provision_runner_capability(
                &host,
                serde_json::from_value::<RunnerCapabilityProvisionRequestV1>(mismatched_principal,)
                    .unwrap_or_else(|error| unreachable!("mismatched Runner principal: {error}")),
            ),
            Err(BackendError::Forbidden)
        ));
        let mut mismatched_principal_change = sealed_runner.clone();
        mismatched_principal_change["principal_idempotency_key"] =
            json!("01ARZ3NDEKTSV4RRFFQ69G5FE9");
        assert!(matches!(
            restarted.provision_runner_capability(
                &host,
                serde_json::from_value::<RunnerCapabilityProvisionRequestV1>(
                    mismatched_principal_change,
                )
                .unwrap_or_else(|error| {
                    unreachable!("mismatched Runner principal change: {error}")
                }),
            ),
            Err(BackendError::Conflict)
        ));
        let mut conflicting_runner = sealed_runner;
        conflicting_runner["capability"]["bearer"] =
            json!(BearerWireV1::from_bytes([0xc4; 32]).to_wire());
        assert!(matches!(
            restarted.provision_runner_capability(
                &host,
                serde_json::from_value::<RunnerCapabilityProvisionRequestV1>(conflicting_runner)
                    .unwrap_or_else(|error| unreachable!("conflicting sealed runner: {error}")),
            ),
            Err(BackendError::Conflict)
        ));
        let ActionReply::Accepted(duplicate) = restarted
            .action(&member, action.clone())
            .unwrap_or_else(|error| unreachable!("duplicate Action: {error:?}"))
        else {
            unreachable!("duplicate Action rejected")
        };
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.transition_id, first.transition_id);
        let ActionReply::Rejected(stale_duplicate) = restarted
            .action(&member, stale_action)
            .unwrap_or_else(|error| unreachable!("duplicate stale Action: {error:?}"))
        else {
            unreachable!("duplicate stale Action was accepted")
        };
        assert_eq!(stale_duplicate.code, "stale_room_state");
        assert_eq!(stale_duplicate.current_room_seq, 1);
        assert!(stale_duplicate.duplicate);
        let mut conflicting_action = action;
        conflicting_action.payload = json!({"changed": true});
        assert!(matches!(
            restarted.action(&member, conflicting_action),
            Err(BackendError::Conflict)
        ));
        let lobby_duplicate = restarted
            .launch_lobby(&host, &lobby_room.room_id, lobby_request.clone())
            .unwrap_or_else(|error| unreachable!("restart duplicate Lobby launch: {error:?}"));
        assert!(lobby_duplicate.duplicate);
        assert_eq!(lobby_duplicate.transition_id, lobby_first.transition_id);
        assert!(matches!(
            restarted.launch_lobby(
                &host,
                &lobby_room.room_id,
                LobbyLaunchRequest {
                    based_on_room_seq: 1,
                    ..lobby_request.clone()
                },
            ),
            Err(BackendError::Conflict)
        ));
        assert!(matches!(
            restarted.launch_lobby(
                &host,
                &lobby_room.room_id,
                LobbyLaunchRequest {
                    input_id: "01ARZ3NDEKTSV4RRFFQ69G5FE4".to_owned(),
                    based_on_room_seq: 1,
                    pack_digest: None,
                },
            ),
            Err(BackendError::WrongPhase)
        ));
        assert!(matches!(
            restarted.launch_lobby(
                &session(0xab, "01ARZ3NDEKTSV4RRFFQ69G5FE5"),
                &lobby_room.room_id,
                lobby_request,
            ),
            Err(BackendError::Forbidden)
        ));

        // The restarted projection above installs a verified serving trace.
        // A production Activation claim must reuse that executor and avoid
        // both full Room verification and Genesis replay.
        let mut activation_fixture = Client::connect(&dsn, NoTls)
            .unwrap_or_else(|error| unreachable!("Activation fixture connection: {error}"));
        activation_fixture
            .execute(
                "DELETE FROM worldstream_activation_intents WHERE activation_id LIKE 'operator-status-%'",
                &[],
            )
            .unwrap_or_else(|error| unreachable!("clean status Activation fixtures: {error}"));
        activation_fixture
            .execute(
                "INSERT INTO worldstream_activation_intents(\
                 activation_id, room_id, cause_room_seq, decision_id, target_member_id,\
                 reason_code, deduplication_key, priority, policy_revision, state,\
                 intent_generation, lease_generation)\
                 VALUES ($1, $2, 1, $3, $4, 'warm-path', $5, 1, 1, 'pending', 1, 0)",
                &[
                    &"postgres-warm-activation",
                    &room_id,
                    &"postgres-warm-decision",
                    &member_id,
                    &"postgres-warm-dedup",
                ],
            )
            .unwrap_or_else(|error| unreachable!("seed warm Activation: {error}"));
        drop(activation_fixture);
        restarted
            .with_serving_trace(
                &room_id
                    .parse()
                    .unwrap_or_else(|_| unreachable!("warm Activation Room")),
                |_, _| Ok(()),
            )
            .unwrap_or_else(|error| unreachable!("warm Activation serving trace: {error:?}"));
        restarted.forbid_recovery_for_test();
        let warm_claim = ActivationClaim {
            activation_id: "postgres-warm-activation".to_owned(),
            runner_id: "01ARZ3NDEKTSV4RRFFQ69G5FF2".to_owned(),
            claim_id: "postgres-warm-claim".to_owned(),
            requested_lease_ms: 30_000,
        };
        let runner = session(0xc2, "01ARZ3NDEKTSV4RRFFQ69G5FF4");
        let granted = restarted
            .activation_claim(&runner, warm_claim.clone())
            .unwrap_or_else(|error| unreachable!("warm Activation claim: {error:?}"));
        assert_eq!(granted.code, ActivationResultCode::Granted);
        assert_eq!(
            restarted
                .activation_claim(&runner, warm_claim.clone())
                .unwrap_or_else(|error| unreachable!("warm claim retry: {error:?}")),
            granted
        );
        let initial_generation = granted
            .lease_generation
            .unwrap_or_else(|| unreachable!("warm Activation omitted lease generation"));
        let released = restarted
            .activation_release(
                &runner,
                ActivationLeaseOperation {
                    activation_id: warm_claim.activation_id.clone(),
                    runner_id: warm_claim.runner_id.clone(),
                    claim_id: warm_claim.claim_id.clone(),
                    operation_id: "postgres-warm-retry-release".to_owned(),
                    lease_generation: initial_generation,
                    requested_lease_ms: None,
                    disposition: None,
                },
            )
            .unwrap_or_else(|error| unreachable!("release retried warm claim: {error:?}"));
        assert_eq!(released.code, ActivationResultCode::Released);

        // A duplicate claim is intentionally receipt-first. Fresh identities
        // are the qualification vector: every iteration must borrow the same
        // restart-installed executor and prepare a new durable claim context.
        let warm_samples = env::var("WORLDSTREAM_POSTGRES_WARM_CLAIM_SAMPLES")
            .ok()
            .map(|value| {
                value
                    .parse::<usize>()
                    .unwrap_or_else(|error| unreachable!("warm sample count: {error}"))
            })
            .unwrap_or(32);
        assert!(
            (1..=4096).contains(&warm_samples),
            "warm sample count must stay bounded"
        );
        let warm_room_id = room_id
            .parse::<worldstream_core::RoomId>()
            .unwrap_or_else(|_| unreachable!("warm Activation Room"));
        let callbacks_before = restarted
            .traces
            .with_room(&warm_room_id, |slot| {
                slot.as_ref()
                    .map(|cached| cached.trace().activity_callback_count())
                    .unwrap_or_else(|| unreachable!("warm serving trace was not cached"))
            })
            .unwrap_or_else(|error| unreachable!("inspect warm trace: {error:?}"));
        // Current reads use the same fenced executor as claims. Sample them
        // independently so the production PostgreSQL audit does not mistake
        // claim preparation latency for projection latency.
        let mut current_read_latencies = Vec::with_capacity(warm_samples);
        for index in 0..warm_samples {
            let started = Instant::now();
            let projection = restarted
                .projection(&member, &room_id)
                .unwrap_or_else(|error| unreachable!("warm current read {index}: {error:?}"));
            assert_eq!(projection.room_head.room_seq, 1);
            current_read_latencies.push(started.elapsed());
        }
        let callbacks_after_current_reads = restarted
            .traces
            .with_room(&warm_room_id, |slot| {
                slot.as_ref()
                    .map(|cached| cached.trace().activity_callback_count())
                    .unwrap_or_else(|| unreachable!("warm serving trace was evicted"))
            })
            .unwrap_or_else(|error| unreachable!("inspect warm current reads: {error:?}"));
        assert_eq!(callbacks_after_current_reads, callbacks_before);
        current_read_latencies.sort_unstable();
        let current_read_latency_us = |percentile: usize| {
            u64::try_from(
                current_read_latencies[((current_read_latencies.len().saturating_sub(1)
                    * percentile)
                    / 100)
                    .min(current_read_latencies.len() - 1)]
                .as_micros(),
            )
            .unwrap_or(u64::MAX)
        };
        let current_read_p50_us = current_read_latency_us(50);
        let current_read_p95_us = current_read_latency_us(95);
        let current_read_p99_us = current_read_latency_us(99);
        let warm_transition_rows_before_claims = Client::connect(&dsn, NoTls)
            .unwrap_or_else(|error| unreachable!("warm claim row counter connection: {error}"))
            .query_one(
                "SELECT count(*) FROM worldstream_transitions WHERE room_id = $1",
                &[&room_id],
            )
            .unwrap_or_else(|error| unreachable!("warm claim row counter before: {error}"))
            .get::<_, i64>(0);
        let mut latencies = Vec::with_capacity(warm_samples);
        for index in 0..warm_samples {
            let claim_id = format!("postgres-warm-fresh-claim-{index}");
            let started = Instant::now();
            let reply = restarted
                .activation_claim(
                    &runner,
                    ActivationClaim {
                        activation_id: warm_claim.activation_id.clone(),
                        runner_id: warm_claim.runner_id.clone(),
                        claim_id: claim_id.clone(),
                        requested_lease_ms: 30_000,
                    },
                )
                .unwrap_or_else(|error| unreachable!("fresh warm claim {index}: {error:?}"));
            assert_eq!(reply.code, ActivationResultCode::Granted);
            latencies.push(started.elapsed());
            let lease_generation = reply
                .lease_generation
                .unwrap_or_else(|| unreachable!("fresh warm claim omitted lease generation"));
            let released = restarted
                .activation_release(
                    &runner,
                    ActivationLeaseOperation {
                        activation_id: warm_claim.activation_id.clone(),
                        runner_id: warm_claim.runner_id.clone(),
                        claim_id,
                        operation_id: format!("postgres-warm-fresh-release-{index}"),
                        lease_generation,
                        requested_lease_ms: None,
                        disposition: None,
                    },
                )
                .unwrap_or_else(|error| {
                    unreachable!("fresh warm claim release {index}: {error:?}")
                });
            assert_eq!(released.code, ActivationResultCode::Released);
        }
        let callbacks_after = restarted
            .traces
            .with_room(&warm_room_id, |slot| {
                slot.as_ref()
                    .map(|cached| cached.trace().activity_callback_count())
                    .unwrap_or_else(|| unreachable!("warm serving trace was evicted"))
            })
            .unwrap_or_else(|error| unreachable!("inspect final warm trace: {error:?}"));
        assert_eq!(callbacks_after, callbacks_before);
        let warm_transition_rows_after_claims = Client::connect(&dsn, NoTls)
            .unwrap_or_else(|error| unreachable!("warm claim row counter connection: {error}"))
            .query_one(
                "SELECT count(*) FROM worldstream_transitions WHERE room_id = $1",
                &[&room_id],
            )
            .unwrap_or_else(|error| unreachable!("warm claim row counter after: {error}"))
            .get::<_, i64>(0);
        assert_eq!(
            warm_transition_rows_after_claims, warm_transition_rows_before_claims,
            "fresh warm claim/release cycles must not append canonical Transitions"
        );
        latencies.sort_unstable();
        let latency_us = |percentile: usize| {
            u64::try_from(
                latencies[((latencies.len().saturating_sub(1) * percentile) / 100)
                    .min(latencies.len() - 1)]
                .as_micros(),
            )
            .unwrap_or(u64::MAX)
        };
        let warm_p50_us = latency_us(50);
        let warm_p95_us = latency_us(95);
        let warm_p99_us = latency_us(99);

        let connector = native_tls::TlsConnector::builder()
            .build()
            .unwrap_or_else(|error| unreachable!("PostgreSQL inspection TLS: {error}"));
        let mut inspector =
            postgres::Client::connect(&dsn, postgres_native_tls::MakeTlsConnector::new(connector))
                .unwrap_or_else(|error| unreachable!("PostgreSQL inspection connection: {error}"));
        let counts = inspector
            .query_one(
                "SELECT \
                    (SELECT count(*) FROM worldstream_authority_principals WHERE principal_id = '01ARZ3NDEKTSV4RRFFQ69G5FC6'), \
                    (SELECT count(*) FROM worldstream_authority_runners WHERE runner_id = '01ARZ3NDEKTSV4RRFFQ69G5FF2'), \
                    (SELECT count(*) FROM worldstream_authority_capabilities WHERE capability_id IN ('01ARZ3NDEKTSV4RRFFQ69G5FF0', '01ARZ3NDEKTSV4RRFFQ69G5FF4')), \
                    (SELECT count(*) FROM worldstream_authority_change_receipts WHERE change_id IN ('01ARZ3NDEKTSV4RRFFQ69G5FC6', '01ARZ3NDEKTSV4RRFFQ69G5FF1', '01ARZ3NDEKTSV4RRFFQ69G5FF3', '01ARZ3NDEKTSV4RRFFQ69G5FF5')), \
                    (SELECT count(*) FROM worldstream_authority_audit WHERE change_id IN ('01ARZ3NDEKTSV4RRFFQ69G5FC6', '01ARZ3NDEKTSV4RRFFQ69G5FF1', '01ARZ3NDEKTSV4RRFFQ69G5FF3', '01ARZ3NDEKTSV4RRFFQ69G5FF5')), \
                    (SELECT count(*) FROM worldstream_authority_capability_scopes WHERE capability_id IN ('01ARZ3NDEKTSV4RRFFQ69G5FF0', '01ARZ3NDEKTSV4RRFFQ69G5FF4')), \
                    (SELECT count(*) FROM worldstream_authority_runner_capability_memberships WHERE capability_id = '01ARZ3NDEKTSV4RRFFQ69G5FF4')",
                &[],
            )
            .unwrap_or_else(|error| unreachable!("PostgreSQL authority counts: {error}"));
        assert_eq!(counts.get::<_, i64>(0), 1);
        assert_eq!(counts.get::<_, i64>(1), 1);
        assert_eq!(counts.get::<_, i64>(2), 2);
        assert_eq!(counts.get::<_, i64>(3), 4);
        assert_eq!(counts.get::<_, i64>(4), 4);
        assert_eq!(counts.get::<_, i64>(5), 6);
        assert_eq!(counts.get::<_, i64>(6), 1);

        eprintln!(
            "LIVE_POSTGRES_GATEWAY=PASS create+duplicate+conflict+projection+replay+attach+sync+resync+action+stale+live+ack+restart+lobby-launch+sealed-provision-replay-conflict+warm-activation-no-recovery+warm-activation-fresh-leases-no-recovery path={connection_path:?} samples={warm_samples} current_read_p50_us={current_read_p50_us} current_read_p95_us={current_read_p95_us} current_read_p99_us={current_read_p99_us} current_read_callbacks={callbacks_after_current_reads} p50_us={warm_p50_us} p95_us={warm_p95_us} p99_us={warm_p99_us} reducer_callbacks={callbacks_after} canonical_transition_rows_before_claims={warm_transition_rows_before_claims} canonical_transition_rows_after_claims={warm_transition_rows_after_claims}"
        );
    }
}
struct PostgresLiveFence {
    store: Arc<PostgresRoomStore>,
    bindings: Arc<SessionBindings>,
    grant: Arc<worldstream_core::ViewerAdapterInputV1>,
    cut: Arc<worldstream_postgres::PostgresLiveObservationCut>,
    session_id: worldstream_protocol::UlidString,
    capability_id: worldstream_core::CapabilityId,
    reset: u64,
}
impl crate::LiveObservationFence for PostgresLiveFence {
    fn revalidate(&self, session: &GatewaySession) -> Result<(), BackendError> {
        if session.session_id() != &self.session_id {
            return Err(BackendError::Rejected);
        }
        let reset = self.bindings.live_reset_generation(
            session.session_id(),
            &self.capability_id,
            self.grant.room_id(),
            self.grant.membership().member_id(),
        )?;
        if reset != self.reset {
            return Err(BackendError::Rejected);
        }
        self.store
            .revalidate_live_observation(&self.grant, &self.cut)
            .map_err(map_observation_error)
    }
}
