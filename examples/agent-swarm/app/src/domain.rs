//! Validated application inputs and read models.

use std::{collections::HashSet, path::PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_GOAL_BYTES: usize = 16 * 1024;
const MAX_ITEM_BYTES: usize = 2 * 1024;
const MAX_ITEMS: usize = 64;
const MAX_MEMBERS: usize = 16;
const MAX_LABEL_BYTES: usize = 128;
pub const DEFAULT_PROGRESS_REVIEW_INTERVAL_SECONDS: u32 = 300;
const MAX_PROGRESS_REVIEW_INTERVAL_SECONDS: u32 = 31_536_000;
pub const DEFAULT_CORRECTION_FAILURE_LIMIT: u32 = 3;
const MAX_CORRECTION_FAILURE_LIMIT: u32 = 64;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SwarmId(String);

impl SwarmId {
    /// Constructs a bounded application Swarm identifier.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::InvalidSwarmId`] for empty, oversized, or
    /// non-portable identifiers.
    pub fn new(value: String) -> Result<Self, ValidationError> {
        if valid_identifier(&value) {
            Ok(Self(value))
        } else {
            Err(ValidationError::InvalidSwarmId)
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCriterion {
    pub text: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderConfigurationState {
    /// A controlled test member. No provider process will be started.
    FixtureUnavailable,
    /// Requested settings are retained but no provider has reported resolution.
    ResolutionUnreported,
}

/// Operational state of the local coordinator loop.
///
/// This is deliberately separate from [`SwarmView`]: it describes local
/// execution progress and never changes or completes authoritative Room work.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorLoopState {
    Idle,
    Running,
    WaitingForCapacity,
    WaitingForExplicitResume,
    RecoveryRequired,
    ManualReconciliationRequired,
}

/// How much of a provider's effective configuration was actually verified.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderEffectiveState {
    AwaitingReport,
    Verified,
    Unreported,
    Mismatch,
    Unsupported,
}

/// Non-authoritative provider configuration evidence for one Invocation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderOperationalView {
    pub state: ProviderEffectiveState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reported_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reported_effort: Option<String>,
    /// A provider-reported conversation identity. It is not a Room identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reported_session_id: Option<String>,
}

/// Local coordinator disposition for one Invocation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoordinatorOutcome {
    Pending,
    Running,
    ProcessingResult,
    LaunchUncertain,
    NeedsReevaluation,
    SubmissionUncertain,
    Accepted { duplicate: bool },
    Rejected { code: String, duplicate: bool },
    NotSubmitted { reason: String },
    SettledWithoutSubmission,
    PlanningDecisionRecorded,
}

/// Local adaptive-loop status. Neither a waiting planner nor exhausted budget
/// means that the Room has accepted a Result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AutonomyView {
    pub phase: String,
    pub max_work_items: usize,
    pub invocation_limit: usize,
    pub generated_work_count: usize,
    pub generated_invocation_count: usize,
    pub reason: String,
}

/// Safe local status for one retained coordinator Invocation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorInvocationStatus {
    pub invocation_id: String,
    pub member_key: String,
    pub provider: String,
    pub configuration_revision: u64,
    pub requested_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_effort: Option<String>,
    pub provider_evidence: ProviderOperationalView,
    pub coordinator_outcome: CoordinatorOutcome,
}

/// Read-only local coordinator status surfaced beside daemon state.
///
/// `loop_state` is the last atomically published observation, not a liveness
/// claim. No field in this view is an authoritative Room fact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorServiceView {
    pub swarm_id: SwarmId,
    pub revision: u64,
    pub loop_state: CoordinatorLoopState,
    pub pending_count: usize,
    pub manual_attention_count: usize,
    #[serde(default)]
    pub progress_reviews: Vec<ProgressReviewOperationalView>,
    pub invocations: Vec<CoordinatorInvocationStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<AutonomyView>,
}

/// Canonical status copied from one exact outstanding Progress Review.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressReviewAuthoritativeStatus {
    Due,
    Claimed,
    Blocked,
}

/// Local execution disposition for an authoritative review obligation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressReviewExecutionStatus {
    Due,
    Claimed,
    WaitingForCapacity,
    Running,
    Blocked,
    NoEligibleProvider,
    ManualReconciliationRequired,
}

/// Safe operational correspondence between Pack review work and local worker
/// execution. It contains no prompt, credential, or provider output.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressReviewOperationalView {
    pub review_id: String,
    pub review_revision: u64,
    pub work_id: String,
    pub work_revision: u64,
    pub authoritative_status: ProgressReviewAuthoritativeStatus,
    pub execution_status: ProgressReviewExecutionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invocation_id: Option<String>,
}

impl ProviderConfigurationState {
    #[must_use]
    pub const fn display_label(self) -> &'static str {
        match self {
            Self::FixtureUnavailable => "fixture only · unavailable",
            Self::ResolutionUnreported => "resolution unreported",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemberConfiguration {
    pub member_key: String,
    pub label: String,
    pub provider: String,
    pub requested_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_effort: Option<String>,
    /// Authoritative per-member selection revision. Creation always starts at
    /// one; a Human configuration Action advances it without replacing the
    /// Participant identity.
    #[serde(default = "initial_configuration_revision")]
    pub configuration_revision: u64,
    /// Records the Human's deliberate choice when the requested model is a
    /// moving or otherwise unpinned provider alias.
    #[serde(default)]
    pub moving_alias_acknowledged: bool,
    pub configuration_state: ProviderConfigurationState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSwarm {
    pub goal: String,
    pub constraints: Vec<String>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub working_area: PathBuf,
    pub roster: Vec<MemberConfiguration>,
    #[serde(default = "default_progress_review_interval_seconds")]
    pub progress_review_interval_seconds: u32,
    #[serde(default = "default_correction_failure_limit")]
    pub correction_failure_limit: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatedCreateSwarm {
    pub goal: String,
    pub constraints: Vec<String>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub working_area: PathBuf,
    pub roster: Vec<MemberConfiguration>,
    #[serde(default = "default_progress_review_interval_seconds")]
    pub progress_review_interval_seconds: u32,
    #[serde(default = "default_correction_failure_limit")]
    pub correction_failure_limit: u32,
}

impl TryFrom<CreateSwarm> for ValidatedCreateSwarm {
    type Error = ValidationError;

    fn try_from(value: CreateSwarm) -> Result<Self, Self::Error> {
        bounded_text(&value.goal, MAX_GOAL_BYTES).ok_or(ValidationError::InvalidGoal)?;
        if value.constraints.len() > MAX_ITEMS {
            return Err(ValidationError::TooManyConstraints);
        }
        for constraint in &value.constraints {
            bounded_text(constraint, MAX_ITEM_BYTES).ok_or(ValidationError::InvalidConstraint)?;
        }
        if value.acceptance_criteria.is_empty() || value.acceptance_criteria.len() > MAX_ITEMS {
            return Err(ValidationError::InvalidAcceptanceCriteria);
        }
        for criterion in &value.acceptance_criteria {
            bounded_text(&criterion.text, MAX_ITEM_BYTES)
                .ok_or(ValidationError::InvalidAcceptanceCriteria)?;
        }
        if !value.working_area.is_absolute() {
            return Err(ValidationError::WorkingAreaNotAbsolute);
        }
        if value.roster.is_empty() || value.roster.len() > MAX_MEMBERS {
            return Err(ValidationError::InvalidRosterSize);
        }
        if value.progress_review_interval_seconds == 0
            || value.progress_review_interval_seconds > MAX_PROGRESS_REVIEW_INTERVAL_SECONDS
        {
            return Err(ValidationError::InvalidProgressReviewInterval);
        }
        if value.correction_failure_limit == 0
            || value.correction_failure_limit > MAX_CORRECTION_FAILURE_LIMIT
        {
            return Err(ValidationError::InvalidCorrectionFailureLimit);
        }
        let mut keys = HashSet::with_capacity(value.roster.len());
        for member in &value.roster {
            if !valid_identifier(&member.member_key)
                || !keys.insert(member.member_key.as_str())
                || bounded_text(&member.label, MAX_LABEL_BYTES).is_none()
                || bounded_text(&member.provider, MAX_LABEL_BYTES).is_none()
                || bounded_text(&member.requested_model, MAX_LABEL_BYTES).is_none()
                || member.configuration_revision != initial_configuration_revision()
                || (model_needs_moving_alias_acknowledgement(&member.requested_model)
                    && !member.moving_alias_acknowledged)
                || member
                    .requested_effort
                    .as_ref()
                    .is_some_and(|effort| bounded_text(effort, MAX_LABEL_BYTES).is_none())
            {
                return Err(ValidationError::InvalidMember);
            }
        }
        Ok(Self {
            goal: value.goal,
            constraints: value.constraints,
            acceptance_criteria: value.acceptance_criteria,
            working_area: value.working_area,
            roster: value.roster,
            progress_review_interval_seconds: value.progress_review_interval_seconds,
            correction_failure_limit: value.correction_failure_limit,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SwarmSummary {
    pub swarm_id: SwarmId,
    pub room_id: String,
    pub goal: String,
    pub member_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SwarmView {
    pub swarm_id: SwarmId,
    pub room_id: String,
    pub goal: String,
    pub constraints: Vec<String>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub working_area: PathBuf,
    pub roster: Vec<MemberConfiguration>,
    #[serde(default = "default_progress_review_interval_seconds")]
    pub progress_review_interval_seconds: u32,
    #[serde(default = "default_correction_failure_limit")]
    pub correction_failure_limit: u32,
    /// Identifies whether this is canonical `WorldStream` data or a test fixture.
    pub source_label: String,
}

impl SwarmView {
    #[must_use]
    pub fn summary(&self) -> SwarmSummary {
        SwarmSummary {
            swarm_id: self.swarm_id.clone(),
            room_id: self.room_id.clone(),
            goal: self.goal.clone(),
            member_count: self.roster.len(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ValidationError {
    #[error("swarm id is invalid")]
    InvalidSwarmId,
    #[error("goal is empty or too large")]
    InvalidGoal,
    #[error("too many constraints")]
    TooManyConstraints,
    #[error("constraint is empty or too large")]
    InvalidConstraint,
    #[error("acceptance criteria are empty, invalid, or too numerous")]
    InvalidAcceptanceCriteria,
    #[error("working area must be an absolute path")]
    WorkingAreaNotAbsolute,
    #[error("roster size is invalid")]
    InvalidRosterSize,
    #[error("roster member is invalid or duplicated")]
    InvalidMember,
    #[error("progress review interval is outside the supported range")]
    InvalidProgressReviewInterval,
    #[error("correction failure limit is outside the supported range")]
    InvalidCorrectionFailureLimit,
}

const fn default_progress_review_interval_seconds() -> u32 {
    DEFAULT_PROGRESS_REVIEW_INTERVAL_SECONDS
}

const fn initial_configuration_revision() -> u64 {
    1
}

const fn default_correction_failure_limit() -> u32 {
    DEFAULT_CORRECTION_FAILURE_LIMIT
}

fn bounded_text(value: &str, maximum: usize) -> Option<()> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && value.len() <= maximum).then_some(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LABEL_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn model_needs_moving_alias_acknowledgement(model: &str) -> bool {
    let mut pinned_version = false;
    for segment in model.split(['-', '_', '.', ':']) {
        let normalized = segment.to_ascii_lowercase();
        if matches!(normalized.as_str(), "auto" | "default" | "latest") {
            return true;
        }
        let digits = normalized.strip_prefix('v').unwrap_or(&normalized);
        pinned_version |= !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit());
    }
    !pinned_version
}
