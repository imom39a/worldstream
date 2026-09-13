import assert from "node:assert/strict";
import test from "node:test";

import { canonicalStringify, encodeCanonical, type CanonicalJson, type CanonicalObject } from "@worldstream/pack-sdk";

import { goldenFixture } from "../fixtures/golden.js";
import pack, { MAX_OPEN_WORK, MAX_PROJECTION_BYTES } from "../src/pack.js";
import { assessmentActionSchema, sourceActionSchema } from "../src/schemas.js";
import { contextDigest, DeterministicLateJoinRunner, RunnerBudgetError, type RunnerProjection } from "../src/runner.js";

const analyst = "late-analyst";
const reviewer = "late-reviewer";
const core: CanonicalObject = {
  memberships: {
    [analyst]: membership(analyst, "analyst"),
    [reviewer]: membership(reviewer, "reviewer"),
  },
  room_status: "active",
};

function membership(memberId: string, role: string): CanonicalObject {
  return {
    access_mode: "participant",
    member_id: memberId,
    principal_id: `principal-${memberId}`,
    principal_kind: "agent",
    role,
    standing: "enabled",
  };
}

function initialState(): CanonicalJson {
  return pack.initialize({
    configuration: { max_open_work: MAX_OPEN_WORK },
    created_at: "2026-09-12T15:00:00Z",
    initial_core_state: core,
  }).initial_activity_state;
}

function reduce(
  state: CanonicalJson,
  memberId: string,
  actionType: string,
  payload: CanonicalObject,
  sequence: number,
  basisSequence = 0,
): CanonicalObject {
  return pack.reduce({
    core_before: core,
    next_room_seq: sequence,
    prior_activity_state: state,
    proposed_core_after: core,
    recorded_stimulus: {
      action_id: `${memberId}-${sequence}`,
      action_type: actionType,
      admitted_at: "2026-09-12T15:00:00Z",
      canonical_payload: payload,
      exact_basis_head: {
        activity_state_hash: "blake3:" + "0".repeat(64),
        authoritative_state_hash: "blake3:" + "0".repeat(64),
        core_state_hash: "blake3:" + "0".repeat(64),
        room_seq: basisSequence,
      },
      member_id: memberId,
      payload_schema_digest: "blake3:" + "0".repeat(64),
      stimulus_type: "participant_action",
    },
    scheduled_timers: {},
  }) as CanonicalObject;
}

function participantView(state: CanonicalJson, memberId = reviewer): CanonicalObject {
  return pack.view({
    activity_state: state,
    complete_head: { room_seq: 0 },
    core,
    viewer: { member_id: memberId, viewer_type: "participant" },
  }) as CanonicalObject;
}

function stateAfterSourceChange(): CanonicalJson {
  const first = reduce(initialState(), analyst, "record_assessment", {
    arrival_version: 1,
    claim: "connection_feasible",
    departure_version: 1,
    expected_work_revision: 1,
    reason_code: "transfer_time_sufficient",
    work_id: "work-C17",
  }, 1);
  assert.equal(first.activity_disposition_type, "apply");
  const second = reduce(first.next_activity_state!, analyst, "record_source_update", {
    arrival_minute: 630,
    arrival_version: 2,
    connection_id: "C17",
    departure_minute: 650,
    departure_version: 1,
    minimum_transfer_minutes: 25,
  }, 2);
  assert.equal(second.activity_disposition_type, "apply");
  return second.next_activity_state!;
}

test("late join receives equivalent bounded context at small and 100k heads", () => {
  const current = stateAfterSourceChange();
  const small = JSON.parse(canonicalStringify(current)) as CanonicalObject;
  const large = { ...small, room_seq: 100_000 };
  const schemas = { record_assessment: assessmentActionSchema(), record_source_update: sourceActionSchema() };
  const brief = "Connection Monitor v1: assess every open work item against current source revisions.";
  const makeContext = (state: CanonicalJson) => {
    const view = participantView(state);
    return new DeterministicLateJoinRunner({ actionSchemas: schemas, memberId: reviewer, role: "reviewer", ruleBrief: brief })
      .attach(view as unknown as RunnerProjection);
  };
  const smallContext = makeContext(small);
  const largeContext = makeContext(large);
  assert.equal(contextDigest(smallContext), contextDigest(largeContext));
  assert.deepEqual(Object.keys(smallContext.action_schemas), ["record_assessment"]);
  assert.deepEqual(smallContext.projection.open_work, largeContext.projection.open_work);
  assert.ok(smallContext.encoded_bytes.total < 24 * 1024);
  assert.ok(smallContext.estimated_tokens > 0);
});

test("source revision supersedes assessment and a reviewer can contribute current evidence", () => {
  const state = stateAfterSourceChange();
  const superseded = ((state as CanonicalObject).open_work as readonly CanonicalObject[])[0]!.last_assessment as CanonicalObject;
  assert.equal(superseded.validity, "superseded");
  const view = participantView(state, reviewer);
  const runner = new DeterministicLateJoinRunner({
    actionSchemas: { record_assessment: assessmentActionSchema() },
    memberId: reviewer,
    role: "reviewer",
    ruleBrief: "Assess each current connection using its source and work revisions.",
  });
  const context = runner.attach(view as unknown as RunnerProjection);
  const proposal = runner.startContribution();
  if (proposal === null) throw new Error("reference fixture must have open work");
  assert.equal(proposal.canonical_payload.expected_work_revision, 2);
  assert.equal(proposal.canonical_payload.claim, "connection_at_risk");
  assert.equal((context.projection.open_work as readonly CanonicalJson[]).length, 1);
  const applied = reduce(state, reviewer, proposal.action_type, proposal.canonical_payload, 3);
  assert.equal(applied.activity_disposition_type, "apply");
  const work = (applied.next_activity_state as CanonicalObject).open_work as readonly CanonicalObject[];
  assert.equal(work[0]!.last_assessment && (work[0]!.last_assessment as CanonicalObject).validity, "current");
});

test("full observations replace the current view, while stale and lost replies stay fenced", () => {
  const before = initialState();
  const after = stateAfterSourceChange();
  const beforeView = participantView(before);
  const afterView = participantView(after);
  const runner = new DeterministicLateJoinRunner({
    actionSchemas: { record_assessment: assessmentActionSchema() },
    memberId: reviewer,
    role: "reviewer",
    ruleBrief: "Current facts are authoritative; refresh after stale proposals.",
  });
  runner.attach(beforeView as unknown as RunnerProjection);
  const proposal = runner.startContribution()!;
  const lost = runner.markLost(proposal.action_id);
  assert.deepEqual(lost, { action_id: proposal.action_id, kind: "lost_reply" });
  const retry = runner.retryLost(proposal.action_id);
  assert.equal(retry.action_id, proposal.action_id);
  runner.handleStale(proposal.action_id);
  assert.deepEqual(runner.pendingActionIds, []);
  const observation = pack.observe({
    activity_after: after,
    activity_before: before,
    after_view: afterView,
    core_after: core,
    core_before: core,
    ordered_domain_events: [],
    recorded_stimulus: { stimulus_type: "participant_action" },
    viewer: { member_id: reviewer, viewer_type: "participant" },
  }) as CanonicalObject;
  assert.equal((observation.observation as CanonicalObject).change_type, "projection_replaced");
  runner.applyObservation(observation.observation as CanonicalObject, afterView as unknown as RunnerProjection);
  assert.equal(runner.retainedViewCount, 1);
  assert.equal(runner.startContribution()!.canonical_payload.expected_work_revision, 2);
});

test("safe dispositions cover stale input, wrong Role, unsupported completion, and capacity", () => {
  const stale = reduce(initialState(), reviewer, "record_assessment", {
    arrival_version: 1,
    claim: "connection_feasible",
    departure_version: 1,
    expected_work_revision: 1,
    reason_code: "transfer_time_sufficient",
    work_id: "work-C17",
  }, 1, 3);
  assert.equal(stale.declared_code, "stale_room_head");
  const wrongRole = reduce(initialState(), reviewer, "record_source_update", {
    arrival_minute: 601,
    arrival_version: 2,
    connection_id: "C17",
    departure_minute: 650,
    departure_version: 1,
    minimum_transfer_minutes: 25,
  }, 1);
  assert.equal(wrongRole.declared_code, "role_violation");
  const unsupported = reduce(initialState(), reviewer, "close_work", { expected_work_revision: 1, work_id: "work-C17" }, 1);
  assert.equal(unsupported.declared_code, "unsupported_completion");

  const fullState: Record<string, CanonicalJson> = JSON.parse(canonicalStringify(initialState())) as Record<string, CanonicalJson>;
  fullState.connections = Array.from({ length: MAX_OPEN_WORK }, (_, index) => ({
    arrival_minute: 600,
    arrival_version: 1,
    connection_id: `C${index}`,
    departure_minute: 650,
    departure_version: 1,
    minimum_transfer_minutes: 25,
  }));
  fullState.open_work = Array.from({ length: MAX_OPEN_WORK }, (_, index) => ({
    connection_id: `C${index}`,
    reason: "initial",
    revision: 1,
    status: "needs_assessment",
    work_id: `work-C${index}`,
  }));
  const capacity = reduce(fullState, analyst, "record_source_update", {
    arrival_minute: 600,
    arrival_version: 1,
    connection_id: "new",
    departure_minute: 650,
    departure_version: 1,
    minimum_transfer_minutes: 25,
  }, 1);
  assert.equal(capacity.declared_code, "capacity_exhausted");
  assert.ok(encodeCanonical(participantView(initialState()).projection!).length < MAX_PROJECTION_BYTES);
});

test("a new Membership gets current public authorization only", () => {
  const view = pack.view({
    activity_state: initialState(),
    complete_head: { room_seq: 0 },
    core,
    viewer: { member_id: "new-member", viewer_type: "participant" },
  }) as CanonicalObject;
  assert.equal(view.projection_schema, "public");
  assert.deepEqual(view.action_offers, []);
  assert.equal(canonicalStringify(view.projection!).includes("role_guidance"), false);
});

test("Runner rejects oversized rule briefs before an Invocation exists", () => {
  assert.throws(() => new DeterministicLateJoinRunner({
    actionSchemas: {},
    memberId: reviewer,
    role: "reviewer",
    ruleBrief: "x".repeat(4 * 1024 + 1),
  }), RunnerBudgetError);
});
