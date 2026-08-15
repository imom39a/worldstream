//! Canonical implementation of `WorldStream` Core v1.
//!
//! The module owns canonical Core construction, strict canonical JSON, the
//! domain-separated BLAKE3 lineage, accepted-transition tracing, replay, and
//! the backend-neutral prepared Room commit/recovery interfaces. It also owns
//! opaque Frame and Activation consequence types. Concrete database, network,
//! scheduler, plugin, and server wiring live in substitutable adapters.

mod activity_pack;
mod canonical;
mod counter;
mod counter_registry;
mod lineage;
mod model;
mod primitives;
mod reducer;
mod room_commit;
mod trace;

pub use activity_pack::{
    ACTION_OFFER_DOMAIN, ACTIVITY_PACK_HOST_CONTRACT_ID, ActionAdmissionErrorV1,
    ActionDefinitionV1, ActionOfferV1, ActivityGenesisInputV1, ActivityObservationOutcomeV1,
    ActivityPackHostV1, ActivityPackOperationV1, ActivityPackReduceErrorV1, ActivityPackV1,
    CanonicalActionOffersV1, CanonicalPackCodecV1, DeterministicContextErrorV1,
    DeterministicContextV1, EligibilityTimeV1, EligibilityWindowV1, InitialOutputV1, NamedDigestV1,
    ObserveInputV1, PACK_REVISION_LOCK_ID, PackCodecBundleV1, PackCodecKindV1, PackGenesisErrorV1,
    PackGenesisRequestV1, PackGoldenActionV1, PackGoldenCorpusV1, PackGoldenViewerKindV1,
    PackGoldenViewerV1, PackLimitsV1, PackObservationV1, PackRegistryErrorV1, PackRegistryStatusV1,
    PackRegistryV1, PackRevisionDescriptorV1, PackRevisionLockV1, PackSchemaBundleV1, PackSchemaV1,
    PackViewV1, PackViewerClassV1, PackViewerV1, PreparedNewRoomGenesisV1, RetainedActivityPackV1,
    RoleDefinitionV1, SchemaReferenceV1, ValidatedPackObservationV1, ValidatedPackViewV1,
    ViewInputV1,
};
pub use canonical::{CanonicalJsonError, CanonicalJsonV1, MAX_SAFE_INTEGER, MIN_SAFE_INTEGER};
pub use counter_registry::{builtin_counter_registry, counter_v1_digest, counter_v2_digest};
#[cfg(any(test, feature = "conformance-tracer"))]
pub use counter_registry::{
    counter_v1_only_registry_for_conformance,
    counter_v2_invalid_timer_output_registry_for_conformance,
    counter_v2_malformed_output_registry_for_conformance,
    counter_v2_returned_fault_registry_for_conformance,
    counter_v2_runtime_fault_registry_for_conformance,
    counter_v2_semantic_mismatch_registry_for_conformance,
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
    ActionAdmittedAt, ActionId, Blake3DigestV1, CoreRecordedAt, CreationRecordedAt,
    DigestParseError, ExternalInputRecordedAt, InputId, IntegrityGenerationV1, MemberId,
    PackDigestV1, PrincipalId, RoomId, RoomSeedV1, RoomSequenceV1, SafeCounterError,
    SeedParseError, SourceId, TimerGenerationV1, TimerId, TimerScheduledFor, TimestampParseError,
    TransitionId,
};
pub use reducer::{CORE_OPERATION_KIND, CoreValidationErrorV1};
pub use room_commit::{
    ActionAdmissionContextV1, ActionOfferWitnessV1, ActionOffersUnavailableReasonV1,
    ActorInstallationV1, CREATE_ROOM_OPERATION_KIND, CanonicalRequestHashV1,
    ExistingRoomCommitOutcomeV1, ExistingRoomPendingAttemptV1, ExistingRoomReprepareV1,
    ExistingRoomResolveV1, ExistingRoomRetryV1, ExternalInputOperationIdentityV1,
    InitialMembershipProposalV1, OperationIdentityV1, ParticipantActionOperationIdentityV1,
    ParticipantActionReprepareV1, ParticipantActionRequestV1, PrepareRoomWriteErrorV1,
    PreparedActionInputWitnessV1, PreparedActivationDecisionV1, PreparedAdvancePersistenceV1,
    PreparedAuthorityWitnessV1, PreparedCreationPersistenceV1, PreparedExistingIntentV1,
    PreparedMembershipMaterializationV1, PreparedObservationFrameV1,
    PreparedOperationInputWitnessV1, PreparedRoomCommitV1, PreparedRoomCreationV1,
    PreparedRoomWriteV1, PreparedTimerInputWitnessV1, PreparedTimerMaterializationV1,
    PreparedTimerMutationKindV1, PreparedTimerMutationV1, ReceiptSemanticInputV1,
    ReceiptSemanticTimeV1, RecoveredObservationFrameV1, RecoveredRoomMaterializationsV1,
    RecoveredTimerMaterializationV1, RecoveredTimerStateV1, RecoveryIntegrityDispositionV1,
    ResolutionStatusV1, ResolveOutcomeV1, RoomCommitResolutionV1, RoomCommitStorageV1,
    RoomCreationCommitOutcomeV1, RoomCreationPendingAttemptV1, RoomCreationReprepareV1,
    RoomCreationRequestV1, RoomCreationResolveV1, RoomCreationRetryV1, RoomRecoveryCandidateV1,
    RoomRecoveryErrorV1, RoomRecoveryStorageV1, SemanticResultV1, StoredSemanticResultV1,
    TimerFiredReprepareV1, TimerFiredRequestV1, TimerOperationIdentityV1, TimerReprepareOutcomeV1,
    commit_existing_room, commit_room_creation, recover_room_from_storage,
};
pub use trace::{
    AdvanceDispositionV1, CoreReducerV1, CoreTraceV1, PackFaultV1, PreparedCoreStateV1,
    PreparedRoomTransitionV1, ReplayFailureClassV1, ReplayFailureV1, ReplayReportV1, ReplayStepV1,
    RoomTransitionPreparerV1, RoomTransitionStateV1, TraceErrorV1, VerifiedCoreStateV1,
};

#[cfg(test)]
mod room_commit_tests;
#[cfg(test)]
mod tests;
