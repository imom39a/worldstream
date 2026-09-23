//! Persistent execution loop around the per-Invocation coordinator.
//!
//! The service retains the plans it is responsible for before delegating to
//! the coordinator journal. Reopening the service performs no daemon command.
//! An explicit run may restore a missing queue entry, but it never issues
//! `Resume` and never retries an uncertain launch or Action submission.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use worldstream_runtime::{
    create_owner_only_file, create_owner_only_renameable_file, prepare_data_directory,
    validate_data_directory, validate_owner_only_file,
};

use crate::{
    CoordinatorError, CoordinatorEvent, CoordinatorIntentState, CoordinatorIntentView,
    NonSubmissionReason, SubmissionDisposition, SwarmActionReceipt, SwarmActor, SwarmBackend,
    SwarmCoordinator, SwarmId, SwarmObservation, WorkerActionPlan, WorkerSemanticTarget,
    coordinator::validate_plan,
    domain::{
        CoordinatorInvocationStatus, CoordinatorLoopState, CoordinatorOutcome,
        CoordinatorServiceView, ProgressReviewAuthoritativeStatus, ProgressReviewExecutionStatus,
        ProgressReviewOperationalView, ProviderEffectiveState, ProviderOperationalView,
    },
    execution::{
        ConfigurationResolution, ControlCommand, ControlResult, DaemonError, DesiredExecution,
        ExecutionControlClient, ExecutionPhase, ExecutionSnapshot, InvocationKind,
        InvocationResolution, ProviderKind, ResourcePolicy, SessionSelection,
        SwarmExecutionSnapshot,
    },
};

const JOURNAL_SCHEMA: &str = "worldstream/agent-swarm-coordinator-service@1";
const JOURNAL_DIRECTORY: &str = "coordinator-service";
const JOURNAL_FILE: &str = "service.json";
const JOURNAL_LOCK: &str = ".service.lock";
const MAX_JOURNAL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(30);
const MAX_DISCOVERED_REVIEWS: usize = 64;
const MAX_DISCOVERED_WORK_ITEMS: usize = 64;
const MAX_DISCOVERED_WORK_ATTEMPTS: usize = 64;
const MAX_DISCOVERED_ROSTER: usize = 16;
const PROGRESS_REVIEW_INSTRUCTION_SCHEMA: &str =
    "worldstream/agent-swarm-progress-review-instruction@1";

mod autonomy;
pub use autonomy::{AutonomyPolicy, DeliveryCheckPolicy, DeliveryPolicy};

#[derive(Clone)]
struct DiscoveredReview {
    target: WorkerSemanticTarget,
    authoritative_status: ProgressReviewAuthoritativeStatus,
    required_member_key: Option<String>,
    plan: Option<WorkerActionPlan>,
}

#[derive(Clone)]
struct MemberExecutionProfile {
    swarm_id: SwarmId,
    member_key: String,
    provider: ProviderKind,
    configuration_revision: u64,
    model: String,
    effort: Option<String>,
    moving_alias_acknowledged: bool,
    resource_policy: ResourcePolicy,
    allowed_tools: Vec<String>,
    session: SessionSelection,
}

struct ObservedMemberConfiguration {
    provider: String,
    model: String,
    effort: Option<String>,
    revision: u64,
    moving_alias_acknowledged: bool,
}

impl DiscoveredReview {
    fn review_id(&self) -> &str {
        match &self.target {
            WorkerSemanticTarget::Planning { .. }
            | WorkerSemanticTarget::ControlledFixtureAdmin { .. }
            | WorkerSemanticTarget::WorkProposal { .. }
            | WorkerSemanticTarget::WorkClaim { .. }
            | WorkerSemanticTarget::WorkDependencyRevision { .. }
            | WorkerSemanticTarget::WorkAttempt { .. }
            | WorkerSemanticTarget::LateContribution { .. }
            | WorkerSemanticTarget::SuggestionSubmission { .. }
            | WorkerSemanticTarget::CandidateReview { .. }
            | WorkerSemanticTarget::FindingResolution { .. }
            | WorkerSemanticTarget::CorrectionOutcome { .. } => {
                unreachable!("a discovered Progress Review cannot target ordinary work")
            }
            WorkerSemanticTarget::ProgressReviewClaim { review_id, .. }
            | WorkerSemanticTarget::ProgressReviewReport { review_id, .. } => review_id,
        }
    }

    fn review_revision(&self) -> u64 {
        match &self.target {
            WorkerSemanticTarget::Planning { .. }
            | WorkerSemanticTarget::ControlledFixtureAdmin { .. }
            | WorkerSemanticTarget::WorkProposal { .. }
            | WorkerSemanticTarget::WorkClaim { .. }
            | WorkerSemanticTarget::WorkDependencyRevision { .. }
            | WorkerSemanticTarget::WorkAttempt { .. }
            | WorkerSemanticTarget::LateContribution { .. }
            | WorkerSemanticTarget::SuggestionSubmission { .. }
            | WorkerSemanticTarget::CandidateReview { .. }
            | WorkerSemanticTarget::FindingResolution { .. }
            | WorkerSemanticTarget::CorrectionOutcome { .. } => {
                unreachable!("a discovered Progress Review cannot target ordinary work")
            }
            WorkerSemanticTarget::ProgressReviewClaim {
                review_revision, ..
            }
            | WorkerSemanticTarget::ProgressReviewReport {
                review_revision, ..
            } => *review_revision,
        }
    }

    fn work_id(&self) -> &str {
        match &self.target {
            WorkerSemanticTarget::Planning { .. }
            | WorkerSemanticTarget::ControlledFixtureAdmin { .. }
            | WorkerSemanticTarget::WorkProposal { .. }
            | WorkerSemanticTarget::WorkClaim { .. }
            | WorkerSemanticTarget::WorkDependencyRevision { .. }
            | WorkerSemanticTarget::WorkAttempt { .. }
            | WorkerSemanticTarget::LateContribution { .. }
            | WorkerSemanticTarget::SuggestionSubmission { .. }
            | WorkerSemanticTarget::CandidateReview { .. }
            | WorkerSemanticTarget::FindingResolution { .. }
            | WorkerSemanticTarget::CorrectionOutcome { .. } => {
                unreachable!("a discovered Progress Review cannot target ordinary work")
            }
            WorkerSemanticTarget::ProgressReviewClaim { work_id, .. }
            | WorkerSemanticTarget::ProgressReviewReport { work_id, .. } => work_id,
        }
    }

    fn work_revision(&self) -> u64 {
        match &self.target {
            WorkerSemanticTarget::Planning { .. }
            | WorkerSemanticTarget::ControlledFixtureAdmin { .. }
            | WorkerSemanticTarget::WorkProposal { .. }
            | WorkerSemanticTarget::WorkClaim { .. }
            | WorkerSemanticTarget::WorkDependencyRevision { .. }
            | WorkerSemanticTarget::WorkAttempt { .. }
            | WorkerSemanticTarget::LateContribution { .. }
            | WorkerSemanticTarget::SuggestionSubmission { .. }
            | WorkerSemanticTarget::CandidateReview { .. }
            | WorkerSemanticTarget::FindingResolution { .. }
            | WorkerSemanticTarget::CorrectionOutcome { .. } => {
                unreachable!("a discovered Progress Review cannot target ordinary work")
            }
            WorkerSemanticTarget::ProgressReviewClaim { work_revision, .. }
            | WorkerSemanticTarget::ProgressReviewReport { work_revision, .. } => *work_revision,
        }
    }
}

trait CoordinatorDriver {
    fn submit_delivery_action(
        &mut self,
        _swarm_id: &SwarmId,
        _action: &crate::ExactSwarmAction,
    ) -> Result<SwarmActionReceipt, CoordinatorError> {
        Err(CoordinatorError::InvalidPlan)
    }
    fn stage(&mut self, plan: WorkerActionPlan) -> Result<CoordinatorIntentView, CoordinatorError>;
    fn dispatch(&mut self) -> Result<Vec<CoordinatorEvent>, CoordinatorError>;
    fn harvest(&mut self) -> Result<Vec<CoordinatorEvent>, CoordinatorError>;
    fn retry_action(&mut self, invocation_id: &str) -> Result<CoordinatorEvent, CoordinatorError>;
    fn retry_launch(&mut self, invocation_id: &str) -> Result<CoordinatorEvent, CoordinatorError>;
    fn intent(&self, invocation_id: &str) -> Result<CoordinatorIntentView, CoordinatorError>;
    fn reconcile_interruption(
        &mut self,
        invocation_id: &str,
        resolution: InvocationResolution,
    ) -> Result<CoordinatorEvent, CoordinatorError>;
    fn observe(
        &self,
        swarm_id: &SwarmId,
        actor: &SwarmActor,
    ) -> Result<SwarmObservation, CoordinatorError>;
    fn preflight(&self, plan: &WorkerActionPlan) -> Result<(), CoordinatorError>;
}

impl<B: SwarmBackend> CoordinatorDriver for SwarmCoordinator<B> {
    fn submit_delivery_action(
        &mut self,
        swarm_id: &SwarmId,
        action: &crate::ExactSwarmAction,
    ) -> Result<SwarmActionReceipt, CoordinatorError> {
        Self::submit_delivery_action(self, swarm_id, action)
    }
    fn stage(&mut self, plan: WorkerActionPlan) -> Result<CoordinatorIntentView, CoordinatorError> {
        Self::stage(self, plan)
    }

    fn dispatch(&mut self) -> Result<Vec<CoordinatorEvent>, CoordinatorError> {
        Self::dispatch(self)
    }

    fn harvest(&mut self) -> Result<Vec<CoordinatorEvent>, CoordinatorError> {
        Self::harvest(self)
    }

    fn retry_action(&mut self, invocation_id: &str) -> Result<CoordinatorEvent, CoordinatorError> {
        self.retry_uncertain(invocation_id)
    }

    fn retry_launch(&mut self, invocation_id: &str) -> Result<CoordinatorEvent, CoordinatorError> {
        self.retry_uncertain_launch(invocation_id)
    }

    fn intent(&self, invocation_id: &str) -> Result<CoordinatorIntentView, CoordinatorError> {
        Self::intent(self, invocation_id)
    }

    fn reconcile_interruption(
        &mut self,
        invocation_id: &str,
        resolution: InvocationResolution,
    ) -> Result<CoordinatorEvent, CoordinatorError> {
        Self::reconcile_interruption(self, invocation_id, resolution)
    }

    fn observe(
        &self,
        swarm_id: &SwarmId,
        actor: &SwarmActor,
    ) -> Result<SwarmObservation, CoordinatorError> {
        self.observe_actor(swarm_id, actor)
    }

    fn preflight(&self, plan: &WorkerActionPlan) -> Result<(), CoordinatorError> {
        Self::preflight(self, plan)
    }
}

trait ExecutionStatusSource {
    fn status(&self) -> Result<ExecutionSnapshot, DaemonError>;

    fn retained_completion(
        &self,
        _swarm_id: &SwarmId,
        _invocation_id: &str,
    ) -> Result<bool, DaemonError> {
        Ok(false)
    }

    fn cancel_invocation(
        &self,
        _swarm_id: &SwarmId,
        _invocation_id: &str,
    ) -> Result<ExecutionPhase, DaemonError> {
        Err(DaemonError::InvalidResponse)
    }
}

impl ExecutionStatusSource for ExecutionControlClient {
    fn status(&self) -> Result<ExecutionSnapshot, DaemonError> {
        match self.request(ControlCommand::Status { swarm_id: None })? {
            ControlResult::Status(snapshot) => Ok(snapshot),
            _ => Err(DaemonError::InvalidResponse),
        }
    }

    fn cancel_invocation(
        &self,
        swarm_id: &SwarmId,
        invocation_id: &str,
    ) -> Result<ExecutionPhase, DaemonError> {
        match self.request(ControlCommand::CancelInvocation {
            swarm_id: swarm_id.as_str().to_owned(),
            invocation_id: invocation_id.to_owned(),
        })? {
            ControlResult::Mutation(receipt) => receipt.phase.ok_or(DaemonError::InvalidResponse),
            _ => Err(DaemonError::InvalidResponse),
        }
    }

    fn retained_completion(
        &self,
        swarm_id: &SwarmId,
        invocation_id: &str,
    ) -> Result<bool, DaemonError> {
        match self.request(ControlCommand::Collect {
            invocation_id: invocation_id.to_owned(),
        }) {
            Ok(ControlResult::Completion(result)) => Ok(result.swarm_id == swarm_id.as_str()
                && result.invocation_id == invocation_id
                && result.exit.is_some()
                && matches!(
                    result.resolution,
                    InvocationResolution::Completed | InvocationResolution::Failed
                )),
            Err(DaemonError::Control(crate::execution::ControlFault::NotFound)) => Ok(false),
            Ok(_) => Err(DaemonError::InvalidResponse),
            Err(error) => Err(error),
        }
    }
}

/// Result of one bounded cycle or a continuous run that reached a safe stop.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorRunReceipt {
    pub events: Vec<CoordinatorEvent>,
    pub service: CoordinatorServiceView,
}

/// Single-writer persistent coordinator execution loop.
pub struct CoordinatorService {
    driver: Box<dyn CoordinatorDriver>,
    execution: Box<dyn ExecutionStatusSource>,
    journal: ServiceJournal,
    state: ServiceState,
}

impl CoordinatorService {
    /// Opens retained service state without contacting or resuming the daemon.
    ///
    /// # Errors
    /// Rejects corrupt/unsafe state, a different retained Swarm, or another
    /// live service writer.
    pub fn open<B: SwarmBackend + 'static>(
        root: &Path,
        swarm_id: &SwarmId,
        coordinator: SwarmCoordinator<B>,
        execution: ExecutionControlClient,
    ) -> Result<Self, CoordinatorServiceError> {
        Self::open_with_ports(root, swarm_id, Box::new(coordinator), Box::new(execution))
    }

    fn open_with_ports(
        root: &Path,
        swarm_id: &SwarmId,
        driver: Box<dyn CoordinatorDriver>,
        execution: Box<dyn ExecutionStatusSource>,
    ) -> Result<Self, CoordinatorServiceError> {
        let journal = ServiceJournal::open(root)?;
        let state = journal
            .load()?
            .unwrap_or_else(|| ServiceState::new(swarm_id.clone()));
        validate_state(&state)?;
        if &state.swarm_id != swarm_id {
            return Err(CoordinatorServiceError::SwarmMismatch);
        }
        Ok(Self {
            driver,
            execution,
            journal,
            state,
        })
    }

    /// Retains a plan in the service journal before asking the coordinator to
    /// stage it. A crash between those writes is recovered by a later run.
    ///
    /// # Errors
    /// Rejects a different plan reusing an Invocation identity and propagates
    /// coordinator, daemon, and durability failures.
    pub fn stage(
        &mut self,
        plan: WorkerActionPlan,
    ) -> Result<CoordinatorIntentView, CoordinatorServiceError> {
        validate_plan(&plan)?;
        if plan.swarm_id != self.state.swarm_id || !valid_identifier(&plan.invocation_id) {
            return Err(CoordinatorServiceError::InvalidState);
        }
        if self
            .state
            .plans
            .get(&plan.invocation_id)
            .is_some_and(|retained| retained != &plan)
        {
            return Err(CoordinatorServiceError::Conflict);
        }
        let placeholder = status_from_plan(&plan);
        self.mutate(|state| {
            state
                .plans
                .entry(plan.invocation_id.clone())
                .or_insert_with(|| plan.clone());
            state.known_invocations.insert(plan.invocation_id.clone());
            upsert_status(&mut state.view.invocations, placeholder);
        })?;

        let intent = self.driver.stage(plan)?;
        self.record_intents([intent.clone()])?;
        Ok(intent)
    }

    /// Runs one bounded dispatch cycle and updates the safe service view.
    ///
    /// # Errors
    /// Propagates closed coordinator, daemon, and durability failures.
    pub fn dispatch_once(&mut self) -> Result<Vec<CoordinatorEvent>, CoordinatorServiceError> {
        self.cycle(true, false)
    }

    /// Runs one bounded harvest cycle and updates the safe service view.
    ///
    /// # Errors
    /// Propagates closed coordinator, daemon, and durability failures.
    pub fn harvest_once(&mut self) -> Result<Vec<CoordinatorEvent>, CoordinatorServiceError> {
        self.cycle(false, true)
    }

    /// Runs one bounded restore, dispatch, and harvest cycle.
    ///
    /// # Errors
    /// Propagates closed coordinator, daemon, and durability failures.
    pub fn run_once(&mut self) -> Result<CoordinatorRunReceipt, CoordinatorServiceError> {
        let events = self.cycle(true, true)?;
        Ok(CoordinatorRunReceipt {
            events,
            service: self.state.view.clone(),
        })
    }

    /// Polls until the Swarm is locally idle or an uncertain launch/submission
    /// requires this writer to yield for explicit reconciliation. Paused and
    /// recovery-required execution remains observable while waiting for an
    /// external explicit control. This loop never issues `Resume` or invokes
    /// either uncertain-effect retry operation.
    ///
    /// # Errors
    /// Rejects a zero/oversized poll interval and propagates closed service
    /// failures.
    pub fn run_until_idle(
        &mut self,
        poll_interval: Duration,
    ) -> Result<CoordinatorRunReceipt, CoordinatorServiceError> {
        if poll_interval.is_zero() || poll_interval > MAX_POLL_INTERVAL {
            return Err(CoordinatorServiceError::InvalidPollInterval);
        }
        let mut events = Vec::new();
        loop {
            let receipt = self.run_once()?;
            events.extend(receipt.events);
            match receipt.service.loop_state {
                CoordinatorLoopState::Idle
                    if receipt.service.autonomy.as_ref().is_some_and(|autonomy| {
                        matches!(
                            autonomy.phase.as_str(),
                            "enabled" | "reasoning" | "executing"
                        )
                    }) =>
                {
                    // A just-retained planning decision or worker outcome
                    // still needs its next adaptive cycle before becoming idle.
                    thread::sleep(poll_interval);
                }
                CoordinatorLoopState::Idle | CoordinatorLoopState::ManualReconciliationRequired => {
                    return Ok(CoordinatorRunReceipt {
                        events,
                        service: receipt.service,
                    });
                }
                CoordinatorLoopState::Running
                | CoordinatorLoopState::WaitingForCapacity
                | CoordinatorLoopState::WaitingForExplicitResume
                | CoordinatorLoopState::RecoveryRequired => thread::sleep(poll_interval),
            }
        }
    }

    /// Runs the coordinator as a long-lived service. Idle, capacity waits,
    /// pause, and recovery gates are observations rather than terminal
    /// conditions. Only an uncertain external effect yields to an operator;
    /// this loop never resumes execution or retries that effect implicitly.
    ///
    /// # Errors
    /// Rejects a zero/oversized poll interval and propagates closed service
    /// failures.
    pub fn run_continuously(
        &mut self,
        poll_interval: Duration,
    ) -> Result<CoordinatorRunReceipt, CoordinatorServiceError> {
        if poll_interval.is_zero() || poll_interval > MAX_POLL_INTERVAL {
            return Err(CoordinatorServiceError::InvalidPollInterval);
        }
        let mut events = Vec::new();
        loop {
            let receipt = self.run_once()?;
            events.extend(receipt.events);
            if events.len() > 1_024 {
                events.drain(..events.len() - 1_024);
            }
            if receipt.service.loop_state == CoordinatorLoopState::ManualReconciliationRequired {
                return Ok(CoordinatorRunReceipt {
                    events,
                    service: receipt.service,
                });
            }
            thread::sleep(poll_interval);
        }
    }

    /// Explicitly retries one transport-uncertain retained Action.
    ///
    /// # Errors
    /// Propagates coordinator and durability failures.
    pub fn retry_uncertain_action(
        &mut self,
        invocation_id: &str,
    ) -> Result<CoordinatorEvent, CoordinatorServiceError> {
        let event = self.driver.retry_action(invocation_id)?;
        self.retain_event_identity(&event)?;
        self.refresh_known_intents()?;
        self.refresh_loop_from_local_outcomes()?;
        Ok(event)
    }

    /// Explicitly reconciles one uncertain launch with its retained exact
    /// request.
    ///
    /// # Errors
    /// Propagates coordinator and durability failures.
    pub fn retry_uncertain_launch(
        &mut self,
        invocation_id: &str,
    ) -> Result<CoordinatorEvent, CoordinatorServiceError> {
        let event = self.driver.retry_launch(invocation_id)?;
        self.retain_event_identity(&event)?;
        self.refresh_known_intents()?;
        self.refresh_loop_from_local_outcomes()?;
        Ok(event)
    }

    /// Reads one coordinator intent without changing daemon state.
    ///
    /// # Errors
    /// Returns the coordinator's closed lookup failure.
    pub fn intent(
        &self,
        invocation_id: &str,
    ) -> Result<CoordinatorIntentView, CoordinatorServiceError> {
        self.driver.intent(invocation_id).map_err(Into::into)
    }

    /// Returns the last durably published safe operational view.
    #[must_use]
    pub fn view(&self) -> &CoordinatorServiceView {
        &self.state.view
    }

    fn cycle(
        &mut self,
        dispatch: bool,
        harvest: bool,
    ) -> Result<Vec<CoordinatorEvent>, CoordinatorServiceError> {
        let initial = self.execution.status()?;
        let target = target_snapshot(&initial, &self.state.swarm_id)?.clone();
        self.retain_daemon_identities(&target)?;
        self.refresh_known_intents()?;

        let observation = self.authoritative_observation()?;
        let discovered = self.discover_progress_reviews(&observation)?;
        self.publish_progress_reviews(&discovered, &target)?;
        let (mut events, cancellation_unknown) =
            self.reconcile_requested_interruptions(&observation, &target)?;
        self.refresh_known_intents()?;
        if cancellation_unknown {
            self.set_loop_state(CoordinatorLoopState::RecoveryRequired)?;
            return Ok(events);
        }

        let gate = execution_gate(&target);
        if let Gate::Stop(_) = gate {
            self.set_loop_state(derive_loop_state(&target, &self.state.view.invocations))?;
            return Ok(Vec::new());
        }

        if gate == Gate::Run {
            self.stage_new_progress_review_plans(&discovered)?;
            self.advance_autonomy(&observation)?;
            self.restore_plans(&discovered)?;
            self.refresh_known_intents()?;
            self.publish_progress_reviews(&discovered, &target)?;
        }
        if self.has_blocking_ambiguity() {
            self.set_loop_state(CoordinatorLoopState::ManualReconciliationRequired)?;
            return Ok(Vec::new());
        }

        if dispatch && gate == Gate::Run {
            let dispatched = self.driver.dispatch()?;
            self.retain_event_identities(&dispatched)?;
            events.extend(dispatched);
            self.refresh_known_intents()?;
            if self.has_blocking_ambiguity() {
                self.set_loop_state(CoordinatorLoopState::ManualReconciliationRequired)?;
                return Ok(events);
            }
        }
        if harvest {
            let harvested = self.driver.harvest()?;
            self.retain_event_identities(&harvested)?;
            events.extend(harvested);
            self.refresh_known_intents()?;
        }

        let final_snapshot = self.execution.status()?;
        let target = target_snapshot(&final_snapshot, &self.state.swarm_id)?.clone();
        self.retain_daemon_identities(&target)?;
        self.refresh_known_intents()?;
        let observation = self.authoritative_observation()?;
        let discovered = self.discover_progress_reviews(&observation)?;
        self.publish_progress_reviews(&discovered, &target)?;
        self.set_loop_state(derive_loop_state(&target, &self.state.view.invocations))?;
        Ok(events)
    }

    fn restore_plans(
        &mut self,
        discovered: &[DiscoveredReview],
    ) -> Result<(), CoordinatorServiceError> {
        let eligible_targets = discovered
            .iter()
            .filter_map(|review| review.plan.as_ref()?.semantic_target.as_ref())
            .collect::<Vec<_>>();
        let plans = self
            .state
            .plans
            .values()
            .filter(|plan| {
                plan.semantic_target
                    .as_ref()
                    .is_some_and(|target| match target {
                        WorkerSemanticTarget::Planning { .. }
                        | WorkerSemanticTarget::ControlledFixtureAdmin { .. }
                        | WorkerSemanticTarget::WorkProposal { .. }
                        | WorkerSemanticTarget::WorkClaim { .. }
                        | WorkerSemanticTarget::WorkDependencyRevision { .. }
                        | WorkerSemanticTarget::WorkAttempt { .. }
                        | WorkerSemanticTarget::LateContribution { .. }
                        | WorkerSemanticTarget::SuggestionSubmission { .. }
                        | WorkerSemanticTarget::CandidateReview { .. }
                        | WorkerSemanticTarget::FindingResolution { .. }
                        | WorkerSemanticTarget::CorrectionOutcome { .. } => true,
                        WorkerSemanticTarget::ProgressReviewClaim { .. }
                        | WorkerSemanticTarget::ProgressReviewReport { .. } => {
                            eligible_targets.contains(&target)
                        }
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut intents = Vec::with_capacity(plans.len());
        for plan in plans {
            let unstarted_autonomy_plan = self
                .state
                .autonomy
                .as_ref()
                .is_some_and(|autonomy| autonomy.owns_invocation(&plan.invocation_id))
                && matches!(
                    self.driver.intent(&plan.invocation_id),
                    Err(CoordinatorError::NotFound)
                );
            if unstarted_autonomy_plan
                && self.state.view.invocations.iter().any(|status| {
                    status.invocation_id == plan.invocation_id
                        && !outcome_is_pending(&status.coordinator_outcome)
                })
            {
                // A failed preflight consumed this exact local intent. Only
                // a new retained model decision may create another attempt.
                continue;
            }
            if self.driver.preflight(&plan).is_err() {
                if unstarted_autonomy_plan {
                    self.mutate(|state| {
                        if let Some(status) = state
                            .view
                            .invocations
                            .iter_mut()
                            .find(|status| status.invocation_id == plan.invocation_id)
                        {
                            status.coordinator_outcome = CoordinatorOutcome::NotSubmitted {
                                reason: "preflight_failed".to_owned(),
                            };
                        }
                    })?;
                }
                continue;
            }
            intents.push(self.driver.stage(plan)?);
        }
        self.record_intents(intents)
    }

    fn authoritative_observation(&self) -> Result<SwarmObservation, CoordinatorServiceError> {
        let observation = self
            .driver
            .observe(&self.state.swarm_id, &SwarmActor::HumanCoordinator)?;
        if observation.swarm.swarm_id != self.state.swarm_id
            || observation.actor != SwarmActor::HumanCoordinator
        {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
        Ok(observation)
    }

    fn discover_progress_reviews(
        &self,
        observation: &SwarmObservation,
    ) -> Result<Vec<DiscoveredReview>, CoordinatorServiceError> {
        let authenticated_member_keys = observation
            .activity
            .get("outstanding_progress_reviews")
            .and_then(Value::as_array)
            .is_some_and(|reviews| !reviews.is_empty())
            .then(|| self.authenticated_worker_member_keys(observation))
            .transpose()?
            .unwrap_or_default();
        let mut reviews = parse_progress_reviews(observation, &authenticated_member_keys)?;
        let profiles = member_execution_profiles(observation, &self.state.plans)?;
        for review in &mut reviews {
            if review.authoritative_status == ProgressReviewAuthoritativeStatus::Blocked {
                continue;
            }
            if let Some(existing) = self
                .state
                .plans
                .values()
                .find(|plan| plan.semantic_target.as_ref() == Some(&review.target))
                && self.driver.preflight(existing).is_ok()
            {
                review.plan = Some(existing.clone());
                continue;
            }

            let candidates = profiles
                .values()
                .filter(|profile| {
                    review
                        .required_member_key
                        .as_ref()
                        .is_none_or(|member_key| &profile.member_key == member_key)
                })
                .collect::<Vec<_>>();
            for profile in candidates {
                let plan =
                    progress_review_plan(profile, &review.target, observation.room_seq.max(1))?;
                if self.driver.preflight(&plan).is_ok() {
                    review.plan = Some(plan);
                    break;
                }
            }
        }
        Ok(reviews)
    }

    fn authenticated_worker_member_keys(
        &self,
        observation: &SwarmObservation,
    ) -> Result<BTreeMap<String, String>, CoordinatorServiceError> {
        let mut member_keys = BTreeMap::new();
        let mut requested_member_keys = BTreeSet::new();
        for member in &observation.swarm.roster {
            if !valid_identifier(&member.member_key)
                || !requested_member_keys.insert(member.member_key.as_str())
            {
                return Err(CoordinatorServiceError::InvalidObservation);
            }
            let actor = SwarmActor::Worker {
                member_key: member.member_key.clone(),
            };
            let worker = self.driver.observe(&observation.swarm.swarm_id, &actor)?;
            if worker.swarm.swarm_id != observation.swarm.swarm_id
                || worker.actor != actor
                || !valid_identifier(&worker.member_id)
                || member_keys
                    .insert(worker.member_id, member.member_key.clone())
                    .is_some()
            {
                return Err(CoordinatorServiceError::InvalidObservation);
            }
        }
        if member_keys.len() != observation.swarm.roster.len() {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
        Ok(member_keys)
    }

    fn reconcile_requested_interruptions(
        &mut self,
        observation: &SwarmObservation,
        snapshot: &SwarmExecutionSnapshot,
    ) -> Result<(Vec<CoordinatorEvent>, bool), CoordinatorServiceError> {
        if !snapshot.unknown_effects.is_empty()
            || matches!(
                snapshot.phase,
                ExecutionPhase::BlockedUnknown | ExecutionPhase::RecoveryRequired
            )
        {
            return Ok((Vec::new(), true));
        }
        let needs_member_identity = self.state.plans.values().any(|plan| {
            matches!(
                plan.semantic_target,
                Some(WorkerSemanticTarget::WorkAttempt { .. })
            )
        }) && observation
            .activity
            .get("work_attempts")
            .and_then(Value::as_array)
            .is_some_and(|attempts| {
                attempts.iter().any(|attempt| {
                    attempt.get("status").and_then(Value::as_str) == Some("interruption_requested")
                })
            });
        let authenticated_member_keys = needs_member_identity
            .then(|| self.authenticated_worker_member_keys(observation))
            .transpose()?
            .unwrap_or_default();
        let requested = targeted_interruption_requests(
            &self.state,
            observation,
            snapshot,
            &authenticated_member_keys,
        )?;
        self.mutate(|state| {
            state.pending_targeted_cancellations.extend(requested);
        })?;

        let pending = self
            .state
            .pending_targeted_cancellations
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut events = Vec::with_capacity(pending.len());
        for invocation_id in pending {
            if interruption_is_terminal(&self.driver.intent(&invocation_id)?) {
                self.mutate(|state| {
                    state.pending_targeted_cancellations.remove(&invocation_id);
                })?;
                continue;
            }
            let phase = match self
                .execution
                .cancel_invocation(&self.state.swarm_id, &invocation_id)
            {
                Ok(phase) => phase,
                Err(DaemonError::Control(crate::execution::ControlFault::Conflict))
                    if self
                        .execution
                        .retained_completion(&self.state.swarm_id, &invocation_id)? =>
                {
                    // The process finished between observation and cancellation.
                    // Preserve its evidence and apply the ordinary fresh authority
                    // fence instead of calling a completed process terminated.
                    events.extend(self.harvest_completed_interruption(&invocation_id)?);
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let unknown = matches!(
                phase,
                ExecutionPhase::BlockedUnknown | ExecutionPhase::RecoveryRequired
            );
            let resolution = if unknown {
                InvocationResolution::Unknown
            } else {
                InvocationResolution::Terminated
            };
            let event = self
                .driver
                .reconcile_interruption(&invocation_id, resolution)?;
            events.push(event);
            self.mutate(|state| {
                state.pending_targeted_cancellations.remove(&invocation_id);
            })?;
            if unknown {
                return Ok((events, true));
            }
        }
        Ok((events, false))
    }

    fn harvest_completed_interruption(
        &mut self,
        invocation_id: &str,
    ) -> Result<Vec<CoordinatorEvent>, CoordinatorServiceError> {
        let events = self.driver.harvest()?;
        self.retain_event_identities(&events)?;
        self.refresh_known_intents()?;
        let intent = self.driver.intent(invocation_id)?;
        if !interruption_is_terminal(&intent) {
            return Err(CoordinatorError::Conflict.into());
        }
        self.mutate(|state| {
            state.pending_targeted_cancellations.remove(invocation_id);
        })?;
        Ok(events)
    }

    fn stage_new_progress_review_plans(
        &mut self,
        discovered: &[DiscoveredReview],
    ) -> Result<(), CoordinatorServiceError> {
        for plan in discovered.iter().filter_map(|review| review.plan.clone()) {
            if !self.state.plans.contains_key(&plan.invocation_id) {
                if let Some(autonomy) = &self.state.autonomy {
                    if autonomy.may_generate() {
                        self.retain_autonomy_batch(vec![plan], None)?;
                    }
                } else {
                    self.stage(plan)?;
                }
            }
        }
        Ok(())
    }

    fn publish_progress_reviews(
        &mut self,
        discovered: &[DiscoveredReview],
        snapshot: &SwarmExecutionSnapshot,
    ) -> Result<(), CoordinatorServiceError> {
        let views = discovered
            .iter()
            .map(|review| {
                let retained = review
                    .plan
                    .as_ref()
                    .filter(|plan| self.state.plans.contains_key(&plan.invocation_id));
                let status = progress_review_execution_status(
                    review,
                    retained,
                    snapshot,
                    &self.state.view.invocations,
                );
                ProgressReviewOperationalView {
                    review_id: review.review_id().to_owned(),
                    review_revision: review.review_revision(),
                    work_id: review.work_id().to_owned(),
                    work_revision: review.work_revision(),
                    authoritative_status: review.authoritative_status,
                    execution_status: status,
                    member_key: retained
                        .map(|plan| plan.member_key.clone())
                        .or_else(|| review.required_member_key.clone()),
                    invocation_id: retained.map(|plan| plan.invocation_id.clone()),
                }
            })
            .collect::<Vec<_>>();
        self.mutate(|state| state.view.progress_reviews = views)
    }

    fn retain_daemon_identities(
        &mut self,
        snapshot: &SwarmExecutionSnapshot,
    ) -> Result<(), CoordinatorServiceError> {
        let identities = snapshot
            .queued
            .iter()
            .map(|ticket| ticket.invocation_id.clone())
            .chain(
                snapshot
                    .active
                    .iter()
                    .map(|active| active.ticket.invocation_id.clone()),
            )
            .collect::<Vec<_>>();
        self.mutate(|state| state.known_invocations.extend(identities))
    }

    fn retain_event_identities(
        &mut self,
        events: &[CoordinatorEvent],
    ) -> Result<(), CoordinatorServiceError> {
        self.mutate(|state| {
            state
                .known_invocations
                .extend(events.iter().map(event_invocation_id).map(str::to_owned));
        })
    }

    fn retain_event_identity(
        &mut self,
        event: &CoordinatorEvent,
    ) -> Result<(), CoordinatorServiceError> {
        self.mutate(|state| {
            state
                .known_invocations
                .insert(event_invocation_id(event).to_owned());
        })
    }

    fn refresh_known_intents(&mut self) -> Result<(), CoordinatorServiceError> {
        let identities = self
            .state
            .known_invocations
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut intents = Vec::with_capacity(identities.len());
        for invocation_id in identities {
            match self.driver.intent(&invocation_id) {
                Ok(intent) => intents.push(intent),
                Err(CoordinatorError::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }
        self.record_intents(intents)
    }

    fn record_intents(
        &mut self,
        intents: impl IntoIterator<Item = CoordinatorIntentView>,
    ) -> Result<(), CoordinatorServiceError> {
        let statuses = intents
            .into_iter()
            .map(|intent| (intent.invocation_id.clone(), status_from_intent(&intent)))
            .collect::<Vec<_>>();
        self.mutate(|state| {
            for (invocation_id, status) in statuses {
                state.known_invocations.insert(invocation_id);
                upsert_status(&mut state.view.invocations, status);
            }
        })
    }

    fn refresh_loop_from_local_outcomes(&mut self) -> Result<(), CoordinatorServiceError> {
        let state = if self.has_blocking_ambiguity() {
            CoordinatorLoopState::ManualReconciliationRequired
        } else if self
            .state
            .view
            .invocations
            .iter()
            .any(|status| outcome_is_pending(&status.coordinator_outcome))
        {
            CoordinatorLoopState::Running
        } else {
            CoordinatorLoopState::Idle
        };
        self.set_loop_state(state)
    }

    fn set_loop_state(
        &mut self,
        loop_state: CoordinatorLoopState,
    ) -> Result<(), CoordinatorServiceError> {
        self.mutate(|state| state.view.loop_state = loop_state)
    }

    fn has_blocking_ambiguity(&self) -> bool {
        self.delivery_has_uncertain_effects()
            || self
                .state
                .view
                .invocations
                .iter()
                .any(|status| outcome_blocks_automatic_progress(&status.coordinator_outcome))
    }

    fn mutate(
        &mut self,
        mutate: impl FnOnce(&mut ServiceState),
    ) -> Result<(), CoordinatorServiceError> {
        let mut next = self.state.clone();
        mutate(&mut next);
        refresh_counts(&mut next.view);
        next.view
            .invocations
            .sort_by(|left, right| left.invocation_id.cmp(&right.invocation_id));
        if next == self.state {
            return Ok(());
        }
        next.revision = next.revision.saturating_add(1);
        next.view.revision = next.revision;
        validate_state(&next)?;
        self.journal.store(&next)?;
        self.state = next;
        Ok(())
    }
}

fn interruption_is_terminal(intent: &CoordinatorIntentView) -> bool {
    matches!(
        intent.state,
        CoordinatorIntentState::Settled | CoordinatorIntentState::SubmissionUncertain
    ) || (intent.state == CoordinatorIntentState::NeedsReevaluation
        && intent.evidence.as_ref().is_some_and(|evidence| {
            matches!(
                evidence.resolution,
                InvocationResolution::Completed | InvocationResolution::Failed
            )
        }))
}

/// Reads the latest atomically published coordinator service view without
/// acquiring its writer lease or contacting the daemon.
///
/// # Errors
/// Rejects unsafe/corrupt state or a state root belonging to another Swarm.
pub fn read_coordinator_service_view(
    root: &Path,
    expected_swarm_id: &SwarmId,
) -> Result<Option<CoordinatorServiceView>, CoordinatorServiceError> {
    let service_root = root.join(JOURNAL_DIRECTORY);
    match fs::metadata(&service_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CoordinatorServiceError::StateUnavailable),
        Ok(_) => {}
    }
    let service_root = validate_data_directory(&service_root)
        .map_err(|_| CoordinatorServiceError::UnsafeStorage)?;
    let Some(state) = load_state(&service_root.join(JOURNAL_FILE))? else {
        return Ok(None);
    };
    validate_state(&state)?;
    if &state.swarm_id != expected_swarm_id {
        return Err(CoordinatorServiceError::SwarmMismatch);
    }
    Ok(Some(state.view))
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ServiceState {
    revision: u64,
    swarm_id: SwarmId,
    plans: BTreeMap<String, WorkerActionPlan>,
    known_invocations: BTreeSet<String>,
    view: CoordinatorServiceView,
    /// Durable prepare-before-effect fence for exact targeted cancellation.
    /// Omitted when empty so existing @1 journal digests remain valid.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pending_targeted_cancellations: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    autonomy: Option<autonomy::AutonomyState>,
}

impl ServiceState {
    fn new(swarm_id: SwarmId) -> Self {
        Self {
            revision: 0,
            swarm_id: swarm_id.clone(),
            plans: BTreeMap::new(),
            known_invocations: BTreeSet::new(),
            view: CoordinatorServiceView {
                swarm_id,
                revision: 0,
                loop_state: CoordinatorLoopState::Idle,
                pending_count: 0,
                manual_attention_count: 0,
                progress_reviews: Vec::new(),
                invocations: Vec::new(),
                autonomy: None,
            },
            pending_targeted_cancellations: BTreeSet::new(),
            autonomy: None,
        }
    }
}

struct ServiceJournal {
    root: PathBuf,
    _process_lock: fs::File,
}

impl ServiceJournal {
    fn open(root: &Path) -> Result<Self, CoordinatorServiceError> {
        let root = prepare_data_directory(&root.join(JOURNAL_DIRECTORY))
            .map_err(|_| CoordinatorServiceError::UnsafeStorage)?;
        let lock_path = root.join(JOURNAL_LOCK);
        let process_lock = match create_owner_only_file(&lock_path) {
            Ok(file) => file,
            Err(_) if lock_path.exists() => fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(|_| CoordinatorServiceError::StateUnavailable)?,
            Err(_) => return Err(CoordinatorServiceError::StateUnavailable),
        };
        validate_owner_only_file(&lock_path).map_err(|_| CoordinatorServiceError::UnsafeStorage)?;
        process_lock
            .try_lock()
            .map_err(|_| CoordinatorServiceError::WriterActive)?;
        Ok(Self {
            root,
            _process_lock: process_lock,
        })
    }

    fn path(&self) -> PathBuf {
        self.root.join(JOURNAL_FILE)
    }

    fn load(&self) -> Result<Option<ServiceState>, CoordinatorServiceError> {
        load_state(&self.path())
    }

    fn store(&self, state: &ServiceState) -> Result<(), CoordinatorServiceError> {
        let envelope = ServiceEnvelope {
            schema: JOURNAL_SCHEMA.to_owned(),
            digest: state_digest(state)?,
            state: state.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&envelope)
            .map_err(|_| CoordinatorServiceError::InvalidState)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_JOURNAL_BYTES {
            return Err(CoordinatorServiceError::InvalidState);
        }
        let mut random = [0_u8; 12];
        getrandom::fill(&mut random).map_err(|_| CoordinatorServiceError::StateUnavailable)?;
        let suffix = random.iter().fold(String::new(), |mut value, byte| {
            let _ = write!(value, "{byte:02x}");
            value
        });
        let staged = self.root.join(format!(".{suffix}.service.tmp"));
        let mut file = create_owner_only_renameable_file(&staged)
            .map_err(|_| CoordinatorServiceError::StateUnavailable)?;
        let result = (|| {
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| CoordinatorServiceError::StateUnavailable)?;
            drop(file);
            let target = self.path();
            if target.exists() {
                atomicwrites::replace_atomic(&staged, &target)
            } else {
                atomicwrites::move_atomic(&staged, &target)
            }
            .map_err(|_| CoordinatorServiceError::StateUnavailable)
        })();
        let _ = fs::remove_file(staged);
        result
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ServiceEnvelope {
    schema: String,
    state: ServiceState,
    digest: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Gate {
    Run,
    Drain,
    Stop(CoordinatorLoopState),
}

fn execution_gate(snapshot: &SwarmExecutionSnapshot) -> Gate {
    if !snapshot.unknown_effects.is_empty()
        || matches!(
            snapshot.phase,
            ExecutionPhase::RecoveryRequired | ExecutionPhase::BlockedUnknown
        )
    {
        return Gate::Stop(CoordinatorLoopState::RecoveryRequired);
    }
    if snapshot.desired == DesiredExecution::Running && snapshot.phase == ExecutionPhase::Running {
        return Gate::Run;
    }
    if !snapshot.active.is_empty() {
        return Gate::Drain;
    }
    Gate::Stop(CoordinatorLoopState::WaitingForExplicitResume)
}

fn derive_loop_state(
    snapshot: &SwarmExecutionSnapshot,
    statuses: &[CoordinatorInvocationStatus],
) -> CoordinatorLoopState {
    if !snapshot.unknown_effects.is_empty()
        || matches!(
            snapshot.phase,
            ExecutionPhase::RecoveryRequired | ExecutionPhase::BlockedUnknown
        )
    {
        return CoordinatorLoopState::RecoveryRequired;
    }
    if statuses
        .iter()
        .any(|status| outcome_blocks_automatic_progress(&status.coordinator_outcome))
    {
        return CoordinatorLoopState::ManualReconciliationRequired;
    }
    if !snapshot.active.is_empty() {
        return CoordinatorLoopState::Running;
    }
    let pending = statuses
        .iter()
        .any(|status| outcome_is_pending(&status.coordinator_outcome));
    if snapshot.desired != DesiredExecution::Running || snapshot.phase != ExecutionPhase::Running {
        return if snapshot.queued.is_empty() && !pending {
            CoordinatorLoopState::Idle
        } else {
            CoordinatorLoopState::WaitingForExplicitResume
        };
    }
    if !snapshot.queued.is_empty() || pending {
        CoordinatorLoopState::WaitingForCapacity
    } else {
        CoordinatorLoopState::Idle
    }
}

fn target_snapshot<'a>(
    snapshot: &'a ExecutionSnapshot,
    swarm_id: &SwarmId,
) -> Result<&'a SwarmExecutionSnapshot, CoordinatorServiceError> {
    snapshot
        .swarms
        .iter()
        .find(|swarm| swarm.swarm_id == swarm_id.as_str())
        .ok_or(CoordinatorServiceError::SwarmNotRegistered)
}

fn status_from_plan(plan: &WorkerActionPlan) -> CoordinatorInvocationStatus {
    CoordinatorInvocationStatus {
        invocation_id: plan.invocation_id.clone(),
        member_key: plan.member_key.clone(),
        provider: plan.provider.as_str().to_owned(),
        configuration_revision: plan.configuration_revision,
        requested_model: plan.model.clone(),
        requested_effort: plan.effort.clone(),
        provider_evidence: ProviderOperationalView {
            state: ProviderEffectiveState::AwaitingReport,
            effective_model: None,
            effective_effort: None,
            reported_model: None,
            reported_effort: None,
            reported_session_id: None,
        },
        coordinator_outcome: CoordinatorOutcome::Pending,
    }
}

fn status_from_intent(intent: &CoordinatorIntentView) -> CoordinatorInvocationStatus {
    let output = intent
        .evidence
        .as_ref()
        .and_then(|evidence| evidence.output.as_ref());
    let configuration = intent
        .evidence
        .as_ref()
        .and_then(|evidence| evidence.configuration.as_ref());
    let (state, effective_model, effective_effort, mismatch_model, mismatch_effort) =
        match configuration {
            Some(ConfigurationResolution::Verified { model, effort }) => (
                ProviderEffectiveState::Verified,
                Some(model.clone()),
                effort.clone(),
                None,
                None,
            ),
            Some(ConfigurationResolution::Unreported) => {
                (ProviderEffectiveState::Unreported, None, None, None, None)
            }
            Some(ConfigurationResolution::Mismatch {
                reported_model,
                reported_effort,
            }) => (
                ProviderEffectiveState::Mismatch,
                None,
                None,
                reported_model.clone(),
                reported_effort.clone(),
            ),
            Some(ConfigurationResolution::Unsupported) => {
                (ProviderEffectiveState::Unsupported, None, None, None, None)
            }
            None if output.is_some() => (ProviderEffectiveState::Mismatch, None, None, None, None),
            None if intent.evidence.is_some() => {
                (ProviderEffectiveState::Unreported, None, None, None, None)
            }
            Some(ConfigurationResolution::PendingReport) | None => (
                ProviderEffectiveState::AwaitingReport,
                None,
                None,
                None,
                None,
            ),
        };
    CoordinatorInvocationStatus {
        invocation_id: intent.invocation_id.clone(),
        member_key: intent.member_key.clone(),
        provider: intent.provider.as_str().to_owned(),
        configuration_revision: intent.configuration_revision,
        requested_model: intent.requested_model.clone(),
        requested_effort: intent.requested_effort.clone(),
        provider_evidence: ProviderOperationalView {
            state,
            effective_model,
            effective_effort,
            reported_model: output
                .map(|output| output.reported_model.clone())
                .or(mismatch_model),
            reported_effort: output
                .and_then(|output| output.reported_effort.clone())
                .or(mismatch_effort),
            reported_session_id: output.and_then(|output| output.session_id.clone()),
        },
        coordinator_outcome: outcome_from_intent(intent),
    }
}

fn outcome_from_intent(intent: &CoordinatorIntentView) -> CoordinatorOutcome {
    match intent.state {
        CoordinatorIntentState::Staged | CoordinatorIntentState::Queued => {
            CoordinatorOutcome::Pending
        }
        CoordinatorIntentState::LaunchPrepared => CoordinatorOutcome::LaunchUncertain,
        CoordinatorIntentState::Running => CoordinatorOutcome::Running,
        CoordinatorIntentState::ProposalRetained | CoordinatorIntentState::AwaitingSubmission => {
            CoordinatorOutcome::ProcessingResult
        }
        CoordinatorIntentState::SubmissionUncertain => CoordinatorOutcome::SubmissionUncertain,
        CoordinatorIntentState::NeedsReevaluation => CoordinatorOutcome::NeedsReevaluation,
        CoordinatorIntentState::Settled => match &intent.submission {
            SubmissionDisposition::Received(SwarmActionReceipt::Accepted { duplicate, .. }) => {
                CoordinatorOutcome::Accepted {
                    duplicate: *duplicate,
                }
            }
            SubmissionDisposition::Received(SwarmActionReceipt::Rejected {
                code,
                duplicate,
                ..
            }) => CoordinatorOutcome::Rejected {
                code: code.clone(),
                duplicate: *duplicate,
            },
            SubmissionDisposition::NotSubmitted(reason) => CoordinatorOutcome::NotSubmitted {
                reason: non_submission_label(*reason).to_owned(),
            },
            SubmissionDisposition::Uncertain => CoordinatorOutcome::SubmissionUncertain,
            SubmissionDisposition::NotAttempted => CoordinatorOutcome::SettledWithoutSubmission,
            SubmissionDisposition::PlanningDecision(_) => {
                CoordinatorOutcome::PlanningDecisionRecorded
            }
        },
    }
}

const fn non_submission_label(reason: NonSubmissionReason) -> &'static str {
    match reason {
        NonSubmissionReason::ProcessFailed => "process_failed",
        NonSubmissionReason::ProviderOutputInvalid => "provider_output_invalid",
        NonSubmissionReason::ArtifactInvalid => "artifact_invalid",
        NonSubmissionReason::ProviderConfigurationMismatch => "provider_configuration_mismatch",
        NonSubmissionReason::ConfigurationSuperseded => "configuration_superseded",
        NonSubmissionReason::AuthorityChanged => "authority_changed",
        NonSubmissionReason::SubmissionUnavailable => "submission_unavailable",
    }
}

const fn outcome_requires_manual_attention(outcome: &CoordinatorOutcome) -> bool {
    matches!(
        outcome,
        CoordinatorOutcome::LaunchUncertain
            | CoordinatorOutcome::NeedsReevaluation
            | CoordinatorOutcome::SubmissionUncertain
    )
}

const fn outcome_blocks_automatic_progress(outcome: &CoordinatorOutcome) -> bool {
    matches!(
        outcome,
        CoordinatorOutcome::LaunchUncertain | CoordinatorOutcome::SubmissionUncertain
    )
}

const fn outcome_is_pending(outcome: &CoordinatorOutcome) -> bool {
    matches!(
        outcome,
        CoordinatorOutcome::Pending
            | CoordinatorOutcome::Running
            | CoordinatorOutcome::ProcessingResult
    )
}

fn refresh_counts(view: &mut CoordinatorServiceView) {
    view.pending_count = view
        .invocations
        .iter()
        .filter(|status| outcome_is_pending(&status.coordinator_outcome))
        .count();
    view.manual_attention_count = view
        .invocations
        .iter()
        .filter(|status| outcome_requires_manual_attention(&status.coordinator_outcome))
        .count();
}

fn upsert_status(
    statuses: &mut Vec<CoordinatorInvocationStatus>,
    status: CoordinatorInvocationStatus,
) {
    if let Some(existing) = statuses
        .iter_mut()
        .find(|existing| existing.invocation_id == status.invocation_id)
    {
        *existing = status;
    } else {
        statuses.push(status);
    }
}

fn event_invocation_id(event: &CoordinatorEvent) -> &str {
    match event {
        CoordinatorEvent::Launched { invocation_id }
        | CoordinatorEvent::LaunchOutcomeUncertain { invocation_id }
        | CoordinatorEvent::NeedsReevaluation { invocation_id }
        | CoordinatorEvent::Accepted { invocation_id, .. }
        | CoordinatorEvent::Rejected { invocation_id, .. }
        | CoordinatorEvent::SubmissionUncertain { invocation_id }
        | CoordinatorEvent::NotSubmitted { invocation_id, .. }
        | CoordinatorEvent::CompletionAcknowledged { invocation_id } => invocation_id,
    }
}

fn targeted_interruption_requests(
    state: &ServiceState,
    observation: &SwarmObservation,
    snapshot: &SwarmExecutionSnapshot,
    authenticated_member_keys: &BTreeMap<String, String>,
) -> Result<Vec<String>, CoordinatorServiceError> {
    if !state.plans.values().any(|plan| {
        matches!(
            plan.semantic_target,
            Some(WorkerSemanticTarget::WorkAttempt { .. })
        )
    }) {
        return Ok(Vec::new());
    }
    let activity = observation
        .activity
        .as_object()
        .ok_or(CoordinatorServiceError::InvalidObservation)?;
    let phase = activity
        .get("phase")
        .and_then(Value::as_str)
        .ok_or(CoordinatorServiceError::InvalidObservation)?;
    if !matches!(phase, "open" | "completed") {
        return Ok(Vec::new());
    }
    let execution_epoch = required_revision(&observation.activity, "execution_epoch")?;
    let attempts = bounded_array(activity.get("work_attempts"), MAX_DISCOVERED_WORK_ATTEMPTS)?;

    let attempts_by_id = interruption_attempts_by_id(attempts)?;

    let mut requested = Vec::new();
    for plan in state.plans.values() {
        let Some(WorkerSemanticTarget::WorkAttempt {
            execution_epoch: target_epoch,
            work_id,
            attempt_id,
            attempt_revision,
            ..
        }) = plan.semantic_target.as_ref()
        else {
            continue;
        };
        if *target_epoch != execution_epoch {
            continue;
        }
        let Some(attempt) = attempts_by_id.get(attempt_id.as_str()).copied() else {
            continue;
        };
        if required_revision(attempt, "execution_epoch")? != *target_epoch
            || required_identifier(attempt, "work_id")? != work_id
        {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
        if attempt.get("status").and_then(Value::as_str) != Some("interruption_requested") {
            continue;
        }
        if required_revision(attempt, "revision")? <= *attempt_revision {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
        let member_id = authenticated_member_keys
            .iter()
            .find_map(|(member_id, member_key)| {
                (member_key == &plan.member_key).then_some(member_id.as_str())
            })
            .ok_or(CoordinatorServiceError::InvalidObservation)?;
        if required_identifier(attempt, "owner_member_id")? != member_id {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
        let active = snapshot
            .active
            .iter()
            .filter(|active| active.ticket.invocation_id == plan.invocation_id)
            .collect::<Vec<_>>();
        if active.len() > 1 {
            return Err(CoordinatorServiceError::InvalidState);
        }
        let Some(active) = active.first().copied() else {
            continue;
        };
        if active.resolution != InvocationResolution::Running
            || active.ticket.swarm_id != plan.swarm_id.as_str()
            || active.ticket.member_id != member_id
            || active.ticket.provider != plan.provider
            || active.ticket.configuration_revision != plan.configuration_revision
            || active.ticket.kind != InvocationKind::Work
            || active.ticket.due_sequence != plan.due_sequence
        {
            return Err(CoordinatorServiceError::InvalidState);
        }
        let outcome = state
            .view
            .invocations
            .iter()
            .find(|status| status.invocation_id == plan.invocation_id)
            .map(|status| &status.coordinator_outcome);
        if !matches!(
            outcome,
            Some(CoordinatorOutcome::Running | CoordinatorOutcome::LaunchUncertain)
        ) {
            continue;
        }
        requested.push(plan.invocation_id.clone());
    }
    Ok(requested)
}

fn interruption_attempts_by_id(
    attempts: &[Value],
) -> Result<BTreeMap<&str, &Value>, CoordinatorServiceError> {
    let mut attempts_by_id = BTreeMap::new();
    for attempt in attempts {
        let attempt_id = required_identifier(attempt, "attempt_id")?;
        let _ = required_revision(attempt, "revision")?;
        let _ = required_revision(attempt, "execution_epoch")?;
        let _ = required_identifier(attempt, "work_id")?;
        let _ = required_identifier(attempt, "owner_member_id")?;
        if !matches!(
            attempt.get("status").and_then(Value::as_str),
            Some("active" | "blocked" | "completed" | "interruption_requested" | "fenced")
        ) || attempts_by_id.insert(attempt_id, attempt).is_some()
        {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
    }
    Ok(attempts_by_id)
}

fn parse_progress_reviews(
    observation: &SwarmObservation,
    authenticated_member_keys: &BTreeMap<String, String>,
) -> Result<Vec<DiscoveredReview>, CoordinatorServiceError> {
    let activity = observation
        .activity
        .as_object()
        .ok_or(CoordinatorServiceError::InvalidObservation)?;
    let phase = activity
        .get("phase")
        .and_then(Value::as_str)
        .ok_or(CoordinatorServiceError::InvalidObservation)?;
    if phase != "open" {
        return match phase {
            "awaiting_human_confirmation" | "completed" => Ok(Vec::new()),
            _ => Err(CoordinatorServiceError::InvalidObservation),
        };
    }
    if observation.room_seq == 0 {
        return Err(CoordinatorServiceError::InvalidObservation);
    }
    let execution_epoch = activity
        .get("execution_epoch")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or(CoordinatorServiceError::InvalidObservation)?;
    let reviews = bounded_array(
        activity.get("outstanding_progress_reviews"),
        MAX_DISCOVERED_REVIEWS,
    )?;
    let work_items = bounded_array(activity.get("work_items"), MAX_DISCOVERED_WORK_ITEMS)?;
    let work_attempts = bounded_array(activity.get("work_attempts"), MAX_DISCOVERED_WORK_ATTEMPTS)?;
    let roster = bounded_array(activity.get("roster"), MAX_DISCOVERED_ROSTER)?;

    let mut seen_member_keys = BTreeSet::new();
    let mut projected_member_ids = BTreeSet::new();
    for entry in roster {
        let member_key = required_identifier(entry, "member_key")?;
        if !seen_member_keys.insert(member_key.to_owned()) {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
        if let Some(projected_member_id) = entry.get("member_id") {
            let projected_member_id = projected_member_id
                .as_str()
                .filter(|member_id| valid_identifier(member_id))
                .ok_or(CoordinatorServiceError::InvalidObservation)?;
            if !projected_member_ids.insert(projected_member_id)
                || (!authenticated_member_keys.is_empty()
                    && authenticated_member_keys
                        .get(projected_member_id)
                        .map(String::as_str)
                        != Some(member_key))
            {
                return Err(CoordinatorServiceError::InvalidObservation);
            }
        }
    }
    if !authenticated_member_keys.is_empty()
        && (authenticated_member_keys.len() != seen_member_keys.len()
            || authenticated_member_keys
                .values()
                .any(|member_key| !seen_member_keys.contains(member_key)))
    {
        return Err(CoordinatorServiceError::InvalidObservation);
    }

    let mut indexed_work = BTreeMap::new();
    for work in work_items {
        let work_id = required_identifier(work, "work_id")?;
        if indexed_work.insert(work_id, work).is_some() {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
    }
    let mut indexed_attempts = BTreeMap::new();
    for attempt in work_attempts {
        let attempt_id = required_identifier(attempt, "attempt_id")?;
        if indexed_attempts.insert(attempt_id, attempt).is_some() {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
    }

    let mut seen_reviews = BTreeSet::new();
    let mut discovered = Vec::with_capacity(reviews.len());
    for review in reviews {
        let review_id = required_identifier(review, "review_id")?;
        if !seen_reviews.insert(review_id) {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
        discovered.push(parse_progress_review(
            review,
            execution_epoch,
            &indexed_work,
            &indexed_attempts,
            authenticated_member_keys,
        )?);
    }
    Ok(discovered)
}

fn parse_progress_review(
    review: &Value,
    execution_epoch: u64,
    indexed_work: &BTreeMap<&str, &Value>,
    indexed_attempts: &BTreeMap<&str, &Value>,
    member_keys: &BTreeMap<String, String>,
) -> Result<DiscoveredReview, CoordinatorServiceError> {
    let review_id = required_identifier(review, "review_id")?;
    let review_revision = required_revision(review, "revision")?;
    if required_revision(review, "execution_epoch")? != execution_epoch {
        return Err(CoordinatorServiceError::InvalidObservation);
    }
    let work_id = required_identifier(review, "work_id")?;
    let work = indexed_work
        .get(work_id)
        .copied()
        .ok_or(CoordinatorServiceError::InvalidObservation)?;
    let work_revision = required_revision(work, "revision")?;
    if required_revision(work, "execution_epoch")? != execution_epoch
        || work.get("kind").and_then(Value::as_str) != Some("progress_review")
    {
        return Err(CoordinatorServiceError::InvalidObservation);
    }
    let (authoritative_status, required_member_key, target) =
        match review.get("status").and_then(Value::as_str) {
            Some("due")
                if work.get("status").and_then(Value::as_str) == Some("open")
                    && work.get("owner_member_id").is_none_or(Value::is_null)
                    && work.get("active_attempt_id").is_none_or(Value::is_null) =>
            {
                (
                    ProgressReviewAuthoritativeStatus::Due,
                    None,
                    WorkerSemanticTarget::ProgressReviewClaim {
                        execution_epoch,
                        review_id: review_id.to_owned(),
                        review_revision,
                        work_id: work_id.to_owned(),
                        work_revision,
                    },
                )
            }
            Some("claimed") if work.get("status").and_then(Value::as_str) == Some("claimed") => {
                let owner = required_identifier(work, "owner_member_id")?;
                validate_active_attempt(indexed_attempts, work, work_id, owner, execution_epoch)?;
                let member_key = member_keys
                    .get(owner)
                    .cloned()
                    .ok_or(CoordinatorServiceError::InvalidObservation)?;
                (
                    ProgressReviewAuthoritativeStatus::Claimed,
                    Some(member_key),
                    WorkerSemanticTarget::ProgressReviewReport {
                        execution_epoch,
                        review_id: review_id.to_owned(),
                        review_revision,
                        work_id: work_id.to_owned(),
                        work_revision,
                    },
                )
            }
            Some("blocked") if work.get("status").and_then(Value::as_str) == Some("blocked") => (
                ProgressReviewAuthoritativeStatus::Blocked,
                None,
                WorkerSemanticTarget::ProgressReviewReport {
                    execution_epoch,
                    review_id: review_id.to_owned(),
                    review_revision,
                    work_id: work_id.to_owned(),
                    work_revision,
                },
            ),
            _ => return Err(CoordinatorServiceError::InvalidObservation),
        };
    Ok(DiscoveredReview {
        target,
        authoritative_status,
        required_member_key,
        plan: None,
    })
}

fn validate_active_attempt(
    indexed_attempts: &BTreeMap<&str, &Value>,
    work: &Value,
    work_id: &str,
    owner: &str,
    execution_epoch: u64,
) -> Result<(), CoordinatorServiceError> {
    let attempt_id = required_identifier(work, "active_attempt_id")?;
    let attempt = indexed_attempts
        .get(attempt_id)
        .copied()
        .ok_or(CoordinatorServiceError::InvalidObservation)?;
    if required_revision(attempt, "execution_epoch")? != execution_epoch
        || required_identifier(attempt, "work_id")? != work_id
        || required_identifier(attempt, "owner_member_id")? != owner
        || attempt.get("status").and_then(Value::as_str) != Some("active")
    {
        return Err(CoordinatorServiceError::InvalidObservation);
    }
    Ok(())
}

fn bounded_array(
    value: Option<&Value>,
    maximum: usize,
) -> Result<&[Value], CoordinatorServiceError> {
    value
        .and_then(Value::as_array)
        .filter(|values| values.len() <= maximum)
        .map(Vec::as_slice)
        .ok_or(CoordinatorServiceError::InvalidObservation)
}

fn required_identifier<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a str, CoordinatorServiceError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|identifier| valid_identifier(identifier))
        .ok_or(CoordinatorServiceError::InvalidObservation)
}

fn required_bounded_text<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a str, CoordinatorServiceError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| bounded_text(value))
        .ok_or(CoordinatorServiceError::InvalidObservation)
}

fn bounded_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.contains('\0')
}

fn required_revision(value: &Value, field: &str) -> Result<u64, CoordinatorServiceError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .filter(|revision| *revision > 0)
        .ok_or(CoordinatorServiceError::InvalidObservation)
}

fn retained_member_profiles(
    plans: &BTreeMap<String, WorkerActionPlan>,
) -> BTreeMap<String, WorkerActionPlan> {
    let mut profiles = BTreeMap::new();
    for plan in plans.values() {
        let replace = profiles
            .get(&plan.member_key)
            .is_none_or(|current: &WorkerActionPlan| {
                current.configuration_revision < plan.configuration_revision
            });
        if replace {
            profiles.insert(plan.member_key.clone(), plan.clone());
        }
    }
    profiles
}

fn member_execution_profiles(
    observation: &SwarmObservation,
    plans: &BTreeMap<String, WorkerActionPlan>,
) -> Result<BTreeMap<String, MemberExecutionProfile>, CoordinatorServiceError> {
    if observation.swarm.roster.is_empty() || observation.swarm.roster.len() > MAX_DISCOVERED_ROSTER
    {
        return Err(CoordinatorServiceError::InvalidObservation);
    }
    let authoritative = observed_member_configurations(&observation.activity)?;

    let retained = retained_member_profiles(plans);
    let mut profiles = BTreeMap::new();
    for member in &observation.swarm.roster {
        let Some(authority) = authoritative.get(member.member_key.as_str()) else {
            return Err(CoordinatorServiceError::InvalidObservation);
        };
        if authority.provider != member.provider
            || authority.model != member.requested_model
            || authority.effort != member.requested_effort
            || authority.revision != member.configuration_revision
            || authority.moving_alias_acknowledged != member.moving_alias_acknowledged
        {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
        let provider = ProviderKind::from_roster_name(&member.provider)
            .ok_or(CoordinatorServiceError::InvalidObservation)?;
        let exact_retained = retained.get(&member.member_key).filter(|plan| {
            plan.provider == provider
                && plan.configuration_revision == member.configuration_revision
                && plan.model == member.requested_model
                && plan.effort == member.requested_effort
                && plan.moving_alias_acknowledged == member.moving_alias_acknowledged
        });
        let (resource_policy, allowed_tools, session) = exact_retained.map_or_else(
            || {
                (
                    ResourcePolicy::ReadOnly,
                    Vec::new(),
                    SessionSelection::Fresh { requested_id: None },
                )
            },
            |plan| {
                (
                    plan.resource_policy,
                    plan.allowed_tools.clone(),
                    plan.session.clone(),
                )
            },
        );
        if profiles
            .insert(
                member.member_key.clone(),
                MemberExecutionProfile {
                    swarm_id: observation.swarm.swarm_id.clone(),
                    member_key: member.member_key.clone(),
                    provider,
                    configuration_revision: member.configuration_revision,
                    model: member.requested_model.clone(),
                    effort: member.requested_effort.clone(),
                    moving_alias_acknowledged: member.moving_alias_acknowledged,
                    resource_policy,
                    allowed_tools,
                    session,
                },
            )
            .is_some()
        {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
    }
    if profiles.len() != authoritative.len() {
        return Err(CoordinatorServiceError::InvalidObservation);
    }
    Ok(profiles)
}

fn observed_member_configurations(
    activity: &Value,
) -> Result<BTreeMap<String, ObservedMemberConfiguration>, CoordinatorServiceError> {
    let activity = activity
        .as_object()
        .ok_or(CoordinatorServiceError::InvalidObservation)?;
    let roster = bounded_array(activity.get("roster"), MAX_DISCOVERED_ROSTER)?;
    let mut configurations = BTreeMap::new();
    for entry in roster {
        let member_key = required_identifier(entry, "member_key")?.to_owned();
        let provider = required_identifier(entry, "provider")?.to_owned();
        let model = required_bounded_text(entry, "requested_model")?.to_owned();
        let effort = match entry.get("requested_effort") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_str()
                    .filter(|value| bounded_text(value))
                    .ok_or(CoordinatorServiceError::InvalidObservation)?
                    .to_owned(),
            ),
        };
        let revision = required_revision(entry, "configuration_revision")?;
        let moving_alias_acknowledged = entry
            .get("moving_alias_acknowledged")
            .and_then(Value::as_bool)
            .ok_or(CoordinatorServiceError::InvalidObservation)?;
        if configurations
            .insert(
                member_key,
                ObservedMemberConfiguration {
                    provider,
                    model,
                    effort,
                    revision,
                    moving_alias_acknowledged,
                },
            )
            .is_some()
        {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
    }
    Ok(configurations)
}

fn progress_review_plan(
    profile: &MemberExecutionProfile,
    target: &WorkerSemanticTarget,
    due_sequence: u64,
) -> Result<WorkerActionPlan, CoordinatorServiceError> {
    let (task, action_type, execution_epoch, review_id, review_revision, work_id, work_revision) =
        match target {
            WorkerSemanticTarget::Planning { .. }
            | WorkerSemanticTarget::ControlledFixtureAdmin { .. }
            | WorkerSemanticTarget::WorkProposal { .. }
            | WorkerSemanticTarget::WorkClaim { .. }
            | WorkerSemanticTarget::WorkDependencyRevision { .. }
            | WorkerSemanticTarget::WorkAttempt { .. }
            | WorkerSemanticTarget::LateContribution { .. }
            | WorkerSemanticTarget::SuggestionSubmission { .. }
            | WorkerSemanticTarget::CandidateReview { .. }
            | WorkerSemanticTarget::FindingResolution { .. }
            | WorkerSemanticTarget::CorrectionOutcome { .. } => {
                return Err(CoordinatorServiceError::InvalidState);
            }
            WorkerSemanticTarget::ProgressReviewClaim {
                execution_epoch,
                review_id,
                review_revision,
                work_id,
                work_revision,
            } => (
                "claim",
                "claim_work_item",
                *execution_epoch,
                review_id,
                *review_revision,
                work_id,
                *work_revision,
            ),
            WorkerSemanticTarget::ProgressReviewReport {
                execution_epoch,
                review_id,
                review_revision,
                work_id,
                work_revision,
            } => (
                "report",
                "report_progress_review",
                *execution_epoch,
                review_id,
                *review_revision,
                work_id,
                *work_revision,
            ),
        };
    let identity = serde_json::to_vec(&json!({
        "configuration_revision": profile.configuration_revision,
        "member_key": profile.member_key,
        "target": target,
    }))
    .map_err(|_| CoordinatorServiceError::InvalidState)?;
    let invocation_id = format!("auto-review-{}", blake3::hash(&identity).to_hex());
    let instruction = serde_json::to_string(&json!({
        "schema": PROGRESS_REVIEW_INSTRUCTION_SCHEMA,
        "task": task,
        "target": {
            "execution_epoch": execution_epoch,
            "review_id": review_id,
            "review_revision": review_revision,
            "work_id": work_id,
            "work_revision": work_revision,
        },
        "instructions": if task == "claim" {
            "Author exactly one claim_work_item Action proposal for this exact Work Item and revision. Choose a new portable attempt_id."
        } else {
            "Assess current Swarm progress and author exactly one report_progress_review Action proposal for this exact Progress Review and revision."
        },
    }))
    .map_err(|_| CoordinatorServiceError::InvalidState)?;
    Ok(WorkerActionPlan {
        invocation_id,
        swarm_id: profile.swarm_id.clone(),
        member_key: profile.member_key.clone(),
        allowed_action_types: vec![action_type.to_owned()],
        provider: profile.provider,
        configuration_revision: profile.configuration_revision,
        model: profile.model.clone(),
        effort: profile.effort.clone(),
        moving_alias_acknowledged: profile.moving_alias_acknowledged,
        resource_policy: profile.resource_policy,
        allowed_tools: profile.allowed_tools.clone(),
        session: profile.session.clone(),
        kind: InvocationKind::ProgressReview,
        due_sequence,
        semantic_target: Some(target.clone()),
        instruction,
    })
}

fn progress_review_execution_status(
    review: &DiscoveredReview,
    retained: Option<&WorkerActionPlan>,
    snapshot: &SwarmExecutionSnapshot,
    invocations: &[CoordinatorInvocationStatus],
) -> ProgressReviewExecutionStatus {
    if review.authoritative_status == ProgressReviewAuthoritativeStatus::Blocked {
        return ProgressReviewExecutionStatus::Blocked;
    }
    let Some(plan) = &review.plan else {
        return ProgressReviewExecutionStatus::NoEligibleProvider;
    };
    let Some(_) = retained else {
        return match review.authoritative_status {
            ProgressReviewAuthoritativeStatus::Due => ProgressReviewExecutionStatus::Due,
            ProgressReviewAuthoritativeStatus::Claimed => ProgressReviewExecutionStatus::Claimed,
            ProgressReviewAuthoritativeStatus::Blocked => ProgressReviewExecutionStatus::Blocked,
        };
    };
    if snapshot
        .active
        .iter()
        .any(|active| active.ticket.invocation_id == plan.invocation_id)
    {
        return ProgressReviewExecutionStatus::Running;
    }
    let outcome = invocations
        .iter()
        .find(|status| status.invocation_id == plan.invocation_id)
        .map(|status| &status.coordinator_outcome);
    match outcome {
        Some(CoordinatorOutcome::Running | CoordinatorOutcome::ProcessingResult) => {
            ProgressReviewExecutionStatus::Running
        }
        Some(CoordinatorOutcome::LaunchUncertain | CoordinatorOutcome::SubmissionUncertain) => {
            ProgressReviewExecutionStatus::ManualReconciliationRequired
        }
        Some(CoordinatorOutcome::Pending) | None => {
            ProgressReviewExecutionStatus::WaitingForCapacity
        }
        Some(
            CoordinatorOutcome::NeedsReevaluation
            | CoordinatorOutcome::Accepted { .. }
            | CoordinatorOutcome::Rejected { .. }
            | CoordinatorOutcome::NotSubmitted { .. }
            | CoordinatorOutcome::SettledWithoutSubmission
            | CoordinatorOutcome::PlanningDecisionRecorded,
        ) => ProgressReviewExecutionStatus::Blocked,
    }
}

fn validate_state(state: &ServiceState) -> Result<(), CoordinatorServiceError> {
    autonomy::validate_autonomy(state)?;
    if state.view.swarm_id != state.swarm_id
        || state.view.revision != state.revision
        || state.plans.iter().any(|(invocation_id, plan)| {
            invocation_id != &plan.invocation_id
                || plan.swarm_id != state.swarm_id
                || !valid_identifier(invocation_id)
                || validate_plan(plan).is_err()
                || !state.known_invocations.contains(invocation_id)
        })
        || state
            .known_invocations
            .iter()
            .any(|invocation_id| !valid_identifier(invocation_id))
        || state
            .pending_targeted_cancellations
            .iter()
            .any(|invocation_id| {
                !state.known_invocations.contains(invocation_id)
                    || state.plans.get(invocation_id).is_none_or(|plan| {
                        !matches!(
                            plan.semantic_target,
                            Some(WorkerSemanticTarget::WorkAttempt { .. })
                        )
                    })
            })
    {
        return Err(CoordinatorServiceError::InvalidState);
    }
    let mut identities = BTreeSet::new();
    if state.view.invocations.iter().any(|status| {
        !valid_identifier(&status.invocation_id)
            || !state.known_invocations.contains(&status.invocation_id)
            || !identities.insert(status.invocation_id.as_str())
    }) {
        return Err(CoordinatorServiceError::InvalidState);
    }
    let mut review_ids = BTreeSet::new();
    if state.view.progress_reviews.iter().any(|review| {
        !valid_identifier(&review.review_id)
            || !valid_identifier(&review.work_id)
            || review.review_revision == 0
            || review.work_revision == 0
            || !review_ids.insert(review.review_id.as_str())
            || review
                .member_key
                .as_deref()
                .is_some_and(|member_key| !valid_identifier(member_key))
            || review.invocation_id.as_ref().is_some_and(|invocation_id| {
                !valid_identifier(invocation_id)
                    || !state.known_invocations.contains(invocation_id)
                    || state
                        .plans
                        .get(invocation_id)
                        .is_none_or(|plan| !operational_review_matches_plan(review, plan))
            })
    }) {
        return Err(CoordinatorServiceError::InvalidState);
    }
    let mut expected = state.view.clone();
    refresh_counts(&mut expected);
    if expected.pending_count != state.view.pending_count
        || expected.manual_attention_count != state.view.manual_attention_count
    {
        return Err(CoordinatorServiceError::InvalidState);
    }
    Ok(())
}

fn operational_review_matches_plan(
    review: &ProgressReviewOperationalView,
    plan: &WorkerActionPlan,
) -> bool {
    let Some(target) = &plan.semantic_target else {
        return false;
    };
    let (review_id, review_revision, work_id, work_revision) = match target {
        WorkerSemanticTarget::Planning { .. }
        | WorkerSemanticTarget::ControlledFixtureAdmin { .. }
        | WorkerSemanticTarget::WorkProposal { .. }
        | WorkerSemanticTarget::WorkClaim { .. }
        | WorkerSemanticTarget::WorkDependencyRevision { .. }
        | WorkerSemanticTarget::WorkAttempt { .. }
        | WorkerSemanticTarget::LateContribution { .. }
        | WorkerSemanticTarget::SuggestionSubmission { .. }
        | WorkerSemanticTarget::CandidateReview { .. }
        | WorkerSemanticTarget::FindingResolution { .. }
        | WorkerSemanticTarget::CorrectionOutcome { .. } => return false,
        WorkerSemanticTarget::ProgressReviewClaim {
            review_id,
            review_revision,
            work_id,
            work_revision,
            ..
        }
        | WorkerSemanticTarget::ProgressReviewReport {
            review_id,
            review_revision,
            work_id,
            work_revision,
            ..
        } => (review_id, review_revision, work_id, work_revision),
    };
    review_id == &review.review_id
        && *review_revision == review.review_revision
        && work_id == &review.work_id
        && *work_revision == review.work_revision
        && review.member_key.as_deref() == Some(plan.member_key.as_str())
}

fn load_state(path: &Path) -> Result<Option<ServiceState>, CoordinatorServiceError> {
    match fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CoordinatorServiceError::StateUnavailable),
        Ok(metadata) if metadata.len() > MAX_JOURNAL_BYTES => {
            return Err(CoordinatorServiceError::InvalidState);
        }
        Ok(_) => {}
    }
    validate_owner_only_file(path).map_err(|_| CoordinatorServiceError::UnsafeStorage)?;
    let bytes = fs::read(path).map_err(|_| CoordinatorServiceError::StateUnavailable)?;
    let envelope: ServiceEnvelope =
        serde_json::from_slice(&bytes).map_err(|_| CoordinatorServiceError::InvalidState)?;
    if envelope.schema != JOURNAL_SCHEMA || envelope.digest != state_digest(&envelope.state)? {
        return Err(CoordinatorServiceError::InvalidState);
    }
    Ok(Some(envelope.state))
}

fn state_digest(state: &ServiceState) -> Result<String, CoordinatorServiceError> {
    let bytes = serde_json::to_vec(state).map_err(|_| CoordinatorServiceError::InvalidState)?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// Closed service failures. Display text excludes prompts and provider output.
#[derive(Debug, Error)]
pub enum CoordinatorServiceError {
    #[error("the coordinator service state is invalid")]
    InvalidState,
    #[error("coordinator service state is unavailable")]
    StateUnavailable,
    #[error("coordinator service storage is unsafe")]
    UnsafeStorage,
    #[error("another coordinator service owns this journal")]
    WriterActive,
    #[error("the coordinator service root belongs to another Swarm")]
    SwarmMismatch,
    #[error("the worker plan conflicts with retained service state")]
    Conflict,
    #[error("the execution daemon has not registered this Swarm")]
    SwarmNotRegistered,
    #[error("the managed Progress Review observation is invalid or exceeds discovery bounds")]
    InvalidObservation,
    #[error("the coordinator poll interval is invalid")]
    InvalidPollInterval,
    #[error(transparent)]
    Coordinator(#[from] CoordinatorError),
    #[error(transparent)]
    Daemon(#[from] DaemonError),
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};

    use super::*;
    use crate::{
        AcceptanceCriterion, ArtifactRef, InvocationEvidence, MemberConfiguration,
        ProviderConfigurationState, SwarmView,
        execution::{
            ActiveInvocation, InvocationKind, InvocationOutput, InvocationResolution,
            InvocationTicket, ProviderKind, ResourcePolicy, RunBudget, SessionSelection,
        },
    };

    struct FakeDriver {
        state: Arc<Mutex<FakeDriverState>>,
    }

    struct FakeDriverState {
        fail_stage: bool,
        staged_state: CoordinatorIntentState,
        stage_calls: usize,
        dispatch_calls: usize,
        harvest_calls: usize,
        retry_action_calls: usize,
        retry_launch_calls: usize,
        interruption_reconciliations: Vec<(String, InvocationResolution)>,
        fail_interruption_reconcile_once: bool,
        intents: BTreeMap<String, CoordinatorIntentView>,
        observation: Option<SwarmObservation>,
        fail_preflight: bool,
    }

    impl FakeDriverState {
        fn new(staged_state: CoordinatorIntentState) -> Self {
            Self {
                fail_stage: false,
                staged_state,
                stage_calls: 0,
                dispatch_calls: 0,
                harvest_calls: 0,
                retry_action_calls: 0,
                retry_launch_calls: 0,
                interruption_reconciliations: Vec::new(),
                fail_interruption_reconcile_once: false,
                intents: BTreeMap::new(),
                observation: None,
                fail_preflight: false,
            }
        }
    }

    impl CoordinatorDriver for FakeDriver {
        fn stage(
            &mut self,
            plan: WorkerActionPlan,
        ) -> Result<CoordinatorIntentView, CoordinatorError> {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.stage_calls += 1;
            if state.fail_stage {
                return Err(CoordinatorError::StateUnavailable);
            }
            let intent = intent_from_plan(&plan, state.staged_state);
            state
                .intents
                .insert(plan.invocation_id.clone(), intent.clone());
            Ok(intent)
        }

        fn dispatch(&mut self) -> Result<Vec<CoordinatorEvent>, CoordinatorError> {
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .dispatch_calls += 1;
            Ok(Vec::new())
        }

        fn harvest(&mut self) -> Result<Vec<CoordinatorEvent>, CoordinatorError> {
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .harvest_calls += 1;
            Ok(Vec::new())
        }

        fn retry_action(
            &mut self,
            _invocation_id: &str,
        ) -> Result<CoordinatorEvent, CoordinatorError> {
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retry_action_calls += 1;
            Err(CoordinatorError::Conflict)
        }

        fn retry_launch(
            &mut self,
            _invocation_id: &str,
        ) -> Result<CoordinatorEvent, CoordinatorError> {
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retry_launch_calls += 1;
            Err(CoordinatorError::Conflict)
        }

        fn intent(&self, invocation_id: &str) -> Result<CoordinatorIntentView, CoordinatorError> {
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .intents
                .get(invocation_id)
                .cloned()
                .ok_or(CoordinatorError::NotFound)
        }

        fn reconcile_interruption(
            &mut self,
            invocation_id: &str,
            resolution: InvocationResolution,
        ) -> Result<CoordinatorEvent, CoordinatorError> {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state
                .interruption_reconciliations
                .push((invocation_id.to_owned(), resolution));
            if state.fail_interruption_reconcile_once {
                state.fail_interruption_reconcile_once = false;
                return Err(CoordinatorError::StateUnavailable);
            }
            let intent = state
                .intents
                .get_mut(invocation_id)
                .ok_or(CoordinatorError::NotFound)?;
            match resolution {
                InvocationResolution::Terminated => {
                    intent.state = CoordinatorIntentState::Settled;
                    intent.submission =
                        SubmissionDisposition::NotSubmitted(NonSubmissionReason::ProcessFailed);
                    Ok(CoordinatorEvent::NotSubmitted {
                        invocation_id: invocation_id.to_owned(),
                        reason: NonSubmissionReason::ProcessFailed,
                    })
                }
                InvocationResolution::Unknown => {
                    intent.state = CoordinatorIntentState::NeedsReevaluation;
                    intent.submission =
                        SubmissionDisposition::NotSubmitted(NonSubmissionReason::AuthorityChanged);
                    Ok(CoordinatorEvent::NeedsReevaluation {
                        invocation_id: invocation_id.to_owned(),
                    })
                }
                InvocationResolution::Running
                | InvocationResolution::Completed
                | InvocationResolution::Failed => Err(CoordinatorError::Conflict),
            }
        }

        fn observe(
            &self,
            swarm_id: &SwarmId,
            actor: &SwarmActor,
        ) -> Result<SwarmObservation, CoordinatorError> {
            let mut observation = self
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .observation
                .clone()
                .unwrap_or_else(|| review_observation(swarm_id, None));
            observation.actor = actor.clone();
            if let SwarmActor::Worker { member_key } = actor {
                observation.member_id = member_id_for(member_key).to_owned();
            }
            Ok(observation)
        }

        fn preflight(&self, _plan: &WorkerActionPlan) -> Result<(), CoordinatorError> {
            if self
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .fail_preflight
            {
                Err(CoordinatorError::ProviderBlocked)
            } else {
                Ok(())
            }
        }
    }

    struct FakeExecution {
        calls: Arc<Mutex<usize>>,
        snapshot: ExecutionSnapshot,
    }

    impl ExecutionStatusSource for FakeExecution {
        fn status(&self) -> Result<ExecutionSnapshot, DaemonError> {
            *self.calls.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            Ok(self.snapshot.clone())
        }
    }

    struct TargetedFakeExecution {
        state: Arc<Mutex<TargetedFakeExecutionState>>,
    }

    struct TargetedFakeExecutionState {
        snapshot: ExecutionSnapshot,
        cancel_calls: Vec<(String, String)>,
        cancelled: BTreeSet<String>,
        unknown_on_cancel: bool,
        conflict_on_cancel: bool,
        retained_completion: bool,
    }

    impl TargetedFakeExecutionState {
        fn new(snapshot: ExecutionSnapshot) -> Self {
            Self {
                snapshot,
                cancel_calls: Vec::new(),
                cancelled: BTreeSet::new(),
                unknown_on_cancel: false,
                conflict_on_cancel: false,
                retained_completion: false,
            }
        }
    }

    impl ExecutionStatusSource for TargetedFakeExecution {
        fn status(&self) -> Result<ExecutionSnapshot, DaemonError> {
            Ok(self
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .snapshot
                .clone())
        }

        fn cancel_invocation(
            &self,
            swarm_id: &SwarmId,
            invocation_id: &str,
        ) -> Result<ExecutionPhase, DaemonError> {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state
                .cancel_calls
                .push((swarm_id.as_str().to_owned(), invocation_id.to_owned()));
            if state.conflict_on_cancel {
                return Err(DaemonError::Control(
                    crate::execution::ControlFault::Conflict,
                ));
            }
            if state.cancelled.contains(invocation_id) {
                return state
                    .snapshot
                    .swarms
                    .iter()
                    .find(|snapshot| snapshot.swarm_id == swarm_id.as_str())
                    .map(|snapshot| snapshot.phase)
                    .ok_or(DaemonError::InvalidResponse);
            }
            let unknown_on_cancel = state.unknown_on_cancel;
            let target = state
                .snapshot
                .swarms
                .iter_mut()
                .find(|snapshot| snapshot.swarm_id == swarm_id.as_str())
                .ok_or(DaemonError::InvalidResponse)?;
            let Some(position) = target
                .active
                .iter()
                .position(|active| active.ticket.invocation_id == invocation_id)
            else {
                return Err(DaemonError::InvalidResponse);
            };
            if unknown_on_cancel {
                target.active[position].resolution = InvocationResolution::Unknown;
                target.phase = ExecutionPhase::BlockedUnknown;
            } else {
                target.active.remove(position);
            }
            let phase = target.phase;
            state.cancelled.insert(invocation_id.to_owned());
            Ok(phase)
        }

        fn retained_completion(&self, _: &SwarmId, _: &str) -> Result<bool, DaemonError> {
            Ok(self
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retained_completion)
        }
    }

    #[test]
    fn cancellation_conflict_requires_native_completion_and_terminal_coordinator_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        for completion_proven in [false, true] {
            let directory = tempfile::tempdir()?;
            let swarm_id = test_swarm_id()?;
            let affected =
                work_attempt_plan(&swarm_id, "affected", "worker-a", "work-a", "attempt-a");
            let unaffected =
                work_attempt_plan(&swarm_id, "unaffected", "worker-b", "work-b", "attempt-b");
            let driver = Arc::new(Mutex::new(FakeDriverState::new(
                CoordinatorIntentState::Running,
            )));
            let mut snapshot = running_snapshot(&swarm_id);
            snapshot.swarms[0].active = vec![active_invocation(&affected)];
            let mut execution = TargetedFakeExecutionState::new(snapshot);
            execution.conflict_on_cancel = true;
            execution.retained_completion = completion_proven;
            let mut service = CoordinatorService::open_with_ports(
                directory.path(),
                &swarm_id,
                Box::new(FakeDriver {
                    state: Arc::clone(&driver),
                }),
                Box::new(TargetedFakeExecution {
                    state: Arc::new(Mutex::new(execution)),
                }),
            )?;
            service.stage(affected.clone())?;
            {
                let mut state = driver.lock().unwrap_or_else(PoisonError::into_inner);
                state.observation = Some(direction_observation(&swarm_id, &affected, &unaffected));
                state.fail_preflight = true;
            }
            let result = service.run_once();
            if completion_proven {
                assert!(matches!(
                    result,
                    Err(CoordinatorServiceError::Coordinator(
                        CoordinatorError::Conflict
                    ))
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(CoordinatorServiceError::Daemon(DaemonError::Control(
                        crate::execution::ControlFault::Conflict
                    )))
                ));
            }
            assert!(
                service
                    .state
                    .pending_targeted_cancellations
                    .contains(&affected.invocation_id)
            );
            assert_eq!(
                service.intent(&affected.invocation_id)?.state,
                CoordinatorIntentState::Running
            );
            let state = driver.lock().unwrap_or_else(PoisonError::into_inner);
            assert_eq!(state.harvest_calls, usize::from(completion_proven));
            assert!(state.interruption_reconciliations.is_empty());
        }
        Ok(())
    }

    #[test]
    fn unknown_interruption_without_completion_evidence_is_not_terminal()
    -> Result<(), Box<dyn std::error::Error>> {
        let plan = work_attempt_plan(
            &test_swarm_id()?,
            "unknown",
            "worker-a",
            "work-a",
            "attempt-a",
        );
        let mut intent = intent_from_plan(&plan, CoordinatorIntentState::NeedsReevaluation);
        intent.submission =
            SubmissionDisposition::NotSubmitted(NonSubmissionReason::AuthorityChanged);
        assert!(!interruption_is_terminal(&intent));
        Ok(())
    }

    #[test]
    fn opening_service_performs_no_daemon_command() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let execution_calls = Arc::new(Mutex::new(0));
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        let _service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver { state: driver }),
            Box::new(FakeExecution {
                calls: Arc::clone(&execution_calls),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        assert_eq!(
            *execution_calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            0
        );
        Ok(())
    }

    #[test]
    fn stage_rejects_an_unbound_plan_before_retaining_or_calling_the_driver()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: Arc::clone(&driver),
            }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        let mut plan = test_plan(&swarm_id);
        plan.semantic_target = None;

        assert!(matches!(
            service.stage(plan),
            Err(CoordinatorServiceError::Coordinator(
                CoordinatorError::InvalidPlan
            ))
        ));
        assert_eq!(
            driver
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .stage_calls,
            0
        );
        assert!(service.state.plans.is_empty());
        assert!(service.state.known_invocations.is_empty());
        assert!(service.state.view.invocations.is_empty());
        Ok(())
    }

    #[test]
    fn direction_cancels_only_the_exact_bound_invocation_and_is_cycle_idempotent()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let affected = work_attempt_plan(
            &swarm_id,
            "invocation-affected",
            "worker-a",
            "work-affected",
            "attempt-affected",
        );
        let unaffected = work_attempt_plan(
            &swarm_id,
            "invocation-unaffected",
            "worker-b",
            "work-unaffected",
            "attempt-unaffected",
        );
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Running,
        )));
        let mut snapshot = running_snapshot(&swarm_id);
        snapshot.provider_caps.insert(ProviderKind::Controlled, 2);
        snapshot.swarms[0].active =
            vec![active_invocation(&affected), active_invocation(&unaffected)];
        let execution = Arc::new(Mutex::new(TargetedFakeExecutionState::new(snapshot)));
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: Arc::clone(&driver),
            }),
            Box::new(TargetedFakeExecution {
                state: Arc::clone(&execution),
            }),
        )?;
        service.stage(affected.clone())?;
        service.stage(unaffected.clone())?;
        {
            let mut driver = driver.lock().unwrap_or_else(PoisonError::into_inner);
            driver.observation = Some(direction_observation(&swarm_id, &affected, &unaffected));
            driver.fail_preflight = true;
        }

        let first = service.run_once()?;
        assert_eq!(
            first.events,
            vec![CoordinatorEvent::NotSubmitted {
                invocation_id: affected.invocation_id.clone(),
                reason: NonSubmissionReason::ProcessFailed,
            }]
        );
        let affected_status = first
            .service
            .invocations
            .iter()
            .find(|status| status.invocation_id == affected.invocation_id)
            .ok_or("affected status missing")?;
        assert_eq!(
            affected_status.coordinator_outcome,
            CoordinatorOutcome::NotSubmitted {
                reason: "process_failed".to_owned(),
            }
        );
        let unaffected_status = first
            .service
            .invocations
            .iter()
            .find(|status| status.invocation_id == unaffected.invocation_id)
            .ok_or("unaffected status missing")?;
        assert_eq!(
            unaffected_status.coordinator_outcome,
            CoordinatorOutcome::Running
        );
        {
            let execution = execution.lock().unwrap_or_else(PoisonError::into_inner);
            assert_eq!(
                execution.cancel_calls,
                vec![(swarm_id.as_str().to_owned(), affected.invocation_id.clone())]
            );
            assert_eq!(
                execution.snapshot.swarms[0].desired,
                DesiredExecution::Running
            );
            assert_eq!(execution.snapshot.swarms[0].active.len(), 1);
            assert_eq!(
                execution.snapshot.swarms[0].active[0].ticket.invocation_id,
                unaffected.invocation_id
            );
        }

        let second = service.run_once()?;
        assert!(second.events.is_empty());
        assert_eq!(
            execution
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .cancel_calls
                .len(),
            1
        );
        Ok(())
    }

    #[test]
    fn cancellation_prepare_fence_replays_safely_after_restart()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let affected = work_attempt_plan(
            &swarm_id,
            "invocation-restart",
            "worker-a",
            "work-restart",
            "attempt-restart",
        );
        let unaffected = work_attempt_plan(
            &swarm_id,
            "invocation-restart-fixture",
            "worker-b",
            "work-restart-fixture",
            "attempt-restart-fixture",
        );
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Running,
        )));
        let mut snapshot = running_snapshot(&swarm_id);
        snapshot.swarms[0].active = vec![active_invocation(&affected)];
        let execution = Arc::new(Mutex::new(TargetedFakeExecutionState::new(snapshot)));
        let mut first = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: Arc::clone(&driver),
            }),
            Box::new(TargetedFakeExecution {
                state: Arc::clone(&execution),
            }),
        )?;
        first.stage(affected.clone())?;
        {
            let mut driver = driver.lock().unwrap_or_else(PoisonError::into_inner);
            driver.observation = Some(direction_observation(&swarm_id, &affected, &unaffected));
            driver.fail_preflight = true;
            driver.fail_interruption_reconcile_once = true;
        }
        assert!(matches!(
            first.run_once(),
            Err(CoordinatorServiceError::Coordinator(
                CoordinatorError::StateUnavailable
            ))
        ));
        assert!(
            first
                .state
                .pending_targeted_cancellations
                .contains(&affected.invocation_id)
        );
        drop(first);

        let mut restarted = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: Arc::clone(&driver),
            }),
            Box::new(TargetedFakeExecution {
                state: Arc::clone(&execution),
            }),
        )?;
        let receipt = restarted.run_once()?;
        assert_eq!(receipt.events.len(), 1);
        assert!(restarted.state.pending_targeted_cancellations.is_empty());
        let execution = execution.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(execution.cancel_calls.len(), 2);
        assert!(
            execution
                .cancel_calls
                .iter()
                .all(|(_, invocation_id)| invocation_id == &affected.invocation_id)
        );
        Ok(())
    }

    #[test]
    fn unknown_targeted_cancellation_requires_recovery_without_touching_other_work()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let affected = work_attempt_plan(
            &swarm_id,
            "invocation-unknown",
            "worker-a",
            "work-unknown",
            "attempt-unknown",
        );
        let unaffected = work_attempt_plan(
            &swarm_id,
            "invocation-still-running",
            "worker-b",
            "work-still-running",
            "attempt-still-running",
        );
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Running,
        )));
        let mut snapshot = running_snapshot(&swarm_id);
        snapshot.provider_caps.insert(ProviderKind::Controlled, 2);
        snapshot.swarms[0].active =
            vec![active_invocation(&affected), active_invocation(&unaffected)];
        let mut execution_state = TargetedFakeExecutionState::new(snapshot);
        execution_state.unknown_on_cancel = true;
        let execution = Arc::new(Mutex::new(execution_state));
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: Arc::clone(&driver),
            }),
            Box::new(TargetedFakeExecution {
                state: Arc::clone(&execution),
            }),
        )?;
        service.stage(affected.clone())?;
        service.stage(unaffected.clone())?;
        {
            let mut driver = driver.lock().unwrap_or_else(PoisonError::into_inner);
            driver.observation = Some(direction_observation(&swarm_id, &affected, &unaffected));
            driver.fail_preflight = true;
        }

        let receipt = service.run_once()?;
        assert_eq!(
            receipt.events,
            vec![CoordinatorEvent::NeedsReevaluation {
                invocation_id: affected.invocation_id.clone(),
            }]
        );
        assert_eq!(
            receipt.service.loop_state,
            CoordinatorLoopState::RecoveryRequired
        );
        let execution = execution.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(execution.cancel_calls.len(), 1);
        assert_eq!(execution.snapshot.swarms[0].active.len(), 2);
        assert_eq!(
            execution.snapshot.swarms[0]
                .active
                .iter()
                .find(|active| active.ticket.invocation_id == unaffected.invocation_id)
                .map(|active| active.resolution),
            Some(InvocationResolution::Running)
        );
        Ok(())
    }

    #[test]
    fn an_unbound_plan_is_never_guessed_as_the_interruption_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let plan = test_plan(&swarm_id);
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Running,
        )));
        let mut observation = review_observation(&swarm_id, None);
        observation.activity["work_items"] = json!([{
            "work_id": "work-unbound",
            "revision": 3,
            "execution_epoch": 1,
            "kind": "goal",
            "status": "blocked",
            "owner_member_id": null,
            "active_attempt_id": null
        }]);
        observation.activity["work_attempts"] = json!([{
            "attempt_id": "attempt-unbound",
            "revision": 2,
            "work_id": "work-unbound",
            "execution_epoch": 1,
            "owner_member_id": "member-1",
            "status": "interruption_requested"
        }]);
        driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observation = Some(observation);
        let mut snapshot = running_snapshot(&swarm_id);
        snapshot.swarms[0].active = vec![active_invocation(&plan)];
        let execution = Arc::new(Mutex::new(TargetedFakeExecutionState::new(snapshot)));
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver { state: driver }),
            Box::new(TargetedFakeExecution {
                state: Arc::clone(&execution),
            }),
        )?;
        service.stage(plan)?;
        service.run_once()?;
        assert!(
            execution
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .cancel_calls
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn retained_plan_is_restaged_after_a_crash_window() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let plan = test_plan(&swarm_id);
        let failed_driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        failed_driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .fail_stage = true;
        let mut first = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: Arc::clone(&failed_driver),
            }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        assert!(matches!(
            first.stage(plan.clone()),
            Err(CoordinatorServiceError::Coordinator(
                CoordinatorError::StateUnavailable
            ))
        ));
        let retained = read_coordinator_service_view(directory.path(), &swarm_id)?
            .ok_or("service view missing")?;
        assert_eq!(retained.pending_count, 1);
        assert!(!serde_json::to_string(&retained)?.contains("secret instruction"));
        drop(first);

        let recovered_driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        let mut recovered = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: Arc::clone(&recovered_driver),
            }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        let receipt = recovered.run_once()?;
        let driver = recovered_driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert_eq!(driver.stage_calls, 1);
        assert_eq!(driver.dispatch_calls, 1);
        assert_eq!(driver.harvest_calls, 1);
        assert_eq!(
            receipt.service.loop_state,
            CoordinatorLoopState::WaitingForCapacity
        );
        Ok(())
    }

    #[test]
    fn due_review_generates_one_durable_plan_across_polls_and_restart()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observation = Some(review_observation(&swarm_id, Some("due")));
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: Arc::clone(&driver),
            }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        let first = service.run_once()?;
        let generated = service
            .state
            .plans
            .values()
            .filter(|plan| plan.kind == InvocationKind::ProgressReview)
            .collect::<Vec<_>>();
        assert_eq!(generated.len(), 1);
        assert_eq!(generated[0].kind, InvocationKind::ProgressReview);
        assert_eq!(generated[0].member_key, "worker-a");
        assert_eq!(generated[0].provider, ProviderKind::Controlled);
        assert_eq!(generated[0].configuration_revision, 1);
        assert_eq!(generated[0].model, "fixture-v1");
        assert_eq!(generated[0].effort.as_deref(), Some("medium"));
        assert_eq!(generated[0].resource_policy, ResourcePolicy::ReadOnly);
        assert!(generated[0].allowed_tools.is_empty());
        assert_eq!(
            generated[0].session,
            SessionSelection::Fresh { requested_id: None }
        );
        assert!(matches!(
            generated[0].semantic_target,
            Some(WorkerSemanticTarget::ProgressReviewClaim { .. })
        ));
        assert_eq!(first.service.progress_reviews.len(), 1);
        assert_eq!(
            first.service.progress_reviews[0].execution_status,
            ProgressReviewExecutionStatus::WaitingForCapacity
        );
        let generated_id = generated[0].invocation_id.clone();
        service.run_once()?;
        assert_eq!(
            service
                .state
                .plans
                .values()
                .filter(|plan| plan.kind == InvocationKind::ProgressReview)
                .count(),
            1
        );
        drop(service);

        let restarted_driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        restarted_driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observation = Some(review_observation(&swarm_id, Some("due")));
        let mut restarted = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: restarted_driver,
            }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        restarted.run_once()?;
        let generated = restarted
            .state
            .plans
            .values()
            .filter(|plan| plan.kind == InvocationKind::ProgressReview)
            .collect::<Vec<_>>();
        assert_eq!(generated.len(), 1);
        assert_eq!(generated[0].invocation_id, generated_id);
        Ok(())
    }

    #[test]
    fn review_bootstrap_uses_fresh_roster_revision_and_not_a_stale_retained_policy()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        let mut observation = review_observation(&swarm_id, Some("due"));
        observation.swarm.roster[0].configuration_revision = 2;
        observation.swarm.roster[0].requested_model = "fixture/model:2".to_owned();
        observation.swarm.roster[0].requested_effort = Some("high".to_owned());
        observation.swarm.roster[0].moving_alias_acknowledged = true;
        observation.activity["roster"][0]["configuration_revision"] = json!(2);
        observation.activity["roster"][0]["requested_model"] = json!("fixture/model:2");
        observation.activity["roster"][0]["requested_effort"] = json!("high");
        observation.activity["roster"][0]["moving_alias_acknowledged"] = json!(true);
        driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observation = Some(observation);
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver { state: driver }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        let mut stale = test_plan(&swarm_id);
        stale.resource_policy = ResourcePolicy::WorkspaceWrite;
        stale.session = SessionSelection::Resume {
            session_id: "stale-session".to_owned(),
        };
        service.stage(stale)?;

        service.run_once()?;
        let generated = service
            .state
            .plans
            .values()
            .find(|plan| plan.kind == InvocationKind::ProgressReview)
            .ok_or("generated review plan missing")?;
        assert_eq!(generated.configuration_revision, 2);
        assert_eq!(generated.model, "fixture/model:2");
        assert_eq!(generated.effort.as_deref(), Some("high"));
        assert!(generated.moving_alias_acknowledged);
        assert_eq!(generated.resource_policy, ResourcePolicy::ReadOnly);
        assert_eq!(
            generated.session,
            SessionSelection::Fresh { requested_id: None }
        );
        Ok(())
    }

    #[test]
    fn claimed_review_is_bound_to_its_owner_and_exact_revisions()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observation = Some(review_observation(&swarm_id, Some("claimed")));
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver { state: driver }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        service.run_once()?;
        let generated = service
            .state
            .plans
            .values()
            .find(|plan| plan.kind == InvocationKind::ProgressReview)
            .ok_or("generated report plan missing")?;
        assert_eq!(generated.member_key, "worker-a");
        assert!(matches!(
            generated.semantic_target,
            Some(WorkerSemanticTarget::ProgressReviewReport {
                review_revision: 2,
                work_revision: 2,
                ..
            })
        ));

        let no_profile_directory = tempfile::tempdir()?;
        let no_profile_driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        no_profile_driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observation = Some(review_observation(&swarm_id, Some("claimed")));
        no_profile_driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .fail_preflight = true;
        let mut no_profile = CoordinatorService::open_with_ports(
            no_profile_directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: no_profile_driver,
            }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        no_profile.run_once()?;
        assert_eq!(
            no_profile.view().progress_reviews[0].member_key.as_deref(),
            Some("worker-a")
        );
        assert_eq!(
            no_profile.view().progress_reviews[0].execution_status,
            ProgressReviewExecutionStatus::NoEligibleProvider
        );
        Ok(())
    }

    #[test]
    fn review_generation_is_suppressed_by_pause_stop_recovery_and_completion()
    -> Result<(), Box<dyn std::error::Error>> {
        for (desired, phase) in [
            (DesiredExecution::Paused, ExecutionPhase::Paused),
            (DesiredExecution::Stopped, ExecutionPhase::Stopped),
            (DesiredExecution::Running, ExecutionPhase::RecoveryRequired),
        ] {
            let directory = tempfile::tempdir()?;
            let swarm_id = test_swarm_id()?;
            let driver = Arc::new(Mutex::new(FakeDriverState::new(
                CoordinatorIntentState::Queued,
            )));
            driver
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .observation = Some(review_observation(&swarm_id, Some("due")));
            let mut snapshot = running_snapshot(&swarm_id);
            snapshot.swarms[0].desired = desired;
            snapshot.swarms[0].phase = phase;
            let mut service = CoordinatorService::open_with_ports(
                directory.path(),
                &swarm_id,
                Box::new(FakeDriver { state: driver }),
                Box::new(FakeExecution {
                    calls: Arc::new(Mutex::new(0)),
                    snapshot,
                }),
            )?;
            service.stage(test_plan(&swarm_id))?;
            service.run_once()?;
            assert_eq!(service.state.plans.len(), 1);
            assert_eq!(
                service.view().progress_reviews[0].execution_status,
                ProgressReviewExecutionStatus::Due
            );
        }

        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        let mut completed = review_observation(&swarm_id, Some("due"));
        completed.activity["phase"] = json!("completed");
        driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observation = Some(completed);
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver { state: driver }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        service.stage(test_plan(&swarm_id))?;
        service.run_once()?;
        assert_eq!(service.state.plans.len(), 1);
        assert!(service.view().progress_reviews.is_empty());
        Ok(())
    }

    #[test]
    fn due_review_reports_no_provider_or_waits_at_zero_capacity()
    -> Result<(), Box<dyn std::error::Error>> {
        let swarm_id = test_swarm_id()?;
        let no_profile_directory = tempfile::tempdir()?;
        let no_profile_driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        no_profile_driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observation = Some(review_observation(&swarm_id, Some("due")));
        no_profile_driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .fail_preflight = true;
        let mut no_profile = CoordinatorService::open_with_ports(
            no_profile_directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: no_profile_driver,
            }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        no_profile.run_once()?;
        assert_eq!(
            no_profile.view().progress_reviews[0].execution_status,
            ProgressReviewExecutionStatus::NoEligibleProvider
        );
        assert!(no_profile.state.plans.is_empty());

        let zero_capacity_directory = tempfile::tempdir()?;
        let zero_capacity_driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        zero_capacity_driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observation = Some(review_observation(&swarm_id, Some("due")));
        let mut snapshot = running_snapshot(&swarm_id);
        snapshot.provider_caps.insert(ProviderKind::Controlled, 0);
        let mut zero_capacity = CoordinatorService::open_with_ports(
            zero_capacity_directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: zero_capacity_driver,
            }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot,
            }),
        )?;
        zero_capacity.stage(test_plan(&swarm_id))?;
        let receipt = zero_capacity.run_once()?;
        assert!(receipt.events.is_empty());
        assert_eq!(
            zero_capacity.view().progress_reviews[0].execution_status,
            ProgressReviewExecutionStatus::WaitingForCapacity
        );
        Ok(())
    }

    #[test]
    fn genesis_observation_at_sequence_zero_suppresses_review_discovery()
    -> Result<(), Box<dyn std::error::Error>> {
        let swarm_id = test_swarm_id()?;
        let mut observation = review_observation(&swarm_id, None);
        observation.room_seq = 0;
        observation.activity["phase"] = json!("awaiting_human_confirmation");
        assert!(parse_progress_reviews(&observation, &BTreeMap::new())?.is_empty());
        Ok(())
    }

    #[test]
    fn claimed_review_uses_authenticated_identity_when_human_projection_strips_member_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::Queued,
        )));
        let mut observation = review_observation(&swarm_id, Some("claimed"));
        for member in observation.activity["roster"]
            .as_array_mut()
            .ok_or("fixture roster missing")?
        {
            member
                .as_object_mut()
                .ok_or("fixture roster member invalid")?
                .remove("member_id");
        }
        driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observation = Some(observation);
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver { state: driver }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;

        service.run_once()?;
        let generated = service
            .state
            .plans
            .values()
            .find(|plan| plan.kind == InvocationKind::ProgressReview)
            .ok_or("generated review plan missing")?;
        assert_eq!(generated.member_key, "worker-a");
        assert!(matches!(
            generated.semantic_target,
            Some(WorkerSemanticTarget::ProgressReviewReport { .. })
        ));
        Ok(())
    }

    #[test]
    fn ambiguous_launch_stops_without_dispatch_harvest_or_retry()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let swarm_id = test_swarm_id()?;
        let driver = Arc::new(Mutex::new(FakeDriverState::new(
            CoordinatorIntentState::LaunchPrepared,
        )));
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(FakeDriver {
                state: Arc::clone(&driver),
            }),
            Box::new(FakeExecution {
                calls: Arc::new(Mutex::new(0)),
                snapshot: running_snapshot(&swarm_id),
            }),
        )?;
        service.stage(test_plan(&swarm_id))?;
        let receipt = service.run_once()?;
        let driver = driver.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(driver.dispatch_calls, 0);
        assert_eq!(driver.harvest_calls, 0);
        assert_eq!(driver.retry_action_calls, 0);
        assert_eq!(driver.retry_launch_calls, 0);
        assert_eq!(
            receipt.service.loop_state,
            CoordinatorLoopState::ManualReconciliationRequired
        );
        assert_eq!(receipt.service.manual_attention_count, 1);
        Ok(())
    }

    #[test]
    fn verified_effective_configuration_is_distinct_from_reported_session()
    -> Result<(), Box<dyn std::error::Error>> {
        let plan = test_plan(&test_swarm_id()?);
        let artifact: ArtifactRef = serde_json::from_value(serde_json::json!({
            "digest": format!("blake3:{}", "a".repeat(64)),
            "byte_length": 0
        }))?;
        let mut intent = intent_from_plan(&plan, CoordinatorIntentState::Settled);
        intent.evidence = Some(InvocationEvidence {
            stdout: artifact.clone(),
            stderr: artifact,
            resolution: InvocationResolution::Completed,
            success: true,
            exit_code: Some(0),
            stdout_truncated: false,
            stderr_truncated: false,
            output: Some(InvocationOutput {
                invocation_id: plan.invocation_id.clone(),
                reported_model: plan.model.clone(),
                reported_effort: plan.effort.clone(),
                session_id: Some("provider-session-7".to_owned()),
                text: "not surfaced".to_owned(),
            }),
            configuration: Some(ConfigurationResolution::Verified {
                model: plan.model.clone(),
                effort: plan.effort.clone(),
            }),
        });
        intent.submission = SubmissionDisposition::Received(SwarmActionReceipt::Accepted {
            action_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
            room_seq: 9,
            duplicate: false,
        });

        let status = status_from_intent(&intent);
        assert_eq!(
            status.provider_evidence.state,
            ProviderEffectiveState::Verified
        );
        assert_eq!(
            status.provider_evidence.effective_model.as_deref(),
            Some("fixture-v1")
        );
        assert_eq!(
            status.provider_evidence.reported_session_id.as_deref(),
            Some("provider-session-7")
        );
        assert_eq!(
            status.coordinator_outcome,
            CoordinatorOutcome::Accepted { duplicate: false }
        );
        Ok(())
    }

    fn test_swarm_id() -> Result<SwarmId, crate::ValidationError> {
        SwarmId::new("swarm-service".to_owned())
    }

    fn test_plan(swarm_id: &SwarmId) -> WorkerActionPlan {
        WorkerActionPlan {
            invocation_id: "invocation-service-1".to_owned(),
            swarm_id: swarm_id.clone(),
            member_key: "worker-a".to_owned(),
            allowed_action_types: vec!["report_progress".to_owned()],
            provider: ProviderKind::Controlled,
            configuration_revision: 1,
            model: "fixture-v1".to_owned(),
            effort: Some("medium".to_owned()),
            moving_alias_acknowledged: false,
            resource_policy: ResourcePolicy::ReadOnly,
            allowed_tools: Vec::new(),
            session: SessionSelection::Fresh { requested_id: None },
            kind: InvocationKind::Work,
            due_sequence: 1,
            semantic_target: Some(WorkerSemanticTarget::ControlledFixtureAdmin {
                fixture_id: "coordinator-service-test".to_owned(),
            }),
            instruction: "secret instruction must stay protected".to_owned(),
        }
    }

    fn work_attempt_plan(
        swarm_id: &SwarmId,
        invocation_id: &str,
        member_key: &str,
        work_id: &str,
        attempt_id: &str,
    ) -> WorkerActionPlan {
        WorkerActionPlan {
            invocation_id: invocation_id.to_owned(),
            swarm_id: swarm_id.clone(),
            member_key: member_key.to_owned(),
            allowed_action_types: vec!["submit_contribution".to_owned()],
            provider: ProviderKind::Controlled,
            configuration_revision: 1,
            model: "fixture-v1".to_owned(),
            effort: Some("medium".to_owned()),
            moving_alias_acknowledged: false,
            resource_policy: ResourcePolicy::WorkspaceWrite,
            allowed_tools: Vec::new(),
            session: SessionSelection::Fresh { requested_id: None },
            kind: InvocationKind::Work,
            due_sequence: 2,
            semantic_target: Some(WorkerSemanticTarget::WorkAttempt {
                execution_epoch: 1,
                work_id: work_id.to_owned(),
                work_revision: 2,
                attempt_id: attempt_id.to_owned(),
                attempt_revision: 1,
            }),
            instruction: format!("complete {work_id}"),
        }
    }

    fn member_id_for(member_key: &str) -> &'static str {
        match member_key {
            "worker-a" => "member-1",
            "worker-b" => "member-2",
            other => unreachable!("unsupported fixture member {other}"),
        }
    }

    fn intent_from_plan(
        plan: &WorkerActionPlan,
        state: CoordinatorIntentState,
    ) -> CoordinatorIntentView {
        CoordinatorIntentView {
            invocation_id: plan.invocation_id.clone(),
            swarm_id: plan.swarm_id.clone(),
            member_key: plan.member_key.clone(),
            member_id: member_id_for(&plan.member_key).to_owned(),
            configuration_revision: plan.configuration_revision,
            provider: plan.provider,
            requested_model: plan.model.clone(),
            requested_effort: plan.effort.clone(),
            session: plan.session.clone(),
            proposal: None,
            action: None,
            state,
            evidence: None,
            submission: SubmissionDisposition::NotAttempted,
            validation_feedback: None,
        }
    }

    fn running_snapshot(swarm_id: &SwarmId) -> ExecutionSnapshot {
        ExecutionSnapshot {
            revision: 1,
            provider_caps: BTreeMap::from([(ProviderKind::Controlled, 1)]),
            swarms: vec![SwarmExecutionSnapshot {
                swarm_id: swarm_id.as_str().to_owned(),
                desired: DesiredExecution::Running,
                phase: ExecutionPhase::Running,
                execution_epoch: 1,
                priority: 1,
                budget: RunBudget::default(),
                invocations_started: 0,
                active_time_ms: 0,
                queued: Vec::new(),
                active: Vec::new(),
                unknown_effects: Vec::new(),
            }],
        }
    }

    fn active_invocation(plan: &WorkerActionPlan) -> ActiveInvocation {
        ActiveInvocation {
            ticket: InvocationTicket {
                invocation_id: plan.invocation_id.clone(),
                swarm_id: plan.swarm_id.as_str().to_owned(),
                member_id: member_id_for(&plan.member_key).to_owned(),
                provider: plan.provider,
                configuration_revision: plan.configuration_revision,
                kind: plan.kind,
                due_sequence: plan.due_sequence,
            },
            execution_epoch: 1,
            started_at_ms: 1,
            resolution: InvocationResolution::Running,
        }
    }

    fn direction_observation(
        swarm_id: &SwarmId,
        affected: &WorkerActionPlan,
        unaffected: &WorkerActionPlan,
    ) -> SwarmObservation {
        let mut observation = review_observation(swarm_id, None);
        observation.swarm.roster.push(MemberConfiguration {
            member_key: "worker-b".to_owned(),
            label: "Worker B".to_owned(),
            provider: "controlled".to_owned(),
            requested_model: "fixture-v1".to_owned(),
            requested_effort: Some("medium".to_owned()),
            configuration_revision: 1,
            moving_alias_acknowledged: false,
            configuration_state: ProviderConfigurationState::ResolutionUnreported,
        });
        observation.activity["roster"] = json!([
            {
                "member_key": affected.member_key,
                "provider": "controlled",
                "configuration_revision": 1,
                "moving_alias_acknowledged": false,
                "requested_model": "fixture-v1",
                "requested_effort": "medium"
            },
            {
                "member_key": unaffected.member_key,
                "provider": "controlled",
                "configuration_revision": 1,
                "moving_alias_acknowledged": false,
                "requested_model": "fixture-v1",
                "requested_effort": "medium"
            }
        ]);
        let WorkerSemanticTarget::WorkAttempt {
            work_id: affected_work,
            attempt_id: affected_attempt,
            ..
        } = affected
            .semantic_target
            .as_ref()
            .unwrap_or_else(|| unreachable!("fixture target"))
        else {
            unreachable!("fixture target")
        };
        let WorkerSemanticTarget::WorkAttempt {
            work_id: unaffected_work,
            attempt_id: unaffected_attempt,
            ..
        } = unaffected
            .semantic_target
            .as_ref()
            .unwrap_or_else(|| unreachable!("fixture target"))
        else {
            unreachable!("fixture target")
        };
        observation.activity["work_items"] = json!([
            {
                "work_id": affected_work,
                "revision": 3,
                "execution_epoch": 1,
                "kind": "goal",
                "status": "blocked",
                "owner_member_id": null,
                "active_attempt_id": null
            },
            {
                "work_id": unaffected_work,
                "revision": 2,
                "execution_epoch": 1,
                "kind": "goal",
                "status": "claimed",
                "owner_member_id": member_id_for(&unaffected.member_key),
                "active_attempt_id": unaffected_attempt
            }
        ]);
        observation.activity["work_attempts"] = json!([
            {
                "attempt_id": affected_attempt,
                "revision": 2,
                "work_id": affected_work,
                "execution_epoch": 1,
                "owner_member_id": member_id_for(&affected.member_key),
                "status": "interruption_requested"
            },
            {
                "attempt_id": unaffected_attempt,
                "revision": 1,
                "work_id": unaffected_work,
                "execution_epoch": 1,
                "owner_member_id": member_id_for(&unaffected.member_key),
                "status": "active"
            }
        ]);
        observation
    }

    fn review_observation(swarm_id: &SwarmId, status: Option<&str>) -> SwarmObservation {
        let (reviews, work_items, work_attempts) = review_facts(status);
        let member = MemberConfiguration {
            member_key: "worker-a".to_owned(),
            label: "Worker A".to_owned(),
            provider: "controlled".to_owned(),
            requested_model: "fixture-v1".to_owned(),
            requested_effort: Some("medium".to_owned()),
            configuration_revision: 1,
            moving_alias_acknowledged: false,
            configuration_state: ProviderConfigurationState::ResolutionUnreported,
        };
        SwarmObservation {
            swarm: SwarmView {
                swarm_id: swarm_id.clone(),
                room_id: "room-service".to_owned(),
                goal: "Exercise Progress Reviews".to_owned(),
                constraints: Vec::new(),
                acceptance_criteria: vec![AcceptanceCriterion {
                    text: "Review progress".to_owned(),
                }],
                working_area: PathBuf::from("/tmp/agent-swarm-service"),
                roster: vec![member],
                progress_review_interval_seconds: 300,
                correction_failure_limit: 3,
                source_label: "test".to_owned(),
            },
            actor: SwarmActor::HumanCoordinator,
            member_id: "human-1".to_owned(),
            room_seq: 7,
            authoritative_state_hash: "blake3:test".to_owned(),
            action_offers: Vec::new(),
            activity: json!({
                "phase": "open",
                "execution_epoch": 1,
                "roster": [{
                    "member_id": "member-1",
                    "member_key": "worker-a",
                    "provider": "controlled",
                    "configuration_revision": 1,
                    "moving_alias_acknowledged": false,
                    "requested_model": "fixture-v1",
                    "requested_effort": "medium"
                }],
                "outstanding_progress_reviews": reviews,
                "work_items": work_items,
                "work_attempts": work_attempts
            }),
        }
    }

    fn review_facts(status: Option<&str>) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
        match status {
            Some("due") => (
                vec![json!({
                    "review_id": "progress-review-1-1",
                    "revision": 1,
                    "execution_epoch": 1,
                    "work_id": "progress-work-1-1",
                    "status": "due"
                })],
                vec![json!({
                    "work_id": "progress-work-1-1",
                    "revision": 1,
                    "execution_epoch": 1,
                    "kind": "progress_review",
                    "status": "open",
                    "owner_member_id": null,
                    "active_attempt_id": null
                })],
                Vec::new(),
            ),
            Some("claimed") => (
                vec![json!({
                    "review_id": "progress-review-1-1",
                    "revision": 2,
                    "execution_epoch": 1,
                    "work_id": "progress-work-1-1",
                    "status": "claimed"
                })],
                vec![json!({
                    "work_id": "progress-work-1-1",
                    "revision": 2,
                    "execution_epoch": 1,
                    "kind": "progress_review",
                    "status": "claimed",
                    "owner_member_id": "member-1",
                    "active_attempt_id": "attempt-review-1"
                })],
                vec![json!({
                    "attempt_id": "attempt-review-1",
                    "revision": 1,
                    "work_id": "progress-work-1-1",
                    "execution_epoch": 1,
                    "owner_member_id": "member-1",
                    "status": "active"
                })],
            ),
            Some("blocked") => (
                vec![json!({
                    "review_id": "progress-review-1-1",
                    "revision": 2,
                    "execution_epoch": 1,
                    "work_id": "progress-work-1-1",
                    "status": "blocked"
                })],
                vec![json!({
                    "work_id": "progress-work-1-1",
                    "revision": 2,
                    "execution_epoch": 1,
                    "kind": "progress_review",
                    "status": "blocked",
                    "owner_member_id": null,
                    "active_attempt_id": null
                })],
                Vec::new(),
            ),
            Some(other) => unreachable!("unsupported test status {other}"),
            None => (Vec::new(), Vec::new(), Vec::new()),
        }
    }
}
