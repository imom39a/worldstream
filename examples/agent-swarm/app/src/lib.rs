//! Local Agent Swarm application boundary.
//!
//! The optional `managed-local-runtime` feature provides an authenticated local
//! `WorldStream` backend. [`fixture::FixtureFileBackend`] remains deterministic
//! test scaffolding; its records are not authoritative Room state.

pub mod application;
pub mod artifacts;
pub mod backend;
pub mod checks;
pub mod code_change;
pub mod code_change_service;
#[cfg(feature = "managed-local-runtime")]
pub mod coordinator;
#[cfg(feature = "managed-local-runtime")]
pub mod coordinator_service;
pub mod domain;
#[cfg(feature = "managed-local-runtime")]
pub mod execution;
pub mod fixture;
#[cfg(feature = "managed-local-runtime")]
pub mod managed_local;
#[cfg(feature = "managed-local-runtime")]
pub mod planning;
pub mod smoke;
pub mod tui;

pub use application::{ApplicationError, SwarmApplication};
pub use artifacts::{
    ArtifactError, ArtifactPath, ArtifactRef, ArtifactWorkspace, AuthoritativeArtifactRef,
    ContentDigest, authoritative_artifact_for_path,
};
pub use backend::{
    BackendError, ExactSwarmAction, SwarmActionOffer, SwarmActionReceipt, SwarmActor, SwarmBackend,
    SwarmObservation,
};
pub use code_change_service::{
    BoundInputRequest, CandidateWorkflowStatus, CheckStatus, CodeChangeService,
    CodeChangeServiceError, CodeChangeServiceStatus, PackArtifactRef, PackCandidateRef,
    PackCheckEvidenceRef, PackRecordCheckPayload, PackVersionRef, PrepareCodeChangeRequest,
    PreparedCodeChange, RecordCodeReviewRequest, RecordedCodeCheck, RecordedCodeReview, ReviewGate,
    RevisionWorkflowStatus, RunCodeCheckRequest, SealCodeChangeRequest, SealedCodeChange,
    StageCodeWriteBackRequest, WriteBackWorkflowStatus,
};
#[cfg(feature = "managed-local-runtime")]
pub use coordinator::{
    CoordinatorError, CoordinatorEvent, CoordinatorIntentState, CoordinatorIntentView,
    InvocationEvidence, NonSubmissionReason, SubmissionDisposition, SwarmCoordinator,
    WorkerActionPlan, WorkerSemanticTarget,
};
#[cfg(feature = "managed-local-runtime")]
pub use coordinator_service::{
    AutonomyPolicy, CoordinatorRunReceipt, CoordinatorService, CoordinatorServiceError,
    DeliveryCheckPolicy, DeliveryPolicy, read_coordinator_service_view,
};
pub use domain::{
    AcceptanceCriterion, CoordinatorInvocationStatus, CoordinatorLoopState, CoordinatorOutcome,
    CoordinatorServiceView, CreateSwarm, DEFAULT_CORRECTION_FAILURE_LIMIT,
    DEFAULT_PROGRESS_REVIEW_INTERVAL_SECONDS, MemberConfiguration,
    ProgressReviewAuthoritativeStatus, ProgressReviewExecutionStatus,
    ProgressReviewOperationalView, ProviderConfigurationState, ProviderEffectiveState,
    ProviderOperationalView, SwarmId, SwarmSummary, SwarmView, ValidatedCreateSwarm,
    ValidationError,
};
