//! Canonical implementation of `WorldStream` Core v1.
//!
//! The module owns canonical Core construction, strict canonical JSON, the
//! domain-separated BLAKE3 lineage, accepted-transition tracing, replay, and
//! the backend-neutral prepared Room commit/recovery and operational authority
//! interfaces. It also owns opaque Frame and Activation consequence types.
//! Concrete database, network, scheduler, plugin, and server wiring live in
//! substitutable adapters.

mod activation;
mod activity_pack;
mod agent_heist;
mod agent_heist_clock_safe;
mod agent_heist_lobby;
mod agent_heist_lobby_v3;
mod agent_heist_lobby_v4;
#[cfg(test)]
mod agent_heist_lobby_v4_tests;
mod agent_heist_lobby_v5;
#[cfg(test)]
mod agent_heist_lobby_v5_tests;
mod agent_heist_registry;
mod authority;
mod canonical;
mod counter;
#[allow(
    clippy::assigning_clones,
    reason = "Counter v3's self-hashed source freezes this one-time descriptor construction."
)]
mod counter_attention;
#[allow(
    clippy::assigning_clones,
    reason = "Counter v4's self-hashed source freezes this one-time descriptor construction."
)]
mod counter_attention_v4;
mod counter_registry;
mod lineage;
mod model;
mod operational_mmr;
mod primitives;
mod reducer;
mod registry;
mod room_commit;
mod room_trace_cache;
mod semantic_time;
mod session;
mod trace;

pub use activation::{
    ACTIVATION_ATTENTION_POLICY_REVISION_V1, ACTIVATION_ATTENTION_ROW_OVERHEAD_BYTES_V1,
    ActivationAttentionClassV1, ActivationAttentionV1, ActivationContextErrorV1,
    ActivationContextInputV1, ActivationDecisionV1, ActivationDeliveryV1, ActivationFrameV1,
    ActivationIntentStateV1, ActivationInvocationContextV1, ActivationOperationRequestV1,
    ActivationOperationResultV1, ActivationPolicyDecisionV1, ActivationPolicyDispositionV1,
    ActivationResultCodeV1, ActivationShapeErrorV1, MAX_ACTIVATION_EXECUTIONS_PER_MINUTE_V1,
    MAX_ACTIVATION_INVOCATION_CONTEXT_BYTES, MAX_PENDING_REFRESH_ACTIVATIONS_V1,
    MAX_PENDING_REFRESH_AGE_MS_V1, MAX_PENDING_REFRESH_BYTES_V1, activation_id_for_attention_v1,
    activation_refresh_budget_allows_v1, prepare_activation_context,
    prepare_activation_context_with_byte_limit,
};
pub use activity_pack::{
    ACTION_OFFER_DOMAIN, ACTIVITY_PACK_HOST_CONTRACT_ID, ACTIVITY_PACK_OPERATION_CODEC_ID,
    ACTIVITY_START_CONTRACT_ID, ACTIVITY_START_SOURCE_ID, ActionAdmissionErrorV1,
    ActionDefinitionV1, ActionOfferV1, ActivityGenesisInputV1, ActivityObservationOutcomeV1,
    ActivityPackCatalogRevisionV1, ActivityPackHostV1, ActivityPackOperationV1,
    ActivityPackReduceErrorV1, ActivityPackV1, ActivityStartCompatibilityV1,
    ActivityStartContractV1, CanonicalActionOffersV1, CanonicalPackCodecV1,
    CanonicalPackOperationCodecV1, DeterministicContextErrorV1, DeterministicContextV1,
    EligibilityTimeV1, EligibilityWindowV1, InitialOutputV1,
    MAX_V2_CHECKPOINT_ACTIVITY_STATE_BYTES, NamedDigestV1, ObserveInputV1, PACK_REVISION_LOCK_ID,
    PROJECTION_HASH_DOMAIN_V1, PROJECTION_SCHEMA_V1, PackCallbackFaultV1, PackCodecBundleV1,
    PackCodecKindV1, PackDescriptorContentV1, PackDeterministicContextV1, PackGenesisErrorV1,
    PackGenesisRequestV1, PackGoldenActionV1, PackGoldenCorpusV1, PackGoldenExternalInputV1,
    PackGoldenViewerKindV1, PackGoldenViewerV1, PackInitializeRequestV1, PackLimitsV1,
    PackObservationV1, PackObserveRequestV1, PackOperationResultV1, PackReduceRequestV1,
    PackRegistryErrorV1, PackRegistryStatusV1, PackRegistryV1, PackRevisionDescriptorV1,
    PackRevisionLockV1, PackSchemaBundleV1, PackSchemaV1, PackViewRequestV1, PackViewV1,
    PackViewerClassV1, PackViewerV1, PackWireObservationOffersV1, PackWireObservationV1,
    PackWireValidatedViewV1, PackWireViewerV1, PortablePackAdmissionV1, PreparedNewRoomGenesisV1,
    RetainedActivityPackV1, RoleDefinitionV1, SchemaReferenceV1, ValidatedPackObservationV1,
    ValidatedPackViewV1, ViewInputV1, activity_start_is_applicable,
    projection_hash_for_canonical_bytes,
};
pub use agent_heist::{
    ACCEPT_EXCHANGE, ACKNOWLEDGE_RESULT, AGENT_HEIST_PACK_ID, AGENT_HEIST_RETAINED_VERSION,
    AGENT_HEIST_VERSION, AgentHeistV0, AgentHeistV1, BROKER, CHALLENGE_PLAN, COMMIT_MOVE,
    ENDORSE_PLAN, INSIDER, INSPECT_CLUE, NAVIGATOR, OFFER_EXCHANGE, PROPOSE_PLAN, PUBLISH_CLUE,
    ROLES, outcome_for_matrix,
};
pub use agent_heist_lobby::{
    AGENT_HEIST_LOBBY_CONTRACT, AGENT_HEIST_LOBBY_VERSION, AgentHeistLobbyV2,
    HOST_LAUNCH_INPUT_TYPE, HOST_LOBBY_LAUNCH_SOURCE, agent_heist_lobby_launch_applicable,
};

/// The sole generic external-input kind admitted by the v1 gateway surface.
/// Activity Packs still decide whether this typed input is semantically valid.
pub const EXTERNAL_INPUT_INGRESS_SOURCE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH2";
pub const EXTERNAL_INPUT_INGRESS_TYPE: &str = "worldstream.external_input.v1";
pub use agent_heist_lobby_v3::{
    AGENT_HEIST_LOBBY_VERSION as AGENT_HEIST_CLOCK_SAFE_VERSION, AgentHeistLobbyV3,
};
pub use agent_heist_lobby_v4::{
    AGENT_HEIST_AGENT_READY_VERSION, AgentHeistLobbyV4, agent_heist_agent_ready_digest,
};
pub use agent_heist_lobby_v5::{
    AGENT_HEIST_SCHEMA_SAFE_VERSION, AgentHeistLobbyV5, agent_heist_schema_safe_digest,
};
pub use agent_heist_registry::{
    agent_heist_clock_safe_digest, agent_heist_digest, agent_heist_lobby_contract_declared,
    agent_heist_lobby_digest, agent_heist_retained_digest, builtin_agent_heist_registry,
};
#[cfg(any(test, feature = "conformance-tracer"))]
pub use authority::InMemoryAuthorityStoreV1;
pub use authority::{
    AuthorityBootstrapStateV1, AuthorityBootstrapV1, AuthorityChangeReceiptV1,
    AuthorityChangeResultV1, AuthorityChangeStatePartsV1, AuthorityChangeStateV1,
    AuthorityChangeTargetV1, AuthorityChangeV1, AuthorityErrorV1, AuthorityGrantV1,
    AuthorityReasonCodeV1, AuthorityShapeErrorV1, AuthoritySnapshotQueryV1, AuthoritySnapshotV1,
    AuthorityStoreErrorV1, AuthorityStoreV1, AuthorityUseV1, AuthorityV1,
    AuthorizedCoreAdministrationV1, AuthorizedDiagnosticV1, AuthorizedExternalInputV1,
    AuthorizedParticipantActionV1, AuthorizedReceiptReadV1, AuthorizedReplayV1,
    AuthorizedRoomCreationV1, AuthorizedRunnerControlV1, AuthorizedStableActionDispositionV1,
    AuthorizedTimerFiredV1, AuthorizedViewerV1, CapabilityAuthoritySnapshotPartsV1,
    CapabilityAuthoritySnapshotV1, CapabilityBearerV1, CapabilityProfileV1, CapabilityScopeSetV1,
    CapabilityScopeV1, CapabilityTokenHashV1, ClassifiedCoreAdministrationV1,
    CoreAdministrationClassV1, DiagnosticAdapterInputV1, DiagnosticOperationV1, DiagnosticTargetV1,
    MemberAuthorityUseV1, MemberReadOperationV1, MembershipAuthoritySnapshotV1, NewCapabilityV1,
    ParticipantActionAuthorityV1, PreparedAuthorityBootstrapV1, PreparedAuthorityChangeV1,
    PresentedCapabilityV1, PrincipalAuthoritySnapshotV1, PrincipalAuthorityStatusV1,
    ReceiptReadAdapterInputV1, ReplayAdapterInputV1, ReplayProjectionKindV1, RoomMembershipKeyV1,
    RunnerAuthoritySnapshotV1, RunnerAuthorityStatusV1, RunnerControlAdapterInputV1,
    RunnerControlOperationV1, RunnerMembershipSetV1, ValidatedAuthorityBootstrapV1,
    ValidatedAuthorityChangeV1, ViewerAdapterInputV1,
};
pub use canonical::{CanonicalJsonError, CanonicalJsonV1, MAX_SAFE_INTEGER, MIN_SAFE_INTEGER};
pub use counter_registry::{
    builtin_counter_registry, counter_v1_digest, counter_v2_digest, counter_v3_digest,
    counter_v4_digest,
};
#[cfg(any(test, feature = "conformance-tracer"))]
pub use counter_registry::{
    counter_v1_only_registry_for_conformance,
    counter_v2_invalid_timer_output_registry_for_conformance,
    counter_v2_malformed_output_registry_for_conformance,
    counter_v2_returned_fault_registry_for_conformance,
    counter_v2_runtime_fault_registry_for_conformance,
    counter_v2_semantic_mismatch_registry_for_conformance,
    counter_v3_historical_creation_registry_for_conformance,
};
pub use lineage::{
    CANONICAL_CODEC_ID, CORE_SCHEMA_VERSION, GENESIS_VERSION, GenesisV1, HASH_SUITE_ID,
    TRANSITION_VERSION, TransitionV1,
};
pub use model::{
    AccessModeV1, ActivityApplyV1, ActivityDispositionV1, ActivityReduceInputV1,
    ActivityRejectionV1, AdministrationOperationIdentityV1, CompleteHeadV1,
    CoreAuthorityAttributionV1, CoreAuthorityKindV1, CoreChangeSetV1, CoreProposedKindV1,
    CoreProposedV1, CoreRoomStateV1, CoreShapeErrorV1, ExternalInputV1, GenesisInputV1,
    MembershipChangeKindV1, MembershipChangeV1, MembershipStandingV1, MembershipV1,
    ParticipantActionV1, PrincipalKindV1, RecordedStimulusV1, RoomIntegrityAuditRecordV1,
    RoomIntegrityStateV1, RoomIntegrityStatusV1, RoomStatusV1, ScheduledTimerV1, TimerChangeV1,
    TimerFiredV1, TimerRequestV1,
};
pub use primitives::{
    ActionAdmittedAt, ActionId, AuthorityChangeId, AuthorityCheckedAt, AuthorityGenerationV1,
    Blake3DigestV1, CapabilityExpiresAt, CapabilityId, CapabilityRevokedAt, CoreRecordedAt,
    CreationRecordedAt, DigestParseError, ExternalInputRecordedAt, InputId, IntegrityGenerationV1,
    MemberId, MembershipGenerationV1, PackDigestV1, PrincipalGenerationV1, PrincipalId, RoomId,
    RoomSeedV1, RoomSequenceV1, RunnerGenerationV1, RunnerId, SafeCounterError, SeedParseError,
    SourceId, TimerGenerationV1, TimerId, TimerScheduledFor, TimestampParseError, TransitionId,
};
pub use reducer::{CORE_OPERATION_KIND, CoreValidationErrorV1};
pub use registry::builtin_worldstream_registry;
pub use room_commit::{
    ActionAdmissionContextV1, ActionOfferWitnessV1, ActionOffersUnavailableReasonV1,
    ActorInstallationV1, AuthorizedReceiptResolverV1, CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V1,
    CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V2, CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V3,
    CREATE_ROOM_OPERATION_KIND, CanonicalRequestHashV1, CoreAdministrationIngressV1,
    CoreAdministrationRequestV1, ExistingRoomCommitOutcomeV1, ExistingRoomPendingAttemptV1,
    ExistingRoomReprepareV1, ExistingRoomResolveV1, ExistingRoomRetryV1,
    ExternalInputOperationIdentityV1, InitialMembershipProposalV1, OperationIdentityV1,
    OperationalHistoryRootV2, ParticipantActionIngressErrorV1, ParticipantActionIngressV1,
    ParticipantActionOperationIdentityV1, ParticipantActionReprepareV1, ParticipantActionRequestV1,
    PrepareRoomWriteErrorV1, PreparedActionInputWitnessV1, PreparedActivationDecisionV1,
    PreparedAdvancePersistenceV1, PreparedAuthorityWitnessV1,
    PreparedCoreAdministrationInputWitnessV1, PreparedCreationPersistenceV1,
    PreparedExistingIntentV1, PreparedExternalInputWitnessV1, PreparedMembershipMaterializationV1,
    PreparedObservationConsequenceV1, PreparedObservationFrameV1, PreparedOperationInputWitnessV1,
    PreparedRoomCommitV1, PreparedRoomCreationV1, PreparedRoomWriteV1, PreparedTimerInputWitnessV1,
    PreparedTimerMaterializationV1, PreparedTimerMutationKindV1, PreparedTimerMutationV1,
    ReceiptSemanticInputV1, ReceiptSemanticTimeV1, RecoveredActivationDecisionV1,
    RecoveredObservationConsequenceV1, RecoveredObservationFrameV1, RecoveredRoomExecutionV1,
    RecoveredRoomMaterializationsV1, RecoveredTimerMaterializationV1, RecoveredTimerStateV1,
    RecoveryIntegrityDispositionV1, ResolutionStatusV1, ResolveOutcomeV1,
    RoomCheckpointOperationalWitnessV1, RoomCheckpointOperationalWitnessV2,
    RoomCheckpointOperationalWitnessV3, RoomCommitResolutionV1, RoomCommitStorageV1,
    RoomCreationCommitOutcomeV1, RoomCreationIngressV1, RoomCreationPendingAttemptV1,
    RoomCreationReprepareV1, RoomCreationRequestV1, RoomCreationResolveV1, RoomCreationRetryV1,
    RoomOperationIngressErrorV1, RoomRecoveryCandidateV1, RoomRecoveryCheckpointV1,
    RoomRecoveryErrorV1, RoomRecoveryExecutionPathV1, RoomRecoveryExecutionReceiptV1,
    RoomRecoveryStorageV1, SemanticResultV1, StoredSemanticResultV1, TimerFiredReprepareV1,
    TimerFiredRequestV1, TimerOperationIdentityV1, TimerReprepareOutcomeV1,
    VerifiedCurrentRoomMaterializationV1, authorize_core_administration_operation,
    authorize_participant_action_operation, authorize_room_creation_operation,
    commit_existing_room, commit_room_creation, external_input_request_hash,
    resolve_authorized_room_operation_for_adapter,
};
#[doc(hidden)]
pub use room_commit::{
    recover_room_from_full_storage, recover_room_from_storage,
    recover_room_from_storage_with_receipt,
};
pub use room_trace_cache::{CachedRoomTraceV1, RoomTraceCacheErrorV1, RoomTraceCacheV1};
pub use semantic_time::{
    ActionLaneReservationV1, ActionRoomAdmissionV1, AdmissionLaneClassV1, AdmissionLaneErrorV1,
    HostClockErrorV1, HostClockSampleV1, HostClockV1, LaneReservationV1, MonotonicHostClockV1,
    ROOM_ADMISSION_HOST_RESERVE_V1, ROOM_ADMISSION_LANE_CAPACITY_V1, RoomAdmissionLaneV1,
    RoomAdmissionLanesV1, RoomAdmissionQueueSnapshotV1, RoomAdmissionTurnV1,
};
pub use session::{
    CapturedSessionBarrierV1, SessionBarrierV1, SessionCloseReasonV1, SessionErrorV1,
    SessionFrameV1, SessionPublishOutcomeV1, SessionStateV1, SessionSyncTokenV1, SessionV1,
};
pub use trace::{
    AdvanceDispositionV1, CoreReducerV1, CoreTraceV1, HistoricalEvidencePageOutcomeV1,
    HistoricalEvidenceReferenceV1, HistoricalReplayAccumulatorV1, HistoricalReplayErrorV1,
    HistoricalReplayProjectionRequestV1, HistoricalReplayProjectionV1, MAX_DISTINCT_TIMER_IDS_V1,
    MAX_HISTORICAL_EVIDENCE_BYTES_PER_PAGE_V1, MAX_HISTORICAL_EVIDENCE_ROWS_PER_PAGE_V1,
    MAX_HISTORICAL_EVIDENCE_TIME_MS_V1, MAX_ROOM_MEMBERSHIPS_V1, PackFaultV1, PreparedCoreStateV1,
    PreparedRoomTransitionV1, ReplayActivationDecisionWitnessV1, ReplayFailureClassV1,
    ReplayFailureV1, ReplayMembershipWitnessV1, ReplayObservationPositionWitnessV1, ReplayReportV1,
    ReplayStepV1, ReplayStorageVerificationV1, RoomTransitionPreparerV1, RoomTransitionStateV1,
    StorageExecutableReplayV1, StorageHistoryPreflightV1, TraceErrorV1, VerifiedCoreStateV1,
};

#[cfg(test)]
mod activation_tests;
#[cfg(test)]
mod authority_tests;
#[cfg(test)]
mod room_commit_tests;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod tests;

pub use operational_mmr::{
    OperationalMmrAppendV1, OperationalMmrErrorV1, OperationalMmrNodeCoordinateV1,
    OperationalMmrNodeV1, OperationalMmrProofNodeV1, OperationalMmrProofPlanV1,
    OperationalMmrProofV1, OperationalMmrReceiptV1, OperationalMmrV1,
};
