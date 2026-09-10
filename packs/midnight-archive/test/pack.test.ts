import assert from "node:assert/strict";
import test from "node:test";
import "./specialists.test.js";
import "./unavailable.test.js";

import {
  ACTIVITY_START_SOURCE_ID,
  canonicalStringify,
  type CanonicalJson,
  type CanonicalObject,
} from "@worldstream/pack-sdk";

import { bothObjectivesRouteActions } from "../fixtures/golden.js";
import type { ArchiveState } from "../src/model.js";
import { RuleRejection } from "../src/model.js";
import pack, { ACTIVITY_START_INPUT_TYPE, reduceArchive } from "../src/pack.js";
import {
  applyLeadAction,
  initializeArchiveState,
  startArchive,
  validateAuthoredScenario,
} from "../src/rules.js";
import { authorizedView, participantProjection } from "../src/view.js";
import {
  MIRA_PLAN_ATTENTION_REASON,
  MIRA_PLAN_TIMER_ID,
  applyMiraLeadControl,
  expireMiraOpportunity,
  initialMiraState,
  reconcileMira,
  submitMiraPlan,
} from "../src/companions.js";

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

test("lead Action offers remain in descriptor order after assigning Mira", () => {
  const assigned = miraControl(freshActiveWithMira(), "assign_mira_task", {
    task_kind: "investigate_records",
    power_allowance: 0,
  });
  const offered = authorizedView(
    assigned,
    core(),
    { viewer_type: "participant", member_id: "lead-1" },
  ).actionOffers.map((offer) => typeof offer === "string" ? offer : offer.actionType);
  const descriptorOrder = new Map(
    pack.descriptor.actions.map((action, index) => [action.actionType, index]),
  );
  const offeredOrder = offered.map((actionType) => descriptorOrder.get(actionType));

  assert.deepEqual(offered, [
    "stage_move",
    "stage_extract",
    "stage_wait",
    "assign_mira_task",
    "cancel_mira_task",
    "set_mira_follow",
    "set_mira_hold",
    "set_mira_regroup",
    "request_mira_plan",
    "defer_mira_contribution",
  ]);
  assert.deepEqual(offeredOrder, [...offeredOrder].sort((left, right) => left! - right!));
});

test("briefing is a complete safe projection with no Action offers", () => {
  const state = freshBriefing();
  const view = authorizedView(state, core(), { viewer_type: "participant", member_id: "lead-1" });
  assert.equal(view.schema, "participant");
  assert.deepEqual(view.actionOffers, []);
  assert.deepEqual(Object.keys(view.projection).sort(), [
    "candidates", "carried_candidate", "crew_debrief", "debrief", "extraction", "gates", "jonah", "location", "map", "mira",
    "objective", "optional_objectives", "outcome", "phase", "power", "preservation_agreement",
    "staged_action", "turn_resolution", "turns_remaining", "turns_used", "verifier_result",
  ]);
  assert.equal(view.projection.phase, "briefing");
  assert.equal(view.projection.staged_action, null);
  assert.equal(view.projection.carried_candidate, null);
  assert.equal(view.projection.verifier_result, null);
  assert.equal(view.projection.outcome, null);
  assert.equal(view.projection.debrief, null);
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

test("the fifteen-turn powered agreement route completes both optional objectives and replays byte-for-byte", () => {
  const first = play(freshActive(), bothObjectivesRouteActions);
  const restarted = play(
    JSON.parse(canonicalStringify(freshActive() as unknown as CanonicalJson)) as ArchiveState,
    bothObjectivesRouteActions,
  );
  assert.equal(first.phase, "complete");
  assert.equal(first.turns_used, 15);
  assert.equal(first.power_remaining, 0);
  assert.equal(first.outcome.kind, "success");
  assert.equal(first.carried_candidate_id, "ledger-violet");
  assert.equal(first.carried_confidence, "verified");
  assert.equal(first.preservation_agreement, "accepted");
  assert.equal(first.collection_preservation, "preserved");
  assert.equal(first.source_record_protected, true);
  assert.equal(first.gates.conservation_vault_open, true);
  assert.equal(
    canonicalStringify(first as unknown as CanonicalJson),
    canonicalStringify(restarted as unknown as CanonicalJson),
  );
});

test("extraction distinguishes wrong and missing ledgers without early truth leaks", () => {
  const missing = commit(freshActive(), "stage_extract", {});
  assert.equal(missing.outcome.kind, "no_ledger");

  const wrongActions = bothObjectivesRouteActions.map((action) =>
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
  assertRule("invalid_payload", () => applyLeadAction(state, "stage_accept_preservation_agreement", { terms: "anything" }));
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

test("the authored agreement requires a human lead and exposes fixed terms without dialogue", () => {
  let state = freshActive();
  state = commit(state, "stage_move", { destination: "conservation" });
  const projection = participantProjection(state, "lead");
  assert.deepEqual(projection.preservation_agreement, {
    speaker: "Archivist",
    statement: "Preserve the threatened collection and I will open the Conservation–Vault gate.",
    commitment: "not_accepted",
    conditions: [
      {
        condition_id: "lead_acceptance",
        label: "Human lead accepts this fixed agreement",
        status: "pending",
        turn_cost: 1,
        power_cost: 0,
      },
      {
        condition_id: "collection_preparation",
        label: "Prepare the threatened collection",
        status: "pending",
        turn_cost: 1,
        power_cost: 0,
      },
      {
        condition_id: "equipment_energized",
        label: "Energize the preservation equipment",
        status: "blocked",
        turn_cost: 1,
        power_cost: 1,
      },
    ],
  });

  const companion = reduceArchive(reduceInput(state, {
    stimulus_type: "participant_action",
    member_id: "mira-1",
    action_type: "stage_accept_preservation_agreement",
    canonical_payload: {},
  }));
  assert.equal(companion.activity_disposition_type, "reject");
  assert.equal(companion.declared_code, "role_violation");

  const inventedTerms = reduceArchive(reduceInput(state, {
    stimulus_type: "participant_action",
    member_id: "lead-1",
    action_type: "stage_accept_preservation_agreement",
    canonical_payload: { terms: "open now" },
  }));
  assert.equal(inventedTerms.activity_disposition_type, "reject");
  assert.equal(inventedTerms.declared_code, "invalid_payload");
  assert.equal(canonicalStringify(companion as unknown as CanonicalJson).includes("provider"), false);
  assert.equal(canonicalStringify(inventedTerms as unknown as CanonicalJson).includes("provider"), false);

  const accepted = reduceArchive(reduceInput(state, {
    stimulus_type: "participant_action",
    member_id: "lead-1",
    action_type: "stage_accept_preservation_agreement",
    canonical_payload: {},
  }));
  assert.equal(accepted.activity_disposition_type, "apply");
  if (accepted.activity_disposition_type !== "apply") throw new Error("agreement staging did not apply");
  assert.deepEqual(accepted.timer_requests, []);
  assert.deepEqual(accepted.ordered_attention_signals, []);
  assert.equal(canonicalStringify(accepted as unknown as CanonicalJson).includes("provider"), false);
});

test("contextual offers expose only currently eligible agreement and optional-objective actions", () => {
  let state = commit(freshActive(), "stage_move", { destination: "conservation" });
  assert.deepEqual(contextualOffers(state), [
    "stage_move",
    "stage_inspect_conservation",
    "stage_accept_preservation_agreement",
    "stage_prepare_collection",
    "stage_wait",
  ]);
  state = commit(state, "stage_prepare_collection", {});
  assert.deepEqual(contextualOffers(state), [
    "stage_move",
    "stage_inspect_conservation",
    "stage_accept_preservation_agreement",
    "stage_energize_preservation_equipment",
    "stage_wait",
  ]);
  state = commit(state, "stage_energize_preservation_equipment", {});
  assert.deepEqual(contextualOffers(state), [
    "stage_move",
    "stage_inspect_conservation",
    "stage_accept_preservation_agreement",
    "stage_wait",
  ]);
});

test("the catalog verifier cannot spend power again after recording its result", () => {
  let state = commit(freshActive(), "stage_move", { destination: "records" });
  state = commit(state, "stage_use_verifier", {});
  assert.equal(state.power_remaining, 2);
  assert.equal(contextualOffers(state).includes("stage_use_verifier"), false);
  assertRule("illegal_action", () => applyLeadAction(state, "stage_use_verifier", {}));
  assert.equal(state.power_remaining, 2);
});

test("agreement, preparation, and energizing commit on separate turns before the gate opens", () => {
  let state = commit(freshActive(), "stage_move", { destination: "conservation" });
  const acceptedStage = applyLeadAction(state, "stage_accept_preservation_agreement", {}).state;
  assert.equal(acceptedStage.turns_used, 1);
  assert.equal(acceptedStage.gates.conservation_vault_open, false);
  state = applyLeadAction(acceptedStage, "commit_turn", {}).state;
  assert.equal(state.turns_used, 2);
  assert.equal(state.preservation_agreement, "accepted");
  assert.equal(state.gates.conservation_vault_open, false);

  state = commit(state, "stage_prepare_collection", {});
  assert.equal(state.turns_used, 3);
  assert.equal(state.collection_preservation, "prepared");
  assert.equal(state.gates.conservation_vault_open, false);

  const energizedStage = applyLeadAction(state, "stage_energize_preservation_equipment", {}).state;
  assert.equal(energizedStage.turns_used, 3);
  assert.equal(energizedStage.power_remaining, 3);
  assert.equal(energizedStage.gates.conservation_vault_open, false);
  assertRule("gate_closed", () => applyLeadAction(energizedStage, "stage_move", { destination: "vault" }));
  state = applyLeadAction(energizedStage, "commit_turn", {}).state;
  assert.equal(state.turns_used, 4);
  assert.equal(state.power_remaining, 2);
  assert.equal(state.collection_preservation, "preserved");
  assert.equal(state.gates.conservation_vault_open, true);

  state = commit(state, "stage_move", { destination: "vault" });
  assert.equal(state.turns_used, 5);
  assert.equal(state.location, "vault");
});

test("collection preservation is independent of accepting or using the negotiated route", () => {
  let state = commit(freshActive(), "stage_move", { destination: "conservation" });
  state = commit(state, "stage_prepare_collection", {});
  state = commit(state, "stage_energize_preservation_equipment", {});
  assert.equal(state.collection_preservation, "preserved");
  assert.equal(state.preservation_agreement, "offered");
  assert.equal(state.gates.conservation_vault_open, false);
  assert.deepEqual(participantProjection(state, "lead").optional_objectives, {
    collection_preserved: {
      label: "Preserve the threatened collection",
      status: "complete",
      turn_cost: 2,
      power_cost: 1,
    },
    source_record_protected: {
      label: "Protect the source's identifying record",
      status: "locked",
      turn_cost: 1,
      power_cost: 1,
    },
  });
});

test("the eleven-turn powered agreement route succeeds with one charge and one optional objective", () => {
  const route = [
    ["stage_move", { destination: "records" }],
    ["stage_use_verifier", {}],
    ["stage_move", { destination: "conservation" }],
    ["stage_accept_preservation_agreement", {}],
    ["stage_prepare_collection", {}],
    ["stage_energize_preservation_equipment", {}],
    ["stage_move", { destination: "vault" }],
    ["stage_recover_candidate", { candidate_id: "ledger-violet" }],
    ["stage_move", { destination: "conservation" }],
    ["stage_move", { destination: "atrium" }],
    ["stage_extract", {}],
  ] as const;
  let state = freshActive();
  for (const [actionType, payload] of route) state = commit(state, actionType, payload);
  assert.equal(state.turns_used, 11);
  assert.equal(state.power_remaining, 1);
  assert.equal(state.outcome.kind, "success");
  assert.equal(state.collection_preservation, "preserved");
  assert.equal(state.source_record_protected, false);
  assert.deepEqual(participantProjection(state, "lead").debrief, {
    evidence_status: "none",
    message: "No authored source was inspected.",
    agreement_commitment: "honored",
    optional_objectives: {
      collection_preserved: true,
      source_record_protected: false,
    },
  });
});

test("source protection requires a recovered ledger, Plant, one turn, and one charge", () => {
  let state = freshActive();
  assertRule("illegal_action", () => applyLeadAction(state, "stage_protect_source_record", {}));
  for (const [actionType, payload] of [
    ["stage_move", { destination: "records" }],
    ["stage_move", { destination: "plant" }],
    ["stage_open_service_hatch", {}],
    ["stage_move", { destination: "vault" }],
    ["stage_recover_candidate", { candidate_id: "ledger-amber" }],
    ["stage_move", { destination: "plant" }],
  ] as const) state = commit(state, actionType, payload);
  const before = state;
  const staged = applyLeadAction(state, "stage_protect_source_record", {}).state;
  assert.equal(staged.turns_used, before.turns_used);
  assert.equal(staged.power_remaining, before.power_remaining);
  state = applyLeadAction(staged, "commit_turn", {}).state;
  assert.equal(state.turns_used, before.turns_used + 1);
  assert.equal(state.power_remaining, before.power_remaining - 1);
  assert.equal(state.source_record_protected, true);
});

test("insufficient power rejects preservation while a committed wait remains a recoverable setback", () => {
  let depleted = freshActive();
  for (const [actionType, payload] of [
    ["stage_move", { destination: "records" }],
    ["stage_use_verifier", {}],
    ["stage_move", { destination: "plant" }],
    ["stage_open_service_hatch", {}],
    ["stage_move", { destination: "records" }],
    ["stage_move", { destination: "conservation" }],
    ["stage_prepare_collection", {}],
  ] as const) depleted = commit(depleted, actionType, payload);
  assert.equal(depleted.power_remaining, 0);
  assertRule("insufficient_power", () =>
    applyLeadAction(depleted, "stage_energize_preservation_equipment", {})
  );

  let recovered = commit(freshActive(), "stage_wait", {});
  const route = [
    ["stage_move", { destination: "records" }],
    ["stage_use_verifier", {}],
    ["stage_move", { destination: "conservation" }],
    ["stage_accept_preservation_agreement", {}],
    ["stage_prepare_collection", {}],
    ["stage_energize_preservation_equipment", {}],
    ["stage_move", { destination: "vault" }],
    ["stage_recover_candidate", { candidate_id: "ledger-violet" }],
    ["stage_move", { destination: "conservation" }],
    ["stage_move", { destination: "atrium" }],
    ["stage_extract", {}],
  ] as const;
  for (const [actionType, payload] of route) recovered = commit(recovered, actionType, payload);
  assert.equal(recovered.turns_used, 12);
  assert.equal(recovered.outcome.kind, "success");
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
    candidate_id: "ledger-violet",
    confidence: "verified",
  });
});

test("authored sources validate ambiguity alone and a unique authentic intersection", () => {
  const scenario = {
    scenario_id: "standard-v1" as const,
    candidates: freshActive().candidates,
    authentic_candidate_id: "ledger-violet" as const,
    evidence_sources: [
      { source_id: "records" as const, source_label: "Records", attribute: "binding" as const, value: "calfskin" },
      { source_id: "conservation" as const, source_label: "Conservation", attribute: "marking" as const, value: "split_star" },
    ],
  };
  assert.doesNotThrow(() => validateAuthoredScenario(scenario));
  assert.throws(() => validateAuthoredScenario({
    ...scenario,
    evidence_sources: [
      { source_id: "records", source_label: "Records", attribute: "binding", value: "linen" },
      { source_id: "conservation", source_label: "Conservation", attribute: "marking", value: "split_star" },
    ],
  }), /more than one candidate possible/);
  assert.throws(() => validateAuthoredScenario({
    ...scenario,
    evidence_sources: [scenario.evidence_sources[0]!, scenario.evidence_sources[0]!],
  }), /Records and Conservation/);
  assert.throws(() => validateAuthoredScenario({
    ...scenario,
    authentic_candidate_id: "ledger-amber",
  }), /unique intersection/);
  assert.throws(() => validateAuthoredScenario({
    ...scenario,
    evidence_sources: [
      { source_id: "records", source_label: "Records", attribute: "binding", value: "calfskin" },
      { source_id: "conservation", source_label: "Conservation", attribute: "year", value: 1891 },
    ],
  }), /unique intersection/);
});

test("Records and Conservation inspections disclose sourced evidence only after committed turns", () => {
  let state = freshActive();
  state = commit(state, "stage_move", { destination: "records" });
  const before = participantProjection(state, "lead");
  assert.equal((before.candidates as { observed_evidence: unknown[] }[])[0]!.observed_evidence.length, 0);
  state = applyLeadAction(state, "stage_inspect_records", {}).state;
  assert.equal(state.turns_used, 1);
  state = applyLeadAction(state, "commit_turn", {}).state;
  assert.equal(state.turns_used, 2);
  const afterRecords = participantProjection(state, "lead");
  assert.equal((afterRecords.candidates as { observed_evidence: unknown[] }[])[0]!.observed_evidence.length, 1);
  assertNoTruth(afterRecords);
  state = commit(state, "stage_move", { destination: "conservation" });
  state = commit(state, "stage_inspect_conservation", {});
  const afterConservation = participantProjection(state, "lead");
  const violet = (afterConservation.candidates as { candidate_id: string; evidence_assessment: string }[])
    .find((candidate) => candidate.candidate_id === "ledger-violet");
  assert.equal(violet?.evidence_assessment, "recommended");
  assert.equal(state.gates.conservation_vault_open, false);
  assertNoTruth(afterConservation);
});

test("the twelve-turn solo evidence and service witness succeeds without the verifier", () => {
  const route = [
    ["stage_move", { destination: "conservation" }], ["stage_inspect_conservation", {}],
    ["stage_move", { destination: "records" }], ["stage_inspect_records", {}],
    ["stage_move", { destination: "plant" }], ["stage_open_service_hatch", {}],
    ["stage_move", { destination: "vault" }],
    ["stage_recover_candidate", { candidate_id: "ledger-violet" }],
    ["stage_move", { destination: "plant" }], ["stage_move", { destination: "records" }],
    ["stage_move", { destination: "atrium" }], ["stage_extract", {}],
  ] as const;
  let state = freshActive();
  for (const [actionType, actionPayload] of route) state = commit(state, actionType, actionPayload);
  assert.equal(state.turns_used, 12);
  assert.equal(state.outcome.kind, "success");
  assert.equal(state.carried_candidate_id, "ledger-violet");
  assert.equal(state.carried_confidence, "unverified");
  assert.equal(state.gates.conservation_vault_open, false);
  assert.equal(state.gates.plant_vault_open, true);
});

test("an unverified mistaken recovery can be exchanged later for another paid turn", () => {
  let state = freshActive();
  for (const [actionType, actionPayload] of [
    ["stage_move", { destination: "records" }], ["stage_inspect_records", {}],
    ["stage_move", { destination: "plant" }], ["stage_open_service_hatch", {}],
    ["stage_move", { destination: "vault" }], ["stage_recover_candidate", { candidate_id: "ledger-amber" }],
  ] as const) state = commit(state, actionType, actionPayload);
  assert.equal(state.carried_confidence, "unverified");
  const mistakenRecovery = state;
  const beforeExchange = state.turns_used;
  const powerBeforeExchange = state.power_remaining;
  state = commit(state, "stage_recover_candidate", { candidate_id: "ledger-violet" });
  assert.equal(state.turns_used, beforeExchange + 1);
  assert.equal(state.power_remaining, powerBeforeExchange);
  assert.equal(state.carried_candidate_id, "ledger-violet");
  assert.equal(state.carried_confidence, "unverified");

  let partial = mistakenRecovery;
  partial = commit(partial, "stage_move", { destination: "plant" });
  partial = commit(partial, "stage_move", { destination: "records" });
  partial = commit(partial, "stage_move", { destination: "atrium" });
  partial = commit(partial, "stage_extract", {});
  assert.equal(partial.outcome.kind, "wrong_ledger");
  assert.deepEqual(participantProjection(partial, "lead").debrief, {
    evidence_status: "partial",
    message: "Only one authored source was inspected; it did not uniquely identify a candidate.",
    agreement_commitment: "not_accepted",
    optional_objectives: {
      collection_preserved: false,
      source_record_protected: false,
    },
  });
});

test("terminal debrief reports partial evidence without claiming a hidden verification", () => {
  let state = freshActive();
  state = commit(state, "stage_move", { destination: "records" });
  state = commit(state, "stage_inspect_records", {});
  state = commit(state, "stage_move", { destination: "atrium" });
  state = commit(state, "stage_extract", {});
  const projection = participantProjection(state, "lead");
  assert.deepEqual(projection.debrief, {
    evidence_status: "partial",
    message: "Only one authored source was inspected; it did not uniquely identify a candidate.",
    agreement_commitment: "not_accepted",
    optional_objectives: {
      collection_preserved: false,
      source_record_protected: false,
    },
  });
  assertNoTruth(projection);
});

test("Mira advances one planned step beside each committed lead action and keeps inspection private until sharing", () => {
  let state = freshActiveWithMira();
  state = miraControl(state, "assign_mira_task", {
    task_kind: "investigate_records",
    power_allowance: 0,
  });
  state = applyLeadAction(state, "stage_move", { destination: "conservation" }, core()).state;
  const requested = applyMiraLeadControl(
    state,
    "request_mira_plan",
    {},
    core(),
    "2026-09-09T12:00:00.125Z",
    {},
  );
  state = requested.state;
  assert.equal(state.staged_action.kind, "move");
  assert.equal(state.turns_used, 0);
  assert.equal(state.mira.opportunity.deadline, "2026-09-09T12:00:15.125Z");
  assert.equal(requested.attentionSignals.length, 1);
  assert.equal(requested.timerRequests.length, 1);

  state = submitMiraPlan(state, {
    task_revision: 1,
    opportunity_revision: 1,
    steps: [
      { step_type: "move", destination: "records", source_id: "none", power_cost: 0 },
      { step_type: "inspect_source", destination: "none", source_id: "records", power_cost: 0 },
      { step_type: "share_source", destination: "none", source_id: "records", power_cost: 0 },
    ],
  }, core(), "mira-1", "2026-09-09T12:00:01.000Z", {}).state;
  assert.equal(canonicalStringify(participantProjection(state, "lead")).includes("inspect_source"), false);

  state = miraControl(state, "prepare_mira_contribution", {});
  state = applyLeadAction(state, "commit_turn", {}, core()).state;
  assert.equal(state.location, "conservation");
  assert.equal(state.mira.location, "records");
  assert.equal(state.mira.plan.next_step_index, 1);
  assert.equal(state.turns_used, 1);

  state = applyLeadAction(state, "stage_inspect_conservation", {}, core()).state;
  state = miraControl(state, "prepare_mira_contribution", {});
  state = applyLeadAction(state, "commit_turn", {}, core()).state;
  assert.equal(state.evidence.records, "unknown");
  assert.equal(state.evidence.conservation, "observed");
  assert.equal(state.mira.knowledge.records, "private");
  const leadEvidence = participantProjection(state, "lead").candidates as { observed_evidence: unknown[] }[];
  const miraEvidence = participantProjection(state, "mira").candidates as { observed_evidence: unknown[] }[];
  assert.equal(leadEvidence[0]!.observed_evidence.length, 1);
  assert.equal(miraEvidence[0]!.observed_evidence.length, 2);

  state = applyLeadAction(state, "stage_wait", {}, core()).state;
  state = miraControl(state, "prepare_mira_contribution", {});
  state = applyLeadAction(state, "commit_turn", {}, core()).state;
  assert.equal(state.evidence.records, "observed");
  assert.equal(state.mira.knowledge.records, "shared");
  assert.equal(state.mira.plan.status, "complete");
  assert.equal(state.mira.plan.next_step_index, 3);
  assert.equal(state.mira.mode, "holding");
});

test("the reducer authenticates Mira's one-shot plan and replays without a provider", () => {
  const active = freshActiveWithMira();
  const assigned = reduceArchive(reduceInput(active, {
    stimulus_type: "participant_action",
    member_id: "lead-1",
    action_type: "assign_mira_task",
    canonical_payload: { task_kind: "investigate_records", power_allowance: 0 },
  }));
  assert.equal(assigned.activity_disposition_type, "apply");
  if (assigned.activity_disposition_type !== "apply") throw new Error("assignment did not apply");
  const assignedState = assigned.next_activity_state as unknown as ArchiveState;
  const requestInput = reduceInput(assignedState, {
    stimulus_type: "participant_action",
    member_id: "lead-1",
    action_type: "request_mira_plan",
    admitted_at: "2026-09-09T12:00:00.000Z",
    canonical_payload: {},
  });
  const requested = reduceArchive(requestInput);
  const replayed = reduceArchive(requestInput);
  assert.equal(
    canonicalStringify(requested as unknown as CanonicalJson),
    canonicalStringify(replayed as unknown as CanonicalJson),
  );
  assert.equal(requested.activity_disposition_type, "apply");
  if (requested.activity_disposition_type !== "apply") throw new Error("plan request did not apply");
  assert.equal(requested.ordered_attention_signals.length, 1);
  assert.equal(
    (requested.ordered_attention_signals[0] as CanonicalObject | undefined)?.reason,
    "companion_plan_requested",
  );
  assert.equal(MIRA_PLAN_ATTENTION_REASON, "companion_plan_requested");
  assert.equal(requested.timer_requests.length, 1);
  assert.equal(canonicalStringify(requested as unknown as CanonicalJson).includes("provider"), false);
  const waiting = requested.next_activity_state as unknown as ArchiveState;

  const unauthorized = reduceArchive(reduceInput(waiting, {
    stimulus_type: "participant_action",
    member_id: "jonah-1",
    action_type: "submit_companion_plan",
    canonical_payload: {
      task_revision: 1,
      opportunity_revision: 1,
      steps: [{ step_type: "move", destination: "records", source_id: "none", power_cost: 0 }],
    },
  }));
  assert.equal(unauthorized.activity_disposition_type, "reject");
  assert.equal(unauthorized.declared_code, "companion_unavailable");

  const submitted = reduceArchive(reduceInput(waiting, {
    stimulus_type: "participant_action",
    member_id: "mira-1",
    action_type: "submit_companion_plan",
    admitted_at: "2026-09-09T12:00:01.000Z",
    canonical_payload: {
      task_revision: 1,
      opportunity_revision: 1,
      steps: [{ step_type: "move", destination: "records", source_id: "none", power_cost: 0 }],
    },
  }));
  assert.equal(submitted.activity_disposition_type, "apply");
  if (submitted.activity_disposition_type !== "apply") throw new Error("Mira plan did not apply");
  assert.equal((submitted.next_activity_state as unknown as ArchiveState).mira.plan.status, "active");
});

test("an exhausted short plan preserves its standing task and enables a bounded replan", () => {
  let state = freshActiveWithMira();
  assert.equal(contextualOffers(state).includes("request_mira_plan"), false);
  assert.equal(contextualOffers(state).includes("defer_mira_contribution"), false);
  assert.equal(contextualOffers(state).includes("cancel_mira_task"), false);

  state = miraControl(state, "assign_mira_task", {
    task_kind: "investigate_records",
    power_allowance: 0,
  });
  assert.equal(contextualOffers(state).includes("request_mira_plan"), true);
  assert.equal(contextualOffers(state).includes("defer_mira_contribution"), true);
  assert.equal(contextualOffers(state).includes("prepare_mira_contribution"), false);

  state = applyMiraLeadControl(
    state, "request_mira_plan", {}, core(), "2026-09-09T12:00:00.000Z", {},
  ).state;
  assert.equal(contextualOffers(state).includes("request_mira_plan"), false);
  assert.equal(contextualOffers(state).includes("defer_mira_contribution"), true);
  state = submitMiraPlan(state, {
    task_revision: 1,
    opportunity_revision: 1,
    steps: [{ step_type: "move", destination: "records", source_id: "none", power_cost: 0 }],
  }, core(), "mira-1", "2026-09-09T12:00:01.000Z", {}).state;
  assertRule("task_violation", () => applyMiraLeadControl(
    state, "request_mira_plan", {}, core(), "2026-09-09T12:00:02.000Z", {},
  ));
  state = applyLeadAction(state, "stage_wait", {}, core()).state;
  state = miraControl(state, "prepare_mira_contribution", {});
  state = applyLeadAction(state, "commit_turn", {}, core()).state;

  assert.equal(state.mira.location, "records");
  assert.equal(state.mira.plan.status, "complete");
  assert.equal(state.mira.task.status, "assigned");
  assert.equal(state.mira.mode, "tasked");
  assert.equal(state.mira.knowledge.records, "unknown");
  assert.equal(contextualOffers(state).includes("request_mira_plan"), true);
  assert.equal(contextualOffers(state).includes("prepare_mira_contribution"), false);

  state = applyMiraLeadControl(
    state, "request_mira_plan", {}, core(), "2026-09-09T12:00:03.000Z", {},
  ).state;
  state = submitMiraPlan(state, {
    task_revision: 1,
    opportunity_revision: 2,
    steps: [
      { step_type: "inspect_source", destination: "none", source_id: "records", power_cost: 0 },
      { step_type: "share_source", destination: "none", source_id: "records", power_cost: 0 },
    ],
  }, core(), "mira-1", "2026-09-09T12:00:04.000Z", {}).state;
  assertRule("plan_invalid", () => submitMiraPlan(
    applyMiraLeadControl(
      miraControl(state, "assign_mira_task", {
        task_kind: "investigate_records", power_allowance: 0,
      }),
      "request_mira_plan", {}, core(), "2026-09-09T12:00:05.000Z", {},
    ).state,
    {
      task_revision: 2,
      opportunity_revision: 3,
      steps: [
        { step_type: "inspect_source", destination: "none", source_id: "records", power_cost: 0 },
        { step_type: "share_source", destination: "none", source_id: "records", power_cost: 0 },
        { step_type: "move", destination: "atrium", source_id: "none", power_cost: 0 },
      ],
    },
    core(), "mira-1", "2026-09-09T12:00:06.000Z", {},
  ));
  for (let index = 0; index < 2; index += 1) {
    state = applyLeadAction(state, "stage_wait", {}, core()).state;
    state = miraControl(state, "prepare_mira_contribution", {});
    state = applyLeadAction(state, "commit_turn", {}, core()).state;
  }
  assert.equal(state.mira.knowledge.records, "shared");
  assert.equal(state.mira.task.status, "complete");
  assert.equal(state.mira.mode, "holding");
});

test("terminal turns close Mira planning and cancel its one-shot timer", () => {
  let state = { ...freshActiveWithMira(), turns_used: 15 };
  state = miraControl(state, "assign_mira_task", {
    task_kind: "investigate_records",
    power_allowance: 0,
  });
  state = applyMiraLeadControl(
    state, "request_mira_plan", {}, core(), "2026-09-09T12:00:00.000Z", {},
  ).state;
  state = applyLeadAction(state, "stage_wait", {}, core()).state;
  state = miraControl(state, "defer_mira_contribution", {});
  const input = {
    ...reduceInput(state, {
      stimulus_type: "participant_action",
      member_id: "lead-1",
      action_type: "commit_turn",
      canonical_payload: {},
    }),
    scheduled_timers: { [MIRA_PLAN_TIMER_ID]: { generation: 7 } },
  };
  const terminal = reduceArchive(input);
  assert.equal(terminal.activity_disposition_type, "apply");
  if (terminal.activity_disposition_type !== "apply") throw new Error("terminal commit did not apply");
  const finished = terminal.next_activity_state as unknown as ArchiveState;
  assert.equal(finished.phase, "complete");
  assert.notEqual(finished.mira.opportunity.status, "open");
  assert.equal(finished.mira.preparation.status, "none");
  assert.deepEqual(terminal.timer_requests, [{
    timer_request_type: "cancel_current",
    timer_id: MIRA_PLAN_TIMER_ID,
    expected_generation: 7,
  }]);
  assertRule("inactive", () => submitMiraPlan(finished, {
    task_revision: 1,
    opportunity_revision: 1,
    steps: [{ step_type: "move", destination: "records", source_id: "none", power_cost: 0 }],
  }, core(), "mira-1", "2026-09-09T12:00:01.000Z", {}));
});

test("extraction requires a preview before a prepared companion move away from the Atrium", () => {
  let state = freshActiveWithMira();
  state = miraControl(state, "assign_mira_task", {
    task_kind: "investigate_records",
    power_allowance: 0,
  });
  state = applyMiraLeadControl(
    state, "request_mira_plan", {}, core(), "2026-09-09T12:00:00.000Z", {},
  ).state;
  state = submitMiraPlan(state, {
    task_revision: 1,
    opportunity_revision: 1,
    steps: [{ step_type: "move", destination: "records", source_id: "none", power_cost: 0 }],
  }, core(), "mira-1", "2026-09-09T12:00:01.000Z", {}).state;
  state = applyLeadAction(state, "stage_extract", {}, core()).state;
  state = miraControl(state, "prepare_mira_contribution", {});

  assertRule("extraction_preview_required", () => applyLeadAction(state, "commit_turn", {}, core()));
  assert.equal(state.phase, "active");
  assert.equal(state.mira.location, "atrium");
});

test("late, stale, and replaced Companion Plans cannot execute", () => {
  let state = freshActiveWithMira();
  state = miraControl(state, "assign_mira_task", {
    task_kind: "investigate_records",
    power_allowance: 0,
  });
  state = applyMiraLeadControl(
    state, "request_mira_plan", {}, core(), "2026-09-09T12:00:00.000Z", {},
  ).state;
  const move = {
    task_revision: 1,
    opportunity_revision: 1,
    steps: [{ step_type: "move", destination: "records", source_id: "none", power_cost: 0 }],
  };
  assertRule("stale_plan", () =>
    submitMiraPlan(state, move, core(), "mira-1", "2026-09-09T12:00:15.000Z", {})
  );
  const expired = expireMiraOpportunity(state, {
    timer_id: MIRA_PLAN_TIMER_ID,
    canonical_payload: {
      timer: "companion_plan_opportunity",
      opportunity_revision: 1,
      task_revision: 1,
    },
  }).state;
  assert.equal(expired.mira.opportunity.status, "expired");
  assertRule("stale_plan", () =>
    submitMiraPlan(expired, move, core(), "mira-1", "2026-09-09T12:00:02.000Z", {})
  );

  const replacementCore = coreWithMira("mira-2", "enabled");
  const replaced = reconcileMira(state, replacementCore, {}).state;
  assert.equal(replaced.mira.member_id, "mira-2");
  assert.equal(replaced.mira.plan.status, "none");
  assert.equal(replaced.mira.preparation.status, "none");
  assertRule("role_violation", () =>
    submitMiraPlan(replaced, move, replacementCore, "mira-1", "2026-09-09T12:00:02.000Z", {})
  );
  const suspended = reconcileMira(replaced, coreWithMira("mira-2", "suspended"), {}).state;
  assert.equal(suspended.mira.presence, "suspended");
  assert.equal(suspended.mira.task.status, "cancelled");
});

test("task allowance fences verifier use and unprepared work remains explicitly deferrable", () => {
  let state = freshActiveWithMira();
  state = miraControl(state, "assign_mira_task", {
    task_kind: "investigate_records",
    power_allowance: 0,
  });
  state = applyMiraLeadControl(
    state, "request_mira_plan", {}, core(), "2026-09-09T12:00:00.000Z", {},
  ).state;
  assertRule("task_violation", () => submitMiraPlan(state, {
    task_revision: 1,
    opportunity_revision: 1,
    steps: [
      { step_type: "move", destination: "records", source_id: "none", power_cost: 0 },
      { step_type: "use_verifier", destination: "none", source_id: "records", power_cost: 1 },
    ],
  }, core(), "mira-1", "2026-09-09T12:00:01.000Z", {}));
  state = applyLeadAction(state, "stage_wait", {}, core()).state;
  assert.equal(applyLeadAction(state, "commit_turn", {}, core()).state.turns_used, 1);
  state = miraControl(state, "defer_mira_contribution", {});
  state = applyLeadAction(state, "commit_turn", {}, core()).state;
  assert.equal(state.turns_used, 1);
  assert.equal(state.mira.location, "atrium");

  let powered = freshActiveWithMira();
  powered = miraControl(powered, "assign_mira_task", {
    task_kind: "investigate_records",
    power_allowance: 1,
  });
  powered = applyMiraLeadControl(
    powered, "request_mira_plan", {}, core(), "2026-09-09T12:00:00.000Z", {},
  ).state;
  powered = submitMiraPlan(powered, {
    task_revision: 1,
    opportunity_revision: 1,
    steps: [
      { step_type: "move", destination: "records", source_id: "none", power_cost: 0 },
      { step_type: "use_verifier", destination: "none", source_id: "records", power_cost: 1 },
    ],
  }, core(), "mira-1", "2026-09-09T12:00:01.000Z", {}).state;
  for (let index = 0; index < 2; index += 1) {
    powered = applyLeadAction(powered, "stage_wait", {}, core()).state;
    powered = miraControl(powered, "prepare_mira_contribution", {});
    powered = applyLeadAction(powered, "commit_turn", {}, core()).state;
  }
  assert.equal(powered.power_remaining, 2);
  assert.equal(powered.mira.task.power_spent, 1);
  assert.equal(powered.mira.knowledge.verifier_result, "ledger-violet");
  assert.equal(powered.verifier_result, "ledger-violet");
});

test("regrouping moves one deterministic open edge per lead commit and gates extraction", () => {
  let state: ArchiveState = {
    ...freshActiveWithMira(),
    mira: { ...initialMiraState("mira-1"), location: "plant" as const, mode: "holding" as const },
  };
  state = applyLeadAction(state, "stage_extract", {}, core()).state;
  assertRule("extraction_preview_required", () => applyLeadAction(state, "commit_turn", {}, core()));
  state = miraControl(state, "set_mira_regroup", {});
  state = applyLeadAction(state, "stage_wait", {}, core()).state;
  state = applyLeadAction(state, "commit_turn", {}, core()).state;
  assert.equal(state.mira.location, "records");
  state = applyLeadAction(state, "stage_wait", {}, core()).state;
  state = applyLeadAction(state, "commit_turn", {}, core()).state;
  assert.equal(state.mira.location, "atrium");
  assert.equal(state.mira.mode, "holding");
  state = applyLeadAction(state, "stage_extract", {}, core()).state;
  state = acknowledgeCurrentExtraction(state, core());
  state = applyLeadAction(state, "commit_turn", {}, core()).state;
  assert.equal(state.outcome.kind, "no_ledger");
});

function freshBriefing(): ArchiveState {
  return initializeArchiveState({ scenario_id: "standard-v1" });
}

function freshActive(): ArchiveState {
  return startArchive(freshBriefing());
}

function freshActiveWithMira(): ArchiveState {
  return { ...freshActive(), mira: initialMiraState("mira-1"), starting_crew: [{ role: "lead", member_id: "lead-1" }, { role: "mira", member_id: "mira-1" }] };
}

function miraControl(state: ArchiveState, actionType: string, actionPayload: CanonicalObject): ArchiveState {
  return applyMiraLeadControl(
    state,
    actionType,
    actionPayload,
    core(),
    "2026-09-09T12:00:02.000Z",
    {},
  ).state;
}

function commit(
  state: ArchiveState,
  actionType: string,
  actionPayload: CanonicalObject,
): ArchiveState {
  let staged = applyLeadAction(state, actionType, actionPayload).state;
  if (actionType === "stage_extract") staged = acknowledgeCurrentExtraction(staged);
  return applyLeadAction(staged, "commit_turn", {}).state;
}

function acknowledgeCurrentExtraction(state: ArchiveState, currentCore?: CanonicalObject): ArchiveState {
  state = applyLeadAction(state, "prepare_extraction", {}, currentCore).state;
  return applyLeadAction(state, "acknowledge_extraction", { preview_revision: state.extraction.revision,
    left_behind_roles: [...state.extraction.left_behind_roles] }, currentCore).state;
}

function play(
  initial: ArchiveState,
  actions: readonly { readonly action_type: string; readonly canonical_payload: unknown }[],
): ArchiveState {
  let state = initial;
  for (const action of actions) {
    if (action.action_type === "commit_turn" && state.staged_action.kind === "extract" && state.extraction.status !== "acknowledged") {
      state = acknowledgeCurrentExtraction(state);
    }
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

function contextualOffers(state: ArchiveState): string[] {
  return authorizedView(
    state,
    core(),
    { viewer_type: "participant", member_id: "lead-1" },
  ).actionOffers.map((offer) => typeof offer === "string" ? offer : offer.actionType);
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

function coreWithMira(memberId: string, standing: "enabled" | "suspended"): CanonicalObject {
  const value = core();
  const memberships = value.memberships as CanonicalObject;
  return {
    memberships: {
      ...Object.fromEntries(Object.entries(memberships).filter(([id]) => id !== "mira-1")),
      [memberId]: { ...membership(memberId, "mira", "agent"), standing },
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
  const recordedStimulus = stimulus.stimulus_type === "participant_action" && stimulus.admitted_at === undefined
    ? { ...stimulus, admitted_at: "2026-09-09T12:00:00.000Z" }
    : stimulus;
  return {
    prior_activity_state: state as unknown as CanonicalJson,
    core_before: core(),
    proposed_core_after: core(),
    recorded_stimulus: recordedStimulus,
    scheduled_timers: {},
  };
}
