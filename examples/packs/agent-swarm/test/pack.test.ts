import assert from "node:assert/strict";
import test from "node:test";

import type { CanonicalJson, CanonicalObject, PackReduceOutput } from "@worldstream/pack-sdk";
import pack, { reduceSwarm } from "../src/pack.js";
import { PROGRESS_REVIEW_TIMER_ID } from "../src/workflow.js";

const HUMAN = "01ARZ3NDEKTSV4RRFFQ69G5SA1";
const WORKER_A = "01ARZ3NDEKTSV4RRFFQ69G5SA2";
const WORKER_B = "01ARZ3NDEKTSV4RRFFQ69G5SA3";

function membership(member_id: string, role: "human_coordinator" | "worker", principal_kind: "agent" | "human"): CanonicalObject {
  return { access_mode: "participant", member_id, principal_id: `${member_id}P`, principal_kind, role, standing: "enabled" };
}

function core(): CanonicalObject {
  return { room_status: "active", memberships: {
    [HUMAN]: membership(HUMAN, "human_coordinator", "human"),
    [WORKER_A]: membership(WORKER_A, "worker", "agent"),
    [WORKER_B]: membership(WORKER_B, "worker", "agent"),
  } };
}

function roster(key: string): CanonicalObject {
  return {
    member_key: key, label: `Fixture ${key}`, moving_alias_acknowledged: false, provider: "codex",
    requested_model: "fixture-model-v1", requested_effort: "medium", configuration_state: "fixture_unavailable",
  };
}

function initial(
  progressReviewIntervalSeconds = 300,
  correctionFailureLimit = 3,
  acceptanceCriteria: string[] = ["The report cites each source."],
): CanonicalObject {
  return pack.initialize({
    configuration: {
      goal: "Prepare a local supplied-material report.", constraints: ["Use only approved local inputs."],
      acceptance_criteria: acceptanceCriteria, approved_working_area: { root: "/tmp/swarm", resource_paths: ["inputs", "result"] },
      roster: [roster("worker-a"), roster("worker-b")], progress_review_interval_seconds: progressReviewIntervalSeconds, correction_failure_limit: correctionFailureLimit,
    },
    initial_core_state: core(),
  }).initial_activity_state as CanonicalObject;
}

function applied(result: PackReduceOutput): CanonicalObject {
  assert.equal(result.activity_disposition_type, "apply");
  if (result.activity_disposition_type !== "apply") throw new Error("expected apply");
  return result.next_activity_state as CanonicalObject;
}

let actionSequence = 0;
function action(
  state: CanonicalObject,
  action_type: string,
  canonical_payload: CanonicalObject,
  member_id = WORKER_A,
  scheduled_timers: CanonicalObject = {},
  admittedAt = "2026-09-15T12:00:00Z",
): PackReduceOutput {
  actionSequence += 1;
  const roomSeq = state.room_seq as number;
  return reduceSwarm({
    core_before: core(), proposed_core_after: core(), prior_activity_state: state, next_room_seq: roomSeq + 1, scheduled_timers,
    recorded_stimulus: {
      action_id: `action-${actionSequence}`, action_type, admitted_at: admittedAt, canonical_payload,
      exact_basis_head: { room_seq: roomSeq }, member_id, payload_schema_digest: `blake3:${"0".repeat(64)}`, stimulus_type: "participant_action",
    },
  });
}

function timer(state: CanonicalObject, scheduledFor = "2026-09-15T12:05:00Z"): PackReduceOutput {
  const roomSeq = state.room_seq as number;
  return reduceSwarm({
    core_before: core(), proposed_core_after: core(), prior_activity_state: state, next_room_seq: roomSeq + 1, scheduled_timers: {},
    recorded_stimulus: {
      canonical_payload: { execution_epoch: state.execution_epoch as number, timer: "progress_review" },
      generation: 1, scheduled_for: scheduledFor, stimulus_type: "timer_fired", timer_id: PROGRESS_REVIEW_TIMER_ID,
    },
  });
}

function confirm(state = initial()): CanonicalObject {
  return applied(action(state, "confirm_initial_setup", { setup_revision: 2 }, HUMAN));
}

function propose(state: CanonicalObject, work_id: string, dependencies: string[] = [], kind = "goal"): CanonicalObject {
  return applied(action(state, "propose_work_item", {
    dependency_ids: dependencies, description: `Complete ${work_id}.`, expected_goal_revision: 1, kind, title: work_id, work_id,
  }));
}

function claim(state: CanonicalObject, work_id: string, actor = WORKER_A): CanonicalObject {
  const work = (state.work_items as CanonicalObject[]).find((item) => item.work_id === work_id)!;
  return applied(action(state, "claim_work_item", { attempt_id: `attempt-${work_id}`, expected_work_revision: work.revision as number, work_id }, actor));
}

function artifact(id: string): CanonicalObject {
  return { artifact_id: id, digest: `blake3:${id}`, local_path: `/tmp/swarm/${id}.md`, media_type: "text/markdown" };
}

function checkEvidence(
  candidate_id: string,
  version: number,
  check_id: string,
  resource_basis: CanonicalObject[] = [],
  criterion = "The report cites each source.",
): CanonicalObject {
  return {
    artifact: {
      artifact_id: `evidence-${candidate_id}-${version}-${check_id}`,
      digest: `blake3:${"e".repeat(64)}`,
      local_path: `/tmp/swarm/evidence-${candidate_id}-${version}-${check_id}.json`,
      media_type: "application/vnd.worldstream.agent-swarm-check-evidence+json;version=1",
    },
    candidate: { candidate_id, version }, candidate_artifact: artifact(candidate_id), check_id, criterion, criteria_revision: 1, resource_basis,
  };
}

function contribute(state: CanonicalObject, work_id: string, contribution_id: string, actor = WORKER_A): CanonicalObject {
  const work = (state.work_items as CanonicalObject[]).find((item) => item.work_id === work_id)!;
  const attempt = (state.work_attempts as CanonicalObject[]).find((item) => item.attempt_id === work.active_attempt_id)!;
  return applied(action(state, "submit_contribution", {
    artifact: artifact(contribution_id), completes_work: true, contribution_id, expected_attempt_revision: attempt.revision as number,
    expected_work_revision: work.revision as number, resource_basis: [], source_refs: ["fixture:source"], summary: `Contribution ${contribution_id}.`, work_id,
  }, actor));
}

function candidate(state: CanonicalObject, work_id: string, id = "candidate-1", actor = WORKER_A): CanonicalObject {
  const work = (state.work_items as CanonicalObject[]).find((item) => item.work_id === work_id)!;
  const contribution = (state.contributions as CanonicalObject[]).find((item) => item.work_id === work_id)!;
  return applied(action(state, "submit_candidate", {
    artifact: artifact(id), candidate_id: id, contribution_refs: [{ contribution_id: contribution.contribution_id as string, version: contribution.version as number }],
    expected_work_revision: work.revision as number, resource_basis: [], work_id,
  }, actor));
}

test("0.2.0 initializes exact setup, schedules supervision only after Human confirmation, and keeps public projection private", () => {
  const genesis = initial();
  assert.equal(pack.descriptor.version, "0.2.0");
  assert.equal(genesis.setup_revision, 2);
  assert.equal(genesis.phase, "awaiting_human_confirmation");
  const result = action(genesis, "confirm_initial_setup", { setup_revision: 2 }, HUMAN);
  const state = applied(result);
  assert.equal(state.phase, "open");
  if (result.activity_disposition_type === "apply") assert.equal((result.timer_requests[0] as CanonicalObject).due, "2026-09-15T12:05:00Z");
  const publicView = pack.view({ activity_state: state, core: core(), viewer: { member_id: "public", viewer_type: "public" } });
  assert.equal(publicView.projection.phase, "open");
  assert.equal(Object.hasOwn(publicView.projection, "goal"), false);
  assert.equal(Object.hasOwn(publicView.projection, "roster"), false);
});

test("initial roster configuration requires and records deliberate moving-alias acknowledgement", () => {
  const baseConfiguration = {
    goal: "Prepare a local supplied-material report.", constraints: ["Use only approved local inputs."],
    acceptance_criteria: ["The report cites each source."], approved_working_area: { root: "/tmp/swarm", resource_paths: ["inputs", "result"] },
    roster: [{ ...roster("worker-a"), requested_model: "latest" }, roster("worker-b")],
  };
  assert.throws(() => pack.initialize({ configuration: baseConfiguration, initial_core_state: core() }), /must explicitly acknowledge/u);
  const missingAcknowledgement = { ...roster("worker-a") };
  delete missingAcknowledgement.moving_alias_acknowledged;
  assert.throws(() => pack.initialize({
    configuration: { ...baseConfiguration, roster: [missingAcknowledgement, roster("worker-b")] },
    initial_core_state: core(),
  }), /moving_alias_acknowledged must be a boolean/u);
  const movingSelection = { ...roster("worker-a"), moving_alias_acknowledged: true, requested_model: "latest" };
  assert.equal(Object.hasOwn(movingSelection, "configuration_revision"), false);
  const state = pack.initialize({
    configuration: { ...baseConfiguration, roster: [movingSelection, roster("worker-b")] },
    initial_core_state: core(),
  }).initial_activity_state as CanonicalObject;
  const bound = (state.roster as CanonicalObject[]).find((item) => item.member_key === "worker-a")!;
  assert.equal(bound.configuration_revision, 1);
  assert.equal(bound.moving_alias_acknowledged, true);
  assert.equal(bound.requested_model, "latest");
});

test("the configured progress review interval controls initial and recurring supervision schedules", () => {
  const confirmed = action(
    initial(37, 5),
    "confirm_initial_setup",
    { setup_revision: 2 },
    HUMAN,
    {},
    "2026-09-15T12:34:56Z",
  );
  let state = applied(confirmed);
  assert.equal(state.progress_review_interval_seconds, 37);
  assert.equal(state.correction_failure_limit, 5);
  const participant = pack.view({ activity_state: state, core: core(), viewer: { member_id: WORKER_A, viewer_type: "participant" } });
  assert.equal(participant.projection.progress_review_interval_seconds, 37);
  assert.equal(participant.projection.correction_failure_limit, 5);
  if (confirmed.activity_disposition_type === "apply") {
    assert.deepEqual(confirmed.timer_requests, [{
      canonical_payload: { execution_epoch: 1, timer: "progress_review" },
      due: "2026-09-15T12:35:33Z",
      timer_id: PROGRESS_REVIEW_TIMER_ID,
      timer_request_type: "schedule_next",
    }]);
  }

  state = applied(timer(state, "2026-09-15T12:35:33Z"));
  state = claim(state, "progress-work-1-1", WORKER_A);
  const review = (state.outstanding_progress_reviews as CanonicalObject[])[0]!;
  const reported = action(state, "report_progress_review", {
    assessment: "aligned", corrective_work_ids: [], evidence_refs: ["fixture:progress"],
    expected_review_revision: review.revision as number, review_id: review.review_id as string,
    summary: "Work remains aligned.",
  }, WORKER_A, {}, "2026-09-15T13:00:05Z");
  applied(reported);
  if (reported.activity_disposition_type === "apply") {
    assert.deepEqual(reported.timer_requests, [{
      canonical_payload: { execution_epoch: 1, timer: "progress_review" },
      due: "2026-09-15T13:00:42Z",
      timer_id: PROGRESS_REVIEW_TIMER_ID,
      timer_request_type: "schedule_next",
    }]);
  }
});

test("independent Work Items retain exclusive owners and integration waits for every dependency", () => {
  let state = propose(confirm(), "source-a");
  state = propose(state, "source-b");
  state = propose(state, "integrate", ["source-a", "source-b"], "integration");
  const humanClaim = action(state, "claim_work_item", { attempt_id: "attempt-human", expected_work_revision: 1, work_id: "source-a" }, HUMAN);
  assert.equal(humanClaim.activity_disposition_type, "reject");
  assert.equal(humanClaim.activity_disposition_type === "reject" ? humanClaim.declared_code : "", "role_violation");
  const early = action(state, "claim_work_item", { attempt_id: "attempt-early", expected_work_revision: 1, work_id: "integrate" }, WORKER_B);
  assert.equal(early.activity_disposition_type, "reject");
  state = claim(state, "source-a");
  const race = action(state, "claim_work_item", { attempt_id: "attempt-race", expected_work_revision: 1, work_id: "source-a" }, WORKER_B);
  assert.equal(race.activity_disposition_type, "reject");
  assert.equal(race.activity_disposition_type === "reject" ? race.declared_code : "", "stale_work_revision");
  const ownedWork = (state.work_items as CanonicalObject[]).find((item) => item.work_id === "source-a")!;
  const currentRevisionRace = action(state, "claim_work_item", {
    attempt_id: "attempt-current-race", expected_work_revision: ownedWork.revision as number, work_id: "source-a",
  }, WORKER_B);
  assert.equal(currentRevisionRace.activity_disposition_type, "reject");
  assert.equal(currentRevisionRace.activity_disposition_type === "reject" ? currentRevisionRace.declared_code : "", "work_ineligible");
  state = claim(state, "source-b", WORKER_B);
  assert.deepEqual((state.work_items as CanonicalObject[]).filter((item) => item.status === "claimed")
    .map((item) => [item.work_id, item.owner_member_id, item.active_attempt_id]), [
    ["source-a", WORKER_A, "attempt-source-a"], ["source-b", WORKER_B, "attempt-source-b"],
  ]);
  assert.equal((state.work_attempts as CanonicalObject[]).length, 2);
  state = contribute(state, "source-a", "contribution-a");
  const partial = action(state, "claim_work_item", { attempt_id: "attempt-partial", expected_work_revision: 1, work_id: "integrate" }, WORKER_A);
  assert.equal(partial.activity_disposition_type, "reject");
  assert.equal(partial.activity_disposition_type === "reject" ? partial.declared_code : "", "work_ineligible");
  state = contribute(state, "source-b", "contribution-b", WORKER_B);
  state = claim(state, "integrate", WORKER_B);
  assert.equal(((state.work_items as CanonicalObject[]).find((item) => item.work_id === "integrate")!).owner_member_id, WORKER_B);
});

test("a completed integration Work Item still offers Candidate submission", () => {
  let state = claim(propose(confirm(), "integrate", [], "integration"), "integrate");
  state = contribute(state, "integrate", "integration-contribution");
  const workerView = pack.view({ activity_state: state, core: core(), viewer: { member_id: WORKER_A, viewer_type: "participant" } });
  const offeredActions = workerView.action_offers.map((offer) => typeof offer === "string" ? offer : offer.actionType);
  assert.equal(offeredActions.includes("submit_candidate"), true);
  assert.equal(offeredActions.includes("submit_contribution"), false);
  state = candidate(state, "integrate");
  assert.equal((state.candidates as CanonicalObject[])[0]!.work_id, "integrate");
});

test("reconciled handoff fences the old attempt and retains the receiving roster configuration", () => {
  let state = claim(propose(confirm(), "handoff-work", [], "integration"), "handoff-work", WORKER_A);
  let work = (state.work_items as CanonicalObject[]).find((item) => item.work_id === "handoff-work")!;
  let attempt = (state.work_attempts as CanonicalObject[]).find((item) => item.attempt_id === work.active_attempt_id)!;
  state = applied(action(state, "submit_contribution", {
    artifact: artifact("handoff-draft"), completes_work: false, contribution_id: "handoff-draft",
    expected_attempt_revision: attempt.revision as number, expected_work_revision: work.revision as number,
    resource_basis: [], source_refs: ["fixture:handoff"], summary: "Draft before recovery.", work_id: "handoff-work",
  }, WORKER_A));
  state = candidate(state, "handoff-work", "handoff-candidate", WORKER_A);
  state = applied(action(state, "request_writeback", {
    candidate: { candidate_id: "handoff-candidate", version: 1 }, expected_resource_version: 0,
    operation_id: "handoff-writeback", resource_id: "handoff-resource",
  }, WORKER_A));
  state = applied(action(state, "record_writeback_outcome", {
    evidence_refs: ["fixture:lost-reply"], expected_writeback_revision: 1, operation_id: "handoff-writeback", status: "uncertain",
  }, WORKER_A));
  const workerView = pack.view({ activity_state: state, core: core(), viewer: { member_id: WORKER_A, viewer_type: "participant" } });
  assert.equal(workerView.action_offers.some((offer) => typeof offer !== "string" && offer.actionType === "record_writeback_outcome"), false);
  const uncertain = (state.blockers as CanonicalObject[]).find((item) => item.blocker_id === "writeback-uncertain:handoff-writeback")!;
  const selfReconcile = action(state, "resolve_work_blocker", {
    blocker_id: uncertain.blocker_id as string, evidence_refs: ["fixture:self-assertion"], expected_blocker_revision: uncertain.revision as number,
  }, WORKER_A);
  assert.equal(selfReconcile.activity_disposition_type, "reject");
  assert.equal(selfReconcile.activity_disposition_type === "reject" ? selfReconcile.declared_code : "", "role_violation");

  work = (state.work_items as CanonicalObject[]).find((item) => item.work_id === "handoff-work")!;
  state = applied(action(state, "report_work_blocker", {
    blocker_id: "handoff-stop", evidence_refs: ["fixture:stopped"], expected_work_revision: work.revision as number,
    summary: "The old invocation stopped.", work_id: "handoff-work",
  }, WORKER_A));
  const stop = (state.blockers as CanonicalObject[]).find((item) => item.blocker_id === "handoff-stop")!;
  state = applied(action(state, "resolve_work_blocker", {
    blocker_id: "handoff-stop", evidence_refs: ["fixture:terminal"], expected_blocker_revision: stop.revision as number,
  }, WORKER_A));
  work = (state.work_items as CanonicalObject[]).find((item) => item.work_id === "handoff-work")!;
  attempt = (state.work_attempts as CanonicalObject[]).find((item) => item.attempt_id === "attempt-handoff-work")!;
  const blockedHandoff = action(state, "handoff_work_item", {
    evidence_refs: ["fixture:terminal"], expected_attempt_revision: attempt.revision as number, expected_work_revision: work.revision as number,
    from_attempt_id: attempt.attempt_id as string, handoff_id: "handoff-1", reason: "Provider unavailable.",
    receiving_member_id: WORKER_B, reconciliation: "confirmed_terminal", to_attempt_id: "attempt-handoff-receiver", work_id: "handoff-work",
  }, WORKER_B);
  assert.equal(blockedHandoff.activity_disposition_type, "reject");

  state = applied(action(state, "resolve_work_blocker", {
    blocker_id: uncertain.blocker_id as string, evidence_refs: ["fixture:probed-no-effect"], expected_blocker_revision: uncertain.revision as number,
  }, HUMAN));
  const receiverBefore = (state.roster as CanonicalObject[]).find((item) => item.member_id === WORKER_B)!;
  work = (state.work_items as CanonicalObject[]).find((item) => item.work_id === "handoff-work")!;
  attempt = (state.work_attempts as CanonicalObject[]).find((item) => item.attempt_id === "attempt-handoff-work")!;
  const handedOff = action(state, "handoff_work_item", {
    evidence_refs: ["fixture:terminal", "fixture:probed-no-effect"], expected_attempt_revision: attempt.revision as number,
    expected_work_revision: work.revision as number, from_attempt_id: attempt.attempt_id as string, handoff_id: "handoff-1",
    reason: "Continue with the existing receiving Worker settings.", receiving_member_id: WORKER_B,
    reconciliation: "effects_reconciled", to_attempt_id: "attempt-handoff-receiver", work_id: "handoff-work",
  }, WORKER_B);
  state = applied(handedOff);
  const receiverAfter = (state.roster as CanonicalObject[]).find((item) => item.member_id === WORKER_B)!;
  assert.deepEqual(receiverAfter, receiverBefore);
  assert.equal((state.work_items as CanonicalObject[]).find((item) => item.work_id === "handoff-work")!.owner_member_id, WORKER_B);
  assert.equal((state.work_attempts as CanonicalObject[]).find((item) => item.attempt_id === "attempt-handoff-work")!.status, "fenced");
  assert.equal((state.handoffs as CanonicalObject[])[0]!.authorized_by_member_id, WORKER_B);
  if (handedOff.activity_disposition_type === "apply") {
    assert.equal((handedOff.ordered_domain_events[0] as CanonicalObject).event_type, "work_item_handed_off");
  }
});

test("participant Actions fail closed unless they bind the current exact Room Head", () => {
  const state = confirm();
  const roomSeq = state.room_seq as number;
  const stale = reduceSwarm({
    core_before: core(), proposed_core_after: core(), prior_activity_state: state, next_room_seq: roomSeq + 1, scheduled_timers: {},
    recorded_stimulus: {
      action_id: "action-stale-head", action_type: "propose_work_item", admitted_at: "2026-09-15T12:00:00Z",
      canonical_payload: { dependency_ids: [], description: "Stale proposal.", expected_goal_revision: 1, kind: "goal", title: "stale", work_id: "stale" },
      exact_basis_head: { room_seq: roomSeq - 1 }, member_id: WORKER_A,
      payload_schema_digest: `blake3:${"0".repeat(64)}`, stimulus_type: "participant_action",
    },
  });
  assert.equal(stale.activity_disposition_type, "reject");
  assert.equal(stale.activity_disposition_type === "reject" ? stale.declared_code : "", "stale_room_head");
});

test("candidate acceptance binds exact passing checks, independent review, and unresolved Findings", () => {
  let state = contribute(claim(propose(confirm(), "report", [], "integration"), "report"), "report", "contribution-report");
  state = candidate(state, "report");
  state = applied(action(state, "record_check", { candidate: { candidate_id: "candidate-1", version: 1 }, check_id: "criterion-1", evidence_refs: [checkEvidence("candidate-1", 1, "criterion-1")], expected_criteria_revision: 1, resource_basis: [], status: "failed" }, WORKER_B));
  const selfReview = action(state, "record_review", { candidate: { candidate_id: "candidate-1", version: 1 }, expected_criteria_revision: 1, findings: [], resource_basis: [], review_id: "self", verdict: "passed" }, WORKER_A);
  assert.equal(selfReview.activity_disposition_type, "reject");
  state = applied(action(state, "record_review", { candidate: { candidate_id: "candidate-1", version: 1 }, expected_criteria_revision: 1, findings: [{ finding_id: "finding-1", severity: "blocking", summary: "Citation missing." }], resource_basis: [], review_id: "review-blocking", verdict: "changes_requested" }, WORKER_B));
  state = applied(action(state, "record_review", { candidate: { candidate_id: "candidate-1", version: 1 }, expected_criteria_revision: 1, findings: [], resource_basis: [], review_id: "review-pass", verdict: "passed" }, WORKER_B));
  state = candidate(state, "report");
  state = applied(action(state, "record_check", { candidate: { candidate_id: "candidate-1", version: 2 }, check_id: "criterion-1", evidence_refs: [checkEvidence("candidate-1", 2, "criterion-1")], expected_criteria_revision: 1, resource_basis: [], status: "passed" }, WORKER_B));
  state = applied(action(state, "record_review", { candidate: { candidate_id: "candidate-1", version: 2 }, expected_criteria_revision: 1, findings: [], resource_basis: [], review_id: "review-pass-v2", verdict: "passed" }, WORKER_B));
  const blocked = action(state, "accept_result", { candidate: { candidate_id: "candidate-1", version: 2 }, check_refs: [{ id: "criterion-1", revision: 2 }], expected_direction_revision: 0, result_id: "result", review_refs: [{ id: "review-pass-v2", revision: 1 }] }, HUMAN);
  assert.equal(blocked.activity_disposition_type, "reject");
  assert.equal(blocked.activity_disposition_type === "reject" ? blocked.declared_code : "", "blocking_finding");
  const authorResolution = action(state, "resolve_review_finding", { evidence_refs: ["fixture:corrected"], expected_finding_revision: 1, finding_id: "finding-1", resolution: "resolved" }, WORKER_A);
  assert.equal(authorResolution.activity_disposition_type, "reject");
  state = applied(action(state, "resolve_review_finding", { evidence_refs: ["fixture:corrected"], expected_finding_revision: 1, finding_id: "finding-1", resolution: "resolved" }, HUMAN));
  const accepted = action(state, "accept_result", { candidate: { candidate_id: "candidate-1", version: 2 }, check_refs: [{ id: "criterion-1", revision: 2 }], expected_direction_revision: 0, result_id: "result", review_refs: [{ id: "review-pass-v2", revision: 1 }] }, HUMAN);
  state = applied(accepted);
  assert.equal(state.phase, "completed");
  assert.equal((state.results as CanonicalObject[]).length, 1);
});

test("criteria define the complete immutable Check policy and exact evidence binding", () => {
  let state = contribute(
    claim(propose(confirm(initial(300, 3, ["Cites every source.", "Contains no unsupported claim."])), "policy-integration", [], "integration"), "policy-integration"),
    "policy-integration", "contribution-policy",
  );
  state = candidate(state, "policy-integration", "candidate-policy");
  const arbitrary = action(state, "record_check", {
    candidate: { candidate_id: "candidate-policy", version: 1 }, check_id: "caller-invented", evidence_refs: [checkEvidence("candidate-policy", 1, "caller-invented")],
    expected_criteria_revision: 1, resource_basis: [], status: "passed",
  }, WORKER_B);
  assert.equal(arbitrary.activity_disposition_type, "reject");
  assert.equal(arbitrary.activity_disposition_type === "reject" ? arbitrary.declared_code : "", "invalid_check");
  const missingEvidence = action(state, "record_check", {
    candidate: { candidate_id: "candidate-policy", version: 1 }, check_id: "criterion-1", evidence_refs: [],
    expected_criteria_revision: 1, resource_basis: [], status: "passed",
  }, WORKER_B);
  assert.equal(missingEvidence.activity_disposition_type, "reject");
  assert.equal(missingEvidence.activity_disposition_type === "reject" ? missingEvidence.declared_code : "", "missing_evidence");
  const selfReport = action(state, "record_check", {
    candidate: { candidate_id: "candidate-policy", version: 1 }, check_id: "criterion-1", evidence_refs: ["exit:0"],
    expected_criteria_revision: 1, resource_basis: [], status: "passed",
  }, WORKER_B);
  assert.equal(selfReport.activity_disposition_type, "reject");
  assert.equal(selfReport.activity_disposition_type === "reject" ? selfReport.declared_code : "", "invalid_check_evidence");
  const wrongCandidateEvidence = action(state, "record_check", {
    candidate: { candidate_id: "candidate-policy", version: 1 }, check_id: "criterion-1",
    evidence_refs: [checkEvidence("some-other-candidate", 1, "criterion-1", [], "Cites every source.")], expected_criteria_revision: 1, resource_basis: [], status: "passed",
  }, WORKER_B);
  assert.equal(wrongCandidateEvidence.activity_disposition_type, "reject");
  assert.equal(wrongCandidateEvidence.activity_disposition_type === "reject" ? wrongCandidateEvidence.declared_code : "", "invalid_check_evidence");
  const wrongCriterionEvidence = action(state, "record_check", {
    candidate: { candidate_id: "candidate-policy", version: 1 }, check_id: "criterion-1",
    evidence_refs: [checkEvidence("candidate-policy", 1, "criterion-1", [], "A caller-authored substitute criterion.")], expected_criteria_revision: 1, resource_basis: [], status: "passed",
  }, WORKER_B);
  assert.equal(wrongCriterionEvidence.activity_disposition_type, "reject");
  assert.equal(wrongCriterionEvidence.activity_disposition_type === "reject" ? wrongCriterionEvidence.declared_code : "", "invalid_check_evidence");
  const correctArtifactEvidence = checkEvidence("candidate-policy", 1, "criterion-1", [], "Cites every source.");
  const wrongArtifact: CanonicalObject = {
    ...correctArtifactEvidence,
    candidate_artifact: artifact("some-other-artifact"),
  };
  const wrongCandidateArtifactEvidence = action(state, "record_check", {
    candidate: { candidate_id: "candidate-policy", version: 1 }, check_id: "criterion-1",
    evidence_refs: [wrongArtifact], expected_criteria_revision: 1, resource_basis: [], status: "passed",
  }, WORKER_B);
  assert.equal(wrongCandidateArtifactEvidence.activity_disposition_type, "reject");
  assert.equal(wrongCandidateArtifactEvidence.activity_disposition_type === "reject" ? wrongCandidateArtifactEvidence.declared_code : "", "invalid_check_evidence");
  const firstPayload = {
    candidate: { candidate_id: "candidate-policy", version: 1 }, check_id: "criterion-1",
    evidence_refs: [checkEvidence("candidate-policy", 1, "criterion-1", [], "Cites every source.")], expected_criteria_revision: 1, resource_basis: [], status: "passed",
  };
  state = applied(action(state, "record_check", firstPayload, WORKER_B));
  const duplicate = action(state, "record_check", firstPayload, WORKER_B);
  assert.equal(duplicate.activity_disposition_type, "reject");
  assert.equal(duplicate.activity_disposition_type === "reject" ? duplicate.declared_code : "", "duplicate_check");
  state = applied(action(state, "record_check", {
    candidate: { candidate_id: "candidate-policy", version: 1 }, check_id: "criterion-2",
    evidence_refs: [checkEvidence("candidate-policy", 1, "criterion-2", [], "Contains no unsupported claim.")], expected_criteria_revision: 1, resource_basis: [], status: "passed",
  }, WORKER_B));
  assert.deepEqual((state.checks as CanonicalObject[]).map((check) => [check.check_id, check.criterion]), [
    ["criterion-1", "Cites every source."],
    ["criterion-2", "Contains no unsupported claim."],
  ]);
  state = applied(action(state, "record_review", {
    candidate: { candidate_id: "candidate-policy", version: 1 }, expected_criteria_revision: 1, findings: [],
    resource_basis: [], review_id: "review-policy", verdict: "passed",
  }, WORKER_B));
  const subset = action(state, "accept_result", {
    candidate: { candidate_id: "candidate-policy", version: 1 }, check_refs: [{ id: "criterion-1", revision: 1 }],
    expected_direction_revision: 0, result_id: "result-policy", review_refs: [{ id: "review-policy", revision: 1 }],
  }, HUMAN);
  assert.equal(subset.activity_disposition_type, "reject");
  assert.equal(subset.activity_disposition_type === "reject" ? subset.declared_code : "", "validation_incomplete");
  const reordered = action(state, "accept_result", {
    candidate: { candidate_id: "candidate-policy", version: 1 },
    check_refs: [{ id: "criterion-2", revision: 1 }, { id: "criterion-1", revision: 1 }],
    expected_direction_revision: 0, result_id: "result-policy", review_refs: [{ id: "review-policy", revision: 1 }],
  }, HUMAN);
  assert.equal(reordered.activity_disposition_type, "reject");
  state = applied(action(state, "accept_result", {
    candidate: { candidate_id: "candidate-policy", version: 1 },
    check_refs: [{ id: "criterion-1", revision: 1 }, { id: "criterion-2", revision: 1 }],
    expected_direction_revision: 0, result_id: "result-policy", review_refs: [{ id: "review-policy", revision: 1 }],
  }, HUMAN));
  assert.equal(state.phase, "completed");
});

test("only an integration Work Item can produce the accepted Candidate", () => {
  let state = contribute(claim(propose(confirm(), "draft"), "draft"), "draft", "contribution-draft");
  state = candidate(state, "draft");
  state = applied(action(state, "record_check", { candidate: { candidate_id: "candidate-1", version: 1 }, check_id: "criterion-1", evidence_refs: [checkEvidence("candidate-1", 1, "criterion-1")], expected_criteria_revision: 1, resource_basis: [], status: "passed" }, WORKER_B));
  state = applied(action(state, "record_review", { candidate: { candidate_id: "candidate-1", version: 1 }, expected_criteria_revision: 1, findings: [], resource_basis: [], review_id: "review", verdict: "passed" }, WORKER_B));
  const accepted = action(state, "accept_result", { candidate: { candidate_id: "candidate-1", version: 1 }, check_refs: [{ id: "criterion-1", revision: 1 }], expected_direction_revision: 0, result_id: "result", review_refs: [{ id: "review", revision: 1 }] }, HUMAN);
  assert.equal(accepted.activity_disposition_type, "reject");
  assert.equal(accepted.activity_disposition_type === "reject" ? accepted.declared_code : "", "validation_incomplete");
});

test("Suggestions are advisory while Human Directions interrupt only the dependent closure", () => {
  let state = propose(confirm(), "branch-a");
  state = propose(state, "dependent", ["branch-a"]);
  state = propose(state, "independent");
  state = claim(state, "branch-a");
  state = applied(action(state, "submit_suggestion", { suggestion_id: "suggestion-1", summary: "Prefer a shorter report.", target_scope: "work", target_work_ids: ["branch-a"] }, WORKER_B));
  assert.equal(((state.work_items as CanonicalObject[]).find((item) => item.work_id === "branch-a")!).status, "claimed");
  state = applied(action(state, "issue_direction", { direction_id: "direction-1", expected_direction_revision: 0, instruction: "Revalidate against the new priority.", target_scope: "work", target_work_ids: ["branch-a"] }, HUMAN));
  const work = state.work_items as CanonicalObject[];
  assert.equal(work.find((item) => item.work_id === "branch-a")!.status, "blocked");
  assert.equal(work.find((item) => item.work_id === "dependent")!.status, "blocked");
  assert.equal(work.find((item) => item.work_id === "independent")!.status, "open");
  assert.equal(((state.work_attempts as CanonicalObject[])[0]!).status, "interruption_requested");
});

test("only the Human coordinator can clear a Human Direction blocker", () => {
  let state = propose(confirm(), "directed");
  state = applied(action(state, "issue_direction", {
    direction_id: "direction-authority", expected_direction_revision: 0, instruction: "Revalidate this branch.",
    target_scope: "work", target_work_ids: ["directed"],
  }, HUMAN));
  const blocker = (state.blockers as CanonicalObject[]).find((item) => item.blocker_id === "direction:direction-authority")!;
  const workerAttempt = action(state, "resolve_work_blocker", {
    blocker_id: blocker.blocker_id as string, evidence_refs: ["fixture:worker-claim"], expected_blocker_revision: blocker.revision as number,
  }, WORKER_A);
  assert.equal(workerAttempt.activity_disposition_type, "reject");
  assert.equal(workerAttempt.activity_disposition_type === "reject" ? workerAttempt.declared_code : "", "role_violation");
  const workerView = pack.view({ activity_state: state, core: core(), viewer: { member_id: WORKER_A, viewer_type: "participant" } });
  assert.equal(workerView.action_offers.some((offer) => typeof offer !== "string" && offer.actionType === "resolve_work_blocker"), false);
  state = applied(action(state, "resolve_work_blocker", {
    blocker_id: blocker.blocker_id as string, evidence_refs: ["fixture:human-approved"], expected_blocker_revision: blocker.revision as number,
  }, HUMAN));
  assert.equal((state.blockers as CanonicalObject[]).find((item) => item.blocker_id === blocker.blocker_id)!.status, "resolved");
});

test("progress Timer creates one goal-scoped canonical obligation and report reschedules it", () => {
  let state = confirm();
  const due = timer(state);
  state = applied(due);
  if (due.activity_disposition_type === "apply") {
    assert.equal((due.ordered_domain_events[0] as CanonicalObject).event_type, "progress_review_due");
    assert.equal(due.ordered_attention_signals.length, 1);
  }
  assert.equal((state.work_items as CanonicalObject[]).length, 1);
  assert.equal((state.outstanding_progress_reviews as CanonicalObject[]).length, 1);
  const duplicate = timer(state);
  const afterDuplicate = applied(duplicate);
  assert.equal((afterDuplicate.work_items as CanonicalObject[]).length, 1);
  assert.equal((afterDuplicate.outstanding_progress_reviews as CanonicalObject[]).length, 1);
  if (duplicate.activity_disposition_type === "apply") {
    assert.equal((duplicate.ordered_domain_events[0] as CanonicalObject).event_type, "progress_review_timer_suppressed");
    assert.deepEqual(duplicate.ordered_attention_signals, []);
    assert.deepEqual(duplicate.timer_requests, []);
  }
  state = claim(state, "progress-work-1-1", WORKER_A);
  const claimedWork = (state.work_items as CanonicalObject[]).find((item) => item.work_id === "progress-work-1-1")!;
  const competingClaim = action(state, "claim_work_item", {
    attempt_id: "attempt-progress-competitor",
    expected_work_revision: claimedWork.revision as number,
    work_id: claimedWork.work_id as string,
  }, WORKER_B);
  assert.equal(competingClaim.activity_disposition_type, "reject");
  assert.equal(competingClaim.activity_disposition_type === "reject" ? competingClaim.declared_code : "", "work_ineligible");
  assert.equal(claimedWork.owner_member_id, WORKER_A);
  const review = (state.outstanding_progress_reviews as CanonicalObject[])[0]!;
  const reported = action(state, "report_progress_review", { assessment: "aligned", corrective_work_ids: [], evidence_refs: ["fixture:progress"], expected_review_revision: review.revision as number, review_id: review.review_id as string, summary: "Work remains aligned." }, WORKER_A);
  state = applied(reported);
  assert.deepEqual(state.outstanding_progress_reviews, []);
  if (reported.activity_disposition_type === "apply") assert.equal((reported.timer_requests[0] as CanonicalObject).timer_request_type, "schedule_next");
});

test("work blockers allow independent completion and the interval Timer retains one obligation per scope", () => {
  let state = claim(propose(confirm(), "blocked-work"), "blocked-work", WORKER_A);
  state = claim(propose(state, "independent"), "independent", WORKER_B);
  const work = (state.work_items as CanonicalObject[]).find((item) => item.work_id === "blocked-work")!;
  state = applied(action(state, "report_work_blocker", {
    blocker_id: "blocker-scope", evidence_refs: [], expected_work_revision: work.revision as number,
    summary: "Awaiting source evidence.", work_id: "blocked-work",
  }, WORKER_A));
  state = contribute(state, "independent", "independent-contribution", WORKER_B);
  assert.equal((state.work_items as CanonicalObject[]).find((item) => item.work_id === "blocked-work")!.status, "blocked");
  assert.equal((state.work_items as CanonicalObject[]).find((item) => item.work_id === "independent")!.status, "completed");
  assert.deepEqual((state.outstanding_progress_reviews as CanonicalObject[]).map((item) => item.scope), ["work"]);
  state = applied(timer(state));
  assert.deepEqual((state.outstanding_progress_reviews as CanonicalObject[]).map((item) => item.scope).sort(), ["goal", "work"]);
  const duplicate = applied(timer(state));
  assert.equal((duplicate.outstanding_progress_reviews as CanonicalObject[]).length, 2);
  const workReview = (state.outstanding_progress_reviews as CanonicalObject[]).find((item) => item.scope === "work")!;
  state = claim(state, workReview.work_id as string, WORKER_B);
  const reviewWork = (state.work_items as CanonicalObject[]).find((item) => item.work_id === workReview.work_id)!;
  state = applied(action(state, "report_work_blocker", {
    blocker_id: "review-blocker", evidence_refs: [], expected_work_revision: reviewWork.revision as number,
    summary: "Review capacity is blocked.", work_id: reviewWork.work_id as string,
  }, WORKER_B));
  assert.equal((state.outstanding_progress_reviews as CanonicalObject[]).find((item) => item.review_id === workReview.review_id)!.status, "blocked");
});

test("newer resources create scoped conflicts while independent work remains eligible", () => {
  let state = propose(confirm(), "affected");
  state = propose(state, "independent");
  state = applied(action(state, "record_resource_version", { affected_work_ids: ["affected"], digest: "digest-v1", expected_previous_version: 0, local_path: "/tmp/swarm/input", resource_id: "input", version: 1 }, HUMAN));
  state = applied(action(state, "record_resource_version", { affected_work_ids: ["affected"], digest: "digest-v2", expected_previous_version: 0, local_path: "/tmp/swarm/input", resource_id: "input", version: 2 }, HUMAN));
  assert.equal((state.resource_conflicts as CanonicalObject[]).length, 1);
  const affected = action(state, "claim_work_item", { attempt_id: "attempt-affected", expected_work_revision: 1, work_id: "affected" }, WORKER_A);
  assert.equal(affected.activity_disposition_type, "reject");
  state = claim(state, "independent", WORKER_B);
  assert.equal(((state.work_items as CanonicalObject[]).find((item) => item.work_id === "independent")!).owner_member_id, WORKER_B);
});

test("resource metadata cannot escape the approved working area", () => {
  const rejected = action(confirm(), "record_resource_version", {
    affected_work_ids: [], digest: "digest-outside", expected_previous_version: 0,
    local_path: "/tmp/swarm/../outside", resource_id: "outside", version: 1,
  }, HUMAN);
  assert.equal(rejected.activity_disposition_type, "reject");
  assert.equal(rejected.activity_disposition_type === "reject" ? rejected.declared_code : "", "artifact_outside_working_area");
});

test("three evidence-free correction failures escalate only the affected dependency branch", () => {
  let state = propose(confirm(), "stalled");
  state = propose(state, "downstream", ["stalled"]);
  state = propose(state, "independent");
  state = claim(state, "stalled", WORKER_A);
  const stalled = (state.work_items as CanonicalObject[]).find((item) => item.work_id === "stalled")!;
  state = applied(action(state, "report_work_blocker", { blocker_id: "blocker-stalled", evidence_refs: [], expected_work_revision: stalled.revision as number, summary: "No useful evidence yet.", work_id: "stalled" }, WORKER_A));
  for (const id of ["correction-1", "correction-2", "correction-3"]) state = propose(state, id, [], "correction");
  const progress = (state.outstanding_progress_reviews as CanonicalObject[]).find((item) => item.scope === "work")!;
  state = claim(state, progress.work_id as string, WORKER_B);
  const claimedProgress = (state.outstanding_progress_reviews as CanonicalObject[]).find((item) => item.review_id === progress.review_id)!;
  state = applied(action(state, "report_progress_review", {
    assessment: "realignment_required", corrective_work_ids: ["correction-1", "correction-2", "correction-3"], evidence_refs: [],
    expected_review_revision: claimedProgress.revision as number, problem_id: "problem-stalled", problem_scope: "work", review_id: claimedProgress.review_id as string,
    summary: "The stalled branch needs correction.",
  }, WORKER_B));
  for (const id of ["correction-1", "correction-2", "correction-3"]) {
    state = contribute(claim(state, id, WORKER_B), id, `contribution-${id}`, WORKER_B);
    const problem = (state.problems as CanonicalObject[]).find((item) => item.problem_id === "problem-stalled")!;
    state = applied(action(state, "record_correction_outcome", { evidence_refs: [], expected_problem_revision: problem.revision as number, outcome: "unsuccessful", problem_id: "problem-stalled", work_id: id }, WORKER_B));
  }
  const problem = (state.problems as CanonicalObject[]).find((item) => item.problem_id === "problem-stalled")!;
  assert.equal(problem.status, "escalated");
  assert.equal(problem.failed_corrections, 3);
  const work = state.work_items as CanonicalObject[];
  assert.equal(work.find((item) => item.work_id === "stalled")!.owner_member_id, null);
  assert.equal(work.find((item) => item.work_id === "stalled")!.active_attempt_id, null);
  assert.equal(work.find((item) => item.work_id === "downstream")!.status, "blocked");
  assert.equal(work.find((item) => item.work_id === "independent")!.status, "open");
  assert.equal((state.work_attempts as CanonicalObject[]).find((item) => item.attempt_id === "attempt-stalled")!.status, "interruption_requested");
});

test("completed Swarms preserve late output and reopen only by explicit Human action", () => {
  let state = contribute(claim(propose(confirm(), "report", [], "integration"), "report"), "report", "contribution-report");
  state = candidate(state, "report");
  state = applied(action(state, "record_check", { candidate: { candidate_id: "candidate-1", version: 1 }, check_id: "criterion-1", evidence_refs: [checkEvidence("candidate-1", 1, "criterion-1")], expected_criteria_revision: 1, resource_basis: [], status: "passed" }, WORKER_B));
  state = applied(action(state, "record_review", { candidate: { candidate_id: "candidate-1", version: 1 }, expected_criteria_revision: 1, findings: [], resource_basis: [], review_id: "review", verdict: "passed" }, WORKER_B));
  const scheduledTimer = {
    [PROGRESS_REVIEW_TIMER_ID]: { generation: 4 },
  };
  const accepted = action(state, "accept_result", { candidate: { candidate_id: "candidate-1", version: 1 }, check_refs: [{ id: "criterion-1", revision: 1 }], expected_direction_revision: 0, result_id: "result", review_refs: [{ id: "review", revision: 1 }] }, HUMAN, scheduledTimer);
  state = applied(accepted);
  if (accepted.activity_disposition_type === "apply") {
    assert.deepEqual(accepted.timer_requests, [{
      expected_generation: 4,
      timer_id: PROGRESS_REVIEW_TIMER_ID,
      timer_request_type: "cancel_current",
    }]);
  }
  const completedWorkCount = (state.work_items as CanonicalObject[]).length;
  const completedReviewSequence = state.progress_review_sequence;
  const lateTimer = timer(state);
  state = applied(lateTimer);
  assert.equal(state.phase, "completed");
  assert.equal((state.work_items as CanonicalObject[]).length, completedWorkCount);
  assert.equal(state.progress_review_sequence, completedReviewSequence);
  assert.deepEqual(state.outstanding_progress_reviews, []);
  if (lateTimer.activity_disposition_type === "apply") {
    assert.equal((lateTimer.ordered_domain_events[0] as CanonicalObject).event_type, "progress_review_timer_suppressed");
    assert.deepEqual(lateTimer.ordered_attention_signals, []);
    assert.deepEqual(lateTimer.timer_requests, []);
  }
  const work = (state.work_items as CanonicalObject[]).find((item) => item.work_id === "report")!;
  state = applied(action(state, "record_late_contribution", { artifact: artifact("late"), attempt_id: "attempt-report", expected_work_revision: work.revision as number, late_id: "late-1", summary: "Arrived after acceptance.", work_id: "report" }, WORKER_A));
  assert.equal((state.results as CanonicalObject[]).length, 1);
  assert.equal((state.late_contributions as CanonicalObject[]).length, 1);
  const workerReopen = action(state, "reopen_swarm", { expected_execution_epoch: 1, reason: "Changed input." }, WORKER_A);
  assert.equal(workerReopen.activity_disposition_type, "reject");
  state = applied(action(state, "reopen_swarm", { expected_execution_epoch: 1, reason: "Changed input." }, HUMAN));
  assert.equal(state.phase, "open");
  assert.equal(state.execution_epoch, 2);
  assert.equal((state.results as CanonicalObject[]).length, 1);
});

function completedCorrectionProblem(problemId: string, correctionId: string): CanonicalObject {
  let state = propose(confirm(), `stalled-${problemId}`);
  state = claim(state, `stalled-${problemId}`, WORKER_A);
  const stalled = (state.work_items as CanonicalObject[]).find((item) => item.work_id === `stalled-${problemId}`)!;
  state = applied(action(state, "report_work_blocker", {
    blocker_id: `blocker-${problemId}`, evidence_refs: [], expected_work_revision: stalled.revision as number,
    summary: "Progress has stalled.", work_id: stalled.work_id as string,
  }, WORKER_A));
  state = propose(state, correctionId, [], "correction");
  const progress = (state.outstanding_progress_reviews as CanonicalObject[]).find((item) => item.scope === "work")!;
  state = claim(state, progress.work_id as string, WORKER_B);
  const claimedProgress = (state.outstanding_progress_reviews as CanonicalObject[]).find((item) => item.review_id === progress.review_id)!;
  state = applied(action(state, "report_progress_review", {
    assessment: "realignment_required", corrective_work_ids: [correctionId], evidence_refs: [],
    expected_review_revision: claimedProgress.revision as number, problem_id: problemId, problem_scope: "work",
    review_id: claimedProgress.review_id as string, summary: "The stalled work needs an evidence-producing correction.",
  }, WORKER_B));
  return contribute(claim(state, correctionId, WORKER_B), correctionId, `contribution-${correctionId}`, WORKER_B);
}

test("Human-required facts remain durable without manufacturing an Agent activation for the Human coordinator", () => {
  const result = action(confirm(), "submit_suggestion", {
    suggestion_id: "suggestion-human-target", summary: "Please decide this tradeoff.", target_scope: "goal", target_work_ids: [],
  }, WORKER_A);
  assert.equal(result.activity_disposition_type, "apply");
  if (result.activity_disposition_type === "apply") {
    assert.deepEqual(result.ordered_attention_signals, []);
    const suggestion = (result.next_activity_state as CanonicalObject).suggestions as CanonicalObject[];
    assert.equal(suggestion[0]?.suggestion_id, "suggestion-human-target");
    assert.equal(suggestion[0]?.disposition, "pending");
  }
});

test("blocking Findings follow explicit integration Work lineage across fresh Candidate ids", () => {
  let state = contribute(claim(propose(confirm(), "lineage-integration", [], "integration"), "lineage-integration"), "lineage-integration", "contribution-lineage");
  state = candidate(state, "lineage-integration", "candidate-old");
  state = applied(action(state, "record_review", {
    candidate: { candidate_id: "candidate-old", version: 1 }, expected_criteria_revision: 1,
    findings: [{ finding_id: "finding-lineage", severity: "blocking", summary: "The integration omits required evidence." }],
    resource_basis: [], review_id: "review-lineage-blocked", verdict: "changes_requested",
  }, WORKER_B));
  state = candidate(state, "lineage-integration", "candidate-fresh-id");
  state = applied(action(state, "record_check", {
    candidate: { candidate_id: "candidate-fresh-id", version: 1 }, check_id: "criterion-1", evidence_refs: [checkEvidence("candidate-fresh-id", 1, "criterion-1")],
    expected_criteria_revision: 1, resource_basis: [], status: "passed",
  }, WORKER_B));
  state = applied(action(state, "record_review", {
    candidate: { candidate_id: "candidate-fresh-id", version: 1 }, expected_criteria_revision: 1, findings: [],
    resource_basis: [], review_id: "review-fresh-id", verdict: "passed",
  }, WORKER_B));
  const blocked = action(state, "accept_result", {
    candidate: { candidate_id: "candidate-fresh-id", version: 1 }, check_refs: [{ id: "criterion-1", revision: 1 }],
    expected_direction_revision: 0, result_id: "result-lineage", review_refs: [{ id: "review-fresh-id", revision: 1 }],
  }, HUMAN);
  assert.equal(blocked.activity_disposition_type, "reject");
  assert.equal(blocked.activity_disposition_type === "reject" ? blocked.declared_code : "", "blocking_finding");
  state = applied(action(state, "resolve_review_finding", {
    evidence_refs: ["fixture:human-resolution"], expected_finding_revision: 1, finding_id: "finding-lineage", resolution: "resolved",
  }, HUMAN));
  state = applied(action(state, "accept_result", {
    candidate: { candidate_id: "candidate-fresh-id", version: 1 }, check_refs: [{ id: "criterion-1", revision: 1 }],
    expected_direction_revision: 0, result_id: "result-lineage", review_refs: [{ id: "review-fresh-id", revision: 1 }],
  }, HUMAN));
  assert.equal(state.phase, "completed");
});

test("a shared source Contribution does not merge independent integration Work lineages", () => {
  let state = contribute(claim(propose(confirm(), "integration-a", [], "integration"), "integration-a"), "integration-a", "contribution-shared");
  state = candidate(state, "integration-a", "candidate-integration-a");
  state = applied(action(state, "record_review", {
    candidate: { candidate_id: "candidate-integration-a", version: 1 }, expected_criteria_revision: 1,
    findings: [{ finding_id: "finding-integration-a", severity: "blocking", summary: "Only integration A is blocked." }],
    resource_basis: [], review_id: "review-integration-a", verdict: "changes_requested",
  }, WORKER_B));
  state = contribute(claim(propose(state, "integration-b", [], "integration"), "integration-b"), "integration-b", "contribution-integration-b");
  const work = (state.work_items as CanonicalObject[]).find((item) => item.work_id === "integration-b")!;
  state = applied(action(state, "submit_candidate", {
    artifact: artifact("candidate-integration-b"), candidate_id: "candidate-integration-b",
    contribution_refs: [{ contribution_id: "contribution-shared", version: 1 }, { contribution_id: "contribution-integration-b", version: 1 }],
    expected_work_revision: work.revision as number, resource_basis: [], work_id: "integration-b",
  }, WORKER_A));
  state = applied(action(state, "record_check", {
    candidate: { candidate_id: "candidate-integration-b", version: 1 }, check_id: "criterion-1", evidence_refs: [checkEvidence("candidate-integration-b", 1, "criterion-1")],
    expected_criteria_revision: 1, resource_basis: [], status: "passed",
  }, WORKER_B));
  state = applied(action(state, "record_review", {
    candidate: { candidate_id: "candidate-integration-b", version: 1 }, expected_criteria_revision: 1, findings: [],
    resource_basis: [], review_id: "review-integration-b", verdict: "passed",
  }, WORKER_B));
  state = applied(action(state, "accept_result", {
    candidate: { candidate_id: "candidate-integration-b", version: 1 }, check_refs: [{ id: "criterion-1", revision: 1 }],
    expected_direction_revision: 0, result_id: "result-integration-b", review_refs: [{ id: "review-integration-b", revision: 1 }],
  }, HUMAN));
  assert.equal(state.phase, "completed");
});

test("correction outcomes are exactly once per completed Work Attempt", () => {
  let state = completedCorrectionProblem("problem-once", "correction-once");
  let problem = (state.problems as CanonicalObject[]).find((item) => item.problem_id === "problem-once")!;
  state = applied(action(state, "record_correction_outcome", {
    evidence_refs: [], expected_problem_revision: problem.revision as number, outcome: "unsuccessful",
    problem_id: "problem-once", work_id: "correction-once",
  }, WORKER_B));
  problem = (state.problems as CanonicalObject[]).find((item) => item.problem_id === "problem-once")!;
  assert.deepEqual(problem.recorded_correction_attempt_ids, ["attempt-correction-once"]);
  const duplicate = action(state, "record_correction_outcome", {
    evidence_refs: [], expected_problem_revision: problem.revision as number, outcome: "unsuccessful",
    problem_id: "problem-once", work_id: "correction-once",
  }, WORKER_B);
  assert.equal(duplicate.activity_disposition_type, "reject");
  assert.equal(duplicate.activity_disposition_type === "reject" ? duplicate.declared_code : "", "invalid_correction");
  assert.equal(problem.failed_corrections, 1);
});

test("useful correction outcomes require exact goal-relevant authoritative evidence", () => {
  let state = completedCorrectionProblem("problem-evidence", "correction-evidence");
  const problem = (state.problems as CanonicalObject[]).find((item) => item.problem_id === "problem-evidence")!;
  const selfReport = action(state, "record_correction_outcome", {
    evidence_refs: ["self-report:looks-good"], expected_problem_revision: problem.revision as number, outcome: "useful",
    problem_id: "problem-evidence", work_id: "correction-evidence",
  }, WORKER_B);
  assert.equal(selfReport.activity_disposition_type, "reject");
  assert.equal(selfReport.activity_disposition_type === "reject" ? selfReport.declared_code : "", "missing_evidence");
  state = applied(action(state, "record_correction_outcome", {
    evidence_refs: ["contribution:contribution-correction-evidence:1"], expected_problem_revision: problem.revision as number,
    outcome: "useful", problem_id: "problem-evidence", work_id: "correction-evidence",
  }, WORKER_B));
  const resolved = (state.problems as CanonicalObject[]).find((item) => item.problem_id === "problem-evidence")!;
  assert.equal(resolved.status, "resolved");
  assert.deepEqual(resolved.recorded_correction_attempt_ids, ["attempt-correction-evidence"]);
});

test("fresh Problem ids cannot reset correction history for the same active scope", () => {
  let state = completedCorrectionProblem("problem-original", "correction-original");
  state = applied(action(state, "issue_direction", {
    direction_id: "direction-repeat-problem", expected_direction_revision: 0, instruction: "Reassess the same stalled work.",
    target_scope: "work", target_work_ids: ["stalled-problem-original"],
  }, HUMAN));
  const progress = (state.outstanding_progress_reviews as CanonicalObject[])[0]!;
  state = claim(state, progress.work_id as string, WORKER_B);
  const claimed = (state.outstanding_progress_reviews as CanonicalObject[])[0]!;
  const repeated = action(state, "report_progress_review", {
    assessment: "realignment_required", corrective_work_ids: ["correction-original"], evidence_refs: [],
    expected_review_revision: claimed.revision as number, problem_id: "problem-cosmetic-new-id", problem_scope: "work",
    review_id: claimed.review_id as string, summary: "Cosmetically renamed recurrence.",
  }, WORKER_B);
  assert.equal(repeated.activity_disposition_type, "reject");
  assert.equal(repeated.activity_disposition_type === "reject" ? repeated.declared_code : "", "invalid_correction");
  assert.deepEqual((state.problems as CanonicalObject[]).map((item) => item.problem_id), ["problem-original"]);
});

test("only the Human can revise next-Invocation member configuration at its exact revision", () => {
  let state = claim(propose(confirm(), "active-during-config"), "active-during-config", WORKER_A);
  const humanView = pack.view({ activity_state: state, core: core(), viewer: { member_id: HUMAN, viewer_type: "participant" } });
  const workerView = pack.view({ activity_state: state, core: core(), viewer: { member_id: WORKER_A, viewer_type: "participant" } });
  assert.equal(humanView.action_offers.some((offer) => typeof offer !== "string" && offer.actionType === "update_member_configuration"), true);
  assert.equal(workerView.action_offers.some((offer) => typeof offer !== "string" && offer.actionType === "update_member_configuration"), false);
  const payload = {
    expected_configuration_revision: 1, member_key: "worker-a", moving_alias_acknowledged: false,
    provider: "claude", requested_effort: "high", requested_model: "claude-sonnet-4-5",
  };
  const unauthorized = action(state, "update_member_configuration", payload, WORKER_A);
  assert.equal(unauthorized.activity_disposition_type, "reject");
  assert.equal(unauthorized.activity_disposition_type === "reject" ? unauthorized.declared_code : "", "role_violation");
  const workBefore = structuredClone(state.work_items);
  const updated = action(state, "update_member_configuration", payload, HUMAN);
  state = applied(updated);
  const member = (state.roster as CanonicalObject[]).find((item) => item.member_key === "worker-a")!;
  assert.equal(member.member_id, WORKER_A);
  assert.equal(member.configuration_revision, 2);
  assert.equal(member.configuration_state, "resolution_unreported");
  assert.equal(member.provider, "claude");
  assert.equal(member.requested_model, "claude-sonnet-4-5");
  assert.equal(member.requested_effort, "high");
  assert.deepEqual(state.work_items, workBefore);
  if (updated.activity_disposition_type === "apply") {
    assert.equal((updated.ordered_domain_events[0] as CanonicalObject).event_type, "member_configuration_updated");
  }
  const stale = action(state, "update_member_configuration", payload, HUMAN);
  assert.equal(stale.activity_disposition_type, "reject");
  assert.equal(stale.activity_disposition_type === "reject" ? stale.declared_code : "", "stale_configuration_revision");
  const unacknowledgedAlias = action(state, "update_member_configuration", {
    expected_configuration_revision: 2, member_key: "worker-a", moving_alias_acknowledged: false,
    provider: "codex", requested_model: "latest",
  }, HUMAN);
  assert.equal(unacknowledgedAlias.activity_disposition_type, "reject");
  assert.equal(unacknowledgedAlias.activity_disposition_type === "reject" ? unacknowledgedAlias.declared_code : "", "moving_alias_unacknowledged");
  state = applied(action(state, "update_member_configuration", {
    expected_configuration_revision: 2, member_key: "worker-a", moving_alias_acknowledged: true,
    provider: "codex", requested_model: "latest",
  }, HUMAN));
  const aliasSelection = (state.roster as CanonicalObject[]).find((item) => item.member_key === "worker-a")!;
  assert.equal(aliasSelection.configuration_revision, 3);
  assert.equal(aliasSelection.moving_alias_acknowledged, true);
  assert.equal(aliasSelection.configuration_state, "resolution_unreported");
  const beforeConfirmation = action(initial(), "update_member_configuration", payload, HUMAN);
  assert.equal(beforeConfirmation.activity_disposition_type, "reject");
  assert.equal(beforeConfirmation.activity_disposition_type === "reject" ? beforeConfirmation.declared_code : "", "invalid_phase");
});
