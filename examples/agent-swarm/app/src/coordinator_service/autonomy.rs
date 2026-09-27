//! Adaptive, model-directed planning with durable budgets. This module builds
//! an authority menu, not a task sequence: only a retained model decision may
//! select work, assign a member, revise dependencies, retry, or wait.

use super::*;
use crate::{
    domain::AutonomyView,
    planning::{PlanningDecisionKind, SelectedStep},
};

const INSTRUCTION_SCHEMA: &str = "worldstream/agent-swarm-planning-instruction@1";

mod delivery;
pub use delivery::{DeliveryCheckPolicy, DeliveryPolicy};

/// Explicit local execution limits for an adaptive run. Model/provider/effort
/// always come from the Human-confirmed roster, never from a planner answer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AutonomyPolicy {
    pub max_work_items: usize,
    /// Counts reasoning, selected workers, and automatic Progress Reviews
    /// generated after this policy is enabled.
    pub invocation_limit: usize,
    pub resource_policy: ResourcePolicy,
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<DeliveryPolicy>,
}

impl AutonomyPolicy {
    fn validate(&self) -> Result<(), CoordinatorServiceError> {
        if !(1..=32).contains(&self.max_work_items)
            || !(1..=256).contains(&self.invocation_limit)
            || self.allowed_tools.len() > 64
            || self
                .allowed_tools
                .iter()
                .any(|tool| !valid_identifier(tool))
            || self.allowed_tools.iter().collect::<BTreeSet<_>>().len() != self.allowed_tools.len()
        {
            return Err(CoordinatorServiceError::InvalidState);
        }
        if let Some(delivery) = &self.delivery {
            delivery.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AutonomyState {
    policy: AutonomyPolicy,
    execution_epoch: u64,
    namespace: String,
    invocation_ids: BTreeSet<String>,
    consumed_decisions: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    rejected_decisions: BTreeMap<String, RejectedDecision>,
    proposed_work_ids: BTreeSet<String>,
    wait_basis: Option<(u64, String)>,
    halted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery: Option<delivery::DeliveryState>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RejectedDecision {
    decision: crate::planning::PlanningDecision,
    feedback: String,
}

impl AutonomyState {
    pub(super) fn owns_invocation(&self, invocation_id: &str) -> bool {
        self.invocation_ids.contains(invocation_id)
    }

    pub(super) fn may_generate(&self) -> bool {
        !self.halted && self.invocation_ids.len() < self.policy.invocation_limit
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlanningOption {
    target_id: String,
    member_key: String,
    semantic_target: WorkerSemanticTarget,
    allowed_action_types: Vec<String>,
    instruction: String,
    resource_policy: ResourcePolicy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlanningInstruction {
    schema: String,
    instructions: String,
    options: Vec<PlanningOption>,
    recent_outcomes: Vec<CoordinatorInvocationStatus>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    rejected_decisions: BTreeMap<String, RejectedDecision>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    worker_feedback: BTreeMap<String, String>,
    remaining_invocations: usize,
    max_work_items: usize,
    work_revisions: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    delivery_options: Vec<delivery::DeliveryOption>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    delivery_evidence: Value,
}

impl CoordinatorService {
    /// Enables adaptive reasoning for this execution epoch. Repeating the
    /// identical policy is idempotent and never replenishes its budget.
    ///
    /// # Errors
    /// Rejects unconfirmed setup, invalid limits, a changed retained policy,
    /// or unavailable authoritative state. This never resumes execution.
    pub fn enable_autonomy(
        &mut self,
        policy: AutonomyPolicy,
    ) -> Result<(), CoordinatorServiceError> {
        policy.validate()?;
        if let Some(retained) = &self.state.autonomy {
            return if retained.policy == policy {
                Ok(())
            } else {
                Err(CoordinatorServiceError::Conflict)
            };
        }
        let observation = self.authoritative_observation()?;
        if observation.activity.get("phase").and_then(Value::as_str) != Some("open") {
            return Err(CoordinatorServiceError::InvalidObservation);
        }
        let epoch = required_revision(&observation.activity, "execution_epoch")?;
        let delivery = policy
            .delivery
            .as_ref()
            .map(|policy| delivery::DeliveryState::bind(policy, &observation, &self.journal.root))
            .transpose()?;
        let identity = serde_json::to_vec(&json!([self.state.swarm_id, epoch, policy]))
            .map_err(|_| CoordinatorServiceError::InvalidState)?;
        let namespace = blake3::hash(&identity).to_hex()[..16].to_owned();
        let max_work_items = policy.max_work_items;
        let invocation_limit = policy.invocation_limit;
        self.mutate(|state| {
            state.autonomy = Some(AutonomyState {
                policy,
                execution_epoch: epoch,
                namespace,
                invocation_ids: BTreeSet::new(),
                consumed_decisions: BTreeSet::new(),
                rejected_decisions: BTreeMap::new(),
                proposed_work_ids: BTreeSet::new(),
                wait_basis: None,
                halted: false,
                delivery,
            });
            state.view.autonomy = Some(AutonomyView {
                phase: "enabled".to_owned(),
                max_work_items,
                invocation_limit,
                generated_work_count: 0,
                generated_invocation_count: 0,
                reason: "Adaptive planning enabled; execution still requires explicit Resume."
                    .to_owned(),
            });
        })
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn advance_autonomy(
        &mut self,
        observation: &SwarmObservation,
    ) -> Result<(), CoordinatorServiceError> {
        let Some(retained) = self.state.autonomy.clone() else {
            return Ok(());
        };
        if retained.halted {
            return Ok(());
        }
        if observation.activity["phase"].as_str() == Some("completed") {
            return self.autonomy_status("completed", "WorldStream accepted the Result.");
        }
        if self.delivery_interrupted() {
            return self.halt_autonomy("A delivery operation was interrupted. Reconcile its retained exact effects before further work; no automatic retry was made.");
        }
        if !self.delivery_scope_current(observation) {
            return self.halt_autonomy("The goal, Direction, criteria or resource basis changed after delivery authorization. Reauthorize against the new scope.");
        }
        if observation.activity.get("phase").and_then(Value::as_str) != Some("open")
            || observation
                .activity
                .get("execution_epoch")
                .and_then(Value::as_u64)
                != Some(retained.execution_epoch)
        {
            return self.autonomy_status(
                "needs_attention",
                "The confirmed execution epoch is no longer open.",
            );
        }
        // Do not reinterpret a model decision while any selected effect is
        // pending. A decision may dispatch multiple independent Work Attempts.
        if self.state.view.invocations.iter().any(|status| {
            outcome_is_pending(&status.coordinator_outcome)
                || outcome_blocks_automatic_progress(&status.coordinator_outcome)
        }) {
            return self.autonomy_status(
                "executing",
                "Waiting for retained Invocations to settle before reassessment.",
            );
        }
        if self.consume_planning_decision(observation)? {
            return Ok(());
        }
        let retained = self
            .state
            .autonomy
            .clone()
            .ok_or(CoordinatorServiceError::InvalidState)?;
        if retained.wait_basis.as_ref()
            == Some(&(
                observation.room_seq,
                observation.authoritative_state_hash.clone(),
            ))
        {
            return Ok(());
        }
        if retained.invocation_ids.len() >= retained.policy.invocation_limit {
            return self.autonomy_status(
                "budget_exhausted",
                "Invocation budget exhausted; this is not goal completion.",
            );
        }
        if self.autonomy_failure_streak() >= 3 {
            return self.halt_autonomy(
                "Three consecutive failed autonomous Invocations require human attention.",
            );
        }
        let profiles = member_execution_profiles(observation, &self.state.plans)?;
        let options = self.planning_options(observation, &profiles)?;
        let sequence = retained.invocation_ids.len() + 1;
        let instruction = PlanningInstruction {
            schema: INSTRUCTION_SCHEMA.to_owned(),
            instructions: concat!(
                "Reason about the accepted goal, constraints, evidence, dependencies, blockers and recent outcomes. ",
                "Choose the next useful steps from the authority options; these are possibilities, not a prescribed workflow. ",
                "An offered Action establishes authority, not semantic readiness. Before selecting work, identify each prerequisite from the accepted goal and Work Item, and cite observed evidence satisfying it in the decision reason. ",
                "A missing dependency_id does not waive a natural-language ordering constraint. If prerequisite evidence is absent, choose work that establishes it, revise dependencies when justified, or report a blocker. ",
                "Decompose work only when useful, adjust dependencies, retry with a revised approach after a known failure, ",
                "or wait/handoff when appropriate. Do not repeat completed work. Independent owned Work Attempts can run in parallel ",
                "on distinct members. Parallelism is useful only among semantically ready work; required evidence and order outrank concurrency. ",
                "Execution is batch-barriered: the coordinator will NOT plan again until ALL selected steps finish. ",
                "Starting one WorkAttempt alone therefore serializes it with any unclaimed work. ",
                "When independent work is useful, prepare separate ownership claims in separate decisions BEFORE ",
                "dispatching those owned WorkAttempts together. Consider ready independent work before starting long computation. ",
                "Proposals, claims and dependency edits require exactly ONE step in their decision; ",
                "the offered member alternatives for a proposal share one Work ID and are not separate tasks. ",
                "Correct rejected_decisions and worker_feedback using their exact validation feedback; no part of a rejected batch was dispatched. ",
                "At planning-step level OMIT dependency_ids entirely except for dependency revision options (even [] is invalid). ",
                "A work proposal's payload may have dependency_ids; that is a separate object inside the worker action. ",
                "For each step copy its target_id and member_key and author a complete worker instruction, using the option's instruction ",
                "as the action contract. A dependency revision also supplies dependency_ids from work_revisions. ",
                "When delivery_options are present, operate selects exactly one target_id and reason; it supplies no command or payload. ",
                "Use integration Work Items to combine Contributions into Candidates, request configured checks, select independent review, ",
                "revise after failed evidence, resolve blocking findings through an eligible independent member, and deliver when ready. ",
                "Do not wait for an external delivery driver: this planner owns continuation when a delivery policy is enabled. ",
                "Return the full envelope with schema worldstream/agent-swarm-planning-decision@1 and decision. ",
                "Dispatch has steps and reason; operate has target_id and reason; wait/handoff have reason. ",
                "Wait means no useful action now, not an accepted Result. You cannot expand permissions or change the roster."
            ).to_owned(),
            options,
            recent_outcomes: self.state.view.invocations.iter().rev().take(24).cloned().collect(),
            rejected_decisions: retained.rejected_decisions.iter().rev().take(3)
                .map(|(id, rejection)| (id.clone(), rejection.clone())).collect(),
            worker_feedback: self.recent_worker_feedback(),
            remaining_invocations: retained.policy.invocation_limit - retained.invocation_ids.len() - 1,
            max_work_items: retained.policy.max_work_items,
            work_revisions: work_revisions(observation)?,
            delivery_options: self.delivery_options(observation)?,
            delivery_evidence: self.delivery_feedback()?,
        };
        let target = WorkerSemanticTarget::Planning {
            execution_epoch: retained.execution_epoch,
            goal_revision: required_revision(&observation.activity, "goal_revision")?,
            direction_revision: observation
                .activity
                .get("direction_revision")
                .and_then(Value::as_u64)
                .ok_or(CoordinatorServiceError::InvalidObservation)?,
            room_seq: observation.room_seq,
            authoritative_state_hash: observation.authoritative_state_hash.clone(),
        };
        for profile in profiles.values() {
            let plan = make_plan(
                profile,
                target.clone(),
                Vec::new(),
                ResourcePolicy::ReadOnly,
                Vec::new(),
                format!("auto-{}-{sequence:04}-reason", retained.namespace),
                serde_json::to_string(&instruction)
                    .map_err(|_| CoordinatorServiceError::InvalidState)?,
                observation.room_seq,
            );
            if self.driver.preflight(&plan).is_ok() {
                self.retain_autonomy_batch(vec![plan], None)?;
                return self.autonomy_status(
                    "reasoning",
                    "A roster LLM is choosing the next steps from current Room evidence.",
                );
            }
        }
        self.autonomy_status(
            "no_eligible_provider",
            "No roster provider currently satisfies the qualified read-only planning contract.",
        )
    }

    fn consume_planning_decision(
        &mut self,
        observation: &SwarmObservation,
    ) -> Result<bool, CoordinatorServiceError> {
        let state = self
            .state
            .autonomy
            .clone()
            .ok_or(CoordinatorServiceError::InvalidState)?;
        let pending = state
            .invocation_ids
            .iter()
            .filter(|id| !state.consumed_decisions.contains(*id))
            .filter_map(|id| self.state.plans.get(id))
            .find(|plan| {
                matches!(
                    plan.semantic_target,
                    Some(WorkerSemanticTarget::Planning { .. })
                )
            })
            .cloned();
        let Some(plan) = pending else {
            return Ok(false);
        };
        let intent = match self.driver.intent(&plan.invocation_id) {
            Ok(intent) => intent,
            Err(CoordinatorError::NotFound) => {
                self.retain_autonomy_batch(Vec::new(), Some(&plan.invocation_id))?;
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        };
        let SubmissionDisposition::PlanningDecision(decision) = intent.submission else {
            // A failed or stale reasoning turn has no decisions to apply. A
            // fresh bounded Invocation may reconsider its retained evidence.
            self.mutate(|state| {
                if let Some(autonomy) = &mut state.autonomy {
                    autonomy
                        .consumed_decisions
                        .insert(plan.invocation_id.clone());
                }
            })?;
            return Ok(false);
        };
        let Some(WorkerSemanticTarget::Planning {
            room_seq,
            authoritative_state_hash,
            ..
        }) = &plan.semantic_target
        else {
            return Err(CoordinatorServiceError::InvalidState);
        };
        if observation.room_seq != *room_seq
            || observation.authoritative_state_hash != *authoritative_state_hash
        {
            self.retain_autonomy_batch(Vec::new(), Some(&plan.invocation_id))?;
            self.autonomy_status(
                "reasoning",
                "The Room advanced after reasoning; reassessment is required.",
            )?;
            return Ok(true);
        }
        match decision.decision.clone() {
            PlanningDecisionKind::Operate { target_id, .. } => {
                self.consume_delivery_option(observation, &plan, decision, &target_id)?;
            }
            PlanningDecisionKind::Wait { reason } => {
                self.retain_autonomy_batch(Vec::new(), Some(&plan.invocation_id))?;
                self.mutate(|state| {
                    if let Some(autonomy) = &mut state.autonomy {
                        autonomy.wait_basis = Some((
                            observation.room_seq,
                            observation.authoritative_state_hash.clone(),
                        ));
                    }
                })?;
                self.autonomy_status("waiting", &reason)?;
            }
            PlanningDecisionKind::Handoff { reason } => {
                self.retain_autonomy_batch(Vec::new(), Some(&plan.invocation_id))?;
                self.halt_autonomy(&reason)?;
            }
            PlanningDecisionKind::Dispatch { steps, reason } => {
                let plans = self.selected_plans(observation, &plan, &steps);
                match plans {
                    Ok(plans)
                        if state.invocation_ids.len() + plans.len()
                            <= state.policy.invocation_limit =>
                    {
                        self.retain_autonomy_batch(plans, Some(&plan.invocation_id))?;
                        self.autonomy_status("executing", &reason)?;
                    }
                    Ok(_) => {
                        self.retain_autonomy_batch(Vec::new(), Some(&plan.invocation_id))?;
                        self.autonomy_status("budget_exhausted", "The selected batch exceeds the remaining Invocation budget; no part was dispatched.")?;
                        self.mutate(|state| {
                            if let Some(autonomy) = &mut state.autonomy {
                                autonomy.halted = true;
                            }
                        })?;
                    }
                    Err(feedback) => self.retain_rejected_decision(&plan, decision, &feedback)?,
                }
            }
        }
        Ok(true)
    }

    fn retain_rejected_decision(
        &mut self,
        plan: &WorkerActionPlan,
        decision: crate::planning::PlanningDecision,
        feedback: &str,
    ) -> Result<(), CoordinatorServiceError> {
        self.mutate(|state| {
            if let Some(autonomy) = &mut state.autonomy {
                autonomy
                    .consumed_decisions
                    .insert(plan.invocation_id.clone());
                autonomy.wait_basis = None;
                autonomy.rejected_decisions.insert(
                    plan.invocation_id.clone(),
                    RejectedDecision {
                        decision,
                        feedback: feedback.to_owned(),
                    },
                );
            }
        })?;
        self.autonomy_status("reasoning", feedback)
    }

    fn recent_worker_feedback(&self) -> BTreeMap<String, String> {
        self.state.view.invocations.iter().rev().take(24).filter_map(|status| {
            let plan = self.state.plans.get(&status.invocation_id)?;
            let intent = self.driver.intent(&status.invocation_id).ok()?;
            if intent.submission != SubmissionDisposition::NotSubmitted(NonSubmissionReason::ProviderOutputInvalid) {
                return None;
            }
            if let Some(feedback) = intent.validation_feedback {
                return Some((status.invocation_id.clone(), feedback));
            }
            let output = intent.evidence?.output?;
            if matches!(plan.semantic_target, Some(WorkerSemanticTarget::Planning { .. })) {
                return Some((status.invocation_id.clone(), if serde_json::from_str::<Value>(&output.text).ok()?.get("schema").and_then(Value::as_str)
                    == Some(crate::planning::PLANNING_DECISION_SCHEMA) {
                    "Invalid planning decision. Follow the complete response contract, with exactly one decision and only the fields allowed for its kind."
                } else {
                    "Missing or wrong top-level schema field. Return {\"schema\":\"worldstream/agent-swarm-planning-decision@1\",\"decision\":{...}}. A valid JSON decision alone is insufficient."
                }.to_owned()));
            }
            if serde_json::from_str::<Value>(&output.text).ok()?.get("schema").and_then(Value::as_str)
                != Some(crate::execution::ACTION_PROPOSAL_SCHEMA) {
                return Some((status.invocation_id.clone(), "Missing or wrong top-level schema field. Include \"schema\":\"worldstream/agent-swarm-action-proposal@1\" alongside action_type and payload. A valid JSON object alone is insufficient.".to_owned()));
            }
            let feedback = crate::execution::decode_action_proposal(&output.text).map_or(
                "Output was not a valid action-proposal JSON object. Follow the exact response contract.",
                |proposal| crate::coordinator::proposal_rejection_feedback(plan, &proposal).unwrap_or(
                    "The returned action could not be bound to a current offered action."));
            Some((status.invocation_id.clone(), feedback.to_owned()))
        }).collect()
    }

    fn selected_plans(
        &self,
        observation: &SwarmObservation,
        planning: &WorkerActionPlan,
        steps: &[SelectedStep],
    ) -> Result<Vec<WorkerActionPlan>, String> {
        let instruction: PlanningInstruction = serde_json::from_str(&planning.instruction)
            .map_err(|_| "Retained planning instruction is invalid.".to_owned())?;
        let state = self
            .state
            .autonomy
            .as_ref()
            .ok_or_else(|| "Retained autonomy state is unavailable.".to_owned())?;
        let profiles = member_execution_profiles(observation, &self.state.plans)
            .map_err(|error| error.to_string())?;
        let mut plans = Vec::new();
        let mut work_ids = BTreeSet::new();
        for (index, step) in steps.iter().enumerate() {
            let option = instruction
                .options
                .iter()
                .find(|option| {
                    option.target_id == step.target_id && option.member_key == step.member_key
                })
                .ok_or_else(|| {
                    "Unknown target_id/member_key pair: copy an exact pair from current options."
                        .to_owned()
                })?;
            let mut target = option.semantic_target.clone();
            if let WorkerSemanticTarget::WorkDependencyRevision {
                dependency_revisions,
                ..
            } = &mut target
            {
                let ids = step.dependency_ids.as_ref().ok_or_else(|| {
                    "Dependency revision requires step.dependency_ids with offered Work IDs."
                        .to_owned()
                })?;
                *dependency_revisions = ids
                    .iter()
                    .map(|id| {
                        instruction
                            .work_revisions
                            .get(id)
                            .map(|revision| (id.clone(), *revision))
                            .ok_or_else(|| {
                                "Unknown Work ID in dependency_ids; use work_revisions.".to_owned()
                            })
                    })
                    .collect::<Result<_, _>>()?;
            } else if step.dependency_ids.is_some() {
                return Err("Remove step.dependency_ids entirely: this option is not a dependency revision. An empty array is also invalid. The proposed work payload dependencies belong inside the worker instruction, not the planning step.".to_owned());
            }
            if steps.len() > 1 {
                let WorkerSemanticTarget::WorkAttempt { work_id, .. } = &target else {
                    return Err("Metadata actions must be the sole step of a decision. Only distinct owned WorkAttempts may be dispatched in parallel; no part of this batch was dispatched.".to_owned());
                };
                if !work_ids.insert(work_id.clone()) {
                    return Err("Parallel WorkAttempts must target distinct Work IDs.".to_owned());
                }
            }
            let mut profile = profiles
                .get(&step.member_key)
                .cloned()
                .ok_or_else(|| "Selected member has no current execution profile.".to_owned())?;
            // Preserve reviewer isolation when materializing the selected
            // option; a roster member's older Resume plan is not authority to
            // carry its conversation into an independent delivery judgment.
            if state.policy.delivery.is_some()
                && matches!(
                    &target,
                    WorkerSemanticTarget::CandidateReview { .. }
                        | WorkerSemanticTarget::FindingResolution { .. }
                )
            {
                profile.session = SessionSelection::Fresh { requested_id: None };
            }
            let plan = make_plan(
                &profile,
                target,
                option.allowed_action_types.clone(),
                option.resource_policy,
                if option.resource_policy == ResourcePolicy::ReadOnly {
                    Vec::new()
                } else {
                    state.policy.allowed_tools.clone()
                },
                format!("{}-step-{index}", planning.invocation_id),
                serde_json::to_string(&json!({
                    "action_contract": option.instruction,
                    "model_instruction": step.instruction,
                    "notice": "Follow the action contract and required response envelope while carrying out the model instruction."
                })).map_err(|error| error.to_string())?,
                observation.room_seq,
            );
            validate_plan(&plan)
                .map_err(|error| format!("Selected worker plan is invalid: {error}"))?;
            self.driver
                .preflight(&plan)
                .map_err(|error| format!("Selected worker preflight failed: {error}"))?;
            plans.push(plan);
        }
        Ok(plans)
    }

    pub(super) fn retain_autonomy_batch(
        &mut self,
        plans: Vec<WorkerActionPlan>,
        consumed: Option<&str>,
    ) -> Result<(), CoordinatorServiceError> {
        self.mutate(|state| {
            if let Some(autonomy) = &mut state.autonomy {
                for plan in plans {
                    if let Some(WorkerSemanticTarget::WorkProposal { work_id, .. }) =
                        &plan.semantic_target
                    {
                        autonomy.proposed_work_ids.insert(work_id.clone());
                    }
                    autonomy.invocation_ids.insert(plan.invocation_id.clone());
                    state.known_invocations.insert(plan.invocation_id.clone());
                    upsert_status(&mut state.view.invocations, status_from_plan(&plan));
                    state.plans.insert(plan.invocation_id.clone(), plan);
                }
                if let Some(id) = consumed {
                    autonomy.consumed_decisions.insert(id.to_owned());
                }
                autonomy.wait_basis = None;
            }
        })
    }

    fn autonomy_status(
        &mut self,
        phase: &str,
        reason: &str,
    ) -> Result<(), CoordinatorServiceError> {
        self.mutate(|state| {
            if let (Some(retained), Some(view)) = (&state.autonomy, &mut state.view.autonomy) {
                phase.clone_into(&mut view.phase);
                reason.clone_into(&mut view.reason);
                view.generated_work_count = retained.proposed_work_ids.len();
                view.generated_invocation_count = retained.invocation_ids.len();
            }
        })
    }

    fn halt_autonomy(&mut self, reason: &str) -> Result<(), CoordinatorServiceError> {
        self.mutate(|state| {
            if let Some(retained) = &mut state.autonomy {
                retained.halted = true;
            }
        })?;
        self.autonomy_status("needs_attention", reason)
    }

    fn autonomy_failure_streak(&self) -> usize {
        let Some(autonomy) = &self.state.autonomy else {
            return 0;
        };
        // Sorted IDs have a monotonic run ordinal. Planning decisions do not
        // erase worker failure history; only accepted worker Actions do.
        autonomy
            .invocation_ids
            .iter()
            .rev()
            .filter_map(|id| {
                if self.state.plans.get(id)?.kind != InvocationKind::Work {
                    return None;
                }
                self.state
                    .view
                    .invocations
                    .iter()
                    .find(|status| &status.invocation_id == id)
            })
            .take_while(|status| {
                !matches!(
                    status.coordinator_outcome,
                    CoordinatorOutcome::Accepted { .. }
                )
            })
            .filter(|status| {
                autonomy
                    .rejected_decisions
                    .contains_key(&status.invocation_id)
                    || matches!(
                        status.coordinator_outcome,
                        CoordinatorOutcome::Rejected { .. }
                            | CoordinatorOutcome::NotSubmitted { .. }
                            | CoordinatorOutcome::NeedsReevaluation
                    )
            })
            .count()
    }
}

impl CoordinatorService {
    #[allow(clippy::too_many_lines)]
    fn planning_options(
        &self,
        observation: &SwarmObservation,
        profiles: &BTreeMap<String, MemberExecutionProfile>,
    ) -> Result<Vec<PlanningOption>, CoordinatorServiceError> {
        let state = self
            .state
            .autonomy
            .as_ref()
            .ok_or(CoordinatorServiceError::InvalidState)?;
        let members = self.authenticated_worker_member_keys(observation)?;
        let activity = observation
            .activity
            .as_object()
            .ok_or(CoordinatorServiceError::InvalidObservation)?;
        let works = bounded_array(activity.get("work_items"), MAX_DISCOVERED_WORK_ITEMS)?;
        let attempts = bounded_array(activity.get("work_attempts"), MAX_DISCOVERED_WORK_ATTEMPTS)?;
        let goal_revision = required_revision(&observation.activity, "goal_revision")?;
        let epoch = state.execution_epoch;
        let mut options = Vec::new();
        for profile in profiles.values() {
            let member_id = members
                .iter()
                .find_map(|(id, key)| (key == &profile.member_key).then_some(id))
                .ok_or(CoordinatorServiceError::InvalidObservation)?;
            if let Some((index, work_id)) = (1..=state.policy.max_work_items)
                .map(|index| (index, format!("auto-work-{}-{index}", state.namespace)))
                .find(|(_, id)| {
                    !works
                        .iter()
                        .any(|work| work.get("work_id").and_then(Value::as_str) == Some(id))
                })
            {
                let target = WorkerSemanticTarget::WorkProposal {
                    execution_epoch: epoch,
                    goal_revision,
                    work_id,
                };
                self.push_option(&mut options, profile, target, vec!["propose_work_item".to_owned()], ResourcePolicy::ReadOnly, json!({
                    "task":"propose", "proposal_index":index,
                    "instructions":"Propose metadata only: omit artifact entirely and do not implement the deliverable yet. Propose one useful bounded deliverable for the accepted goal. Author its title, description, kind and dependencies from current evidence; include acceptance expectations in the description. Do not duplicate completed work. Payload fields: work_id, expected_goal_revision, kind (goal/integration/correction), title, description, dependency_ids."
                }), observation.room_seq)?;
            }
            let has_claim = works.iter().any(|work| {
                work.get("execution_epoch").and_then(Value::as_u64) == Some(epoch)
                    && work.get("status").and_then(Value::as_str) == Some("claimed")
                    && work.get("owner_member_id").and_then(Value::as_str) == Some(member_id)
            });
            for work in works {
                if work.get("execution_epoch").and_then(Value::as_u64) != Some(epoch)
                    || !matches!(
                        work.get("kind").and_then(Value::as_str),
                        Some("goal" | "integration" | "correction")
                    )
                {
                    continue;
                }
                let work_id = required_identifier(work, "work_id")?.to_owned();
                // Automatically operate only on work created inside this run.
                if !state.proposed_work_ids.contains(&work_id) {
                    continue;
                }
                let work_revision = required_revision(work, "revision")?;
                let unowned = work.get("owner_member_id").is_some_and(Value::is_null)
                    && work.get("active_attempt_id").is_some_and(Value::is_null);
                if work.get("status").and_then(Value::as_str) == Some("open") && unowned {
                    let dependencies = work
                        .get("dependency_ids")
                        .and_then(Value::as_array)
                        .ok_or(CoordinatorServiceError::InvalidObservation)?;
                    let dependency_revisions = dependencies
                        .iter()
                        .map(|id| {
                            let id = id
                                .as_str()
                                .ok_or(CoordinatorServiceError::InvalidObservation)?;
                            let dependency = works
                                .iter()
                                .find(|work| {
                                    work.get("work_id").and_then(Value::as_str) == Some(id)
                                })
                                .ok_or(CoordinatorServiceError::InvalidObservation)?;
                            Ok((id.to_owned(), required_revision(dependency, "revision")?))
                        })
                        .collect::<Result<BTreeMap<_, _>, CoordinatorServiceError>>()?;
                    self.push_option(&mut options, profile, WorkerSemanticTarget::WorkDependencyRevision { execution_epoch:epoch, work_id:work_id.clone(), work_revision, dependency_revisions },
                        vec!["revise_work_dependencies".to_owned()], ResourcePolicy::ReadOnly, json!({"task":"revise_dependencies","instructions":"Revise this unclaimed Work Item's dependencies only if current evidence justifies it. Payload: work_id, expected_work_revision, dependency_ids exactly matching the selected planning dependencies."}), observation.room_seq)?;
                    let ready = dependencies.iter().all(|id| {
                        works.iter().any(|work| {
                            work.get("work_id") == Some(id)
                                && work.get("status").and_then(Value::as_str) == Some("completed")
                        })
                    });
                    if !has_claim
                        && ready
                        && crate::coordinator::work_attempt_may_launch(activity, &work_id)
                    {
                        let target = WorkerSemanticTarget::WorkClaim {
                            execution_epoch: epoch,
                            work_id: work_id.clone(),
                            work_revision,
                        };
                        self.push_option(&mut options, profile, target, vec!["claim_work_item".to_owned()], ResourcePolicy::ReadOnly, json!({
                            "task":"claim", "attempt_id":format!("attempt-{}-{}-{}", state.namespace, work_id, state.invocation_ids.len()),
                            "instructions":"Claim this exact currently eligible Work Item. Payload: work_id, expected_work_revision, attempt_id. Use the supplied attempt_id."
                        }), observation.room_seq)?;
                    }
                }
                if work.get("status").and_then(Value::as_str) == Some("claimed")
                    && work.get("owner_member_id").and_then(Value::as_str) == Some(member_id)
                    && crate::coordinator::work_attempt_may_launch(activity, &work_id)
                {
                    let attempt_id = required_identifier(work, "active_attempt_id")?.to_owned();
                    let attempt = attempts
                        .iter()
                        .find(|attempt| {
                            attempt.get("attempt_id").and_then(Value::as_str) == Some(&attempt_id)
                        })
                        .ok_or(CoordinatorServiceError::InvalidObservation)?;
                    if attempt.get("status").and_then(Value::as_str) != Some("active") {
                        continue;
                    }
                    let attempt_revision = required_revision(attempt, "revision")?;
                    if state.policy.delivery.is_some()
                        && work["kind"].as_str() == Some("integration")
                    {
                        self.push_delivery_candidate_option(
                            &mut options,
                            profile,
                            observation,
                            WorkerSemanticTarget::WorkAttempt {
                                execution_epoch: epoch,
                                work_id: work_id.clone(),
                                work_revision,
                                attempt_id,
                                attempt_revision,
                            },
                        )?;
                        continue;
                    }
                    self.push_option(&mut options, profile, WorkerSemanticTarget::WorkAttempt { execution_epoch:epoch, work_id:work_id.clone(), work_revision, attempt_id, attempt_revision },
                        vec!["submit_contribution".to_owned(), "report_work_blocker".to_owned()], state.policy.resource_policy, json!({
                            "task":"contribute", "artifact_path":format!(".swarm-contributions/{}/{work_id}-{attempt_revision}.txt", state.namespace),
                            "instructions":"Check the Work Item's stated prerequisites against observed evidence before execution, even when dependency_ids is empty. If prerequisite evidence is missing, report a blocker rather than perform premature work. Perform this exact owned Work Attempt using current evidence and dependency Contributions. Keep new output in the supplied artifact_path; do not edit other workers' files or apply code to the source checkout. Author submit_contribution payload: work_id, expected_work_revision, expected_attempt_revision, contribution_id, completes_work, resource_basis, source_refs, summary; declare artifact separately as {artifact_id,local_path,media_type}. The coordinator supplies the captured digest. If blocked, author report_work_blocker with work_id, expected_work_revision, blocker_id, evidence_refs, summary instead. A Contribution is not an accepted Result."
                        }), observation.room_seq)?;
                }
            }
        }
        self.push_delivery_review_options(&mut options, profiles, observation)?;
        Ok(options)
    }

    #[allow(clippy::too_many_arguments)]
    fn push_option(
        &self,
        options: &mut Vec<PlanningOption>,
        profile: &MemberExecutionProfile,
        target: WorkerSemanticTarget,
        action_types: Vec<String>,
        resource_policy: ResourcePolicy,
        mut instruction: Value,
        room_seq: u64,
    ) -> Result<(), CoordinatorServiceError> {
        instruction["schema"] = json!("worldstream/agent-swarm-autonomy-instruction@1");
        instruction["target"] =
            serde_json::to_value(&target).map_err(|_| CoordinatorServiceError::InvalidState)?;
        let identity = serde_json::to_vec(&json!([profile.member_key, target, action_types]))
            .map_err(|_| CoordinatorServiceError::InvalidState)?;
        let target_id = format!("option-{}", blake3::hash(&identity).to_hex());
        let instruction = serde_json::to_string(&instruction)
            .map_err(|_| CoordinatorServiceError::InvalidState)?;
        let tools = if resource_policy == ResourcePolicy::ReadOnly {
            Vec::new()
        } else {
            self.state
                .autonomy
                .as_ref()
                .ok_or(CoordinatorServiceError::InvalidState)?
                .policy
                .allowed_tools
                .clone()
        };
        let plan = make_plan(
            profile,
            target.clone(),
            action_types.clone(),
            resource_policy,
            tools,
            format!("probe-{target_id}"),
            instruction.clone(),
            room_seq,
        );
        if self.driver.preflight(&plan).is_ok() {
            options.push(PlanningOption {
                target_id,
                member_key: profile.member_key.clone(),
                semantic_target: target,
                allowed_action_types: action_types,
                instruction,
                resource_policy,
            });
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn make_plan(
    profile: &MemberExecutionProfile,
    target: WorkerSemanticTarget,
    allowed_action_types: Vec<String>,
    resource_policy: ResourcePolicy,
    allowed_tools: Vec<String>,
    invocation_id: String,
    instruction: String,
    due_sequence: u64,
) -> WorkerActionPlan {
    WorkerActionPlan {
        invocation_id,
        swarm_id: profile.swarm_id.clone(),
        member_key: profile.member_key.clone(),
        provider: profile.provider,
        configuration_revision: profile.configuration_revision,
        model: profile.model.clone(),
        effort: profile.effort.clone(),
        moving_alias_acknowledged: profile.moving_alias_acknowledged,
        resource_policy,
        allowed_tools,
        session: profile.session.clone(),
        kind: InvocationKind::Work,
        due_sequence,
        semantic_target: Some(target),
        allowed_action_types,
        instruction,
    }
}

fn work_revisions(
    observation: &SwarmObservation,
) -> Result<BTreeMap<String, u64>, CoordinatorServiceError> {
    bounded_array(
        observation.activity.get("work_items"),
        MAX_DISCOVERED_WORK_ITEMS,
    )?
    .iter()
    .map(|work| {
        Ok((
            required_identifier(work, "work_id")?.to_owned(),
            required_revision(work, "revision")?,
        ))
    })
    .collect()
}

pub(super) fn validate_autonomy(state: &ServiceState) -> Result<(), CoordinatorServiceError> {
    let Some(autonomy) = &state.autonomy else {
        return if state.view.autonomy.is_none() {
            Ok(())
        } else {
            Err(CoordinatorServiceError::InvalidState)
        };
    };
    autonomy.policy.validate()?;
    if autonomy.policy.delivery.is_some() != autonomy.delivery.is_some() {
        return Err(CoordinatorServiceError::InvalidState);
    }
    if let Some(delivery) = &autonomy.delivery {
        delivery.validate(autonomy)?;
    }
    let proposed_work_ids = autonomy
        .invocation_ids
        .iter()
        .filter_map(|id| match state.plans.get(id)?.semantic_target.as_ref()? {
            WorkerSemanticTarget::WorkProposal { work_id, .. } => Some(work_id.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if autonomy.execution_epoch == 0
        || autonomy.namespace.len() != 16
        || autonomy.invocation_ids.len() > autonomy.policy.invocation_limit
        || autonomy.proposed_work_ids.len() > autonomy.policy.max_work_items
        || autonomy.proposed_work_ids != proposed_work_ids
        || autonomy.consumed_decisions.iter().any(|id| {
            state.plans.get(id).is_none_or(|plan| {
                !matches!(
                    plan.semantic_target,
                    Some(WorkerSemanticTarget::Planning { .. })
                )
            })
        })
        || !autonomy
            .consumed_decisions
            .is_subset(&autonomy.invocation_ids)
        || autonomy.rejected_decisions.iter().any(|(id, rejected)| {
            !autonomy.consumed_decisions.contains(id)
                || rejected.decision.validate().is_err()
                || rejected.feedback.is_empty()
                || rejected.feedback.len() > 4096
        })
        || autonomy
            .invocation_ids
            .iter()
            .any(|id| !state.plans.contains_key(id))
        || state.view.autonomy.as_ref().is_none_or(|view| {
            view.max_work_items != autonomy.policy.max_work_items
                || view.invocation_limit != autonomy.policy.invocation_limit
        })
    {
        return Err(CoordinatorServiceError::InvalidState);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    mod delivery_tests;
    use std::{cell::RefCell, error::Error, path::PathBuf, rc::Rc};

    use crate::{
        AcceptanceCriterion, CoordinatorIntentState, MemberConfiguration,
        ProviderConfigurationState, SwarmView,
        planning::{PLANNING_DECISION_SCHEMA, PlanningDecision},
    };

    use super::*;

    #[derive(Clone)]
    struct Driver(Rc<RefCell<DriverState>>);

    struct DriverState {
        observation: SwarmObservation,
        intents: BTreeMap<String, CoordinatorIntentView>,
        fail_preflight: bool,
    }

    impl CoordinatorDriver for Driver {
        fn submit_delivery_action(
            &mut self,
            _: &SwarmId,
            action: &crate::ExactSwarmAction,
        ) -> Result<SwarmActionReceipt, CoordinatorError> {
            delivery_tests::submit_fixture_action(self, action)
        }
        fn stage(
            &mut self,
            plan: WorkerActionPlan,
        ) -> Result<CoordinatorIntentView, CoordinatorError> {
            let intent = intent_for(&plan, SubmissionDisposition::NotAttempted);
            self.0
                .borrow_mut()
                .intents
                .insert(plan.invocation_id, intent.clone());
            Ok(intent)
        }

        fn dispatch(&mut self) -> Result<Vec<CoordinatorEvent>, CoordinatorError> {
            Ok(Vec::new())
        }

        fn harvest(&mut self) -> Result<Vec<CoordinatorEvent>, CoordinatorError> {
            Ok(Vec::new())
        }

        fn retry_action(&mut self, _: &str) -> Result<CoordinatorEvent, CoordinatorError> {
            Err(CoordinatorError::Conflict)
        }

        fn retry_launch(&mut self, _: &str) -> Result<CoordinatorEvent, CoordinatorError> {
            Err(CoordinatorError::Conflict)
        }

        fn intent(&self, id: &str) -> Result<CoordinatorIntentView, CoordinatorError> {
            self.0
                .borrow()
                .intents
                .get(id)
                .cloned()
                .ok_or(CoordinatorError::NotFound)
        }

        fn reconcile_interruption(
            &mut self,
            _: &str,
            _: InvocationResolution,
        ) -> Result<CoordinatorEvent, CoordinatorError> {
            Err(CoordinatorError::Conflict)
        }

        fn observe(
            &self,
            _: &SwarmId,
            actor: &SwarmActor,
        ) -> Result<SwarmObservation, CoordinatorError> {
            let mut observation = self.0.borrow().observation.clone();
            observation.actor = actor.clone();
            observation.member_id = match actor {
                SwarmActor::HumanCoordinator => "human".to_owned(),
                SwarmActor::Worker { member_key } => format!("member-{member_key}"),
            };
            Ok(observation)
        }

        fn preflight(&self, plan: &WorkerActionPlan) -> Result<(), CoordinatorError> {
            validate_plan(plan)?;
            let state = self.0.borrow();
            if state.fail_preflight {
                return Err(CoordinatorError::ProviderBlocked);
            }
            let member = state
                .observation
                .swarm
                .roster
                .iter()
                .find(|member| member.member_key == plan.member_key)
                .ok_or(CoordinatorError::AuthorityChanged)?;
            if member.configuration_revision != plan.configuration_revision
                || member.requested_model != plan.model
            {
                return Err(CoordinatorError::AuthorityChanged);
            }
            Ok(())
        }
    }

    struct Execution;

    impl ExecutionStatusSource for Execution {
        fn status(&self) -> Result<ExecutionSnapshot, DaemonError> {
            Err(DaemonError::Unavailable)
        }
    }

    fn observation() -> Result<SwarmObservation, Box<dyn Error>> {
        let roster = ["worker-a", "worker-b"]
            .into_iter()
            .map(|key| MemberConfiguration {
                member_key: key.to_owned(),
                label: key.to_owned(),
                provider: "controlled".to_owned(),
                requested_model: "fixture-v1".to_owned(),
                requested_effort: Some("medium".to_owned()),
                configuration_revision: 1,
                moving_alias_acknowledged: false,
                configuration_state: ProviderConfigurationState::FixtureUnavailable,
            })
            .collect::<Vec<_>>();
        Ok(SwarmObservation {
            swarm: SwarmView {
                swarm_id: SwarmId::new("swarm-planning-service".to_owned())?,
                room_id: "room-planning-service".to_owned(),
                goal: "Investigate the goal and choose useful bounded work".to_owned(),
                constraints: Vec::new(),
                acceptance_criteria: vec![AcceptanceCriterion {
                    text: "Independent evidence".to_owned(),
                }],
                working_area: PathBuf::from("/tmp/autonomy-tests"),
                roster: roster.clone(),
                progress_review_interval_seconds: 300,
                correction_failure_limit: 3,
                source_label: "test".to_owned(),
            },
            actor: SwarmActor::HumanCoordinator,
            member_id: "human".to_owned(),
            room_seq: 1,
            authoritative_state_hash: "blake3:planning-test".to_owned(),
            action_offers: Vec::new(),
            activity: json!({
                "phase":"open", "execution_epoch":1, "goal_revision":1, "direction_revision":0,
                "roster": roster, "work_items":[], "work_attempts":[], "blockers":[], "problems":[], "resource_conflicts":[],
                "outstanding_progress_reviews":[]
            }),
        })
    }

    fn fixture(
        limit: usize,
    ) -> Result<(tempfile::TempDir, CoordinatorService, Driver), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let observation = observation()?;
        let swarm_id = observation.swarm.swarm_id.clone();
        let driver = Driver(Rc::new(RefCell::new(DriverState {
            observation,
            intents: BTreeMap::new(),
            fail_preflight: false,
        })));
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(driver.clone()),
            Box::new(Execution),
        )?;
        service.enable_autonomy(AutonomyPolicy {
            max_work_items: 2,
            invocation_limit: limit,
            resource_policy: ResourcePolicy::WorkspaceWrite,
            allowed_tools: Vec::new(),
            delivery: None,
        })?;
        Ok((directory, service, driver))
    }

    fn intent_for(
        plan: &WorkerActionPlan,
        submission: SubmissionDisposition,
    ) -> CoordinatorIntentView {
        CoordinatorIntentView {
            invocation_id: plan.invocation_id.clone(),
            swarm_id: plan.swarm_id.clone(),
            member_key: plan.member_key.clone(),
            member_id: format!("member-{}", plan.member_key),
            configuration_revision: plan.configuration_revision,
            provider: plan.provider,
            requested_model: plan.model.clone(),
            requested_effort: plan.effort.clone(),
            session: plan.session.clone(),
            proposal: None,
            action: None,
            state: CoordinatorIntentState::Settled,
            evidence: None,
            submission,
            validation_feedback: None,
        }
    }

    fn reason(
        service: &mut CoordinatorService,
        driver: &Driver,
    ) -> Result<WorkerActionPlan, Box<dyn Error>> {
        service.advance_autonomy(&driver.0.borrow().observation)?;
        service
            .state
            .plans
            .values()
            .last()
            .cloned()
            .ok_or_else(|| "planning plan missing".into())
    }

    fn settle_decision(
        service: &mut CoordinatorService,
        driver: &Driver,
        plan: &WorkerActionPlan,
        decision: PlanningDecisionKind,
    ) -> Result<(), Box<dyn Error>> {
        let intent = intent_for(
            plan,
            SubmissionDisposition::PlanningDecision(PlanningDecision {
                schema: PLANNING_DECISION_SCHEMA.to_owned(),
                decision,
            }),
        );
        driver
            .0
            .borrow_mut()
            .intents
            .insert(plan.invocation_id.clone(), intent.clone());
        service.record_intents([intent])?;
        Ok(())
    }

    fn instruction(plan: &WorkerActionPlan) -> Result<PlanningInstruction, serde_json::Error> {
        serde_json::from_str(&plan.instruction)
    }

    fn select(option: &PlanningOption) -> SelectedStep {
        SelectedStep {
            member_key: option.member_key.clone(),
            target_id: option.target_id.clone(),
            instruction: "Model-authored next approach".to_owned(),
            dependency_ids: None,
        }
    }

    #[test]
    fn invalid_parallel_metadata_is_retained_for_model_correction_after_reopen()
    -> Result<(), Box<dyn Error>> {
        let (directory, mut service, driver) = fixture(12)?;
        let first = reason(&mut service, &driver)?;
        let options = instruction(&first)?.options;
        settle_decision(
            &mut service,
            &driver,
            &first,
            PlanningDecisionKind::Dispatch {
                steps: options.iter().take(2).map(select).collect(),
                reason: "Model incorrectly batches two proposal alternatives".to_owned(),
            },
        )?;
        service.advance_autonomy(&driver.0.borrow().observation)?;
        assert_eq!(service.state.plans.len(), 1);
        assert_eq!(service.autonomy_failure_streak(), 1);
        assert!(
            service
                .state
                .autonomy
                .as_ref()
                .is_some_and(|state| !state.halted)
        );
        let swarm_id = service.state.swarm_id.clone();
        drop(service);
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(driver.clone()),
            Box::new(Execution),
        )?;
        let second = reason(&mut service, &driver)?;
        let context = instruction(&second)?;
        assert_ne!(first.invocation_id, second.invocation_id);
        let rejection = context
            .rejected_decisions
            .get(&first.invocation_id)
            .ok_or("missing durable rejection feedback")?;
        assert!(rejection.feedback.contains("sole step"));
        settle_decision(
            &mut service,
            &driver,
            &second,
            PlanningDecisionKind::Dispatch {
                steps: vec![select(&context.options[0])],
                reason: "Model corrects the metadata batch and selects one proposal".to_owned(),
            },
        )?;
        service.advance_autonomy(&driver.0.borrow().observation)?;
        assert_eq!(
            service
                .state
                .plans
                .values()
                .filter(|plan| matches!(
                    plan.semantic_target,
                    Some(WorkerSemanticTarget::WorkProposal { .. })
                ))
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn proposal_dependency_field_rejection_names_the_exact_correction() -> Result<(), Box<dyn Error>>
    {
        let (_directory, mut service, driver) = fixture(12)?;
        let first = reason(&mut service, &driver)?;
        let option = instruction(&first)?.options.remove(0);
        let mut step = select(&option);
        step.dependency_ids = Some(Vec::new());
        settle_decision(
            &mut service,
            &driver,
            &first,
            PlanningDecisionKind::Dispatch {
                steps: vec![step],
                reason: "Proposal payload confused with planning step".to_owned(),
            },
        )?;
        service.advance_autonomy(&driver.0.borrow().observation)?;
        assert_eq!(service.state.plans.len(), 1);
        let next = reason(&mut service, &driver)?;
        let context = instruction(&next)?;
        let feedback = &context
            .rejected_decisions
            .get(&first.invocation_id)
            .ok_or("missing rejection")?
            .feedback;
        assert!(feedback.contains("Remove step.dependency_ids entirely"));
        assert!(feedback.contains("empty array"));
        assert!(!feedback.contains("Unknown target_id"));
        Ok(())
    }

    #[test]
    fn schema_rejection_feedback_reaches_next_planning_instruction() -> Result<(), Box<dyn Error>> {
        let (directory, mut service, driver) = fixture(12)?;
        let first = reason(&mut service, &driver)?;
        let option = instruction(&first)?.options.remove(0);
        settle_decision(
            &mut service,
            &driver,
            &first,
            PlanningDecisionKind::Dispatch {
                steps: vec![select(&option)],
                reason: "Propose one bounded Work Item".to_owned(),
            },
        )?;
        service.advance_autonomy(&driver.0.borrow().observation)?;
        let worker = service
            .state
            .plans
            .values()
            .find(|plan| {
                matches!(
                    plan.semantic_target,
                    Some(WorkerSemanticTarget::WorkProposal { .. })
                )
            })
            .cloned()
            .ok_or("worker plan missing")?;
        let mut rejected = intent_for(
            &worker,
            SubmissionDisposition::NotSubmitted(NonSubmissionReason::ProviderOutputInvalid),
        );
        rejected.validation_feedback = Some("Action payload violates the offered Pack schema at $.contribution_refs: array is shorter than minItems. No Action was submitted; revise this field using the offered Action contract.".to_owned());
        driver
            .0
            .borrow_mut()
            .intents
            .insert(worker.invocation_id.clone(), rejected.clone());
        service.record_intents([rejected])?;
        let swarm_id = service.state.swarm_id.clone();
        drop(service);
        let mut service = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(driver.clone()),
            Box::new(Execution),
        )?;
        service.advance_autonomy(&driver.0.borrow().observation)?;
        let next = reason(&mut service, &driver)?;
        let feedback = instruction(&next)?.worker_feedback;
        assert!(
            feedback[&worker.invocation_id]
                .contains("$.contribution_refs: array is shorter than minItems")
        );
        Ok(())
    }

    #[test]
    fn repeated_invalid_decisions_stop_at_the_existing_failure_bound() -> Result<(), Box<dyn Error>>
    {
        let (_directory, mut service, driver) = fixture(12)?;
        for _ in 0..3 {
            let plan = reason(&mut service, &driver)?;
            let options = instruction(&plan)?.options;
            settle_decision(
                &mut service,
                &driver,
                &plan,
                PlanningDecisionKind::Dispatch {
                    steps: options.iter().take(2).map(select).collect(),
                    reason: "Repeated invalid batch".to_owned(),
                },
            )?;
            service.advance_autonomy(&driver.0.borrow().observation)?;
        }
        service.advance_autonomy(&driver.0.borrow().observation)?;
        assert_eq!(service.state.plans.len(), 3);
        assert!(
            service
                .state
                .autonomy
                .as_ref()
                .is_some_and(|state| state.halted)
        );
        assert_eq!(
            service
                .state
                .view
                .autonomy
                .as_ref()
                .ok_or("missing view")?
                .phase,
            "needs_attention"
        );
        Ok(())
    }

    #[test]
    fn enabling_only_schedules_reasoning_and_wait_does_not_poll_same_head()
    -> Result<(), Box<dyn Error>> {
        let (directory, mut service, driver) = fixture(12)?;
        assert!(service.state.plans.is_empty());
        let plan = reason(&mut service, &driver)?;
        assert!(matches!(
            plan.semantic_target,
            Some(WorkerSemanticTarget::Planning { .. })
        ));
        assert_eq!(service.state.plans.len(), 1);
        assert!(instruction(&plan)?.options.iter().all(|option| matches!(
            option.semantic_target,
            WorkerSemanticTarget::WorkProposal { .. }
        )));
        settle_decision(
            &mut service,
            &driver,
            &plan,
            PlanningDecisionKind::Wait {
                reason: "Need new evidence".to_owned(),
            },
        )?;
        service.advance_autonomy(&driver.0.borrow().observation)?;
        service.advance_autonomy(&driver.0.borrow().observation)?;
        assert_eq!(service.state.plans.len(), 1);
        let swarm_id = service.state.swarm_id.clone();
        let ids = service
            .state
            .autonomy
            .as_ref()
            .ok_or("autonomy missing")?
            .invocation_ids
            .clone();
        drop(service);
        let mut reopened = CoordinatorService::open_with_ports(
            directory.path(),
            &swarm_id,
            Box::new(driver.clone()),
            Box::new(Execution),
        )?;
        reopened.enable_autonomy(AutonomyPolicy {
            max_work_items: 2,
            invocation_limit: 12,
            resource_policy: ResourcePolicy::WorkspaceWrite,
            allowed_tools: Vec::new(),
            delivery: None,
        })?;
        reopened.advance_autonomy(&driver.0.borrow().observation)?;
        assert_eq!(
            reopened
                .state
                .autonomy
                .as_ref()
                .ok_or("autonomy missing")?
                .invocation_ids,
            ids
        );
        driver.0.borrow_mut().observation.room_seq += 1;
        reopened.advance_autonomy(&driver.0.borrow().observation)?;
        assert_eq!(reopened.state.plans.len(), 2);
        Ok(())
    }

    #[test]
    fn changed_head_discards_a_decision_before_selecting_new_configuration()
    -> Result<(), Box<dyn Error>> {
        let (_directory, mut service, driver) = fixture(12)?;
        let plan = reason(&mut service, &driver)?;
        let option = instruction(&plan)?.options.remove(0);
        settle_decision(
            &mut service,
            &driver,
            &plan,
            PlanningDecisionKind::Dispatch {
                steps: vec![select(&option)],
                reason: "Propose bounded work".to_owned(),
            },
        )?;
        {
            let mut state = driver.0.borrow_mut();
            state.observation.room_seq += 1;
            state.observation.swarm.roster[0].configuration_revision = 2;
            state.observation.activity["roster"][0]["configuration_revision"] = json!(2);
        }
        service.advance_autonomy(&driver.0.borrow().observation)?;
        assert_eq!(service.state.plans.len(), 1);
        assert!(
            service
                .state
                .autonomy
                .as_ref()
                .ok_or("autonomy missing")?
                .consumed_decisions
                .contains(&plan.invocation_id)
        );
        let fresh = reason(&mut service, &driver)?;
        assert_eq!(fresh.configuration_revision, 2);
        assert!(matches!(
            fresh.semantic_target,
            Some(WorkerSemanticTarget::Planning { room_seq: 2, .. })
        ));
        Ok(())
    }

    #[test]
    fn invalid_selection_or_parallel_metadata_dispatches_no_partial_batch()
    -> Result<(), Box<dyn Error>> {
        for unknown in [true, false] {
            let (_directory, mut service, driver) = fixture(12)?;
            let plan = reason(&mut service, &driver)?;
            let options = instruction(&plan)?.options;
            let mut steps = options.iter().map(select).collect::<Vec<_>>();
            if unknown {
                steps.truncate(1);
                "unknown-target".clone_into(&mut steps[0].target_id);
            }
            settle_decision(
                &mut service,
                &driver,
                &plan,
                PlanningDecisionKind::Dispatch {
                    steps,
                    reason: "Choose next work".to_owned(),
                },
            )?;
            service.advance_autonomy(&driver.0.borrow().observation)?;
            assert_eq!(service.state.plans.len(), 1);
            assert!(
                !service
                    .state
                    .autonomy
                    .as_ref()
                    .ok_or("autonomy missing")?
                    .halted
            );
            assert_eq!(service.autonomy_failure_streak(), 1);
            assert_eq!(
                service
                    .state
                    .view
                    .autonomy
                    .as_ref()
                    .ok_or("view missing")?
                    .phase,
                "reasoning"
            );
        }
        Ok(())
    }

    #[test]
    fn selected_scope_preserves_model_instruction_and_enforces_budget() -> Result<(), Box<dyn Error>>
    {
        for limit in [1, 2] {
            let (_directory, mut service, driver) = fixture(limit)?;
            let plan = reason(&mut service, &driver)?;
            let option = instruction(&plan)?.options.remove(0);
            settle_decision(
                &mut service,
                &driver,
                &plan,
                PlanningDecisionKind::Dispatch {
                    steps: vec![select(&option)],
                    reason: "Choose a useful decomposition".to_owned(),
                },
            )?;
            service.advance_autonomy(&driver.0.borrow().observation)?;
            assert_eq!(service.state.plans.len(), limit);
            if limit == 1 {
                assert_eq!(
                    service
                        .state
                        .view
                        .autonomy
                        .as_ref()
                        .ok_or("view missing")?
                        .phase,
                    "budget_exhausted"
                );
            } else {
                let selected = service
                    .state
                    .plans
                    .get(&format!("{}-step-0", plan.invocation_id))
                    .ok_or("selected plan missing")?;
                let worker_instruction: Value = serde_json::from_str(&selected.instruction)?;
                assert_eq!(
                    worker_instruction["model_instruction"],
                    "Model-authored next approach"
                );
                assert_eq!(worker_instruction["action_contract"], option.instruction);
                assert_eq!(selected.semantic_target, Some(option.semantic_target));
                assert_eq!(selected.resource_policy, ResourcePolicy::ReadOnly);
            }
        }
        Ok(())
    }

    #[test]
    fn dependency_selection_uses_retained_revisions_and_rejects_unknown_ids()
    -> Result<(), Box<dyn Error>> {
        let (_directory, mut service, driver) = fixture(12)?;
        let mut plan = reason(&mut service, &driver)?;
        let mut retained = instruction(&plan)?;
        retained.options[0].semantic_target = WorkerSemanticTarget::WorkDependencyRevision {
            execution_epoch: 1,
            work_id: "work-a".to_owned(),
            work_revision: 3,
            dependency_revisions: BTreeMap::new(),
        };
        retained.options[0].allowed_action_types = vec!["revise_work_dependencies".to_owned()];
        retained.work_revisions = BTreeMap::from([("work-b".to_owned(), 7)]);
        plan.instruction = serde_json::to_string(&retained)?;
        let mut step = select(&retained.options[0]);
        step.dependency_ids = Some(vec!["work-b".to_owned()]);
        let selected =
            service.selected_plans(&driver.0.borrow().observation, &plan, &[step.clone()])?;
        assert!(
            matches!(&selected[0].semantic_target, Some(WorkerSemanticTarget::WorkDependencyRevision { dependency_revisions, .. })
            if dependency_revisions == &BTreeMap::from([("work-b".to_owned(), 7)]))
        );
        step.dependency_ids = Some(vec!["unknown".to_owned()]);
        assert!(
            service
                .selected_plans(&driver.0.borrow().observation, &plan, &[step])
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn failed_unstaged_planning_does_not_resurrect_when_provider_recovers()
    -> Result<(), Box<dyn Error>> {
        let (_directory, mut service, driver) = fixture(12)?;
        let plan = reason(&mut service, &driver)?;
        driver.0.borrow_mut().fail_preflight = true;
        service.restore_plans(&[])?;
        assert!(matches!(
            service.state.view.invocations[0].coordinator_outcome,
            CoordinatorOutcome::NotSubmitted { .. }
        ));
        service.advance_autonomy(&driver.0.borrow().observation)?;
        assert!(
            service
                .state
                .autonomy
                .as_ref()
                .ok_or("autonomy missing")?
                .consumed_decisions
                .contains(&plan.invocation_id)
        );
        driver.0.borrow_mut().fail_preflight = false;
        service.restore_plans(&[])?;
        assert!(!driver.0.borrow().intents.contains_key(&plan.invocation_id));
        Ok(())
    }

    #[test]
    fn parallel_work_attempt_batch_is_all_or_none_at_budget_boundary() -> Result<(), Box<dyn Error>>
    {
        for limit in [2, 3] {
            let (_directory, mut service, driver) = fixture(limit)?;
            let mut plan = reason(&mut service, &driver)?;
            let mut retained = instruction(&plan)?;
            for (index, option) in retained.options.iter_mut().enumerate() {
                option.semantic_target = WorkerSemanticTarget::WorkAttempt {
                    execution_epoch: 1,
                    work_id: format!("work-{index}"),
                    work_revision: 2,
                    attempt_id: format!("attempt-{index}"),
                    attempt_revision: 1,
                };
                option.allowed_action_types = vec!["submit_contribution".to_owned()];
                option.resource_policy = ResourcePolicy::WorkspaceWrite;
            }
            plan.instruction = serde_json::to_string(&retained)?;
            service.mutate(|state| {
                state.plans.insert(plan.invocation_id.clone(), plan.clone());
            })?;
            settle_decision(
                &mut service,
                &driver,
                &plan,
                PlanningDecisionKind::Dispatch {
                    steps: retained.options.iter().map(select).collect(),
                    reason: "Execute independent owned attempts in parallel".to_owned(),
                },
            )?;
            service.advance_autonomy(&driver.0.borrow().observation)?;
            let expected = if limit == 2 { 1 } else { 3 };
            assert_eq!(service.state.plans.len(), expected);
            assert_eq!(
                service
                    .state
                    .autonomy
                    .as_ref()
                    .ok_or("autonomy missing")?
                    .invocation_ids
                    .len(),
                expected
            );
            if limit == 3 {
                assert_eq!(
                    service
                        .state
                        .plans
                        .values()
                        .filter(|plan| matches!(
                            plan.semantic_target,
                            Some(WorkerSemanticTarget::WorkAttempt { .. })
                        ))
                        .count(),
                    2
                );
            }
        }
        Ok(())
    }

    #[test]
    fn three_failed_reasoning_turns_stop_before_spending_the_full_budget()
    -> Result<(), Box<dyn Error>> {
        let (_directory, mut service, driver) = fixture(12)?;
        for _ in 0..3 {
            let plan = reason(&mut service, &driver)?;
            let intent = intent_for(
                &plan,
                SubmissionDisposition::NotSubmitted(
                    crate::NonSubmissionReason::ProviderOutputInvalid,
                ),
            );
            driver
                .0
                .borrow_mut()
                .intents
                .insert(plan.invocation_id.clone(), intent.clone());
            service.record_intents([intent])?;
        }
        service.advance_autonomy(&driver.0.borrow().observation)?;
        let autonomy = service.state.autonomy.as_ref().ok_or("autonomy missing")?;
        assert!(autonomy.halted);
        assert_eq!(autonomy.invocation_ids.len(), 3);
        assert_eq!(
            service
                .state
                .view
                .autonomy
                .as_ref()
                .ok_or("view missing")?
                .phase,
            "needs_attention"
        );
        Ok(())
    }

    #[test]
    fn progress_review_remains_visible_when_adaptive_invocation_budget_is_exhausted()
    -> Result<(), Box<dyn Error>> {
        for limit in [1, 2] {
            let (_directory, mut service, driver) = fixture(limit)?;
            reason(&mut service, &driver)?;
            let observation = driver.0.borrow().observation.clone();
            let profiles = member_execution_profiles(&observation, &service.state.plans)?;
            let target = WorkerSemanticTarget::ProgressReviewClaim {
                execution_epoch: 1,
                review_id: "progress-1".to_owned(),
                review_revision: 1,
                work_id: "review-work-1".to_owned(),
                work_revision: 1,
            };
            let plan = progress_review_plan(
                profiles.get("worker-a").ok_or("profile missing")?,
                &target,
                1,
            )?;
            let discovered = [DiscoveredReview {
                target,
                authoritative_status: ProgressReviewAuthoritativeStatus::Due,
                required_member_key: None,
                plan: Some(plan),
            }];
            service.stage_new_progress_review_plans(&discovered)?;
            service.stage_new_progress_review_plans(&discovered)?;
            assert_eq!(service.state.plans.len(), limit);
            assert_eq!(
                service
                    .state
                    .autonomy
                    .as_ref()
                    .ok_or("autonomy missing")?
                    .invocation_ids
                    .len(),
                limit
            );
            let snapshot = SwarmExecutionSnapshot {
                swarm_id: service.state.swarm_id.as_str().to_owned(),
                desired: DesiredExecution::Running,
                phase: ExecutionPhase::Running,
                execution_epoch: 1,
                priority: 1,
                budget: crate::execution::RunBudget::default(),
                invocations_started: 0,
                active_time_ms: 0,
                queued: Vec::new(),
                active: Vec::new(),
                unknown_effects: Vec::new(),
            };
            service.publish_progress_reviews(&discovered, &snapshot)?;
            assert_eq!(service.state.view.progress_reviews.len(), 1);
            assert_eq!(
                service.state.view.progress_reviews[0].authoritative_status,
                ProgressReviewAuthoritativeStatus::Due
            );
            if limit == 1 {
                assert!(
                    service.state.view.progress_reviews[0]
                        .invocation_id
                        .is_none()
                );
                assert_eq!(
                    service.state.view.progress_reviews[0].execution_status,
                    ProgressReviewExecutionStatus::Due
                );
            }
        }
        Ok(())
    }
}
