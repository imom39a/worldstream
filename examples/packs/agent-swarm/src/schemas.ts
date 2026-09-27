import type { CanonicalObject } from "@worldstream/pack-sdk";

import { CHECK_EVIDENCE_MEDIA_TYPE, MAX_ITEMS, MAX_TEXT, MAX_WORKERS, SETUP_REVISION } from "./model.js";

export function actionNames() { return {
  confirm: "confirm_initial_setup",
  proposeWork: "propose_work_item",
  reviseDependencies: "revise_work_dependencies",
  claimWork: "claim_work_item",
  handoffWork: "handoff_work_item",
  reportBlocker: "report_work_blocker",
  resolveBlocker: "resolve_work_blocker",
  submitContribution: "submit_contribution",
  submitCandidate: "submit_candidate",
  recordCheck: "record_check",
  recordReview: "record_review",
  resolveFinding: "resolve_review_finding",
  acceptResult: "accept_result",
  submitSuggestion: "submit_suggestion",
  dispositionSuggestion: "record_suggestion_disposition",
  issueDirection: "issue_direction",
  reportProgress: "report_progress_review",
  correctionOutcome: "record_correction_outcome",
  updateMemberConfiguration: "update_member_configuration",
  resourceVersion: "record_resource_version",
  requestWriteback: "request_writeback",
  writebackOutcome: "record_writeback_outcome",
  lateContribution: "record_late_contribution",
  reopen: "reopen_swarm",
} as const; }

export type ActionType = ReturnType<typeof actionNames>[keyof ReturnType<typeof actionNames>];

function text(): CanonicalObject { return { maxLength: MAX_TEXT, minLength: 1, type: "string" }; }
function checkId(): CanonicalObject { return { maxLength: MAX_TEXT, minLength: 11, type: "string" }; }
function integer(minimum = 0): CanonicalObject { return { minimum, type: "integer" }; }
function bool(): CanonicalObject { return { type: "boolean" }; }
function enumText(values: readonly string[]): CanonicalObject { return { enum: [...values], type: "string" }; }
function strings(minItems = 0): CanonicalObject { return { items: text(), maxItems: MAX_ITEMS, minItems, type: "array" }; }
function objects(): CanonicalObject { return { items: { additionalProperties: true, type: "object" }, maxItems: MAX_ITEMS, type: "array" }; }
function object(properties: CanonicalObject, required: readonly string[]): CanonicalObject {
  return { additionalProperties: false, properties, required: [...required], type: "object" };
}

function entityRevisionRef(): CanonicalObject {
  return object({ id: text(), revision: integer(1) }, ["id", "revision"]);
}

function checkRevisionRef(): CanonicalObject {
  return object({ id: checkId(), revision: integer(1) }, ["id", "revision"]);
}

function candidateRef(): CanonicalObject {
  return object({ candidate_id: text(), version: integer(1) }, ["candidate_id", "version"]);
}

function contributionRef(): CanonicalObject {
  return object({ contribution_id: text(), version: integer(1) }, ["contribution_id", "version"]);
}

function versionRef(): CanonicalObject {
  return object({ resource_id: text(), version: integer(1) }, ["resource_id", "version"]);
}

function artifactRef(): CanonicalObject {
  return object({ artifact_id: text(), digest: text(), local_path: text(), media_type: text() }, ["artifact_id", "digest", "local_path", "media_type"]);
}

function checkEvidenceRef(): CanonicalObject {
  return object({
    artifact: object({
      artifact_id: text(),
      digest: { maxLength: 71, minLength: 71, type: "string" },
      local_path: text(),
      media_type: { const: CHECK_EVIDENCE_MEDIA_TYPE, type: "string" },
    }, ["artifact_id", "digest", "local_path", "media_type"]),
    candidate: candidateRef(),
    candidate_artifact: artifactRef(),
    check_id: checkId(),
    criterion: text(),
    criteria_revision: integer(1),
    resource_basis: referenceArray(versionRef()),
  }, ["artifact", "candidate", "candidate_artifact", "check_id", "criterion", "criteria_revision", "resource_basis"]);
}

function referenceArray(schema: CanonicalObject, minItems = 0): CanonicalObject {
  return { items: schema, maxItems: MAX_ITEMS, minItems, type: "array" };
}

function rosterEntry(bound: boolean): CanonicalObject {
  return object({
    ...(bound ? { configuration_revision: integer(1) } : {}),
    configuration_state: enumText(["fixture_unavailable", "resolution_unreported"]),
    label: text(),
    ...(bound ? { member_id: text() } : {}),
    member_key: text(),
    moving_alias_acknowledged: bool(),
    provider: text(),
    requested_effort: text(),
    requested_model: text(),
  }, [
    ...(bound ? ["configuration_revision"] : []),
    "configuration_state", "label", ...(bound ? ["member_id"] : []), "member_key",
    "moving_alias_acknowledged", "provider", "requested_model",
  ]);
}

function area(): CanonicalObject {
  return object({ resource_paths: strings(), root: text() }, ["resource_paths", "root"]);
}

export function configurationSchema(): CanonicalObject {
  return object({
    acceptance_criteria: strings(1),
    approved_working_area: area(),
    constraints: strings(),
    correction_failure_limit: integer(1),
    goal: text(),
    progress_review_interval_seconds: integer(1),
    roster: { items: rosterEntry(false), maxItems: MAX_WORKERS, minItems: 1, type: "array" },
  }, ["acceptance_criteria", "approved_working_area", "constraints", "goal", "roster"]);
}

export function actionDescriptors(): readonly { actionType: ActionType; payloadSchema: CanonicalObject }[] {
  const ACTIONS = actionNames();
  const scope = enumText(["goal", "work"]);
  const status = enumText(["passed", "failed"]);
  return [
    { actionType: ACTIONS.confirm, payloadSchema: object({ setup_revision: { const: SETUP_REVISION, type: "integer" } }, ["setup_revision"]) },
    { actionType: ACTIONS.proposeWork, payloadSchema: object({ dependency_ids: strings(), description: text(), expected_goal_revision: integer(1), kind: enumText(["goal", "integration", "correction"]), supersedes_work_id: text(), title: text(), work_id: text() }, ["dependency_ids", "description", "expected_goal_revision", "kind", "title", "work_id"]) },
    { actionType: ACTIONS.reviseDependencies, payloadSchema: object({ dependency_ids: strings(), expected_work_revision: integer(1), work_id: text() }, ["dependency_ids", "expected_work_revision", "work_id"]) },
    { actionType: ACTIONS.claimWork, payloadSchema: object({ attempt_id: text(), expected_work_revision: integer(1), work_id: text() }, ["attempt_id", "expected_work_revision", "work_id"]) },
    { actionType: ACTIONS.handoffWork, payloadSchema: object({ evidence_refs: strings(1), expected_attempt_revision: integer(1), expected_work_revision: integer(1), from_attempt_id: text(), handoff_id: text(), reason: text(), receiving_member_id: text(), reconciliation: enumText(["confirmed_terminal", "effects_reconciled"]), to_attempt_id: text(), work_id: text() }, ["evidence_refs", "expected_attempt_revision", "expected_work_revision", "from_attempt_id", "handoff_id", "reason", "receiving_member_id", "reconciliation", "to_attempt_id", "work_id"]) },
    { actionType: ACTIONS.reportBlocker, payloadSchema: object({ blocker_id: text(), expected_work_revision: integer(1), evidence_refs: strings(), summary: text(), work_id: text() }, ["blocker_id", "evidence_refs", "expected_work_revision", "summary", "work_id"]) },
    { actionType: ACTIONS.resolveBlocker, payloadSchema: object({ blocker_id: text(), evidence_refs: strings(1), expected_blocker_revision: integer(1) }, ["blocker_id", "evidence_refs", "expected_blocker_revision"]) },
    { actionType: ACTIONS.submitContribution, payloadSchema: object({ artifact: artifactRef(), completes_work: bool(), contribution_id: text(), expected_attempt_revision: integer(1), expected_work_revision: integer(1), resource_basis: referenceArray(versionRef()), source_refs: strings(), summary: text(), work_id: text() }, ["artifact", "completes_work", "contribution_id", "expected_attempt_revision", "expected_work_revision", "resource_basis", "source_refs", "summary", "work_id"]) },
    { actionType: ACTIONS.submitCandidate, payloadSchema: object({ artifact: artifactRef(), candidate_id: text(), contribution_refs: referenceArray(contributionRef(), 1), expected_work_revision: integer(1), resource_basis: referenceArray(versionRef()), work_id: text() }, ["artifact", "candidate_id", "contribution_refs", "expected_work_revision", "resource_basis", "work_id"]) },
    { actionType: ACTIONS.recordCheck, payloadSchema: object({ candidate: candidateRef(), check_id: checkId(), evidence_refs: { items: checkEvidenceRef(), maxItems: 1, minItems: 1, type: "array" }, expected_criteria_revision: integer(1), resource_basis: referenceArray(versionRef()), status }, ["candidate", "check_id", "evidence_refs", "expected_criteria_revision", "resource_basis", "status"]) },
    { actionType: ACTIONS.recordReview, payloadSchema: object({ candidate: candidateRef(), expected_criteria_revision: integer(1), findings: referenceArray(object({ corrective_work_id: text(), finding_id: text(), severity: enumText(["blocking", "advisory"]), summary: text() }, ["finding_id", "severity", "summary"])), resource_basis: referenceArray(versionRef()), review_id: text(), verdict: enumText(["passed", "changes_requested", "disputed"]) }, ["candidate", "expected_criteria_revision", "findings", "resource_basis", "review_id", "verdict"]) },
    { actionType: ACTIONS.resolveFinding, payloadSchema: object({ expected_finding_revision: integer(1), evidence_refs: strings(1), finding_id: text(), resolution: enumText(["resolved", "disputed"]) }, ["evidence_refs", "expected_finding_revision", "finding_id", "resolution"]) },
    { actionType: ACTIONS.acceptResult, payloadSchema: object({ candidate: candidateRef(), check_refs: referenceArray(checkRevisionRef(), 1), expected_direction_revision: integer(), result_id: text(), review_refs: referenceArray(entityRevisionRef(), 1) }, ["candidate", "check_refs", "expected_direction_revision", "result_id", "review_refs"]) },
    { actionType: ACTIONS.submitSuggestion, payloadSchema: object({ suggestion_id: text(), summary: text(), target_scope: scope, target_work_ids: strings() }, ["suggestion_id", "summary", "target_scope", "target_work_ids"]) },
    { actionType: ACTIONS.dispositionSuggestion, payloadSchema: object({ disposition: enumText(["accepted", "rejected"]), expected_suggestion_revision: integer(1), reason: text(), suggestion_id: text() }, ["disposition", "expected_suggestion_revision", "reason", "suggestion_id"]) },
    { actionType: ACTIONS.issueDirection, payloadSchema: object({ direction_id: text(), expected_direction_revision: integer(), instruction: text(), target_scope: scope, target_work_ids: strings() }, ["direction_id", "expected_direction_revision", "instruction", "target_scope", "target_work_ids"]) },
    { actionType: ACTIONS.reportProgress, payloadSchema: object({ assessment: enumText(["aligned", "realignment_required", "blocked"]), corrective_work_ids: strings(), evidence_refs: strings(), expected_review_revision: integer(1), problem_id: text(), problem_scope: scope, review_id: text(), summary: text() }, ["assessment", "corrective_work_ids", "evidence_refs", "expected_review_revision", "review_id", "summary"]) },
    { actionType: ACTIONS.correctionOutcome, payloadSchema: object({ evidence_refs: strings(), expected_problem_revision: integer(1), outcome: enumText(["useful", "unsuccessful"]), problem_id: text(), work_id: text() }, ["evidence_refs", "expected_problem_revision", "outcome", "problem_id", "work_id"]) },
    { actionType: ACTIONS.updateMemberConfiguration, payloadSchema: object({ expected_configuration_revision: integer(1), member_key: text(), moving_alias_acknowledged: bool(), provider: enumText(["codex", "claude", "kiro", "controlled"]), requested_effort: text(), requested_model: text() }, ["expected_configuration_revision", "member_key", "moving_alias_acknowledged", "provider", "requested_model"]) },
    { actionType: ACTIONS.resourceVersion, payloadSchema: object({ affected_work_ids: strings(), digest: text(), expected_previous_version: integer(), local_path: text(), resource_id: text(), version: integer(1) }, ["affected_work_ids", "digest", "expected_previous_version", "local_path", "resource_id", "version"]) },
    { actionType: ACTIONS.requestWriteback, payloadSchema: object({ candidate: candidateRef(), expected_resource_version: integer(), operation_id: text(), resource_id: text() }, ["candidate", "expected_resource_version", "operation_id", "resource_id"]) },
    { actionType: ACTIONS.writebackOutcome, payloadSchema: object({ evidence_refs: strings(), expected_writeback_revision: integer(1), operation_id: text(), resulting_version: integer(1), status: enumText(["applied", "conflicted", "uncertain"]) }, ["evidence_refs", "expected_writeback_revision", "operation_id", "status"]) },
    { actionType: ACTIONS.lateContribution, payloadSchema: object({ artifact: artifactRef(), attempt_id: text(), expected_work_revision: integer(1), late_id: text(), summary: text(), work_id: text() }, ["artifact", "attempt_id", "expected_work_revision", "late_id", "summary", "work_id"]) },
    { actionType: ACTIONS.reopen, payloadSchema: object({ expected_execution_epoch: integer(1), reason: text() }, ["expected_execution_epoch", "reason"]) },
  ];
}

export function stateSchema(): CanonicalObject {
  return object({
    acceptance_criteria: strings(1), approved_working_area: area(), blockers: objects(), candidates: objects(), checks: objects(),
    confirmed_at_room_seq: integer(1), confirmed_by_member_id: text(), constraints: strings(), contributions: objects(),
    correction_failure_limit: integer(1), criteria_revision: integer(1), direction_revision: integer(), directions: objects(),
    execution_epoch: integer(1), findings: objects(), goal: text(), goal_revision: integer(1), human_coordinator_member_id: text(),
    handoffs: objects(), late_contributions: objects(), member_notices: { additionalProperties: true, type: "object" },
    outstanding_progress_reviews: objects(),
    phase: enumText(["awaiting_human_confirmation", "open", "completed"]), problems: objects(), progress_review_interval_seconds: integer(1),
    progress_review_sequence: integer(), resource_conflicts: objects(), resources: objects(), results: objects(),
    reviews: objects(), room_seq: integer(), roster: { items: rosterEntry(true), maxItems: MAX_WORKERS, minItems: 1, type: "array" },
    setup_revision: { const: SETUP_REVISION, type: "integer" }, suggestions: objects(), work_attempts: objects(), work_items: objects(), writebacks: objects(),
  }, [
    "acceptance_criteria", "approved_working_area", "blockers", "candidates", "checks",
    "constraints", "contributions", "correction_failure_limit", "criteria_revision", "direction_revision", "directions", "execution_epoch",
    "findings", "goal", "goal_revision", "handoffs", "human_coordinator_member_id", "late_contributions", "member_notices", "outstanding_progress_reviews",
    "phase", "problems", "progress_review_interval_seconds", "progress_review_sequence", "resource_conflicts", "resources", "results", "reviews",
    "room_seq", "roster", "setup_revision", "suggestions", "work_attempts", "work_items", "writebacks",
  ]);
}

export function publicProjectionSchema(): CanonicalObject {
  return object({
    blocker_count: integer(), completed_result_count: integer(), execution_epoch: integer(1), open_work_count: integer(),
    outstanding_progress_review_count: integer(), phase: enumText(["awaiting_human_confirmation", "open", "completed"]),
    setup_revision: { const: SETUP_REVISION, type: "integer" }, worker_count: integer(1),
  }, ["blocker_count", "completed_result_count", "execution_epoch", "open_work_count", "outstanding_progress_review_count", "phase", "setup_revision", "worker_count"]);
}

export function participantProjectionSchema(): CanonicalObject {
  return object({
    acceptance_criteria: strings(1), approved_working_area: area(), blockers: objects(), candidates: objects(), checks: objects(), constraints: strings(),
    contributions: objects(), correction_failure_limit: integer(1), criteria_revision: integer(1), direction_revision: integer(), directions: objects(), execution_epoch: integer(1), findings: objects(),
    goal: text(), goal_revision: integer(1), handoffs: objects(), late_contributions: objects(), member_notice: text(), outstanding_progress_reviews: objects(),
    phase: enumText(["awaiting_human_confirmation", "open", "completed"]), problems: objects(), progress_review_interval_seconds: integer(1), resource_conflicts: objects(), resources: objects(), results: objects(),
    reviews: objects(), roster: { items: rosterEntry(true), maxItems: MAX_WORKERS, minItems: 1, type: "array" }, setup_revision: { const: SETUP_REVISION, type: "integer" },
    suggestions: objects(), viewer: object({ member_id: text(), role: enumText(["human_coordinator", "worker"]) }, ["member_id", "role"]),
    work_attempts: objects(), work_items: objects(), writebacks: objects(),
  }, [
    "acceptance_criteria", "approved_working_area", "blockers", "candidates", "checks", "constraints", "contributions", "correction_failure_limit", "criteria_revision",
    "direction_revision", "directions", "execution_epoch", "findings", "goal", "goal_revision", "handoffs", "late_contributions", "member_notice", "outstanding_progress_reviews",
    "phase", "problems", "progress_review_interval_seconds", "resource_conflicts", "resources", "results", "reviews", "roster", "setup_revision",
    "suggestions", "viewer", "work_attempts", "work_items", "writebacks",
  ]);
}
