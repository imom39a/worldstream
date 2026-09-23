import assert from "node:assert/strict";
import test from "node:test";

import pack, { reduceTower } from "../src/pack.js";
import type { CanonicalJson, CanonicalObject } from "@worldstream/pack-sdk";

const A = "01ARZ3NDEKTSV4RRFFQ69G5FH1";
const B = "01ARZ3NDEKTSV4RRFFQ69G5FJ1";
const C = "01ARZ3NDEKTSV4RRFFQ69G5FL1";
const D = "01ARZ3NDEKTSV4RRFFQ69G5FM1";
const E = "01ARZ3NDEKTSV4RRFFQ69G5FP1";
const OBSERVER = "01ARZ3NDEKTSV4RRFFQ69G5FT1";
const PUBLIC = "01ARZ3NDEKTSV4RRFFQ69G5FK1";

function solver(member_id: string): CanonicalObject {
  return { access_mode: "participant", member_id, principal_id: `${member_id}P`, principal_kind: "agent", role: "solver", standing: "enabled" };
}
function observer(member_id: string): CanonicalObject {
  return { access_mode: "participant", member_id, principal_id: `${member_id}P`, principal_kind: "agent", role: "observer", standing: "enabled" };
}
function coreFor(ids: readonly string[], includeObserver = false): CanonicalObject {
  return {
    room_status: "active",
    memberships: {
      ...Object.fromEntries(ids.map((id) => [id, solver(id)])),
      ...(includeObserver ? { [OBSERVER]: observer(OBSERVER) } : {}),
    },
  };
}
function initial(inputCore: CanonicalObject = coreFor([A, B]), disks = 3): CanonicalObject {
  return pack.initialize({ configuration: { disks, move_limit: 10000 }, initial_core_state: inputCore }).initial_activity_state as CanonicalObject;
}
function reduce(state: CanonicalJson, member_id: string, action_type: string, payload: CanonicalJson, inputCore: CanonicalObject = coreFor([A, B]), seq = 1) {
  return reduceTower({
    core_before: inputCore,
    next_room_seq: seq,
    prior_activity_state: state as CanonicalObject,
    proposed_core_after: inputCore,
    recorded_stimulus: {
      action_id: "01H00000000000000000000000",
      action_type,
      admitted_at: "2026-09-13T12:00:00Z",
      canonical_payload: payload,
      exact_basis_head: { room_seq: seq - 1 },
      member_id,
      payload_schema_digest: "blake3:0000000000000000000000000000000000000000000000000000000000000",
      stimulus_type: "participant_action",
    },
  });
}
function applied(result: ReturnType<typeof reduceTower>): CanonicalObject {
  assert.equal(result.activity_disposition_type, "apply");
  if (result.activity_disposition_type !== "apply") throw new Error("expected apply");
  return result.next_activity_state as CanonicalObject;
}

// The Pack records only legal work and participant evidence. It cannot know a goal board.
test("records a legal move and increments work_revision", () => {
  const result = reduce(initial(), A, "move_disk", { from: "A", to: "C", disk: 1 });
  const next = applied(result);
  assert.deepEqual(next.board, { A: [3, 2], B: [], C: [1] });
  assert.equal(next.work_revision, 1);
  assert.equal(next.completion_claim, undefined);
  assert.deepEqual(next.claim_assessments, {});
  if (result.activity_disposition_type === "apply") {
    assert.deepEqual(result.ordered_domain_events, [{
      event_type: "disk_moved", member_id: A, move: { from: "A", to: "C", disk: 1 }, round: 2, work_revision: 1, claim_superseded: false,
    }]);
  }
});

test("allows one enabled agent solver and accepts its own anchored claim", () => {
  const single = coreFor([A]);
  const result = reduce(initial(single), A, "post_completion_claim", { work_revision: 0 }, single);
  const next = applied(result);
  assert.equal(next.phase, "complete");
  assert.deepEqual(next.outcome, { moves: 0, status: "participant_accepted_completion" });
  assert.deepEqual(next.completion_claim, { claimant_member_id: A, work_revision: 0, claim_round: 2, electorate_members: [A], quorum: 1 });
});

test("uses strict majority among five eligible solver Participants and never inspects board destination", () => {
  const five = coreFor([A, B, C, D, E]);
  const arbitrary = initial(five);
  const claimed = applied(reduce(arbitrary, A, "post_completion_claim", { work_revision: 0 }, five, 1));
  const firstEndorsement = applied(reduce(claimed, B, "assess_claim", { work_revision: 0, claim_round: 2, assessment: "endorse" }, five, 2));
  assert.equal(firstEndorsement.phase, "solving");
  const accepted = applied(reduce(firstEndorsement, C, "assess_claim", { work_revision: 0, claim_round: 2, assessment: "endorse" }, five, 3));
  assert.equal(accepted.phase, "complete");
  assert.equal((accepted.outcome as CanonicalObject).status, "participant_accepted_completion");
  // It remains the initial board: mechanics did not encode a completion predicate.
  assert.deepEqual(accepted.board, { A: [3, 2, 1], B: [], C: [] });
});

test("claim publication leaves moves offered and emits one review Attention signal per other eligible solver", () => {
  const five = coreFor([A, B, C, D, E], true);
  const claim = reduce(initial(five), A, "post_completion_claim", { work_revision: 0 }, five);
  const state = applied(claim);
  const claimantView = pack.view({ activity_state: state, core: five, viewer: { member_id: A, viewer_type: "participant" } });
  const reviewerView = pack.view({ activity_state: state, core: five, viewer: { member_id: B, viewer_type: "participant" } });
  assert.deepEqual(claimantView.action_offers, ["move_disk", "post_completion_claim"]);
  assert.deepEqual(reviewerView.action_offers, ["move_disk", "post_completion_claim", "assess_claim"]);
  if (claim.activity_disposition_type === "apply") {
    assert.deepEqual(claim.ordered_attention_signals, [B, C, D, E].map((target_member_id) => ({
      action_types: ["assess_claim"],
      deduplication_key: `claim-review:0:2:2:${target_member_id}`,
      priority: 1,
      reason: "claim_review_requested",
      target_member_id,
    })));
  }
});

test("a deferral re-signals every non-endorsing reviewer until quorum", () => {
  const five = coreFor([A, B, C, D, E], true);
  const claim = applied(reduce(initial(five), A, "post_completion_claim", { work_revision: 0 }, five));
  const deferred = reduce(claim, B, "assess_claim", { work_revision: 0, claim_round: 2, assessment: "defer" }, five, 2);
  assert.equal(deferred.activity_disposition_type, "apply");
  if (deferred.activity_disposition_type !== "apply") throw new Error("expected apply");
  const signals = deferred.ordered_attention_signals as ReadonlyArray<{
    target_member_id: string;
    reason: string;
    deduplication_key: string;
  }>;
  assert.deepEqual(signals.map((signal) => signal.target_member_id), [B, C, D, E]);
  for (const signal of signals) {
    assert.equal(signal.reason, "claim_review_requested");
    assert.equal(signal.deduplication_key, `claim-review:0:2:3:${signal.target_member_id}`);
  }
});

test("an accepted move supersedes the claim and all assessments while advancing work_revision", () => {
  const claimed = applied(reduce(initial(), A, "post_completion_claim", { work_revision: 0 }));
  const assessed = applied(reduce(claimed, B, "assess_claim", { work_revision: 0, claim_round: 2, assessment: "defer" }, coreFor([A, B]), 2));
  const move = reduce(assessed, B, "move_disk", { from: "A", to: "C", disk: 1 }, coreFor([A, B]), 3);
  const next = applied(move);
  assert.equal(next.work_revision, 1);
  assert.equal(next.completion_claim, undefined);
  assert.deepEqual(next.claim_assessments, {});
  if (move.activity_disposition_type === "apply") assert.equal((move.ordered_domain_events[0]! as CanonicalObject).claim_superseded, true);
});

test("rejects an assessment anchored to superseded work and lets a reviewer update its latest assessment", () => {
  const claimed = applied(reduce(initial(), A, "post_completion_claim", { work_revision: 0 }));
  const endorsed = applied(reduce(claimed, B, "assess_claim", { work_revision: 0, claim_round: 2, assessment: "endorse" }, coreFor([A, B]), 2));
  // With two participants this endorsement accepts, so use five reviewers for update mechanics.
  const five = coreFor([A, B, C, D, E]);
  const fiveClaim = applied(reduce(initial(five), A, "post_completion_claim", { work_revision: 0 }, five));
  const first = applied(reduce(fiveClaim, B, "assess_claim", { work_revision: 0, claim_round: 2, assessment: "endorse" }, five, 2));
  const updated = applied(reduce(first, B, "assess_claim", { work_revision: 0, claim_round: 2, assessment: "challenge" }, five, 3));
  assert.deepEqual(updated.claim_assessments, { [B]: "challenge" });
  assert.equal(updated.phase, "solving");
  const moved = applied(reduce(updated, C, "move_disk", { from: "A", to: "B", disk: 1 }, five, 4));
  const stale = reduce(moved, B, "assess_claim", { work_revision: 0, claim_round: 2, assessment: "endorse" }, five, 5);
  assert.equal(stale.activity_disposition_type, "reject");
  if (stale.activity_disposition_type === "reject") assert.equal(stale.declared_code, "claim_absent");
  assert.equal((endorsed.outcome as CanonicalObject).status, "participant_accepted_completion");
});

test("rejects absent, self-authored, stale-revision, malformed, and observer claim actions", () => {
  const state = initial();
  const absent = reduce(state, B, "assess_claim", { work_revision: 0, claim_round: 2, assessment: "endorse" });
  assert.equal(absent.activity_disposition_type, "reject");
  if (absent.activity_disposition_type === "reject") assert.equal(absent.declared_code, "claim_absent");
  const claim = applied(reduce(state, A, "post_completion_claim", { work_revision: 0 }));
  const self = reduce(claim, A, "assess_claim", { work_revision: 0, claim_round: 2, assessment: "endorse" }, coreFor([A, B]), 2);
  assert.equal(self.activity_disposition_type, "reject");
  if (self.activity_disposition_type === "reject") assert.equal(self.declared_code, "claimant_cannot_assess");
  const stale = reduce(claim, B, "assess_claim", { work_revision: 1, claim_round: 2, assessment: "endorse" }, coreFor([A, B]), 2);
  assert.equal(stale.activity_disposition_type, "reject");
  if (stale.activity_disposition_type === "reject") assert.equal(stale.declared_code, "claim_superseded");
  const malformed = reduce(state, A, "post_completion_claim", { work_revision: 0, extra: true });
  assert.equal(malformed.activity_disposition_type, "reject");
  const withObserver = coreFor([A, B], true);
  const observerAction = reduce(initial(withObserver), OBSERVER, "post_completion_claim", { work_revision: 0 }, withObserver);
  assert.equal(observerAction.activity_disposition_type, "reject");
  if (observerAction.activity_disposition_type === "reject") assert.equal(observerAction.declared_code, "role_violation");
});

test("provides an actionless observer view and historical solver view", () => {
  const inputCore = coreFor([A, B], true);
  const state = initial(inputCore, 10);
  const observerView = pack.view({ activity_state: state, core: inputCore, viewer: { member_id: OBSERVER, viewer_type: "participant" } });
  const historicalSolver = pack.view({ activity_state: state, core: inputCore, viewer: { member_id: A, viewer_type: "historical" } });
  assert.equal(observerView.projection_schema, "participant");
  assert.deepEqual(observerView.action_offers, []);
  assert.equal(observerView.projection.can_move, false);
  assert.equal(observerView.projection.private_role, "observer");
  assert.equal(historicalSolver.projection_schema, "historical_participant");
  assert.deepEqual(historicalSolver.action_offers, []);
  assert.equal(historicalSolver.projection.can_move, false);
  assert.equal(historicalSolver.projection.private_role, "solver");
  assert.deepEqual((observerView.projection.board as CanonicalObject).A, [10, 9, 8, 7, 6, 5, 4, 3, 2, 1]);
});

test("public projection exposes claim mechanics but no member-private fields", () => {
  const claimed = applied(reduce(initial(), A, "post_completion_claim", { work_revision: 0 }));
  const view = pack.view({ activity_state: claimed, core: coreFor([A, B]), viewer: { member_id: PUBLIC, viewer_type: "public" } });
  assert.equal(view.projection_schema, "public");
  assert.deepEqual(view.action_offers, []);
  const completion = view.projection.completion as CanonicalObject;
  assert.equal(completion.claim_open, true);
  assert.deepEqual(completion.claim, { claimant_member_id: A, claim_round: 2, electorate_size: 2, quorum: 2, work_revision: 0 });
  assert.ok(!("private_member_id" in view.projection));
  assert.ok(!("member_notice" in view.projection));
});

test("has no path, target-board, or answer-policy fields in source and descriptor", () => {
  assert.doesNotMatch(String(reduceTower), /optimal|solution|target[_-]?board/i);
  assert.doesNotMatch(JSON.stringify(pack.descriptor), /optimal|solution|target[_-]?board/i);
});

test("move Attention wakes every current eligible solver, including its actor", () => {
  const five = coreFor([A, B, C, D, E]);
  const moved = reduce(initial(five), A, "move_disk", { from: "A", to: "C", disk: 1 }, five);
  if (moved.activity_disposition_type !== "apply") throw new Error("expected apply");
  assert.deepEqual(moved.ordered_attention_signals, [A, B, C, D, E].map((target_member_id) => ({
    action_types: ["move_disk", "post_completion_claim"],
    deduplication_key: `board-changed:1:${target_member_id}`,
    priority: 1,
    reason: "board_changed",
    target_member_id,
  })));
});

test("does not overwrite a current claim and fences assessments by exact claim identity", () => {
  const five = coreFor([A, B, C, D, E]);
  const firstClaim = applied(reduce(initial(five), A, "post_completion_claim", { work_revision: 0 }, five));
  const overwrite = reduce(firstClaim, B, "post_completion_claim", { work_revision: 0 }, five, 2);
  assert.equal(overwrite.activity_disposition_type, "reject");
  if (overwrite.activity_disposition_type === "reject") assert.equal(overwrite.declared_code, "claim_open");
  const moved = applied(reduce(firstClaim, B, "move_disk", { from: "A", to: "C", disk: 1 }, five, 2));
  const secondClaim = applied(reduce(moved, B, "post_completion_claim", { work_revision: 1 }, five, 3));
  const staleIdentity = reduce(secondClaim, C, "assess_claim", { work_revision: 1, claim_round: 2, assessment: "endorse" }, five, 4);
  assert.equal(staleIdentity.activity_disposition_type, "reject");
  if (staleIdentity.activity_disposition_type === "reject") assert.equal(staleIdentity.declared_code, "claim_superseded");
});

test("a claim retains its electorate and majority after a later Membership view changes", () => {
  const five = coreFor([A, B, C, D, E]);
  const claim = applied(reduce(initial(five), A, "post_completion_claim", { work_revision: 0 }, five));
  const reducedCore = coreFor([A, B]);
  const view = pack.view({ activity_state: claim, core: reducedCore, viewer: { member_id: A, viewer_type: "participant" } });
  const completion = view.projection.completion as CanonicalObject;
  assert.deepEqual(completion.claim, { claimant_member_id: A, claim_round: 2, electorate_size: 5, quorum: 3, work_revision: 0 });
  assert.equal(completion.quorum, 3);
  assert.equal(completion.approval_count, 1);
});

test("the public A-to-C objective does not auto-complete a target-looking board", () => {
  const state = initial(coreFor([A]));
  const targetLooking = { ...state, board: { A: [], B: [], C: [3, 2, 1] } };
  const view = pack.view({ activity_state: targetLooking, core: coreFor([A]), viewer: { member_id: A, viewer_type: "participant" } });
  assert.deepEqual(view.projection.objective, { source_rod: "A", target_rod: "C", description: "Participants may aim to move the full tower from A to C." });
  assert.equal(view.projection.phase, "solving");
  assert.deepEqual(view.projection.outcome, { moves: 0, status: "in_progress" });
});

test("accepts a reviewed mid-state initial_board and starts from it", () => {
  const state = pack.initialize({
    configuration: { disks: 3, move_limit: 10000, initial_board: { A: [3], B: [2, 1], C: [] } },
    initial_core_state: coreFor([A, B]),
  }).initial_activity_state as CanonicalObject;
  assert.deepEqual(state.board, { A: [3], B: [2, 1], C: [] });
  assert.equal(state.work_revision, 0);
  const view = pack.view({ activity_state: state, core: coreFor([A, B]), viewer: { member_id: A, viewer_type: "participant" } });
  assert.deepEqual(view.projection.board, { A: [3], B: [2, 1], C: [] });
});

test("rejects an initial_board that duplicates, omits, or blocks a disk", () => {
  const bad = [
    { A: [3, 3], B: [], C: [1] },
    { A: [3], B: [2], C: [] },
    { A: [2, 3], B: [1], C: [] },
  ];
  for (const initial_board of bad) {
    assert.throws(() => pack.initialize({
      configuration: { disks: 3, move_limit: 10000, initial_board },
      initial_core_state: coreFor([A, B]),
    }));
  }
});
