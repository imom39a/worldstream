import {
  ACTIVITY_START_CONTRACT_ID,
  ACTIVITY_START_SOURCE_ID,
  canonicalStringify,
  defineActivityPack,
  type ActivityPackDefinition,
  type CanonicalJson,
  type CanonicalObject,
  type PackReduceOutput,
} from "@worldstream/pack-sdk";

import {
  RuleRejection,
  asCanonical,
  record,
  stateValue,
  stringValue,
  type Role,
} from "./model.js";
import {
  emptyPayloadSchema,
  participantProjectionSchema, dialogueRecordSchema, dialogueTextSchema,
  publicProjectionSchema,
  stateSchema,
} from "./schemas.js";
import {
  applyLeadAction,
  emptyStage,
  initializeArchiveState,
  payload,
  startArchive,
  stateAsCanonical,
} from "./rules.js";
import { authorizedView } from "./view.js";
import {
  MIRA_PLAN_ATTENTION_REASON,
  applyMiraLeadControl,
  cancelPendingOpportunities,
  expireMiraOpportunity,
  initialCompanionFromCore,
  isMiraLeadControl,
  reconcileMira,
  submitMiraPlan,
} from "./companions.js";
import { JONAH_PLAN_TIMER_ID } from "./companions.js";
import { invalidateExtraction } from "./turn-resolution.js";
import { finalizeTerminal, isCurrentSessionExpiry, requireSessionAdmission, SESSION_TIMER_ID } from "./session.js";

export const ACTIVITY_START_INPUT_TYPE =
  "worldstream.midnight-archive/briefing-opened/v1";

function actionStagedSchema(): CanonicalObject {
  return {
    additionalProperties: false,
    properties: {
      event_type: { const: "action_staged" },
      power_cost: { maximum: 2, minimum: 0, type: "integer" },
      staged_action_type: {
        enum: [
          "stage_move", "stage_use_verifier", "stage_open_service_hatch",
          "stage_inspect_records", "stage_inspect_conservation",
          "stage_accept_preservation_agreement", "stage_prepare_collection",
          "stage_energize_preservation_equipment", "stage_recover_candidate",
          "stage_protect_source_record", "stage_extract", "stage_wait",
        ],
      },
      turn_cost: { const: 1 },
    },
    required: ["event_type", "staged_action_type", "turn_cost", "power_cost"],
    type: "object",
  };
}

function turnCommittedSchema(): CanonicalObject {
  return {
    additionalProperties: false,
    properties: {
      committed_action_type: {
        enum: [
          "stage_move", "stage_use_verifier", "stage_open_service_hatch",
          "stage_inspect_records", "stage_inspect_conservation",
          "stage_accept_preservation_agreement", "stage_prepare_collection",
          "stage_energize_preservation_equipment", "stage_recover_candidate",
          "stage_protect_source_record", "stage_extract", "stage_wait",
        ],
      },
      event_type: { const: "turn_committed" },
      location: { enum: ["atrium", "records", "conservation", "plant", "vault"] },
      outcome_kind: {
        enum: ["pending", "success", "partial_extraction", "wrong_ledger", "no_ledger", "exhausted_inside"],
      },
      mira_contribution: {
        enum: ["none", "move", "inspect_source", "share_source", "use_verifier", "open_service_hatch", "collect_assay_sample", "complete_field_assay", "follow_move", "regroup_move"],
      },
      mira_plan_revision: { maximum: 65_535, minimum: 0, type: "integer" },
      jonah_contribution: {
        enum: ["none", "move", "inspect_source", "share_source", "use_verifier", "open_service_hatch", "collect_assay_sample", "complete_field_assay", "follow_move", "regroup_move"],
      },
      jonah_plan_revision: { maximum: 65_535, minimum: 0, type: "integer" },
      power: { maximum: 3, minimum: 0, type: "integer" },
      turn: { maximum: 16, minimum: 1, type: "integer" },
    },
    required: [
      "event_type", "committed_action_type", "turn", "location", "power", "outcome_kind",
      "mira_contribution", "mira_plan_revision", "jonah_contribution", "jonah_plan_revision",
    ],
    type: "object",
  };
}

function companionStateUpdatedSchema(): CanonicalObject {
  return {
    additionalProperties: false,
    properties: {
      action_type: { enum: [
        "assign_mira_task", "cancel_mira_task", "set_mira_follow", "set_mira_hold",
        "set_mira_regroup", "request_mira_plan", "prepare_mira_contribution",
        "defer_mira_contribution", "submit_companion_plan", "expire_companion_plan",
        "assign_jonah_task", "cancel_jonah_task", "set_jonah_follow", "set_jonah_hold", "set_jonah_regroup",
        "request_jonah_plan", "prepare_jonah_contribution", "defer_jonah_contribution",
      ] },
      event_type: { const: "companion_state_updated" },
      role: { enum: ["mira", "jonah"] },
      planning_status: { enum: ["not_requested", "waiting", "ready", "expired", "complete"] },
      task_revision: { maximum: 65_535, minimum: 0, type: "integer" },
    },
    required: ["event_type", "role", "action_type", "task_revision", "planning_status"],
    type: "object",
  };
}

function archiveStartedSchema(): CanonicalObject {
  return {
    additionalProperties: false,
    properties: {
      event_type: { const: "archive_started" },
      phase: { const: "active" },
    },
    required: ["event_type", "phase"],
    type: "object",
  };
}

export function reduceArchive(input: CanonicalObject): PackReduceOutput {
  const current = stateValue(input.prior_activity_state);
  const coreBefore = record(input.core_before, "core_before");
  const coreAfter = record(input.proposed_core_after, "proposed_core_after");
  const stimulus = record(input.recorded_stimulus, "recorded_stimulus");
  const scheduled = input.scheduled_timers === undefined
    ? {}
    : record(input.scheduled_timers, "scheduled_timers");
  const stimulusType = stringValue(stimulus.stimulus_type, "stimulus_type");

  try {
    validateCoreInvariant(coreBefore);
    validateCoreInvariant(coreAfter);
    if (stimulusType === "external_input") {
      return applyActivityStart(current, stimulus);
    }
    if (stimulusType === "participant_action") {
      const actionType = stringValue(stimulus.action_type, "action_type");
      const memberId = stringValue(stimulus.member_id, "member_id");
      const role = roleForMember(coreBefore, memberId);
      const actionPayload = payload(stimulus.canonical_payload);
      const admittedAt = stringValue(stimulus.admitted_at, "admitted_at");
      requireSessionAdmission(current, admittedAt);
      // A request must still reject while another window is open. A proposal
      // consumes its own exact window. Every other accepted edit supersedes it.
      const editing = actionType === "request_mira_plan" || actionType === "request_jonah_plan" || actionType === "submit_companion_plan"
        ? { state: current, scheduled, timerRequests: [] }
        : cancelPendingOpportunities(current, scheduled);
      const applied = role === "lead"
        ? isMiraLeadControl(actionType)
          ? applyMiraLeadControl(editing.state, actionType, actionPayload, coreBefore, admittedAt, editing.scheduled, actionType.includes("_jonah_") ? "jonah" : "mira")
          : applyLeadAction(editing.state, actionType, actionPayload, coreBefore, editing.scheduled)
        : (role === "mira" || role === "jonah") && actionType === "submit_companion_plan"
        ? submitMiraPlan(current, actionPayload, coreBefore, memberId, admittedAt, scheduled, role)
        : (() => { throw new RuleRejection("role_violation", "the acting Role cannot perform this Action"); })();
      return {
        activity_disposition_type: "apply",
        next_activity_state: stateAsCanonical(isMiraLeadControl(actionType) || actionType === "submit_companion_plan" ? invalidateExtraction(applied.state) : applied.state),
        ordered_domain_events: [applied.event, ...("dialogueEvent" in applied && applied.dialogueEvent ? [applied.dialogueEvent as CanonicalObject] : [])],
        timer_requests: [...("timerRequests" in applied
          ? applied.timerRequests as readonly CanonicalJson[]
          : []), ...editing.timerRequests],
        ordered_attention_signals: "attentionSignals" in applied
          ? applied.attentionSignals as readonly CanonicalJson[]
          : [],
      };
    }
    if (stimulusType === "timer_fired") {
      if (current.phase !== "active" || stimulus.timer_id === SESSION_TIMER_ID) {
        if (stimulus.timer_id === SESSION_TIMER_ID && isCurrentSessionExpiry(current, stimulus)) {
          const terminal = finalizeTerminal({ ...current, phase: "expired", outcome: null,
            staged_action: emptyStage(), extraction: { ...current.extraction, status: "none", for_turn: 0,
              contribution_fingerprint: "none", extracted_roles: [], left_behind_roles: [] } }, scheduled, SESSION_TIMER_ID);
          return { activity_disposition_type: "apply", next_activity_state: stateAsCanonical(terminal.state),
            ordered_domain_events: [{ event_type: "archive_session_expired", deadline: current.session_deadline! }],
            timer_requests: terminal.timerRequests, ordered_attention_signals: [] };
        }
        return { activity_disposition_type: "apply", next_activity_state: stateAsCanonical(current),
          ordered_domain_events: [], timer_requests: [], ordered_attention_signals: [] };
      }
      const applied = expireMiraOpportunity(current, stimulus, stimulus.timer_id === JONAH_PLAN_TIMER_ID ? "jonah" : "mira");
      if (canonicalStringify(stateAsCanonical(applied.state)) === canonicalStringify(stateAsCanonical(current))) {
        return { activity_disposition_type: "apply", next_activity_state: stateAsCanonical(current),
          ordered_domain_events: [], timer_requests: [], ordered_attention_signals: [] };
      }
      return {
        activity_disposition_type: "apply",
        next_activity_state: stateAsCanonical(invalidateExtraction(applied.state)),
        ordered_domain_events: [applied.event, ...("dialogueEvent" in applied && applied.dialogueEvent ? [applied.dialogueEvent as CanonicalObject] : [])],
        timer_requests: applied.timerRequests,
        ordered_attention_signals: applied.attentionSignals,
      };
    }
    if (stimulusType === "core_proposed") {
      const editing = cancelPendingOpportunities(current, scheduled);
      const reconciled = reconcileMira(editing.state, coreAfter, editing.scheduled);
      const jonah = reconcileMira(reconciled.state, coreAfter, editing.scheduled, "jonah");
      return {
        activity_disposition_type: "apply",
        next_activity_state: stateAsCanonical(invalidateExtraction(jonah.state)),
        ordered_domain_events: [],
        timer_requests: [...reconciled.timerRequests, ...jonah.timerRequests, ...editing.timerRequests],
        ordered_attention_signals: [],
      };
    }
    throw new TypeError("Midnight Archive does not accept this Stimulus kind");
  } catch (error) {
    if (error instanceof RuleRejection && cleanRejectionAllowed(stimulusType)) {
      return {
        activity_disposition_type: "reject",
        declared_code: error.code,
        bounded_safe_details: { reason: error.message },
      };
    }
    throw error;
  }
}

function applyActivityStart(
  current: ReturnType<typeof stateValue>,
  stimulus: CanonicalObject,
): PackReduceOutput {
  if (
    current.phase !== "briefing" ||
    stimulus.source_id !== ACTIVITY_START_SOURCE_ID ||
    stimulus.input_type !== ACTIVITY_START_INPUT_TYPE ||
    canonicalStringify(stimulus.canonical_payload as CanonicalJson) !==
      canonicalStringify({ opened_by: "host" }) ||
    !Array.isArray(stimulus.immutable_resource_references) ||
    stimulus.immutable_resource_references.length !== 0
  ) {
    throw new RuleRejection("inactive", "Activity Start does not match the briefing contract");
  }
  const next = startArchive(stringValue(stimulus.recorded_at, "recorded_at"), current);
  return {
    activity_disposition_type: "apply",
    next_activity_state: stateAsCanonical(next),
    ordered_domain_events: [{ event_type: "archive_started", phase: "active" }],
    timer_requests: [{ timer_request_type: "schedule_next", timer_id: SESSION_TIMER_ID,
      due: next.session_deadline!, canonical_payload: { timer: "archive_session", deadline: next.session_deadline! } }],
    ordered_attention_signals: [],
  };
}

export function validateCoreInvariant(core: CanonicalObject): void {
  const memberships = record(core.memberships, "Core memberships");
  const counts: Record<Role, number> = { lead: 0, mira: 0, jonah: 0 };
  for (const value of Object.values(memberships)) {
    const membership = record(value, "Core membership");
    if (membership.role === null) {
      if (membership.access_mode !== "spectator" && membership.access_mode !== "operator") {
        throw new RuleRejection(
          "core_role_invariant",
          "nonparticipants must use spectator or operator access",
        );
      }
      continue;
    }
    if (
      membership.role !== "lead" &&
      membership.role !== "mira" &&
      membership.role !== "jonah"
    ) {
      throw new RuleRejection("core_role_invariant", "participant has an unknown Role");
    }
    const role = membership.role;
    counts[role] += 1;
    const expectedKind = role === "lead" ? "human" : "agent";
    const standingIsValid = role === "lead"
      ? membership.standing === "enabled"
      : membership.standing === "enabled" || membership.standing === "suspended" || membership.standing === "departed";
    if (
      membership.access_mode !== "participant" ||
      membership.principal_kind !== expectedKind ||
      !standingIsValid
    ) {
      throw new RuleRejection(
        "core_role_invariant",
        `${role} must retain participant access and its ${expectedKind} principal kind`,
      );
    }
  }
  if (counts.lead !== 1 || counts.mira > 1 || counts.jonah > 1) {
    throw new RuleRejection(
      "core_role_invariant",
      "the roster requires one lead and at most one of each optional companion",
    );
  }
}

function roleForMember(core: CanonicalObject, memberId: string): Role {
  const memberships = record(core.memberships, "Core memberships");
  const membership = record(memberships[memberId], "acting Membership");
  const role = membership.role;
  if (membership.access_mode !== "participant" || membership.standing !== "enabled") {
    throw new RuleRejection("role_violation", "acting Membership is not an enabled Participant");
  }
  if (role === "lead" || role === "mira" || role === "jonah") return role;
  throw new RuleRejection("role_violation", "acting Membership has no participant Role");
}

function cleanRejectionAllowed(stimulusType: string): boolean {
  return stimulusType === "participant_action" ||
    stimulusType === "external_input" ||
    stimulusType === "core_proposed";
}

export default defineActivityPack({
  descriptor: {
    packId: "worldstream.midnight-archive",
    schemaVersion: 5,
    name: "Midnight Archive",
    version: "0.1.0",
    roles: [
      { role: "lead", minimum: 1, maximum: 1 },
      { role: "mira", minimum: 0, maximum: 1 },
      { role: "jonah", minimum: 0, maximum: 1 },
    ],
    actions: [
      {
        actionType: "stage_move",
        payloadSchema: {
          additionalProperties: false,
          properties: {
            destination: { enum: ["atrium", "records", "conservation", "plant", "vault"] },
          },
          required: ["destination"],
          type: "object",
        },
      },
      { actionType: "stage_inspect_records", payloadSchema: emptyPayloadSchema() },
      { actionType: "stage_inspect_conservation", payloadSchema: emptyPayloadSchema() },
      { actionType: "stage_use_verifier", payloadSchema: emptyPayloadSchema() },
      { actionType: "stage_accept_preservation_agreement", payloadSchema: emptyPayloadSchema() },
      { actionType: "stage_prepare_collection", payloadSchema: emptyPayloadSchema() },
      { actionType: "stage_energize_preservation_equipment", payloadSchema: emptyPayloadSchema() },
      { actionType: "stage_open_service_hatch", payloadSchema: emptyPayloadSchema() },
      {
        actionType: "stage_recover_candidate",
        payloadSchema: {
          additionalProperties: false,
          properties: {
            candidate_id: { enum: ["ledger-amber", "ledger-cobalt", "ledger-violet"] },
          },
          required: ["candidate_id"],
          type: "object",
        },
      },
      { actionType: "stage_protect_source_record", payloadSchema: emptyPayloadSchema() },
      { actionType: "stage_extract", payloadSchema: emptyPayloadSchema() },
      { actionType: "stage_wait", payloadSchema: emptyPayloadSchema() },
      { actionType: "commit_turn", payloadSchema: emptyPayloadSchema() },
      {
        actionType: "assign_mira_task",
        payloadSchema: {
          additionalProperties: false,
          properties: {
            power_allowance: { enum: [0, 1, 2] },
            task_kind: { enum: ["investigate_records", "investigate_conservation", "open_service_hatch", "field_assay"] },
          },
          required: ["task_kind", "power_allowance"],
          type: "object",
        },
      },
      { actionType: "cancel_mira_task", payloadSchema: emptyPayloadSchema() },
      { actionType: "set_mira_follow", payloadSchema: emptyPayloadSchema() },
      { actionType: "set_mira_hold", payloadSchema: emptyPayloadSchema() },
      { actionType: "set_mira_regroup", payloadSchema: emptyPayloadSchema() },
      { actionType: "request_mira_plan", payloadSchema: emptyPayloadSchema() },
      { actionType: "prepare_mira_contribution", payloadSchema: emptyPayloadSchema() },
      { actionType: "defer_mira_contribution", payloadSchema: emptyPayloadSchema() },
      {
        actionType: "assign_jonah_task",
        payloadSchema: {
          additionalProperties: false,
          properties: {
            power_allowance: { enum: [0, 1] },
            task_kind: { enum: ["investigate_records", "investigate_conservation", "open_service_hatch"] },
          },
          required: ["task_kind", "power_allowance"],
          type: "object",
        },
      },
      { actionType: "cancel_jonah_task", payloadSchema: emptyPayloadSchema() },
      { actionType: "set_jonah_follow", payloadSchema: emptyPayloadSchema() },
      { actionType: "set_jonah_hold", payloadSchema: emptyPayloadSchema() },
      { actionType: "set_jonah_regroup", payloadSchema: emptyPayloadSchema() },
      { actionType: "request_jonah_plan", payloadSchema: emptyPayloadSchema() },
      { actionType: "prepare_jonah_contribution", payloadSchema: emptyPayloadSchema() },
      { actionType: "defer_jonah_contribution", payloadSchema: emptyPayloadSchema() },
      { actionType: "prepare_extraction", payloadSchema: emptyPayloadSchema() },
      { actionType: "acknowledge_extraction", payloadSchema: {
        additionalProperties: false, type: "object",
        properties: { preview_revision: { type: "integer", minimum: 1, maximum: 65535 },
          left_behind_roles: { type: "array", items: { enum: ["mira", "jonah"] }, minItems: 0, maxItems: 2 } },
        required: ["preview_revision", "left_behind_roles"],
      } },
      {
        actionType: "submit_companion_plan",
        payloadSchema: {
          additionalProperties: false,
          properties: {
            dialogue: dialogueTextSchema(),
            opportunity_revision: { maximum: 65_535, minimum: 1, type: "integer" },
            steps: {
              items: {
                additionalProperties: false,
                properties: {
                  destination: { enum: ["none", "atrium", "records", "conservation", "plant", "vault"] },
                  power_cost: { enum: [0, 1, 2] },
                  source_id: { enum: ["none", "records", "conservation"] },
                  step_type: { enum: ["move", "inspect_source", "share_source", "use_verifier", "open_service_hatch", "collect_assay_sample", "complete_field_assay"] },
                },
                required: ["step_type", "destination", "source_id", "power_cost"],
                type: "object",
              },
              maxItems: 3,
              minItems: 1,
              type: "array",
            },
            task_revision: { maximum: 65_535, minimum: 1, type: "integer" },
          },
          required: ["task_revision", "opportunity_revision", "steps", "dialogue"],
          type: "object",
        },
      },
    ],
    rejectionCodes: [
      "inactive",
      "role_violation",
      "invalid_payload",
      "illegal_action",
      "nothing_staged",
      "insufficient_power",
      "gate_closed",
      "unknown_candidate",
      "companion_unavailable",
      "stale_plan",
      "task_violation",
      "plan_invalid",
      "preparation_required",
      "crew_not_regrouped",
      "turn_conflict", "extraction_preview_required", "extraction_preview_stale",
      "core_role_invariant",
    ],
    events: [
      { eventType: "archive_session_expired", payloadSchema: { additionalProperties: false, type: "object",
        properties: { event_type: { const: "archive_session_expired" }, deadline: { type: "string", minLength: 20, maxLength: 30 } },
        required: ["event_type", "deadline"] } },
      { eventType: "companion_dialogue_recorded", payloadSchema: { ...dialogueRecordSchema(), properties: { ...record(dialogueRecordSchema().properties, "properties"), event_type: { const: "companion_dialogue_recorded" } }, required: [...dialogueRecordSchema().required as string[], "event_type"] } },
      { eventType: "archive_started", payloadSchema: archiveStartedSchema() },
      { eventType: "action_staged", payloadSchema: actionStagedSchema() },
      { eventType: "turn_committed", payloadSchema: turnCommittedSchema() },
      { eventType: "companion_state_updated", payloadSchema: companionStateUpdatedSchema() },
      { eventType: "extraction_updated", payloadSchema: { additionalProperties: false, type: "object",
        properties: { event_type: { const: "extraction_updated" }, status: { enum: ["prepared", "acknowledged"] }, preview_revision: { type: "integer", minimum: 1, maximum: 65535 } },
        required: ["event_type", "status", "preview_revision"],
      } },
    ],
    attentionReasons: [MIRA_PLAN_ATTENTION_REASON],
    configurationSchema: {
      additionalProperties: false,
      properties: { scenario_id: { enum: ["standard-v1", "low-reserve-v1"], type: "string" } },
      required: ["scenario_id"],
      type: "object",
    },
    stateSchema: stateSchema(),
    projectionSchemas: {
      public: publicProjectionSchema(),
      participant: participantProjectionSchema(),
      operator: publicProjectionSchema(),
      historical_public: publicProjectionSchema(),
      historical_participant: participantProjectionSchema(),
      historical_operator: publicProjectionSchema(),
      final_reveal: publicProjectionSchema(),
    },
    observationSchemas: {
      public: publicProjectionSchema(),
      participant: participantProjectionSchema(),
      operator: publicProjectionSchema(),
      historical_public: publicProjectionSchema(),
      historical_participant: participantProjectionSchema(),
      historical_operator: publicProjectionSchema(),
      final_reveal: publicProjectionSchema(),
    },
    externalInputSchemas: {
      [ACTIVITY_START_INPUT_TYPE]: {
        additionalProperties: false,
        properties: { opened_by: { const: "host" } },
        required: ["opened_by"],
        type: "object",
      },
    },
    activityStartContract: {
      contract: ACTIVITY_START_CONTRACT_ID,
      preStartPhase: "briefing",
      inputType: ACTIVITY_START_INPUT_TYPE,
      canonicalPayload: { opened_by: "host" },
    },
  },

  initialize(input) {
    const configuration = record(input.configuration, "configuration");
    validateCoreInvariant(record(input.initial_core_state, "initial_core_state"));
    const core = record(input.initial_core_state, "initial_core_state");
    const state = initializeArchiveState(configuration);
    return {
      initial_activity_state: stateAsCanonical({ ...state,
        mira: initialCompanionFromCore(core, "mira"), jonah: initialCompanionFromCore(core, "jonah"),
        starting_crew: (["lead", "mira", "jonah"] as const).flatMap((role) => Object.values(record(core.memberships, "Core memberships"))
          .map((value) => record(value, "Membership")).filter((member) => member.role === role)
          .map((member) => ({ role, member_id: stringValue(member.member_id, "member_id") }))),
      }),
      timer_requests: [],
    };
  },

  reduce(input) {
    return reduceArchive(input);
  },

  view(input) {
    const result = authorizedView(
      stateValue(input.activity_state),
      record(input.core, "core"),
      record(input.viewer, "viewer"),
    );
    return {
      projection_schema: result.schema,
      projection: result.projection,
      action_offers: result.actionOffers,
    };
  },

  observe(input) {
    const afterView = record(input.after_view, "after_view");
    stringValue(afterView.projection_schema, "after projection schema");
    const viewer = record(input.viewer, "viewer");
    const beforeView = authorizedView(
      stateValue(input.activity_before),
      record(input.core_before, "core_before"),
      viewer,
    );
    const afterOwnedView = authorizedView(
      stateValue(input.activity_after),
      record(input.core_after, "core_after"),
      viewer,
    );
    const afterOffers = afterView.action_offers;
    if (!Array.isArray(afterOffers)) {
      throw new TypeError("after_view action_offers must be an array");
    }
    const offersChanged = canonicalStringify(beforeView.actionOffers as CanonicalJson) !==
      canonicalStringify(afterOwnedView.actionOffers as CanonicalJson);
    return {
      observation_schema: afterOwnedView.schema,
      observation: record(afterView.projection, "after projection"),
      action_offers: offersChanged ? "reuse_after_view" : "unchanged",
    };
  },
} satisfies ActivityPackDefinition);
