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
    AdmissionLaneErrorV1, AuthorityChangeId, AuthorityChangeV1, AuthorityCheckedAt,
    AuthorityErrorV1, AuthorityStoreV1, AuthorityV1, AuthorizedRunnerControlV1,
    CREATE_ROOM_OPERATION_KIND, CanonicalJsonV1, CapabilityBearerV1, CapabilityExpiresAt,
    CapabilityId, CapabilityProfileV1, CapabilityScopeSetV1, CreationRecordedAt,
    DiagnosticOperationV1, DiagnosticTargetV1, ExternalInputRecordedAt, ExternalInputV1,
    HistoricalReplayErrorV1, HostClockErrorV1, HostClockSampleV1, HostClockV1,
    InitialMembershipProposalV1, InputId, MemberReadOperationV1, MembershipStandingV1,
    MembershipV1, MonotonicHostClockV1, NewCapabilityV1, PackGenesisRequestV1, PackRegistryV1,
    PackViewerV1, ParticipantActionIngressErrorV1, ParticipantActionIngressV1,
    ParticipantActionRequestV1, PreparedRoomCreationV1, PrincipalKindV1, ReplayProjectionKindV1,
    RoomAdmissionLanesV1, RoomCommitResolutionV1, RoomCommitStorageV1, RoomCreationIngressV1,
    RoomCreationRequestV1, RoomId, RoomMembershipKeyV1, RoomSeedV1, RoomSequenceV1,
    RunnerControlOperationV1, RunnerId, RunnerMembershipSetV1, SemanticResultV1, SessionErrorV1,
    SessionFrameV1, SessionSyncTokenV1, SessionV1, SourceId, StoredSemanticResultV1,
    TimerFiredRequestV1, TimerGenerationV1, TimerId, TransitionId,
    authorize_participant_action_operation, authorize_room_creation_operation,
    commit_room_creation, external_input_request_hash,
};
use worldstream_postgres::{
    PostgresActivationError, PostgresAuthorityAuthenticationError,
    PostgresExternalInputPreparationErrorV1, PostgresFrameEvidenceV1,
    PostgresObservationDeliveryV1, PostgresObservationError, PostgresRoomCommitError,
    PostgresRoomDiagnosticErrorV1, PostgresRoomDiagnosticSummaryV1, PostgresRoomStore,
    PostgresSchemaVerificationError, PostgresTimerStateV1,
};
use worldstream_protocol::{
    AccessMode, ActionAccepted, ActionRejected, ActionSubmit, ActivationClaim, ActivationDelivery,
    ActivationFrame, ActivationIntentState, ActivationLeaseOperation, ActivationOffer,
    ActivationOfferRequest, ActivationOffers, ActivationOperationReply, ActivationResultCode,
    BearerWireV1, ClientHello, CreateRoomRequest, CreateRoomResponse, LobbyLaunchRequest,
    LobbyLaunchResponse, MAX_MESSAGE_BYTES, MemberCapabilityProvisionRequestV1,
    MemberCapabilityProvisionResponseV1, ObservationAck, ObservationDeliver, OperatorActivityPhase,
    OperatorBackupProfileStatus, OperatorBackupStorageHealth, OperatorBackupStorageProfile,
    OperatorBackupVerification, OperatorDataFreshness, OperatorLiveBackupPrepareRequest,
    OperatorLiveBackupStatus, OperatorRoomIntegrity, OperatorRoomIntegrityStatus,
    OperatorRoomInventoryPage, OperatorRoomInventoryRequest, OperatorRoomSummary, PROTOCOL_VERSION,
    PackReference, Principal, PrincipalKind, Projection, ProjectionReset, ProjectionResponse,
    ReplayResponse, RoomAttach, RoomAttached, RoomHead, RoomSyncAck,
    RunnerCapabilityProvisionRequestV1, RunnerCapabilityProvisionResponseV1, RunnerHello,
    RunnerReady, ServerWelcome, SyncBranch, TimerFireRequest, TimerFireResponse,
};
use worldstream_runtime::SecretSource;

use crate::{
    ActionReply, AttachReply, BackendError, GatewayBackend, GatewaySession,
    MemberCapabilityIssueRequest, MemberCapabilityIssueResponse, RunnerCapabilityIssueRequest,
    RunnerCapabilityIssueResponse, RunnerMembershipTarget, fill_random_bytes,
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
    registry: Option<Arc<PackRegistryV1>>,
    bindings: SessionBindings,
    host_clock: Arc<dyn HostClockV1>,
    admission_lanes: RoomAdmissionLanesV1,
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
            .field("host_clock", &"[OPAQUE]")
            .field("admission_lanes", &self.admission_lanes)
            .finish()
    }
}

impl PostgresGatewayBackend {
    /// Retains the already-configured least-privilege runtime store.
    #[must_use]
    pub fn new(store: PostgresRoomStore) -> Self {
        Self::with_host_clock(store, Arc::new(MonotonicHostClockV1::new(RuntimeWallClock)))
    }

    /// Wires the gateway to one application-owned semantic `HostClock`.
    ///
    /// Callers providing a raw wall source should wrap it in
    /// [`MonotonicHostClockV1`].
    #[must_use]
    pub fn with_host_clock(store: PostgresRoomStore, host_clock: Arc<dyn HostClockV1>) -> Self {
        Self::with_runtime(store, host_clock, RoomAdmissionLanesV1::default())
    }

    fn with_runtime(
        store: PostgresRoomStore,
        host_clock: Arc<dyn HostClockV1>,
        admission_lanes: RoomAdmissionLanesV1,
    ) -> Self {
        Self {
            store: Arc::new(store),
            registry: worldstream_core::builtin_worldstream_registry()
                .ok()
                .map(Arc::new),
            bindings: SessionBindings::default(),
            host_clock,
            admission_lanes,
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

    fn registry(&self) -> Result<&PackRegistryV1, BackendError> {
        self.registry
            .as_deref()
            .ok_or(BackendError::StorageUnavailable)
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
        self.verified_trace(&room_id)?;
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
        let (trace, _) = self.verified_trace(&room_id)?;
        let membership = trace
            .core_state()
            .membership(&member_id)
            .filter(|membership| membership.standing() == MembershipStandingV1::Enabled)
            .ok_or(BackendError::Forbidden)?;
        if membership.principal_id() != &principal_id
            || membership.principal_kind() != core_principal_kind(request.principal_kind)
            || membership.access_mode() != core_access_mode(request.access_mode)
            || membership.role() != Some(request.role.as_str())
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
            worldstream_core::CoreTraceV1,
            worldstream_postgres::PostgresRoomVerification,
        ),
        BackendError,
    > {
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
            .recover_room(self.registry()?, room_id.as_ref())
            .map_err(map_recovery_error)?
            .ok_or(BackendError::NotFound)?;
        Ok((trace, verification))
    }

    fn member_for_principal(
        trace: &worldstream_core::CoreTraceV1,
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
        trace: &worldstream_core::CoreTraceV1,
        membership: &MembershipV1,
    ) -> Result<worldstream_core::ValidatedPackViewV1, BackendError> {
        self.registry()?
            .load_retained(trace.head().pack_digest())
            .map_err(|_| BackendError::InvalidResult)?
            .host()
            .view(&worldstream_core::ViewInputV1 {
                core: trace.core_state(),
                activity_state: trace.activity_state(),
                complete_head: trace.head(),
                viewer: &viewer_for(membership),
            })
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
    ) -> Result<CreateAttempt, BackendError> {
        let authenticated = self.authenticate(session)?;
        let (core_request, identity) = creation_request(&authenticated, request)?;
        let grant = match authorize_room_creation_operation(
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
            .registry()?
            .select_for_new_room(core_request.pack_digest())
            .map_err(|_| BackendError::Rejected)?;
        if selected.descriptor().pack_id != request.pack.id
            || selected.descriptor().explanatory_version != request.pack.version
        {
            return Err(BackendError::Rejected);
        }
        let room_id = next_core_id::<RoomId>()?;
        let genesis = self
            .registry()?
            .prepare_genesis_for_new_room(&PackGenesisRequestV1 {
                room_id,
                pack_digest: core_request.pack_digest().clone(),
                configuration: core_request.configuration().clone(),
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
            })
            .map_err(|_| BackendError::Rejected)?;
        let prepared =
            PreparedRoomCreationV1::from_registry_genesis(identity, &core_request, grant, genesis)
                .map_err(|_| BackendError::InvalidResult)?;
        let (resolution, _, _) = commit_room_creation(self.store.as_ref(), prepared).into_parts();
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
        let (trace, verification) = self.verified_trace(&room_id)?;
        let membership = Self::member_for_principal(&trace, authenticated.principal_id())?;
        self.authority()
            .authorize_member_read(
                &authenticated.into_presented(),
                room_id.clone(),
                membership.member_id().clone(),
                MemberReadOperationV1::CurrentProjection,
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
        let view = self.view_for(&trace, &membership)?;
        let (current, _) = self.verified_trace(&room_id)?;
        if current.head() != trace.head() {
            return Err(BackendError::Busy);
        }
        Ok(ProjectionResponse {
            room_id: room_id.to_string(),
            room_head: room_head(trace.head()),
            room_health: verification.integrity_status,
            integrity_generation: verification.integrity_generation,
            projection_schema: view.projection_schema().to_owned(),
            projection: projection_from_view(&view)?,
            projection_hash: view
                .projection_hash()
                .map_err(|_| BackendError::InvalidResult)?
                .to_string(),
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
                self.registry()?,
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
        let descriptor = self
            .registry()?
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
        let (trace, verification) = self.verified_trace(&room_id)?;
        let membership = trace
            .core_state()
            .membership(&member_id)
            .filter(|membership| {
                membership.principal_id() == authenticated.principal_id()
                    && membership.standing() == MembershipStandingV1::Enabled
            })
            .cloned()
            .ok_or(BackendError::Forbidden)?;
        let view = self.view_for(&trace, &membership)?;
        self.authority()
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
            .read_observation(
                room_id.as_ref(),
                member_id.as_ref(),
                request.after_frame_seq,
                &canonical,
            )
            .map_err(map_observation_error)?;
        let (sync, reset, frames, frame_head, retained_floor, cursor) =
            Self::protocol_delivery(&delivery, &room_id, &member_id, &trace, &verification)?;
        if request.after_frame_seq != cursor {
            return Err(BackendError::Rejected);
        }
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
                session: core_session,
                core_token: barrier.sync_token().clone(),
            },
        )?;
        let descriptor = self
            .registry()?
            .load_retained(trace.head().pack_digest())
            .map_err(|_| BackendError::InvalidResult)?
            .descriptor();
        Ok(AttachReply {
            attached: RoomAttached {
                room_id: room_id.to_string(),
                member_id: member_id.to_string(),
                principal_kind: protocol_principal_kind(membership.principal_kind()),
                access_mode: protocol_access_mode(membership.access_mode()),
                role: membership.role().map(str::to_owned),
                membership_status: membership_status(membership.standing()),
                room_status: room_status(trace.core_state().room_status()),
                room_health: verification.integrity_status,
                integrity_generation: verification.integrity_generation,
                room_head: room_head(trace.head()),
                cursor,
                frame_head,
                retained_floor,
                sync_token: token,
                sync,
                pack: worldstream_protocol::PackReference {
                    id: descriptor.pack_id.clone(),
                    version: descriptor.explanatory_version.clone(),
                    digest: trace.head().pack_digest().to_string(),
                },
            },
            reset,
            frames,
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
                let (trace, _) = self.verified_trace(&action_room_id)?;
                let member_id = request
                    .member_id
                    .parse()
                    .map_err(|_| BackendError::Rejected)?;
                let membership = trace
                    .core_state()
                    .membership(&member_id)
                    .ok_or(BackendError::Forbidden)?;
                let view = self.view_for(&trace, membership)?;
                let offer = view
                    .action_offers()
                    .offers()
                    .iter()
                    .find(|offer| offer.action_type == request.action_type)
                    .ok_or(BackendError::Rejected)?;
                let admission = self
                    .admission_lanes
                    .reserve_action(&action_room_id, self.host_clock.as_ref())
                    .map_err(|error| map_admission_lane_error(&error))?;
                let stimulus = worldstream_core::ParticipantActionV1 {
                    member_id,
                    action_id: request
                        .action_id
                        .parse()
                        .map_err(|_| BackendError::Rejected)?,
                    action_type: request.action_type.clone(),
                    payload_schema_digest: offer.payload_schema_digest.clone(),
                    canonical_payload: canonical_json(&request.payload)?,
                    exact_basis_head: trace.head().clone(),
                    admitted_at: admission.admitted_at().clone(),
                };
                let resolution = self
                    .store
                    .commit_authorized_participant_action(
                        self.registry()?,
                        *authority,
                        &core_request,
                        stimulus,
                        next_core_id::<TransitionId>()?,
                    )
                    .map_err(map_room_commit_error)?;
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
        Ok(crate::activity_pack_catalog_from_registry(self.registry()?))
    }

    fn activity_pack_revision(
        &self,
        session: &GatewaySession,
        revision_digest: &str,
    ) -> Result<worldstream_protocol::ActivityPackCatalogRevisionResponse, BackendError> {
        self.authorize_activity_pack_catalog(session)?;
        crate::activity_pack_revision_from_registry(self.registry()?, revision_digest)
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
            match self.creation_attempt(session, &request)? {
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
            self.authority()
                .authorize_member_read(
                    &authenticated.into_presented(),
                    room_id.clone(),
                    member_id.clone(),
                    MemberReadOperationV1::CatchUp,
                    self.checked_at()?,
                )
                .map_err(map_authority_error)?;
            let (trace, _) = self.verified_trace(&room_id)?;
            let membership = trace
                .core_state()
                .membership(&member_id)
                .ok_or(BackendError::Forbidden)?;
            let view = self.view_for(&trace, membership)?;
            let projection = CanonicalJsonV1::from_canonical_bytes(view.canonical_bytes())
                .map_err(|_| BackendError::InvalidResult)?;
            let delivery = self
                .store
                .read_observation(
                    room_id.as_ref(),
                    member_id.as_ref(),
                    Some(binding.baseline_frame_head),
                    &projection,
                )
                .map_err(map_observation_error)?;
            let PostgresObservationDeliveryV1::Retained { frames, .. } = delivery else {
                return Err(BackendError::Busy);
            };
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
        self.authority()
            .authorize_member_read(
                &authenticated.into_presented(),
                room_id.clone(),
                member_id.clone(),
                MemberReadOperationV1::CatchUp,
                self.checked_at()?,
            )
            .map_err(map_authority_error)?;
        let (trace, _) = self.verified_trace(&room_id)?;
        let view = self.view_for(
            &trace,
            trace
                .core_state()
                .membership(&member_id)
                .ok_or(BackendError::Forbidden)?,
        )?;
        let projection = CanonicalJsonV1::from_canonical_bytes(view.canonical_bytes())
            .map_err(|_| BackendError::InvalidResult)?;
        match self
            .store
            .read_observation(
                room_id.as_ref(),
                member_id.as_ref(),
                Some(after_frame_seq),
                &projection,
            )
            .map_err(map_observation_error)?
        {
            PostgresObservationDeliveryV1::Retained { frames, .. } => frames
                .into_iter()
                .map(|frame| observation_deliver(&frame, &room_id, &member_id))
                .collect(),
            PostgresObservationDeliveryV1::Reset { .. } => Err(BackendError::Busy),
        }
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
        self.verified_trace(&room_id)?;
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
        let resolution = self
            .store
            .commit_authorized_timer_fired(
                self.registry()?,
                authority,
                timer_request,
                next_core_id::<TransitionId>()?,
            )
            .map_err(map_room_commit_error)?;
        timer_response_from_resolution(timer_request, &resolution)
    }

    #[allow(clippy::too_many_lines)]
    fn launch_lobby(
        &self,
        session: &GatewaySession,
        room_id: &str,
        request: LobbyLaunchRequest,
    ) -> Result<LobbyLaunchResponse, BackendError> {
        let room_id = RoomId::from_str(room_id).map_err(|_| BackendError::Rejected)?;
        let input_id = InputId::from_str(&request.input_id).map_err(|_| BackendError::Rejected)?;
        let based_on_room_seq =
            RoomSequenceV1::new(request.based_on_room_seq).map_err(|_| BackendError::Rejected)?;
        let checked_at = self.checked_at()?;
        let mut input = ExternalInputV1 {
            source_id: SourceId::from_str(worldstream_core::HOST_LOBBY_LAUNCH_SOURCE)
                .map_err(|_| BackendError::InvalidResult)?,
            input_id,
            input_type: worldstream_core::HOST_LAUNCH_INPUT_TYPE.to_owned(),
            recorded_at: ExternalInputRecordedAt::from_str(checked_at.as_str())
                .map_err(|_| BackendError::StorageUnavailable)?,
            canonical_payload: CanonicalJsonV1::parse(br"{}")
                .map_err(|_| BackendError::InvalidResult)?,
            immutable_resource_references: Vec::new(),
        };
        let identity = worldstream_core::OperationIdentityV1::ExternalInput(Box::new(
            worldstream_core::ExternalInputOperationIdentityV1 {
                room_id: room_id.clone(),
                source_id: input.source_id.clone(),
                input_id: input.input_id.clone(),
            },
        ));
        let request_hash = external_input_request_hash(&room_id, based_on_room_seq, &input)
            .map_err(|_| BackendError::Rejected)?;
        let authenticated = self.authenticate(session)?;
        let authority = self
            .authority()
            .authorize_external_input(
                &authenticated.into_presented(),
                room_id.clone(),
                request_hash.clone(),
                checked_at,
            )
            .map_err(map_authority_error)?;
        match RoomCommitStorageV1::resolve(self.store.as_ref(), &identity, &request_hash) {
            worldstream_core::ResolveOutcomeV1::StoredResolution(result) => {
                return lobby_response_from_result(&request.input_id, &result, true);
            }
            worldstream_core::ResolveOutcomeV1::Conflict { .. } => {
                return Err(BackendError::Conflict);
            }
            worldstream_core::ResolveOutcomeV1::ResolutionUnavailable => {
                return Err(BackendError::Indeterminate);
            }
            worldstream_core::ResolveOutcomeV1::KnownAbsent => {}
        }
        let _admission = self
            .admission_lanes
            .reserve_host_stimulus(&room_id)
            .map_err(|error| map_admission_lane_error(&error))?;
        match RoomCommitStorageV1::resolve(self.store.as_ref(), &identity, &request_hash) {
            worldstream_core::ResolveOutcomeV1::StoredResolution(result) => {
                return lobby_response_from_result(&request.input_id, &result, true);
            }
            worldstream_core::ResolveOutcomeV1::Conflict { .. } => {
                return Err(BackendError::Conflict);
            }
            worldstream_core::ResolveOutcomeV1::ResolutionUnavailable => {
                return Err(BackendError::Indeterminate);
            }
            worldstream_core::ResolveOutcomeV1::KnownAbsent => {}
        }
        let (trace, _) = self.verified_trace(&room_id)?;
        if !worldstream_core::agent_heist_lobby_contract_declared(
            self.registry()?,
            trace.head().pack_digest(),
        ) || !worldstream_core::agent_heist_lobby_launch_applicable(trace.activity_state())
        {
            return Err(BackendError::WrongPhase);
        }
        let proposed_recorded_at = self.checked_at()?;
        input.recorded_at = ExternalInputRecordedAt::from_str(proposed_recorded_at.as_str())
            .map_err(|_| BackendError::StorageUnavailable)?;
        input.recorded_at = self
            .store
            .reserve_external_input_recorded_at(&identity, &request_hash, &input.recorded_at)
            .map_err(map_external_input_preparation_error)?;
        match RoomCommitStorageV1::resolve(self.store.as_ref(), &identity, &request_hash) {
            worldstream_core::ResolveOutcomeV1::StoredResolution(result) => {
                return lobby_response_from_result(&request.input_id, &result, true);
            }
            worldstream_core::ResolveOutcomeV1::Conflict { .. } => {
                return Err(BackendError::Conflict);
            }
            worldstream_core::ResolveOutcomeV1::ResolutionUnavailable => {
                return Err(BackendError::Indeterminate);
            }
            worldstream_core::ResolveOutcomeV1::KnownAbsent => {}
        }
        let (trace, _) = self.verified_trace(&room_id)?;
        if !worldstream_core::agent_heist_lobby_contract_declared(
            self.registry()?,
            trace.head().pack_digest(),
        ) || !worldstream_core::agent_heist_lobby_launch_applicable(trace.activity_state())
        {
            return Err(BackendError::WrongPhase);
        }
        let resolution = match self.store.commit_authorized_external_input(
            self.registry()?,
            authority,
            &room_id,
            based_on_room_seq,
            &input,
            next_core_id::<TransitionId>()?,
        ) {
            Ok(resolution) => resolution,
            Err(PostgresRoomCommitError::Recovery(
                worldstream_core::RoomRecoveryErrorV1::ConcurrentChange,
            )) => return Err(BackendError::WrongPhase),
            Err(error) => return Err(map_room_commit_error(error)),
        };
        lobby_response_from_resolution(&request.input_id, &resolution)
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
            room_id,
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
        match self
            .store
            .prepare_activation_claim(self.registry()?, authority, operation)
            .map_err(map_activation_error)?
        {
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

    fn scheduler_tick(&self) -> Result<(), BackendError> {
        self.store
            .reclaim_expired_activation_leases()
            .map(|_| ())
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
        if binding.capability_id != *capability_id || binding.sync.is_some() {
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

impl PostgresGatewayBackend {
    fn protocol_delivery(
        delivery: &PostgresObservationDeliveryV1,
        room_id: &RoomId,
        member_id: &worldstream_core::MemberId,
        trace: &worldstream_core::CoreTraceV1,
        verification: &worldstream_postgres::PostgresRoomVerification,
    ) -> Result<ProtocolDelivery, BackendError> {
        match delivery {
            PostgresObservationDeliveryV1::Retained {
                cursor,
                cursor_exclusive,
                frame_head,
                retained_floor,
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
            )),
            PostgresObservationDeliveryV1::Reset {
                cursor,
                frame_head,
                retained_floor,
                reset_through,
                projection_bytes,
            } => {
                let projection = projection_from_canonical_bytes(projection_bytes)?;
                let projection_hash =
                    worldstream_core::projection_hash_for_canonical_bytes(projection_bytes)
                        .map_err(|_| BackendError::InvalidResult)?
                        .to_string();
                let reset_reason = if cursor.is_none() {
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
                        room_health: verification.integrity_status.clone(),
                        integrity_generation: verification.integrity_generation,
                        baseline_frame_head: *frame_head,
                        reset_reason: reset_reason.to_owned(),
                        projection_schema: worldstream_core::PROJECTION_SCHEMA_V1.to_owned(),
                        projection,
                        projection_hash,
                    }),
                    Vec::new(),
                    *frame_head,
                    *retained_floor,
                    *cursor,
                ))
            }
        }
    }
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

fn lobby_response_from_result(
    input_id: &str,
    result: &StoredSemanticResultV1,
    duplicate: bool,
) -> Result<LobbyLaunchResponse, BackendError> {
    let SemanticResultV1::TransitionCommitted {
        room_id,
        transition_id,
        complete_head,
        ..
    } = result.result()
    else {
        return Err(BackendError::InvalidResult);
    };
    Ok(LobbyLaunchResponse {
        room_id: room_id.to_string(),
        input_id: input_id.to_owned(),
        transition_id: transition_id.to_string(),
        room_head: room_head(complete_head),
        duplicate,
    })
}

fn lobby_response_from_resolution(
    input_id: &str,
    resolution: &RoomCommitResolutionV1,
) -> Result<LobbyLaunchResponse, BackendError> {
    if let Some(result) = resolution.stored_result() {
        return lobby_response_from_result(input_id, result, resolution.duplicate());
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

#[allow(clippy::needless_pass_by_value)]
fn map_observation_error(error: PostgresObservationError) -> BackendError {
    match error {
        PostgresObservationError::FutureCursor => BackendError::Rejected,
        PostgresObservationError::Corrupt
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
    }
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
    };

    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        ActionReply, ActionSubmit, AttachReply, BackendError, GatewayBackend, GatewaySession,
        MAX_DSN_BYTES, MemberCapabilityIssueRequest, ObservationAck, PostgresGatewayBackend,
        RoomAttach, RoomSyncAck, creation_time, map_admission_lane_error, read_postgres_dsn,
    };
    use worldstream_core::{
        AdmissionLaneErrorV1, AuthorityBootstrapV1, AuthorityCheckedAt, AuthorityV1,
        CapabilityBearerV1, CapabilityScopeV1, PrincipalKindV1, agent_heist_lobby_digest,
        builtin_agent_heist_registry, builtin_counter_registry, counter_v2_digest,
    };
    use worldstream_postgres::{
        PostgresConnectionConfig, PostgresConnectionPath, PostgresRoomStore,
    };
    use worldstream_protocol::{
        AccessMode, BearerWireV1, CreateMember, CreateRoomRequest, LobbyLaunchRequest,
        MemberCapabilityProvisionRequestV1, OperatorActivityPhase, OperatorRoomIntegrityStatus,
        OperatorRoomInventoryRequest, PackReference, PrincipalKind,
        RunnerCapabilityProvisionRequestV1,
    };
    use worldstream_runtime::SecretSource;

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
        let backend = PostgresGatewayBackend::new(store);
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
            "commit_authorized_timer_fired(",
            ".reserve_action(&action_room_id, self.host_clock.as_ref())",
            ".reserve_host_stimulus(&room_id)",
            "offer_activations_authorized(",
            "prepare_activation_claim(",
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
    #[allow(clippy::too_many_lines)]
    fn live_postgres_gateway_counter_workflow_uses_production_backend() {
        let Some(path) = env::var_os("WORLDSTREAM_POSTGRES_GATEWAY_DSN_FILE") else {
            return;
        };
        let dsn = read_postgres_dsn(&SecretSource::File(path.into()))
            .unwrap_or_else(|error| unreachable!("live runtime DSN file: {error}"));
        let config = PostgresConnectionConfig::runtime(dsn.clone(), PostgresConnectionPath::Direct)
            .unwrap_or_else(|error| unreachable!("runtime config: {error}"));
        let store = PostgresRoomStore::new(config)
            .unwrap_or_else(|error| unreachable!("runtime store: {error}"));
        let backend = PostgresGatewayBackend::new(store);
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
        };
        let lobby_first = backend
            .launch_lobby(&host, &lobby_room.room_id, lobby_request.clone())
            .unwrap_or_else(|error| unreachable!("launch Lobby: {error:?}"));
        assert_eq!(lobby_first.room_head.room_seq, 1);
        assert!(!lobby_first.duplicate);

        drop(backend);
        let restarted = PostgresGatewayBackend::new(
            PostgresRoomStore::new(
                PostgresConnectionConfig::runtime(dsn.clone(), PostgresConnectionPath::Direct)
                    .unwrap_or_else(|error| unreachable!("restart config: {error}")),
            )
            .unwrap_or_else(|error| unreachable!("restart store: {error}")),
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
            "LIVE_POSTGRES_GATEWAY=PASS create+duplicate+conflict+projection+replay+attach+sync+resync+action+stale+live+ack+restart+lobby-launch+sealed-provision-replay-conflict"
        );
    }
}
