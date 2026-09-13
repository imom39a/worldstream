import type { CanonicalObject } from "@worldstream/pack-sdk";

function sourceSchema(): CanonicalObject {
  return {
    additionalProperties: false,
    properties: {
      arrival_minute: { maximum: 1_000_000, minimum: 0, type: "integer" },
      arrival_version: { maximum: 1_000_000, minimum: 1, type: "integer" },
      connection_id: { maxLength: 32, minLength: 1, type: "string" },
      departure_minute: { maximum: 1_000_000, minimum: 0, type: "integer" },
      departure_version: { maximum: 1_000_000, minimum: 1, type: "integer" },
      minimum_transfer_minutes: { maximum: 1_000, minimum: 0, type: "integer" },
    },
    required: ["arrival_minute", "arrival_version", "connection_id", "departure_minute", "departure_version", "minimum_transfer_minutes"],
    type: "object",
  };
}

function assessmentSchema(): CanonicalObject {
  return {
    additionalProperties: false,
    properties: {
      assessor_member_id: { maxLength: 64, minLength: 1, type: "string" },
      arrival_version: { maximum: 1_000_000, minimum: 1, type: "integer" },
      claim: { enum: ["connection_feasible", "connection_at_risk"] },
      departure_version: { maximum: 1_000_000, minimum: 1, type: "integer" },
      reason_code: { enum: ["transfer_time_sufficient", "transfer_time_insufficient"] },
      validity: { enum: ["current", "superseded"] },
    },
    required: ["assessor_member_id", "arrival_version", "claim", "departure_version", "reason_code", "validity"],
    type: "object",
  };
}

function workSchema(): CanonicalObject {
  const assessment = assessmentSchema();
  const source = sourceSchema();
  void source;
  return {
  additionalProperties: false,
  properties: {
    connection_id: { maxLength: 32, minLength: 1, type: "string" },
    last_assessment: assessment,
    reason: { enum: ["initial", "arrival_changed", "departure_changed"] },
    revision: { maximum: 1_000_000, minimum: 1, type: "integer" },
    status: { enum: ["needs_assessment", "resolved"] },
    work_id: { maxLength: 32, minLength: 1, type: "string" },
  },
  required: ["connection_id", "reason", "revision", "status", "work_id"],
  type: "object",
  };
}

export function configurationSchema(): CanonicalObject {
  return {
    additionalProperties: false,
    description: "A bounded reference configuration; the active work limit is fixed at sixteen.",
    properties: { max_open_work: { const: 16, type: "integer" } },
    required: ["max_open_work"],
    type: "object",
  };
}

export function stateSchema(): CanonicalObject {
  const source = sourceSchema();
  const work = workSchema();
  return {
  additionalProperties: false,
  properties: {
    connections: { items: source, maxItems: 16, type: "array" },
    max_open_work: { const: 16, type: "integer" },
    objective: { maxLength: 256, minLength: 1, type: "string" },
    open_work: { items: work, maxItems: 16, type: "array" },
    phase: { const: "monitoring", type: "string" },
    role_guidance: {
      additionalProperties: false,
      properties: {
        analyst: { maxLength: 256, minLength: 1, type: "string" },
        reviewer: { maxLength: 256, minLength: 1, type: "string" },
      },
      required: ["analyst", "reviewer"],
      type: "object",
    },
    room_seq: { minimum: 0, type: "integer" },
  },
  required: ["connections", "max_open_work", "objective", "open_work", "phase", "role_guidance", "room_seq"],
  type: "object",
  };
}

export function sourceActionSchema(): CanonicalObject {
  const source = sourceSchema();
  return {
  additionalProperties: false,
  description: "Replace one current source fact with strictly newer source revisions.",
  properties: source.properties!,
  required: source.required!,
  type: "object",
  };
}

export function assessmentActionSchema(): CanonicalObject {
  return {
  additionalProperties: false,
  description: "Attribute one bounded assessment to the current source and work revisions.",
  properties: {
    arrival_version: { maximum: 1_000_000, minimum: 1, type: "integer" },
    claim: { enum: ["connection_feasible", "connection_at_risk"] },
    departure_version: { maximum: 1_000_000, minimum: 1, type: "integer" },
    expected_work_revision: { maximum: 1_000_000, minimum: 1, type: "integer" },
    reason_code: { enum: ["transfer_time_sufficient", "transfer_time_insufficient"] },
    work_id: { maxLength: 32, minLength: 1, type: "string" },
  },
  required: ["arrival_version", "claim", "departure_version", "expected_work_revision", "reason_code", "work_id"],
  type: "object",
  };
}

export function closeActionSchema(): CanonicalObject {
  return {
  additionalProperties: false,
  description: "Reserved completion spelling; this reference Pack deliberately requires evidence first.",
  properties: {
    expected_work_revision: { maximum: 1_000_000, minimum: 1, type: "integer" },
    work_id: { maxLength: 32, minLength: 1, type: "string" },
  },
  required: ["expected_work_revision", "work_id"],
  type: "object",
  };
}

export function participantProjectionSchema(): CanonicalObject {
  const source = sourceSchema();
  const work = workSchema();
  return {
  additionalProperties: false,
  properties: {
    action_policy: {
      additionalProperties: false,
      properties: {
        can_record_source_update: { type: "boolean" },
        can_record_assessment: { type: "boolean" },
      },
      required: ["can_record_assessment", "can_record_source_update"],
      type: "object",
    },
    connections: { items: source, maxItems: 16, type: "array" },
    objective: { maxLength: 256, minLength: 1, type: "string" },
    open_work: { items: work, maxItems: 16, type: "array" },
    phase: { const: "monitoring", type: "string" },
    role: { enum: ["analyst", "reviewer"] },
    role_guidance: { maxLength: 256, minLength: 1, type: "string" },
  },
  required: ["action_policy", "connections", "objective", "open_work", "phase", "role", "role_guidance"],
  type: "object",
  };
}

export function publicProjectionSchema(): CanonicalObject {
  return {
  additionalProperties: false,
  properties: {
    open_work_count: { minimum: 0, type: "integer" },
    objective: { maxLength: 256, minLength: 1, type: "string" },
    phase: { const: "monitoring", type: "string" },
    source_count: { minimum: 0, type: "integer" },
  },
  required: ["objective", "open_work_count", "phase", "source_count"],
  type: "object",
  };
}
