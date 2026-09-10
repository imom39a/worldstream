import assert from "node:assert/strict";
import test from "node:test";
import type { CanonicalObject } from "@worldstream/pack-sdk";
import pack, { reduceArchive } from "../src/pack.js";
import type { ArchiveState } from "../src/model.js";
import { startArchive } from "../src/rules.js";
import { authorizedView, participantProjection } from "../src/view.js";
import { canonicalStringify } from "@worldstream/pack-sdk";
import { prepareTurn } from "../src/turn-resolution.js";
import { bothObjectivesRouteActions } from "../fixtures/golden.js";
import { JONAH_PLAN_TIMER_ID, MIRA_PLAN_TIMER_ID } from "../src/companions.js";

function core(roles = ["lead", "mira", "jonah"]): CanonicalObject {
  return { room_status: "active", memberships: Object.fromEntries(roles.map((role) => [role, {
    member_id: role, principal_id: `${role}-principal`, principal_kind: role === "lead" ? "human" : "agent",
    access_mode: "participant", role, standing: "enabled",
  }])) };
}

function fresh(roles?: string[], scenarioId: "standard-v1" | "low-reserve-v1" = "standard-v1"): ArchiveState {
  return startArchive(pack.initialize({ configuration: { scenario_id: scenarioId }, initial_core_state: core(roles) }).initial_activity_state as unknown as ArchiveState);
}

function action(state: ArchiveState, member: string, actionType: string, payload: CanonicalObject = {}, currentCore = core()): ArchiveState {
  const result = reduceArchive({
    prior_activity_state: state as unknown as CanonicalObject, core_before: currentCore, proposed_core_after: currentCore,
    recorded_stimulus: { stimulus_type: "participant_action", member_id: member, action_type: actionType,
      canonical_payload: payload, admitted_at: "2026-09-10T12:00:01.000Z" }, scheduled_timers: {},
  });
  assert.equal(result.activity_disposition_type, "apply", JSON.stringify(result));
  if (result.activity_disposition_type !== "apply") throw new Error("rejected");
  return result.next_activity_state as unknown as ArchiveState;
}

test("Jonah accepts an authenticated bounded service task with one power allowance", () => {
  const assigned = action(fresh(), "lead", "assign_jonah_task", { task_kind: "open_service_hatch", power_allowance: 1 });
  assert.equal((assigned as unknown as { jonah: { task: { kind: string } } }).jonah.task.kind, "open_service_hatch");
});

test("every supported starting roster is retained at Genesis with the same budget", () => {
  for (const roles of [["lead"], ["lead", "mira"], ["lead", "jonah"], ["lead", "mira", "jonah"]]) {
    const state = fresh(roles);
    assert.equal(state.power_remaining, 3);
    assert.equal(state.turn_limit, 16);
    assert.deepEqual((state as unknown as { starting_crew: { role: string }[] }).starting_crew.map((member) => member.role), roles);
  }
});

test("each authored scenario starts every supported roster with its own fixed budget", () => {
  for (const [scenarioId, expectedPower] of [["standard-v1", 3], ["low-reserve-v1", 2]] as const) {
    for (const roles of [["lead"], ["lead", "mira"], ["lead", "jonah"], ["lead", "mira", "jonah"]]) {
      const state = fresh(roles, scenarioId);
      assert.equal(state.power_remaining, expectedPower);
      assert.equal(state.initial_power, expectedPower);
      assert.deepEqual(participantProjection(state, "lead", core(roles)).scenario, {
        id: scenarioId,
        label: scenarioId === "standard-v1" ? "Standard" : "Low Reserve",
      });
    }
  }
});

test("every supported roster completes the same fifteen-turn route with its entire starting crew", () => {
  for (const roles of [["lead"], ["lead", "mira"], ["lead", "jonah"], ["lead", "mira", "jonah"]]) {
    let state = fresh(roles);
    const currentCore = core(roles);
    for (const item of bothObjectivesRouteActions) state = action(state, "lead", item.action_type, item.canonical_payload, currentCore);
    assert.equal(state.outcome.kind, "success");
    assert.equal(state.turns_used, 15);
    assert.equal(state.power_remaining, 0);
    assert.deepEqual(state.extraction.extracted_roles, roles);
    assert.deepEqual(state.extraction.left_behind_roles, []);
  }
});

function plan(state: ArchiveState, role: "mira" | "jonah", task: string, steps: CanonicalObject[], allowance = 0): ArchiveState {
  state = action(state, "lead", `assign_${role}_task`, { task_kind: task, power_allowance: allowance });
  state = action(state, "lead", `request_${role}_plan`);
  return action(state, role, "submit_companion_plan", { task_revision: state[role].task.revision,
    opportunity_revision: state[role].opportunity.revision, dialogue: "", steps });
}

function step(kind: string, destination = "none", source = "none", power = 0): CanonicalObject {
  return { step_type: kind, destination, source_id: source, power_cost: power };
}

function rejectAction(state: ArchiveState, member: string, type: string, expected: string, payload: CanonicalObject = {}, currentCore = core()) {
  const before = canonicalStringify(state as unknown as CanonicalObject);
  const result = reduceArchive({ prior_activity_state: state as unknown as CanonicalObject,
    core_before: currentCore, proposed_core_after: currentCore, scheduled_timers: {},
    recorded_stimulus: { stimulus_type: "participant_action", member_id: member, action_type: type,
      canonical_payload: payload, admitted_at: "2026-09-10T12:00:01.000Z" } });
  assert.equal(result.activity_disposition_type, "reject");
  assert.equal(result.declared_code, expected);
  assert.equal(canonicalStringify(state as unknown as CanonicalObject), before);
}

function wait(state: ArchiveState, prepare: ("mira" | "jonah")[] = []): ArchiveState {
  state = action(state, "lead", "stage_wait");
  for (const role of prepare) state = action(state, "lead", `prepare_${role}_contribution`);
  return action(state, "lead", "commit_turn");
}

test("Jonah opens the hatch for one charge; Mira's ordinary method costs two", () => {
  for (const role of ["mira", "jonah"] as const) {
    const charge = role === "jonah" ? 1 : 2;
    let state = fresh();
    state = { ...state, [role]: { ...state[role], location: "plant", mode: "holding" } };
    state = plan(state, role, "open_service_hatch", [step("open_service_hatch", "none", "none", charge)], charge);
    const before = state;
    state = wait(state, [role]);
    assert.equal(state.gates.plant_vault_open, true);
    assert.equal(state.power_remaining, before.power_remaining - charge);
    assert.equal(state[role].task.status, "complete");
    assert.deepEqual(state.completed_crew_work.filter((work) => work.role === role), [{ role, kind: "open_service_hatch", turn: 1 }]);
  }
});

test("Jonah's verifier plan consumes the declared role-neutral companion cost", () => {
  let state = fresh();
  state = { ...state, jonah: { ...state.jonah, location: "records", mode: "holding" } };
  state = plan(state, "jonah", "investigate_records", [step("use_verifier", "none", "records", 1)], 1);
  const beforePower = state.power_remaining;

  state = wait(state, ["jonah"]);

  assert.deepEqual(state.operation_costs.companion_verifier, { turn_cost: 0, power_cost: 1 });
  assert.equal(state.power_remaining, beforePower - state.operation_costs.companion_verifier.power_cost);
  assert.equal(state.jonah.task.power_spent, state.operation_costs.companion_verifier.power_cost);
  assert.equal(state.jonah.task.status, "complete");
  assert.equal(state.verifier_result, "ledger-violet");
});

test("Mira's two Vault assay steps disclose no finding before eligible completion and replan after sample", () => {
  let state = fresh();
  state = { ...state, mira: { ...state.mira, location: "vault", mode: "holding" } };
  state = plan(state, "mira", "field_assay", [step("collect_assay_sample")]);
  assert.equal(state.mira.field_assay.result, "none");
  state = wait(state, ["mira"]);
  assert.equal(state.mira.field_assay.steps_completed, 1);
  assert.equal(state.mira.field_assay.result, "none");
  assert.equal(state.evidence.records, "unknown");
  assert.equal(state.evidence.conservation, "unknown");
  const firstProjection = participantProjection(state, "lead", core());
  assert.deepEqual((firstProjection.mira as CanonicalObject).field_assay, { steps_completed: 1, result: null });
  assert.equal(firstProjection.verifier_result, null);
  assert.ok((firstProjection.candidates as CanonicalObject[]).every((candidate) => (candidate.observed_evidence as unknown[]).length === 0));
  state = action(state, "lead", "request_mira_plan");
  state = action(state, "mira", "submit_companion_plan", { task_revision: state.mira.task.revision,
    opportunity_revision: state.mira.opportunity.revision, dialogue: "", steps: [step("complete_field_assay")] });
  state = wait(state, ["mira"]);
  assert.equal(state.mira.field_assay.steps_completed, 2);
  assert.equal(state.mira.field_assay.result, "ledger-violet");
  assert.deepEqual(state.evidence, { records: "observed", conservation: "observed" });
  assert.equal(state.power_remaining, 3);
  assert.equal(state.mira.task.status, "complete");
  const finalProjection = participantProjection(state, "lead", core());
  assert.deepEqual((finalProjection.mira as CanonicalObject).field_assay, { steps_completed: 2, result: { candidate_id: "ledger-violet", confidence: "verified" } });
  assert.ok((finalProjection.candidates as CanonicalObject[]).every((candidate) => (candidate.observed_evidence as unknown[]).length === 2));
});

test("Mira's pending assay sample resets when the next turn or task interrupts it", () => {
  let state = fresh();
  state = { ...state, mira: { ...state.mira, location: "vault", mode: "holding" } };
  state = plan(state, "mira", "field_assay", [step("collect_assay_sample")]);
  state = wait(state, ["mira"]);
  assert.equal(state.mira.field_assay.steps_completed, 1);

  state = wait(state);
  assert.deepEqual(state.mira.field_assay, { steps_completed: 0, result: "none" });
  state = action(state, "lead", "request_mira_plan");
  rejectAction(state, "mira", "submit_companion_plan", "plan_invalid", {
    task_revision: state.mira.task.revision,
    opportunity_revision: state.mira.opportunity.revision, dialogue: "",
    steps: [step("complete_field_assay")],
  });

  state = action(state, "lead", "cancel_mira_task");
  state = { ...state, mira: { ...state.mira, location: "vault", mode: "holding" } };
  state = plan(state, "mira", "field_assay", [step("collect_assay_sample")]);
  state = wait(state, ["mira"]);
  assert.equal(state.mira.field_assay.steps_completed, 1);
  state = action(state, "lead", "set_mira_hold");
  assert.deepEqual(state.mira.field_assay, { steps_completed: 0, result: "none" });
});

test("a hatch opening this turn cannot authorize another participant's traversal", () => {
  let state = fresh();
  state = { ...state, location: "plant", jonah: { ...state.jonah, location: "plant", mode: "holding" }, mira: { ...state.mira, mode: "holding" } };
  state = plan(state, "jonah", "open_service_hatch", [step("open_service_hatch", "none", "none", 1)], 1);
  state = action(state, "lead", "prepare_jonah_contribution");
  rejectAction(state, "lead", "stage_move", "gate_closed", { destination: "vault" });
  state = wait(state, ["jonah"]);
  state = action(state, "lead", "stage_move", { destination: "vault" });
  state = action(state, "lead", "commit_turn");
  assert.equal(state.location, "vault");
});

test("duplicate service work is visible before commit and explicit deferral resolves it", () => {
  let state = fresh();
  state = { ...state, location: "plant", mira: { ...state.mira, mode: "holding" }, jonah: { ...state.jonah, location: "plant", mode: "holding" } };
  state = plan(state, "jonah", "open_service_hatch", [step("open_service_hatch", "none", "none", 1)], 1);
  state = action(state, "lead", "stage_open_service_hatch");
  state = action(state, "lead", "prepare_jonah_contribution");
  assert.deepEqual(prepareTurn(state, core()).view.conflicts, [{ code: "service_hatch", roles: ["lead", "jonah"] }]);
  rejectAction(state, "lead", "commit_turn", "turn_conflict");
  state = action(state, "lead", "defer_jonah_contribution");
  state = action(state, "lead", "commit_turn");
  assert.equal(state.power_remaining, 1);
  assert.equal(state.jonah.plan.next_step_index, 0);
  assert.equal(state.completed_crew_work.length, 0);
  rejectAction(state, "lead", "prepare_jonah_contribution", "task_violation");
});

test("aggregate reservations use beginning power without choosing a silent winner", () => {
  let state = fresh();
  state = { ...state, location: "conservation", collection_preservation: "prepared", power_remaining: 2,
    mira: { ...state.mira, location: "records", mode: "holding" }, jonah: { ...state.jonah, location: "plant", mode: "holding" } };
  state = plan(state, "mira", "investigate_records", [step("use_verifier", "none", "records", 1)], 1);
  state = plan(state, "jonah", "open_service_hatch", [step("open_service_hatch", "none", "none", 1)], 1);
  state = action(state, "lead", "stage_energize_preservation_equipment");
  state = action(state, "lead", "prepare_mira_contribution");
  state = action(state, "lead", "prepare_jonah_contribution");
  const view = prepareTurn(state, core()).view;
  assert.equal(view.power_reserved, 3);
  assert.deepEqual(view.conflicts, [{ code: "shared_power", roles: ["lead", "mira", "jonah"] }]);
  rejectAction(state, "lead", "commit_turn", "turn_conflict");
  state = action(state, "lead", "defer_mira_contribution");
  state = action(state, "lead", "commit_turn");
  assert.equal(state.power_remaining, 0);
  assert.equal(state.mira.plan.next_step_index, 0);
  assert.equal(state.jonah.plan.next_step_index, 1);
});

test("only one Room-wide planning opportunity is open and unprepared tasks do not block", () => {
  let state = fresh();
  for (const role of ["mira", "jonah"] as const) state = action(state, "lead", `assign_${role}_task`, { task_kind: "investigate_records", power_allowance: 0 });
  state = action(state, "lead", "request_mira_plan");
  rejectAction(state, "lead", "request_jonah_plan", "task_violation");
  assert.equal(prepareTurn(state, core()).view.status, "clear");
  assert.deepEqual(prepareTurn(state, core()).view.unprepared_roles, ["mira", "jonah"]);
  state = wait(state);
  assert.equal(state.turns_used, 1);
  assert.equal(state.completed_crew_work.length, 0);
});

test("extraction includes same-turn returns, requires exact acknowledgement, and attributes actual work", () => {
  let state = fresh();
  state = { ...state, carried_candidate_id: "ledger-violet", mira: { ...state.mira, location: "records", mode: "holding" }, jonah: { ...state.jonah, location: "plant", mode: "holding" } };
  state = action(state, "lead", "set_mira_regroup");
  state = action(state, "lead", "set_jonah_regroup");
  state = action(state, "lead", "stage_extract");
  rejectAction(state, "lead", "commit_turn", "extraction_preview_required");
  state = action(state, "lead", "prepare_extraction");
  assert.deepEqual(state.extraction.extracted_roles, ["lead", "mira"]);
  assert.deepEqual(state.extraction.left_behind_roles, ["jonah"]);
  rejectAction(state, "lead", "acknowledge_extraction", "extraction_preview_stale", { preview_revision: state.extraction.revision, left_behind_roles: [] });
  state = action(state, "lead", "acknowledge_extraction", { preview_revision: state.extraction.revision, left_behind_roles: ["jonah"] });
  const acknowledged = state;
  state = action(state, "lead", "set_jonah_hold");
  assert.equal(state.extraction.status, "none");
  rejectAction(state, "lead", "commit_turn", "extraction_preview_required");
  const final = action(acknowledged, "lead", "commit_turn");
  assert.equal(final.outcome.kind, "partial_extraction");
  assert.equal(final.mira.location, "atrium");
  assert.equal(final.jonah.location, "records");
  assert.deepEqual(final.completed_crew_work.map((work) => work.role), ["mira", "jonah"]);
  const debrief = participantProjection(final, "lead", core()).crew_debrief as CanonicalObject;
  assert.deepEqual(debrief.left_behind_roles, ["jonah"]);
});

test("suspension invalidates accepted extraction and never removes a starting crew member", () => {
  let state = action(fresh(), "lead", "stage_extract");
  state = action(state, "lead", "prepare_extraction");
  state = action(state, "lead", "acknowledge_extraction", { preview_revision: state.extraction.revision, left_behind_roles: [] });
  const changed = core();
  ((changed.memberships as CanonicalObject).mira as { standing: string }).standing = "suspended";
  const reconciled = reduceArchive({ prior_activity_state: state as unknown as CanonicalObject, core_before: core(), proposed_core_after: changed,
    scheduled_timers: {}, recorded_stimulus: { stimulus_type: "core_proposed" } });
  assert.equal(reconciled.activity_disposition_type, "apply");
  if (reconciled.activity_disposition_type !== "apply") throw new Error("core rejected");
  const next = reconciled.next_activity_state as unknown as ArchiveState;
  assert.deepEqual(next.starting_crew, state.starting_crew);
  assert.equal(next.extraction.status, "none");
  const preview = action(next, "lead", "prepare_extraction", {}, changed);
  assert.deepEqual(preview.extraction.left_behind_roles, ["mira"]);
});

test("a departed companion remains in the immutable extraction liability", () => {
  const state = fresh();
  const changed = core();
  ((changed.memberships as CanonicalObject).jonah as { standing: string }).standing = "departed";
  const reconciled = reduceArchive({ prior_activity_state: state as unknown as CanonicalObject, core_before: core(), proposed_core_after: changed,
    scheduled_timers: {}, recorded_stimulus: { stimulus_type: "core_proposed" } });
  assert.equal(reconciled.activity_disposition_type, "apply");
  if (reconciled.activity_disposition_type !== "apply") throw new Error("core rejected");
  let next = reconciled.next_activity_state as unknown as ArchiveState;
  assert.deepEqual(next.starting_crew, state.starting_crew);
  next = action(next, "lead", "stage_extract", {}, changed);
  next = action(next, "lead", "prepare_extraction", {}, changed);
  assert.deepEqual(next.extraction.left_behind_roles, ["jonah"]);
});

test("suspended starting crew are at Atrium and represented as unavailable rather than absent", () => {
  const currentCore = core();
  ((currentCore.memberships as CanonicalObject).mira as { standing: string }).standing = "suspended";
  const state = pack.initialize({ configuration: { scenario_id: "standard-v1" }, initial_core_state: currentCore }).initial_activity_state as unknown as ArchiveState;
  assert.equal(state.mira.location, "atrium");
  assert.equal(state.mira.presence, "suspended");
  assert.equal(state.mira.mode, "unavailable");
  assert.deepEqual(state.starting_crew.map((member) => member.role), ["lead", "mira", "jonah"]);
});

test("each specialist sees only their own unshared investigation and lead sees neither", () => {
  let state = fresh();
  state = { ...state, mira: { ...state.mira, location: "records", mode: "holding" }, jonah: { ...state.jonah, location: "conservation", mode: "holding" } };
  state = plan(state, "mira", "investigate_records", [step("inspect_source", "none", "records"), step("share_source", "none", "records")]);
  state = plan(state, "jonah", "investigate_conservation", [step("inspect_source", "none", "conservation"), step("share_source", "none", "conservation")]);
  state = wait(state, ["mira", "jonah"]);
  for (const role of ["lead", "mira", "jonah"] as const) {
    const projection = participantProjection(state, role, core());
    const evidence = (projection.candidates as CanonicalObject[])[0]!.observed_evidence as CanonicalObject[];
    assert.deepEqual(evidence.map((item) => item.source_id), role === "lead" ? [] : [role === "mira" ? "records" : "conservation"]);
  }
});

test("extraction offers expose only the current preview, acknowledgement and commit stage", () => {
  let state = action(fresh(), "lead", "stage_extract");
  const relevant = (value: ArchiveState) => authorizedView(value, core(), { viewer_type: "participant", member_id: "lead" }).actionOffers
    .map((offer) => typeof offer === "string" ? offer : offer.actionType).filter((type) => ["prepare_extraction", "acknowledge_extraction", "commit_turn"].includes(type));
  assert.deepEqual(relevant(state), ["prepare_extraction"]);
  state = action(state, "lead", "prepare_extraction");
  assert.deepEqual(state.extraction.extracted_roles, ["lead", "mira", "jonah"]);
  assert.deepEqual(state.extraction.left_behind_roles, []);
  assert.deepEqual(relevant(state), ["acknowledge_extraction"]);
  state = action(state, "lead", "acknowledge_extraction", { preview_revision: state.extraction.revision, left_behind_roles: [] });
  assert.deepEqual(relevant(state), ["commit_turn"]);
  state = action(state, "lead", "commit_turn");
  assert.deepEqual((participantProjection(state, "lead", core()).crew_debrief as CanonicalObject).extracted_roles, ["lead", "mira", "jonah"]);
});

test("extraction cancels an old reply window and an explicit new request invalidates its acknowledgement", () => {
  let state = action(fresh(), "lead", "assign_mira_task", { task_kind: "investigate_records", power_allowance: 0 });
  state = action(state, "lead", "request_mira_plan");
  state = action(state, "lead", "stage_extract");
  state = action(state, "lead", "prepare_extraction");
  state = action(state, "lead", "acknowledge_extraction", { preview_revision: state.extraction.revision, left_behind_roles: [] });
  rejectAction(state, "mira", "submit_companion_plan", "stale_plan", { task_revision: state.mira.task.revision, opportunity_revision: state.mira.opportunity.revision, dialogue: "",
    steps: [step("move", "records")] });
  state = action(state, "lead", "request_mira_plan");
  assert.equal(state.extraction.status, "none");
  const accepted = action(state, "mira", "submit_companion_plan", { task_revision: state.mira.task.revision, opportunity_revision: state.mira.opportunity.revision, dialogue: "",
    steps: [step("move", "records")] });
  assert.equal(accepted.extraction.status, "none");
  assert.equal(accepted.turns_used, 0);
  const expired = reduceArchive({ prior_activity_state: state as unknown as CanonicalObject, core_before: core(), proposed_core_after: core(), scheduled_timers: {},
    recorded_stimulus: { stimulus_type: "timer_fired", timer_id: MIRA_PLAN_TIMER_ID,
      canonical_payload: { timer: "companion_plan_opportunity", task_revision: state.mira.task.revision, opportunity_revision: state.mira.opportunity.revision } } });
  assert.equal(expired.activity_disposition_type, "apply");
  if (expired.activity_disposition_type !== "apply") throw new Error("timer rejected");
  assert.equal((expired.next_activity_state as unknown as ArchiveState).extraction.status, "none");
  assert.equal((expired.next_activity_state as unknown as ArchiveState).turns_used, 0);
  assert.equal((expired.ordered_domain_events[0] as CanonicalObject).role, "mira");
  assert.equal((expired.ordered_domain_events[0] as CanonicalObject).action_type, "expire_companion_plan");
});

test("terminal full and partial extraction facts survive Core retirement and late specialist timers", () => {
  for (const partial of [false, true]) {
    let state = fresh();
    state = { ...state, carried_candidate_id: "ledger-violet",
      mira: { ...state.mira, location: "records", mode: "regrouping" },
      jonah: { ...state.jonah, location: partial ? "plant" : "atrium", mode: "holding" } };
    state = action(state, "lead", "stage_extract");
    state = action(state, "lead", "prepare_extraction");
    state = action(state, "lead", "acknowledge_extraction", { preview_revision: state.extraction.revision,
      left_behind_roles: [...state.extraction.left_behind_roles] });
    state = action(state, "lead", "commit_turn");
    assert.equal(state.outcome.kind, partial ? "partial_extraction" : "success");
    assert.deepEqual(state.completed_crew_work, [{ role: "mira", kind: "regroup_move", turn: 1 }]);
    const terminalFacts = (value: ArchiveState, currentCore: CanonicalObject) => canonicalStringify({
      outcome: value.outcome as unknown as CanonicalObject,
      extraction: value.extraction as unknown as CanonicalObject,
      crew_debrief: participantProjection(value, "lead", currentCore).crew_debrief!,
      completed_work: value.completed_crew_work as unknown as CanonicalObject[],
    });
    const expected = terminalFacts(state, core());
    const retiredCore = core();
    ((retiredCore.memberships as CanonicalObject).mira as { standing: string }).standing = "suspended";
    ((retiredCore.memberships as CanonicalObject).jonah as { standing: string }).standing = "departed";
    const retired = reduceArchive({ prior_activity_state: state as unknown as CanonicalObject,
      core_before: core(), proposed_core_after: retiredCore, scheduled_timers: {},
      recorded_stimulus: { stimulus_type: "core_proposed" } });
    assert.equal(retired.activity_disposition_type, "apply");
    if (retired.activity_disposition_type !== "apply") throw new Error("retirement rejected");
    state = retired.next_activity_state as unknown as ArchiveState;
    assert.equal(terminalFacts(state, retiredCore), expected, "post-terminal Membership changes must not rewrite extraction history");
    for (const timerId of [MIRA_PLAN_TIMER_ID, JONAH_PLAN_TIMER_ID]) {
      const expired = reduceArchive({ prior_activity_state: state as unknown as CanonicalObject,
        core_before: retiredCore, proposed_core_after: retiredCore, scheduled_timers: {},
        recorded_stimulus: { stimulus_type: "timer_fired", timer_id: timerId,
          canonical_payload: { timer: "companion_plan_opportunity", task_revision: 1, opportunity_revision: 1 } } });
      assert.equal(expired.activity_disposition_type, "apply");
      if (expired.activity_disposition_type !== "apply") throw new Error("late timer rejected");
      state = expired.next_activity_state as unknown as ArchiveState;
      assert.equal(terminalFacts(state, retiredCore), expected, "late planning timers must not rewrite extraction history");
    }
  }
});

test("extraction retains a digest without embedding private contribution or identity values", () => {
  let state = fresh();
  state = { ...state, verifier_result: "ledger-violet", carried_candidate_id: "ledger-violet",
    mira: { ...state.mira, knowledge: { ...state.mira.knowledge, verifier_result: "ledger-violet" } } };
  state = action(state, "lead", "stage_extract");
  state = action(state, "lead", "prepare_extraction");
  const fingerprint = state.extraction.contribution_fingerprint;
  assert.match(fingerprint, /^blake3:[0-9a-f]{64}$/u);
  for (const privateValue of ["ledger-violet", "lead", "mira", "jonah", "lead-principal", "mira-principal", "jonah-principal", "member_id", "principal_id"]) {
    assert.equal(fingerprint.includes(privateValue), false);
  }
});
