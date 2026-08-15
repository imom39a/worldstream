//! Pure in-memory implementation of `WorldStream` Core v1.
//!
//! The module owns canonical Core construction, strict canonical JSON, the
//! domain-separated BLAKE3 lineage, accepted-transition tracing, and replay.
//! It deliberately contains no storage, network, frame, scheduler, Activation,
//! plugin, or server wiring.

mod canonical;
mod lineage;
mod model;
mod primitives;
mod reducer;
mod trace;

pub use canonical::{CanonicalJsonError, CanonicalJsonV1, MAX_SAFE_INTEGER, MIN_SAFE_INTEGER};
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
};
pub use reducer::{CORE_OPERATION_KIND, CoreValidationErrorV1};
pub use trace::{
    AdvanceDispositionV1, CoreReducerV1, CoreTraceV1, PackFaultV1, PreparedCoreStateV1,
    PreparedRoomTransitionV1, ReplayFailureClassV1, ReplayFailureV1, ReplayReportV1, ReplayStepV1,
    RoomTransitionPreparerV1, RoomTransitionStateV1, TraceErrorV1, VerifiedCoreStateV1,
};

#[cfg(test)]
mod tests;
