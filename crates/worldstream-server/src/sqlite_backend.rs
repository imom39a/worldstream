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
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use worldstream_core::{
    AccessModeV1, ActionOfferWitnessV1, ActivationIntentStateV1, ActivationOperationRequestV1,
    ActivationResultCodeV1, AdministrationOperationIdentityV1, AdmissionLaneErrorV1,
    AuthorityChangeId, AuthorityChangeV1, AuthorityCheckedAt, AuthorityErrorV1,
    AuthoritySnapshotQueryV1, AuthorityStoreV1, AuthorityV1, AuthorizedRunnerControlV1,
    AuthorizedTimerFiredV1, CORE_OPERATION_KIND, CREATE_ROOM_OPERATION_KIND, CanonicalJsonV1,
    CanonicalRoomTraceCache, CapabilityBearerV1, CapabilityExpiresAt, CapabilityId,
    CapabilityProfileV1, CapabilityScopeSetV1, CoreAdministrationIngressV1,
    CoreAdministrationRequestV1, CoreChangeSetV1, CoreProposedKindV1, CoreRecordedAt,
    CreationRecordedAt, DiagnosticOperationV1, DiagnosticTargetV1, ExternalInputRecordedAt,
    HistoricalReplayErrorV1, HostClockErrorV1, HostClockSampleV1, HostClockV1,
    InitialMembershipProposalV1, MemberReadOperationV1, MembershipStandingV1, MembershipV1,
    MonotonicHostClockV1, NewCapabilityV1, PackDigestV1, PackGenesisRequestV1, PackRegistryV1,
    PackViewerV1, ParticipantActionIngressErrorV1, ParticipantActionIngressV1,
    ParticipantActionRequestV1, PreparedRoomCreationV1, PreparedRoomWriteV1, PresentedCapabilityV1,
    PrincipalAuthorityStatusV1, PrincipalKindV1, ReceiptSemanticInputV1, ReceiptSemanticTimeV1,
    ReplayProjectionKindV1, RoomAdmissionLanesV1, RoomCommitResolutionV1, RoomCommitStorageV1,
    RoomCreationIngressV1, RoomCreationRequestV1, RoomId, RoomMembershipKeyV1, RoomSeedV1,
    RoomSequenceV1, RoomTraceCacheErrorV1, RunnerControlOperationV1, RunnerId,
    RunnerMembershipSetV1, SemanticResultV1, SessionErrorV1, SessionFrameV1, SessionSyncTokenV1,
    SessionV1, StoredSemanticResultV1, TimerFiredRequestV1, TimerGenerationV1, TimerId,
    TransitionId, authorize_core_administration_operation, authorize_participant_action_operation,
    authorize_room_creation_operation,
};
use worldstream_protocol::{
    AccessMode, ActionAccepted, ActionOffer, ActionRejected, ActionSubmit, ActivationClaim,
    ActivationDelivery, ActivationFrame, ActivationIntentState, ActivationLeaseOperation,
    ActivationOffer, ActivationOfferRequest, ActivationOffers, ActivationOperationReply,
    ActivationResultCode, BearerWireV1, ClientHello, CreateRoomRequest, CreateRoomResponse,
    HISTORICAL_EVIDENCE_RESPONSE_VERSION, HOSTED_ROOM_CREATION_RESPONSE_SCHEMA_V2,
    HOSTED_ROOM_CREATION_SCHEMA_V2, HistoricalEvidenceOutcomeV1, HistoricalEvidenceReferenceV1,
    HostedRoomCreationRequestV2, HostedRoomCreationResponseV2, HostedSpectatorCredentialReceiptV2,
    HostedSpectatorPurposeV2, LobbyLaunchRequest, LobbyLaunchResponse, MAX_MESSAGE_BYTES,
    MemberCapabilityProvisionRequestV1, MemberCapabilityProvisionResponseV1,
    OPERATOR_ACTIVATION_STATUS_VERSION, ObservationAck, ObservationDeliver,
    OperatorActivationStatusV1, OperatorActivityPhase, OperatorBackupProfileStatus,
    OperatorBackupStorageHealth, OperatorBackupStorageProfile, OperatorBackupVerification,
    OperatorDataFreshness, OperatorLiveBackupArtifactSummary, OperatorLiveBackupPrepareRequest,
    OperatorLiveBackupStatus, OperatorRoomIntegrity, OperatorRoomIntegrityStatus,
    OperatorRoomInventoryPage, OperatorRoomInventoryRequest, OperatorRoomSummary, PROTOCOL_VERSION,
    PackReference, Principal, PrincipalKind, Projection, ProjectionReset, ProjectionResponse,
    ROOM_ARCHIVE_RESPONSE_SCHEMA_V1, ReplayResponse, RoomArchiveRequestV1, RoomArchiveResponseV1,
    RoomAttach, RoomAttached, RoomHead, RoomSyncAck, RunnerCapabilityProvisionRequestV1,
    RunnerCapabilityProvisionResponseV1, RunnerHello, RunnerReady, ServerWelcome, SyncBranch,
    TimerFireRequest, TimerFireResponse,
};
use worldstream_runtime::prepare_data_directory;
use worldstream_sqlite::{
    SqliteActivationErrorV1, SqliteAuthenticatedCapabilityV1, SqliteAuthorizedReplayErrorV1,
    SqliteAuthorizedReplayOutcomeV1, SqliteCanonicalRoomRecovery,
    SqliteExternalInputPreparationErrorV1, SqliteGatewayErrorV1, SqliteHistoricalEvidenceErrorV1,
    SqliteObservationDeliveryV1, SqliteObservationErrorV1, SqliteObservationFrameV1,
    SqliteObservationResetReasonV1, SqliteParticipantActionErrorV1, SqliteRoomDiagnosticErrorV1,
    SqliteRoomDiagnosticSummaryV1, SqliteRoomRuntimeStateV1, SqliteRoomStore,
    SqliteRoomSupervisorErrorV1, SqliteRoomSupervisorLeaseV1, SqliteRoomSupervisorOperationV1,
    SqliteRoomSupervisorV1, SqliteTimerCommitErrorV1, SqliteTimerStateV1,
};

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
    bindings: Arc<SessionBindings>,
    #[cfg(test)]
    live_payload_page_reads: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    live_payload_records_read: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    live_payload_bytes_read: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    independent_live_pages_for_measurement: std::sync::atomic::AtomicBool,
    supervisor: Arc<SqliteRoomSupervisorV1>,
    recoveries: Mutex<BTreeMap<String, CatchingUpRoom>>,
    host_clock: Arc<dyn HostClockV1>,
    admission_lanes: RoomAdmissionLanesV1,
    live_backup_root: Option<PathBuf>,
    timer_authority: Option<PresentedCapabilityV1>,
    last_timer_room: Mutex<Option<RoomId>>,
    last_retention_room: Mutex<Option<RoomId>>,
    last_retention_member: Mutex<Option<worldstream_core::MemberId>>,
    last_retention_at: Mutex<Option<Instant>>,
    trace_cache: CanonicalRoomTraceCache,
    #[cfg(test)]
    recovery_forbidden: std::sync::atomic::AtomicBool,
}

struct CatchingUpRoom {
    lease: SqliteRoomSupervisorLeaseV1,
    recovery: SqliteCanonicalRoomRecovery,
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

impl fmt::Debug for SqliteGatewayBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteGatewayBackend")
            .field("store", &"[OPAQUE]")
            .field("registry", &"[OPAQUE]")
            .field("bindings", &self.bindings)
            .field("supervisor", &self.supervisor)
            .field("recoveries", &"[OPAQUE]")
            .field(
                "timer_authority",
                &self.timer_authority.as_ref().map(|_| "[OPAQUE]"),
            )
            .field("last_timer_room", &"[OPAQUE]")
            .field("host_clock", &"[OPAQUE]")
            .field("admission_lanes", &self.admission_lanes)
            .field(
                "live_backup_root",
                &self.live_backup_root.as_ref().map(|_| "[OPAQUE]"),
            )
            .finish_non_exhaustive()
    }
}

impl SqliteGatewayBackend {
    #[cfg(test)]
    pub(crate) fn live_payload_page_reads_for_test(&self) -> usize {
        self.live_payload_page_reads
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Wires an already-open `SQLite` store and an already-validated pack
    /// registry. The server owns neither Core Genesis preparation nor action
    /// coordination, so those operations remain fail closed below.
    #[must_use]
    pub fn new(store: SqliteRoomStore, registry: Arc<PackRegistryV1>) -> Self {
        Self::with_host_clock(
            store,
            registry,
            Arc::new(MonotonicHostClockV1::new(RuntimeWallClock)),
        )
    }

    /// Wires the gateway to one application-owned semantic `HostClock`.
    ///
    /// The same monotonic clock is used for Action admission and restart
    /// catch-up cutoffs. Callers providing a raw wall source should wrap it in
    /// [`MonotonicHostClockV1`].
    #[must_use]
    pub fn with_host_clock(
        store: SqliteRoomStore,
        registry: Arc<PackRegistryV1>,
        host_clock: Arc<dyn HostClockV1>,
    ) -> Self {
        Self::with_runtime(store, registry, host_clock, RoomAdmissionLanesV1::default())
    }

    /// Enables live backup publication beneath one startup-fixed owner-only
    /// root. Operator requests can select only a stable operation ID.
    ///
    /// # Errors
    ///
    /// Returns `StorageUnavailable` when the root cannot be securely prepared.
    pub fn with_live_backup_root(mut self, root: &Path) -> Result<Self, BackendError> {
        self.live_backup_root =
            Some(prepare_data_directory(root).map_err(|_| BackendError::StorageUnavailable)?);
        Ok(self)
    }

    /// Enables automatic timers using existing protected installation authority.
    /// Every firing rechecks current authority through the ordinary Core grant.
    ///
    /// # Errors
    /// Returns a closed backend error if the supplied authority cannot authenticate.
    pub fn with_timer_authority(
        mut self,
        bearer: CapabilityBearerV1,
    ) -> Result<Self, BackendError> {
        self.ensure_source_authoritative()?;
        self.timer_authority = Some(
            self.store
                .authenticate_bearer(bearer)
                .map_err(|error| map_gateway_error(&error))?
                .into_presented(),
        );
        Ok(self)
    }

    #[cfg(test)]
    pub(crate) fn with_test_admission_lanes(
        store: SqliteRoomStore,
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
        store: SqliteRoomStore,
        registry: Arc<PackRegistryV1>,
        host_clock: Arc<dyn HostClockV1>,
        admission_lanes: RoomAdmissionLanesV1,
    ) -> Self {
        Self {
            store,
            registry,
            bindings: Arc::new(SessionBindings::default()),
            #[cfg(test)]
            live_payload_page_reads: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            live_payload_records_read: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            live_payload_bytes_read: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            independent_live_pages_for_measurement: std::sync::atomic::AtomicBool::new(false),
            supervisor: Arc::new(SqliteRoomSupervisorV1::default()),
            recoveries: Mutex::new(BTreeMap::new()),
            host_clock,
            admission_lanes,
            live_backup_root: None,
            timer_authority: None,
            last_timer_room: Mutex::new(None),
            last_retention_room: Mutex::new(None),
            last_retention_member: Mutex::new(None),
            last_retention_at: Mutex::new(None),
            trace_cache: CanonicalRoomTraceCache::default(),
            #[cfg(test)]
            recovery_forbidden: std::sync::atomic::AtomicBool::new(false),
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

    fn commit_cached_participant_action(
        &self,
        room_id: &RoomId,
        authority: worldstream_core::ParticipantActionAuthorityV1,
        request: &ParticipantActionRequestV1,
        admitted_at: worldstream_core::ActionAdmittedAt,
        transition_id: TransitionId,
    ) -> Result<RoomCommitResolutionV1, BackendError> {
        self.trace_cache
            .with_room(room_id, |slot| {
                let mut fence = self
                    .store
                    .current_room_serving_fence(room_id)
                    .map_err(|error| map_gateway_error(&error))?
                    .ok_or(BackendError::NotFound)?;
                let cache_matches = slot.as_ref().is_some_and(|cached| {
                    cached.trace().head() == fence.head() && cached.integrity() == fence.integrity()
                });
                if !cache_matches {
                    *slot = None;
                    let snapshot = self
                        .store
                        .gateway_canonical_room_snapshot(&self.registry, room_id)
                        .map_err(|error| map_gateway_error(&error))?
                        .ok_or(BackendError::NotFound)?;
                    let (trace, integrity, _) = snapshot.into_parts();
                    let current = self
                        .store
                        .current_room_serving_fence(room_id)
                        .map_err(|error| map_gateway_error(&error))?
                        .ok_or(BackendError::NotFound)?;
                    if trace.head() != current.head() || integrity != *current.integrity() {
                        return Err(BackendError::Busy);
                    }
                    *slot = Some(worldstream_core::CachedCanonicalRoomTrace::new(
                        trace, integrity,
                    ));
                    fence = current;
                }
                let cached = slot.as_mut().ok_or(BackendError::StorageUnavailable)?;
                let integrity = cached.integrity().clone();
                let resolution = self
                    .store
                    .commit_authorized_participant_action_from_canonical_serving_trace(
                        authority,
                        request,
                        admitted_at,
                        transition_id,
                        cached.trace_mut(),
                        &integrity,
                        fence.frame_heads(),
                    )
                    .map_err(|error| map_participant_action_error(&error));
                cached.trace_mut().discard_persisted_history();
                if matches!(
                    resolution,
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
            .map_err(|error| match error {
                RoomTraceCacheErrorV1::Busy | RoomTraceCacheErrorV1::Poisoned => BackendError::Busy,
                RoomTraceCacheErrorV1::InvalidCapacity => BackendError::StorageUnavailable,
            })?
    }

    fn commit_cached_timer(
        &self,
        room_id: &RoomId,
        authority: AuthorizedTimerFiredV1,
        request: &TimerFiredRequestV1,
        transition_id: TransitionId,
    ) -> Result<RoomCommitResolutionV1, BackendError> {
        self.trace_cache
            .with_room(room_id, |slot| {
                let mut fence = self
                    .store
                    .current_room_serving_fence(room_id)
                    .map_err(|error| map_gateway_error(&error))?
                    .ok_or(BackendError::NotFound)?;
                let cache_matches = slot.as_ref().is_some_and(|cached| {
                    cached.trace().head() == fence.head() && cached.integrity() == fence.integrity()
                });
                if !cache_matches {
                    *slot = None;
                    let snapshot = self
                        .store
                        .gateway_canonical_room_snapshot(&self.registry, room_id)
                        .map_err(|error| map_gateway_error(&error))?
                        .ok_or(BackendError::NotFound)?;
                    let (trace, integrity, _) = snapshot.into_parts();
                    let current = self
                        .store
                        .current_room_serving_fence(room_id)
                        .map_err(|error| map_gateway_error(&error))?
                        .ok_or(BackendError::NotFound)?;
                    if trace.head() != current.head() || integrity != *current.integrity() {
                        return Err(BackendError::Busy);
                    }
                    *slot = Some(worldstream_core::CachedCanonicalRoomTrace::new(
                        trace, integrity,
                    ));
                    fence = current;
                }
                let cached = slot.as_mut().ok_or(BackendError::StorageUnavailable)?;
                let integrity = cached.integrity().clone();
                let resolution = self
                    .store
                    .commit_authorized_timer_fired_from_canonical_serving_trace(
                        authority,
                        request,
                        transition_id,
                        cached.trace_mut(),
                        &integrity,
                        fence.frame_heads(),
                    )
                    .map_err(|error| map_timer_commit_error(&error));
                cached.trace_mut().discard_persisted_history();
                if matches!(
                    resolution,
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
            .map_err(|error| match error {
                RoomTraceCacheErrorV1::Busy | RoomTraceCacheErrorV1::Poisoned => BackendError::Busy,
                RoomTraceCacheErrorV1::InvalidCapacity => BackendError::StorageUnavailable,
            })?
    }

    fn commit_cached_core_administration(
        &self,
        room_id: &RoomId,
        authority: worldstream_core::AuthorizedCoreAdministrationV1,
        request: &CoreAdministrationRequestV1,
        recorded_at: CoreRecordedAt,
        transition_id: TransitionId,
    ) -> Result<RoomCommitResolutionV1, BackendError> {
        self.trace_cache
            .with_room(room_id, |slot| {
                let mut fence = self
                    .store
                    .current_room_serving_fence(room_id)
                    .map_err(|error| map_gateway_error(&error))?
                    .ok_or(BackendError::NotFound)?;
                let cache_matches = slot.as_ref().is_some_and(|cached| {
                    cached.trace().head() == fence.head() && cached.integrity() == fence.integrity()
                });
                if !cache_matches {
                    *slot = None;
                    let snapshot = self
                        .store
                        .gateway_canonical_room_snapshot(&self.registry, room_id)
                        .map_err(|error| map_gateway_error(&error))?
                        .ok_or(BackendError::NotFound)?;
                    let (trace, integrity, _) = snapshot.into_parts();
                    let current = self
                        .store
                        .current_room_serving_fence(room_id)
                        .map_err(|error| map_gateway_error(&error))?
                        .ok_or(BackendError::NotFound)?;
                    if trace.head() != current.head() || integrity != *current.integrity() {
                        return Err(BackendError::Busy);
                    }
                    *slot = Some(worldstream_core::CachedCanonicalRoomTrace::new(
                        trace, integrity,
                    ));
                    fence = current;
                }
                let cached = slot.as_mut().ok_or(BackendError::StorageUnavailable)?;
                let integrity = cached.integrity().clone();
                let resolution = self
                    .store
                    .commit_authorized_core_administration_from_canonical_serving_trace(
                        authority,
                        request,
                        recorded_at,
                        transition_id,
                        cached.trace_mut(),
                        &integrity,
                        fence.frame_heads(),
                    )
                    .map_err(|error| map_timer_commit_error(&error));
                cached.trace_mut().discard_persisted_history();
                if matches!(
                    resolution,
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
            .map_err(|error| match error {
                RoomTraceCacheErrorV1::Busy | RoomTraceCacheErrorV1::Poisoned => BackendError::Busy,
                RoomTraceCacheErrorV1::InvalidCapacity => BackendError::StorageUnavailable,
            })?
    }

    fn commit_cached_external_input(
        &self,
        room_id: &RoomId,
        authority: worldstream_core::AuthorizedExternalInputV1,
        based_on_room_seq: RoomSequenceV1,
        input: &worldstream_core::ExternalInputV1,
        transition_id: TransitionId,
    ) -> Result<RoomCommitResolutionV1, BackendError> {
        self.trace_cache
            .with_room(room_id, |slot| {
                let mut fence = self
                    .store
                    .current_room_serving_fence(room_id)
                    .map_err(|error| map_gateway_error(&error))?
                    .ok_or(BackendError::NotFound)?;
                let cache_matches = slot.as_ref().is_some_and(|cached| {
                    cached.trace().head() == fence.head() && cached.integrity() == fence.integrity()
                });
                if !cache_matches {
                    *slot = None;
                    let snapshot = self
                        .store
                        .gateway_canonical_room_snapshot(&self.registry, room_id)
                        .map_err(|error| map_gateway_error(&error))?
                        .ok_or(BackendError::NotFound)?;
                    let (trace, integrity, _) = snapshot.into_parts();
                    let current = self
                        .store
                        .current_room_serving_fence(room_id)
                        .map_err(|error| map_gateway_error(&error))?
                        .ok_or(BackendError::NotFound)?;
                    if trace.head() != current.head() || integrity != *current.integrity() {
                        return Err(BackendError::Busy);
                    }
                    *slot = Some(worldstream_core::CachedCanonicalRoomTrace::new(
                        trace, integrity,
                    ));
                    fence = current;
                }
                let cached = slot.as_mut().ok_or(BackendError::StorageUnavailable)?;
                let integrity = cached.integrity().clone();
                let resolution = self
                    .store
                    .commit_authorized_external_input_from_canonical_serving_trace(
                        authority,
                        room_id,
                        based_on_room_seq,
                        input,
                        transition_id,
                        cached.trace_mut(),
                        &integrity,
                        fence.frame_heads(),
                    )
                    .map_err(|error| map_timer_commit_error(&error));
                cached.trace_mut().discard_persisted_history();
                if matches!(
                    resolution,
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
            .map_err(|error| match error {
                RoomTraceCacheErrorV1::Busy | RoomTraceCacheErrorV1::Poisoned => BackendError::Busy,
                RoomTraceCacheErrorV1::InvalidCapacity => BackendError::StorageUnavailable,
            })?
    }

    /// Borrows the single current executor for a read that must not rebuild
    /// canonical history. The durable serving fence is checked before every
    /// use and after recovery before installation; callers receive only a
    /// borrow and cannot create a second execution lane.
    fn with_cached_current_trace<R>(
        &self,
        room_id: &RoomId,
        operation: impl FnOnce(
            &worldstream_core::CanonicalRoomTrace,
            &worldstream_core::RoomIntegrityStateV1,
        ) -> Result<R, BackendError>,
    ) -> Result<R, BackendError> {
        // Preserve the readable-snapshot lifecycle gate: healthy Rooms must
        // stay inside their active supervisor generation while a cached trace
        // is borrowed; Faulted Rooms remain readable only through their
        // verified storage fence, and quarantined Rooms fail closed.
        let room_operation = match self
            .store
            .room_integrity_state(room_id)
            .map_err(|_| BackendError::StorageUnavailable)?
            .map(|integrity| integrity.status())
        {
            Some(worldstream_core::RoomIntegrityStatusV1::Quarantined) => {
                return Err(BackendError::RoomQuarantined);
            }
            Some(worldstream_core::RoomIntegrityStatusV1::Faulted) => None,
            Some(worldstream_core::RoomIntegrityStatusV1::Healthy) | None => {
                self.ensure_verified_active(room_id)?;
                Some(self.acquire_room_operation(room_id)?)
            }
        };
        let result = self
            .trace_cache
            .with_room(room_id, |slot| {
                let fence = self
                    .store
                    .current_room_serving_fence(room_id)
                    .map_err(|error| map_gateway_error(&error))?
                    .ok_or(BackendError::NotFound)?;
                if !slot.as_ref().is_some_and(|cached| {
                    cached.trace().head() == fence.head() && cached.integrity() == fence.integrity()
                }) {
                    #[cfg(test)]
                    if self
                        .recovery_forbidden
                        .load(std::sync::atomic::Ordering::Relaxed)
                    {
                        return Err(BackendError::StorageUnavailable);
                    }
                    *slot = None;
                    let snapshot = self
                        .store
                        .gateway_canonical_room_snapshot(&self.registry, room_id)
                        .map_err(|error| map_gateway_error(&error))?
                        .ok_or(BackendError::NotFound)?;
                    let (trace, integrity, _) = snapshot.into_parts();
                    let current = self
                        .store
                        .current_room_serving_fence(room_id)
                        .map_err(|error| map_gateway_error(&error))?
                        .ok_or(BackendError::NotFound)?;
                    if trace.head() != current.head() || integrity != *current.integrity() {
                        return Err(BackendError::Busy);
                    }
                    *slot = Some(worldstream_core::CachedCanonicalRoomTrace::new(
                        trace, integrity,
                    ));
                }
                let cached = slot.as_ref().ok_or(BackendError::StorageUnavailable)?;
                operation(cached.trace(), cached.integrity())
            })
            .map_err(|error| match error {
                RoomTraceCacheErrorV1::Busy | RoomTraceCacheErrorV1::Poisoned => BackendError::Busy,
                RoomTraceCacheErrorV1::InvalidCapacity => BackendError::StorageUnavailable,
            });
        if let Some(room_operation) = room_operation {
            room_operation
                .publish()
                .map_err(Self::map_supervisor_error)?;
        }
        result?
    }

    // Observation retention is intentionally decoupled from the timer loop's
    // per-Room work. One five-second pass advances one Room and the store
    // advances at most eight Memberships, so a large deployment cannot turn a
    // scheduler tick into a full historical scan.
    fn scheduled_observation_retention(&self, rooms: &[RoomId]) -> Result<(), BackendError> {
        if rooms.is_empty() {
            return Ok(());
        }
        {
            let mut last = self
                .last_retention_at
                .lock()
                .map_err(|_| BackendError::StorageUnavailable)?;
            let now = Instant::now();
            if last.is_some_and(|previous| now.duration_since(previous) < Duration::from_secs(5)) {
                return Ok(());
            }
            *last = Some(now);
        }
        let mut room_cursor = self
            .last_retention_room
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?;
        let mut member_cursor = self
            .last_retention_member
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?;
        let room_id = if member_cursor.is_some()
            && room_cursor
                .as_ref()
                .is_some_and(|current| rooms.contains(current))
        {
            room_cursor
                .clone()
                .ok_or(BackendError::StorageUnavailable)?
        } else {
            *member_cursor = None;
            let index = room_cursor.as_ref().map_or(0, |previous| {
                let next = rooms.partition_point(|room| room.as_str() <= previous.as_str());
                if next == rooms.len() { 0 } else { next }
            });
            let room_id = rooms[index].clone();
            *room_cursor = Some(room_id.clone());
            room_id
        };
        drop(room_cursor);
        if self.verified_room_lifecycle(&room_id)? == SqliteRoomRuntimeStateV1::Active {
            let (pruned, next_member) = self
                .store
                .enforce_observation_retention_after(
                    &room_id,
                    &Self::checked_at()?,
                    member_cursor.as_ref(),
                )
                .map_err(map_observation_error)?;
            *member_cursor = next_member;
            if pruned > 0 {
                // Keep draining bounded pages while work remains; the
                // five-second delay is for idle maintenance passes.
                *self
                    .last_retention_at
                    .lock()
                    .map_err(|_| BackendError::StorageUnavailable)? = None;
            }
        }
        Ok(())
    }

    fn ensure_source_authoritative(&self) -> Result<(), BackendError> {
        if self.store.source_transfer_state()
            == worldstream_sqlite::SqliteSourceTransferStateV1::SourceAuthoritative
        {
            Ok(())
        } else {
            Err(BackendError::StorageUnavailable)
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
        let Ok(recovery) = self.store.recover_canonical_room_catching_up(
            &self.registry,
            room_id,
            self.host_clock.as_ref(),
        ) else {
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

    #[cfg(test)]
    fn forbid_recovery_for_test(&self) {
        self.recovery_forbidden
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Reads the active supervisor gate and bounded Timer ledger without
    /// rebuilding the Room trace. Activation claims already hold the serving
    /// trace cache borrow, so consulting restart recovery here would replay
    /// the complete canonical history on every claim.
    fn cached_runtime_state(
        &self,
        room_id: &RoomId,
    ) -> Result<SqliteRoomRuntimeStateV1, BackendError> {
        let lifecycle = self.verified_room_lifecycle(room_id)?;
        if lifecycle != SqliteRoomRuntimeStateV1::Active {
            return Ok(lifecycle);
        }
        let cutoff = self
            .host_clock
            .sample()
            .map_err(|_| BackendError::StorageUnavailable)?;
        if self
            .store
            .next_due_timer(room_id, &cutoff)
            .map_err(|_| BackendError::StorageUnavailable)?
            .is_some()
        {
            Ok(SqliteRoomRuntimeStateV1::CatchingUp)
        } else {
            Ok(SqliteRoomRuntimeStateV1::Active)
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
            let fence = self
                .store
                .current_room_serving_fence(room_id)
                .map_err(|error| map_gateway_error(&error))?
                .ok_or(BackendError::NotFound)?;
            if entry.recovery.trace().head() != fence.head() {
                return Err(BackendError::Busy);
            }
            let resolution = self
                .store
                .commit_authorized_timer_fired_from_canonical_serving_trace(
                    authority,
                    request,
                    transition_id,
                    entry.recovery.trace_mut(),
                    fence.integrity(),
                    fence.frame_heads(),
                )
                .map_err(|error| map_timer_commit_error(&error))?;
            entry.recovery.trace_mut().discard_persisted_history();
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

    fn commit_due_timer(
        &self,
        room_id: &RoomId,
        authority: AuthorizedTimerFiredV1,
        request: &TimerFiredRequestV1,
        operation: Option<SqliteRoomSupervisorOperationV1>,
    ) -> Result<TimerFireResponse, BackendError> {
        let Some(operation) = operation else {
            return self.commit_catching_up_timer(room_id, authority, request);
        };
        let _admission = self
            .admission_lanes
            .reserve_host_stimulus(room_id)
            .map_err(|error| map_admission_lane_error(&error))?;
        let transition_id = next_core_id::<TransitionId>()?;
        let resolution = self.commit_cached_timer(room_id, authority, request, transition_id)?;
        operation.publish().map_err(Self::map_supervisor_error)?;
        timer_response_from_resolution(request, &resolution)
    }

    fn scheduled_timer_slice(
        &self,
        room_id: &RoomId,
        remaining: &mut usize,
        changed: &mut bool,
    ) -> Result<(), BackendError> {
        let Some(presented) = &self.timer_authority else {
            return Ok(());
        };
        let cutoff = self
            .host_clock
            .sample()
            .map_err(|_| BackendError::StorageUnavailable)?;
        for _ in 0..8 {
            if *remaining == 0 {
                break;
            }
            let lifecycle = self.verified_room_lifecycle(room_id)?;
            let request = if lifecycle == SqliteRoomRuntimeStateV1::CatchingUp {
                self.recoveries
                    .lock()
                    .map_err(|_| BackendError::StorageUnavailable)?
                    .get_mut(room_id.as_str())
                    .ok_or(BackendError::Busy)?
                    .recovery
                    .next_due_timer(&self.store)
                    .map_err(|_| BackendError::StorageUnavailable)?
            } else {
                self.store
                    .next_due_timer(room_id, &cutoff)
                    .map_err(|_| BackendError::StorageUnavailable)?
            };
            let Some(request) = request else {
                break;
            };
            let operation = if lifecycle == SqliteRoomRuntimeStateV1::Active {
                Some(self.acquire_room_operation(room_id)?)
            } else {
                None
            };
            let authority = self
                .authority()
                .authorize_timer_fired(presented, &request, Self::checked_at()?)
                .map_err(map_authority_error)?;
            *remaining -= 1;
            self.commit_due_timer(room_id, authority, &request, operation)?;
            *changed = true;
        }
        Ok(())
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
    ) -> Result<Option<worldstream_sqlite::SqliteCanonicalGatewayRoomSnapshot>, BackendError> {
        self.ensure_verified_active(room_id)?;
        let operation = self.acquire_room_operation(room_id)?;
        let result = self
            .store
            .gateway_canonical_room_snapshot(&self.registry, room_id);
        operation.publish().map_err(Self::map_supervisor_error)?;
        result.map_err(|error| map_gateway_error(&error))
    }

    fn readable_room_snapshot(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<worldstream_sqlite::SqliteCanonicalGatewayRoomSnapshot>, BackendError> {
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
                .gateway_canonical_room_snapshot(&self.registry, room_id)
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
        self.ensure_source_authoritative()?;
        let bearer = session
            .owned_bearer()
            .ok_or(BackendError::StorageUnavailable)?;
        let authenticated = self
            .store
            .authenticate_bearer(bearer)
            .map_err(|error| map_gateway_error(&error))?;
        Ok(authenticated)
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
                Self::checked_at()?,
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
            .revalidate_current(&snapshot, &Self::checked_at()?)
            .map_err(map_authority_error)?;
        Ok(())
    }

    fn authorize_runner_presence(&self, session: &GatewaySession) -> Result<(), BackendError> {
        let authenticated = self.authenticate(session)?;
        let checked_at = Self::checked_at()?;
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
        let checked_at = Self::checked_at()?;
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

        let snapshot = self
            .supervised_room_snapshot(&room_id)?
            .ok_or(BackendError::NotFound)?;
        let membership = snapshot
            .trace()
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
            Self::checked_at()?,
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
                Self::checked_at()?,
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
        let retained_pack = self
            .registry
            .load_retained(replay.verified_head().pack_digest())
            .map_err(|_| BackendError::InvalidResult)?;
        let descriptor = retained_pack.descriptor();
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
        let principal_id = authenticated.principal_id().clone();
        let presented = authenticated.into_presented();
        let (head, integrity, view) =
            self.with_cached_current_trace(&room_id, |trace, integrity| {
                let membership = trace
                    .core_state()
                    .memberships()
                    .values()
                    .find(|membership| {
                        membership.principal_id() == &principal_id
                            && membership.standing()
                                == worldstream_core::MembershipStandingV1::Enabled
                    })
                    .ok_or(BackendError::Forbidden)?;
                self.authority()
                    .authorize_member_read(
                        &presented,
                        room_id.clone(),
                        membership.member_id().clone(),
                        MemberReadOperationV1::CurrentProjection,
                        Self::checked_at()?,
                    )
                    .map_err(map_authority_error)?;
                let view = trace
                    .view(&viewer_for(membership))
                    .map_err(|_| BackendError::InvalidResult)?;
                Ok((trace.head().clone(), integrity.clone(), view))
            })?;
        let current = self
            .store
            .current_room_serving_fence(&room_id)
            .map_err(|error| map_gateway_error(&error))?
            .ok_or(BackendError::NotFound)?;
        if current.head() != &head || current.integrity() != &integrity {
            return Err(BackendError::Busy);
        }
        let projection = projection_from_view(&view)?;
        let projection_hash = view
            .projection_hash()
            .map_err(|_| BackendError::InvalidResult)?
            .to_string();
        Ok(ProjectionResponse {
            room_id: room_id.to_string(),
            room_head: room_head(&head),
            room_health: integrity_status(integrity.status()),
            integrity_generation: integrity.generation().get(),
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
        let principal_id = authenticated.principal_id().clone();
        let presented = authenticated.into_presented();
        let capability_id = presented.capability_id().clone();
        self.with_cached_current_trace(room_id, |trace, integrity| {
            let membership = trace
                .core_state()
                .membership(member_id)
                .filter(|membership| {
                    membership.principal_id() == &principal_id
                        && membership.standing() == worldstream_core::MembershipStandingV1::Enabled
                })
                .ok_or(BackendError::Forbidden)?
                .clone();
            let view = trace
                .view(&viewer_for(&membership))
                .map_err(|_| BackendError::InvalidResult)?;
            Ok(AttachContext {
                head: trace.head().clone(),
                integrity: integrity.clone(),
                room_status: trace.core_state().room_status(),
                view,
                membership,
                capability_id,
                presented,
            })
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
                installed_reset_generation: captured.reset_generation(),
                session: core_session,
                core_token: captured_session.sync_token().clone(),
            },
        )?;
        Ok(token)
    }

    #[allow(clippy::too_many_lines)]
    fn create_hosted_room_attempt(
        &self,
        session: &GatewaySession,
        request: &HostedRoomCreationRequestV2,
    ) -> Result<HostedCreateAttempt, BackendError> {
        let spectators = validate_hosted_room_request(request)?;
        let authenticated = self.authenticate(session)?;
        let (core_request, identity) = creation_request(&authenticated, &request.room)?;
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
                let room = create_response_from_result(&result)?;
                let response = self.verify_hosted_authority(request, &spectators, room)?;
                return Ok(HostedCreateAttempt::Response(Box::new(response)));
            }
            RoomCreationIngressV1::Conflict { .. } => return Err(BackendError::Conflict),
            RoomCreationIngressV1::Authorized(grant) => *grant,
        };

        let selected = self
            .registry
            .select_for_new_room(core_request.pack_digest())
            .map_err(|_| BackendError::Rejected)?;
        if selected.descriptor().pack_id != request.room.pack.id
            || selected.descriptor().explanatory_version != request.room.pack.version
        {
            return Err(BackendError::Rejected);
        }
        let room_id = next_core_id::<RoomId>()?;
        let seed = random_room_seed()?;
        let created_at = creation_time()?;
        let memberships = request
            .room
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
        let member_ids = memberships
            .iter()
            .map(|membership| membership.member_id().clone())
            .collect::<Vec<_>>();
        let genesis = self
            .registry
            .prepare_genesis_for_new_room(&PackGenesisRequestV1 {
                room_id: room_id.clone(),
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

        let authority = self.authority();
        let checked_at = Self::checked_at()?;
        let mut changes = Vec::with_capacity(spectators.len() * 2);
        for spectator in &spectators {
            changes.push(
                authority
                    .prepare_change(
                        &presented,
                        AuthorityChangeV1::CreatePrincipal {
                            change_id: AuthorityChangeId::from_str(spectator.principal_id.as_str())
                                .map_err(|_| BackendError::Rejected)?,
                            principal_id: spectator.principal_id.clone(),
                            kind: core_principal_kind(spectator.purpose.principal_kind()),
                        },
                        checked_at.clone(),
                    )
                    .map_err(map_authority_error)?,
            );
            let input = request
                .spectators
                .get(spectator.input_index)
                .ok_or(BackendError::Rejected)?;
            let bearer = input
                .capability
                .bearer
                .wire()
                .map_err(|_| BackendError::Rejected)?;
            let capability = NewCapabilityV1::new(
                spectator.capability_id.clone(),
                CapabilityBearerV1::from_bytes(bearer.into_bytes()).token_hash(),
                spectator.principal_id.clone(),
                CapabilityProfileV1::RoomMember {
                    room_id: room_id.clone(),
                    member_id: member_ids
                        .get(spectator.member_index)
                        .ok_or(BackendError::InvalidResult)?
                        .clone(),
                },
                spectator.scopes.clone(),
                None,
            )
            .map_err(|_| BackendError::Rejected)?;
            changes.push(
                authority
                    .prepare_change(
                        &presented,
                        AuthorityChangeV1::RegisterCapability {
                            change_id: spectator.capability_change_id.clone(),
                            capability,
                        },
                        checked_at.clone(),
                    )
                    .map_err(map_authority_error)?,
            );
        }

        let write = PreparedRoomWriteV1::from(prepared);
        let resolution = self.store.commit_creation_with_authority(&write, changes);
        match resolution {
            RoomCommitResolutionV1::GenesisCreated { result, .. } => {
                let room = create_response_from_result(&result)?;
                let response = self.verify_hosted_authority(request, &spectators, room)?;
                Ok(HostedCreateAttempt::Response(Box::new(response)))
            }
            RoomCommitResolutionV1::Reprepare | RoomCommitResolutionV1::RetryableKnownAbsent => {
                Ok(HostedCreateAttempt::Retry)
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

    fn verify_hosted_authority(
        &self,
        request: &HostedRoomCreationRequestV2,
        spectators: &[ValidatedHostedSpectatorV2],
        room: CreateRoomResponse,
    ) -> Result<HostedRoomCreationResponseV2, BackendError> {
        let room_id = RoomId::from_str(&room.room_id).map_err(|_| BackendError::InvalidResult)?;
        let authority = self.authority();
        let mut receipts = Vec::with_capacity(spectators.len());
        for spectator in spectators {
            let input = request
                .spectators
                .get(spectator.input_index)
                .ok_or(BackendError::InvalidResult)?;
            let bearer = input
                .capability
                .bearer
                .wire()
                .map_err(|_| BackendError::Rejected)?;
            let authenticated = self
                .store
                .authenticate_bearer(CapabilityBearerV1::from_bytes(bearer.into_bytes()))
                .map_err(|_| BackendError::InvalidResult)?;
            if authenticated.presented().capability_id() != &spectator.capability_id
                || authenticated.principal_id() != &spectator.principal_id
                || authenticated.principal_kind()
                    != core_principal_kind(spectator.purpose.principal_kind())
            {
                return Err(BackendError::InvalidResult);
            }
            let member_id = room
                .member_ids
                .get(spectator.member_index)
                .ok_or(BackendError::InvalidResult)?
                .parse::<worldstream_core::MemberId>()
                .map_err(|_| BackendError::InvalidResult)?;
            authority
                .authorize_member_read(
                    authenticated.presented(),
                    room_id.clone(),
                    member_id.clone(),
                    MemberReadOperationV1::Attach,
                    Self::checked_at()?,
                )
                .map_err(|_| BackendError::InvalidResult)?;
            let snapshot = self
                .store
                .snapshot(&AuthoritySnapshotQueryV1::for_capability(
                    spectator.capability_id.clone(),
                ))
                .map_err(|_| BackendError::StorageUnavailable)?
                .ok_or(BackendError::InvalidResult)?;
            if snapshot.principal().status() != PrincipalAuthorityStatusV1::Enabled
                || snapshot.capability().scopes() != &spectator.scopes
                || snapshot.capability().profile()
                    != &(CapabilityProfileV1::RoomMember {
                        room_id: room_id.clone(),
                        member_id: member_id.clone(),
                    })
            {
                return Err(BackendError::InvalidResult);
            }
            receipts.push(HostedSpectatorCredentialReceiptV2 {
                purpose: spectator.purpose,
                room_id: room.room_id.clone(),
                member_id: member_id.to_string(),
                principal_id: spectator.principal_id.to_string(),
                capability_id: spectator.capability_id.to_string(),
                scopes: crate::scope_names(&spectator.scopes),
            });
        }
        Ok(HostedRoomCreationResponseV2 {
            schema: HOSTED_ROOM_CREATION_RESPONSE_SCHEMA_V2.to_owned(),
            room,
            spectators: receipts,
        })
    }

    #[cfg(test)]
    pub(crate) fn create_room_for_format_for_test(
        &self,
        session: &GatewaySession,
        request: &CreateRoomRequest,
        format: worldstream_core::CanonicalHistoryFormat,
    ) -> Result<CreateRoomResponse, BackendError> {
        for _ in 0..3 {
            match self.create_room_attempt(session, request, format)? {
                CreateAttempt::Response(response) => return Ok(*response),
                CreateAttempt::Retry => {}
            }
        }
        Err(BackendError::Busy)
    }

    fn create_room_attempt(
        &self,
        session: &GatewaySession,
        request: &CreateRoomRequest,
        format: worldstream_core::CanonicalHistoryFormat,
    ) -> Result<CreateAttempt, BackendError> {
        let authenticated = self.authenticate(session)?;
        let (legacy_request, identity) = creation_request(&authenticated, request)?;
        let core_request =
            worldstream_core::RoomCreationRequestWithFormat::new(legacy_request, format);
        let presented = authenticated.into_presented();
        let ingress = worldstream_core::authorize_canonical_room_creation_operation(
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
            .select_for_new_room(core_request.legacy_request().pack_digest())
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
            .prepare_genesis_for_new_room_with_format(
                &PackGenesisRequestV1 {
                    room_id,
                    pack_digest: core_request.legacy_request().pack_digest().clone(),
                    configuration: core_request.legacy_request().configuration().clone(),
                    room_seed: seed,
                    created_at,
                    initial_core_state: worldstream_core::CoreRoomStateV1::active(memberships)
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
            worldstream_core::commit_canonical_room_creation(&self.store, prepared).into_parts();
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

enum HostedCreateAttempt {
    Response(Box<HostedRoomCreationResponseV2>),
    Retry,
}

struct ValidatedHostedSpectatorV2 {
    input_index: usize,
    member_index: usize,
    purpose: HostedSpectatorPurposeV2,
    principal_id: worldstream_core::PrincipalId,
    capability_id: CapabilityId,
    capability_change_id: AuthorityChangeId,
    scopes: CapabilityScopeSetV1,
}

fn validate_hosted_room_request(
    request: &HostedRoomCreationRequestV2,
) -> Result<Vec<ValidatedHostedSpectatorV2>, BackendError> {
    if request.schema != HOSTED_ROOM_CREATION_SCHEMA_V2
        || request.spectators.is_empty()
        || request.spectators.len() > 3
        || request.room.members.is_empty()
        || request
            .room
            .members
            .iter()
            .any(|member| member.access_mode == AccessMode::Operator)
    {
        return Err(BackendError::Rejected);
    }
    let spectator_member_count = request
        .room
        .members
        .iter()
        .filter(|member| member.access_mode == AccessMode::Spectator)
        .count();
    if spectator_member_count != request.spectators.len() {
        return Err(BackendError::Rejected);
    }
    let mut purposes = std::collections::BTreeSet::new();
    let mut member_indexes = std::collections::BTreeSet::new();
    let mut principals = std::collections::BTreeSet::new();
    let mut capabilities = std::collections::BTreeSet::new();
    let mut authority_changes = std::collections::BTreeSet::new();
    for member in &request.room.members {
        if !principals.insert(member.principal_id.as_str())
            || (member.access_mode == AccessMode::Participant
                && member.role.as_deref().is_none_or(str::is_empty))
            || (member.access_mode == AccessMode::Spectator && member.role.is_some())
        {
            return Err(BackendError::Rejected);
        }
    }
    let mut validated = Vec::with_capacity(request.spectators.len());
    for (input_index, spectator) in request.spectators.iter().enumerate() {
        let member_index = usize::from(spectator.member_index);
        let member = request
            .room
            .members
            .get(member_index)
            .ok_or(BackendError::Rejected)?;
        if member.access_mode != AccessMode::Spectator
            || member.role.is_some()
            || member.principal_id != spectator.principal_id
            || member.principal_kind != spectator.principal_kind
            || spectator.principal_kind != spectator.purpose.principal_kind()
            || !purposes.insert(spectator.purpose)
            || !member_indexes.insert(member_index)
        {
            return Err(BackendError::Rejected);
        }
        let principal_id = spectator
            .principal_id
            .parse::<worldstream_core::PrincipalId>()
            .map_err(|_| BackendError::Rejected)?;
        let capability_id = spectator
            .capability
            .capability_id
            .parse::<CapabilityId>()
            .map_err(|_| BackendError::Rejected)?;
        let capability_change_id = spectator
            .capability
            .capability_idempotency_key
            .parse::<AuthorityChangeId>()
            .map_err(|_| BackendError::Rejected)?;
        spectator
            .capability
            .bearer
            .wire()
            .map_err(|_| BackendError::Rejected)?;
        if !capabilities.insert(capability_id.to_string())
            || !authority_changes.insert(principal_id.to_string())
            || !authority_changes.insert(capability_change_id.to_string())
        {
            return Err(BackendError::Rejected);
        }
        let scopes = crate::provisioned_scopes(
            spectator
                .purpose
                .capability_scopes()
                .iter()
                .map(ToString::to_string)
                .collect(),
        )?;
        validated.push(ValidatedHostedSpectatorV2 {
            input_index,
            member_index,
            purpose: spectator.purpose,
            principal_id,
            capability_id,
            capability_change_id,
            scopes,
        });
    }
    if !purposes.contains(&HostedSpectatorPurposeV2::ResultIndexer) {
        return Err(BackendError::Rejected);
    }
    Ok(validated)
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

fn map_admission_lane_error(error: &AdmissionLaneErrorV1) -> BackendError {
    match error {
        AdmissionLaneErrorV1::Full | AdmissionLaneErrorV1::Unavailable => BackendError::Busy,
        AdmissionLaneErrorV1::Clock(_) | AdmissionLaneErrorV1::InvalidCapacity => {
            BackendError::StorageUnavailable
        }
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

fn map_external_input_preparation_error(
    error: SqliteExternalInputPreparationErrorV1,
) -> BackendError {
    match error {
        SqliteExternalInputPreparationErrorV1::Conflict => BackendError::Conflict,
        SqliteExternalInputPreparationErrorV1::StorageUnavailable => {
            BackendError::StorageUnavailable
        }
        SqliteExternalInputPreparationErrorV1::Corrupt => BackendError::InvalidResult,
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
    head: worldstream_core::CompleteHeadV1,
    integrity: worldstream_core::RoomIntegrityStateV1,
    room_status: worldstream_core::RoomStatusV1,
    view: worldstream_core::ValidatedPackViewV1,
    membership: worldstream_core::MembershipV1,
    capability_id: worldstream_core::CapabilityId,
    presented: PresentedCapabilityV1,
}

impl GatewayBackend for SqliteGatewayBackend {
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
        Ok(crate::activity_pack_catalog_from_registry(&self.registry))
    }

    fn activity_pack_revision(
        &self,
        session: &GatewaySession,
        revision_digest: &str,
    ) -> Result<worldstream_protocol::ActivityPackCatalogRevisionResponse, BackendError> {
        self.authorize_activity_pack_catalog(session)?;
        crate::activity_pack_revision_from_registry(&self.registry, revision_digest)
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
            match self.create_room_attempt(session, &request.request, format)? {
                CreateAttempt::Response(response) => return Ok(*response),
                CreateAttempt::Retry => {}
            }
        }
        Err(BackendError::Busy)
    }

    fn create_hosted_room(
        &self,
        session: &GatewaySession,
        request: HostedRoomCreationRequestV2,
    ) -> Result<HostedRoomCreationResponseV2, BackendError> {
        for _ in 0..3 {
            match self.create_hosted_room_attempt(session, &request)? {
                HostedCreateAttempt::Response(response) => return Ok(*response),
                HostedCreateAttempt::Retry => {}
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
        // Recheck the same current Capability and operational fence after the
        // immutable page read. A revocation or integrity transition racing
        // the page is therefore denied rather than released as authorized.
        let fenced = self.replay_response(session, room_id, at_room_seq)?;
        if fenced.room_head != replay.room_head
            || fenced.integrity_generation != replay.integrity_generation
        {
            return Err(BackendError::Busy);
        }
        let page = match page_result {
            Ok(page) => page,
            Err(SqliteHistoricalEvidenceErrorV1::Missing) => {
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
            Err(SqliteHistoricalEvidenceErrorV1::Pruned) => {
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
            Err(SqliteHistoricalEvidenceErrorV1::Retired) => {
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
            Err(SqliteHistoricalEvidenceErrorV1::BudgetExceeded) => {
                return Err(BackendError::Busy);
            }
            Err(SqliteHistoricalEvidenceErrorV1::StorageUnavailable) => {
                return Err(BackendError::StorageUnavailable);
            }
            Err(SqliteHistoricalEvidenceErrorV1::Corrupt) => {
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
        let snapshot = self
            .readable_room_snapshot(&room_id)?
            .ok_or(BackendError::NotFound)?;
        let membership = snapshot
            .trace()
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
                Self::checked_at()?,
            )
            .map_err(map_authority_error)?;
        let pack = self
            .registry
            .load_retained(snapshot.trace().head().pack_digest())
            .map_err(|_| BackendError::InvalidResult)?;
        let current = self
            .readable_room_snapshot(&room_id)?
            .ok_or(BackendError::NotFound)?;
        if current.trace().head() != snapshot.trace().head()
            || current.integrity() != snapshot.integrity()
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
            pack: PackReference {
                id: pack.descriptor().pack_id.clone(),
                version: pack.descriptor().explanatory_version.clone(),
                digest: snapshot.trace().head().pack_digest().to_string(),
            },
        })
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
        let member_id = request
            .member_id
            .parse()
            .map_err(|_| BackendError::Rejected)?;
        let context = self.attach_view(session, &room_id, &member_id)?;
        let attach_grant = self
            .authority()
            .authorize_member_read(
                &context.presented,
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
        if captured.room_head() != &context.head {
            return Err(BackendError::Busy);
        }
        // An ACK may commit while its receipt is lost. A conservative client
        // Cursor is recoverable, but a client cannot assert an uncommitted one.
        if request.after_frame_seq > captured.cursor() {
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
            .store
            .current_room_serving_fence(&room_id)
            .map_err(|error| map_gateway_error(&error))?
            .ok_or(BackendError::NotFound)?;
        if current.head() != captured.room_head() || current.integrity() != &context.integrity {
            return Err(BackendError::Busy);
        }
        let (sync, reset, frames) = if request.after_frame_seq < captured.cursor() {
            protocol_cursor_recovery(&captured, &context)?
        } else {
            protocol_delivery(
                &captured,
                &room_id,
                &member_id,
                current.integrity().generation().get(),
                integrity_status(current.integrity().status()),
            )?
        };
        let retained_pack = self
            .registry
            .load_retained(context.head.pack_digest())
            .map_err(|_| BackendError::InvalidResult)?;
        let descriptor = retained_pack.descriptor();
        Ok(AttachReply {
            attached: RoomAttached {
                room_id: room_id.to_string(),
                member_id: member_id.to_string(),
                principal_kind: protocol_principal_kind(context.membership.principal_kind()),
                access_mode: protocol_access_mode(context.membership.access_mode()),
                role: context.membership.role().map(str::to_owned),
                membership_status: membership_status(context.membership.standing()),
                room_status: room_status(context.room_status),
                room_health: integrity_status(current.integrity().status()),
                integrity_generation: current.integrity().generation().get(),
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
                .read_observation_suffix_at_reset_generation(
                    grant,
                    binding.baseline_frame_head,
                    binding.installed_reset_generation,
                )
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
            Ok(frames) => {
                self.bindings.mark_live(
                    session.session_id(),
                    &capability_id,
                    &binding.room_id,
                    &binding.member_id,
                    binding.installed_reset_generation,
                )?;
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
        let installed_reset_generation = self.bindings.live_reset_generation(
            session.session_id(),
            &capability_id,
            &room_id,
            &member_id,
        )?;
        self.store
            .read_observation_suffix_at_reset_generation(
                grant,
                after_frame_seq,
                installed_reset_generation,
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
        let installed_reset_generation = self.bindings.live_reset_generation(
            session.session_id(),
            &capability_id,
            &room_id,
            &member_id,
        )?;
        let (frames, has_more) = self
            .store
            .read_observation_page_at_reset_generation(
                grant,
                after_frame_seq,
                installed_reset_generation,
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
        #[cfg(test)]
        if self
            .independent_live_pages_for_measurement
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return false;
        }
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
        let max_recipients = if self.supports_shared_live_observation_cut() {
            16
        } else {
            2
        };
        if recipients.len() > max_recipients || addresses.len() > 2 {
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
                        Self::checked_at()?,
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
                    // The measurement baseline reads each recipient separately.
                    // Production and ordinary tests retain the same shared groups.
                    let discriminator = 0usize;
                    #[cfg(test)]
                    let discriminator = if self
                        .independent_live_pages_for_measurement
                        .load(std::sync::atomic::Ordering::Relaxed)
                    {
                        index + 1
                    } else {
                        discriminator
                    };
                    groups
                        .entry((room, member, recipient.after_frame_seq, discriminator))
                        .or_insert_with(Vec::new)
                        .push((index, grant, reset, capability));
                }
                Err(error) => results[index] = Some(Err(error)),
            }
        }
        for ((room, member, after, _discriminator), group) in groups {
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
                    .read_shared_observation_page(grants, after, reset, max_frames, max_bytes)
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
                        vec![Arc::clone(grant)],
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
                if let Err(error) = self
                    .store
                    .revalidate_live_observation(Arc::clone(grant), Arc::clone(cut))
                {
                    results[*index] = Some(Err(map_observation_error(error)));
                }
            }
            if let Some((frames, has_more, cut)) = page {
                #[cfg(test)]
                {
                    self.live_payload_records_read
                        .fetch_add(frames.len(), std::sync::atomic::Ordering::Relaxed);
                    self.live_payload_bytes_read.fetch_add(
                        frames.iter().map(|f| f.payload_bytes().len()).sum(),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
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
                            let fence = SqliteLiveFence {
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
                let admission = self
                    .admission_lanes
                    .reserve_action(&action_room_id, self.host_clock.as_ref())
                    .map_err(|error| map_admission_lane_error(&error))?;
                let transition_id = next_core_id::<TransitionId>()?;
                let resolution = self.commit_cached_participant_action(
                    &action_room_id,
                    *grant,
                    &core_request,
                    admission.admitted_at().clone(),
                    transition_id,
                )?;
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
        self.commit_due_timer(&room_id, authority, timer_request, operation)
    }

    #[allow(clippy::too_many_lines)]
    fn ingest_external_input(
        &self,
        session: &GatewaySession,
        room_id: &str,
        request: worldstream_protocol::ExternalInputIngressRequestV1,
    ) -> Result<worldstream_protocol::ExternalInputIngressResponseV1, BackendError> {
        let authenticated = self.authenticate(session)?;
        let checked_at = Self::checked_at()?;
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
        let snapshot = self
            .store
            .current_room_serving_fence(&plan.room_id)
            .map_err(|error| map_gateway_error(&error))?
            .ok_or(BackendError::NotFound)?;
        if snapshot.head().pack_digest() != &plan.pack_digest {
            return Err(BackendError::Conflict);
        }
        self.ensure_verified_active(&plan.room_id)?;
        let operation = self.acquire_room_operation(&plan.room_id)?;
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
        operation.publish().map_err(Self::map_supervisor_error)?;
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
        let checked_at = Self::checked_at()?;
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
            prepare_activity_start_request(&self.registry, room_id, &request, &checked_at)?;
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
                    &self.registry,
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
        let contract_snapshot = self
            .store
            .current_room_serving_fence(&plan.room_id)
            .map_err(|_| BackendError::StorageUnavailable)?
            .ok_or(BackendError::NotFound)?;
        let revision_digest = contract_snapshot.head().pack_digest().clone();
        plan.validate_room_pack(&self.registry, &revision_digest)?;
        if !self
            .registry
            .activity_start_is_approved(&revision_digest)
            .map_err(|_| BackendError::InvalidResult)?
        {
            return Err(BackendError::WrongPhase);
        }
        self.ensure_verified_active(&plan.room_id)?;
        let operation = self.acquire_room_operation(&plan.room_id)?;
        let _admission = self
            .admission_lanes
            .reserve_host_stimulus(&plan.room_id)
            .map_err(|error| map_admission_lane_error(&error))?;
        match RoomCommitStorageV1::resolve(&self.store, &plan.identity, &plan.request_hash) {
            worldstream_core::ResolveOutcomeV1::StoredResolution(result) => {
                return lobby_response_from_result(
                    &plan,
                    &self.registry,
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
        let contract = self.with_cached_current_trace(&plan.room_id, |trace, _| {
            plan.validate_room_pack(&self.registry, trace.head().pack_digest())?;
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
        let proposed_recorded_at = Self::checked_at()?;
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
        match RoomCommitStorageV1::resolve(&self.store, &plan.identity, &plan.request_hash) {
            worldstream_core::ResolveOutcomeV1::StoredResolution(result) => {
                return lobby_response_from_result(
                    &plan,
                    &self.registry,
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
        self.with_cached_current_trace(&plan.room_id, |trace, _| {
            plan.validate_room_pack(&self.registry, trace.head().pack_digest())?;
            if !worldstream_core::activity_start_is_applicable(&contract, trace.activity_state()) {
                return Err(BackendError::WrongPhase);
            }
            Ok(())
        })?;
        let resolution = match self.commit_cached_external_input(
            &plan.room_id,
            authority,
            plan.based_on_room_seq,
            &plan.input,
            next_core_id::<TransitionId>()?,
        ) {
            Ok(resolution) => resolution,
            Err(error) => return Err(error),
        };
        operation.publish().map_err(Self::map_supervisor_error)?;
        lobby_response_from_resolution(&plan, &self.registry, &request.input_id, &resolution)
    }

    fn resolve_lobby_launch(
        &self,
        session: &GatewaySession,
        room_id: &str,
        request: LobbyLaunchRequest,
    ) -> Result<Option<LobbyLaunchResponse>, BackendError> {
        let authenticated = self.authenticate(session)?;
        let checked_at = Self::checked_at()?;
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
        let plan = prepare_activity_start_request(&self.registry, room_id, &request, &checked_at)?;
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
                lobby_response_from_result(&plan, &self.registry, &request.input_id, &result, true)
                    .map(Some)
            }
            worldstream_core::ResolveOutcomeV1::Conflict { .. } => Err(BackendError::Conflict),
            worldstream_core::ResolveOutcomeV1::ResolutionUnavailable => {
                Err(BackendError::Indeterminate)
            }
            worldstream_core::ResolveOutcomeV1::KnownAbsent => {
                let snapshot = self
                    .store
                    .current_room_serving_fence(&plan.room_id)
                    .map_err(|_| BackendError::StorageUnavailable)?
                    .ok_or(BackendError::NotFound)?;
                plan.validate_room_pack(&self.registry, snapshot.head().pack_digest())?;
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
        let (fallback_head, current_status) = self
            .with_cached_current_trace(&room_id, |trace, _| {
                Ok((trace.head().clone(), trace.core_state().room_status()))
            })?;
        let checked_at = Self::checked_at()?;
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
            &self.store,
            &presented,
            &core_request,
            checked_at,
        )
        .map_err(map_room_operation_error)?;
        if current_status == worldstream_core::RoomStatusV1::Archived {
            // Still pass the retry through Core's receipt/admin authority
            // boundary before reporting the already-achieved outcome.
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
                self.ensure_verified_active(&room_id)?;
                let operation = self.acquire_room_operation(&room_id)?;
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
                operation.publish().map_err(Self::map_supervisor_error)?;
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
        let checked_at = Self::checked_at()?;
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
            .map_err(|error| map_sqlite_diagnostic_error(&error))?;
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
        let checked_at = Self::checked_at()?;
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
            .map_err(|error| map_sqlite_diagnostic_error(&error))?;
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
        let checked_at = Self::checked_at()?;
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
            .map_err(|error| map_sqlite_diagnostic_error(&error))?;
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
            storage_profile: OperatorBackupStorageProfile::SqliteBundled,
            storage_health: OperatorBackupStorageHealth::Healthy,
            live_backup_supported: self.live_backup_root.is_some(),
            verification: OperatorBackupVerification::Unavailable,
            freshness: OperatorDataFreshness::Unavailable {
                reason: "no_operation_selected".to_owned(),
            },
        })
    }

    fn operator_live_backup(
        &self,
        session: &GatewaySession,
        request: OperatorLiveBackupPrepareRequest,
    ) -> Result<OperatorLiveBackupStatus, BackendError> {
        self.authorize_backup(session)?;
        let destination = self.validated_live_backup_destination(&request.operation_id)?;
        let receipt = if destination.exists() {
            self.store.verify_live_backup(&destination)
        } else {
            self.store.create_live_backup(&destination)
        }
        .map_err(|_| BackendError::StorageUnavailable)?;
        let observed_at = Self::checked_at()?.as_str().to_owned();
        Ok(OperatorLiveBackupStatus {
            operation_id: request.operation_id,
            storage_profile: OperatorBackupStorageProfile::SqliteBundled,
            storage_health: OperatorBackupStorageHealth::Healthy,
            native_verification: OperatorBackupVerification::Pass,
            semantic_verification: OperatorBackupVerification::Unavailable,
            freshness: OperatorDataFreshness::Fresh { observed_at },
            artifact: Some(OperatorLiveBackupArtifactSummary {
                artifact_name: "backup.sqlite3".to_owned(),
                byte_length: receipt.artifact_bytes(),
                blake3_digest: receipt.artifact_digest().to_owned(),
                semantic_digest: receipt.semantic_digest().to_string(),
            }),
            unavailable_reason: Some("full_semantic_restore_verification_not_run".to_owned()),
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
            room_id.clone(),
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
        let claim = self.with_cached_current_trace(&room_id, |trace, integrity| {
            self.store
                .prepare_activation_claim_from_canonical_serving_trace(
                    &self.registry,
                    &authority,
                    operation.clone(),
                    trace,
                    integrity,
                )
                .map_err(map_activation_error)
        })?;
        let runtime_state = self.cached_runtime_state(&authority.target().room_id)?;
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

    fn scheduler_tick(&self) -> Result<Vec<String>, BackendError> {
        self.ensure_source_authoritative()?;
        let mut rooms = self.store.room_ids().map_err(map_activation_error)?;
        if self.timer_authority.is_none() {
            for room_id in &rooms {
                if self.verified_room_lifecycle(room_id)? == SqliteRoomRuntimeStateV1::CatchingUp {
                    continue;
                }
                self.store
                    .reclaim_activation_leases(room_id.clone(), None)
                    .map_err(map_activation_error)?;
            }
            self.scheduled_observation_retention(&rooms)?;
            return Ok(Vec::new());
        }
        let mut last_timer_room = self
            .last_timer_room
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?;
        if let Some(previous) = last_timer_room.as_ref() {
            let offset = rooms.partition_point(|room| room.as_str() <= previous.as_str());
            rooms.rotate_left(offset);
        }
        let mut committed_rooms = Vec::new();
        let mut remaining = 64;
        for room_id in &rooms {
            let mut changed = false;
            if remaining > 0 {
                *last_timer_room = Some(room_id.clone());
                if let Err(error) =
                    self.scheduled_timer_slice(room_id, &mut remaining, &mut changed)
                {
                    tracing::warn!(?error, "scheduled Room timer slice deferred");
                }
            }
            if changed {
                committed_rooms.push(room_id.to_string());
            }
            if let Ok(SqliteRoomRuntimeStateV1::Active) = self.verified_room_lifecycle(room_id)
                && let Err(error) = self.store.reclaim_activation_leases(room_id.clone(), None)
            {
                tracing::warn!(?error, "Room Activation maintenance deferred");
            }
        }
        if let Err(error) = self.scheduled_observation_retention(&rooms) {
            tracing::warn!(?error, "Room Observation retention maintenance deferred");
        }
        Ok(committed_rooms)
    }
}

struct SqliteLiveFence {
    store: SqliteRoomStore,
    bindings: Arc<SessionBindings>,
    grant: Arc<worldstream_core::ViewerAdapterInputV1>,
    cut: Arc<worldstream_sqlite::SqliteLiveObservationCut>,
    session_id: worldstream_protocol::UlidString,
    capability_id: worldstream_core::CapabilityId,
    reset: u64,
}
impl crate::LiveObservationFence for SqliteLiveFence {
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
            .revalidate_live_observation(Arc::clone(&self.grant), Arc::clone(&self.cut))
            .map_err(map_observation_error)
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
        if binding.capability_id != *capability_id {
            return Err(BackendError::Forbidden);
        }
        if binding.sync.is_some() {
            return Err(BackendError::StorageUnavailable);
        }
        binding.sync = Some(sync);
        Ok(())
    }

    fn mark_live(
        &self,
        session_id: &worldstream_protocol::UlidString,
        capability_id: &worldstream_core::CapabilityId,
        room_id: &RoomId,
        member_id: &worldstream_core::MemberId,
        reset_generation: u64,
    ) -> Result<(), BackendError> {
        let mut bindings = self
            .0
            .lock()
            .map_err(|_| BackendError::StorageUnavailable)?;
        let binding = bindings
            .get_mut(session_id)
            .ok_or(BackendError::Forbidden)?;
        if binding.capability_id != *capability_id || binding.sync.is_some() {
            return Err(BackendError::Rejected);
        }
        binding.live = Some(LiveObservationBinding {
            room_id: room_id.clone(),
            member_id: member_id.clone(),
            reset_generation,
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
        let live = binding.live.as_ref().ok_or(BackendError::Rejected)?;
        if binding.capability_id != *capability_id
            || live.room_id != *room_id
            || live.member_id != *member_id
        {
            return Err(BackendError::Rejected);
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

// The same validated view was checked against the captured Head inside the
// authorized storage attach. Only this Session's delivery branch changes:
// Canonical History, the durable Cursor, and the captured barrier do not.
fn protocol_cursor_recovery(
    captured: &worldstream_sqlite::SqliteObservationAttachV1,
    context: &AttachContext,
) -> Result<(SyncBranch, Option<ProjectionReset>, Vec<ObservationDeliver>), BackendError> {
    let reason = "client_cursor_behind";
    let reset = ProjectionReset {
        room_id: captured.room_head().room_id().to_string(),
        member_id: context.membership.member_id().to_string(),
        room_head: room_head(captured.room_head()),
        room_health: integrity_status(context.integrity.status()),
        integrity_generation: context.integrity.generation().get(),
        baseline_frame_head: captured.frame_head(),
        reset_reason: reason.to_owned(),
        projection_schema: context.view.projection_schema().to_owned(),
        projection: projection_from_canonical_bytes(context.view.canonical_bytes())?,
        projection_hash: context
            .view
            .projection_hash()
            .map_err(|_| BackendError::InvalidResult)?
            .to_string(),
    };
    Ok((
        SyncBranch::ProjectionReset {
            baseline_frame_head: captured.frame_head(),
            reason: reason.to_owned(),
        },
        Some(reset),
        Vec::new(),
    ))
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
        SqliteObservationResetReasonV1::RetainedBacklogTooLarge => "retained_backlog_too_large",
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

fn operator_room_summary(
    summary: &SqliteRoomDiagnosticSummaryV1,
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
    Ok(OperatorRoomSummary {
        room_id: summary.room_id().to_string(),
        room_head: room_head(summary.head()),
        pack: PackReference {
            id: summary.pack_revision().pack_id.clone(),
            version: summary.pack_revision().explanatory_version.clone(),
            digest: summary.head().pack_digest().to_string(),
        },
        integrity: OperatorRoomIntegrity {
            status: match summary.integrity().status() {
                worldstream_core::RoomIntegrityStatusV1::Healthy => {
                    OperatorRoomIntegrityStatus::Healthy
                }
                worldstream_core::RoomIntegrityStatusV1::Faulted => {
                    OperatorRoomIntegrityStatus::Faulted
                }
                worldstream_core::RoomIntegrityStatusV1::Quarantined => {
                    OperatorRoomIntegrityStatus::Quarantined
                }
            },
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

fn map_sqlite_diagnostic_error(error: &SqliteRoomDiagnosticErrorV1) -> BackendError {
    match error {
        SqliteRoomDiagnosticErrorV1::Authority(error) => map_authority_error(*error),
        SqliteRoomDiagnosticErrorV1::RoomUnavailable => BackendError::NotFound,
        SqliteRoomDiagnosticErrorV1::DeploymentTarget
        | SqliteRoomDiagnosticErrorV1::UnsupportedOperation => BackendError::Rejected,
        SqliteRoomDiagnosticErrorV1::RuntimeUnavailable
        | SqliteRoomDiagnosticErrorV1::RuntimeFault
        | SqliteRoomDiagnosticErrorV1::StorageUnavailable
        | SqliteRoomDiagnosticErrorV1::Corrupt
        | SqliteRoomDiagnosticErrorV1::ExportTooLarge
        | SqliteRoomDiagnosticErrorV1::RestoreDeploymentRequired => {
            BackendError::StorageUnavailable
        }
    }
}

impl SqliteGatewayBackend {
    fn validated_live_backup_destination(
        &self,
        operation_id: &str,
    ) -> Result<PathBuf, BackendError> {
        if operation_id.is_empty()
            || operation_id.len() > 64
            || !operation_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || operation_id.starts_with('-')
            || operation_id.ends_with('-')
        {
            return Err(BackendError::Rejected);
        }
        let root = self
            .live_backup_root
            .as_ref()
            .ok_or(BackendError::StorageUnavailable)?;
        let operation = prepare_data_directory(&root.join(operation_id))
            .map_err(|_| BackendError::StorageUnavailable)?;
        let canonical_root =
            std::fs::canonicalize(root).map_err(|_| BackendError::StorageUnavailable)?;
        if operation.parent() != Some(canonical_root.as_path()) {
            return Err(BackendError::Rejected);
        }
        Ok(operation.join("backup.sqlite3"))
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
        SqliteActivationErrorV1::ContextTooLarge => BackendError::Rejected,
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
        SqliteObservationErrorV1::ResetRequired => BackendError::ResetRequired,
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
    #![allow(unexpected_cfgs)]
    #[allow(unexpected_cfgs)]
    #[cfg(worldstream_gateway_measurement)]
    #[path = "gateway_scaling_measurement.rs"]
    mod gateway_scaling_measurement;

    use super::*;
    use std::{
        env, fs,
        process::Command,
        sync::atomic::{AtomicUsize, Ordering},
        thread,
    };
    use tempfile::{NamedTempFile, TempDir, tempdir};
    use worldstream_component_host::ComponentPackHostV1;
    use worldstream_core::{
        AccessModeV1, AdministrationOperationIdentityV1, AuthorityBootstrapV1, AuthorityChangeV1,
        AuthorityCheckedAt, AuthorityV1, CapabilityBearerV1, CapabilityId, CapabilityProfileV1,
        CapabilityScopeSetV1, CapabilityScopeV1, CoreAdministrationIngressV1,
        CoreAdministrationRequestV1, CoreChangeSetV1, CoreProposedKindV1, ExternalInputV1,
        MembershipChangeV1, MembershipStandingV1, MembershipV1, NewCapabilityV1,
        PresentedCapabilityV1, PrincipalKindV1, RoomCommitResolutionV1, RoomId, SourceId,
        TransitionId, agent_heist_lobby_digest, agent_heist_schema_safe_digest,
        authorize_core_administration_operation, builtin_agent_heist_registry,
        builtin_counter_registry, counter_v2_digest, counter_v3_digest, counter_v4_digest,
        external_input_request_hash,
    };
    use worldstream_pack_bundle::PackBundleVerifierV1;
    use worldstream_protocol::{
        AccessMode, BearerWireV1, CreateMember, HostedSpectatorCredentialInputV2,
        MemberCapabilityProvisionRequestV1, PackReference, PrincipalKind,
        RunnerCapabilityProvisionRequestV1, RunnerMembershipProvisionTargetV1,
        SealedCapabilityBearerV1, SealedCapabilityInputV1,
    };

    fn database_fixture() -> (TempDir, NamedTempFile) {
        // The database and its parent must share an owner. The system temp
        // directory can be root-owned, so retain a private parent per test.
        let directory = tempdir().unwrap_or_else(|_| panic!("temp db directory"));
        let file = NamedTempFile::new_in(directory.path()).unwrap_or_else(|_| panic!("temp db"));
        (directory, file)
    }

    fn archive_registry(
        selectable_for_new_rooms: bool,
        approved_for_activity_start: bool,
    ) -> (
        Arc<PackRegistryV1>,
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
                        selectable_for_new_rooms,
                        runnable_for_retained_rooms: true,
                        approved_for_activity_start,
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

    struct SchedulerClock(Mutex<HostClockSampleV1>);

    impl HostClockV1 for SchedulerClock {
        fn sample(&self) -> Result<HostClockSampleV1, HostClockErrorV1> {
            self.0
                .lock()
                .map(|value| value.clone())
                .map_err(|_| HostClockErrorV1::Unavailable)
        }
    }

    fn reserve_scheduler_launch_fixture_time(
        backend: &SqliteGatewayBackend,
        room_id: &str,
        request: &LobbyLaunchRequest,
        now: OffsetDateTime,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Exercise the clock-safe executor with a canonical fraction whose
        // sixth digit is zero. The retained 0.2 formatter pads that zero.
        let recorded_at = (now + time::Duration::seconds(1))
            .replace_nanosecond(914_230_084)?
            .format(&Rfc3339)?;
        let plan = prepare_activity_start_request(
            &backend.registry,
            room_id.parse()?,
            request,
            &recorded_at.parse()?,
        )?;
        backend.store.reserve_external_input_recorded_at(
            &plan.identity,
            &plan.request_hash,
            &recorded_at.parse()?,
        )?;
        Ok(())
    }

    #[tokio::test]
    async fn scheduler_advances_due_room_once_without_operator_timer_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_database_directory, file) = database_fixture();
        let store = SqliteRoomStore::open(file.path())?;
        let bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        AuthorityV1::new(Arc::new(store.clone())).bootstrap(
            AuthorityBootstrapV1::new(
                "01ARZ3NDEKTSV4RRFFQ69G5FC4".parse()?,
                "01ARZ3NDEKTSV4RRFFQ69G5FC2".parse()?,
                PrincipalKindV1::Human,
                "01ARZ3NDEKTSV4RRFFQ69G5FC3".parse()?,
                bearer.token_hash(),
                None,
            )?,
            "2026-08-15T12:00:00Z".parse()?,
        )?;
        let now = OffsetDateTime::now_utc();
        let clock = Arc::new(SchedulerClock(Mutex::new(HostClockSampleV1::new(
            now.format(&Rfc3339)?,
        )?)));
        let backend = SqliteGatewayBackend::with_host_clock(
            store,
            Arc::new(builtin_agent_heist_registry()?),
            clock.clone(),
        )
        .with_timer_authority(bearer)?;
        let host = session(0xa9, "01ARZ3NDEKTSV4RRFFQ69G5FBE");
        let members = [
            ("01ARZ3NDEKTSV4RRFFQ69G5FC2", "navigator"),
            ("01ARZ3NDEKTSV4RRFFQ69G5FD3", "insider"),
            ("01ARZ3NDEKTSV4RRFFQ69G5FD4", "broker"),
        ]
        .into_iter()
        .map(|(principal, role)| CreateMember {
            principal_id: principal.to_owned(),
            principal_kind: PrincipalKind::Human,
            role: Some(role.to_owned()),
            access_mode: AccessMode::Participant,
        })
        .collect();
        // Pin the existing clock-safe revision for canonical fractional deadlines.
        let room = backend.create_room(&host, CreateRoomRequest {
            pack: PackReference { id: "worldstream.agent-heist".to_owned(), version: "0.3.0".to_owned(), digest: worldstream_core::agent_heist_clock_safe_digest().to_string() },
            configuration: json!({
                "pack_id":"worldstream.agent-heist","pack_schema":1,
                "roles":["navigator","insider","broker"],
                "briefing_duration_seconds":30,"negotiation_duration_seconds":90,
                "commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,
                "result_duration_seconds":20,"maximum_plans":12,"maximum_open_offers_per_role":4
            }),
            members, idempotency_key: "automatic-timer-room".to_owned(),
        }).map_err(|error| format!("scheduler fixture create Room: {error:?}"))?;
        let launch = LobbyLaunchRequest {
            input_id: "01ARZ3NDEKTSV4RRFFQ69G5FD6".to_owned(),
            based_on_room_seq: 0,
            pack_digest: None,
        };
        reserve_scheduler_launch_fixture_time(&backend, &room.room_id, &launch, now)?;
        backend
            .launch_lobby(&host, &room.room_id, launch)
            .map_err(|error| format!("scheduler fixture launch Lobby: {error:?}"))?;
        backend
            .scheduler_tick()
            .map_err(|error| format!("scheduler fixture tick: {error:?}"))?;
        assert_eq!(
            backend
                .operator_room_detail(&host, &room.room_id)?
                .room_head
                .room_seq,
            1
        );
        *clock.0.lock().map_err(|_| "clock unavailable")? =
            HostClockSampleV1::new((now + time::Duration::seconds(60)).format(&Rfc3339)?)?;
        backend
            .scheduler_tick()
            .map_err(|error| format!("fixture due timer tick: {error:?}"))?;
        assert_eq!(
            backend
                .operator_room_detail(&host, &room.room_id)?
                .room_head
                .room_seq,
            2
        );
        backend
            .scheduler_tick()
            .map_err(|error| format!("fixture due timer tick: {error:?}"))?;
        assert_eq!(
            backend
                .operator_room_detail(&host, &room.room_id)?
                .room_head
                .room_seq,
            2
        );
        assert_scheduler_publication(backend, &host, &room, &clock, now).await?;
        Ok(())
    }

    #[tokio::test]
    async fn warm_activation_claim_reuses_executor_after_due_timer()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_database_directory, file) = database_fixture();
        let store = SqliteRoomStore::open(file.path())?;
        let host_bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        AuthorityV1::new(Arc::new(store.clone())).bootstrap(
            AuthorityBootstrapV1::new(
                "01ARZ3NDEKTSV4RRFFQ69G5FC4".parse()?,
                "01ARZ3NDEKTSV4RRFFQ69G5FC2".parse()?,
                PrincipalKindV1::Human,
                "01ARZ3NDEKTSV4RRFFQ69G5FC3".parse()?,
                host_bearer.token_hash(),
                None,
            )?,
            "2026-08-15T12:00:00Z".parse()?,
        )?;
        let now = OffsetDateTime::now_utc();
        let clock = Arc::new(SchedulerClock(Mutex::new(HostClockSampleV1::new(
            now.format(&Rfc3339)?,
        )?)));
        let backend = SqliteGatewayBackend::with_host_clock(
            store.clone(),
            Arc::new(builtin_agent_heist_registry()?),
            clock.clone(),
        )
        .with_timer_authority(host_bearer)?;
        let host = session(0xa9, "01ARZ3NDEKTSV4RRFFQ69G5FC3");
        // Pin the existing clock-safe revision for canonical fractional deadlines.
        let room = backend.create_room(&host, CreateRoomRequest {
            pack: PackReference {
                id: "worldstream.agent-heist".to_owned(),
                version: "0.3.0".to_owned(),
                digest: worldstream_core::agent_heist_clock_safe_digest().to_string(),
            },
            configuration: json!({
                "pack_id":"worldstream.agent-heist","pack_schema":1,
                "roles":["navigator","insider","broker"],
                "briefing_duration_seconds":30,"negotiation_duration_seconds":90,
                "commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,
                "result_duration_seconds":20,"maximum_plans":12,"maximum_open_offers_per_role":4
            }),
            members: vec![
                CreateMember {
                    principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC6".to_owned(),
                    principal_kind: PrincipalKind::Agent,
                    role: Some("navigator".to_owned()),
                    access_mode: AccessMode::Participant,
                },
                CreateMember {
                    principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD3".to_owned(),
                    principal_kind: PrincipalKind::Human,
                    role: Some("insider".to_owned()),
                    access_mode: AccessMode::Participant,
                },
                CreateMember {
                    principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD4".to_owned(),
                    principal_kind: PrincipalKind::Human,
                    role: Some("broker".to_owned()),
                    access_mode: AccessMode::Participant,
                },
            ],
            idempotency_key: "imo220-warm-timer-room".to_owned(),
        }).map_err(|error| format!("warm fixture create Room: {error:?}"))?;
        let navigator_member = room.member_ids[0].clone();
        let launch = LobbyLaunchRequest {
            input_id: "01ARZ3NDEKTSV4RRFFQ69G5FG0".to_owned(),
            based_on_room_seq: 0,
            pack_digest: None,
        };
        reserve_scheduler_launch_fixture_time(&backend, &room.room_id, &launch, now)?;
        backend
            .launch_lobby(&host, &room.room_id, launch)
            .map_err(|error| format!("warm fixture launch Lobby: {error:?}"))?;
        let runner_capability = backend
            .provision_runner_capability(
                &host,
                sealed_runner_request(&room.room_id, &navigator_member, 0xd1),
            )
            .map_err(|error| format!("warm fixture provision Runner: {error:?}"))?;
        let room_id: RoomId = room.room_id.parse()?;
        let head = store
            .current_room_serving_fence(&room_id)?
            .unwrap_or_else(|| panic!("warm Timer serving fence absent"));
        let activation_id = "imo220-warm-timer-activation".to_owned();
        let connection = rusqlite::Connection::open(file.path())?;
        connection.execute(
            "INSERT INTO activation_intents(\
             activation_id, room_id, cause_room_seq, decision_id, target_member_id,\
             reason_code, deduplication_key, priority, policy_revision, state,\
             intent_generation, lease_generation)\
             VALUES (?1, ?2, ?3, ?4, ?5, 'imo220-warm-timer', ?6, 1, 1, 'pending', 1, 0)",
            rusqlite::params![
                activation_id,
                room.room_id,
                i64::try_from(head.head().room_seq().get())?,
                "imo220-warm-timer-decision",
                navigator_member,
                "imo220-warm-timer-dedup",
            ],
        )?;
        drop(connection);
        let runner = session(0xd1, &runner_capability.capability_id);
        let initial_claim = backend
            .activation_claim(
                &runner,
                ActivationClaim {
                    activation_id: activation_id.clone(),
                    runner_id: runner_capability.runner_id.clone(),
                    claim_id: "imo220-warm-timer-initial".to_owned(),
                    requested_lease_ms: 30_000,
                },
            )
            .map_err(|error| format!("warm fixture initial claim: {error:?}"))?;
        assert_eq!(initial_claim.code, ActivationResultCode::Granted);
        let initial_generation = initial_claim
            .lease_generation
            .unwrap_or_else(|| panic!("initial warm Timer claim omitted lease generation"));
        assert_eq!(
            backend
                .activation_release(
                    &runner,
                    ActivationLeaseOperation {
                        activation_id: activation_id.clone(),
                        runner_id: runner_capability.runner_id.clone(),
                        claim_id: "imo220-warm-timer-initial".to_owned(),
                        operation_id: "imo220-warm-timer-initial-release".to_owned(),
                        lease_generation: initial_generation,
                        requested_lease_ms: None,
                        disposition: None,
                    },
                )?
                .code,
            ActivationResultCode::Released
        );
        let callbacks_before_timer = backend.trace_cache.with_room(&room_id, |slot| {
            slot.as_ref()
                .map(|cached| cached.trace().activity_callback_count())
                .unwrap_or_else(|| panic!("warm Timer executor was not installed"))
        })?;
        backend.forbid_recovery_for_test();
        *clock.0.lock().map_err(|_| "clock unavailable")? =
            HostClockSampleV1::new((now + time::Duration::seconds(60)).format(&Rfc3339)?)?;
        backend
            .scheduler_tick()
            .map_err(|error| format!("fixture due timer tick: {error:?}"))?;
        assert_eq!(
            backend
                .operator_room_detail(&host, &room.room_id)?
                .room_head
                .room_seq,
            2
        );
        let callbacks_after_timer = backend.trace_cache.with_room(&room_id, |slot| {
            slot.as_ref()
                .map(|cached| cached.trace().activity_callback_count())
                .unwrap_or_else(|| panic!("warm Timer executor was evicted"))
        })?;
        assert!(callbacks_after_timer > callbacks_before_timer);
        let timer_claim = backend
            .activation_claim(
                &runner,
                ActivationClaim {
                    activation_id: activation_id.clone(),
                    runner_id: runner_capability.runner_id.clone(),
                    claim_id: "imo220-warm-timer-after".to_owned(),
                    requested_lease_ms: 30_000,
                },
            )
            .map_err(|error| format!("warm fixture timer claim: {error:?}"))?;
        assert_eq!(timer_claim.code, ActivationResultCode::Granted);
        assert_eq!(
            backend
                .activation_release(
                    &runner,
                    ActivationLeaseOperation {
                        activation_id,
                        runner_id: runner_capability.runner_id,
                        claim_id: "imo220-warm-timer-after".to_owned(),
                        operation_id: "imo220-warm-timer-after-release".to_owned(),
                        lease_generation: timer_claim.lease_generation.unwrap_or_else(|| {
                            panic!("Timer-adjacent warm claim omitted lease generation")
                        }),
                        requested_lease_ms: None,
                        disposition: None,
                    },
                )?
                .code,
            ActivationResultCode::Released
        );
        Ok(())
    }

    async fn assert_scheduler_publication(
        backend: SqliteGatewayBackend,
        host: &GatewaySession,
        room: &CreateRoomResponse,
        clock: &SchedulerClock,
        now: OffsetDateTime,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Same acknowledged-registration/publication seam as lib.rs's
        // live_publication_does_not_duplicate_after_registered_catch_up.
        let issued = backend.issue_member_capability(
            host,
            crate::MemberCapabilityIssueRequest {
                room_id: room.room_id.clone(),
                member_id: room.member_ids[0].clone(),
                principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC2".to_owned(),
                scopes: vec![
                    CapabilityScopeV1::RoomAttach,
                    CapabilityScopeV1::RoomObserveMember,
                ],
                idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FE0".to_owned(),
                expires_at: None,
            },
        )?;
        let wire = BearerWireV1::parse(&issued.bearer)?;
        let bearer =
            CapabilityBearerV1::from_bytes(BearerWireV1::parse(&issued.bearer)?.into_bytes());
        let member = Arc::new(GatewaySession::new_with_wire(
            "01ARZ3NDEKTSV4RRFFQ69G5FE1".parse()?,
            bearer,
            wire,
        ));
        let attached = backend.attach(
            &member,
            RoomAttach {
                room_id: room.room_id.clone(),
                member_id: room.member_ids[0].clone(),
                after_frame_seq: None,
            },
        )?;
        backend.sync_ack(
            &member,
            RoomSyncAck {
                room_id: room.room_id.clone(),
                member_id: room.member_ids[0].clone(),
                through_frame_head: attached.attached.frame_head,
                sync_token: attached.attached.sync_token,
            },
        )?;
        let registry = crate::LiveStreamRegistry::default();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(crate::LIVE_PUSH_CAPACITY);
        let (close_sender, _close_receiver) = tokio::sync::watch::channel(None);
        registry.register(
            member,
            room.room_id.clone(),
            room.member_ids[0].clone(),
            attached.attached.frame_head,
            sender,
            close_sender,
        )?;
        let scheduler = crate::SchedulerRuntimeOwner::start(Arc::new(backend), registry)?;
        *clock.0.lock().map_err(|_| "clock unavailable")? =
            HostClockSampleV1::new((now + time::Duration::seconds(125)).format(&Rfc3339)?)?;
        let pushed =
            tokio::time::timeout(std::time::Duration::from_secs(3), receiver.recv()).await?;
        let Some(crate::LivePush::Frame(frame)) = pushed else {
            return Err("timer frame missing".into());
        };
        assert_eq!(frame.frame.cause_room_seq, 3);
        assert_eq!(frame.frame.member_id, room.member_ids[0]);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(350), receiver.recv(),)
                .await
                .is_err(),
            "scheduler must not republish the committed timer frame"
        );
        drop(scheduler);
        Ok(())
    }

    fn sealed_member_request(
        room_id: &str,
        member_id: &str,
        bearer_byte: u8,
    ) -> MemberCapabilityProvisionRequestV1 {
        MemberCapabilityProvisionRequestV1 {
            room_id: room_id.to_owned(),
            member_id: member_id.to_owned(),
            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC6".to_owned(),
            principal_kind: PrincipalKind::Agent,
            role: Some("counter".to_owned()),
            access_mode: AccessMode::Participant,
            scopes: vec![
                "room:attach".to_owned(),
                "room:act".to_owned(),
                "room:observe_member".to_owned(),
            ],
            capability: SealedCapabilityInputV1 {
                capability_id: "01ARZ3NDEKTSV4RRFFQ69G5FF0".to_owned(),
                capability_idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FF1".to_owned(),
                bearer: SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(
                    [bearer_byte; 32],
                )),
            },
            expires_at: None,
        }
    }

    fn hosted_counter_request(
        result_bearer_byte: u8,
        creator_bearer_byte: u8,
    ) -> HostedRoomCreationRequestV2 {
        HostedRoomCreationRequestV2 {
            schema: HOSTED_ROOM_CREATION_SCHEMA_V2.to_owned(),
            room: CreateRoomRequest {
                pack: PackReference {
                    id: "worldstream.counter".to_owned(),
                    version: "2.0.0".to_owned(),
                    digest: counter_v2_digest().to_string(),
                },
                configuration: json!({"initial_value": 0, "maximum_value": 16}),
                members: vec![
                    CreateMember {
                        principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC2".to_owned(),
                        principal_kind: PrincipalKind::Human,
                        role: Some("counter".to_owned()),
                        access_mode: AccessMode::Participant,
                    },
                    CreateMember {
                        principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD2".to_owned(),
                        principal_kind: PrincipalKind::Agent,
                        role: None,
                        access_mode: AccessMode::Spectator,
                    },
                    CreateMember {
                        principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD5".to_owned(),
                        principal_kind: PrincipalKind::Human,
                        role: None,
                        access_mode: AccessMode::Spectator,
                    },
                ],
                idempotency_key: "hosted-counter-room".to_owned(),
            },
            spectators: vec![
                HostedSpectatorCredentialInputV2 {
                    purpose: HostedSpectatorPurposeV2::ResultIndexer,
                    member_index: 1,
                    principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD2".to_owned(),
                    principal_kind: PrincipalKind::Agent,
                    capability: SealedCapabilityInputV1 {
                        capability_id: "01ARZ3NDEKTSV4RRFFQ69G5FD3".to_owned(),
                        capability_idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FD4".to_owned(),
                        bearer: SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(
                            [result_bearer_byte; 32],
                        )),
                    },
                },
                HostedSpectatorCredentialInputV2 {
                    purpose: HostedSpectatorPurposeV2::Creator,
                    member_index: 2,
                    principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD5".to_owned(),
                    principal_kind: PrincipalKind::Human,
                    capability: SealedCapabilityInputV1 {
                        capability_id: "01ARZ3NDEKTSV4RRFFQ69G5FD6".to_owned(),
                        capability_idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FD7".to_owned(),
                        bearer: SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(
                            [creator_bearer_byte; 32],
                        )),
                    },
                },
            ],
        }
    }

    #[test]
    fn hosted_room_creation_is_atomic_restartable_and_non_playing() {
        let (_database_directory, file) = database_fixture();
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let host_bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        AuthorityV1::new(Arc::new(store.clone()))
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FC4"
                        .parse()
                        .unwrap_or_else(|_| panic!("bootstrap change")),
                    "01ARZ3NDEKTSV4RRFFQ69G5FC2"
                        .parse()
                        .unwrap_or_else(|_| panic!("host principal")),
                    PrincipalKindV1::Human,
                    "01ARZ3NDEKTSV4RRFFQ69G5FC3"
                        .parse()
                        .unwrap_or_else(|_| panic!("host capability")),
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|_| panic!("bootstrap request")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|_| panic!("bootstrap time")),
            )
            .unwrap_or_else(|_| panic!("bootstrap authority"));
        let registry =
            Arc::new(builtin_counter_registry().unwrap_or_else(|_| panic!("counter registry")));
        let host = session(0xa9, "01ARZ3NDEKTSV4RRFFQ69G5FC5");
        let backend = SqliteGatewayBackend::new(store.clone(), Arc::clone(&registry));

        let failed = backend.create_hosted_room(&host, hosted_counter_request(0xc1, 0xc1));
        assert!(
            matches!(failed, Err(BackendError::InvalidResult)),
            "unexpected hosted failure: {failed:?}"
        );
        let empty = backend
            .operator_room_inventory(
                &host,
                OperatorRoomInventoryRequest {
                    after_room_id: None,
                    limit: 50,
                },
            )
            .unwrap_or_else(|error| panic!("inventory after rollback: {error:?}"));
        assert!(
            empty.rooms.is_empty(),
            "authority failure must roll Genesis back"
        );

        let first = backend
            .create_hosted_room(&host, hosted_counter_request(0xc1, 0xc2))
            .unwrap_or_else(|error| panic!("hosted create: {error:?}"));
        assert_eq!(first.schema, HOSTED_ROOM_CREATION_RESPONSE_SCHEMA_V2);
        assert_eq!(first.spectators.len(), 2);
        assert_eq!(
            first.spectators[0].scopes,
            ["room:attach", "room:observe_public", "room:replay"]
        );
        assert_eq!(
            first.spectators[1].scopes,
            ["room:attach", "room:observe_public"]
        );
        assert!(first.spectators.iter().all(|receipt| {
            receipt.room_id == first.room.room_id
                && !receipt.scopes.iter().any(|scope| scope == "room:act")
        }));

        let restarted = SqliteGatewayBackend::new(
            SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("restart db")),
            registry,
        );
        let duplicate = restarted
            .create_hosted_room(&host, hosted_counter_request(0xc1, 0xc2))
            .unwrap_or_else(|error| panic!("hosted retry: {error:?}"));
        assert_eq!(duplicate, first);

        let result_indexer = session(0xc1, "01ARZ3NDEKTSV4RRFFQ69G5FE0");
        let attached = restarted
            .attach(
                &result_indexer,
                RoomAttach {
                    room_id: first.room.room_id.clone(),
                    member_id: first.spectators[0].member_id.clone(),
                    after_frame_seq: None,
                },
            )
            .unwrap_or_else(|error| panic!("spectator attach: {error:?}"));
        assert_eq!(attached.attached.access_mode, AccessMode::Spectator);
        assert_eq!(attached.attached.role, None);
        restarted
            .replay(&result_indexer, &first.room.room_id, 0)
            .unwrap_or_else(|error| panic!("result-indexer replay: {error:?}"));
        assert!(matches!(
            restarted.action(
                &result_indexer,
                ActionSubmit {
                    room_id: first.room.room_id,
                    member_id: first.spectators[0].member_id.clone(),
                    action_id: "01ARZ3NDEKTSV4RRFFQ69G5FE1".to_owned(),
                    based_on_room_seq: 0,
                    action_type: "increment".to_owned(),
                    payload: json!({}),
                },
            ),
            Err(BackendError::Forbidden)
        ));
    }

    fn sealed_runner_request(
        room_id: &str,
        member_id: &str,
        bearer_byte: u8,
    ) -> RunnerCapabilityProvisionRequestV1 {
        RunnerCapabilityProvisionRequestV1 {
            runner_id: "01ARZ3NDEKTSV4RRFFQ69G5FF2".to_owned(),
            owner_principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC6".to_owned(),
            permitted_memberships: vec![RunnerMembershipProvisionTargetV1 {
                room_id: room_id.to_owned(),
                member_id: member_id.to_owned(),
            }],
            scopes: vec![
                "activation:offer_receive".to_owned(),
                "activation:claim".to_owned(),
                "activation:complete".to_owned(),
            ],
            principal_idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FC6".to_owned(),
            runner_idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FF3".to_owned(),
            capability: SealedCapabilityInputV1 {
                capability_id: "01ARZ3NDEKTSV4RRFFQ69G5FF4".to_owned(),
                capability_idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FF5".to_owned(),
                bearer: SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(
                    [bearer_byte; 32],
                )),
            },
            expires_at: None,
        }
    }

    #[test]
    fn repeated_warm_activation_claims_do_not_recover_or_read_history() {
        let (_database_directory, file) = database_fixture();
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let host_bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        authority
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FC4"
                        .parse()
                        .unwrap_or_else(|_| panic!("bootstrap change")),
                    "01ARZ3NDEKTSV4RRFFQ69G5FC2"
                        .parse()
                        .unwrap_or_else(|_| panic!("host principal")),
                    PrincipalKindV1::Human,
                    "01ARZ3NDEKTSV4RRFFQ69G5FC3"
                        .parse()
                        .unwrap_or_else(|_| panic!("host capability")),
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|_| panic!("bootstrap request")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|_| panic!("bootstrap time")),
            )
            .unwrap_or_else(|_| panic!("bootstrap authority"));
        let registry =
            Arc::new(builtin_counter_registry().unwrap_or_else(|_| panic!("counter registry")));
        let backend = SqliteGatewayBackend::new(store.clone(), Arc::clone(&registry));
        let host = session(0xa9, "01ARZ3NDEKTSV4RRFFQ69G5FC3");
        let created = backend
            .create_room(
                &host,
                CreateRoomRequest {
                    pack: PackReference {
                        id: "worldstream.counter".to_owned(),
                        version: "2.0.0".to_owned(),
                        digest: counter_v2_digest().to_string(),
                    },
                    configuration: json!({"initial_value": 0, "maximum_value": 16}),
                    members: vec![
                        CreateMember {
                            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC6".to_owned(),
                            principal_kind: PrincipalKind::Agent,
                            role: Some("counter".to_owned()),
                            access_mode: AccessMode::Participant,
                        },
                        CreateMember {
                            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC2".to_owned(),
                            principal_kind: PrincipalKind::Human,
                            role: Some("counter".to_owned()),
                            access_mode: AccessMode::Participant,
                        },
                    ],
                    idempotency_key: "warm-activation-room".to_owned(),
                },
            )
            .unwrap_or_else(|error| panic!("create activation Room: {error:?}"));
        let room_id = created.room_id;
        let member_id = created.member_ids[0].clone();
        let action_member_id = created.member_ids[1].clone();
        let action_capability = backend
            .issue_member_capability(
                &host,
                MemberCapabilityIssueRequest {
                    room_id: room_id.clone(),
                    member_id: action_member_id.clone(),
                    principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC2".to_owned(),
                    scopes: vec![CapabilityScopeV1::RoomAct],
                    idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FG2".to_owned(),
                    expires_at: None,
                },
            )
            .unwrap_or_else(|error| panic!("provision history Member: {error:?}"));
        let action_bearer = CapabilityBearerV1::from_bytes(
            BearerWireV1::parse(&action_capability.bearer)
                .unwrap_or_else(|_| panic!("history Member bearer"))
                .into_bytes(),
        );
        let action_member = GatewaySession::new_with_wire(
            "01ARZ3NDEKTSV4RRFFQ69G5FG3"
                .parse()
                .unwrap_or_else(|_| panic!("history Member session")),
            action_bearer,
            BearerWireV1::parse(&action_capability.bearer)
                .unwrap_or_else(|_| panic!("history Member wire")),
        );
        backend
            .action(
                &action_member,
                ActionSubmit {
                    room_id: room_id.clone(),
                    member_id: action_member_id,
                    action_id: "01ARZ3NDEKTSV4RRFFQ69G5FG4".to_owned(),
                    based_on_room_seq: 0,
                    action_type: "increment".to_owned(),
                    payload: json!({}),
                },
            )
            .unwrap_or_else(|error| panic!("seed activation history: {error:?}"));
        for (action_id, based_on_room_seq) in [
            ("01ARZ3NDEKTSV4RRFFQ69G5FG5", 1),
            ("01ARZ3NDEKTSV4RRFFQ69G5FG6", 2),
        ] {
            backend
                .action(
                    &action_member,
                    ActionSubmit {
                        room_id: room_id.clone(),
                        member_id: created.member_ids[1].clone(),
                        action_id: action_id.to_owned(),
                        based_on_room_seq,
                        action_type: "increment".to_owned(),
                        payload: json!({}),
                    },
                )
                .unwrap_or_else(|error| panic!("extend activation history: {error:?}"));
        }
        assert_eq!(
            backend
                .store
                .current_room_serving_fence(&room_id.parse().unwrap_or_else(|_| panic!("Room ID")))
                .unwrap_or_else(|error| panic!("read seeded serving fence: {error:?}"))
                .unwrap_or_else(|| panic!("seeded Room absent"))
                .head()
                .room_seq()
                .get(),
            3
        );
        let runner = sealed_runner_request(&room_id, &member_id, 0xd1);
        let runner_capability = backend
            .provision_runner_capability(&host, runner)
            .unwrap_or_else(|error| panic!("provision activation Runner: {error:?}"));
        let runner_session = session(0xd1, &runner_capability.capability_id);
        backend.forbid_recovery_for_test();
        let before_callbacks = backend
            .trace_cache
            .with_room(
                &room_id.parse().unwrap_or_else(|_| panic!("Room ID")),
                |slot| {
                    slot.as_ref()
                        .map(|cached| cached.trace().activity_callback_count())
                        .unwrap_or_else(|| panic!("warm trace was not cached"))
                },
            )
            .unwrap_or_else(|_| panic!("inspect warm trace"));
        assert!(before_callbacks >= 1);

        // A warm claim must not fall back to recovery. Add an unreadable
        // future history row after the cache is loaded; the durable claim CAS
        // still has to succeed from the cached executor and operational rows
        // alone. A recovery or historical verifier would fail closed, while
        // the bounded current serving fence ignores rows past Complete Head.
        let connection = rusqlite::Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open history mutator: {error}"));
        connection
            .execute(
                "INSERT INTO transitions(
                     room_id, transition_id, room_seq, transition_hash, previous_lineage_hash,
                     core_schema_version, pack_digest, core_state_hash, activity_state_hash,
                     authoritative_state_hash, transition_bytes
                 ) VALUES (?1, ?2, 999, ?3, ?3, ?4, ?5, ?3, ?3, ?3, x'00')",
                rusqlite::params![
                    room_id,
                    "01ARZ3NDEKTSV4RRFFQ69G5FG7",
                    "blake3:0000000000000000000000000000000000000000000000000000000000000000",
                    worldstream_core::CORE_SCHEMA_VERSION,
                    counter_v2_digest().to_string(),
                ],
            )
            .unwrap_or_else(|error| panic!("insert unreadable future history: {error}"));
        drop(connection);
        backend
            .store
            .current_room_serving_fence(&room_id.parse().unwrap_or_else(|_| panic!("Room ID")))
            .unwrap_or_else(|error| panic!("serving fence after history poison: {error:?}"));
        assert_eq!(
            backend
                .cached_runtime_state(&room_id.parse().unwrap_or_else(|_| panic!("Room ID")))
                .unwrap_or_else(|error| panic!("cached runtime state: {error:?}")),
            SqliteRoomRuntimeStateV1::Active
        );

        let activation_id = "warm-activation".to_owned();
        let connection = rusqlite::Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open Activation fixture: {error}"));
        connection
            .execute(
                "INSERT INTO activation_intents(
                     activation_id, room_id, cause_room_seq, decision_id, target_member_id,
                     reason_code, deduplication_key, priority, policy_revision, state,
                     intent_generation, lease_generation
                 ) VALUES (?1, ?2, 3, ?3, ?4, 'warm-test', ?5, 1, 1, 'pending', 1, 0)",
                rusqlite::params![
                    activation_id,
                    room_id,
                    "warm-decision",
                    member_id,
                    "warm-dedup",
                ],
            )
            .unwrap_or_else(|error| panic!("insert warm Activation: {error}"));
        drop(connection);
        // Exercise the actual warm preparation path on every iteration.  A
        // retry with the same claim identity is intentionally served by the
        // durable receipt lookup before the executor is borrowed, so each
        // iteration claims a fresh identity and releases its lease before the
        // next claim.  This mirrors a Runner repeatedly starting bounded
        // Invocations while retaining one current executor.
        let mut previous_claim: Option<(String, u64)> = None;
        for iteration in 0..3 {
            if let Some((claim_id, lease_generation)) = previous_claim.take() {
                let released = backend
                    .activation_release(
                        &runner_session,
                        ActivationLeaseOperation {
                            activation_id: activation_id.clone(),
                            runner_id: runner_capability.runner_id.clone(),
                            claim_id,
                            operation_id: format!("warm-release-{iteration}"),
                            lease_generation,
                            requested_lease_ms: None,
                            disposition: None,
                        },
                    )
                    .unwrap_or_else(|error| panic!("warm Activation release: {error:?}"));
                assert_eq!(released.code, ActivationResultCode::Released);
            }
            let claim_id = format!("warm-claim-{iteration}");
            let reply = backend
                .activation_claim(
                    &runner_session,
                    ActivationClaim {
                        activation_id: activation_id.clone(),
                        runner_id: runner_capability.runner_id.clone(),
                        claim_id: claim_id.clone(),
                        requested_lease_ms: 30_000,
                    },
                )
                .unwrap_or_else(|error| panic!("warm Activation claim: {error:?}"));
            assert_eq!(reply.code, ActivationResultCode::Granted);
            previous_claim = Some((
                claim_id,
                reply
                    .lease_generation
                    .unwrap_or_else(|| panic!("warm claim omitted lease generation")),
            ));
        }
        if let Some((claim_id, lease_generation)) = previous_claim {
            let released = backend
                .activation_release(
                    &runner_session,
                    ActivationLeaseOperation {
                        activation_id: activation_id.clone(),
                        runner_id: runner_capability.runner_id.clone(),
                        claim_id,
                        operation_id: "warm-release-final".to_owned(),
                        lease_generation,
                        requested_lease_ms: None,
                        disposition: None,
                    },
                )
                .unwrap_or_else(|error| panic!("warm final Activation release: {error:?}"));
            assert_eq!(released.code, ActivationResultCode::Released);
        }
        if let Some(scales) = std::env::var_os("WORLDSTREAM_WARM_CLAIM_SCALES") {
            let scales = scales
                .to_str()
                .unwrap_or_else(|| panic!("warm claim scales must be UTF-8"))
                .split(',')
                .map(|scale| {
                    scale
                        .parse::<usize>()
                        .unwrap_or_else(|error| panic!("invalid warm claim scale: {error}"))
                })
                .collect::<Vec<_>>();
            assert!(!scales.is_empty());
            assert!(scales.iter().all(|scale| *scale > 0));
            let before_history_rows: i64 = rusqlite::Connection::open(file.path())
                .unwrap_or_else(|error| panic!("open history counter: {error}"))
                .query_row(
                    "SELECT count(*) FROM transitions WHERE room_id = ?1",
                    [&room_id],
                    |row| row.get(0),
                )
                .unwrap_or_else(|error| panic!("count history rows: {error}"));
            const MAX_LATENCY_SAMPLES: usize = 4096;
            let mut previous_claim: Option<(String, u64)> = None;
            for scale in scales {
                let mut samples = Vec::with_capacity(MAX_LATENCY_SAMPLES.min(scale));
                for iteration in 0..scale {
                    if let Some((claim_id, lease_generation)) = previous_claim.take() {
                        let released = backend
                            .activation_release(
                                &runner_session,
                                ActivationLeaseOperation {
                                    activation_id: activation_id.clone(),
                                    runner_id: runner_capability.runner_id.clone(),
                                    claim_id,
                                    operation_id: format!("warm-scale-release-{scale}-{iteration}"),
                                    lease_generation,
                                    requested_lease_ms: None,
                                    disposition: None,
                                },
                            )
                            .unwrap_or_else(|error| {
                                panic!("scaled warm Activation release: {error:?}")
                            });
                        assert_eq!(released.code, ActivationResultCode::Released);
                    }
                    let claim_id = format!("warm-scale-claim-{scale}-{iteration}");
                    let started = Instant::now();
                    let reply = backend
                        .activation_claim(
                            &runner_session,
                            ActivationClaim {
                                activation_id: activation_id.clone(),
                                runner_id: runner_capability.runner_id.clone(),
                                claim_id: claim_id.clone(),
                                requested_lease_ms: 30_000,
                            },
                        )
                        .unwrap_or_else(|error| panic!("scaled warm Activation claim: {error:?}"));
                    assert_eq!(reply.code, ActivationResultCode::Granted);
                    previous_claim = Some((
                        claim_id,
                        reply
                            .lease_generation
                            .unwrap_or_else(|| panic!("scaled claim omitted lease generation")),
                    ));
                    let elapsed = started.elapsed();
                    if samples.len() < MAX_LATENCY_SAMPLES {
                        samples.push(elapsed);
                    } else {
                        // Deterministic bounded reservoir: no claim Context,
                        // receipt, or 100k-duration allocation is retained.
                        samples[iteration % MAX_LATENCY_SAMPLES] = elapsed;
                    }
                }
                if let Some((claim_id, lease_generation)) = previous_claim.take() {
                    let released = backend
                        .activation_release(
                            &runner_session,
                            ActivationLeaseOperation {
                                activation_id: activation_id.clone(),
                                runner_id: runner_capability.runner_id.clone(),
                                claim_id,
                                operation_id: format!("warm-scale-final-release-{scale}"),
                                lease_generation,
                                requested_lease_ms: None,
                                disposition: None,
                            },
                        )
                        .unwrap_or_else(|error| {
                            panic!("scaled final warm Activation release: {error:?}")
                        });
                    assert_eq!(released.code, ActivationResultCode::Released);
                }
                samples.sort_unstable();
                let percentile = |numerator: usize| {
                    samples[((samples.len().saturating_sub(1) * numerator) / 100)
                        .min(samples.len() - 1)]
                };
                eprintln!(
                    "warm_activation_claims scale={scale} history_rows={before_history_rows} p50_us={} p95_us={} p99_us={}",
                    percentile(50).as_micros(),
                    percentile(95).as_micros(),
                    percentile(99).as_micros(),
                );
            }
            let after_history_rows: i64 = rusqlite::Connection::open(file.path())
                .unwrap_or_else(|error| panic!("reopen history counter: {error}"))
                .query_row(
                    "SELECT count(*) FROM transitions WHERE room_id = ?1",
                    [&room_id],
                    |row| row.get(0),
                )
                .unwrap_or_else(|error| panic!("recount history rows: {error}"));
            assert_eq!(after_history_rows, before_history_rows);
        }

        // A normal Action advances the same cached executor.  The forged
        // unreadable row is deliberately beyond the current Head, so this
        // proves that an intervening canonical update does not make the next
        // Activation claim fall back to Genesis recovery.
        let action_before_callbacks = backend
            .trace_cache
            .with_room(
                &room_id.parse().unwrap_or_else(|_| panic!("Room ID")),
                |slot| {
                    slot.as_ref()
                        .map(|cached| cached.trace().activity_callback_count())
                        .unwrap_or_else(|| panic!("warm trace was evicted before Action"))
                },
            )
            .unwrap_or_else(|_| panic!("inspect warm trace before Action"));
        backend
            .action(
                &action_member,
                ActionSubmit {
                    room_id: room_id.clone(),
                    member_id: created.member_ids[1].clone(),
                    action_id: "01ARZ3NDEKTSV4RRFFQ69G5FH0".to_owned(),
                    based_on_room_seq: 3,
                    action_type: "increment".to_owned(),
                    payload: json!({}),
                },
            )
            .unwrap_or_else(|error| panic!("intervening warm Action: {error:?}"));
        let action_after_callbacks = backend
            .trace_cache
            .with_room(
                &room_id.parse().unwrap_or_else(|_| panic!("Room ID")),
                |slot| {
                    slot.as_ref()
                        .map(|cached| cached.trace().activity_callback_count())
                        .unwrap_or_else(|| panic!("warm trace was evicted after Action"))
                },
            )
            .unwrap_or_else(|_| panic!("inspect warm trace after Action"));
        assert!(action_after_callbacks > action_before_callbacks);
        let after_action_claim = backend
            .activation_claim(
                &runner_session,
                ActivationClaim {
                    activation_id: activation_id.clone(),
                    runner_id: runner_capability.runner_id.clone(),
                    claim_id: "warm-after-action-claim".to_owned(),
                    requested_lease_ms: 30_000,
                },
            )
            .unwrap_or_else(|error| panic!("warm Activation claim after Action: {error:?}"));
        assert_eq!(after_action_claim.code, ActivationResultCode::Granted);
        let released = backend
            .activation_release(
                &runner_session,
                ActivationLeaseOperation {
                    activation_id,
                    runner_id: runner_capability.runner_id,
                    claim_id: "warm-after-action-claim".to_owned(),
                    operation_id: "warm-after-action-release".to_owned(),
                    lease_generation: after_action_claim
                        .lease_generation
                        .unwrap_or_else(|| panic!("warm Action claim omitted lease generation")),
                    requested_lease_ms: None,
                    disposition: None,
                },
            )
            .unwrap_or_else(|error| panic!("warm Activation release after Action: {error:?}"));
        assert_eq!(released.code, ActivationResultCode::Released);
        let after_callbacks = backend
            .trace_cache
            .with_room(
                &room_id.parse().unwrap_or_else(|_| panic!("Room ID")),
                |slot| {
                    slot.as_ref()
                        .map(|cached| cached.trace().activity_callback_count())
                        .unwrap_or_else(|| panic!("warm trace was evicted"))
                },
            )
            .unwrap_or_else(|_| panic!("inspect post-claim trace"));
        assert_eq!(
            after_callbacks, action_after_callbacks,
            "the post-Action warm claim must not invoke the canonical reducer"
        );
    }

    fn warm_claim_percentile_us(samples: &mut [std::time::Duration], percentile: usize) -> u64 {
        assert!(!samples.is_empty(), "warm Claim sample set is empty");
        samples.sort_unstable();
        u64::try_from(
            samples[((samples.len().saturating_sub(1) * percentile) / 100).min(samples.len() - 1)]
                .as_micros(),
        )
        .unwrap_or(u64::MAX)
    }

    fn warm_claim_rss_bytes() -> Option<u64> {
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let kib = std::str::from_utf8(&output.stdout)
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()?;
        kib.checked_mul(1024)
    }

    /// Verifies the compact V2 checkpoint contract at its actual boundary.
    ///
    /// A snapshot witness carries the head and compact roots at
    /// `checkpoint_room_seq`, while a recovered Room normally has a later
    /// current head after its tail is replayed. The recovery guard advances
    /// the roots through that tail before comparing them with durable storage;
    /// comparing the boundary roots directly to current rows would reject
    /// every checkpoint with a nonempty tail.
    fn verify_source_v2_checkpoint_contract(
        source: &std::path::Path,
        room_id: &RoomId,
        recovered_current_head: &worldstream_core::CompleteHeadV1,
        recovery_receipt: worldstream_core::RoomRecoveryExecutionReceiptV1,
    ) -> serde_json::Value {
        let connection = rusqlite::Connection::open(source)
            .unwrap_or_else(|error| panic!("open V2 checkpoint verifier source: {error}"));
        let room_id_text = room_id.to_string();
        let (checkpoint_room_seq, checkpoint_head_bytes, witness_schema, witness_hash, witness_bytes): (
            i64,
            Vec<u8>,
            String,
            Vec<u8>,
            Vec<u8>,
        ) = connection
            .query_row(
                "SELECT snapshots.room_seq, snapshots.complete_head_bytes, \
                        witness.witness_schema_version, witness.witness_hash, witness.witness_bytes \
                 FROM room_snapshots AS snapshots \
                 JOIN room_snapshot_operational_witnesses_v2 AS witness \
                   ON witness.room_id = snapshots.room_id AND witness.room_seq = snapshots.room_seq \
                 WHERE snapshots.room_id = ?1 \
                 ORDER BY snapshots.room_seq DESC LIMIT 1",
                [&room_id_text],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap_or_else(|error| panic!("read V2 checkpoint witness: {error}"));
        assert_eq!(
            witness_schema,
            worldstream_core::CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V2,
            "selected checkpoint must use the compact V2 witness schema"
        );
        let checkpoint_room_seq = u64::try_from(checkpoint_room_seq)
            .unwrap_or_else(|_| panic!("negative V2 checkpoint Room sequence"));
        let checkpoint_head = worldstream_core::CanonicalJsonV1::decode_canonical::<
            worldstream_core::CompleteHeadV1,
        >(&checkpoint_head_bytes)
        .unwrap_or_else(|error| panic!("decode V2 checkpoint head: {error:?}"));
        assert_eq!(checkpoint_head.room_seq().get(), checkpoint_room_seq);
        let witness = worldstream_core::RoomCheckpointOperationalWitnessV2::from_canonical_bytes(
            &witness_bytes,
            &checkpoint_head,
        )
        .unwrap_or_else(|error| panic!("decode V2 checkpoint witness: {error:?}"));
        let witness_hash_exact = witness_hash.as_slice()
            == worldstream_core::Blake3DigestV1::hash(&witness_bytes)
                .as_bytes()
                .as_slice();
        assert!(witness_hash_exact, "V2 witness hash differs from its bytes");
        assert_eq!(
            witness.checkpoint_head(),
            &checkpoint_head,
            "V2 witness head differs from its selected snapshot head"
        );
        assert!(
            checkpoint_head.room_seq().get() <= recovered_current_head.room_seq().get(),
            "checkpoint must not be ahead of the recovered current head"
        );
        assert!(
            recovery_receipt.used_checkpoint(),
            "source recovery must use the selected V2 checkpoint"
        );
        assert_eq!(
            recovery_receipt
                .checkpoint_room_seq()
                .map(|sequence| sequence.get()),
            Some(checkpoint_room_seq),
            "source recovery selected a different checkpoint boundary"
        );
        assert_eq!(
            recovery_receipt.prefix_transition_records_delivered(),
            0,
            "checkpoint recovery must not deliver a canonical prefix to Core"
        );
        assert_eq!(
            recovery_receipt.prefix_transitions_skipped(),
            checkpoint_room_seq,
            "checkpoint recovery skipped the wrong canonical prefix"
        );
        let expected_tail = recovered_current_head
            .room_seq()
            .get()
            .checked_sub(checkpoint_room_seq)
            .unwrap_or_else(|| panic!("checkpoint tail underflow"));
        assert_eq!(
            recovery_receipt.tail_transition_records_delivered(),
            expected_tail,
            "checkpoint recovery delivered the wrong canonical tail"
        );

        let stored_roots = {
            let mut statement = connection
                .prepare(
                    "SELECT domain, entry_count, root_hash \
                     FROM room_operational_history_roots_v2 \
                     WHERE room_id = ?1 ORDER BY domain",
                )
                .unwrap_or_else(|error| panic!("prepare V2 root verifier: {error}"));
            statement
                .query_map([&room_id_text], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })
                .unwrap_or_else(|error| panic!("read V2 operational roots: {error}"))
                .collect::<Result<Vec<_>, _>>()
                .unwrap_or_else(|error| panic!("decode V2 operational roots: {error}"))
        };
        let root_inventory_exact = stored_roots.len() == witness.operational_history_roots().len()
            && stored_roots.iter().all(|(domain, count, hash)| {
                witness.operational_history_roots().contains_key(domain)
                    && *count >= 0
                    && hash.len() == 32
            });
        assert!(
            root_inventory_exact,
            "durable V2 operational root inventory is malformed"
        );
        // `recover_room_from_storage_with_receipt` can return the checkpoint
        // receipt only after SQLite's guarded install has called
        // `verify_sqlite_v2_roots` with Core's tail-advanced roots. Its
        // success is the exact final-root proof; the boundary witness above
        // separately authenticates the selected snapshot root/head.
        json!({
            "schema": worldstream_core::CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V2,
            "checkpoint_room_seq": checkpoint_room_seq,
            "checkpoint_head_room_seq": checkpoint_head.room_seq().get(),
            "recovered_current_head_room_seq": recovered_current_head.room_seq().get(),
            "witness_hash_exact": witness_hash_exact,
            "witness_snapshot_head_exact": witness.checkpoint_head() == &checkpoint_head,
            "durable_operational_root_count": stored_roots.len(),
            "durable_operational_root_inventory_exact": root_inventory_exact,
            "durable_operational_roots_exact": true,
            "durable_operational_roots_verified_by": "guarded_checkpoint_v2_recovery",
            "checkpoint_recovery_tail_transition_records": recovery_receipt.tail_transition_records_delivered(),
        })
    }

    /// Qualifies the actual production warm Activation preparation path against
    /// one generated canonical-history tier. The source is intentionally
    /// mutable only when the caller opts in: after the cold cache-miss claim,
    /// this test corrupts an in-head historical record to prove every following
    /// claim comes from the retained executor plus the bounded serving fence,
    /// rather than replaying that history.
    ///
    /// `scripts/imo-220-warm-claim-qualification.sh` creates a fresh source
    /// with `history_qualification_fixture`, invokes this test for 1k/10k/100k,
    /// and retains only the redacted JSON emitted below.
    #[test]
    #[ignore = "requires a fresh WORLDSTREAM_WARM_CLAIM_SOURCE_DB generated by the qualification fixture"]
    #[allow(clippy::too_many_lines)]
    fn source_backed_warm_activation_claim_qualification() {
        const ROOM_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        const HUMAN_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
        const HOST_PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
        const HOST_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH2";
        const AGENT_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ1";
        const AGENT_PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC6";
        const JOIN_OPERATION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ2";
        const JOIN_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ3";
        const RUNNER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF2";
        const RUNNER_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF4";
        const MEMBER_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ7";

        assert_eq!(
            env::var("WORLDSTREAM_WARM_CLAIM_MUTABLE_SOURCE").as_deref(),
            Ok("1"),
            "the source is deliberately mutated after cache installation; use the fixture-owned disposable source"
        );
        let source = env::var_os("WORLDSTREAM_WARM_CLAIM_SOURCE_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| panic!("missing WORLDSTREAM_WARM_CLAIM_SOURCE_DB"));
        let expected_history = env::var("WORLDSTREAM_WARM_CLAIM_EXPECTED_HISTORY")
            .unwrap_or_else(|_| panic!("missing WORLDSTREAM_WARM_CLAIM_EXPECTED_HISTORY"))
            .parse::<u64>()
            .unwrap_or_else(|error| panic!("invalid expected history: {error}"));
        let warm_claim_count = env::var("WORLDSTREAM_WARM_CLAIM_SAMPLES")
            .unwrap_or_else(|_| "1000".to_owned())
            .parse::<usize>()
            .unwrap_or_else(|error| panic!("invalid warm claim sample count: {error}"));
        assert!(expected_history > 0);
        assert!(warm_claim_count > 0);

        let store = SqliteRoomStore::open(&source)
            .unwrap_or_else(|error| panic!("open qualification source: {error}"));
        assert_eq!(
            store.source_transfer_state(),
            worldstream_sqlite::SqliteSourceTransferStateV1::SourceAuthoritative,
            "the source must carry the fixture's verified source-authoritative metadata"
        );
        let registry =
            Arc::new(builtin_counter_registry().unwrap_or_else(|_| panic!("counter registry")));
        let room_id = ROOM_ID
            .parse::<RoomId>()
            .unwrap_or_else(|_| panic!("qualification Room ID"));
        let source_transition_rows: u64 = rusqlite::Connection::open(&source)
            .unwrap_or_else(|error| panic!("open source row counter: {error}"))
            .query_row(
                "SELECT count(*) FROM transitions WHERE room_id = ?1",
                [ROOM_ID],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| u64::try_from(count).unwrap_or_else(|_| panic!("negative row count")))
            .unwrap_or_else(|error| panic!("count source transitions: {error}"));
        assert_eq!(
            source_transition_rows, expected_history,
            "fixture transition count must be the requested canonical history tier"
        );

        // Append one normal Core-authorized Agent Membership so the unchanged
        // fixture's human-only counter Room can legitimately receive an
        // Activation target. This is a canonical transition, never SQL state
        // fabrication, and it makes the subsequent Runner authority binding
        // exercise the ordinary production checks.
        let source_recovery = worldstream_core::recover_room_from_storage_with_receipt(
            &store,
            registry.as_ref(),
            &room_id,
        )
        .unwrap_or_else(|error| panic!("recover qualification source: {error:?}"))
        .unwrap_or_else(|| panic!("qualification source Room absent"));
        let source_recovery_receipt = source_recovery.receipt();
        let source_recovery_head = source_recovery.trace().head().clone();
        let checkpoint_v2_contract = verify_source_v2_checkpoint_contract(
            &source,
            &room_id,
            &source_recovery_head,
            source_recovery_receipt,
        );
        assert_eq!(
            source_recovery_head.room_seq().get(),
            source_transition_rows,
            "recovered source head must match its canonical Transition row count"
        );
        let join_basis = source_recovery_head.room_seq();
        // The qualifier already has an authenticated current Head from the
        // guarded recovery above. Calling `gateway_room_snapshot` here would
        // materialize the historical inspection view solely to recover that
        // same Head, making the test setup consume O(history) memory before it
        // reaches the bounded warm path being measured.
        drop(source_recovery);
        let joined = MembershipV1::new(
            AGENT_MEMBER
                .parse()
                .unwrap_or_else(|_| panic!("Agent Member ID")),
            AGENT_PRINCIPAL
                .parse()
                .unwrap_or_else(|_| panic!("Agent Principal ID")),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )
        .unwrap_or_else(|error| panic!("Agent Membership shape: {error:?}"));
        let join = CoreAdministrationRequestV1::new(
            room_id.clone(),
            AdministrationOperationIdentityV1 {
                authenticated_principal: HOST_PRINCIPAL
                    .parse()
                    .unwrap_or_else(|_| panic!("host Principal")),
                versioned_operation_kind: worldstream_core::CORE_OPERATION_KIND.to_owned(),
                idempotency_key: JOIN_OPERATION.to_owned(),
            },
            CoreProposedKindV1::Join,
            join_basis,
            "imo220_warm_claim_agent_join",
            CoreChangeSetV1::one(MembershipChangeV1::join(joined)),
        )
        .unwrap_or_else(|error| panic!("prepare Agent Join request: {error:?}"));
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let presented = PresentedCapabilityV1::new(
            HOST_CAPABILITY
                .parse()
                .unwrap_or_else(|_| panic!("host Capability")),
            CapabilityBearerV1::from_bytes([0xa7; 32]),
        );
        let join_grant = match authorize_core_administration_operation(
            &authority,
            &store,
            &presented,
            &join,
            "2026-08-15T12:00:00Z"
                .parse()
                .unwrap_or_else(|_| panic!("join checked at")),
        )
        .unwrap_or_else(|error| panic!("authorize Agent Join: {error:?}"))
        {
            CoreAdministrationIngressV1::Authorized(grant) => *grant,
            other => panic!("Agent Join must be newly authorized: {other:?}"),
        };
        let join_resolution = store
            .commit_authorized_core_administration(
                &registry,
                join_grant,
                &join,
                "2026-08-15T12:00:00Z"
                    .parse()
                    .unwrap_or_else(|_| panic!("join recorded at")),
                JOIN_TRANSITION
                    .parse::<TransitionId>()
                    .unwrap_or_else(|_| panic!("join Transition ID")),
            )
            .unwrap_or_else(|error| panic!("commit Agent Join: {error:?}"));
        assert!(
            matches!(
                join_resolution,
                RoomCommitResolutionV1::TransitionCommitted { .. }
            ),
            "Agent Join did not advance canonical state: {join_resolution:?}"
        );

        // The membership was just committed through the production Core
        // administration path. Register the two fixed qualification
        // capabilities through the production authority API directly. Going
        // back through the operator provisioning endpoints here would perform
        // two unrelated historical diagnostic inspections before the one
        // cache-miss claim this test intentionally measures.
        let agent_principal = AGENT_PRINCIPAL
            .parse::<worldstream_core::PrincipalId>()
            .unwrap_or_else(|_| panic!("Agent Principal ID"));
        let runner_id = RUNNER_ID
            .parse::<RunnerId>()
            .unwrap_or_else(|_| panic!("qualification Runner ID"));
        authority
            .change(
                &presented,
                AuthorityChangeV1::CreatePrincipal {
                    change_id: AGENT_PRINCIPAL
                        .parse()
                        .unwrap_or_else(|_| panic!("Agent Principal change ID")),
                    principal_id: agent_principal.clone(),
                    kind: PrincipalKindV1::Agent,
                },
                SqliteGatewayBackend::checked_at()
                    .unwrap_or_else(|_| panic!("Agent Principal checked time")),
            )
            .unwrap_or_else(|error| panic!("register qualification Agent: {error:?}"));
        authority
            .change(
                &presented,
                AuthorityChangeV1::RegisterRunner {
                    change_id: "01ARZ3NDEKTSV4RRFFQ69G5FF3"
                        .parse()
                        .unwrap_or_else(|_| panic!("Runner change ID")),
                    runner_id: runner_id.clone(),
                    owner_principal_id: agent_principal.clone(),
                },
                SqliteGatewayBackend::checked_at()
                    .unwrap_or_else(|_| panic!("Runner checked time")),
            )
            .unwrap_or_else(|error| panic!("register qualification Runner: {error:?}"));
        let runner_memberships = RunnerMembershipSetV1::new([RoomMembershipKeyV1 {
            room_id: room_id.clone(),
            member_id: AGENT_MEMBER
                .parse()
                .unwrap_or_else(|_| panic!("Agent Member ID")),
        }])
        .unwrap_or_else(|error| panic!("qualification Runner memberships: {error:?}"));
        let runner_bearer = CapabilityBearerV1::from_bytes([0xd1; 32]);
        let runner_capability = NewCapabilityV1::new(
            RUNNER_CAPABILITY
                .parse()
                .unwrap_or_else(|_| panic!("Runner Capability ID")),
            runner_bearer.token_hash(),
            agent_principal,
            CapabilityProfileV1::RunnerControl {
                runner_id: runner_id.clone(),
                permitted_memberships: runner_memberships,
            },
            CapabilityScopeSetV1::new([
                CapabilityScopeV1::ActivationOfferReceive,
                CapabilityScopeV1::ActivationClaim,
                CapabilityScopeV1::ActivationComplete,
            ])
            .unwrap_or_else(|error| panic!("qualification Runner scopes: {error:?}")),
            None,
        )
        .unwrap_or_else(|error| panic!("qualification Runner Capability: {error:?}"));
        authority
            .change(
                &presented,
                AuthorityChangeV1::RegisterCapability {
                    change_id: "01ARZ3NDEKTSV4RRFFQ69G5FF5"
                        .parse()
                        .unwrap_or_else(|_| panic!("Runner Capability change ID")),
                    capability: runner_capability,
                },
                SqliteGatewayBackend::checked_at()
                    .unwrap_or_else(|_| panic!("Runner Capability checked time")),
            )
            .unwrap_or_else(|error| panic!("register qualification Runner Capability: {error:?}"));
        let member_bearer = CapabilityBearerV1::from_bytes([0xb8; 32]);
        let member_capability = NewCapabilityV1::new(
            MEMBER_CAPABILITY
                .parse()
                .unwrap_or_else(|_| panic!("Member Capability ID")),
            member_bearer.token_hash(),
            HOST_PRINCIPAL
                .parse()
                .unwrap_or_else(|_| panic!("host Principal ID")),
            CapabilityProfileV1::RoomMember {
                room_id: room_id.clone(),
                member_id: HUMAN_MEMBER
                    .parse()
                    .unwrap_or_else(|_| panic!("human Member ID")),
            },
            CapabilityScopeSetV1::new([
                CapabilityScopeV1::RoomAct,
                CapabilityScopeV1::RoomObserveMember,
            ])
            .unwrap_or_else(|error| panic!("qualification Member scopes: {error:?}")),
            None,
        )
        .unwrap_or_else(|error| panic!("qualification Member Capability: {error:?}"));
        authority
            .change(
                &presented,
                AuthorityChangeV1::RegisterCapability {
                    change_id: "01ARZ3NDEKTSV4RRFFQ69G5FJ4"
                        .parse()
                        .unwrap_or_else(|_| panic!("Member Capability change ID")),
                    capability: member_capability,
                },
                SqliteGatewayBackend::checked_at()
                    .unwrap_or_else(|_| panic!("Member Capability checked time")),
            )
            .unwrap_or_else(|error| panic!("register qualification Member Capability: {error:?}"));
        let runner = session(0xd1, RUNNER_CAPABILITY);
        let member = session(0xb8, MEMBER_CAPABILITY);
        let host = session(0xa7, HOST_CAPABILITY);

        let head = store
            .current_room_serving_fence(&room_id)
            .unwrap_or_else(|error| panic!("read post-Join serving fence: {error:?}"))
            .unwrap_or_else(|| panic!("post-Join serving fence absent"));
        let canonical_transition_rows = rusqlite::Connection::open(&source)
            .unwrap_or_else(|error| panic!("open post-Join row counter: {error}"))
            .query_row(
                "SELECT count(*) FROM transitions WHERE room_id = ?1",
                [ROOM_ID],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| u64::try_from(count).unwrap_or_else(|_| panic!("negative row count")))
            .unwrap_or_else(|error| panic!("count post-Join transitions: {error}"));
        assert_eq!(canonical_transition_rows, expected_history + 1);
        let activation_id = format!("imo220-warm-activation-{expected_history}");
        let connection = rusqlite::Connection::open(&source)
            .unwrap_or_else(|error| panic!("open Activation fixture: {error}"));
        connection
            .execute(
                "INSERT INTO activation_intents(\
                 activation_id, room_id, cause_room_seq, decision_id, target_member_id,\
                 reason_code, deduplication_key, priority, policy_revision, state,\
                 intent_generation, lease_generation, created_at)\
                 VALUES (?1, ?2, ?3, ?4, ?5, 'imo220-warm', ?6, 1, 1, 'pending', 1, 0, \
                         strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                rusqlite::params![
                    activation_id,
                    ROOM_ID,
                    i64::try_from(head.head().room_seq().get())
                        .unwrap_or_else(|_| panic!("head sequence exceeds SQLite range")),
                    format!("imo220-warm-decision-{expected_history}"),
                    AGENT_MEMBER,
                    format!("imo220-warm-dedup-{expected_history}"),
                ],
            )
            .unwrap_or_else(|error| panic!("seed qualification Activation: {error}"));
        drop(connection);

        let backend = SqliteGatewayBackend::new(store.clone(), Arc::clone(&registry));
        let cold_started = Instant::now();
        let cold_claim = backend
            .activation_claim(
                &runner,
                ActivationClaim {
                    activation_id: activation_id.clone(),
                    runner_id: runner_id.to_string(),
                    claim_id: "imo220-cold-claim".to_owned(),
                    requested_lease_ms: 30_000,
                },
            )
            .unwrap_or_else(|error| panic!("cold Activation claim: {error:?}"));
        let cold_elapsed_us = u64::try_from(cold_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        assert_eq!(cold_claim.code, ActivationResultCode::Granted);
        let cold_context_bytes = cold_claim
            .context
            .as_ref()
            .map(|context| {
                serde_json::to_vec(context)
                    .unwrap_or_else(|error| panic!("encode redacted context size: {error}"))
                    .len()
            })
            .unwrap_or_else(|| panic!("cold claim omitted invocation Context"));
        let cold_generation = cold_claim
            .lease_generation
            .unwrap_or_else(|| panic!("cold claim omitted lease generation"));
        let cold_release = backend
            .activation_release(
                &runner,
                ActivationLeaseOperation {
                    activation_id: activation_id.clone(),
                    runner_id: runner_id.to_string(),
                    claim_id: "imo220-cold-claim".to_owned(),
                    operation_id: "imo220-cold-release".to_owned(),
                    lease_generation: cold_generation,
                    requested_lease_ms: None,
                    disposition: None,
                },
            )
            .unwrap_or_else(|error| panic!("cold Activation release: {error:?}"));
        assert_eq!(cold_release.code, ActivationResultCode::Released);

        let callbacks_before = backend
            .trace_cache
            .with_room(&room_id, |slot| {
                slot.as_ref()
                    .map(|cached| cached.trace().activity_callback_count())
                    .unwrap_or_else(|| panic!("cold claim did not install cached trace"))
            })
            .unwrap_or_else(|_| panic!("inspect cold cached trace"));
        let fence_before = store
            .current_room_serving_fence(&room_id)
            .unwrap_or_else(|error| panic!("read warm serving fence: {error:?}"))
            .unwrap_or_else(|| panic!("warm serving fence absent"));

        // This row is inside the current canonical prefix. A recovery or a
        // whole-history verifier after this point would fail. A warm claim
        // must instead validate only the bounded current materialization and
        // its ordinary activation witnesses.
        let connection = rusqlite::Connection::open(&source)
            .unwrap_or_else(|error| panic!("open history corruptor: {error}"));
        // The fixture source is explicitly disposable. Its normal immutable
        // Transition guard is removed only to make an in-prefix corruption
        // observable to a hypothetical replay; production callers cannot
        // perform this mutation.
        connection
            .execute_batch("DROP TRIGGER transitions_immutable_update;")
            .unwrap_or_else(|error| panic!("open disposable corruption seam: {error}"));
        assert_eq!(
            connection
                .execute(
                    "UPDATE transitions SET transition_bytes = x'00' \
                     WHERE room_id = ?1 AND room_seq = 1",
                    [ROOM_ID],
                )
                .unwrap_or_else(|error| panic!("corrupt in-head history row: {error}")),
            1,
            "fixture must have canonical Transition 1 to corrupt"
        );
        drop(connection);
        backend.forbid_recovery_for_test();

        const MAX_LATENCY_SAMPLES: usize = 4096;
        const MAX_RSS_SAMPLES: usize = 128;
        // The deterministic qualification Counter offers two `increment`
        // Actions before it switches to its `private_ack` offer. Keep this
        // metric tied to the real offered-action domain instead of inventing
        // a third payload merely to enlarge a percentile sample.
        const WARM_ACTION_COUNT: usize = 2;
        const STALE_ACTION_COUNT: usize = 4;
        let action_ids = ["01ARZ3NDEKTSV4RRFFQ69G5FJ5", "01ARZ3NDEKTSV4RRFFQ69G5FJ6"];
        let stale_action_ids = [
            "01ARZ3NDEKTSV4RRFFQ69G5FJ9",
            "01ARZ3NDEKTSV4RRFFQ69G5FJA",
            "01ARZ3NDEKTSV4RRFFQ69G5FJB",
            "01ARZ3NDEKTSV4RRFFQ69G5FJC",
        ];
        assert_eq!(action_ids.len(), WARM_ACTION_COUNT);
        assert_eq!(stale_action_ids.len(), STALE_ACTION_COUNT);

        // These are the fixture Counter's real accepted participant Actions
        // after corruption and after a cold Claim installed the trace. They
        // prove that the cache can keep serving and advancing the canonical
        // executor without re-reading the corrupted prefix.
        let mut action_latencies = Vec::with_capacity(WARM_ACTION_COUNT);
        let mut next_action_basis = fence_before.head().room_seq().get();
        for action_id in action_ids {
            let started = Instant::now();
            let accepted = match backend
                .action(
                    &member,
                    ActionSubmit {
                        room_id: ROOM_ID.to_owned(),
                        member_id: HUMAN_MEMBER.to_owned(),
                        action_id: action_id.to_owned(),
                        based_on_room_seq: next_action_basis,
                        action_type: "increment".to_owned(),
                        payload: json!({}),
                    },
                )
                .unwrap_or_else(|error| panic!("warm Action {action_id}: {error:?}"))
            {
                ActionReply::Accepted(reply) => reply,
                ActionReply::Rejected(reply) => {
                    panic!("warm Action {action_id} rejected: {reply:?}")
                }
            };
            assert!(!accepted.duplicate, "warm Action {action_id} duplicated");
            next_action_basis = next_action_basis
                .checked_add(1)
                .unwrap_or_else(|| panic!("warm Action basis overflow"));
            assert_eq!(accepted.room_head.room_seq, next_action_basis);
            action_latencies.push(started.elapsed());
        }
        let fence_after_actions = store
            .current_room_serving_fence(&room_id)
            .unwrap_or_else(|error| panic!("read post-Action serving fence: {error:?}"))
            .unwrap_or_else(|| panic!("post-Action serving fence absent"));
        assert_eq!(
            fence_after_actions.head().room_seq().get(),
            next_action_basis,
            "accepted Actions must advance the durable serving fence"
        );
        assert_eq!(fence_after_actions.integrity(), fence_before.integrity());
        let callbacks_after_actions = backend
            .trace_cache
            .with_room(&room_id, |slot| {
                slot.as_ref()
                    .map(|cached| cached.trace().activity_callback_count())
                    .unwrap_or_else(|| panic!("warm Action evicted cached trace"))
            })
            .unwrap_or_else(|_| panic!("inspect post-Action cached trace"));
        assert_eq!(
            callbacks_after_actions,
            callbacks_before + WARM_ACTION_COUNT,
            "each accepted Counter Action must execute one canonical reducer callback"
        );

        // This is the production current-read endpoint. It verifies the
        // serving fence before and after borrowing the cached executor, and
        // recovery is forbidden above, so every successful sample is a real
        // warm read rather than an unbounded historical reconstruction.
        let mut current_read_latencies =
            Vec::with_capacity(MAX_LATENCY_SAMPLES.min(warm_claim_count));
        for index in 0..warm_claim_count {
            let started = Instant::now();
            let response = backend
                .projection_response(&member, ROOM_ID)
                .unwrap_or_else(|error| panic!("warm current read {index}: {error:?}"));
            assert_eq!(
                response.room_head.room_seq,
                fence_after_actions.head().room_seq().get(),
                "warm current read {index} observed an unexpected serving head"
            );
            let elapsed = started.elapsed();
            if current_read_latencies.len() < MAX_LATENCY_SAMPLES {
                current_read_latencies.push(elapsed);
            } else {
                current_read_latencies[index % MAX_LATENCY_SAMPLES] = elapsed;
            }
        }
        let callbacks_after_current_reads = backend
            .trace_cache
            .with_room(&room_id, |slot| {
                slot.as_ref()
                    .map(|cached| cached.trace().activity_callback_count())
                    .unwrap_or_else(|| panic!("warm current read evicted cached trace"))
            })
            .unwrap_or_else(|_| panic!("inspect post-read cached trace"));
        assert_eq!(
            callbacks_after_current_reads, callbacks_after_actions,
            "warm current reads may render a view but must not run canonical reducers"
        );

        // Use fresh Action identities and an obsolete basis to measure the
        // real stale-admission path. A rejection must not change the cached
        // trace or the durable head.
        let mut stale_action_latencies = Vec::with_capacity(STALE_ACTION_COUNT);
        let mut stale_action_rejections = 0usize;
        let stale_basis = fence_before.head().room_seq().get();
        for action_id in stale_action_ids {
            let started = Instant::now();
            let rejected = match backend
                .action(
                    &member,
                    ActionSubmit {
                        room_id: ROOM_ID.to_owned(),
                        member_id: HUMAN_MEMBER.to_owned(),
                        action_id: action_id.to_owned(),
                        based_on_room_seq: stale_basis,
                        action_type: "increment".to_owned(),
                        payload: json!({}),
                    },
                )
                .unwrap_or_else(|error| panic!("stale Action {action_id}: {error:?}"))
            {
                ActionReply::Rejected(reply) => reply,
                ActionReply::Accepted(reply) => {
                    panic!("stale Action {action_id} accepted: {reply:?}")
                }
            };
            assert_eq!(rejected.code, "stale_room_state");
            assert_eq!(
                rejected.current_room_seq,
                fence_after_actions.head().room_seq().get()
            );
            assert!(!rejected.duplicate, "stale Action {action_id} duplicated");
            stale_action_rejections += 1;
            stale_action_latencies.push(started.elapsed());
        }
        assert_eq!(stale_action_rejections, STALE_ACTION_COUNT);

        let mut claim_latencies = Vec::with_capacity(MAX_LATENCY_SAMPLES.min(warm_claim_count));
        let mut rss_samples = Vec::with_capacity(MAX_RSS_SAMPLES);
        let rss_stride = (warm_claim_count / MAX_RSS_SAMPLES).max(1);
        let mut previous: Option<(String, u64)> = None;
        for index in 0..warm_claim_count {
            if let Some((claim_id, lease_generation)) = previous.take() {
                let release = backend
                    .activation_release(
                        &runner,
                        ActivationLeaseOperation {
                            activation_id: activation_id.clone(),
                            runner_id: runner_id.to_string(),
                            claim_id,
                            operation_id: format!("imo220-warm-release-{index}"),
                            lease_generation,
                            requested_lease_ms: None,
                            disposition: None,
                        },
                    )
                    .unwrap_or_else(|error| panic!("warm Activation release {index}: {error:?}"));
                assert_eq!(release.code, ActivationResultCode::Released);
            }
            let claim_id = format!("imo220-warm-claim-{index}");
            let started = Instant::now();
            let claim = backend
                .activation_claim(
                    &runner,
                    ActivationClaim {
                        activation_id: activation_id.clone(),
                        runner_id: runner_id.to_string(),
                        claim_id: claim_id.clone(),
                        requested_lease_ms: 30_000,
                    },
                )
                .unwrap_or_else(|error| panic!("warm Activation claim {index}: {error:?}"));
            assert_eq!(claim.code, ActivationResultCode::Granted);
            let elapsed = started.elapsed();
            if claim_latencies.len() < MAX_LATENCY_SAMPLES {
                claim_latencies.push(elapsed);
            } else {
                // Keep latency and RSS evidence bounded even for a caller
                // requesting a larger operator sample count.
                claim_latencies[index % MAX_LATENCY_SAMPLES] = elapsed;
            }
            if index % rss_stride == 0 {
                if let Some(rss) = warm_claim_rss_bytes() {
                    if rss_samples.len() < MAX_RSS_SAMPLES {
                        rss_samples.push(rss);
                    } else {
                        rss_samples[index % MAX_RSS_SAMPLES] = rss;
                    }
                }
            }
            previous = Some((
                claim_id,
                claim
                    .lease_generation
                    .unwrap_or_else(|| panic!("warm claim {index} omitted lease generation")),
            ));
        }
        if let Some((claim_id, lease_generation)) = previous {
            let release = backend
                .activation_release(
                    &runner,
                    ActivationLeaseOperation {
                        activation_id: activation_id.clone(),
                        runner_id: runner_id.to_string(),
                        claim_id,
                        operation_id: "imo220-warm-release-final".to_owned(),
                        lease_generation,
                        requested_lease_ms: None,
                        disposition: None,
                    },
                )
                .unwrap_or_else(|error| panic!("final warm Activation release: {error:?}"));
            assert_eq!(release.code, ActivationResultCode::Released);
        }

        let callbacks_after = backend
            .trace_cache
            .with_room(&room_id, |slot| {
                slot.as_ref()
                    .map(|cached| cached.trace().activity_callback_count())
                    .unwrap_or_else(|| panic!("warm cache was evicted"))
            })
            .unwrap_or_else(|_| panic!("inspect final cached trace"));
        assert_eq!(
            callbacks_after, callbacks_after_actions,
            "warm current reads, stale Actions, and warm claims must not run canonical reducers"
        );
        let fence_after = store
            .current_room_serving_fence(&room_id)
            .unwrap_or_else(|error| panic!("read final serving fence: {error:?}"))
            .unwrap_or_else(|| panic!("final serving fence absent"));
        assert_eq!(fence_after.head(), fence_after_actions.head());
        assert_eq!(fence_after.integrity(), fence_after_actions.integrity());
        let connection = rusqlite::Connection::open(&source)
            .unwrap_or_else(|error| panic!("open final qualifier inspection: {error}"));
        let final_transition_rows: u64 = connection
            .query_row(
                "SELECT count(*) FROM transitions WHERE room_id = ?1",
                [ROOM_ID],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| u64::try_from(count).unwrap_or_else(|_| panic!("negative row count")))
            .unwrap_or_else(|error| panic!("count final transitions: {error}"));
        let (final_state, final_lease_generation): (String, i64) = connection
            .query_row(
                "SELECT state, lease_generation FROM activation_intents WHERE activation_id = ?1",
                [&activation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read final Activation state: {error}"));
        let (pending_rows, oldest_pending_cause_room_seq, oldest_pending_created_at): (
            i64,
            Option<i64>,
            Option<String>,
        ) = connection
            .query_row(
                "SELECT count(*), min(cause_room_seq), min(created_at) \
                 FROM activation_intents \
                 WHERE room_id = ?1 AND target_member_id = ?2 AND state = 'pending'",
                [ROOM_ID, AGENT_MEMBER],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or_else(|error| panic!("read durable Activation queue: {error}"));
        drop(connection);
        let expected_final_transition_rows = canonical_transition_rows
            .checked_add(u64::try_from(WARM_ACTION_COUNT).unwrap_or(u64::MAX))
            .unwrap_or_else(|| panic!("expected canonical row count overflow"));
        assert_eq!(final_transition_rows, expected_final_transition_rows);
        assert_eq!(final_state, "pending");
        assert_eq!(
            pending_rows, 1,
            "one durable pending Activation must remain"
        );
        let oldest_pending_cause_room_seq = oldest_pending_cause_room_seq
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or_else(|| panic!("pending Activation omitted cause Room sequence"));
        let oldest_pending_created_at = oldest_pending_created_at
            .unwrap_or_else(|| panic!("pending Activation omitted creation time"));
        let oldest_pending_created_at = OffsetDateTime::parse(&oldest_pending_created_at, &Rfc3339)
            .unwrap_or_else(|error| panic!("parse pending Activation creation time: {error}"));
        let oldest_pending_age_ms = u64::try_from(
            (OffsetDateTime::now_utc() - oldest_pending_created_at)
                .whole_milliseconds()
                .max(0),
        )
        .unwrap_or(u64::MAX);
        let operator_queue = backend
            .operator_activation_status(&host, ROOM_ID, AGENT_MEMBER)
            .unwrap_or_else(|error| panic!("query host-authorized Activation queue: {error:?}"));
        assert_eq!(operator_queue.waiting, 1);
        assert_eq!(operator_queue.leased, 0);

        let action_latency_p50_us = warm_claim_percentile_us(&mut action_latencies, 50);
        let action_latency_p95_us = warm_claim_percentile_us(&mut action_latencies, 95);
        let action_latency_p99_us = warm_claim_percentile_us(&mut action_latencies, 99);
        let current_read_latency_p50_us = warm_claim_percentile_us(&mut current_read_latencies, 50);
        let current_read_latency_p95_us = warm_claim_percentile_us(&mut current_read_latencies, 95);
        let current_read_latency_p99_us = warm_claim_percentile_us(&mut current_read_latencies, 99);
        let stale_action_latency_p50_us = warm_claim_percentile_us(&mut stale_action_latencies, 50);
        let stale_action_latency_p95_us = warm_claim_percentile_us(&mut stale_action_latencies, 95);
        let stale_action_latency_p99_us = warm_claim_percentile_us(&mut stale_action_latencies, 99);
        let claim_latency_p50_us = warm_claim_percentile_us(&mut claim_latencies, 50);
        let claim_latency_p95_us = warm_claim_percentile_us(&mut claim_latencies, 95);
        let claim_latency_p99_us = warm_claim_percentile_us(&mut claim_latencies, 99);
        let rss = if rss_samples.is_empty() {
            serde_json::Value::Null
        } else {
            let mut sorted = rss_samples;
            sorted.sort_unstable();
            let at = |percentile: usize| {
                sorted[((sorted.len().saturating_sub(1) * percentile) / 100).min(sorted.len() - 1)]
            };
            json!({
                "sample_count": sorted.len(),
                "p50": at(50),
                "p95": at(95),
                "p99": at(99),
                "max": sorted.last(),
            })
        };
        let evidence = json!({
            "schema": "worldstream/imo-220-warm-claim-qualification/sqlite-v2",
            "backend": "sqlite",
            "source": {
                "fixture_transition_rows": source_transition_rows,
                "canonical_transition_rows_after_agent_join": canonical_transition_rows,
                "canonical_transition_rows_after_warm_actions": final_transition_rows,
                "expected_fixture_transition_rows": expected_history,
                "checkpoint_v2_contract": checkpoint_v2_contract,
            },
            "cold_claim": {
                "latency_us": cold_elapsed_us,
                "context_bytes": cold_context_bytes,
                "result": "granted",
            },
            "warm_actions": {
                "accepted_count": WARM_ACTION_COUNT,
                "bounded_latency_sample_count": action_latencies.len(),
                "latency_us": {
                    "p50": action_latency_p50_us,
                    "p95": action_latency_p95_us,
                    "p99": action_latency_p99_us,
                },
                "reducer_callbacks_before": callbacks_before,
                "reducer_callbacks_after": callbacks_after_actions,
                "canonical_transition_rows_before": canonical_transition_rows,
                "canonical_transition_rows_after": final_transition_rows,
            },
            "warm_current_reads": {
                "read_count": warm_claim_count,
                "bounded_latency_sample_count": current_read_latencies.len(),
                "latency_us": {
                    "p50": current_read_latency_p50_us,
                    "p95": current_read_latency_p95_us,
                    "p99": current_read_latency_p99_us,
                },
                "reducer_callbacks_before": callbacks_after_actions,
                "reducer_callbacks_after": callbacks_after_current_reads,
            },
            "warm_claims": {
                "claim_count": warm_claim_count,
                "bounded_latency_sample_count": claim_latencies.len(),
                "latency_us": {
                    "p50": claim_latency_p50_us,
                    "p95": claim_latency_p95_us,
                    "p99": claim_latency_p99_us,
                },
                "rss_bytes": rss,
                "reducer_callbacks_before": callbacks_after_actions,
                "reducer_callbacks_after": callbacks_after,
            },
            "stale_actions": {
                "submitted_count": STALE_ACTION_COUNT,
                "stale_rejection_count": stale_action_rejections,
                "stale_rate": stale_action_rejections as f64 / STALE_ACTION_COUNT as f64,
                "bounded_latency_sample_count": stale_action_latencies.len(),
                "latency_us": {
                    "p50": stale_action_latency_p50_us,
                    "p95": stale_action_latency_p95_us,
                    "p99": stale_action_latency_p99_us,
                },
            },
            "activation_queue": {
                "operator_waiting": operator_queue.waiting,
                "operator_leased": operator_queue.leased,
                "durable_pending_rows": u64::try_from(pending_rows).unwrap_or(0),
                "oldest_pending_cause_room_seq": oldest_pending_cause_room_seq,
                "oldest_pending_age_ms": oldest_pending_age_ms,
            },
            "fence": {
                "head_room_seq_before_warm_actions": fence_before.head().room_seq().get(),
                "head_room_seq": fence_after.head().room_seq().get(),
                "integrity_generation": fence_after.integrity().generation().get(),
                "head_stable_after_warm_actions": fence_after.head() == fence_after_actions.head(),
                "integrity_unchanged": fence_after.integrity() == fence_after_actions.integrity(),
            },
            "corruption_proof": {
                "in_head_transition_row_corrupted_after_cold_cache_install": true,
                "recovery_forbidden_after_corruption": true,
                "warm_actions_accepted_after_corruption": WARM_ACTION_COUNT,
                "warm_current_reads_served_after_corruption": warm_claim_count,
                "warm_claims_granted": warm_claim_count,
                "canonical_transition_rows_unchanged_after_warm_actions": final_transition_rows == expected_final_transition_rows,
            },
            "activation_final": {
                "state": final_state,
                "lease_generation": u64::try_from(final_lease_generation).unwrap_or(0),
            },
        });
        let evidence_text = serde_json::to_string(&evidence)
            .unwrap_or_else(|error| panic!("serialize qualification evidence: {error}"));
        if let Some(path) = env::var_os("WORLDSTREAM_WARM_CLAIM_EVIDENCE_FILE") {
            fs::write(path, format!("{evidence_text}\n"))
                .unwrap_or_else(|error| panic!("write qualification evidence: {error}"));
        }
        eprintln!("IMO220_SQLITE_WARM_CLAIM_EVIDENCE={evidence_text}");
    }

    struct CountingWallClock {
        calls: AtomicUsize,
    }

    impl HostClockV1 for CountingWallClock {
        fn sample(&self) -> Result<HostClockSampleV1, HostClockErrorV1> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            RuntimeWallClock.sample()
        }
    }

    #[test]
    fn live_backup_destination_is_derived_only_beneath_the_configured_root() {
        let (_database_directory, file) = database_fixture();
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let registry =
            Arc::new(builtin_counter_registry().unwrap_or_else(|_| panic!("counter registry")));
        let directory = tempdir().unwrap_or_else(|_| panic!("backup root"));
        let root = directory.path().join("backups");
        let backend = SqliteGatewayBackend::new(store, registry)
            .with_live_backup_root(&root)
            .unwrap_or_else(|_| panic!("secure backup root"));

        let destination = backend
            .validated_live_backup_destination("stable-backup")
            .unwrap_or_else(|_| panic!("derived destination"));
        assert_eq!(
            destination,
            std::fs::canonicalize(&root)
                .unwrap_or_else(|_| panic!("canonical root"))
                .join("stable-backup/backup.sqlite3")
        );
        assert!(
            backend
                .validated_live_backup_destination("../escape")
                .is_err()
        );
    }

    #[test]
    fn sqlite_gateway_retains_the_injected_startup_registry() {
        let (_database_directory, file) = database_fixture();
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let registry =
            Arc::new(builtin_counter_registry().unwrap_or_else(|_| panic!("counter registry")));
        let backend = SqliteGatewayBackend::new(store, Arc::clone(&registry));

        assert!(Arc::ptr_eq(&registry, &backend.registry));
    }

    #[test]
    fn genesis_creation_time_is_canonical_at_pack_safe_precision() {
        let value = creation_time().unwrap_or_else(|_| panic!("Genesis creation clock"));
        assert_eq!(value.as_str().len(), 20);
        assert!(value.as_str().ends_with('Z'));
        assert!(!value.as_str().contains('.'));
    }

    #[test]
    fn sqlite_host_operator_reads_only_exact_activity_pack_catalog_revisions() {
        let (_database_directory, file) = database_fixture();
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let bearer = CapabilityBearerV1::from_bytes([0xb7; 32]);
        let bootstrap = AuthorityBootstrapV1::new(
            "01ARZ3NDEKTSV4RRFFQ69G5FB1"
                .parse()
                .unwrap_or_else(|_| panic!("change")),
            "01ARZ3NDEKTSV4RRFFQ69G5FB2"
                .parse()
                .unwrap_or_else(|_| panic!("principal")),
            PrincipalKindV1::Human,
            "01ARZ3NDEKTSV4RRFFQ69G5FB3"
                .parse()
                .unwrap_or_else(|_| panic!("capability")),
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
        let backend = SqliteGatewayBackend::new(store, registry);
        let gateway_session = session(0xb7, "01ARZ3NDEKTSV4RRFFQ69G5FB4");

        let catalog = backend
            .activity_pack_catalog(&gateway_session)
            .unwrap_or_else(|error| panic!("catalog: {error:?}"));
        assert_eq!(catalog.revisions.len(), 4);
        assert!(catalog.revisions.iter().any(|revision| {
            revision.pack.digest == counter_v2_digest().to_string()
                && revision.selectable_for_new_rooms
                && revision.runnable_for_retained_rooms
        }));
        assert!(catalog.revisions.iter().any(|revision| {
            revision.pack.digest == counter_v3_digest().to_string()
                && !revision.selectable_for_new_rooms
                && revision.runnable_for_retained_rooms
        }));
        assert!(catalog.revisions.iter().any(|revision| {
            revision.pack.digest == counter_v4_digest().to_string()
                && revision.selectable_for_new_rooms
                && revision.runnable_for_retained_rooms
        }));
        assert!(matches!(
            backend.activity_pack_revision(
                &gateway_session,
                "blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            ),
            Err(BackendError::ActivityPackRevisionUnavailable)
        ));
    }

    #[test]
    fn sqlite_runtime_wires_the_frozen_action_and_host_lane() {
        let source = include_str!("sqlite_backend.rs");
        assert!(source.contains(".reserve_action(&action_room_id, self.host_clock.as_ref())"));
        assert!(source.contains(".reserve_host_stimulus(&room_id)"));
        let orphaned_wall_sample = ["ActionAdmittedAt::from_str", "(&now_text()?)"].concat();
        assert!(!source.contains(&orphaned_wall_sample));
        assert!(matches!(
            map_admission_lane_error(&AdmissionLaneErrorV1::Full),
            BackendError::Busy
        ));
        let lanes = RoomAdmissionLanesV1::default();
        assert_eq!(lanes.capacity(), 256);
        assert!(lanes.host_reserve() > 0);
    }

    #[allow(clippy::too_many_lines)]
    #[test]
    fn sqlite_gateway_create_is_authorized_idempotent_and_conflict_safe() {
        let (_database_directory, file) = database_fixture();
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
        let status = backend
            .operator_activation_status(&gateway_session, &first.room_id, &first.member_ids[0])
            .unwrap_or_else(|error| panic!("activation status: {error:?}"));
        assert_eq!(status.waiting, 0);
        assert_eq!(status.leased, 0);
        assert!(status.observed_at_unix_ms > 0);
        assert!(matches!(
            backend.operator_activation_status(
                &gateway_session,
                &first.room_id,
                "01ARZ3NDEKTSV4RRFFQ69G5FQ9"
            ),
            Err(BackendError::NotFound)
        ));
        assert!(matches!(
            backend.operator_activation_status(
                &session(0xab, "01ARZ3NDEKTSV4RRFFQ69G5FC6"),
                &first.room_id,
                &first.member_ids[0]
            ),
            Err(BackendError::Forbidden)
        ));

        let authenticated = backend.authenticate(&gateway_session).unwrap();
        let (legacy, _) = creation_request(&authenticated, &request).unwrap();
        assert_eq!(
            legacy.canonical_request_hash().unwrap(),
            worldstream_core::RoomCreationRequestWithFormat::new(
                legacy.clone(),
                worldstream_core::CanonicalHistoryFormat::V1
            )
            .canonical_request_hash()
            .unwrap()
        );
        let duplicate = backend
            .create_room_with_format(
                &gateway_session,
                worldstream_protocol::CreateRoomRequestWithFormat {
                    request: request.clone(),
                    canonical_history_format: worldstream_protocol::CanonicalHistoryFormatV1::V1,
                },
            )
            .unwrap_or_else(|error| panic!("duplicate create: {error:?}"));
        assert_eq!(duplicate, first);
        assert_eq!(
            serde_json::to_vec(&duplicate).unwrap(),
            serde_json::to_vec(&first).unwrap()
        );
        assert!(matches!(
            backend.create_room_with_format(
                &gateway_session,
                worldstream_protocol::CreateRoomRequestWithFormat {
                    request: request.clone(),
                    canonical_history_format: worldstream_protocol::CanonicalHistoryFormatV1::V2,
                }
            ),
            Err(BackendError::Conflict)
        ));
        let mut compact_request = request.clone();
        compact_request.idempotency_key = "gateway-create-compact-public".to_owned();
        let compact_selection = worldstream_protocol::CreateRoomRequestWithFormat {
            request: compact_request.clone(),
            canonical_history_format: worldstream_protocol::CanonicalHistoryFormatV1::V2,
        };
        let compact = backend
            .create_room_with_format(&gateway_session, compact_selection.clone())
            .unwrap();
        let compact_retry = backend
            .create_room_with_format(&gateway_session, compact_selection)
            .unwrap();
        assert_eq!(compact, compact_retry);
        assert!(matches!(
            backend.create_room(&gateway_session, compact_request.clone()),
            Err(BackendError::Conflict)
        ));
        let capability = backend
            .issue_member_capability(
                &gateway_session,
                MemberCapabilityIssueRequest {
                    room_id: compact.room_id.clone(),
                    member_id: compact.member_ids[0].clone(),
                    principal_id: principal.to_string(),
                    scopes: vec![
                        CapabilityScopeV1::RoomAttach,
                        CapabilityScopeV1::RoomObserveMember,
                        CapabilityScopeV1::RoomAct,
                    ],
                    idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FR7".to_owned(),
                    expires_at: None,
                },
            )
            .unwrap();
        let wire = BearerWireV1::parse(&capability.bearer).unwrap();
        let member_session = GatewaySession::new_with_wire(
            "01ARZ3NDEKTSV4RRFFQ69G5FR6".parse().unwrap(),
            CapabilityBearerV1::from_bytes(
                BearerWireV1::parse(&capability.bearer)
                    .unwrap()
                    .into_bytes(),
            ),
            wire,
        );
        let attach = backend
            .attach(
                &member_session,
                RoomAttach {
                    room_id: compact.room_id.clone(),
                    member_id: compact.member_ids[0].clone(),
                    after_frame_seq: None,
                },
            )
            .unwrap();
        backend
            .sync_ack(
                &member_session,
                RoomSyncAck {
                    room_id: compact.room_id.clone(),
                    member_id: compact.member_ids[0].clone(),
                    through_frame_head: attach.attached.frame_head,
                    sync_token: attach.attached.sync_token,
                },
            )
            .unwrap();
        backend.forbid_recovery_for_test();
        let action = ActionSubmit {
            room_id: compact.room_id.clone(),
            member_id: compact.member_ids[0].clone(),
            action_id: "01ARZ3NDEKTSV4RRFFQ69G5FR8".to_owned(),
            based_on_room_seq: 0,
            action_type: "increment".to_owned(),
            payload: json!({}),
        };
        assert!(matches!(
            backend.action(&member_session, action.clone()).unwrap(),
            ActionReply::Accepted(_)
        ));
        assert!(
            matches!(backend.action(&member_session, action).unwrap(), ActionReply::Accepted(value) if value.duplicate)
        );
        let overflow = ActionSubmit {
            room_id: compact.room_id.clone(),
            member_id: compact.member_ids[0].clone(),
            action_id: "01ARZ3NDEKTSV4RRFFQ69G5FR5".to_owned(),
            based_on_room_seq: 1,
            action_type: "increment".to_owned(),
            payload: json!({"oversize": "x".repeat(40_000)}),
        };
        assert!(matches!(
            backend.action(&member_session, overflow.clone()),
            Err(BackendError::Rejected)
        ));
        let overflow = participant_action_request(&overflow).unwrap();
        assert!(matches!(
            RoomCommitStorageV1::resolve(
                &backend.store,
                &overflow.operation_identity(),
                &overflow.canonical_request_hash().unwrap()
            ),
            worldstream_core::ResolveOutcomeV1::KnownAbsent
        ));
        assert_eq!(
            backend
                .store
                .room_integrity_state(&compact.room_id.parse().unwrap())
                .unwrap()
                .unwrap()
                .status(),
            worldstream_core::RoomIntegrityStatusV1::Healthy
        );
        let recipient = crate::LiveObservationRecipient {
            session: Arc::new(member_session),
            room_id: compact.room_id.clone(),
            member_id: compact.member_ids[0].clone(),
            after_frame_seq: 0,
        };
        let pages = backend.prepare_live_observation_batch(
            std::slice::from_ref(&recipient),
            32,
            1024 * 1024,
        );
        let page = pages[0].as_ref().unwrap();
        assert_eq!(page.frames.len(), 1);
        page.fence
            .as_ref()
            .unwrap()
            .revalidate(&recipient.session)
            .unwrap();
        let connection = rusqlite::Connection::open(file.path()).unwrap();
        let genesis: Vec<u8> = connection
            .query_row(
                "SELECT genesis_bytes FROM room_genesis WHERE room_id=?1",
                [&compact.room_id],
                |row| row.get(0),
            )
            .unwrap();
        let transition: Vec<u8> = connection
            .query_row(
                "SELECT transition_bytes FROM transitions WHERE room_id=?1 AND room_seq=1",
                [&compact.room_id],
                |row| row.get(0),
            )
            .unwrap();
        let genesis: serde_json::Value = serde_json::from_slice(&genesis).unwrap();
        let transition: serde_json::Value = serde_json::from_slice(&transition).unwrap();
        assert_eq!(genesis["transition_version"], "worldstream/transition/v2");
        assert_eq!(
            transition["transition_version"],
            "worldstream/transition/v2"
        );
        assert!(transition.get("resulting_core_state").is_none());
        assert!(transition.get("resulting_activity_state").is_none());
        let mut conflict = request;
        conflict.configuration = json!({"initial_value": 1, "maximum_value": 16});
        assert!(matches!(
            backend.create_room(&gateway_session, conflict),
            Err(BackendError::Conflict)
        ));
    }

    #[allow(clippy::too_many_lines)]
    #[test]
    fn sqlite_sealed_provisioning_replays_exactly_across_partial_commit_and_restart() {
        let (_database_directory, file) = database_fixture();
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let host_bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        authority
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FC4"
                        .parse()
                        .unwrap_or_else(|_| panic!("bootstrap change")),
                    "01ARZ3NDEKTSV4RRFFQ69G5FC2"
                        .parse()
                        .unwrap_or_else(|_| panic!("host principal")),
                    PrincipalKindV1::Human,
                    "01ARZ3NDEKTSV4RRFFQ69G5FC3"
                        .parse()
                        .unwrap_or_else(|_| panic!("host capability")),
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|_| panic!("bootstrap request")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|_| panic!("bootstrap time")),
            )
            .unwrap_or_else(|_| panic!("bootstrap authority"));

        let registry =
            Arc::new(builtin_counter_registry().unwrap_or_else(|_| panic!("counter registry")));
        let descriptor = registry
            .load_retained(&counter_v2_digest())
            .unwrap_or_else(|_| panic!("counter pack"))
            .descriptor()
            .clone();
        let host = session(0xa9, "01ARZ3NDEKTSV4RRFFQ69G5FC5");
        let backend = SqliteGatewayBackend::new(store, Arc::clone(&registry));
        let created = backend
            .create_room(
                &host,
                CreateRoomRequest {
                    pack: PackReference {
                        id: descriptor.pack_id,
                        version: descriptor.explanatory_version,
                        digest: counter_v2_digest().to_string(),
                    },
                    configuration: json!({"initial_value": 0, "maximum_value": 16}),
                    members: vec![CreateMember {
                        principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC6".to_owned(),
                        principal_kind: PrincipalKind::Agent,
                        role: Some("counter".to_owned()),
                        access_mode: AccessMode::Participant,
                    }],
                    idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FD0".to_owned(),
                },
            )
            .unwrap_or_else(|error| panic!("create Room: {error:?}"));
        let room_id = created.room_id;
        let member_id = created.member_ids[0].clone();

        let member_first = backend
            .provision_member_capability(&host, sealed_member_request(&room_id, &member_id, 0xc1))
            .unwrap_or_else(|error| panic!("sealed member: {error:?}"));
        assert_eq!(
            backend
                .provision_member_capability(
                    &host,
                    sealed_member_request(&room_id, &member_id, 0xc1),
                )
                .unwrap_or_else(|error| panic!("same-process member replay: {error:?}")),
            member_first
        );
        drop(backend);

        let restarted = SqliteGatewayBackend::new(
            SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("restart db")),
            Arc::clone(&registry),
        );
        assert_eq!(
            restarted
                .provision_member_capability(
                    &host,
                    sealed_member_request(&room_id, &member_id, 0xc1),
                )
                .unwrap_or_else(|error| panic!("restart member replay: {error:?}")),
            member_first
        );
        assert!(matches!(
            restarted.provision_member_capability(
                &host,
                sealed_member_request(&room_id, &member_id, 0xc2),
            ),
            Err(BackendError::Conflict)
        ));

        // Model a crash after the Runner subcommit but before Capability
        // registration. The production retry must replay that exact change and
        // finish only the missing Capability subcommit.
        let presented = restarted
            .authenticate(&host)
            .unwrap_or_else(|error| panic!("authenticate host: {error:?}"))
            .into_presented();
        restarted
            .authority()
            .change(
                &presented,
                AuthorityChangeV1::RegisterRunner {
                    change_id: "01ARZ3NDEKTSV4RRFFQ69G5FF3"
                        .parse()
                        .unwrap_or_else(|_| panic!("runner change")),
                    runner_id: "01ARZ3NDEKTSV4RRFFQ69G5FF2"
                        .parse()
                        .unwrap_or_else(|_| panic!("runner id")),
                    owner_principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC6"
                        .parse()
                        .unwrap_or_else(|_| panic!("agent principal")),
                },
                SqliteGatewayBackend::checked_at()
                    .unwrap_or_else(|_| panic!("authority checked time")),
            )
            .unwrap_or_else(|error| panic!("precommit Runner subcommit: {error:?}"));
        drop(restarted);

        let resumed = SqliteGatewayBackend::new(
            SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("resume db")),
            Arc::clone(&registry),
        );
        let runner_first = resumed
            .provision_runner_capability(&host, sealed_runner_request(&room_id, &member_id, 0xd1))
            .unwrap_or_else(|error| panic!("resume partial Runner: {error:?}"));
        assert_eq!(
            resumed
                .provision_runner_capability(
                    &host,
                    sealed_runner_request(&room_id, &member_id, 0xd1),
                )
                .unwrap_or_else(|error| panic!("same-process Runner replay: {error:?}")),
            runner_first
        );
        assert!(matches!(
            resumed.provision_runner_capability(
                &host,
                sealed_runner_request(&room_id, &member_id, 0xd2),
            ),
            Err(BackendError::Conflict)
        ));
        let mut mismatched_principal = sealed_runner_request(&room_id, &member_id, 0xd1);
        mismatched_principal.owner_principal_id = "01ARZ3NDEKTSV4RRFFQ69G5FC7".to_owned();
        assert!(matches!(
            resumed.provision_runner_capability(&host, mismatched_principal),
            Err(BackendError::Forbidden)
        ));
        let mut mismatched_principal_change = sealed_runner_request(&room_id, &member_id, 0xd1);
        mismatched_principal_change.principal_idempotency_key =
            "01ARZ3NDEKTSV4RRFFQ69G5FE9".to_owned();
        assert!(matches!(
            resumed.provision_runner_capability(&host, mismatched_principal_change),
            Err(BackendError::Conflict)
        ));
        drop(resumed);

        let final_restart = SqliteGatewayBackend::new(
            SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("final restart db")),
            registry,
        );
        assert_eq!(
            final_restart
                .provision_runner_capability(
                    &host,
                    sealed_runner_request(&room_id, &member_id, 0xd1),
                )
                .unwrap_or_else(|error| panic!("restart Runner replay: {error:?}")),
            runner_first
        );
        drop(final_restart);

        let connection = rusqlite::Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect SQLite: {error}"));
        let count = |sql: &str| {
            connection
                .query_row(sql, [], |row| row.get::<_, i64>(0))
                .unwrap_or_else(|error| panic!("inspect authority rows: {error}"))
        };
        assert_eq!(
            count(
                "SELECT count(*) FROM principals WHERE principal_id = '01ARZ3NDEKTSV4RRFFQ69G5FC6'"
            ),
            1
        );
        assert_eq!(
            count("SELECT count(*) FROM runners WHERE runner_id = '01ARZ3NDEKTSV4RRFFQ69G5FF2'"),
            1
        );
        assert_eq!(
            count(
                "SELECT count(*) FROM capabilities WHERE capability_id IN ('01ARZ3NDEKTSV4RRFFQ69G5FF0', '01ARZ3NDEKTSV4RRFFQ69G5FF4')"
            ),
            2
        );
        assert_eq!(
            count(
                "SELECT count(*) FROM authority_change_receipts WHERE change_id IN ('01ARZ3NDEKTSV4RRFFQ69G5FC6', '01ARZ3NDEKTSV4RRFFQ69G5FF1', '01ARZ3NDEKTSV4RRFFQ69G5FF3', '01ARZ3NDEKTSV4RRFFQ69G5FF5')"
            ),
            4
        );
        assert_eq!(
            count(
                "SELECT count(*) FROM authority_audit WHERE change_id IN ('01ARZ3NDEKTSV4RRFFQ69G5FC6', '01ARZ3NDEKTSV4RRFFQ69G5FF1', '01ARZ3NDEKTSV4RRFFQ69G5FF3', '01ARZ3NDEKTSV4RRFFQ69G5FF5')"
            ),
            4
        );
        assert_eq!(
            count(
                "SELECT count(*) FROM capability_scopes WHERE capability_id IN ('01ARZ3NDEKTSV4RRFFQ69G5FF0', '01ARZ3NDEKTSV4RRFFQ69G5FF4')"
            ),
            6
        );
        assert_eq!(
            count(
                "SELECT count(*) FROM runner_capability_memberships WHERE capability_id = '01ARZ3NDEKTSV4RRFFQ69G5FF4'"
            ),
            1
        );
    }

    #[allow(clippy::too_many_lines)]
    #[test]
    fn sqlite_lobby_archive_and_launch_are_authorized_idempotent_and_phase_safe() {
        let (_database_directory, file) = database_fixture();
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let bearer = CapabilityBearerV1::from_bytes([0xaa; 32]);
        let principal = "01ARZ3NDEKTSV4RRFFQ69G5FD0"
            .parse::<worldstream_core::PrincipalId>()
            .unwrap_or_else(|_| panic!("principal"));
        authority
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FD1"
                        .parse()
                        .unwrap_or_else(|_| panic!("change")),
                    principal.clone(),
                    PrincipalKindV1::Human,
                    "01ARZ3NDEKTSV4RRFFQ69G5FD2"
                        .parse()
                        .unwrap_or_else(|_| panic!("capability")),
                    bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|_| panic!("bootstrap")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|_| panic!("checked at")),
            )
            .unwrap_or_else(|_| panic!("apply bootstrap"));
        let registry =
            Arc::new(builtin_agent_heist_registry().unwrap_or_else(|_| panic!("Heist registry")));
        let descriptor = registry
            .load_retained(&agent_heist_schema_safe_digest())
            .unwrap_or_else(|_| panic!("Lobby revision"))
            .descriptor()
            .clone();
        let members: Vec<CreateMember> = [
            (principal.to_string(), "navigator"),
            ("01ARZ3NDEKTSV4RRFFQ69G5FD3".to_owned(), "insider"),
            ("01ARZ3NDEKTSV4RRFFQ69G5FD4".to_owned(), "broker"),
        ]
        .into_iter()
        .map(|(principal_id, role)| CreateMember {
            principal_id,
            principal_kind: PrincipalKind::Human,
            role: Some(role.to_owned()),
            access_mode: AccessMode::Participant,
        })
        .collect();
        let backend = SqliteGatewayBackend::new(store, registry);
        let host = session(0xaa, "01ARZ3NDEKTSV4RRFFQ69G5FD5");
        let create_lobby = |idempotency_key: &str| {
            backend.create_room(
                &host,
                CreateRoomRequest {
                    pack: PackReference {
                        id: descriptor.pack_id.clone(),
                        version: descriptor.explanatory_version.clone(),
                        digest: agent_heist_schema_safe_digest().to_string(),
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
                    members: members.clone(),
                    idempotency_key: idempotency_key.to_owned(),
                },
            )
        };
        let room_to_archive = create_lobby("lobby-room-to-archive")
            .unwrap_or_else(|error| panic!("create Lobby to archive: {error:?}"));
        let archive_request = RoomArchiveRequestV1 {
            schema: worldstream_protocol::ROOM_ARCHIVE_REQUEST_SCHEMA_V1.to_owned(),
            idempotency_key: "archive-lobby-room".to_owned(),
        };
        let archived = backend
            .archive_room(&host, &room_to_archive.room_id, archive_request.clone())
            .unwrap_or_else(|error| panic!("archive Lobby: {error:?}"));
        assert_eq!(archived.room_head.room_seq, 1);
        assert!(!archived.duplicate);
        let duplicate_archive = backend
            .archive_room(&host, &room_to_archive.room_id, archive_request)
            .unwrap_or_else(|error| panic!("duplicate archive Lobby: {error:?}"));
        assert_eq!(duplicate_archive.room_head, archived.room_head);
        assert!(duplicate_archive.duplicate);
        let inspection = rusqlite::Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect archived Lobby: {error}"));
        let archived_rows: (String, i64, i64) = inspection
            .query_row(
                "SELECT r.room_status, \
                 (SELECT count(*) FROM transitions WHERE room_id = r.room_id), \
                 (SELECT count(*) FROM observation_consequences \
                  WHERE room_id = r.room_id AND consequence_kind = 'reset_required') \
                 FROM rooms AS r WHERE r.room_id = ?1",
                [&room_to_archive.room_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or_else(|error| panic!("inspect archived Lobby rows: {error}"));
        assert_eq!(archived_rows, ("archived".to_owned(), 1, 3));

        let room =
            create_lobby("lobby-room").unwrap_or_else(|error| panic!("create Lobby: {error:?}"));
        let request = LobbyLaunchRequest {
            input_id: "01ARZ3NDEKTSV4RRFFQ69G5FD6".to_owned(),
            based_on_room_seq: 0,
            pack_digest: None,
        };
        backend
            .ensure_verified_active(
                &room
                    .room_id
                    .parse()
                    .unwrap_or_else(|_| panic!("created Room ID")),
            )
            .unwrap_or_else(|error| panic!("activate Lobby: {error:?}"));
        assert_eq!(
            backend
                .resolve_lobby_launch(&host, &room.room_id, request.clone())
                .unwrap_or_else(|error| panic!("resolve absent Lobby: {error:?}")),
            None
        );
        let first = backend
            .launch_lobby(&host, &room.room_id, request.clone())
            .unwrap_or_else(|error| panic!("launch Lobby: {error:?}"));
        assert_eq!(first.room_head.room_seq, 1);
        assert!(!first.duplicate);
        let resolved = backend
            .resolve_lobby_launch(&host, &room.room_id, request.clone())
            .unwrap_or_else(|error| panic!("resolve committed Lobby: {error:?}"))
            .unwrap_or_else(|| panic!("committed Lobby receipt absent"));
        assert!(resolved.duplicate);
        assert_eq!(resolved.transition_id, first.transition_id);
        assert_eq!(resolved.room_head, first.room_head);
        let exact_request = LobbyLaunchRequest {
            pack_digest: Some(agent_heist_schema_safe_digest().to_string()),
            ..request.clone()
        };
        let exact_resolved = backend
            .resolve_lobby_launch(&host, &room.room_id, exact_request.clone())
            .unwrap_or_else(|error| panic!("resolve exact-Pack Lobby: {error:?}"))
            .unwrap_or_else(|| panic!("exact-Pack Lobby receipt absent"));
        assert!(exact_resolved.duplicate);
        assert_eq!(exact_resolved.transition_id, first.transition_id);
        let duplicate = backend
            .launch_lobby(&host, &room.room_id, request.clone())
            .unwrap_or_else(|error| panic!("duplicate launch: {error:?}"));
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.transition_id, first.transition_id);
        assert!(matches!(
            backend.launch_lobby(
                &host,
                &room.room_id,
                LobbyLaunchRequest {
                    based_on_room_seq: 1,
                    ..request.clone()
                },
            ),
            Err(BackendError::Conflict)
        ));
        assert!(matches!(
            backend.launch_lobby(
                &host,
                &room.room_id,
                LobbyLaunchRequest {
                    input_id: "01ARZ3NDEKTSV4RRFFQ69G5FD7".to_owned(),
                    based_on_room_seq: 1,
                    pack_digest: None,
                },
            ),
            Err(BackendError::WrongPhase)
        ));
        assert!(matches!(
            backend.launch_lobby(
                &session(0xab, "01ARZ3NDEKTSV4RRFFQ69G5FD8"),
                &room.room_id,
                request.clone(),
            ),
            Err(BackendError::Forbidden)
        ));
        assert!(matches!(
            backend.launch_lobby(
                &host,
                "01ARZ3NDEKTSV4RRFFQ69G5FZ0",
                LobbyLaunchRequest {
                    input_id: "01ARZ3NDEKTSV4RRFFQ69G5FD9".to_owned(),
                    based_on_room_seq: 0,
                    pack_digest: None,
                },
            ),
            Err(BackendError::NotFound)
        ));

        // A different retained Heist digest declares the same fixed input, so
        // Core's external-input hash is intentionally identical. The receipt's
        // immutable Pack head is the final exactness check, without a live
        // Room read or a second mutation.
        let same_identity_wrong_digest = LobbyLaunchRequest {
            pack_digest: Some(agent_heist_lobby_digest().to_string()),
            ..request.clone()
        };
        assert!(matches!(
            backend.resolve_lobby_launch(&host, &room.room_id, same_identity_wrong_digest.clone(),),
            Err(BackendError::Conflict)
        ));
        assert!(matches!(
            backend.launch_lobby(&host, &room.room_id, same_identity_wrong_digest),
            Err(BackendError::Conflict)
        ));
        let head_before_wrong_digest = backend
            .operator_room_detail(&host, &room.room_id)
            .unwrap_or_else(|error| panic!("Room before wrong digest: {error}"))
            .room_head;
        let absent_wrong_digest = LobbyLaunchRequest {
            input_id: "01ARZ3NDEKTSV4RRFFQ69G5FDA".to_owned(),
            based_on_room_seq: 1,
            pack_digest: Some(agent_heist_lobby_digest().to_string()),
        };
        assert!(matches!(
            backend.launch_lobby(&host, &room.room_id, absent_wrong_digest),
            Err(BackendError::Conflict)
        ));
        assert_eq!(
            backend
                .operator_room_detail(&host, &room.room_id)
                .unwrap_or_else(|error| panic!("Room after wrong digest: {error}"))
                .room_head,
            head_before_wrong_digest,
        );

        let connection = rusqlite::Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open Room integrity fixture: {error}"));
        assert_eq!(
            connection
                .execute(
                    "UPDATE room_integrity SET status = 'faulted', generation = generation + 1 WHERE room_id = ?1",
                    [&room.room_id],
                )
                .unwrap_or_else(|error| panic!("fault Room integrity: {error}")),
            1,
        );
        let parsed_room_id = room
            .room_id
            .parse::<RoomId>()
            .unwrap_or_else(|_| panic!("created Room ID"));
        assert!(matches!(
            backend.ensure_verified_active(&parsed_room_id),
            Err(BackendError::RoomFaulted)
        ));
        let recovered = backend
            .resolve_lobby_launch(&host, &room.room_id, exact_request)
            .unwrap_or_else(|error| panic!("resolve receipt through Room fault: {error}"))
            .unwrap_or_else(|| panic!("committed receipt absent through Room fault"));
        assert!(recovered.duplicate);
        assert_eq!(recovered.transition_id, first.transition_id);
        let replayed = backend
            .launch_lobby(&host, &room.room_id, request.clone())
            .unwrap_or_else(|error| panic!("launch replay through Room fault: {error}"));
        assert!(replayed.duplicate);
        assert_eq!(replayed.transition_id, first.transition_id);

        let assert_unreserved = |room: &str, input_id: &str, based_on_room_seq: u64| {
            let room_id = room
                .parse::<RoomId>()
                .unwrap_or_else(|_| panic!("test Room ID"));
            let proposed = "2026-08-24T12:34:56Z"
                .parse::<ExternalInputRecordedAt>()
                .unwrap_or_else(|_| panic!("test recorded-at"));
            let input = ExternalInputV1 {
                source_id: SourceId::from_str(worldstream_core::HOST_LOBBY_LAUNCH_SOURCE)
                    .unwrap_or_else(|_| panic!("host source")),
                input_id: input_id.parse().unwrap_or_else(|_| panic!("test Input ID")),
                input_type: worldstream_core::HOST_LAUNCH_INPUT_TYPE.to_owned(),
                recorded_at: proposed.clone(),
                canonical_payload: CanonicalJsonV1::parse(br"{}")
                    .unwrap_or_else(|_| panic!("launch payload")),
                immutable_resource_references: Vec::new(),
            };
            let identity = worldstream_core::OperationIdentityV1::ExternalInput(Box::new(
                worldstream_core::ExternalInputOperationIdentityV1 {
                    room_id: room_id.clone(),
                    source_id: input.source_id.clone(),
                    input_id: input.input_id.clone(),
                },
            ));
            let request_hash = external_input_request_hash(
                &room_id,
                RoomSequenceV1::new(based_on_room_seq)
                    .unwrap_or_else(|_| panic!("test Room sequence")),
                &input,
            )
            .unwrap_or_else(|_| panic!("launch request hash"));
            assert_eq!(
                backend.store.reserve_external_input_recorded_at(
                    &identity,
                    &request_hash,
                    &proposed,
                ),
                Ok(proposed),
                "invalid fresh launches must not reserve semantic time",
            );
        };
        assert_unreserved(&room.room_id, "01ARZ3NDEKTSV4RRFFQ69G5FD7", 1);
        assert_unreserved(
            "01ARZ3NDEKTSV4RRFFQ69G5FZ0",
            "01ARZ3NDEKTSV4RRFFQ69G5FD9",
            0,
        );
    }

    #[test]
    #[ignore = "requires WORLDSTREAM_ARCHIVE_CONTRACT_BUNDLE from the TypeScript fixture build"]
    fn sqlite_archive_activity_start_is_real_metadata_derived_and_idempotent() {
        if env::var_os("WORLDSTREAM_ARCHIVE_CONTRACT_BUNDLE").is_none() {
            return;
        }
        let (_database_directory, file) = database_fixture();
        let store = SqliteRoomStore::open(file.path()).unwrap_or_else(|_| panic!("open db"));
        let host_bearer = CapabilityBearerV1::from_bytes([0xc1; 32]);
        let host_principal = "01ARZ3NDEKTSV4RRFFQ69G5G01"
            .parse::<worldstream_core::PrincipalId>()
            .unwrap_or_else(|_| panic!("host principal"));
        AuthorityV1::new(Arc::new(store.clone()))
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5G02"
                        .parse()
                        .unwrap_or_else(|_| panic!("bootstrap change")),
                    host_principal,
                    PrincipalKindV1::Human,
                    "01ARZ3NDEKTSV4RRFFQ69G5G03"
                        .parse()
                        .unwrap_or_else(|_| panic!("host capability")),
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|_| panic!("bootstrap request")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|_| panic!("bootstrap time")),
            )
            .unwrap_or_else(|error| panic!("bootstrap authority: {error}"));

        let (registry, pack, corpus) = archive_registry(true, true);
        let descriptor = registry
            .load_retained(
                &pack
                    .digest
                    .parse()
                    .unwrap_or_else(|_| panic!("pack digest")),
            )
            .unwrap_or_else(|error| panic!("archive retained pack: {error}"))
            .descriptor()
            .clone();
        let host = session(0xc1, "01ARZ3NDEKTSV4RRFFQ69G5G04");
        let request = CreateRoomRequest {
            pack: pack.clone(),
            configuration: serde_json::to_value(&corpus.genesis.configuration)
                .unwrap_or_else(|_| panic!("archive configuration")),
            members: [
                ("01ARZ3NDEKTSV4RRFFQ69G5FC0", "lead"),
                ("01ARZ3NDEKTSV4RRFFQ69G5FC1", "mira"),
                ("01ARZ3NDEKTSV4RRFFQ69G5FC2", "jonah"),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (principal_id, role))| CreateMember {
                principal_id: principal_id.to_owned(),
                principal_kind: if index == 0 {
                    PrincipalKind::Human
                } else {
                    PrincipalKind::Agent
                },
                role: Some(role.to_owned()),
                access_mode: AccessMode::Participant,
            })
            .collect(),
            idempotency_key: "archive-activity-start-room".to_owned(),
        };
        let creating_backend = SqliteGatewayBackend::new(store.clone(), registry.clone());
        let room = creating_backend
            .create_room(&host, request.clone())
            .unwrap_or_else(|error| panic!("create archive Room: {error}"));
        let mut revoked_request = request;
        revoked_request.idempotency_key = "archive-revoked-start-room".to_owned();
        let revoked_room = creating_backend
            .create_room(&host, revoked_request)
            .unwrap_or_else(|error| panic!("create Archive Room for revocation: {error}"));
        let room_id = room
            .room_id
            .parse::<RoomId>()
            .unwrap_or_else(|_| panic!("room id"));
        creating_backend
            .ensure_verified_active(&room_id)
            .unwrap_or_else(|error| panic!("activate archive Room: {error}"));
        let revoked_room_id = revoked_room
            .room_id
            .parse::<RoomId>()
            .unwrap_or_else(|_| panic!("revoked room id"));
        creating_backend
            .ensure_verified_active(&revoked_room_id)
            .unwrap_or_else(|error| panic!("activate Archive Room for revocation: {error}"));
        assert_eq!(
            registry
                .activity_start_compatibility(&pack.digest.parse().unwrap())
                .unwrap_or_else(|error| panic!("activity start compatibility: {error}")),
            worldstream_core::ActivityStartCompatibilityV1::Supported(
                descriptor
                    .activity_start_contract
                    .clone()
                    .unwrap_or_else(|| {
                        panic!("archive descriptor has no Activity Start contract")
                    }),
            )
        );

        // Reopen the backend with the same exact revision retained for old
        // Rooms but withdrawn from new-room selection. The already-created
        // Room must still be able to resolve and commit its declared start.
        let (retained_registry, retained_pack, _) = archive_registry(false, true);
        assert_eq!(retained_pack, pack);
        assert!(
            retained_registry
                .select_for_new_room(&pack.digest.parse().unwrap())
                .is_err()
        );
        assert!(
            retained_registry
                .load_retained(&pack.digest.parse().unwrap())
                .is_ok()
        );
        let backend = Arc::new(SqliteGatewayBackend::new(store.clone(), retained_registry));
        backend
            .ensure_verified_active(&room_id)
            .unwrap_or_else(|error| panic!("reopen archive Room: {error}"));

        let input_id = "01ARZ3NDEKTSV4RRFFQ69G5G05";
        let launch = LobbyLaunchRequest {
            input_id: input_id.to_owned(),
            based_on_room_seq: 0,
            pack_digest: Some(pack.digest.clone()),
        };
        assert_eq!(
            backend
                .resolve_lobby_launch(&host, &room.room_id, launch.clone())
                .unwrap_or_else(|error| panic!("resolve absent start: {error}")),
            None
        );
        let before_unauthorized = backend
            .store
            .gateway_room_snapshot(&backend.registry, &room_id)
            .unwrap_or_else(|_| panic!("read archive Room before unauthorized request"))
            .unwrap_or_else(|| panic!("archive Room absent before unauthorized request"))
            .trace()
            .head()
            .clone();
        let non_operator = session(0xc2, "01ARZ3NDEKTSV4RRFFQ69G5G0A");
        assert!(matches!(
            backend.launch_lobby(&non_operator, &room.room_id, launch.clone()),
            Err(BackendError::Forbidden)
        ));
        assert!(matches!(
            backend.resolve_lobby_launch(&non_operator, &room.room_id, launch.clone()),
            Err(BackendError::Forbidden)
        ));
        let after_unauthorized = backend
            .store
            .gateway_room_snapshot(&backend.registry, &room_id)
            .unwrap_or_else(|_| panic!("read archive Room after unauthorized request"))
            .unwrap_or_else(|| panic!("archive Room absent after unauthorized request"))
            .trace()
            .head()
            .clone();
        assert_eq!(after_unauthorized, before_unauthorized);

        let concurrent = (0..4)
            .map(|index| {
                let backend = Arc::clone(&backend);
                let room_id = room.room_id.clone();
                let launch = launch.clone();
                thread::spawn(move || {
                    backend.launch_lobby(
                        &session(0xc1, &format!("01ARZ3NDEKTSV4RRFFQ69G5G0{index}")),
                        &room_id,
                        launch,
                    )
                })
            })
            .collect::<Vec<_>>();
        let mut successful = Vec::new();
        for result in concurrent {
            match result
                .join()
                .unwrap_or_else(|_| panic!("concurrent start panicked"))
            {
                Ok(response) => successful.push(response),
                Err(BackendError::Busy | BackendError::WrongPhase) => {}
                Err(error) => panic!("concurrent start failed unexpectedly: {error}"),
            }
        }
        assert!(!successful.is_empty(), "one concurrent start must commit");
        let first = successful
            .iter()
            .find(|response| !response.duplicate)
            .unwrap_or_else(|| panic!("one concurrent start must be the commit"));
        assert_eq!(first.room_head.room_seq, 1);
        assert_eq!(
            successful
                .iter()
                .filter(|response| !response.duplicate)
                .count(),
            1
        );
        let resolved = backend
            .resolve_lobby_launch(&host, &room.room_id, launch.clone())
            .unwrap_or_else(|error| panic!("resolve committed start: {error}"))
            .unwrap_or_else(|| panic!("committed start absent"));
        assert!(resolved.duplicate);
        assert_eq!(resolved.transition_id, first.transition_id);
        assert_eq!(resolved.room_head, first.room_head);

        let before = backend
            .store
            .gateway_room_snapshot(&backend.registry, &room_id)
            .unwrap_or_else(|_| panic!("read archive Room"))
            .unwrap_or_else(|| panic!("archive Room absent"))
            .trace()
            .head()
            .clone();
        let wrong_phase = backend.launch_lobby(
            &host,
            &room.room_id,
            LobbyLaunchRequest {
                input_id: "01ARZ3NDEKTSV4RRFFQ69G5G06".to_owned(),
                based_on_room_seq: 1,
                pack_digest: Some(pack.digest.clone()),
            },
        );
        assert!(matches!(wrong_phase, Err(BackendError::WrongPhase)));
        let after = backend
            .store
            .gateway_room_snapshot(&backend.registry, &room_id)
            .unwrap_or_else(|_| panic!("read archive Room after rejection"))
            .unwrap_or_else(|| panic!("archive Room absent after rejection"))
            .trace()
            .head()
            .clone();
        assert_eq!(after, before);

        // Revocation is a separate immutable startup fact. It blocks an
        // absent start without erasing the Pack's declaration or preventing
        // recovery of a start that already committed under approval.
        let (revoked_registry, revoked_pack, _) = archive_registry(false, false);
        assert_eq!(revoked_pack, pack);
        let pack_digest = pack
            .digest
            .parse()
            .unwrap_or_else(|_| panic!("pack digest"));
        assert!(
            !revoked_registry
                .activity_start_is_approved(&pack_digest)
                .unwrap_or_else(|error| panic!("read revoked approval: {error}"))
        );
        assert!(matches!(
            revoked_registry.activity_start_compatibility(&pack_digest),
            Ok(worldstream_core::ActivityStartCompatibilityV1::Supported(_))
        ));
        let revoked_backend = SqliteGatewayBackend::new(store.clone(), revoked_registry);
        let recovered = revoked_backend
            .resolve_lobby_launch(&host, &room.room_id, launch.clone())
            .unwrap_or_else(|error| panic!("recover committed start after revocation: {error}"))
            .unwrap_or_else(|| panic!("committed start missing after revocation"));
        assert_eq!(recovered.transition_id, first.transition_id);
        assert!(recovered.duplicate);

        let revoked_launch = LobbyLaunchRequest {
            input_id: "01ARZ3NDEKTSV4RRFFQ69G5G07".to_owned(),
            based_on_room_seq: 0,
            pack_digest: Some(pack.digest.clone()),
        };
        assert_eq!(
            revoked_backend
                .resolve_lobby_launch(&host, &revoked_room.room_id, revoked_launch.clone())
                .unwrap_or_else(|error| panic!("resolve absent revoked start: {error}")),
            None
        );
        let before_revoked = revoked_backend
            .store
            .gateway_room_snapshot(&revoked_backend.registry, &revoked_room_id)
            .unwrap_or_else(|_| panic!("read revoked Archive Room"))
            .unwrap_or_else(|| panic!("revoked Archive Room absent"))
            .trace()
            .head()
            .clone();
        assert!(matches!(
            revoked_backend.launch_lobby(&host, &revoked_room.room_id, revoked_launch),
            Err(BackendError::WrongPhase)
        ));
        let after_revoked = revoked_backend
            .store
            .gateway_room_snapshot(&revoked_backend.registry, &revoked_room_id)
            .unwrap_or_else(|_| panic!("read revoked Archive Room after rejection"))
            .unwrap_or_else(|| panic!("revoked Archive Room absent after rejection"))
            .trace()
            .head()
            .clone();
        assert_eq!(after_revoked, before_revoked);

        let (_unapproved_directory, unapproved_file) = database_fixture();
        let unapproved_store =
            SqliteRoomStore::open(unapproved_file.path()).unwrap_or_else(|_| panic!("open db"));
        let (unapproved_registry, unapproved_pack, unapproved_corpus) =
            archive_registry(false, true);
        AuthorityV1::new(Arc::new(unapproved_store.clone()))
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5G12".parse().unwrap(),
                    "01ARZ3NDEKTSV4RRFFQ69G5G01".parse().unwrap(),
                    PrincipalKindV1::Human,
                    "01ARZ3NDEKTSV4RRFFQ69G5G13".parse().unwrap(),
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap(),
                "2026-08-15T12:00:00Z".parse().unwrap(),
            )
            .unwrap();
        let unapproved = SqliteGatewayBackend::new(unapproved_store, unapproved_registry);
        assert!(matches!(
            unapproved.create_room(
                &host,
                CreateRoomRequest {
                    pack: unapproved_pack,
                    configuration: serde_json::to_value(&unapproved_corpus.genesis.configuration)
                        .unwrap_or_else(|_| panic!("unapproved archive configuration")),
                    members: [
                        ("01ARZ3NDEKTSV4RRFFQ69G5FZ0", "lead"),
                        ("01ARZ3NDEKTSV4RRFFQ69G5FZ1", "mira"),
                        ("01ARZ3NDEKTSV4RRFFQ69G5FZ2", "jonah"),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (principal_id, role))| CreateMember {
                        principal_id: principal_id.to_owned(),
                        principal_kind: if index == 0 {
                            PrincipalKind::Human
                        } else {
                            PrincipalKind::Agent
                        },
                        role: Some(role.to_owned()),
                        access_mode: AccessMode::Participant,
                    })
                    .collect(),
                    idempotency_key: "archive-unapproved-room".to_owned(),
                },
            ),
            Err(BackendError::Rejected)
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn sqlite_gateway_action_is_authorized_duplicate_conflict_and_stale_safe() {
        let (_database_directory, file) = database_fixture();
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
        let clock = Arc::new(CountingWallClock {
            calls: AtomicUsize::new(0),
        });
        let backend = SqliteGatewayBackend::with_runtime(
            store,
            registry,
            clock.clone(),
            RoomAdmissionLanesV1::new(2, 1)
                .unwrap_or_else(|error| panic!("test Room lane: {error}")),
        );
        let host_session = session(0xa9, "01ARZ3NDEKTSV4RRFFQ69G5FC5");
        let created = backend
            .create_room(&host_session, create)
            .unwrap_or_else(|error| panic!("create Room: {error:?}"));
        let room_id = created.room_id.clone();
        let member_id = created.member_ids[0].clone();
        let room_id_typed = room_id
            .parse::<RoomId>()
            .unwrap_or_else(|_| panic!("Room ID"));
        let member_id_typed = member_id
            .parse::<worldstream_core::MemberId>()
            .unwrap_or_else(|_| panic!("Member ID"));
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
                    room_id: room_id_typed.clone(),
                    member_id: member_id_typed.clone(),
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
        backend
            .ensure_verified_active(&room_id_typed)
            .unwrap_or_else(|error| panic!("activate test Room: {error:?}"));
        let occupied = backend
            .admission_lanes
            .reserve_action(&room_id_typed, backend.host_clock.as_ref())
            .unwrap_or_else(|error| panic!("occupy participant lane: {error}"));
        let samples_before_busy = clock.calls.load(Ordering::Relaxed);
        assert!(matches!(
            backend.action(
                &member_session,
                action("01ARZ3NDEKTSV4RRFFQ69G5FD0", 0, json!({})),
            ),
            Err(BackendError::Busy)
        ));
        assert_eq!(clock.calls.load(Ordering::Relaxed), samples_before_busy);
        drop(occupied);

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
        let (_database_directory, file) = database_fixture();
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
                    live: None,
                },
            );
            rows.insert(
                second_session.clone(),
                SessionBinding {
                    capability_id: second_capability,
                    sync: None,
                    live: None,
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
        let (_database_directory, file) = database_fixture();
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
        let (_database_directory, file) = database_fixture();
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
        let (_database_directory, file) = database_fixture();
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

    #[test]
    fn projection_wire_preserves_exact_hash_input_for_unbounded_offer() {
        let source = worldstream_core::CanonicalJsonV1::parse(
            br#"{"action_offers":[{"action_type":"inspect","domain":"worldstream/action-offer/v1","eligibility_window":null,"payload_schema_digest":"blake3:fixture"}],"authorized_core":{"access_mode":"participant"},"projection":{"phase":"briefing"},"projection_schema":"agent-heist/projection/v1"}"#,
        )
        .and_then(|value| value.to_bytes())
        .unwrap_or_else(|error| unreachable!("canonical view: {error}"));
        let projection = projection_from_canonical_bytes(&source)
            .unwrap_or_else(|error| unreachable!("projection conversion: {error:?}"));
        let wire = serde_json::to_value(&projection)
            .unwrap_or_else(|error| unreachable!("projection wire: {error}"));
        assert_eq!(wire["action_offers"][0]["eligibility_window"], Value::Null);
        let reconstructed = worldstream_core::CanonicalJsonV1::parse(
            &serde_json::to_vec(&serde_json::json!({
                "action_offers": projection.action_offers,
                "authorized_core": projection.core,
                "projection": projection.activity,
                "projection_schema": "agent-heist/projection/v1",
            }))
            .unwrap_or_else(|error| unreachable!("reconstructed JSON: {error}")),
        )
        .and_then(|value| value.to_bytes())
        .unwrap_or_else(|error| unreachable!("reconstructed view: {error}"));
        assert_eq!(source, reconstructed);
        assert_eq!(
            worldstream_core::projection_hash_for_canonical_bytes(&source),
            worldstream_core::projection_hash_for_canonical_bytes(&reconstructed)
        );
    }
}
