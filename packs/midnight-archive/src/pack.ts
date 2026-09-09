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
  participantProjectionSchema,
  publicProjectionSchema,
  stateSchema,
} from "./schemas.js";
import {
  applyLeadAction,
  initializeArchiveState,
  payload,
  startArchive,
  stateAsCanonical,
} from "./rules.js";
import { authorizedView } from "./view.js";

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
          "stage_inspect_records", "stage_inspect_conservation", "stage_recover_candidate", "stage_extract", "stage_wait",
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
          "stage_inspect_records", "stage_inspect_conservation", "stage_recover_candidate", "stage_extract", "stage_wait",
        ],
      },
      event_type: { const: "turn_committed" },
      location: { enum: ["atrium", "records", "conservation", "plant", "vault"] },
      outcome_kind: {
        enum: ["pending", "success", "wrong_ledger", "no_ledger", "exhausted_inside"],
      },
      power: { maximum: 3, minimum: 0, type: "integer" },
      turn: { maximum: 16, minimum: 1, type: "integer" },
    },
    required: [
      "event_type", "committed_action_type", "turn", "location", "power", "outcome_kind",
    ],
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
      if (roleForMember(coreBefore, memberId) !== "lead") {
        throw new RuleRejection("role_violation", "only the human lead acts in this release");
      }
      const applied = applyLeadAction(current, actionType, payload(stimulus.canonical_payload));
      return {
        activity_disposition_type: "apply",
        next_activity_state: stateAsCanonical(applied.state),
        ordered_domain_events: [applied.event],
        timer_requests: [],
        ordered_attention_signals: [],
      };
    }
    if (stimulusType === "core_proposed") {
      return {
        activity_disposition_type: "apply",
        next_activity_state: stateAsCanonical(current),
        ordered_domain_events: [],
        timer_requests: [],
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
  const next = startArchive(current);
  return {
    activity_disposition_type: "apply",
    next_activity_state: stateAsCanonical(next),
    ordered_domain_events: [{ event_type: "archive_started", phase: "active" }],
    timer_requests: [],
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
    if (
      membership.access_mode !== "participant" ||
      membership.principal_kind !== expectedKind ||
      membership.standing !== "enabled"
    ) {
      throw new RuleRejection(
        "core_role_invariant",
        `${role} must remain an enabled ${expectedKind} Participant`,
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
      { actionType: "stage_extract", payloadSchema: emptyPayloadSchema() },
      { actionType: "stage_wait", payloadSchema: emptyPayloadSchema() },
      { actionType: "commit_turn", payloadSchema: emptyPayloadSchema() },
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
      "core_role_invariant",
    ],
    events: [
      { eventType: "archive_started", payloadSchema: archiveStartedSchema() },
      { eventType: "action_staged", payloadSchema: actionStagedSchema() },
      { eventType: "turn_committed", payloadSchema: turnCommittedSchema() },
    ],
    attentionReasons: [],
    configurationSchema: {
      additionalProperties: false,
      properties: { scenario_id: { const: "standard-v1", type: "string" } },
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
    return {
      initial_activity_state: stateAsCanonical(initializeArchiveState(configuration)),
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
