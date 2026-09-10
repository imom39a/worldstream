import type {
  ActivityPackDefinition,
  CanonicalJson,
  CanonicalObject,
} from "@worldstream/pack-sdk";
import { ACTIVITY_START_SOURCE_ID } from "@worldstream/pack-sdk";

type Role = "lead" | "mira" | "jonah";

interface ArchiveState {
  readonly phase: "briefing" | "active";
  readonly inspections: number;
  readonly lead_secret: string;
  readonly mira_secret: string;
  readonly jonah_secret: string;
}

function stateSchema(): CanonicalObject {
  return {
    additionalProperties: false,
    properties: {
      inspections: { type: "integer" },
      jonah_secret: { maxLength: 16, type: "string" },
      lead_secret: { maxLength: 16, type: "string" },
      mira_secret: { maxLength: 16, type: "string" },
      phase: { enum: ["briefing", "active"] },
    },
    required: ["phase", "inspections", "lead_secret", "mira_secret", "jonah_secret"],
    type: "object",
  } as CanonicalObject;
}

function publicSchema(): CanonicalObject {
  return {
    additionalProperties: false,
    properties: {
      inspections: { type: "integer" },
      phase: { enum: ["briefing", "active"] },
    },
    required: ["phase", "inspections"],
    type: "object",
  } as CanonicalObject;
}

function participantSchema(): CanonicalObject {
  return {
    additionalProperties: false,
    properties: {
      inspections: { type: "integer" },
      phase: { enum: ["briefing", "active"] },
      private_note: { maxLength: 16, type: "string" },
    },
    required: ["phase", "inspections", "private_note"],
    type: "object",
  } as CanonicalObject;
}

function record(value: CanonicalJson | undefined, label: string): CanonicalObject {
  if (value === null || value === undefined || Array.isArray(value) || typeof value !== "object") {
    throw new TypeError(`${label} must be an object`);
  }
  return value as CanonicalObject;
}

function state(value: CanonicalJson | undefined): ArchiveState {
  return record(value, "Activity State") as unknown as ArchiveState;
}

function roleFor(input: CanonicalObject): Role | null {
  const viewer = record(input.viewer, "viewer");
  if (
    (viewer.viewer_type !== "participant" && viewer.viewer_type !== "historical") ||
    typeof viewer.member_id !== "string"
  ) return null;
  const memberships = record(record(input.core ?? input.core_after, "core").memberships, "core.memberships");
  const membership = record(memberships[viewer.member_id], "viewer membership");
  return membership.role === "lead" || membership.role === "mira" || membership.role === "jonah"
    ? membership.role
    : null;
}

function audienceSchema(input: CanonicalObject, isParticipant: boolean):
  | "public"
  | "participant"
  | "operator"
  | "historical_public"
  | "historical_participant"
  | "historical_operator"
  | "final_reveal" {
  const viewer = record(input.viewer, "viewer");
  if (viewer.viewer_type === "participant") return "participant";
  if (viewer.viewer_type === "operator") return "operator";
  if (viewer.viewer_type === "final_reveal") return "final_reveal";
  if (viewer.viewer_type !== "historical") return "public";
  const memberships = record(record(input.core ?? input.core_after, "core").memberships, "core.memberships");
  const membership = record(memberships[String(viewer.member_id)], "viewer membership");
  if (membership.access_mode === "operator") return "historical_operator";
  return isParticipant ? "historical_participant" : "historical_public";
}

function privateNote(value: ArchiveState, role: Role): string {
  return role === "lead" ? value.lead_secret : role === "mira" ? value.mira_secret : value.jonah_secret;
}

export default {
  descriptor: {
    actions: [{
      actionType: "inspect",
      payloadSchema: {
        additionalProperties: false,
        properties: { target: { enum: ["records"] } },
        required: ["target"],
        type: "object",
      },
    }],
    attentionReasons: [],
    configurationSchema: {
      additionalProperties: false,
      properties: { archive_id: { enum: ["midnight"] } },
      required: ["archive_id"],
      type: "object",
    },
    events: [{
      eventType: "record_inspected",
      payloadSchema: {
        additionalProperties: false,
        properties: {
          event_type: { enum: ["record_inspected"] },
          inspections: { type: "integer" },
        },
        required: ["event_type", "inspections"],
        type: "object",
      },
    }],
    externalInputSchemas: {
      "archive.fixture/briefing-opened/v1": {
        additionalProperties: false,
        properties: { opened_by: { enum: ["host"] } },
        required: ["opened_by"],
        type: "object",
      },
    },
    activityStartContract: {
      canonicalPayload: { opened_by: "host" },
      contract: "worldstream/activity-start/v1",
      inputType: "archive.fixture/briefing-opened/v1",
      preStartPhase: "briefing",
    },
    name: "Archive Contract Fixture",
    observationSchemas: {
      final_reveal: publicSchema(),
      historical_operator: publicSchema(),
      historical_participant: participantSchema(),
      historical_public: publicSchema(),
      operator: publicSchema(),
      participant: participantSchema(),
      public: publicSchema(),
    },
    packId: "worldstream.archive-contract-fixture",
    projectionSchemas: {
      final_reveal: publicSchema(),
      historical_operator: publicSchema(),
      historical_participant: participantSchema(),
      historical_public: publicSchema(),
      operator: publicSchema(),
      participant: participantSchema(),
      public: publicSchema(),
    },
    rejectionCodes: ["inactive", "invalid_target"],
    roles: [
      { maximum: 1, minimum: 1, role: "lead" },
      { maximum: 1, minimum: 0, role: "mira" },
      { maximum: 1, minimum: 0, role: "jonah" },
    ],
    stateSchema: stateSchema(),
    schemaVersion: 2,
    version: "0.1.0",
  },

  initialize() {
    return {
      initial_activity_state: {
        inspections: 0,
        jonah_secret: "violet",
        lead_secret: "amber",
        mira_secret: "cobalt",
        phase: "briefing",
      },
      timer_requests: [],
    };
  },

  reduce(input) {
    const current = state(input.prior_activity_state);
    const stimulus = record(input.recorded_stimulus, "stimulus");
    if (stimulus.stimulus_type === "external_input") {
      const external = record(stimulus, "ExternalInput");
      if (
        current.phase !== "briefing" ||
        external.source_id !== ACTIVITY_START_SOURCE_ID ||
        external.input_type !== "archive.fixture/briefing-opened/v1" ||
        JSON.stringify(external.canonical_payload) !== JSON.stringify({ opened_by: "host" }) ||
        !Array.isArray(external.immutable_resource_references) ||
        external.immutable_resource_references.length !== 0
      ) {
        return { activity_disposition_type: "reject", bounded_safe_details: {}, declared_code: "inactive" };
      }
      return {
        activity_disposition_type: "apply",
        next_activity_state: { ...current, phase: "active" },
        ordered_attention_signals: [],
        ordered_domain_events: [],
        timer_requests: [],
      };
    }
    const payload = record(stimulus.canonical_payload, "Action payload");
    if (current.phase !== "active") {
      return { activity_disposition_type: "reject", bounded_safe_details: {}, declared_code: "inactive" };
    }
    if (payload.target !== "records") {
      return { activity_disposition_type: "reject", bounded_safe_details: {}, declared_code: "invalid_target" };
    }
    const inspections = current.inspections + 1;
    return {
      activity_disposition_type: "apply",
      next_activity_state: { ...current, inspections },
      ordered_attention_signals: [],
      ordered_domain_events: [{ event_type: "record_inspected", inspections }],
      timer_requests: [],
    };
  },

  view(input) {
    const current = state(input.activity_state);
    const role = roleFor(input);
    const receivesPrivateProjection = role !== null;
    const liveParticipant = record(input.viewer, "viewer").viewer_type === "participant" && role !== null;
    const projection = receivesPrivateProjection
      ? { inspections: current.inspections, phase: current.phase, private_note: privateNote(current, role) }
      : { inspections: current.inspections, phase: current.phase };
    return {
      action_offers: liveParticipant && current.phase === "active"
        ? [{
            actionType: "inspect",
            eligibilityWindow: {
              deadline: "2026-08-30T12:10:00Z",
              opensAt: "2026-08-30T12:00:00Z",
            },
          }]
        : [],
      projection,
      projection_schema: audienceSchema(input, receivesPrivateProjection),
    };
  },

  observe(input) {
    const after = record(input.after_view, "after view");
    const stimulus = record(input.recorded_stimulus, "recorded stimulus");
    return {
      action_offers: stimulus.stimulus_type === "external_input" &&
          Array.isArray(after.action_offers) && after.action_offers.length > 0
        ? "reuse_after_view"
        : "unchanged",
      observation: record(after.projection, "after view projection"),
      observation_schema: audienceSchema(
        input,
        String(after.projection_schema).includes("participant"),
      ),
    };
  },
} satisfies ActivityPackDefinition;
