import assert from "node:assert/strict";
import test from "node:test";
import { ACTIVITY_START_SOURCE_ID, canonicalStringify, type CanonicalObject } from "@worldstream/pack-sdk";
import pack, { ACTIVITY_START_INPUT_TYPE, reduceArchive } from "../src/pack.js";
import type { ArchiveState } from "../src/model.js";
import { authorizedView } from "../src/view.js";
import { MIRA_PLAN_TIMER_ID } from "../src/companions.js";
import { SESSION_TIMER_ID, sessionDeadline } from "../src/session.js";

const START = "2026-09-10T12:00:00.123456789Z";
const DEADLINE = "2026-09-11T12:00:00.123456789Z";
const BEFORE = "2026-09-11T12:00:00.123456788Z";
const core: CanonicalObject = { room_status: "active", memberships: Object.fromEntries(["lead", "mira", "jonah"].map(role => [role, {
  member_id: role, principal_id: `${role}-principal`, principal_kind: role === "lead" ? "human" : "agent",
  access_mode: "participant", role, standing: "enabled",
}])) };

class Session {
  state = pack.initialize({ configuration: { scenario_id: "standard-v1" }, initial_core_state: core }).initial_activity_state as unknown as ArchiveState;
  scheduled: Record<string, CanonicalObject> = {};
  transcript: { input: CanonicalObject; output: ReturnType<typeof reduceArchive> }[] = [];
  input(stimulus: CanonicalObject): CanonicalObject {
    return { prior_activity_state: this.state as unknown as CanonicalObject, core_before: core,
      proposed_core_after: core, recorded_stimulus: stimulus, scheduled_timers: this.scheduled };
  }
  apply(stimulus: CanonicalObject) {
    const input = structuredClone(this.input(stimulus));
    const output = reduceArchive(input);
    assert.equal(output.activity_disposition_type, "apply", JSON.stringify(output));
    if (output.activity_disposition_type !== "apply") throw new Error("expected accepted transition");
    this.transcript.push({ input, output });
    this.state = output.next_activity_state as unknown as ArchiveState;
    for (const raw of output.timer_requests) {
      const request = raw as CanonicalObject;
      const id = request.timer_id as string;
      if (request.timer_request_type === "cancel_current") {
        assert.equal(request.expected_generation, this.scheduled[id]?.generation, "cancel only an outstanding generation");
        delete this.scheduled[id];
      } else {
        assert.equal(this.scheduled[id], undefined);
        this.scheduled[id] = { generation: 1, due: request.due!, canonical_payload: request.canonical_payload! };
      }
    }
    return output;
  }
  start(at = START) { return this.apply({ stimulus_type: "external_input", source_id: ACTIVITY_START_SOURCE_ID,
    input_type: ACTIVITY_START_INPUT_TYPE, canonical_payload: { opened_by: "host" }, immutable_resource_references: [], recorded_at: at }); }
  stimulus(action: string, at = START, payload: CanonicalObject = {}, role = "lead"): CanonicalObject {
    return { stimulus_type: "participant_action", member_id: role, action_type: action, canonical_payload: payload, admitted_at: at };
  }
  act(action: string, at = START, payload: CanonicalObject = {}, role = "lead") { return this.apply(this.stimulus(action, at, payload, role)); }
  timer(id = SESSION_TIMER_ID): CanonicalObject {
    const timer = this.scheduled[id]!;
    return { stimulus_type: "timer_fired", timer_id: id, generation: timer.generation!, scheduled_for: timer.due!, canonical_payload: timer.canonical_payload! };
  }
  fire(id = SESSION_TIMER_ID) { const stimulus = this.timer(id); delete this.scheduled[id]; return this.apply(stimulus); }
  request() {
    this.act("assign_mira_task", START, { task_kind: "investigate_records", power_allowance: 0 });
    this.act("request_mira_plan", "2026-09-11T11:59:50.123456789Z");
  }
  proposal(): CanonicalObject { return { task_revision: 1, opportunity_revision: 1, dialogue: "Recommend inspecting Records.",
    steps: [{ step_type: "move", destination: "records", source_id: "none", power_cost: 0 }] }; }
}

function assertSameResources(before: ArchiveState, after: ArchiveState) {
  for (const key of ["turns_used", "power_remaining", "location", "gates", "evidence", "candidates", "carried_candidate_id",
    "carried_confidence", "verifier_result", "preservation_agreement", "collection_preservation", "source_record_protected",
    "completed_crew_work", "companion_dialogue", "starting_crew"] as const) assert.deepEqual(after[key], before[key], key);
  for (const role of ["mira", "jonah"] as const) {
    for (const key of ["location", "task", "knowledge", "last_contribution", "field_assay", "plan"] as const) {
      assert.deepEqual(after[role][key], before[role][key], `${role}.${key}`);
    }
  }
}

test("Genesis and briefing have no session timer; recorded Activity Start schedules one exact 24-hour deadline", () => {
  const genesis = pack.initialize({ configuration: { scenario_id: "standard-v1" }, initial_core_state: core });
  assert.deepEqual(genesis.timer_requests, []);
  const session = new Session();
  assert.equal(session.state.session_deadline, "none");
  assert.deepEqual(authorizedView(session.state, core, { viewer_type: "participant", member_id: "lead" }).actionOffers, []);
  const output = session.start();
  assert.equal(session.state.session_started_at, START);
  assert.equal(session.state.session_deadline, DEADLINE);
  assert.deepEqual(output.timer_requests, [{ timer_request_type: "schedule_next", timer_id: SESSION_TIMER_ID,
    due: DEADLINE, canonical_payload: { timer: "archive_session", deadline: DEADLINE } }]);
  assert.equal(reduceArchive(session.input({ stimulus_type: "external_input", source_id: ACTIVITY_START_SOURCE_ID,
    input_type: ACTIVITY_START_INPUT_TYPE, canonical_payload: { opened_by: "host" }, immutable_resource_references: [], recorded_at: START })).activity_disposition_type, "reject");
});

test("24-hour addition preserves nanoseconds across calendar boundaries", () => {
  for (const [start, expected] of [[START, DEADLINE], ["2026-12-31T23:59:59.000000001Z", "2027-01-01T23:59:59.000000001Z"],
    ["2028-02-28T00:00:00.999999999Z", "2028-02-29T00:00:00.999999999Z"],
    ["2028-02-29T12:00:00Z", "2028-03-01T12:00:00Z"]]) assert.equal(sessionDeadline(start!), expected);
});

test("ordinary Actions use recorded admission: one nanosecond before is legal, equality and later reject without changing facts", () => {
  const session = new Session(); session.start();
  for (const action of ["stage_wait", "stage_move", "assign_mira_task"]) {
    const payload = action === "stage_move" ? { destination: "records" }
      : action === "assign_mira_task" ? { task_kind: "investigate_records", power_allowance: 0 } : {};
    assert.equal(reduceArchive(session.input(session.stimulus(action, BEFORE, payload))).activity_disposition_type, "apply");
    for (const at of [DEADLINE, "2026-09-11T12:00:00.12345679Z", "2026-09-20T12:00:00Z"]) {
      assert.equal(reduceArchive(session.input(session.stimulus(action, at, payload))).activity_disposition_type, "reject");
    }
  }
  session.act("stage_wait", BEFORE);
  assert.equal(reduceArchive(session.input(session.stimulus("commit_turn", DEADLINE))).activity_disposition_type, "reject");
  assert.equal(session.state.turns_used, 0);
});

test("expiry consumes its Timer, cancels an outstanding response, and preserves every resource and completed fact", () => {
  const session = new Session(); session.start();
  session.act("stage_move", START, { destination: "records" }); session.act("commit_turn");
  session.act("stage_inspect_records"); session.act("commit_turn");
  session.request();
  const before = structuredClone(session.state);
  const response = session.timer(MIRA_PLAN_TIMER_ID);
  const expiry = session.timer();
  const result = session.fire();
  assert.equal(session.state.phase, "expired"); assert.equal(session.state.outcome, null);
  assert.equal(session.state.session_deadline, DEADLINE);
  assertSameResources(before, session.state);
  const debrief = authorizedView(session.state, core, { viewer_type: "participant", member_id: "lead" }).projection.crew_debrief as CanonicalObject;
  assert.deepEqual(debrief.extracted_roles, []);
  assert.deepEqual(debrief.left_behind_roles, []);
  assert.deepEqual(result.timer_requests, [{ timer_request_type: "cancel_current", timer_id: MIRA_PLAN_TIMER_ID, expected_generation: 1 }]);
  assert.deepEqual(session.scheduled, {});
  for (const stale of [response, expiry]) {
    const output = session.apply(stale);
    assert.deepEqual(output.ordered_domain_events, []);
    assert.deepEqual(output.timer_requests, []);
    assert.equal(session.state.phase, "expired");
  }
  assert.equal(reduceArchive(session.input(session.stimulus("submit_companion_plan", BEFORE, session.proposal(), "mira"))).activity_disposition_type, "reject");
});

test("extraction committed before expiry wins; expiry committed first prevents extraction even for an earlier admitted Action", () => {
  for (const expiryFirst of [false, true]) {
    const session = new Session(); session.start();
    session.act("stage_extract"); session.act("prepare_extraction");
    session.act("acknowledge_extraction", START, { preview_revision: session.state.extraction.revision, left_behind_roles: [] });
    const expiry = session.timer();
    const commit = session.stimulus("commit_turn", BEFORE);
    if (expiryFirst) {
      session.fire();
      assert.equal(reduceArchive(session.input(commit)).activity_disposition_type, "reject");
      assert.equal(session.state.turns_used, 0); assert.equal(session.state.outcome, null);
    } else {
      const result = session.apply(commit);
      assert.equal(session.state.phase, "complete"); assert.equal(session.state.outcome?.kind, "no_ledger");
      assert.deepEqual(result.timer_requests, [{ timer_request_type: "cancel_current", timer_id: SESSION_TIMER_ID, expected_generation: 1 }]);
      const completed = structuredClone(session.state);
      assert.deepEqual(session.apply(expiry).ordered_domain_events, []);
      assert.deepEqual(session.state, completed);
    }
  }
});

test("provider plan and dialogue obey both session boundary and recorded commit order", () => {
  for (const expiryFirst of [false, true]) {
    const session = new Session(); session.start(); session.request();
    const proposal = session.proposal();
    for (const at of [DEADLINE, "2026-09-11T12:00:00.12345679Z"]) {
      assert.equal(reduceArchive(session.input(session.stimulus("submit_companion_plan", at, proposal, "mira"))).activity_disposition_type, "reject");
    }
    if (expiryFirst) session.fire();
    const result = reduceArchive(session.input(session.stimulus("submit_companion_plan", BEFORE, proposal, "mira")));
    assert.equal(result.activity_disposition_type, expiryFirst ? "reject" : "apply");
    if (!expiryFirst) {
      session.act("submit_companion_plan", BEFORE, proposal, "mira");
      session.fire();
    }
    assert.equal(session.state.companion_dialogue.length, expiryFirst ? 0 : 1);
    assert.equal(session.state.turns_used, 0);
  }
});

test("turn exhaustion finalizes through the same session timer cancellation", () => {
  const session = new Session(); session.start();
  let last: ReturnType<Session["act"]> | undefined;
  for (let turn = 0; turn < 16; turn++) {
    session.act("stage_wait"); last = session.act("commit_turn");
  }
  assert.equal(session.state.phase, "complete");
  assert.equal(session.state.outcome?.kind, "exhausted_inside");
  assert.deepEqual(last?.timer_requests, [{ timer_request_type: "cancel_current", timer_id: SESSION_TIMER_ID, expected_generation: 1 }]);
  assert.deepEqual(session.scheduled, {});
});

test("stale session generations, deadlines, payloads and duplicate response Timers are inert", () => {
  const session = new Session(); session.start();
  const timer = session.timer(); const before = structuredClone(session.state);
  for (const stale of [{ ...timer, generation: 2 }, { ...timer, scheduled_for: START },
    { ...timer, canonical_payload: { timer: "archive_session", deadline: START } }]) {
    assert.deepEqual(session.apply(stale).ordered_domain_events, []); assert.deepEqual(session.state, before);
  }
  session.request();
  const response = session.timer(MIRA_PLAN_TIMER_ID);
  session.fire(MIRA_PLAN_TIMER_ID);
  const expiredOpportunity = structuredClone(session.state);
  assert.deepEqual(session.apply(response).ordered_domain_events, []);
  assert.deepEqual(session.state, expiredOpportunity);
});

test("response Timer and session Timer races preserve the same terminal resources in either commit order", () => {
  for (const sessionFirst of [false, true]) {
    const session = new Session(); session.start(); session.request();
    const response = session.timer(MIRA_PLAN_TIMER_ID);
    const before = structuredClone(session.state);
    if (sessionFirst) { session.fire(); session.apply(response); }
    else { session.fire(MIRA_PLAN_TIMER_ID); session.fire(); }
    assertSameResources(before, session.state);
    assert.equal(session.state.phase, "expired"); assert.deepEqual(session.scheduled, {});
  }
});

test("restart retains the overdue recorded Timer and Replay consumes only retained stimuli without reading a clock", () => {
  const session = new Session(); session.start(); session.request();
  const restored = new Session();
  restored.state = JSON.parse(JSON.stringify(session.state)) as ArchiveState;
  restored.scheduled = JSON.parse(JSON.stringify(session.scheduled)) as Record<string, CanonicalObject>;
  // A resumed process can be arbitrarily late. Reading does not mutate Activity State.
  assert.equal(authorizedView(restored.state, core, { viewer_type: "participant", member_id: "lead" }).projection.phase, "active");
  assert.equal(reduceArchive(restored.input(restored.stimulus("stage_wait", "2030-01-01T00:00:00Z"))).activity_disposition_type, "reject");
  assert.deepEqual(restored.fire(), session.fire());
  for (const { input, output } of [...session.transcript, ...restored.transcript]) {
    assert.equal(canonicalStringify(reduceArchive(JSON.parse(JSON.stringify(input))) as unknown as CanonicalObject), canonicalStringify(output as unknown as CanonicalObject));
  }
});

test("disconnect and re-entry do not advance turns or reset the fixed session deadline", () => {
  const session = new Session(); session.start();
  const before = structuredClone(session.state);
  for (let index = 0; index < 2; index++) session.apply({ stimulus_type: "core_proposed", recorded_at: BEFORE });
  assert.equal(session.state.turns_used, before.turns_used);
  assert.equal(session.state.session_deadline, before.session_deadline);
  assert.deepEqual(session.state.completed_crew_work, before.completed_crew_work);
});

test("expired projections expose deadline and public lifecycle without revealing hidden truth or dialogue audiences", () => {
  const session = new Session(); session.start(); session.request();
  session.act("submit_companion_plan", BEFORE, session.proposal(), "mira"); session.fire();
  for (const viewerType of ["public", "operator", "final_reveal"]) {
    const view = authorizedView(session.state, core, { viewer_type: viewerType, member_id: "lead" });
    assert.deepEqual(Object.keys(view.projection).sort(), ["lifecycle", "location", "outcome", "phase", "session_deadline", "turns_used"]);
    assert.equal(view.projection.lifecycle, "terminal"); assert.equal(view.projection.outcome, null);
    assert.equal(view.projection.session_deadline, DEADLINE); assert.deepEqual(view.actionOffers, []);
    assert.equal(JSON.stringify(view.projection).includes("Recommend"), false);
  }
  for (const role of ["lead", "mira", "jonah"]) {
    const view = authorizedView(session.state, core, { viewer_type: "participant", member_id: role });
    assert.equal(view.projection.phase, "expired"); assert.notEqual(view.projection.debrief, null);
    assert.equal((view.projection.mira as CanonicalObject).dialogue_allowed, false);
    assert.deepEqual(view.actionOffers, []);
    assert.equal(JSON.stringify(view.projection).includes("truth_marker"), false);
    assert.equal(JSON.stringify(view.projection).includes("speaker_member_id"), false);
    assert.equal((view.projection.companion_dialogue as unknown[]).length, role === "jonah" ? 0 : 1);
  }
});
