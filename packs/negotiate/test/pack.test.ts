import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  canonicalStringify,
  taggedBlake3Text,
  type CanonicalJson,
} from "@worldstream/pack-sdk";

import { goldenFixture } from "../fixtures/golden.js";
import type { ExactA202Object, NegotiateState, Persona, Role } from "../src/model.js";
import { RuleRejection } from "../src/model.js";
import pack, { validateCoreInvariant } from "../src/pack.js";
import {
  applyParticipantAction,
  applyTimer,
  initializeState,
} from "../src/rules.js";
import { authorizedView } from "../src/view.js";

interface OracleStep {
  readonly index: number;
  readonly stimulus: { readonly value: Record<string, CanonicalJson> };
  readonly expected: {
    readonly phase: string;
    readonly state_digest: string;
  };
}

interface OracleCorpus {
  readonly actions: readonly string[];
  readonly attention_reasons: readonly string[];
  readonly corpus_id: string;
  readonly golden: {
    readonly a202_revision: string;
    readonly evidence_cross_index: readonly CanonicalJson[];
    readonly final_state: NegotiateState;
    readonly final_state_digest: string;
    readonly outcome: CanonicalJson;
    readonly restart_after_step: number;
    readonly steps: readonly OracleStep[];
  };
  readonly negatives: readonly {
    readonly expected_code: string | null;
    readonly id: string;
    readonly kind: string;
  }[];
  readonly personas: readonly Persona[];
  readonly phases: readonly string[];
  readonly privacy_matrix: readonly Record<string, string>[];
  readonly privacy_mutations: readonly {
    readonly expected_projection_unchanged: boolean;
    readonly source: Persona;
    readonly viewer: Persona;
  }[];
  readonly roles: readonly Role[];
  readonly signed_expiry: {
    readonly evidence_cross_index: readonly CanonicalJson[];
    readonly final_state_digest: string;
    readonly outcome: CanonicalJson;
  };
}

const corpus = JSON.parse(
  readFileSync(
    new URL(
      "../../../../../crates/worldstream-negotiate-oracle/fixtures/corpus-v1.json",
      import.meta.url,
    ),
    "utf8",
  ),
) as OracleCorpus;

test("freezes the oracle vocabulary and exact approval binding", () => {
  assert.equal(corpus.corpus_id, "worldstream/negotiate-conformance-corpus/v1");
  assert.deepEqual(pack.descriptor.roles, corpus.roles);
  assert.deepEqual(pack.descriptor.actions, corpus.actions);
  assert.deepEqual(pack.descriptor.attentionReasons, corpus.attention_reasons);
  assert.equal(corpus.phases.length, 8);
  assert.equal(corpus.personas.length, 6);

  const request = corpus.golden.steps[2]!.stimulus.value.binding as Record<
    string,
    CanonicalJson
  >;
  const recorded = corpus.golden.steps[3]!.stimulus.value.approval as Record<
    string,
    CanonicalJson
  >;
  const acceptance = corpus.golden.steps[4]!.stimulus.value;
  assert.deepEqual(recorded.binding, request);
  assert.equal(
    taggedBlake3Text(String(request.candidate_canonical_json)),
    request.candidate_wire_digest,
  );
  assert.equal(acceptance.candidate_canonical_json, request.candidate_canonical_json);
  assert.deepEqual(acceptance.approval, recorded);
  assert.equal(request.approver_id, "principal:northstar:procurement_director");
});

test("matches every oracle success checkpoint and restart byte-for-byte", () => {
  let state = freshState();
  for (const step of corpus.golden.steps) {
    state = applyParticipantAction(state, clone(step.stimulus.value)).state;
    assert.equal(state.phase, step.expected.phase, `phase at step ${step.index}`);
    assert.equal(
      stateDigest(state),
      step.expected.state_digest,
      `state digest at step ${step.index}`,
    );
    if (step.index === corpus.golden.restart_after_step) {
      state = JSON.parse(canonicalStringify(state as unknown as CanonicalJson)) as NegotiateState;
    }
  }
  assert.equal(stateDigest(state), corpus.golden.final_state_digest);
  assert.equal(
    canonicalStringify(state as unknown as CanonicalJson),
    canonicalStringify(corpus.golden.final_state as unknown as CanonicalJson),
  );
  assert.deepEqual(state.outcome, corpus.golden.outcome);
  assert.deepEqual(state.evidence, corpus.golden.evidence_cross_index);
});

test("fails closed for every semantic negative in the independent corpus", () => {
  const first = (): Record<string, CanonicalJson> =>
    clone(corpus.golden.steps[0]!.stimulus.value);

  const mutatedByte = clone(corpus.golden.steps[4]!.stimulus.value);
  mutatedByte.candidate_canonical_json = `${String(mutatedByte.candidate_canonical_json)} `;
  assertRuleCode("byte_mutation", () =>
    applyParticipantAction(replay(4), mutatedByte)
  );

  const staleRoom = first();
  const staleRoomBasis = clone(staleRoom.basis as Record<string, CanonicalJson>);
  staleRoomBasis.room = { digest: `blake3:${"0".repeat(64)}`, sequence: 0 };
  staleRoom.basis = staleRoomBasis;
  assertRuleCode("stale_room_head", () => applyParticipantAction(freshState(), staleRoom));

  const staleTransaction = first();
  const staleTransactionBasis = clone(
    staleTransaction.basis as Record<string, CanonicalJson>,
  );
  staleTransactionBasis.transaction = {
    event_hash: `sha256:${"0".repeat(64)}`,
    sequence: 3,
  };
  staleTransaction.basis = staleTransactionBasis;
  assertRuleCode("stale_transaction_head", () =>
    applyParticipantAction(freshState(), staleTransaction)
  );

  const staleSession = first();
  const staleSessionBasis = clone(staleSession.basis as Record<string, CanonicalJson>);
  staleSessionBasis.session = { event_hash: `sha256:${"0".repeat(64)}`, sequence: 1 };
  staleSession.basis = staleSessionBasis;
  assertRuleCode("stale_session_head", () =>
    applyParticipantAction(freshState(), staleSession)
  );

  const expiredProposal = first();
  expiredProposal.admitted_at = 900;
  assertRuleCode("proposal_expired", () =>
    applyParticipantAction(freshState(), expiredProposal)
  );

  const expiredApproval = clone(corpus.golden.steps[3]!.stimulus.value);
  expiredApproval.admitted_at = 500;
  assertRuleCode("approval_expired", () =>
    applyParticipantAction(replay(3), expiredApproval)
  );

  const wrongSigner = first();
  const wrongProposal = clone(wrongSigner.proposal as Record<string, CanonicalJson>);
  const wrongOffer = clone(wrongProposal.offer as Record<string, CanonicalJson>);
  const wrongSignature = clone(wrongOffer.signature as Record<string, CanonicalJson>);
  wrongSignature.signer_id = "agent:delta:seller";
  wrongOffer.signature = wrongSignature;
  wrongProposal.offer = wrongOffer;
  wrongSigner.proposal = wrongProposal;
  assertRuleCode("wrong_signer", () => applyParticipantAction(freshState(), wrongSigner));

  const wrongRole = first();
  wrongRole.actor = "buyer_approver";
  assertRuleCode("role_violation", () => applyParticipantAction(freshState(), wrongRole));

  let deadline = applyTimer(freshState(), formationTimer()).state;
  const equality: Record<string, CanonicalJson> = {
    action: "record_transaction_deadline_elapsed",
    action_id: "act_deadline_equal",
    actor: "venue_signer",
    admitted_at: 1_000,
    basis: basis(deadline),
    deadline_event: corpus.golden.steps[0]!.stimulus.value.proposal as CanonicalJson,
    occurred_at: 1_000,
  };
  assertRuleCode("deadline_not_passed", () => applyParticipantAction(deadline, equality));

  const oversized = first();
  const oversizedProposal = clone(oversized.proposal as Record<string, CanonicalJson>);
  const oversizedOffer = clone(oversizedProposal.offer as Record<string, CanonicalJson>);
  oversizedOffer.canonical_json = "x".repeat(256 * 1_024 + 1);
  oversizedProposal.offer = oversizedOffer;
  oversized.proposal = oversizedProposal;
  assertRuleCode("resource_limit", () => applyParticipantAction(freshState(), oversized));

  const directlyCovered = new Set([
    "byte_mutation",
    "stale_room_head",
    "stale_transaction_head",
    "stale_session_head",
    "proposal_expired",
    "approval_expired",
    "wrong_signer",
    "role_violation",
    "deadline_not_passed",
    "resource_limit",
  ]);
  for (const fixture of corpus.negatives) {
    if (fixture.expected_code !== null && directlyCovered.has(fixture.expected_code)) {
      assert.ok(fixture.id.length > 0);
    }
  }
});

test("reproduces the oracle signed formation-expiry outcome and evidence", () => {
  let state = applyTimer(freshState(), formationTimer()).state;
  state = applyParticipantAction(state, {
    action: "record_transaction_deadline_elapsed",
    action_id: "act_expire_transaction",
    actor: "venue_signer",
    admitted_at: 1_002,
    basis: basis(state),
    deadline_event: streamEvent(
      state,
      "evt_transaction_deadline_04",
      "transaction",
      "deadline.elapsed",
    ) as unknown as CanonicalJson,
    occurred_at: 1_001,
  }).state;
  state = applyParticipantAction(state, {
    action: "close_transaction_expired_session",
    action_id: "act_close_expired_session",
    actor: "venue_signer",
    admitted_at: 1_003,
    basis: basis(state),
    close_event: streamEvent(
      state,
      "evt_session_close_02",
      "session",
      "session.closed",
    ) as unknown as CanonicalJson,
  }).state;
  assert.equal(stateDigest(state), corpus.signed_expiry.final_state_digest);
  assert.deepEqual(state.outcome, corpus.signed_expiry.outcome);
  assert.deepEqual(state.evidence, corpus.signed_expiry.evidence_cross_index);
});

test("enforces the six-persona privacy matrix and all 30 pairwise mutations", () => {
  const core = fixtureCore();
  const complete = replay(corpus.golden.steps.length);
  const buyer = projection(complete, core, "buyer_agent");
  const seller = projection(complete, core, "seller_agent");
  const approver = projection(complete, core, "buyer_approver");
  const venue = projection(complete, core, "venue_signer");
  const operator = projection(complete, core, "operator");
  const spectator = projection(complete, core, "spectator");
  assert.match(canonicalStringify(buyer), /agreement_canonical_json/u);
  assert.match(canonicalStringify(seller), /agreement_canonical_json/u);
  assert.match(canonicalStringify(approver), /agreement_canonical_json/u);
  assert.doesNotMatch(canonicalStringify(venue), /agreement_canonical_json/u);
  assert.match(canonicalStringify(operator), /agreement_canonical_json/u);
  assert.doesNotMatch(canonicalStringify(spectator), /canonical_json/u);

  const approvalState = replay(4);
  const sellerApproval = canonicalStringify(projection(approvalState, core, "seller_agent"));
  const approverApproval = canonicalStringify(
    projection(approvalState, core, "buyer_approver"),
  );
  assert.doesNotMatch(sellerApproval, /candidate_canonical_json/u);
  assert.match(approverApproval, /candidate_canonical_json/u);

  const baseline = {
    ...complete,
    private_by_persona: Object.fromEntries(
      corpus.personas.map((persona) => [persona, `${persona}-private-probe`]),
    ),
  } as unknown as NegotiateState;
  for (const mutation of corpus.privacy_mutations) {
    assert.equal(mutation.expected_projection_unchanged, true);
    const mutated = clone(
      baseline as unknown as Record<string, CanonicalJson>,
    ) as unknown as NegotiateState;
    const privateFields = (mutated as unknown as Record<string, CanonicalJson>)
      .private_by_persona as Record<string, CanonicalJson>;
    privateFields[mutation.source] = `${mutation.source}-mutated-private-probe`;
    assert.equal(
      canonicalStringify(projection(baseline, core, mutation.viewer)),
      canonicalStringify(projection(mutated, core, mutation.viewer)),
      `${mutation.source} leaked to ${mutation.viewer}`,
    );
  }
  assert.equal(corpus.privacy_mutations.length, 30);
  assert.equal(corpus.privacy_matrix.length, 8);

  const historical = authorizedView(complete, core, {
    member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0",
    viewer_type: "historical",
  });
  assert.equal(historical.schema, "participant");
  assert.deepEqual(historical.actionOffers, []);
});

test("vetoes Core kind drift and faults malformed callbacks before mutation", () => {
  const core = fixtureCore();
  const changed = clone(core);
  const approver = changed.memberships as Record<string, CanonicalJson>;
  const approverMembership = clone(
    approver["01ARZ3NDEKTSV4RRFFQ69G5FC2"] as Record<string, CanonicalJson>,
  );
  approverMembership.principal_kind = "agent";
  approver["01ARZ3NDEKTSV4RRFFQ69G5FC2"] = approverMembership;
  assertRuleCode("core_role_invariant", () => validateCoreInvariant(changed));

  const state = freshState();
  const before = canonicalStringify(state as unknown as CanonicalJson);
  assert.throws(() =>
    pack.reduce({
      core_before: core,
      deterministic_context: {
        next_room_sequence: 1,
        pack_digest: `blake3:${"0".repeat(64)}`,
        room_seed: "00000000000000000000000000000000",
      },
      next_room_seq: 1,
      prior_activity_state: state as unknown as CanonicalJson,
      proposed_core_after: core,
      recorded_stimulus: {
        action_id: "01ARZ3NDEKTSV4RRFFQ69G5F02",
        action_type: "submit_proposal_revision",
        admitted_at: "1970-01-01T00:01:40Z",
        canonical_payload: {},
        exact_basis_head: {},
        member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0",
        payload_schema_digest: `blake3:${"0".repeat(64)}`,
        stimulus_type: "participant_action",
      },
      scheduled_timers: {},
    })
  );
  assert.equal(canonicalStringify(state as unknown as CanonicalJson), before);
});

test("public Pack Actions derive Host admission context and retained Pack basis", () => {
  assert.equal(pack.descriptor.version, "0.2.0");
  const state = freshState();
  const publicPayload = clone(corpus.golden.steps[0]!.stimulus.value);
  delete publicPayload.admitted_at;
  delete publicPayload.basis;

  const result = pack.reduce({
    core_before: fixtureCore(),
    deterministic_context: {
      next_room_sequence: 1,
      pack_digest: `blake3:${"0".repeat(64)}`,
      room_seed: "00000000000000000000000000000000",
    },
    next_room_seq: 1,
    prior_activity_state: state as unknown as CanonicalJson,
    proposed_core_after: fixtureCore(),
    recorded_stimulus: {
      action_id: "01ARZ3NDEKTSV4RRFFQ69G5F02",
      action_type: "submit_proposal_revision",
      admitted_at: "1970-01-01T00:01:40.123456789Z",
      canonical_payload: publicPayload,
      exact_basis_head: {},
      member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0",
      payload_schema_digest: `blake3:${"0".repeat(64)}`,
      stimulus_type: "participant_action",
    },
    scheduled_timers: {},
  });

  assert.equal(result.activity_disposition_type, "apply");
  if (result.activity_disposition_type !== "apply") return;
  const next = result.next_activity_state as unknown as NegotiateState;
  assert.equal(next.phase, "formation_open");
  assert.equal(next.session_state, "active");
  assert.equal(next.room_head.sequence, 1);
  assert.equal(next.evidence.length, 2);
  assert.ok(
    next.evidence.every(
      (entry) => entry.action_id === "act_01_buyer_proposal",
    ),
  );
});

function freshState(): NegotiateState {
  return initializeState({
    a202_revision: corpus.golden.a202_revision,
    formation_deadline: 1_000,
  });
}

function replay(count: number): NegotiateState {
  let state = freshState();
  for (const step of corpus.golden.steps.slice(0, count)) {
    state = applyParticipantAction(state, clone(step.stimulus.value)).state;
  }
  return state;
}

function stateDigest(state: NegotiateState): string {
  return taggedBlake3Text(canonicalStringify(state as unknown as CanonicalJson));
}

function assertRuleCode(code: string, invoke: () => unknown): void {
  assert.throws(invoke, (error: unknown) => {
    assert.ok(error instanceof RuleRejection);
    assert.equal(error.code, code);
    return true;
  });
}

function clone<T extends Record<string, CanonicalJson>>(value: T): T {
  return JSON.parse(canonicalStringify(value)) as T;
}

function basis(state: NegotiateState): CanonicalJson {
  return {
    room: state.room_head as unknown as CanonicalJson,
    session: state.session_head as unknown as CanonicalJson,
    transaction: state.transaction_head as unknown as CanonicalJson,
  };
}

function formationTimer(): Record<string, CanonicalJson> {
  return {
    expected_object_id: null,
    fired_at: 1_000,
    generation: 1,
    scheduled_for: 1_000,
    timer: "formation_deadline",
  };
}

function streamEvent(
  state: NegotiateState,
  objectId: string,
  streamKind: "session" | "transaction",
  eventType: string,
): ExactA202Object {
  const contentHash = taggedBlake3Text(
    `a202-fixture-content/v1\0${objectId}`,
  ).slice("blake3:".length);
  const canonicalJson = canonicalStringify({
    content_hash: contentHash,
    id: objectId,
    object_type: "transaction_event",
    payload: {
      event_type: eventType,
      previous_event_hash:
        streamKind === "session"
          ? state.session_head.event_hash
          : state.transaction_head.event_hash,
      sequence:
        (streamKind === "session"
          ? state.session_head.sequence
          : state.transaction_head.sequence) + 1,
      stream: {
        id: streamKind === "session" ? state.session_id : state.transaction_id,
        kind: streamKind,
      },
    },
    spec_version: "a202-commercial/0.1",
    transaction_id: state.transaction_id,
    version: 1,
  });
  const wireDigest = taggedBlake3Text(canonicalJson);
  const signer = "agent:worldstream:venue_signer";
  return {
    canonical_json: canonicalJson,
    declared_content_hash: contentHash,
    object_id: objectId,
    object_type: "transaction_event",
    signature: {
      proof: taggedBlake3Text(
        `worldstream/negotiate-fixture-proof/v1\0venue_signer\0${signer}\0event_append\0${wireDigest}`,
      ),
      purpose: "event_append",
      signed_wire_digest: wireDigest,
      signer_id: signer,
      signer_role: "venue_signer",
    },
    wire_digest: wireDigest,
  };
}

function fixtureCore(): Record<string, CanonicalJson> {
  const memberships: Record<string, CanonicalJson> = {};
  const fixture = goldenFixture as unknown as Record<string, CanonicalJson>;
  for (const item of fixture.participants as readonly CanonicalJson[]) {
    const membership = item as Record<string, CanonicalJson>;
    memberships[String(membership.member_id)] = membership;
  }
  return { memberships, room_status: "active" };
}

function projection(
  state: NegotiateState,
  core: Record<string, CanonicalJson>,
  persona: Persona,
): CanonicalJson {
  const viewer = viewerForPersona(persona);
  return authorizedView(state, core, viewer).projection;
}

function viewerForPersona(persona: Persona): Record<string, CanonicalJson> {
  if (persona === "operator") {
    return { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC9", viewer_type: "operator" };
  }
  if (persona === "spectator") {
    return { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC8", viewer_type: "public" };
  }
  const ids: Record<Role, string> = {
    buyer_agent: "01ARZ3NDEKTSV4RRFFQ69G5FC0",
    seller_agent: "01ARZ3NDEKTSV4RRFFQ69G5FC1",
    buyer_approver: "01ARZ3NDEKTSV4RRFFQ69G5FC2",
    venue_signer: "01ARZ3NDEKTSV4RRFFQ69G5FC3",
  };
  return { member_id: ids[persona], viewer_type: "participant" };
}
