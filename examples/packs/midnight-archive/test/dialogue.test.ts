import assert from "node:assert/strict";
import test from "node:test";
import type { CanonicalObject } from "@worldstream/pack-sdk";
import pack, { reduceArchive } from "../src/pack.js";
import type { ArchiveState, CompanionRole } from "../src/model.js";
import { startArchive } from "../src/rules.js";
import { authorizedView } from "../src/view.js";
import { validateDialogue } from "../src/companions.js";

function core() { return { room_status: "active", memberships: Object.fromEntries(["lead", "mira", "jonah"].map(role => [role, {
  member_id: role, principal_id: `${role}-principal`, principal_kind: role === "lead" ? "human" : "agent",
  access_mode: "participant", role, standing: "enabled",
}])) } as CanonicalObject; }
function fresh() { return startArchive("2026-09-10T12:00:00Z", pack.initialize({ configuration: { scenario_id: "standard-v1" }, initial_core_state: core() }).initial_activity_state as unknown as ArchiveState); }
function reduce(state: ArchiveState, member: string, action: string, payload: CanonicalObject = {}) {
  return reduceArchive({ prior_activity_state: state as unknown as CanonicalObject, core_before: core(), proposed_core_after: core(), scheduled_timers: {},
    recorded_stimulus: { stimulus_type: "participant_action", member_id: member, action_type: action, canonical_payload: payload, admitted_at: "2026-09-10T12:00:01Z" } });
}
function act(state: ArchiveState, member: string, action: string, payload: CanonicalObject = {}) {
  const result = reduce(state, member, action, payload);
  assert.equal(result.activity_disposition_type, "apply", JSON.stringify(result));
  if (result.activity_disposition_type !== "apply") throw Error("rejected");
  return result.next_activity_state as unknown as ArchiveState;
}
function request(state: ArchiveState, role: CompanionRole) {
  state = act(state, "lead", `assign_${role}_task`, { task_kind: "investigate_records", power_allowance: 0 });
  return act(state, "lead", `request_${role}_plan`);
}
const move = { step_type: "move", destination: "records", source_id: "none", power_cost: 0 };
function proposal(state: ArchiveState, role: CompanionRole, dialogue: string, steps = [move]) {
  return { task_revision: state[role].task.revision, opportunity_revision: state[role].opportunity.revision, dialogue, steps };
}
function view(state: ArchiveState, member: string, viewerType = "participant", currentCore = core()) {
  return authorizedView(state, currentCore, { viewer_type: viewerType, member_id: member }).projection;
}

test("dialogue validates raw UTF-8 and double JSON byte boundaries without truncation", () => {
  for (const text of ["", "a".repeat(160), "é".repeat(80), "😀".repeat(40), '"'.repeat(46) + "aa"]) assert.equal(validateDialogue(text), text);
  for (const text of ["a".repeat(161), "é".repeat(81), "😀".repeat(41), '"'.repeat(47), "a\nb", "\u0000", "\u001f", "\ud800", "\udfff"]) assert.throws(() => validateDialogue(text));
  assert.equal(Buffer.byteLength(JSON.stringify(JSON.stringify('"'.repeat(46) + "aa"))), 192);
});

test("accepted utterances freeze full text and audience, while every rejected or silent plan records nothing", () => {
  const state = request(fresh(), "mira");
  const payload = proposal(state, "mira", "Recommend the Records route.");
  const result = reduce(state, "mira", "submit_companion_plan", payload);
  assert.equal(result.activity_disposition_type, "apply");
  if (result.activity_disposition_type !== "apply") throw Error("rejected");
  const accepted = result.next_activity_state as unknown as ArchiveState;
  const record = { speaker_role: "mira", speaker_member_id: "mira", lead_member_id: "lead", task_revision: 1, opportunity_revision: 1, turn: 0, text: payload.dialogue };
  assert.deepEqual(accepted.companion_dialogue, [record]);
  assert.deepEqual(result.ordered_domain_events.at(-1), { event_type: "companion_dialogue_recorded", ...record });
  assert.deepEqual(reduce(state, "mira", "submit_companion_plan", payload), result, "replay is provider-free and deterministic");
  for (const bad of [{ ...payload, task_revision: 2 }, { ...payload, steps: [{ ...move, destination: "vault" }] }, { ...payload, dialogue: "x".repeat(161) }, { task_revision: 1, opportunity_revision: 1, steps: [move] }]) {
    assert.equal(reduce(state, "mira", "submit_companion_plan", bad).activity_disposition_type, "reject");
    assert.deepEqual(state.companion_dialogue, []);
  }
  assert.equal(reduce(state, "jonah", "submit_companion_plan", payload).activity_disposition_type, "reject");
  assert.deepEqual(act(state, "mira", "submit_companion_plan", { ...payload, dialogue: "" }).companion_dialogue, []);
});

test("dialogue is visible only to original participant Memberships, including historical views", () => {
  let state = request(fresh(), "mira");
  state = act(state, "mira", "submit_companion_plan", proposal(state, "mira", "Only the original lead and Mira see this."));
  const expected = [{ speaker: "mira", turn: 0, text: "Only the original lead and Mira see this." }];
  for (const member of ["lead", "mira"]) for (const viewerType of ["participant", "historical"]) assert.deepEqual(view(state, member, viewerType).companion_dialogue, expected);
  assert.deepEqual(view(state, "jonah").companion_dialogue, []);
  for (const viewerType of ["public", "operator", "final_reveal"]) assert.equal(JSON.stringify(view(state, "lead", viewerType)).includes(expected[0]!.text), false);
  for (const role of ["lead", "mira"]) {
    const replacementCore = core(); const memberships = replacementCore.memberships as Record<string, CanonicalObject>;
    const original = memberships[role]!; delete memberships[role]; memberships[`${role}-new`] = { ...original, member_id: `${role}-new` };
    assert.deepEqual(view(state, `${role}-new`, "participant", replacementCore).companion_dialogue, []);
  }
  assert.equal(JSON.stringify(view(state, "lead")).includes("speaker_member_id"), false);
});

test("bounded retention evicts each speaker's oldest utterance deterministically and exposes newest four visible", () => {
  let state = fresh();
  for (let index = 0; index < 6; index++) for (const role of ["mira", "jonah"] as const) {
    state = request(state, role);
    state = act(state, role, "submit_companion_plan", proposal(state, role, `${role}-${index}`));
  }
  assert.equal(state.companion_dialogue.length, 8);
  assert.deepEqual(state.companion_dialogue.map(item => item.text), ["mira-2", "jonah-2", "mira-3", "jonah-3", "mira-4", "jonah-4", "mira-5", "jonah-5"]);
  assert.deepEqual((view(state, "mira").companion_dialogue as CanonicalObject[]).map(item => item.text), ["mira-2", "mira-3", "mira-4", "mira-5"]);
  assert.deepEqual((view(state, "lead").companion_dialogue as CanonicalObject[]).map(item => item.text), ["mira-4", "jonah-4", "mira-5", "jonah-5"]);
});

test("private source knowledge blocks dialogue until a structured share resolves", () => {
  for (const role of ["mira", "jonah"] as const) {
    let state = request(fresh(), role);
    state = act(state, role, "submit_companion_plan", proposal(state, role, "", [move, { step_type: "inspect_source", destination: "none", source_id: "records", power_cost: 0 }]));
    const advance = () => { state = act(state, "lead", "stage_wait"); state = act(state, "lead", `prepare_${role}_contribution`); state = act(state, "lead", "commit_turn"); };
    advance(); advance();
    state = act(state, "lead", `request_${role}_plan`);
    assert.equal((view(state, role)[role] as CanonicalObject).dialogue_allowed, false);
    const share = [{ step_type: "share_source", destination: "none", source_id: "records", power_cost: 0 }];
    assert.equal(reduce(state, role, "submit_companion_plan", proposal(state, role, "Leaking private information", share)).activity_disposition_type, "reject");
    state = act(state, role, "submit_companion_plan", proposal(state, role, "", share)); advance();
    assert.equal(state.evidence.records, "observed");
    assert.equal((view(state, role)[role] as CanonicalObject).dialogue_allowed, true);
    state = request(state, role);
    state = act(state, role, "submit_companion_plan", proposal(state, role, "I recommend the shared evidence.", [{ ...move, destination: "atrium" }]));
    assert.equal(state.companion_dialogue.length, 1);
  }
});
