import {
  defineActivityPack,
  type ActivityPackDefinition,
  type ActivityPackViewerClass,
  type CanonicalObject,
} from "@worldstream/pack-sdk";

import {
  coreRecord,
  enabledParticipants,
  SETUP_REVISION,
  stateCanonical,
  type SwarmState,
  configurationValue,
} from "./model.js";
import {
  actionDescriptors,
  configurationSchema,
  participantProjectionSchema,
  publicProjectionSchema,
  stateSchema,
} from "./schemas.js";
import { observeSwarm, viewSwarm } from "./view.js";
import { reduceSwarm } from "./workflow.js";

function audienceSchemas(): Readonly<Record<ActivityPackViewerClass, CanonicalObject>> {
  return {
    participant: participantProjectionSchema(),
    public: publicProjectionSchema(),
    operator: publicProjectionSchema(),
    historical_participant: participantProjectionSchema(),
    historical_public: publicProjectionSchema(),
    historical_operator: publicProjectionSchema(),
    final_reveal: publicProjectionSchema(),
  };
}

function eventDescriptors() {
  return [
    "initial_setup_confirmed", "work_item_proposed", "work_dependencies_revised", "work_item_claimed", "work_item_handed_off", "work_blocker_reported",
    "work_blocker_resolved", "contribution_submitted", "candidate_submitted", "candidate_check_recorded", "candidate_review_recorded",
    "review_finding_resolved", "swarm_result_accepted", "suggestion_submitted", "suggestion_disposition_recorded", "human_direction_issued",
    "progress_review_due", "progress_review_reported", "progress_review_timer_suppressed", "correction_outcome_recorded", "member_configuration_updated", "resource_version_recorded",
    "resource_conflict_resolved", "writeback_requested", "writeback_outcome_recorded", "late_contribution_recorded", "swarm_reopened",
  ].map((eventType) => ({ eventType, payloadSchema: { type: "object" } as CanonicalObject }));
}

export default defineActivityPack({
  descriptor: {
    packId: "worldstream.agent-swarm",
    name: "Agent Swarm",
    version: "0.2.0",
    roles: [
      { role: "human_coordinator", minimum: 1, maximum: 1 },
      { role: "worker", minimum: 1, maximum: 16 },
    ],
    actions: actionDescriptors(),
    rejectionCodes: [
      "artifact_outside_working_area", "blocker_resolved", "blocking_finding", "capacity_exhausted", "core_role_invariant", "correction_incomplete", "dependency_cycle",
      "duplicate_check", "duplicate_id", "finding_resolved", "handoff_unreconciled", "invalid_check", "invalid_check_evidence", "invalid_correction", "invalid_dependency", "invalid_phase", "invalid_review", "invalid_target",
      "invalid_writeback", "missing_evidence", "no_progress_review", "not_work_owner", "problem_closed", "resource_conflict", "review_incomplete",
      "role_violation", "self_review", "setup_already_confirmed", "setup_revision_mismatch", "stale_attempt_revision", "stale_blocker_revision",
      "stale_candidate", "stale_criteria_revision", "stale_direction_revision", "stale_execution_epoch", "stale_finding_revision", "stale_goal_revision",
      "stale_configuration_revision", "stale_problem_revision", "stale_progress_review", "stale_resource_basis", "stale_resource_version", "stale_room_head", "stale_suggestion_revision",
      "stale_work_revision", "stale_writeback_revision", "suggestion_disposed", "unknown_attempt", "unknown_blocker", "unknown_candidate",
      "unknown_contribution", "unknown_finding", "unknown_member", "unknown_problem", "unknown_suggestion", "unknown_work", "unknown_writeback", "unsupported_action",
      "moving_alias_unacknowledged",
      "unsupported_stimulus", "validation_incomplete", "work_ineligible", "writeback_closed",
    ],
    events: eventDescriptors(),
    attentionReasons: ["correction_escalated", "progress_review_blocked", "progress_review_due", "resource_conflict", "review_dispute", "suggestion_pending", "uncertain_external_effect"],
    configurationSchema: configurationSchema(),
    stateSchema: stateSchema(),
    projectionSchemas: audienceSchemas(),
    observationSchemas: audienceSchemas(),
  },

  initialize(input) {
    const configuration = configurationValue(input.configuration);
    const members = enabledParticipants(coreRecord(input.initial_core_state));
    if (members.workers.length !== configuration.roster.length) throw new TypeError("configuration roster must correspond one-to-one with Agent Worker Memberships");
    const state: SwarmState = {
      phase: "awaiting_human_confirmation", room_seq: 0, execution_epoch: 1,
      goal: configuration.goal, goal_revision: 1, constraints: configuration.constraints,
      acceptance_criteria: configuration.acceptance_criteria, criteria_revision: 1, direction_revision: 0,
      approved_working_area: configuration.approved_working_area, human_coordinator_member_id: members.coordinator,
      roster: configuration.roster.map((entry, index) => ({
        ...entry,
        configuration_revision: 1,
        member_id: members.workers[index]!,
      })),
      member_notices: {
        [members.coordinator]: "Review the exact goal, criteria, working area and selected roster before confirming setup.",
        ...Object.fromEntries(members.workers.map((memberId) => [memberId, "Provider execution settings are reported by the local application; Pack state records requested settings without claiming provider success."])),
      },
      setup_revision: SETUP_REVISION, confirmed_by_member_id: null, confirmed_at_room_seq: null,
      progress_review_interval_seconds: configuration.progress_review_interval_seconds,
      correction_failure_limit: configuration.correction_failure_limit, progress_review_sequence: 0, outstanding_progress_reviews: [],
      work_items: [], work_attempts: [], handoffs: [], blockers: [], contributions: [], candidates: [], checks: [], reviews: [], findings: [], results: [],
      suggestions: [], directions: [], problems: [], resources: [], resource_conflicts: [], writebacks: [], late_contributions: [],
    };
    return { initial_activity_state: stateCanonical(state), timer_requests: [] };
  },

  reduce(input) { return reduceSwarm(input); },
  view(input) { return viewSwarm(input); },
  observe(input) { return observeSwarm(input); },
} satisfies ActivityPackDefinition);

export { reduceSwarm } from "./workflow.js";
