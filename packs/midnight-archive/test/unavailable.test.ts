import assert from "node:assert/strict";
import test from "node:test";
import { canonicalStringify, type CanonicalObject, type PackReduceOutput } from "@worldstream/pack-sdk";
import pack, { reduceArchive } from "../src/pack.js";
import { startArchive } from "../src/rules.js";
import type { ArchiveState } from "../src/model.js";
import { authorizedView } from "../src/view.js";
import { JONAH_PLAN_TIMER_ID, MIRA_PLAN_TIMER_ID } from "../src/companions.js";

const roles = ["mira", "jonah"] as const;
type Companion = typeof roles[number];
const timerId = (role: Companion) => role === "mira" ? MIRA_PLAN_TIMER_ID : JONAH_PLAN_TIMER_ID;
const instant = (second: number) => `2026-09-10T12:00:${String(second).padStart(2, "0")}Z`;
const step = (kind: string, destination = "none", source = "none", power = 0): CanonicalObject =>
  ({ step_type: kind, destination, source_id: source, power_cost: power });

class Expedition {
  core: CanonicalObject = { room_status: "active", memberships: Object.fromEntries(["lead", ...roles].map((role) => [role, {
    member_id: role, principal_id: `${role}-principal`, principal_kind: role === "lead" ? "human" : "agent",
    access_mode: "participant", role, standing: "enabled",
  }])) };
  state = startArchive("2026-09-10T12:00:00Z", pack.initialize({ configuration: { scenario_id: "standard-v1" }, initial_core_state: this.core }).initial_activity_state as unknown as ArchiveState);
  scheduled: Record<string, CanonicalObject> = {};
  generations = new Map<string, number>();
  transcript: CanonicalObject[] = [];

  input(stimulus: CanonicalObject, proposedCore = this.core): CanonicalObject {
    return { prior_activity_state: this.state as unknown as CanonicalObject, core_before: this.core,
      proposed_core_after: proposedCore, recorded_stimulus: stimulus, scheduled_timers: this.scheduled };
  }

  apply(stimulus: CanonicalObject, proposedCore = this.core) {
    const input = structuredClone(this.input(stimulus, proposedCore));
    const result = reduceArchive(input);
    assert.equal(result.activity_disposition_type, "apply", JSON.stringify(result));
    if (result.activity_disposition_type !== "apply") throw new Error("expected accepted transition");
    this.transcript.push(input);
    this.state = result.next_activity_state as unknown as ArchiveState;
    this.core = proposedCore;
    for (const raw of result.timer_requests) {
      const request = raw as CanonicalObject;
      const id = request.timer_id as string;
      if (request.timer_request_type === "cancel_current") {
        assert.equal(request.expected_generation, (this.scheduled[id] as CanonicalObject).generation);
        delete this.scheduled[id];
      } else {
        assert.equal(request.timer_request_type, "schedule_next");
        const generation = (this.generations.get(id) ?? 0) + 1;
        this.generations.set(id, generation);
        this.scheduled[id] = { generation, due: request.due!, canonical_payload: request.canonical_payload! };
      }
    }
    return result;
  }

  act(role: string, action: string, payload: CanonicalObject = {}, second = 0) {
    return this.apply({ stimulus_type: "participant_action", member_id: role, action_type: action,
      canonical_payload: payload, admitted_at: instant(second) });
  }

  reject(role: string, action: string, payload: CanonicalObject, code: string, second = 0) {
    const before = canonicalStringify(this.state as unknown as CanonicalObject);
    const result = reduceArchive(this.input({ stimulus_type: "participant_action", member_id: role,
      action_type: action, canonical_payload: payload, admitted_at: instant(second) }));
    assert.equal(result.activity_disposition_type, "reject");
    assert.equal(result.declared_code, code);
    assert.equal(canonicalStringify(this.state as unknown as CanonicalObject), before);
  }

  assign(role: Companion, allowance = 0) {
    return this.act("lead", `assign_${role}_task`, { task_kind: "investigate_records", power_allowance: allowance });
  }

  request(role: Companion, second = 0) { return this.act("lead", `request_${role}_plan`, {}, second); }
  proposal(role: Companion, steps = [step("move", "records")]): CanonicalObject {
    return { task_revision: this.state[role].task.revision, opportunity_revision: this.state[role].opportunity.revision, dialogue: "", steps };
  }
  timer(role: Companion): CanonicalObject {
    const timer = this.scheduled[timerId(role)] as CanonicalObject;
    return { stimulus_type: "timer_fired", timer_id: timerId(role), generation: timer.generation!,
      canonical_payload: timer.canonical_payload!, admitted_at: timer.due! };
  }
  expire(role: Companion) {
    const stimulus = this.timer(role);
    delete this.scheduled[timerId(role)];
    return this.apply(stimulus);
  }
  turn(action = "stage_wait", payload: CanonicalObject = {}, prepared: Companion[] = []) {
    this.act("lead", action, payload);
    for (const role of prepared) this.act("lead", `prepare_${role}_contribution`);
    return this.act("lead", "commit_turn");
  }
  projection(role = "lead") {
    return authorizedView(this.state, this.core, { viewer_type: "participant", member_id: role });
  }
}

function noImplicitWork(result: PackReduceOutput, expedition: Expedition, before: ArchiveState) {
  assert.deepEqual(result.ordered_attention_signals, []);
  assert.deepEqual(result.timer_requests, []);
  assert.equal(expedition.state.turns_used, before.turns_used);
  assert.equal(expedition.state.power_remaining, before.power_remaining);
  assert.deepEqual(expedition.state.staged_action, before.staged_action);
  assert.deepEqual(expedition.state.completed_crew_work, before.completed_crew_work);
  for (const role of roles) assert.equal(expedition.state[role].location, before[role].location);
}

test("planning deadlines are canonical and preserve the full fifteen seconds through nanosecond boundaries", () => {
  for (const [opened, deadline] of [
    ["2026-09-10T12:00:00Z", "2026-09-10T12:00:15Z"],
    ["2026-09-10T12:00:00.1Z", "2026-09-10T12:00:15.1Z"],
    ["2026-09-10T12:00:00.12Z", "2026-09-10T12:00:15.12Z"],
    ["2026-09-10T12:00:00.123456789Z", "2026-09-10T12:00:15.123456789Z"],
    ["2026-12-31T23:59:59.000000001Z", "2027-01-01T00:00:14.000000001Z"],
    ["2028-02-29T23:59:59.999999999Z", "2028-03-01T00:00:14.999999999Z"],
  ]) {
    const expedition = new Expedition();
    expedition.state = startArchive(opened!, { ...expedition.state, phase: "briefing" });
    expedition.assign("mira");
    const result = expedition.apply({ stimulus_type: "participant_action", member_id: "lead",
      action_type: "request_mira_plan", canonical_payload: {}, admitted_at: opened! });
    assert.equal(expedition.state.mira.opportunity.deadline, deadline);
    assert.equal((result.timer_requests[0] as CanonicalObject).due, deadline);
    assert.equal((result.ordered_attention_signals[0] as CanonicalObject).deadline, deadline);
  }
  const expedition = new Expedition();
  expedition.assign("mira");
  expedition.apply({ stimulus_type: "participant_action", member_id: "lead", action_type: "request_mira_plan",
    canonical_payload: {}, admitted_at: "2026-09-10T12:00:00.123456789Z" });
  const proposal = expedition.proposal("mira");
  for (const [admitted_at, expected] of [
    ["2026-09-10T12:00:15.123456788Z", "apply"],
    ["2026-09-10T12:00:15.123456789Z", "reject"],
    ["2026-09-10T12:00:15.12345679Z", "reject"],
  ]) {
    const result = reduceArchive(expedition.input({ stimulus_type: "participant_action", member_id: "mira",
      action_type: "submit_companion_plan", canonical_payload: proposal, admitted_at: admitted_at! }));
    assert.equal(result.activity_disposition_type, expected);
  }
});

test("each serialized opportunity lasts exactly fifteen seconds and missing replies do no work or retry", () => {
  const expedition = new Expedition();
  for (const role of roles) expedition.assign(role);
  expedition.act("lead", "stage_move", { destination: "records" });
  for (const [index, role] of roles.entries()) {
    const opened = expedition.request(role, index * 15);
    assert.equal(opened.timer_requests.length, 1);
    assert.equal(opened.ordered_attention_signals.length, 1);
    assert.equal((opened.ordered_attention_signals[0] as CanonicalObject).target_member_id, role);
    assert.equal(expedition.state[role].opportunity.deadline, instant((index + 1) * 15));
    const other = role === "mira" ? "jonah" : "mira";
    expedition.reject("lead", `request_${other}_plan`, {}, "task_violation");
    const proposal = expedition.proposal(role);
    expedition.reject(role, "submit_companion_plan", proposal, "stale_plan", (index + 1) * 15);
    const before = structuredClone(expedition.state);
    noImplicitWork(expedition.expire(role), expedition, before);
    assert.equal(expedition.state[role].opportunity.status, "expired");
    expedition.reject(role, "submit_companion_plan", proposal, "stale_plan", (index + 1) * 15 + 1);
  }
  assert.deepEqual(expedition.scheduled, {});
  const continued = expedition.act("lead", "commit_turn");
  assert.equal(expedition.state.turns_used, 1);
  assert.deepEqual(continued.ordered_attention_signals, []);
  assert.deepEqual(continued.timer_requests, []);
  assert.equal(expedition.state.mira.location, "atrium");
  assert.equal(expedition.state.jonah.location, "atrium");
});

test("accepted personal and sibling edits cancel the old opportunity before another can open", () => {
  for (const role of roles) {
    const other = role === "mira" ? "jonah" : "mira";
    for (const [action, payload] of [
      ["stage_wait", {}], ["commit_turn", {}], [`defer_${role}_contribution`, {}],
      [`set_${other}_hold`, {}], [`assign_${other}_task`, { task_kind: "investigate_records", power_allowance: 0 }],
    ] as [string, CanonicalObject][]) {
      const expedition = new Expedition();
      expedition.assign(role);
      expedition.act("lead", "stage_wait");
      expedition.request(role);
      const proposal = expedition.proposal(role);
      const changed = expedition.act("lead", action, payload, 1);
      assert.notEqual(expedition.state[role].opportunity.status, "open", `${role}: ${action}`);
      assert.deepEqual(changed.timer_requests, [{ timer_request_type: "cancel_current", timer_id: timerId(role), expected_generation: 1 }]);
      assert.deepEqual(changed.ordered_attention_signals, []);
      expedition.reject(role, "submit_companion_plan", proposal, "stale_plan", 2);
      expedition.assign(other);
      expedition.request(other, 2);
      assert.equal(expedition.state[other].opportunity.status, "open");
    }
  }
});

test("Core eligibility edits cancel every superseded opportunity without replacing starting crew", () => {
  for (const role of roles) {
    const expedition = new Expedition();
    expedition.assign(role);
    expedition.request(role);
    const other = role === "mira" ? "jonah" : "mira";
    const memberships = expedition.core.memberships as CanonicalObject;
    const nextCore = { ...expedition.core, memberships: { ...memberships, [other]: { ...(memberships[other] as CanonicalObject), standing: "suspended" } } };
    const result = expedition.apply({ stimulus_type: "core_proposed" }, nextCore);
    assert.notEqual(expedition.state[role].opportunity.status, "open");
    assert.deepEqual(result.ordered_attention_signals, []);
    assert.deepEqual(expedition.state.starting_crew.map((entry) => entry.role), ["lead", "mira", "jonah"]);
    assert.equal(expedition.state.turns_used, 0);
  }
});

test("extraction preview binds the cancelled-window state and a rejected edit leaves the window usable", () => {
  const expedition = new Expedition();
  expedition.assign("mira");
  expedition.act("lead", "stage_extract");
  expedition.request("mira");
  const before = structuredClone(expedition.scheduled);
  expedition.reject("lead", "stage_move", { destination: "vault" }, "illegal_action");
  assert.equal(expedition.state.mira.opportunity.status, "open");
  assert.deepEqual(expedition.scheduled, before);
  expedition.act("lead", "prepare_extraction");
  assert.equal(expedition.state.mira.opportunity.status, "none");
  expedition.act("lead", "acknowledge_extraction", { preview_revision: expedition.state.extraction.revision, left_behind_roles: [] });
  expedition.act("lead", "commit_turn");
  assert.equal(expedition.state.outcome?.kind, "no_ledger");
});

test("cancelled or expired companion replies and obsolete timers cannot consume the other window", () => {
  for (const role of roles) for (const ending of ["cancel", "expire"]) {
    const expedition = new Expedition();
    const other = role === "mira" ? "jonah" : "mira";
    expedition.assign(role);
    expedition.assign(other);
    expedition.request(role);
    const staleTimer = expedition.timer(role);
    const staleProposal = expedition.proposal(role);
    if (ending === "expire") expedition.expire(role);
    else expedition.act("lead", `cancel_${role}_task`);
    expedition.request(other, 16);
    const otherBefore = structuredClone(expedition.state[other]);
    expedition.reject(role, "submit_companion_plan", staleProposal, "stale_plan", 17);
    const result = expedition.apply(staleTimer);
    assert.deepEqual(expedition.state[other], otherBefore);
    assert.deepEqual(result.ordered_attention_signals, []);
    assert.deepEqual(result.timer_requests, []);
    expedition.act(other, "submit_companion_plan", expedition.proposal(other), 17);
    assert.equal(expedition.state[other].plan.origin_member_id, other);
    assert.equal(expedition.state.turns_used, 0);
  }
});

test("malformed, wrong-revision and ineligible plans reject while current valid work remains available", () => {
  for (const role of roles) {
    const expedition = new Expedition();
    expedition.assign(role);
    expedition.request(role);
    const valid = expedition.proposal(role);
    for (const [proposal, code] of [
      [{ ...valid, steps: [] }, "plan_invalid"],
      [{ ...valid, steps: [step("move", "vault")] }, "plan_invalid"],
      [{ ...valid, task_revision: 2 }, "stale_plan"],
      [{ ...valid, opportunity_revision: 2 }, "stale_plan"],
      [{ ...valid, provider_healthy: true }, "invalid_payload"],
    ] as [CanonicalObject, string][]) expedition.reject(role, "submit_companion_plan", proposal, code, 1);
    expedition.act(role, "submit_companion_plan", valid, 1);
    assert.equal(expedition.state[role].plan.status, "active");
    assert.equal(expedition.state[role].plan.next_step_index, 0);
    assert.deepEqual(expedition.scheduled, {});
  }
});

test("standing plans survive ordinary turns and explicit defer without implicit planning or execution", () => {
  const expedition = new Expedition();
  for (const role of roles) {
    expedition.assign(role);
    expedition.request(role);
    expedition.act(role, "submit_companion_plan", expedition.proposal(role, [step("move", "records"), step("inspect_source", "none", "records"), step("share_source", "none", "records")]));
  }
  expedition.turn("stage_wait", {}, ["mira", "jonah"]);
  const plans = roles.map((role) => structuredClone(expedition.state[role].plan));
  expedition.act("lead", "stage_wait");
  for (const role of roles) expedition.act("lead", `defer_${role}_contribution`);
  const skipped = expedition.act("lead", "commit_turn");
  assert.deepEqual(skipped.ordered_attention_signals, []);
  for (const [index, role] of roles.entries()) assert.deepEqual(expedition.state[role].plan, plans[index]);
  expedition.turn("stage_wait", {}, ["mira", "jonah"]);
  expedition.turn("stage_wait", {}, ["mira", "jonah"]);
  for (const role of roles) assert.equal(expedition.state[role].plan.next_step_index, 3);
  assert.equal(expedition.state.turns_used, 4);
  assert.equal(expedition.state.power_remaining, 3);
  assert.equal(expedition.state.completed_crew_work.length, 6);
});

test("preparing a standing step cancels a sibling window without replacing the accepted plan", () => {
  for (const role of roles) {
    const other = role === "mira" ? "jonah" : "mira";
    const expedition = new Expedition();
    expedition.assign(role);
    expedition.request(role);
    expedition.act(role, "submit_companion_plan", expedition.proposal(role), 14);
    const accepted = structuredClone(expedition.state[role].plan);
    expedition.assign(other);
    expedition.request(other);
    const proposal = expedition.proposal(other);
    expedition.act("lead", `prepare_${role}_contribution`);
    assert.equal(expedition.state[other].opportunity.status, "none");
    assert.deepEqual(expedition.state[role].plan, accepted);
    assert.equal(expedition.state[role].preparation.status, "prepared");
    expedition.reject(other, "submit_companion_plan", proposal, "stale_plan", 1);
    assert.deepEqual(expedition.scheduled, {});
  }
});

test("losing and restoring Membership eligibility cannot resurrect a plan or impersonate a replacement", () => {
  for (const role of roles) for (const standing of ["suspended", "departed"]) {
    const expedition = new Expedition();
    expedition.assign(role);
    expedition.request(role);
    const proposal = expedition.proposal(role);
    expedition.act(role, "submit_companion_plan", proposal);
    const enabledCore = expedition.core;
    const memberships = enabledCore.memberships as CanonicalObject;
    expedition.apply({ stimulus_type: "core_proposed" }, { ...enabledCore, memberships: {
      ...memberships, [role]: { ...(memberships[role] as CanonicalObject), standing },
    } });
    assert.equal(expedition.state[role].plan.status, "none");
    assert.equal(expedition.state[role].presence, "suspended");
    expedition.reject(role, "submit_companion_plan", proposal, "role_violation");
    expedition.apply({ stimulus_type: "core_proposed" }, enabledCore);
    assert.equal(expedition.state[role].mode, "holding");
    assert.equal(expedition.state[role].member_id, role);
    expedition.reject(role, "submit_companion_plan", proposal, "stale_plan");
    expedition.turn();
    assert.equal(expedition.state[role].location, "atrium");
    assert.ok(expedition.state.completed_crew_work.every((work) => work.role !== role));
    assert.equal(expedition.state.turns_used, 1);
  }
});

test("zero allowance and missing replies leave disclosed follow/regroup and partial extraction playable", () => {
  const expedition = new Expedition();
  expedition.turn("stage_move", { destination: "records" });
  for (const role of roles) {
    expedition.assign(role, 0);
    expedition.request(role);
    expedition.reject(role, "submit_companion_plan", expedition.proposal(role, [step("use_verifier", "none", "records", 1)]), "task_violation", 1);
    expedition.expire(role);
    const offers = expedition.projection().actionOffers.map((offer) => typeof offer === "string" ? offer : offer.actionType);
    for (const action of [`defer_${role}_contribution`, `set_${role}_follow`, `set_${role}_regroup`, "stage_wait"]) assert.ok(offers.includes(action));
  }
  expedition.act("lead", "set_jonah_regroup");
  for (const [action, payload] of [
    ["stage_use_verifier", {}], ["stage_move", { destination: "plant" }], ["stage_open_service_hatch", {}],
    ["stage_move", { destination: "vault" }], ["stage_recover_candidate", { candidate_id: "ledger-violet" }],
    ["stage_move", { destination: "plant" }], ["stage_move", { destination: "records" }], ["stage_move", { destination: "atrium" }],
  ] as [string, CanonicalObject][]) expedition.turn(action, payload);
  expedition.act("lead", "stage_extract");
  expedition.act("lead", "prepare_extraction");
  assert.deepEqual(expedition.state.extraction.left_behind_roles, ["mira"]);
  expedition.act("lead", "acknowledge_extraction", { preview_revision: expedition.state.extraction.revision, left_behind_roles: ["mira"] });
  expedition.act("lead", "commit_turn");
  assert.equal(expedition.state.outcome?.kind, "partial_extraction");
  assert.deepEqual(expedition.state.extraction.extracted_roles, ["lead", "jonah"]);
  assert.equal(expedition.state.power_remaining, 0);
  assert.ok(expedition.state.completed_crew_work.every((work) => work.kind === "follow_move" || work.kind === "regroup_move"));
  const projected = canonicalStringify(expedition.projection().projection);
  assert.equal(projected.includes("truth_marker"), false);
  assert.equal(projected.includes("origin_member_id"), false);
  // Reconstructing authorized state needs no remembered provider reply or Session.
  for (const input of expedition.transcript) {
    const first = reduceArchive(input);
    const second = reduceArchive(structuredClone(input));
    assert.deepEqual(second, first);
  }
  assert.equal(canonicalStringify(expedition.projection().projection), projected);
});
