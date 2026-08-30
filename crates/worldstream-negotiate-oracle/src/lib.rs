#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::large_enum_variant,
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::unused_self
)]

//! Independent, I/O-free executable specification for the first WorldStream
//! Negotiate profile.
//!
//! This crate intentionally does not depend on `worldstream-core` and is never
//! a production Activity Pack executor. Its public, serializable inputs and
//! outputs are the differential-comparison seam for other implementations.

mod fixtures;
mod model;
mod oracle;
mod privacy;

pub use fixtures::{
    AlternateOutcomeV1, CORPUS_FIXTURE_BYTES, CorpusV1, GoldenCorpusV1, GoldenPlanV1, GoldenStepV1,
    NegativeCaseV1, NegativeKindV1, PrivacyMutationCaseV1, corpus, corpus_bytes, golden_corpus,
    golden_plan, negative_cases, privacy_mutation_cases, run_expiry_path, run_negative,
};
pub use model::{
    A202_PINNED_REVISION, ACTIONS, ATTENTION_REASONS, Action, ActionBasis, ActionKind,
    AgreementSignature, ApprovalBinding, ApprovalDecision, Attention, AttentionReason,
    BoundaryProbe, BundleCondition, DeadlineProgress, EvidenceLink, ExactA202Object, ExactApproval,
    FixtureSignature, GoldenCheckpoint, LogicalHead, OracleError, OracleState, Outcome, PERSONAS,
    PHASES, Persona, Phase, PrivacyClass, PrivacyExposure, Proposal, ROLES, RejectionCode, Role,
    RoomHead, Stimulus, TimerFired, TimerKind, Transition, canonical_bytes, tagged_blake3,
};
pub use oracle::{Oracle, validate_boundary_probe};
pub use privacy::{
    PRIVACY_MATRIX, PrivacyMatrixRow, PrivacyProbe, PrivacyProjection, privacy_projection,
};
