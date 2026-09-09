import assert from "node:assert/strict";
import test from "node:test";

import {
  ACTIVITY_START_SOURCE_ID,
  canonicalStringify,
  type CanonicalJson,
  type CanonicalObject,
} from "@worldstream/pack-sdk";

import { technicalRouteActions } from "../fixtures/golden.js";
import type { ArchiveState } from "../src/model.js";
import { RuleRejection } from "../src/model.js";
import pack, { ACTIVITY_START_INPUT_TYPE, reduceArchive } from "../src/pack.js";
import {
  applyLeadAction,
  initializeArchiveState,
  startArchive,
} from "../src/rules.js";
import { authorizedView, participantProjection } from "../src/view.js";

test("declares the generic Activity Start contract and optional companion Roles", () => {
  assert.equal(pack.descriptor.packId, "worldstream.midnight-archive");
  assert.equal(pack.descriptor.version, "0.1.0");
  assert.deepEqual(pack.descriptor.roles, [
    { role: "lead", minimum: 1, maximum: 1 },
    { role: "mira", minimum: 0, maximum: 1 },
    { role: "jonah", minimum: 0, maximum: 1 },
  ]);
  assert.deepEqual(pack.descriptor.activityStartContract, {
    contract: "worldstream/activity-start/v1",
    preStartPhase: "briefing",
    inputType: ACTIVITY_START_INPUT_TYPE,
    canonicalPayload: { opened_by: "host" },
  });
});

test("briefing is a complete safe projection with no Action offers", () => {
  const state = freshBriefing();
  const view = authorizedView(state, core(), { viewer_type: "participant", member_id: "lead-1" });
  assert.equal(view.schema, "participant");
  assert.deepEqual(view.actionOffers, []);
  assert.deepEqual(Object.keys(view.projection).sort(), [
    "candidates", "carried_candidate", "gates", "location", "map", "objective",
    "outcome", "phase", "power", "staged_action", "turns_remaining", "turns_used",
    "verifier_result",
  ]);
  assert.equal(view.projection.phase, "briefing");
  assert.equal(view.projection.staged_action, null);
  assert.equal(view.projection.carried_candidate, null);
  assert.equal(view.projection.verifier_result, null);
  assert.equal(view.projection.outcome, null);
  assertNoTruth(view.projection);
});

test("only the exact Activity Start input leaves briefing", () => {
  const prior = freshBriefing();
  const accepted = reduceArchive(reduceInput(prior, {
    stimulus_type: "external_input",
    source_id: ACTIVITY_START_SOURCE_ID,
    input_type: ACTIVITY_START_INPUT_TYPE,
    canonical_payload: { opened_by: "host" },
    immutable_resource_references: [],
  }));
  assert.equal(accepted.activity_disposition_type, "apply");
  assert.equal((accepted.next_activity_state as CanonicalObject).phase, "active");
  assert.deepEqual(accepted.ordered_domain_events, [{ event_type: "archive_started", phase: "active" }]);

  const malformed = reduceArchive(reduceInput(prior, {
    stimulus_type: "external_input",
    source_id: ACTIVITY_START_SOURCE_ID,
    input_type: ACTIVITY_START_INPUT_TYPE,
    canonical_payload: { opened_by: "agent" },
    immutable_resource_references: [],
  }));
  assert.equal(malformed.activity_disposition_type, "reject");
  assert.equal(malformed.declared_code, "inactive");
});

test("staging and restaging are free; only commit spends a turn", () => {
  let state = freshActive();
  state = applyLeadAction(state, "stage_move", { destination: "records" }).state;
  assert.equal(state.turns_used, 0);
  assert.deepEqual(participantProjection(state, "lead").staged_action, {
    action_type: "stage_move",
    destination: "records",
    turn_cost: 1,
    power_cost: 0,
  });
  state = applyLeadAction(state, "stage_wait", {}).state;
  assert.equal(state.turns_used, 0);
  assert.deepEqual(participantProjection(state, "lead").staged_action, {
    action_type: "stage_wait",
    turn_cost: 1,
    power_cost: 0,
  });
  state = applyLeadAction(state, "commit_turn", {}).state;
  assert.equal(state.turns_used, 1);
  assert.equal(state.location, "atrium");
  assert.equal(state.staged_action.kind, "none");
});

test("the closed Vault gate cannot be crossed until a prior hatch commit", () => {
  let state = freshActive();
  state = commit(state, "stage_move", { destination: "records" });
  state = commit(state, "stage_move", { destination: "plant" });
  assertRule("gate_closed", () => applyLeadAction(state, "stage_move", { destination: "vault" }));
  state = commit(state, "stage_open_service_hatch", {});
  assert.equal(state.turns_used, 3);
  assert.equal(state.power_remaining, 1);
  assert.equal(state.gates.plant_vault_open, true);
  state = commit(state, "stage_move", { destination: "vault" });
  assert.equal(state.location, "vault");
  assert.equal(state.turns_used, 4);
});

test("the documented ten-turn technical route succeeds and replays byte-for-byte", () => {
  const first = play(freshActive(), technicalRouteActions);
  const restarted = play(
    JSON.parse(canonicalStringify(freshActive() as unknown as CanonicalJson)) as ArchiveState,
    technicalRouteActions,
  );
  assert.equal(first.phase, "complete");
  assert.equal(first.turns_used, 10);
  assert.equal(first.power_remaining, 0);
  assert.equal(first.outcome.kind, "success");
  assert.equal(first.carried_candidate_id, "ledger-cobalt");
  assert.equal(first.carried_confidence, "verified");
  assert.equal(
    canonicalStringify(first as unknown as CanonicalJson),
    canonicalStringify(restarted as unknown as CanonicalJson),
  );
});

test("extraction distinguishes wrong and missing ledgers without early truth leaks", () => {
  const missing = commit(freshActive(), "stage_extract", {});
  assert.equal(missing.outcome.kind, "no_ledger");

  const wrongActions = technicalRouteActions.map((action) =>
    action.action_type === "stage_recover_candidate"
      ? { ...action, canonical_payload: { candidate_id: "ledger-amber" } }
      : action
  );
  const wrong = play(freshActive(), wrongActions);
  assert.equal(wrong.outcome.kind, "wrong_ledger");
  assert.equal(wrong.carried_confidence, "unverified");

  let unverified = freshActive();
  unverified = commit(unverified, "stage_move", { destination: "records" });
  unverified = commit(unverified, "stage_move", { destination: "plant" });
  unverified = commit(unverified, "stage_open_service_hatch", {});
  unverified = commit(unverified, "stage_move", { destination: "vault" });
  unverified = commit(unverified, "stage_recover_candidate", { candidate_id: "ledger-cobalt" });
  const projection = participantProjection(unverified, "lead");
  assert.equal(projection.carried_candidate, "ledger-cobalt");
  assert.equal(projection.verifier_result, null);
  assertNoTruth(projection);
});

test("turn exhaustion is factual, while turn-16 extraction resolves first", () => {
  let exhausted = freshActive();
  for (let index = 0; index < 16; index += 1) {
    exhausted = commit(exhausted, "stage_wait", {});
  }
  assert.equal(exhausted.phase, "complete");
  assert.equal(exhausted.outcome.kind, "exhausted_inside");

  let lastTurn = freshActive();
  for (let index = 0; index < 15; index += 1) {
    lastTurn = commit(lastTurn, "stage_wait", {});
  }
  lastTurn = commit(lastTurn, "stage_extract", {});
  assert.equal(lastTurn.turns_used, 16);
  assert.equal(lastTurn.outcome.kind, "no_ledger");
});

test("malformed, illegal, companion, and stale commits reject without mutation", () => {
  const state = freshActive();
  const before = canonicalStringify(state as unknown as CanonicalJson);
  assertRule("invalid_payload", () => applyLeadAction(state, "stage_wait", { extra: true }));
  assertRule("illegal_action", () => applyLeadAction(state, "stage_use_verifier", {}));
  assertRule("nothing_staged", () => applyLeadAction(state, "commit_turn", {}));
  assert.equal(canonicalStringify(state as unknown as CanonicalJson), before);

  const companion = reduceArchive(reduceInput(state, {
    stimulus_type: "participant_action",
    member_id: "mira-1",
    action_type: "stage_wait",
    canonical_payload: {},
  }));
  assert.equal(companion.activity_disposition_type, "reject");
  assert.equal(companion.declared_code, "role_violation");
});

test("participant, public, operator, and final views never serialize private truth", () => {
  let state = freshActive();
  state = commit(state, "stage_move", { destination: "records" });
  state = commit(state, "stage_use_verifier", {});
  for (const viewer of [
    { viewer_type: "participant", member_id: "lead-1" },
    { viewer_type: "participant", member_id: "mira-1" },
    { viewer_type: "participant", member_id: "jonah-1" },
    { viewer_type: "public", member_id: "spectator-1" },
    { viewer_type: "operator", member_id: "operator-1" },
    { viewer_type: "final_reveal", member_id: "spectator-1" },
  ]) {
    assertNoTruth(authorizedView(state, core(), viewer).projection);
  }
  const lead = authorizedView(state, core(), { viewer_type: "participant", member_id: "lead-1" });
  assert.deepEqual(lead.projection.verifier_result, {
    candidate_id: "ledger-cobalt",
    confidence: "verified",
  });
});

function freshBriefing(): ArchiveState {
  return initializeArchiveState({ scenario_id: "standard-v1" });
}

function freshActive(): ArchiveState {
  return startArchive(freshBriefing());
}

function commit(
  state: ArchiveState,
  actionType: string,
  actionPayload: CanonicalObject,
): ArchiveState {
  const staged = applyLeadAction(state, actionType, actionPayload).state;
  return applyLeadAction(staged, "commit_turn", {}).state;
}

function play(
  initial: ArchiveState,
  actions: readonly { readonly action_type: string; readonly canonical_payload: unknown }[],
): ArchiveState {
  let state = initial;
  for (const action of actions) {
    state = applyLeadAction(
      state,
      action.action_type,
      action.canonical_payload as CanonicalObject,
    ).state;
  }
  return state;
}

function assertRule(code: string, operation: () => unknown): void {
  assert.throws(operation, (error: unknown) =>
    error instanceof RuleRejection && error.code === code
  );
}

function assertNoTruth(projection: CanonicalObject): void {
  const wire = canonicalStringify(projection);
  assert.equal(wire.includes("truth_marker"), false);
  assert.equal(wire.includes("authentic_candidate_id"), false);
}

function core(): CanonicalObject {
  return {
    memberships: {
      "lead-1": membership("lead-1", "lead", "human"),
      "mira-1": membership("mira-1", "mira", "agent"),
      "jonah-1": membership("jonah-1", "jonah", "agent"),
      "spectator-1": {
        access_mode: "spectator",
        member_id: "spectator-1",
        principal_id: "spectator-principal",
        principal_kind: "human",
        role: null,
        standing: "enabled",
      },
      "operator-1": {
        access_mode: "operator",
        member_id: "operator-1",
        principal_id: "operator-principal",
        principal_kind: "human",
        role: null,
        standing: "enabled",
      },
    },
  };
}

function membership(
  memberId: string,
  role: "lead" | "mira" | "jonah",
  principalKind: "human" | "agent",
): CanonicalObject {
  return {
    access_mode: "participant",
    member_id: memberId,
    principal_id: `${memberId}-principal`,
    principal_kind: principalKind,
    role,
    standing: "enabled",
  };
}

function reduceInput(state: ArchiveState, stimulus: CanonicalObject): CanonicalObject {
  return {
    prior_activity_state: state as unknown as CanonicalJson,
    core_before: core(),
    proposed_core_after: core(),
    recorded_stimulus: stimulus,
  };
}
