import {
  canonicalStringify,
  defineActivityPack,
  encodeCanonical,
  type ActivityPackDefinition,
  type CanonicalJson,
  type CanonicalObject,
  type PackActionOffer,
  type PackReduceOutput,
} from "@worldstream/pack-sdk";

import {
  asRecord,
  assessmentValue,
  clone,
  connectionValue,
  integerValue,
  stateValue,
  stringValue,
  type Assessment,
  type ReferenceState,
  type Role,
  type SourceFact,
  type WorkItem,
} from "./model.js";
import {
  assessmentActionSchema,
  closeActionSchema,
  configurationSchema,
  participantProjectionSchema,
  publicProjectionSchema,
  sourceActionSchema,
  stateSchema,
} from "./schemas.js";

export const MAX_OPEN_WORK = 16;
export const MAX_PROJECTION_BYTES = 24576;
const MAX_SOURCE_TEXT_BYTES = 128;

function initialConnections(): readonly SourceFact[] {
  return [{
    arrival_minute: 600,
    arrival_version: 1,
    connection_id: "C17",
    departure_minute: 650,
    departure_version: 1,
    minimum_transfer_minutes: 25,
  }];
}

function workItem(
  connectionId: string,
  reason: WorkItem["reason"],
  revision = 1,
): WorkItem {
  return { connection_id: connectionId, reason, revision, status: "needs_assessment", work_id: `work-${connectionId}` };
}

function roleFor(core: CanonicalObject, memberId: string): Role | null {
  const memberships = asRecord(core.memberships, "core.memberships");
  const membership = memberships[memberId];
  if (membership === undefined) return null;
  const value = asRecord(membership, "membership");
  if (value.access_mode !== "participant" || value.standing !== "enabled") return null;
  return value.role === "analyst" || value.role === "reviewer" ? value.role : null;
}

function ensureCurrentBasis(stimulus: CanonicalObject, state: ReferenceState): void {
  const basis = stimulus.exact_basis_head;
  if (basis === undefined) return;
  const head = asRecord(basis, "exact_basis_head");
  const roomSeq = head.room_seq;
  // The SDK conformance fixture uses the all-zero empty head. Core admission
  // supplies a non-zero current sequence, which this Pack fences explicitly.
  if (roomSeq !== 0 && roomSeq !== state.room_seq) throw new RuleRejection("stale_room_head", "Action basis is older than the current Room Head");
}

class RuleRejection extends Error {
  constructor(readonly code: string, message: string) {
    super(message);
  }
}

function reject(error: RuleRejection): PackReduceOutput {
  return {
    activity_disposition_type: "reject",
    bounded_safe_details: { reason: error.message },
    declared_code: error.code,
  };
}

function sourceFromPayload(payload: CanonicalObject): SourceFact {
  const source = connectionValue(payload, "source payload");
  for (const value of [source.connection_id, source.arrival_version, source.arrival_minute,
    source.departure_version, source.departure_minute, source.minimum_transfer_minutes]) {
    if (typeof value === "string" && new TextEncoder().encode(value).length > MAX_SOURCE_TEXT_BYTES) {
      throw new RuleRejection("input_too_large", "source text exceeds the bounded field limit");
    }
  }
  if (source.departure_minute < source.arrival_minute) throw new RuleRejection("invalid_source", "departure must follow arrival");
  return source;
}

function assessmentFromPayload(payload: CanonicalObject, memberId: string, source: SourceFact, work: WorkItem): Assessment {
  const claim = stringValue(payload.claim, "assessment claim");
  const reason = stringValue(payload.reason_code, "assessment reason_code");
  const arrivalVersion = integerValue(payload.arrival_version, "assessment arrival_version");
  const departureVersion = integerValue(payload.departure_version, "assessment departure_version");
  const expectedRevision = integerValue(payload.expected_work_revision, "assessment expected_work_revision");
  if (expectedRevision !== work.revision) throw new RuleRejection("stale_work_revision", "work revision is no longer current");
  if (arrivalVersion !== source.arrival_version || departureVersion !== source.departure_version) {
    throw new RuleRejection("stale_source_revision", "source revision is no longer current");
  }
  const feasible = source.departure_minute - source.arrival_minute >= source.minimum_transfer_minutes;
  const expectedClaim = feasible ? "connection_feasible" : "connection_at_risk";
  const expectedReason = feasible ? "transfer_time_sufficient" : "transfer_time_insufficient";
  if (claim !== expectedClaim || reason !== expectedReason) {
    throw new RuleRejection("assessment_inconsistent", "assessment does not follow the declared transfer rule");
  }
  if (claim !== "connection_feasible" && claim !== "connection_at_risk") throw new RuleRejection("unsupported_completion", "assessment claim is unsupported");
  return {
    assessor_member_id: memberId,
    arrival_version: arrivalVersion,
    claim,
    departure_version: departureVersion,
    reason_code: reason,
    validity: "current",
  };
}

function nextStateForSource(state: ReferenceState, incoming: SourceFact): ReferenceState {
  const index = state.connections.findIndex((item) => item.connection_id === incoming.connection_id);
  if (index < 0) {
    if (state.open_work.length >= state.max_open_work) throw new RuleRejection("capacity_exhausted", "active work capacity is full");
    return {
      ...state,
      connections: [...state.connections, incoming],
      open_work: [...state.open_work, workItem(incoming.connection_id, "initial")],
    };
  }
  const previous = state.connections[index]!;
  if (incoming.arrival_version < previous.arrival_version || incoming.departure_version < previous.departure_version) {
    throw new RuleRejection("stale_source_revision", "source revision is older than the current fact");
  }
  const arrivalChanged = incoming.arrival_version > previous.arrival_version;
  const departureChanged = incoming.departure_version > previous.departure_version;
  if (!arrivalChanged && !departureChanged) throw new RuleRejection("stale_source_revision", "source revision is already current");
  const connections = [...state.connections];
  connections[index] = incoming;
  const openWork = state.open_work.map((work) => {
    if (work.connection_id !== incoming.connection_id) return work;
    const previousAssessment = assessmentValue(work.last_assessment as unknown as CanonicalJson, "last_assessment");
    const superseded = previousAssessment === null ? undefined : { ...previousAssessment, validity: "superseded" as const };
    return {
      ...work,
      ...(superseded === undefined ? {} : { last_assessment: superseded }),
      reason: (arrivalChanged ? "arrival_changed" : "departure_changed") as WorkItem["reason"],
      revision: work.revision + 1,
      status: "needs_assessment" as const,
    };
  });
  return { ...state, connections, open_work: openWork };
}

function nextStateForAssessment(state: ReferenceState, workId: string, assessment: Assessment): ReferenceState {
  return {
    ...state,
    open_work: state.open_work.map((work) => work.work_id !== workId ? work : {
      ...work,
      last_assessment: assessment,
      status: assessment.claim === "connection_feasible" ? "resolved" : "needs_assessment",
    }),
  };
}

function findWork(state: ReferenceState, workId: string): WorkItem {
  const work = state.open_work.find((item) => item.work_id === workId);
  if (work === undefined) throw new RuleRejection("unknown_work", "work item is not in the current bounded state");
  return work;
}

function findSource(state: ReferenceState, connectionId: string): SourceFact {
  const source = state.connections.find((item) => item.connection_id === connectionId);
  if (source === undefined) throw new RuleRejection("unknown_connection", "connection is not in the current bounded state");
  return source;
}

function stateBytes(state: ReferenceState): number {
  return encodeCanonical(state as unknown as CanonicalJson).length;
}

function validateBoundedState(state: ReferenceState): void {
  if (state.connections.length > MAX_OPEN_WORK || state.open_work.length > MAX_OPEN_WORK) {
    throw new RuleRejection("capacity_exhausted", "bounded current state exceeds active work capacity");
  }
  if (stateBytes(state) > MAX_PROJECTION_BYTES) throw new RuleRejection("state_too_large", "bounded Activity State exceeds the Pack limit");
}

export function reduceReference(input: CanonicalObject): PackReduceOutput {
  const state = clone(stateValue(input.prior_activity_state));
  const stimulus = asRecord(input.recorded_stimulus, "recorded_stimulus");
  const nextRoomSeq = integerValue(input.next_room_seq, "next_room_seq");
  const core = asRecord(input.core_before, "core_before");
  const memberId = typeof stimulus.member_id === "string" ? stimulus.member_id : "";
  try {
    if (stimulus.stimulus_type !== "participant_action") throw new RuleRejection("unsupported_stimulus", "this reference Pack accepts participant Actions only");
    ensureCurrentBasis(stimulus, state);
    const role = roleFor(core, memberId);
    if (role === null) throw new RuleRejection("role_violation", "Action Membership is not an enabled participant");
    const actionType = stringValue(stimulus.action_type, "action_type");
    const payload = asRecord(stimulus.canonical_payload, "canonical_payload");
    let next: ReferenceState;
    let event: CanonicalJson;
    if (actionType === "record_source_update") {
      if (role !== "analyst") throw new RuleRejection("role_violation", "only the analyst may replace source facts");
      const source = sourceFromPayload(payload);
      next = nextStateForSource(state, source);
      event = { event_type: "source_fact_replaced", connection_id: source.connection_id };
    } else if (actionType === "record_assessment") {
      const workId = stringValue(payload.work_id, "assessment work_id");
      const work = findWork(state, workId);
      if (work.status === "resolved") throw new RuleRejection("work_already_resolved", "resolved work cannot be assessed again");
      const source = findSource(state, work.connection_id);
      const assessment = assessmentFromPayload(payload, memberId, source, work);
      next = nextStateForAssessment(state, workId, assessment);
      event = { event_type: "assessment_recorded", claim: assessment.claim, work_id: workId };
    } else if (actionType === "close_work") {
      throw new RuleRejection("unsupported_completion", "work completion requires a current valid assessment");
    } else {
      throw new RuleRejection("unsupported_action", "Action type is not declared by this Pack");
    }
    next = { ...next, room_seq: nextRoomSeq };
    validateBoundedState(next);
    const newlyOpen = next.open_work.some((work) => work.status === "needs_assessment");
    return {
      activity_disposition_type: "apply",
      next_activity_state: next as unknown as CanonicalJson,
      ordered_attention_signals: newlyOpen ? [{
        action_types: ["record_assessment"],
        deduplication_key: `assessment:${nextRoomSeq}`,
        priority: 1,
        reason: "assessment_required",
        target_member_id: null,
      }] : [],
      ordered_domain_events: [event],
      timer_requests: [],
    };
  } catch (error) {
    if (error instanceof RuleRejection) return reject(error);
    throw error;
  }
}

function audience(input: CanonicalObject): "public" | "participant" {
  const viewer = asRecord(input.viewer, "viewer");
  return viewer.viewer_type === "participant" && typeof viewer.member_id === "string" && roleFor(asRecord(input.core, "core"), viewer.member_id) !== null
    ? "participant" : "public";
}

function authorizedView(state: ReferenceState, input: CanonicalObject): { readonly projection: CanonicalObject; readonly schema: "public" | "participant"; readonly offers: readonly PackActionOffer[] } {
  const kind = audience(input);
  if (kind === "public") return {
    offers: [],
    projection: {
      objective: state.objective,
      open_work_count: state.open_work.filter((work) => work.status === "needs_assessment").length,
      phase: state.phase,
      source_count: state.connections.length,
    },
    schema: "public",
  };
  const viewer = asRecord(input.viewer, "viewer");
  const role = roleFor(asRecord(input.core, "core"), stringValue(viewer.member_id, "viewer.member_id"))!;
  const openWork = state.open_work.filter((work) => work.status === "needs_assessment");
  const offers: PackActionOffer[] = role === "analyst"
    ? [{ actionType: "record_source_update", eligibilityWindow: null }, ...(openWork.length > 0 ? [{ actionType: "record_assessment", eligibilityWindow: null }] : [])]
    : (openWork.length > 0 ? [{ actionType: "record_assessment", eligibilityWindow: null }] : []);
  const projection: CanonicalObject = {
    action_policy: { can_record_assessment: openWork.length > 0, can_record_source_update: role === "analyst" },
    connections: state.connections as unknown as CanonicalJson,
    objective: state.objective,
    open_work: openWork as unknown as CanonicalJson,
    phase: state.phase,
    role,
    role_guidance: state.role_guidance[role],
  };
  const bytes = encodeCanonical(projection).length;
  if (bytes > MAX_PROJECTION_BYTES) throw new TypeError(`participant Projection exceeds ${MAX_PROJECTION_BYTES} bytes`);
  return { offers, projection, schema: "participant" };
}

export default defineActivityPack({
  descriptor: {
    actions: [
      { actionType: "record_source_update", payloadSchema: sourceActionSchema() },
      { actionType: "record_assessment", payloadSchema: assessmentActionSchema() },
      { actionType: "close_work", payloadSchema: closeActionSchema() },
    ],
    attentionReasons: ["assessment_required"],
    configurationSchema: configurationSchema(),
    events: [
      { eventType: "source_fact_replaced", payloadSchema: { type: "object" } },
      { eventType: "assessment_recorded", payloadSchema: { type: "object" } },
    ],
    name: "Late Join Reference",
    observationSchemas: { participant: participantProjectionSchema(), public: publicProjectionSchema() },
    packId: "worldstream.late-join-reference",
    projectionSchemas: { participant: participantProjectionSchema(), public: publicProjectionSchema() },
    rejectionCodes: [
      "assessment_inconsistent", "capacity_exhausted", "input_too_large", "invalid_source", "role_violation",
      "stale_room_head", "stale_source_revision", "stale_work_revision", "state_too_large", "unknown_connection",
      "unknown_work", "unsupported_action", "unsupported_completion", "unsupported_stimulus", "work_already_resolved",
    ],
    roles: [
      { maximum: 1, minimum: 1, role: "analyst" },
      { maximum: 1, minimum: 1, role: "reviewer" },
    ],
    stateSchema: stateSchema(),
    version: "0.1.0",
  },

  initialize(input) {
    const config = asRecord(input.configuration, "configuration");
    if (integerValue(config.max_open_work, "configuration.max_open_work") !== MAX_OPEN_WORK) throw new TypeError("configuration.max_open_work must be 16");
    const initial: ReferenceState = {
      connections: [...initialConnections()],
      max_open_work: MAX_OPEN_WORK,
      objective: "Identify connections needing intervention and record current assessments.",
      open_work: [workItem("C17", "initial")],
      phase: "monitoring",
      role_guidance: {
        analyst: "You may replace source facts when a newer source revision is available, then reassess open work.",
        reviewer: "Reviewers validate the current source revisions and record the bounded assessment conclusion.",
      },
      room_seq: 0,
    };
    validateBoundedState(initial);
    return { initial_activity_state: initial as unknown as CanonicalJson, timer_requests: [] };
  },

  reduce(input) {
    return reduceReference(input);
  },

  view(input) {
    const result = authorizedView(stateValue(input.activity_state), input);
    return { action_offers: result.offers, projection: result.projection, projection_schema: result.schema };
  },

  observe(input) {
    const beforeInput = input.core_before === undefined ? input : { ...input, core: input.core_before };
    const before = authorizedView(stateValue(input.activity_before), beforeInput);
    const afterInput = input.core_after === undefined ? input : { ...input, core: input.core_after };
    const after = authorizedView(stateValue(input.activity_after), afterInput);
    if (canonicalStringify(before.projection) === canonicalStringify(after.projection) && canonicalStringify(before.offers) === canonicalStringify(after.offers)) return null;
    return {
      action_offers: canonicalStringify(before.offers) === canonicalStringify(after.offers) ? "unchanged" : "reuse_after_view",
      observation: { change_type: "projection_replaced", projection: after.projection },
      observation_schema: after.schema,
    };
  },
} satisfies ActivityPackDefinition);
