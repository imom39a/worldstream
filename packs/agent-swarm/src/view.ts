import {
  canonicalStringify,
  type ActivityPackViewerClass,
  type CanonicalJson,
  type CanonicalObject,
  type PackActionOffer,
} from "@worldstream/pack-sdk";

import { cloneCanonical, coreRecord, record, roleFor, stateValue, stringValue, type SwarmState } from "./model.js";
import { actionNames, type ActionType } from "./schemas.js";
import { workCanBeHandedOff, workIsEligible } from "./workflow.js";

function viewerClass(core: CanonicalObject, viewer: CanonicalObject): ActivityPackViewerClass {
  const type = stringValue(viewer.viewer_type, "viewer viewer_type");
  if (type === "public" || type === "operator" || type === "final_reveal") return type;
  const memberId = stringValue(viewer.member_id, "viewer member_id");
  if (type === "participant") return roleFor(core, memberId) === null ? "public" : "participant";
  if (type !== "historical") throw new TypeError("viewer viewer_type is invalid");
  const membership = record(record(core.memberships, "Core memberships")[memberId], "Core membership");
  return membership.access_mode === "participant" ? "historical_participant" : membership.access_mode === "operator" ? "historical_operator" : "historical_public";
}

function publicProjection(state: SwarmState): CanonicalObject {
  return {
    blocker_count: state.blockers.filter((item) => item.status === "unresolved").length + state.resource_conflicts.filter((item) => item.status === "unresolved").length,
    completed_result_count: state.results.length,
    execution_epoch: state.execution_epoch,
    open_work_count: state.work_items.filter((item) => item.execution_epoch === state.execution_epoch && (item.status === "open" || item.status === "claimed" || item.status === "blocked")).length,
    outstanding_progress_review_count: state.outstanding_progress_reviews.length,
    phase: state.phase,
    setup_revision: state.setup_revision,
    worker_count: state.roster.length,
  };
}

function participantProjection(state: SwarmState, memberId: string, role: "human_coordinator" | "worker"): CanonicalObject {
  return {
    acceptance_criteria: [...state.acceptance_criteria], approved_working_area: cloneCanonical(state.approved_working_area), blockers: cloneCanonical(state.blockers),
    candidates: cloneCanonical(state.candidates), checks: cloneCanonical(state.checks), constraints: [...state.constraints], contributions: cloneCanonical(state.contributions),
    correction_failure_limit: state.correction_failure_limit, criteria_revision: state.criteria_revision, direction_revision: state.direction_revision, directions: cloneCanonical(state.directions), execution_epoch: state.execution_epoch,
    findings: cloneCanonical(state.findings), goal: state.goal, goal_revision: state.goal_revision, handoffs: cloneCanonical(state.handoffs), late_contributions: cloneCanonical(state.late_contributions),
    member_notice: state.member_notices[memberId] ?? "No private Agent Swarm notice is available.", outstanding_progress_reviews: cloneCanonical(state.outstanding_progress_reviews),
    phase: state.phase, problems: cloneCanonical(state.problems), progress_review_interval_seconds: state.progress_review_interval_seconds,
    resource_conflicts: cloneCanonical(state.resource_conflicts), resources: cloneCanonical(state.resources),
    results: cloneCanonical(state.results), reviews: cloneCanonical(state.reviews), roster: cloneCanonical(state.roster), setup_revision: state.setup_revision,
    suggestions: cloneCanonical(state.suggestions), viewer: { member_id: memberId, role }, work_attempts: cloneCanonical(state.work_attempts),
    work_items: cloneCanonical(state.work_items), writebacks: cloneCanonical(state.writebacks),
  } as unknown as CanonicalObject;
}

function offered(types: readonly ActionType[]): PackActionOffer[] {
  const order = Object.values(actionNames()) as readonly ActionType[];
  return [...new Set(types)]
    .sort((left, right) => order.indexOf(left) - order.indexOf(right))
    .map((actionType) => ({ actionType, eligibilityWindow: null }));
}

function offers(state: SwarmState, memberId: string, role: "human_coordinator" | "worker"): PackActionOffer[] {
  const ACTIONS = actionNames();
  if (state.phase === "awaiting_human_confirmation") return role === "human_coordinator" ? offered([ACTIONS.confirm]) : [];
  if (state.phase === "completed") return offered([...(role === "human_coordinator" ? [ACTIONS.reopen] : []), ACTIONS.lateContribution]);
  const result: ActionType[] = [ACTIONS.proposeWork, ACTIONS.submitSuggestion, ACTIONS.resourceVersion];
  if (role === "human_coordinator") result.push(ACTIONS.issueDirection, ACTIONS.dispositionSuggestion, ACTIONS.updateMemberConfiguration);
  const mayResolveBlocker = state.blockers.some((blocker) => blocker.status === "unresolved" &&
    (role === "human_coordinator" || (blocker.scope === "work" && blocker.reported_by_member_id === memberId && !blocker.blocker_id.startsWith("direction:") && !blocker.blocker_id.startsWith("writeback-uncertain:")))) ||
    (role === "human_coordinator" && state.resource_conflicts.some((conflict) => conflict.status === "unresolved"));
  if (mayResolveBlocker) {
    result.push(ACTIONS.resolveBlocker);
  }
  if (role === "worker" && state.work_items.some((item) => workIsEligible(state, item))) result.push(ACTIONS.claimWork);
  if (state.work_items.some((item) => workCanBeHandedOff(state, item, memberId, role))) result.push(ACTIONS.handoffWork);
  const owned = state.work_items.filter((item) => item.owner_member_id === memberId && item.execution_epoch === state.execution_epoch);
  if (owned.some((item) => item.status === "claimed" && item.kind !== "progress_review")) result.push(ACTIONS.submitContribution, ACTIONS.reportBlocker);
  if (owned.some((item) => (item.status === "claimed" || item.status === "completed") && item.kind !== "progress_review")) result.push(ACTIONS.submitCandidate);
  if (state.outstanding_progress_reviews.some((review) => owned.some((item) => item.work_id === review.work_id))) result.push(ACTIONS.reportProgress);
  if (state.candidates.some((item) => item.execution_epoch === state.execution_epoch)) result.push(ACTIONS.recordCheck, ACTIONS.recordReview, ACTIONS.acceptResult, ACTIONS.requestWriteback);
  const mayResolveFinding = state.findings.some((finding) => {
    if (finding.status === "resolved") return false;
    if (role === "human_coordinator") return true;
    const candidate = state.candidates.find((item) => item.candidate_id === finding.candidate.candidate_id && item.version === finding.candidate.version);
    const review = state.reviews.find((item) => item.review_id === finding.review_id && item.candidate.candidate_id === finding.candidate.candidate_id && item.candidate.version === finding.candidate.version);
    return candidate !== undefined && review !== undefined && memberId !== candidate.author_member_id && memberId !== review.reviewer_member_id;
  });
  if (mayResolveFinding) result.push(ACTIONS.resolveFinding);
  if (state.problems.some((item) => item.status === "unresolved")) result.push(ACTIONS.correctionOutcome);
  if (state.writebacks.some((item, index) => item.status === "requested" && !state.writebacks.slice(index + 1).some((later) => later.operation_id === item.operation_id))) result.push(ACTIONS.writebackOutcome);
  if (state.work_items.some((item) => item.execution_epoch === state.execution_epoch && item.status === "open" && item.owner_member_id === null)) result.push(ACTIONS.reviseDependencies);
  return offered(result);
}

export function authorizedView(state: SwarmState, core: CanonicalObject, viewer: CanonicalObject): { action_offers: PackActionOffer[]; projection: CanonicalObject; projection_schema: ActivityPackViewerClass } {
  const kind = viewerClass(core, viewer);
  if (kind !== "participant" && kind !== "historical_participant") return { action_offers: [], projection: publicProjection(state), projection_schema: kind };
  const memberId = stringValue(viewer.member_id, "viewer member_id");
  const role = roleFor(core, memberId);
  if (role === null) return { action_offers: [], projection: publicProjection(state), projection_schema: kind === "participant" ? "public" : "historical_public" };
  return { action_offers: kind === "participant" ? offers(state, memberId, role) : [], projection: participantProjection(state, memberId, role), projection_schema: kind };
}

export function viewSwarm(input: CanonicalObject) {
  return authorizedView(stateValue(input.activity_state), coreRecord(input.core), record(input.viewer, "viewer"));
}

export function observeSwarm(input: CanonicalObject) {
  const viewer = record(input.viewer, "viewer");
  const beforeCore = input.core_before === undefined ? coreRecord(input.core) : coreRecord(input.core_before);
  const afterCore = input.core_after === undefined ? coreRecord(input.core) : coreRecord(input.core_after);
  const before = authorizedView(stateValue(input.activity_before), beforeCore, viewer);
  const after = authorizedView(stateValue(input.activity_after), afterCore, viewer);
  const offersChanged = canonicalStringify(before.action_offers as unknown as CanonicalJson) !== canonicalStringify(after.action_offers as unknown as CanonicalJson);
  if (!offersChanged && canonicalStringify(before.projection) === canonicalStringify(after.projection)) return null;
  return { observation_schema: after.projection_schema, observation: after.projection, action_offers: offersChanged ? "reuse_after_view" as const : "unchanged" as const };
}
