import type {
  CanonicalJson,
  CanonicalObject,
  PackReduceOutput,
} from "@worldstream/pack-sdk";

import {
  booleanValue,
  CHECK_EVIDENCE_MEDIA_TYPE,
  coreRecord,
  enabledParticipants,
  ensureCapacity,
  ensureCurrentBasis,
  enumValue,
  integerValue,
  modelNeedsMovingAliasAcknowledgement,
  objectArray,
  optionalString,
  record,
  roleFor,
  RuleRejection,
  stateCanonical,
  stateValue,
  stringArray,
  stringValue,
  validateStateSize,
  type ArtifactRef,
  type Blocker,
  type Candidate,
  type CandidateRef,
  type Check,
  type CheckEvidenceRef,
  type Contribution,
  type ContributionRef,
  type Direction,
  type EntityRevisionRef,
  type Finding,
  type Problem,
  type ProgressReview,
  type ResourceConflict,
  type ResourceVersion,
  type Result,
  type Review,
  type Suggestion,
  type SwarmState,
  type VersionRef,
  type WorkAttempt,
  type WorkHandoff,
  type WorkItem,
  type Writeback,
} from "./model.js";
import { actionNames, type ActionType } from "./schemas.js";
import { addSecondsToTimestamp } from "./time.js";

export const PROGRESS_REVIEW_TIMER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FH4";

type Transition = {
  readonly state: SwarmState;
  readonly event: CanonicalObject;
  readonly timerRequests?: readonly CanonicalJson[];
  readonly attention?: readonly CanonicalJson[];
};

function reject(error: RuleRejection): PackReduceOutput {
  return { activity_disposition_type: "reject", bounded_safe_details: { reason: error.message }, declared_code: error.code };
}

function expectedRevision(actual: number, payload: CanonicalObject, field: string, code: string): void {
  if (integerValue(payload[field], field, 0) !== actual) throw new RuleRejection(code, `${field} does not match the current revision`);
}

function uniqueId(items: readonly CanonicalObject[], field: string, id: string, code: string): void {
  if (items.some((item) => item[field] === id)) throw new RuleRejection(code, `${field} is already recorded`);
}

function actorRole(core: CanonicalObject, memberId: string): "human_coordinator" | "worker" {
  const role = roleFor(core, memberId);
  if (role === null) throw new RuleRejection("role_violation", "Action Membership is not an enabled Agent Swarm participant");
  return role;
}

function requireHuman(state: SwarmState, core: CanonicalObject, memberId: string): void {
  if (memberId !== state.human_coordinator_member_id || actorRole(core, memberId) !== "human_coordinator") {
    throw new RuleRejection("role_violation", "only the Human coordinator may perform this Action");
  }
}

function requireOpen(state: SwarmState): void {
  if (state.phase !== "open") throw new RuleRejection("invalid_phase", "goal work is available only while the Swarm is open");
}

function workById(state: SwarmState, id: string): WorkItem {
  const work = state.work_items.find((item) => item.work_id === id);
  if (work === undefined) throw new RuleRejection("unknown_work", "Work Item is not in bounded current state");
  return work;
}

function attemptById(state: SwarmState, id: string): WorkAttempt {
  const attempt = state.work_attempts.find((item) => item.attempt_id === id);
  if (attempt === undefined) throw new RuleRejection("unknown_attempt", "Work Attempt is not in bounded current state");
  return attempt;
}

function candidateByRef(state: SwarmState, ref: CandidateRef): Candidate {
  const candidate = state.candidates.find((item) => item.candidate_id === ref.candidate_id && item.version === ref.version);
  if (candidate === undefined) throw new RuleRejection("unknown_candidate", "Candidate version is not recorded");
  return candidate;
}

function artifactValue(value: CanonicalJson | undefined, label: string): ArtifactRef {
  const item = record(value, label);
  return {
    artifact_id: stringValue(item.artifact_id, `${label}.artifact_id`),
    media_type: stringValue(item.media_type, `${label}.media_type`),
    local_path: stringValue(item.local_path, `${label}.local_path`),
    digest: stringValue(item.digest, `${label}.digest`),
  };
}

function authorizedLocalPath(state: SwarmState, value: CanonicalJson | undefined, label: string): string {
  const localPath = stringValue(value, label);
  const normalize = (path: string): string => path.replace(/\\/gu, "/").replace(/\/+$/u, "");
  const root = normalize(state.approved_working_area.root);
  const normalized = normalize(localPath);
  const relative = normalized === root ? "" : normalized.startsWith(`${root}/`) ? normalized.slice(root.length + 1) : null;
  if (relative === null || relative.split("/").some((segment) => segment === "." || segment === "..")) {
    throw new RuleRejection("artifact_outside_working_area", `${label} is outside the approved working area`);
  }
  return localPath;
}

function authorizedArtifact(state: SwarmState, value: CanonicalJson | undefined, label: string): ArtifactRef {
  const artifact = artifactValue(value, label);
  authorizedLocalPath(state, artifact.local_path, `${label}.local_path`);
  return artifact;
}

function candidateRefValue(value: CanonicalJson | undefined, label: string): CandidateRef {
  const item = record(value, label);
  return { candidate_id: stringValue(item.candidate_id, `${label}.candidate_id`), version: integerValue(item.version, `${label}.version`, 1) };
}

function contributionRefs(value: CanonicalJson | undefined, label: string): ContributionRef[] {
  return objectArray(value, label).map((item, index) => ({
    contribution_id: stringValue(item.contribution_id, `${label}[${index}].contribution_id`),
    version: integerValue(item.version, `${label}[${index}].version`, 1),
  }));
}

function versionRefs(value: CanonicalJson | undefined, label: string): VersionRef[] {
  return objectArray(value, label).map((item, index) => ({
    resource_id: stringValue(item.resource_id, `${label}[${index}].resource_id`),
    version: integerValue(item.version, `${label}[${index}].version`, 1),
  }));
}

function entityRefs(value: CanonicalJson | undefined, label: string): EntityRevisionRef[] {
  return objectArray(value, label).map((item, index) => ({
    id: stringValue(item.id, `${label}[${index}].id`),
    revision: integerValue(item.revision, `${label}[${index}].revision`, 1),
  }));
}

function sameVersionRefs(left: readonly VersionRef[], right: readonly VersionRef[]): boolean {
  const key = (item: VersionRef) => `${item.resource_id}:${item.version}`;
  return left.length === right.length && [...left].map(key).sort().join("|") === [...right].map(key).sort().join("|");
}

function currentResourceBasis(state: SwarmState, refs: readonly VersionRef[]): void {
  for (const ref of refs) {
    const current = state.resources.filter((item) => item.resource_id === ref.resource_id).sort((a, b) => b.version - a.version)[0];
    if (current === undefined || current.version !== ref.version) throw new RuleRejection("stale_resource_basis", "resource basis is not the current recorded version");
  }
}

function workLineage(state: SwarmState, workId: string): Set<string> {
  const lineage = new Set<string>();
  let cursor: string | null = workId;
  while (cursor !== null && !lineage.has(cursor)) {
    lineage.add(cursor);
    cursor = state.work_items.find((item) => item.work_id === cursor)?.supersedes_work_id ?? null;
  }
  return lineage;
}

function workSharesLineage(state: SwarmState, leftWorkId: string, rightWorkId: string): boolean {
  const left = workLineage(state, leftWorkId);
  return [...workLineage(state, rightWorkId)].some((workId) => left.has(workId));
}

function candidatesShareIntegrationLineage(state: SwarmState, left: Candidate, right: Candidate): boolean {
  return workSharesLineage(state, left.work_id, right.work_id);
}

function currentWork(state: SwarmState): readonly WorkItem[] {
  return state.work_items.filter((item) => item.execution_epoch === state.execution_epoch);
}

function dependentClosure(state: SwarmState, initial: readonly string[]): string[] {
  const affected = new Set(initial);
  let changed = true;
  while (changed) {
    changed = false;
    for (const work of currentWork(state)) {
      if (!affected.has(work.work_id) && work.dependency_ids.some((id) => affected.has(id))) {
        affected.add(work.work_id);
        changed = true;
      }
    }
  }
  return [...affected].sort();
}

function validateDependencies(state: SwarmState, workId: string, dependencies: readonly string[]): void {
  for (const dependencyId of dependencies) {
    const dependency = workById(state, dependencyId);
    if (dependency.execution_epoch !== state.execution_epoch) throw new RuleRejection("invalid_dependency", "dependencies must belong to the current execution epoch");
  }
  const graph = new Map(currentWork(state).map((work) => [work.work_id, work.work_id === workId ? [...dependencies] : [...work.dependency_ids]]));
  if (!graph.has(workId)) graph.set(workId, [...dependencies]);
  const visiting = new Set<string>();
  const visited = new Set<string>();
  const visit = (id: string): void => {
    if (visiting.has(id)) throw new RuleRejection("dependency_cycle", "Work Item dependency graph must be acyclic");
    if (visited.has(id)) return;
    visiting.add(id);
    for (const dependency of graph.get(id) ?? []) visit(dependency);
    visiting.delete(id);
    visited.add(id);
  };
  for (const id of graph.keys()) visit(id);
}

export function workIsEligible(state: SwarmState, work: WorkItem): boolean {
  if (state.phase !== "open" || work.execution_epoch !== state.execution_epoch || work.status !== "open") return false;
  if (!work.dependency_ids.every((id) => {
    const dependency = state.work_items.find((item) => item.work_id === id);
    return dependency?.status === "completed";
  })) return false;
  if (work.kind === "progress_review") return true;
  if (state.blockers.some((item) => item.status === "unresolved" && (item.scope === "goal" || item.work_id === work.work_id))) return false;
  if (state.problems.some((item) => item.status === "escalated" && (item.scope === "goal" || item.affected_work_ids.includes(work.work_id)))) return false;
  if (state.resource_conflicts.some((item) => item.status === "unresolved" && item.affected_work_ids.includes(work.work_id))) return false;
  return true;
}

function latestAttemptForWork(state: SwarmState, workId: string): WorkAttempt | undefined {
  const attempts = state.work_attempts.filter((item) => item.work_id === workId && item.execution_epoch === state.execution_epoch);
  return attempts.length === 0 ? undefined : attempts[attempts.length - 1];
}

function hasUnreconciledExternalEffect(state: SwarmState, workId: string): boolean {
  if (state.blockers.some((item) => item.status === "unresolved" && item.work_id === workId && item.blocker_id.startsWith("writeback-uncertain:"))) return true;
  if (state.resource_conflicts.some((item) => item.status === "unresolved" && item.affected_work_ids.includes(workId))) return true;
  const candidateKeys = new Set(state.candidates.filter((item) => item.work_id === workId).map((item) => `${item.candidate_id}:${item.version}`));
  const related = state.writebacks.filter((item) => candidateKeys.has(`${item.candidate.candidate_id}:${item.candidate.version}`));
  const latest = related.filter((item, index) => !related.slice(index + 1).some((later) => later.operation_id === item.operation_id));
  return latest.some((item) => item.status === "requested" || (item.status === "uncertain" && state.blockers.some((blocker) => blocker.blocker_id === `writeback-uncertain:${item.operation_id}` && blocker.status === "unresolved")));
}

export function workCanBeHandedOff(state: SwarmState, work: WorkItem, actor: string, role: "human_coordinator" | "worker"): boolean {
  if (!workIsEligible(state, work) || work.owner_member_id !== null || work.active_attempt_id !== null) return false;
  const previous = latestAttemptForWork(state, work.work_id);
  if (previous === undefined || previous.status !== "interruption_requested" || hasUnreconciledExternalEffect(state, work.work_id)) return false;
  return role === "human_coordinator" || previous.owner_member_id === actor || state.roster.some((item) => item.member_id === actor);
}

function scheduledTimers(input: CanonicalObject): CanonicalObject {
  return input.scheduled_timers === undefined ? {} : record(input.scheduled_timers, "scheduled_timers");
}

function scheduleProgressReview(input: CanonicalObject, state: SwarmState, admittedAt: string): CanonicalJson {
  const scheduled = scheduledTimers(input);
  const current = scheduled[PROGRESS_REVIEW_TIMER_ID];
  const due = addSecondsToTimestamp(admittedAt, state.progress_review_interval_seconds);
  const payload: CanonicalObject = { execution_epoch: state.execution_epoch, timer: "progress_review" };
  return current === undefined
    ? { timer_request_type: "schedule_next", timer_id: PROGRESS_REVIEW_TIMER_ID, due, canonical_payload: payload }
    : { timer_request_type: "reschedule_current", timer_id: PROGRESS_REVIEW_TIMER_ID, expected_generation: integerValue(record(current, "scheduled progress review").generation, "scheduled progress review generation", 1), new_due: due, new_canonical_payload: payload };
}

function cancelProgressReview(input: CanonicalObject): readonly CanonicalJson[] {
  const current = scheduledTimers(input)[PROGRESS_REVIEW_TIMER_ID];
  if (current === undefined) return [];
  return [{ timer_request_type: "cancel_current", timer_id: PROGRESS_REVIEW_TIMER_ID, expected_generation: integerValue(record(current, "scheduled progress review").generation, "scheduled progress review generation", 1) }];
}

function progressAttention(state: SwarmState, core: CanonicalObject, review: ProgressReview, key: string): readonly CanonicalJson[] {
  const ACTIONS = actionNames();
  const enabled = enabledParticipants(core, true);
  const owner = workById(state, review.work_id).owner_member_id;
  const target = owner ?? enabled.workers[0]!;
  return [{ action_types: owner === null ? [ACTIONS.claimWork] : [ACTIONS.reportProgress], deduplication_key: key, priority: review.status === "blocked" ? 3 : 2, reason: review.status === "blocked" ? "progress_review_blocked" : "progress_review_due", target_member_id: target }];
}

type ProgressObligation = { readonly state: SwarmState; readonly review: ProgressReview | null; readonly created: boolean };

function progressScopeKey(scope: "goal" | "work", targetWorkIds: readonly string[]): string {
  return scope === "goal" ? "goal" : `work:${[...targetWorkIds].sort().join("|")}`;
}

function progressReviewForScope(state: SwarmState, scope: "goal" | "work", targetWorkIds: readonly string[]): ProgressReview | undefined {
  const key = progressScopeKey(scope, targetWorkIds);
  return state.outstanding_progress_reviews.find((review) => progressScopeKey(review.scope, review.target_work_ids) === key);
}

function createProgressObligation(state: SwarmState, scope: "goal" | "work", targetWorkIds: readonly string[], reason: ProgressReview["reason"]): ProgressObligation {
  if (state.phase !== "open") return { state, review: null, created: false };
  if (scope === "work" && targetWorkIds.length === 0) throw new RuleRejection("invalid_target", "work-scoped Progress Review requires identified target work");
  const existing = progressReviewForScope(state, scope, targetWorkIds);
  if (existing !== undefined) return { state, review: existing, created: false };
  ensureCapacity(state.work_items, "Work Item");
  ensureCapacity(state.outstanding_progress_reviews, "Progress Review");
  const sequence = state.progress_review_sequence + 1;
  const reviewId = `progress-review-${state.execution_epoch}-${sequence}`;
  const workId = `progress-work-${state.execution_epoch}-${sequence}`;
  const review: ProgressReview = { review_id: reviewId, revision: 1, execution_epoch: state.execution_epoch, scope, target_work_ids: [...targetWorkIds], work_id: workId, status: "due", reason };
  const work: WorkItem = {
    work_id: workId, revision: 1, execution_epoch: state.execution_epoch, kind: "progress_review", title: "Assess Swarm progress",
    description: `Assess current ${scope} progress using accepted goal, evidence, blockers and work state.`, dependency_ids: [], status: "open",
    owner_member_id: null, active_attempt_id: null, blocker_ids: [], direction_revision: state.direction_revision,
    created_by_member_id: state.human_coordinator_member_id, supersedes_work_id: null,
  };
  return {
    state: { ...state, outstanding_progress_reviews: [...state.outstanding_progress_reviews, review], progress_review_sequence: sequence, work_items: [...state.work_items, work] },
    review,
    created: true,
  };
}

function replaceWork(state: SwarmState, replacement: WorkItem): SwarmState {
  return { ...state, work_items: state.work_items.map((item) => item.work_id === replacement.work_id ? replacement : item) };
}

function transitionEvent(type: string, fields: CanonicalObject = {}): CanonicalObject {
  return { event_type: type, ...fields };
}

function confirmSetup(state: SwarmState, payload: CanonicalObject, actor: string, core: CanonicalObject, input: CanonicalObject, admittedAt: string): Transition {
  requireHuman(state, core, actor);
  if (state.phase !== "awaiting_human_confirmation") throw new RuleRejection("setup_already_confirmed", "initial setup is already confirmed");
  expectedRevision(state.setup_revision, payload, "setup_revision", "setup_revision_mismatch");
  const next = { ...state, phase: "open" as const, confirmed_by_member_id: actor, confirmed_at_room_seq: integerValue(input.next_room_seq, "next_room_seq", 1) };
  return {
    state: next,
    event: transitionEvent("initial_setup_confirmed", { member_id: actor, setup_revision: state.setup_revision }),
    timerRequests: [scheduleProgressReview(input, next, admittedAt)],
  };
}

function updateMemberConfiguration(state: SwarmState, payload: CanonicalObject, actor: string, core: CanonicalObject): Transition {
  requireHuman(state, core, actor);
  requireOpen(state);
  const memberKey = stringValue(payload.member_key, "member_key");
  const existing = state.roster.find((entry) => entry.member_key === memberKey);
  if (existing === undefined) throw new RuleRejection("unknown_member", "member_key is not in the authoritative Worker roster");
  expectedRevision(existing.configuration_revision, payload, "expected_configuration_revision", "stale_configuration_revision");
  const requestedModel = stringValue(payload.requested_model, "requested_model");
  const aliasAcknowledged = booleanValue(payload.moving_alias_acknowledged, "moving_alias_acknowledged");
  if (modelNeedsMovingAliasAcknowledgement(requestedModel) && !aliasAcknowledged) {
    throw new RuleRejection("moving_alias_unacknowledged", "a moving or unpinned model alias requires explicit acknowledgement");
  }
  const replacement = {
    ...existing,
    configuration_revision: existing.configuration_revision + 1,
    configuration_state: "resolution_unreported" as const,
    moving_alias_acknowledged: aliasAcknowledged,
    provider: enumValue(payload.provider, ["codex", "claude", "kiro", "controlled"], "provider"),
    requested_effort: optionalString(payload.requested_effort, "requested_effort"),
    requested_model: requestedModel,
  };
  return {
    state: { ...state, roster: state.roster.map((entry) => entry.member_key === memberKey ? replacement : entry) },
    event: transitionEvent("member_configuration_updated", {
      configuration_revision: replacement.configuration_revision,
      member_id: replacement.member_id,
      member_key: replacement.member_key,
      moving_alias_acknowledged: replacement.moving_alias_acknowledged,
      provider: replacement.provider,
      ...(replacement.requested_effort === null ? {} : { requested_effort: replacement.requested_effort }),
      requested_model: replacement.requested_model,
    }),
  };
}

function proposeWork(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  requireOpen(state);
  expectedRevision(state.goal_revision, payload, "expected_goal_revision", "stale_goal_revision");
  ensureCapacity(state.work_items, "Work Item");
  const workId = stringValue(payload.work_id, "work_id");
  if (state.work_items.some((item) => item.work_id === workId)) throw new RuleRejection("duplicate_id", "work_id is already recorded");
  const dependencies = stringArray(payload.dependency_ids, "dependency_ids");
  validateDependencies(state, workId, dependencies);
  const supersedes = optionalString(payload.supersedes_work_id, "supersedes_work_id");
  if (supersedes !== null) workById(state, supersedes);
  const work: WorkItem = {
    work_id: workId, revision: 1, execution_epoch: state.execution_epoch,
    kind: enumValue(payload.kind, ["goal", "integration", "correction"], "kind"),
    title: stringValue(payload.title, "title"), description: stringValue(payload.description, "description"), dependency_ids: dependencies,
    status: "open", owner_member_id: null, active_attempt_id: null, blocker_ids: [], direction_revision: state.direction_revision,
    created_by_member_id: actor, supersedes_work_id: supersedes,
  };
  return { state: { ...state, work_items: [...state.work_items, work] }, event: transitionEvent("work_item_proposed", { work_id: workId, revision: 1 }) };
}

function reviseDependencies(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  requireOpen(state);
  const work = workById(state, stringValue(payload.work_id, "work_id"));
  expectedRevision(work.revision, payload, "expected_work_revision", "stale_work_revision");
  if (work.execution_epoch !== state.execution_epoch || work.status !== "open" || work.owner_member_id !== null) throw new RuleRejection("work_ineligible", "only unclaimed current work may have its dependency plan revised");
  const dependencies = stringArray(payload.dependency_ids, "dependency_ids");
  validateDependencies(state, work.work_id, dependencies);
  const revised: WorkItem = { ...work, revision: work.revision + 1, dependency_ids: dependencies };
  return { state: replaceWork(state, revised), event: transitionEvent("work_dependencies_revised", { member_id: actor, revision: revised.revision, work_id: work.work_id }) };
}

function claimWork(state: SwarmState, payload: CanonicalObject, actor: string, core: CanonicalObject): Transition {
  requireOpen(state);
  if (actorRole(core, actor) !== "worker") throw new RuleRejection("role_violation", "only an enabled roster Worker may claim a Work Item");
  const work = workById(state, stringValue(payload.work_id, "work_id"));
  expectedRevision(work.revision, payload, "expected_work_revision", "stale_work_revision");
  if (!workIsEligible(state, work)) throw new RuleRejection("work_ineligible", "Work Item is not currently eligible to claim");
  ensureCapacity(state.work_attempts, "Work Attempt");
  const attemptId = stringValue(payload.attempt_id, "attempt_id");
  if (state.work_attempts.some((item) => item.attempt_id === attemptId)) throw new RuleRejection("duplicate_id", "attempt_id is already recorded");
  const attempt: WorkAttempt = { attempt_id: attemptId, revision: 1, work_id: work.work_id, execution_epoch: state.execution_epoch, owner_member_id: actor, status: "active" };
  const claimed: WorkItem = { ...work, revision: work.revision + 1, status: "claimed", owner_member_id: actor, active_attempt_id: attemptId };
  let next = replaceWork({ ...state, work_attempts: [...state.work_attempts, attempt] }, claimed);
  next = {
    ...next,
    outstanding_progress_reviews: next.outstanding_progress_reviews.map((review) => review.work_id === work.work_id
      ? { ...review, revision: review.revision + 1, status: "claimed" }
      : review),
  };
  return { state: next, event: transitionEvent("work_item_claimed", { attempt_id: attemptId, member_id: actor, work_id: work.work_id }) };
}

function handoffWork(state: SwarmState, payload: CanonicalObject, actor: string, core: CanonicalObject): Transition {
  requireOpen(state);
  const work = workById(state, stringValue(payload.work_id, "work_id"));
  expectedRevision(work.revision, payload, "expected_work_revision", "stale_work_revision");
  if (!workIsEligible(state, work) || work.owner_member_id !== null || work.active_attempt_id !== null) {
    throw new RuleRejection("work_ineligible", "Work Item must be reconciled, unowned and otherwise eligible before handoff");
  }
  const prior = latestAttemptForWork(state, work.work_id);
  const fromAttemptId = stringValue(payload.from_attempt_id, "from_attempt_id");
  if (prior === undefined || prior.attempt_id !== fromAttemptId || prior.execution_epoch !== state.execution_epoch) {
    throw new RuleRejection("handoff_unreconciled", "handoff must bind the latest current Work Attempt");
  }
  expectedRevision(prior.revision, payload, "expected_attempt_revision", "stale_attempt_revision");
  if (prior.status !== "interruption_requested" || hasUnreconciledExternalEffect(state, work.work_id)) {
    throw new RuleRejection("handoff_unreconciled", "old Work Attempt or external effects are not terminal and reconciled");
  }
  const receiver = stringValue(payload.receiving_member_id, "receiving_member_id");
  const receiverRoster = state.roster.find((item) => item.member_id === receiver);
  if (receiverRoster === undefined || roleFor(core, receiver) !== "worker" || receiver === prior.owner_member_id) {
    throw new RuleRejection("invalid_target", "receiving member must be a different enabled Worker already in the Swarm roster");
  }
  const role = actorRole(core, actor);
  if (role !== "human_coordinator" && actor !== prior.owner_member_id && actor !== receiver) {
    throw new RuleRejection("role_violation", "handoff requires the Human coordinator, prior owner, or receiving Worker");
  }
  const evidence = stringArray(payload.evidence_refs, "evidence_refs", 1);
  const reconciliation = enumValue(payload.reconciliation, ["confirmed_terminal", "effects_reconciled"], "reconciliation");
  ensureCapacity(state.handoffs, "Work Handoff");
  ensureCapacity(state.work_attempts, "Work Attempt");
  const handoffId = stringValue(payload.handoff_id, "handoff_id");
  if (state.handoffs.some((item) => item.handoff_id === handoffId)) throw new RuleRejection("duplicate_id", "handoff_id is already recorded");
  const toAttemptId = stringValue(payload.to_attempt_id, "to_attempt_id");
  if (state.work_attempts.some((item) => item.attempt_id === toAttemptId)) throw new RuleRejection("duplicate_id", "to_attempt_id is already recorded");
  const handoff: WorkHandoff = {
    handoff_id: handoffId, revision: 1, execution_epoch: state.execution_epoch, work_id: work.work_id, work_revision: work.revision,
    from_attempt_id: prior.attempt_id, from_member_id: prior.owner_member_id, to_attempt_id: toAttemptId, to_member_id: receiver,
    authorized_by_member_id: actor, reconciliation, evidence_refs: evidence, reason: stringValue(payload.reason, "reason"),
    direction_revision: work.direction_revision,
  };
  const nextAttempt: WorkAttempt = { attempt_id: toAttemptId, revision: 1, work_id: work.work_id, execution_epoch: state.execution_epoch, owner_member_id: receiver, status: "active" };
  const claimed: WorkItem = { ...work, revision: work.revision + 1, status: "claimed", owner_member_id: receiver, active_attempt_id: toAttemptId };
  let next = replaceWork({
    ...state,
    handoffs: [...state.handoffs, handoff],
    work_attempts: [...state.work_attempts.map((item) => item.attempt_id === prior.attempt_id ? { ...item, revision: item.revision + 1, status: "fenced" as const } : item), nextAttempt],
  }, claimed);
  next = {
    ...next,
    outstanding_progress_reviews: next.outstanding_progress_reviews.map((review) => review.work_id === work.work_id
      ? { ...review, revision: review.revision + 1, status: "claimed" }
      : review),
  };
  return {
    state: next,
    event: transitionEvent("work_item_handed_off", {
      authorized_by_member_id: actor, from_attempt_id: prior.attempt_id, from_member_id: prior.owner_member_id,
      handoff_id: handoffId, reconciliation, to_attempt_id: toAttemptId, to_member_id: receiver, work_id: work.work_id,
    }),
  };
}

function reportBlocker(state: SwarmState, payload: CanonicalObject, actor: string, input: CanonicalObject, core: CanonicalObject): Transition {
  requireOpen(state);
  const work = workById(state, stringValue(payload.work_id, "work_id"));
  expectedRevision(work.revision, payload, "expected_work_revision", "stale_work_revision");
  if (work.owner_member_id !== actor || work.active_attempt_id === null) throw new RuleRejection("not_work_owner", "only the authoritative Work Item owner may report its blocker");
  ensureCapacity(state.blockers, "Blocker");
  const blockerId = stringValue(payload.blocker_id, "blocker_id");
  if (state.blockers.some((item) => item.blocker_id === blockerId)) throw new RuleRejection("duplicate_id", "blocker_id is already recorded");
  const blocker: Blocker = { blocker_id: blockerId, revision: 1, execution_epoch: state.execution_epoch, scope: "work", work_id: work.work_id, summary: stringValue(payload.summary, "summary"), status: "unresolved", reported_by_member_id: actor, evidence_refs: stringArray(payload.evidence_refs, "evidence_refs") };
  const attempt = attemptById(state, work.active_attempt_id);
  const blockedWork: WorkItem = { ...work, revision: work.revision + 1, status: "blocked", blocker_ids: [...work.blocker_ids, blockerId] };
  let next = replaceWork({ ...state, blockers: [...state.blockers, blocker], work_attempts: state.work_attempts.map((item) => item.attempt_id === attempt.attempt_id ? { ...item, revision: item.revision + 1, status: "blocked" } : item) }, blockedWork);
  const currentReview = next.outstanding_progress_reviews.find((review) => review.work_id === work.work_id);
  let review: ProgressReview | null;
  if (currentReview === undefined) {
    const obligation = createProgressObligation(next, "work", [work.work_id], "blocker");
    next = obligation.state;
    review = obligation.review;
  } else {
    review = { ...currentReview, revision: currentReview.revision + 1, status: "blocked" };
    next = { ...next, outstanding_progress_reviews: next.outstanding_progress_reviews.map((item) => item.review_id === review!.review_id ? review! : item) };
  }
  return {
    state: next,
    event: transitionEvent("work_blocker_reported", { blocker_id: blockerId, work_id: work.work_id }),
    attention: review === null ? [] : progressAttention(next, core, review, `progress:blocker:${blockerId}`),
  };
}

function resolveBlocker(state: SwarmState, payload: CanonicalObject, actor: string, core: CanonicalObject): Transition {
  requireOpen(state);
  const blockerId = stringValue(payload.blocker_id, "blocker_id");
  const blocker = state.blockers.find((item) => item.blocker_id === blockerId);
  if (blocker === undefined) {
    const conflict = state.resource_conflicts.find((item) => item.conflict_id === blockerId);
    if (conflict === undefined) throw new RuleRejection("unknown_blocker", "blocker or resource conflict is not recorded");
    expectedRevision(conflict.revision, payload, "expected_blocker_revision", "stale_blocker_revision");
    if (conflict.status !== "unresolved") throw new RuleRejection("blocker_resolved", "resource conflict is already resolved");
    requireHuman(state, core, actor);
    const evidence = stringArray(payload.evidence_refs, "evidence_refs", 1);
    const nextConflict: ResourceConflict = { ...conflict, revision: conflict.revision + 1, status: "resolved", evidence_refs: [...conflict.evidence_refs, ...evidence] };
    const next = { ...state, resource_conflicts: state.resource_conflicts.map((item) => item.conflict_id === blockerId ? nextConflict : item) };
    return { state: next, event: transitionEvent("resource_conflict_resolved", { conflict_id: blockerId, member_id: actor }) };
  }
  expectedRevision(blocker.revision, payload, "expected_blocker_revision", "stale_blocker_revision");
  if (blocker.status !== "unresolved") throw new RuleRejection("blocker_resolved", "blocker is already resolved");
  const protectedReconciliation = blocker.blocker_id.startsWith("direction:") || blocker.blocker_id.startsWith("writeback-uncertain:");
  const workerMayResolveOwn = blocker.scope === "work" && blocker.reported_by_member_id === actor && !protectedReconciliation;
  if (actor !== state.human_coordinator_member_id && !workerMayResolveOwn) {
    throw new RuleRejection("role_violation", "only the Human coordinator or the reporting Work owner may resolve this blocker");
  }
  const evidence = stringArray(payload.evidence_refs, "evidence_refs", 1);
  const replacement: Blocker = { ...blocker, revision: blocker.revision + 1, status: "resolved", evidence_refs: [...blocker.evidence_refs, ...evidence] };
  let next: SwarmState = { ...state, blockers: state.blockers.map((item) => item.blocker_id === blockerId ? replacement : item) };
  const releasedAttemptIds = new Set(next.work_items.filter((work) => {
    if (!work.blocker_ids.includes(blockerId) || work.active_attempt_id === null) return false;
    return !work.blocker_ids.some((id) => id !== blockerId && next.blockers.some((item) => item.blocker_id === id && item.status === "unresolved"));
  }).map((work) => work.active_attempt_id!));
  next = { ...next, work_items: next.work_items.map((work) => {
    if (!work.blocker_ids.includes(blockerId)) return work;
    const hasOther = work.blocker_ids.some((id) => id !== blockerId && next.blockers.some((item) => item.blocker_id === id && item.status === "unresolved"));
    return hasOther ? work : { ...work, revision: work.revision + 1, status: "open", owner_member_id: null, active_attempt_id: null };
  }), work_attempts: next.work_attempts.map((attempt) => releasedAttemptIds.has(attempt.attempt_id) && (attempt.status === "active" || attempt.status === "blocked")
    ? { ...attempt, revision: attempt.revision + 1, status: "interruption_requested" }
    : attempt), outstanding_progress_reviews: next.outstanding_progress_reviews.map((review) => {
    const work = next.work_items.find((item) => item.work_id === review.work_id);
    return work?.blocker_ids.includes(blockerId) === true && review.status === "blocked"
      ? { ...review, revision: review.revision + 1, status: "due" }
      : review;
  }) };
  return { state: next, event: transitionEvent("work_blocker_resolved", { blocker_id: blockerId, member_id: actor }) };
}

function submitContribution(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  requireOpen(state);
  const work = workById(state, stringValue(payload.work_id, "work_id"));
  expectedRevision(work.revision, payload, "expected_work_revision", "stale_work_revision");
  if (work.kind === "progress_review") throw new RuleRejection("work_ineligible", "Progress Review work completes only through its assessment Action");
  if (work.owner_member_id !== actor || work.active_attempt_id === null || work.status !== "claimed") throw new RuleRejection("not_work_owner", "only the active authoritative owner may submit a Contribution");
  const attempt = attemptById(state, work.active_attempt_id);
  expectedRevision(attempt.revision, payload, "expected_attempt_revision", "stale_attempt_revision");
  const basis = versionRefs(payload.resource_basis, "resource_basis");
  currentResourceBasis(state, basis);
  ensureCapacity(state.contributions, "Contribution");
  const contributionId = stringValue(payload.contribution_id, "contribution_id");
  const version = state.contributions.filter((item) => item.contribution_id === contributionId).length + 1;
  const contribution: Contribution = {
    contribution_id: contributionId, version, execution_epoch: state.execution_epoch, work_id: work.work_id, work_revision: work.revision,
    attempt_id: attempt.attempt_id, author_member_id: actor, summary: stringValue(payload.summary, "summary"),
    source_refs: stringArray(payload.source_refs, "source_refs"), resource_basis: basis, artifact: authorizedArtifact(state, payload.artifact, "artifact"),
    direction_revision: state.direction_revision,
  };
  const completes = booleanValue(payload.completes_work, "completes_work");
  const nextAttempt: WorkAttempt = { ...attempt, revision: attempt.revision + 1, status: completes ? "completed" : "active" };
  const nextWork: WorkItem = completes ? { ...work, revision: work.revision + 1, status: "completed" } : work;
  const next = replaceWork({ ...state, contributions: [...state.contributions, contribution], work_attempts: state.work_attempts.map((item) => item.attempt_id === attempt.attempt_id ? nextAttempt : item) }, nextWork);
  return { state: next, event: transitionEvent("contribution_submitted", { contribution_id: contributionId, version, work_id: work.work_id }) };
}

function submitCandidate(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  requireOpen(state);
  const work = workById(state, stringValue(payload.work_id, "work_id"));
  expectedRevision(work.revision, payload, "expected_work_revision", "stale_work_revision");
  if (work.kind === "progress_review") throw new RuleRejection("work_ineligible", "Progress Review work cannot publish a deliverable Candidate");
  if (work.execution_epoch !== state.execution_epoch || (work.status !== "claimed" && work.status !== "completed") || work.owner_member_id !== actor) {
    throw new RuleRejection("not_work_owner", "only the current Work Item owner may submit its Candidate");
  }
  const refs = contributionRefs(payload.contribution_refs, "contribution_refs");
  for (const ref of refs) {
    if (!state.contributions.some((item) => item.contribution_id === ref.contribution_id && item.version === ref.version)) {
      throw new RuleRejection("unknown_contribution", "Candidate references an unknown Contribution version");
    }
  }
  const basis = versionRefs(payload.resource_basis, "resource_basis");
  currentResourceBasis(state, basis);
  ensureCapacity(state.candidates, "Candidate");
  const candidateId = stringValue(payload.candidate_id, "candidate_id");
  const version = state.candidates.filter((item) => item.candidate_id === candidateId).length + 1;
  const candidate: Candidate = {
    candidate_id: candidateId, version, execution_epoch: state.execution_epoch, work_id: work.work_id, work_revision: work.revision,
    author_member_id: actor, contribution_refs: refs, resource_basis: basis, artifact: authorizedArtifact(state, payload.artifact, "artifact"),
    criteria_revision: state.criteria_revision, direction_revision: state.direction_revision,
  };
  return { state: { ...state, candidates: [...state.candidates, candidate] }, event: transitionEvent("candidate_submitted", { candidate_id: candidateId, version, work_id: work.work_id }) };
}

type RequiredCheck = { readonly check_id: string; readonly criterion: string };

function requiredChecks(state: SwarmState): RequiredCheck[] {
  return state.acceptance_criteria.map((criterion, index) => ({ check_id: `criterion-${index + 1}`, criterion }));
}

function exactEvidenceRecord(value: CanonicalJson | undefined, label: string, keys: readonly string[]): CanonicalObject {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new RuleRejection("invalid_check_evidence", `${label} must be an exact object`);
  }
  const item = value as CanonicalObject;
  const actual = Object.keys(item).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    throw new RuleRejection("invalid_check_evidence", `${label} fields do not match the canonical check-evidence reference`);
  }
  return item;
}

function checkEvidenceRefs(
  state: SwarmState,
  value: CanonicalJson | undefined,
  candidate: Candidate,
  checkId: string,
  criterion: string,
): CheckEvidenceRef[] {
  if (!Array.isArray(value) || value.length === 0) {
    throw new RuleRejection("missing_evidence", "a Check requires one canonical evidence artifact reference");
  }
  if (value.length !== 1) {
    throw new RuleRejection("invalid_check_evidence", "a Check records exactly one canonical evidence artifact reference");
  }
  try {
    const item = exactEvidenceRecord(value[0], "evidence_refs[0]", ["artifact", "candidate", "candidate_artifact", "check_id", "criterion", "criteria_revision", "resource_basis"]);
    exactEvidenceRecord(item.artifact, "evidence_refs[0].artifact", ["artifact_id", "digest", "local_path", "media_type"]);
    exactEvidenceRecord(item.candidate_artifact, "evidence_refs[0].candidate_artifact", ["artifact_id", "digest", "local_path", "media_type"]);
    const candidateValue = exactEvidenceRecord(item.candidate, "evidence_refs[0].candidate", ["candidate_id", "version"]);
    const basisItems = objectArray(item.resource_basis, "evidence_refs[0].resource_basis");
    for (const [index, basisItem] of basisItems.entries()) {
      exactEvidenceRecord(basisItem, `evidence_refs[0].resource_basis[${index}]`, ["resource_id", "version"]);
    }
    const evidenceCandidate = candidateRefValue(candidateValue, "evidence_refs[0].candidate");
    const evidenceBasis = versionRefs(item.resource_basis, "evidence_refs[0].resource_basis");
    const artifact = authorizedArtifact(state, item.artifact, "evidence_refs[0].artifact");
    const candidateArtifact = authorizedArtifact(state, item.candidate_artifact, "evidence_refs[0].candidate_artifact");
    if (evidenceCandidate.candidate_id !== candidate.candidate_id || evidenceCandidate.version !== candidate.version ||
      stringValue(item.check_id, "evidence_refs[0].check_id") !== checkId ||
      stringValue(item.criterion, "evidence_refs[0].criterion") !== criterion ||
      integerValue(item.criteria_revision, "evidence_refs[0].criteria_revision", 1) !== state.criteria_revision ||
      !sameVersionRefs(evidenceBasis, candidate.resource_basis) ||
      candidateArtifact.artifact_id !== candidate.artifact.artifact_id ||
      candidateArtifact.digest !== candidate.artifact.digest ||
      candidateArtifact.local_path !== candidate.artifact.local_path ||
      candidateArtifact.media_type !== candidate.artifact.media_type) {
      throw new RuleRejection("invalid_check_evidence", "check evidence must bind the exact Candidate, required Check, criteria and resource basis");
    }
    if (artifact.media_type !== CHECK_EVIDENCE_MEDIA_TYPE || !/^blake3:[0-9a-f]{64}$/u.test(artifact.digest)) {
      throw new RuleRejection("invalid_check_evidence", "check evidence must reference a canonical JSON evidence artifact with an exact BLAKE3 digest");
    }
    return [{ artifact, candidate: evidenceCandidate, candidate_artifact: candidateArtifact, check_id: checkId, criterion, criteria_revision: state.criteria_revision, resource_basis: evidenceBasis }];
  } catch (error) {
    if (error instanceof RuleRejection) throw error;
    throw new RuleRejection("invalid_check_evidence", "check evidence reference is not structurally valid");
  }
}

function recordCheck(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  requireOpen(state);
  expectedRevision(state.criteria_revision, payload, "expected_criteria_revision", "stale_criteria_revision");
  const ref = candidateRefValue(payload.candidate, "candidate");
  const candidate = candidateByRef(state, ref);
  if (candidate.execution_epoch !== state.execution_epoch || candidate.criteria_revision !== state.criteria_revision) {
    throw new RuleRejection("stale_candidate", "Candidate does not bind the current execution and criteria revision");
  }
  const basis = versionRefs(payload.resource_basis, "resource_basis");
  if (!sameVersionRefs(basis, candidate.resource_basis)) throw new RuleRejection("stale_resource_basis", "Check basis must exactly match the Candidate input basis");
  currentResourceBasis(state, basis);
  ensureCapacity(state.checks, "Check");
  const checkId = stringValue(payload.check_id, "check_id");
  const required = requiredChecks(state).find((item) => item.check_id === checkId);
  if (required === undefined) throw new RuleRejection("invalid_check", "check_id is not a canonical Check required by the accepted criteria");
  if (state.checks.some((item) => item.check_id === checkId && item.candidate.candidate_id === ref.candidate_id &&
    item.candidate.version === ref.version && item.criteria_revision === state.criteria_revision)) {
    throw new RuleRejection("duplicate_check", "the required Check already has an immutable outcome for this exact Candidate");
  }
  const evidence = checkEvidenceRefs(state, payload.evidence_refs, candidate, checkId, required.criterion);
  const revision = Math.max(0, ...state.checks.filter((item) => item.check_id === checkId).map((item) => item.revision)) + 1;
  const check: Check = {
    check_id: checkId, revision, criterion: required.criterion, candidate: ref, criteria_revision: state.criteria_revision, resource_basis: basis,
    recorded_by_member_id: actor, status: enumValue(payload.status, ["passed", "failed"], "status"), evidence_refs: evidence,
  };
  return { state: { ...state, checks: [...state.checks, check] }, event: transitionEvent("candidate_check_recorded", { check_id: checkId, revision, status: check.status }) };
}

function recordReview(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  requireOpen(state);
  expectedRevision(state.criteria_revision, payload, "expected_criteria_revision", "stale_criteria_revision");
  const ref = candidateRefValue(payload.candidate, "candidate");
  const candidate = candidateByRef(state, ref);
  if (candidate.execution_epoch !== state.execution_epoch) throw new RuleRejection("stale_candidate", "Candidate belongs to a previous execution epoch");
  if (candidate.author_member_id === actor) throw new RuleRejection("self_review", "Candidate author cannot independently review the same Candidate");
  const basis = versionRefs(payload.resource_basis, "resource_basis");
  if (!sameVersionRefs(basis, candidate.resource_basis)) throw new RuleRejection("stale_resource_basis", "Review basis must exactly match the Candidate input basis");
  currentResourceBasis(state, basis);
  ensureCapacity(state.reviews, "Review");
  const reviewId = stringValue(payload.review_id, "review_id");
  const revision = state.reviews.filter((item) => item.review_id === reviewId).length + 1;
  const verdict = enumValue(payload.verdict, ["passed", "changes_requested", "disputed"], "verdict");
  const rawFindings = objectArray(payload.findings, "findings");
  if (verdict === "changes_requested" && rawFindings.length === 0) throw new RuleRejection("invalid_review", "changes_requested review must record at least one Finding");
  if (state.findings.length + rawFindings.length > 64) throw new RuleRejection("capacity_exhausted", "Finding capacity is exhausted");
  const findings: Finding[] = rawFindings.map((item, index) => {
    const findingId = stringValue(item.finding_id, `findings[${index}].finding_id`);
    if (state.findings.some((existing) => existing.finding_id === findingId) || rawFindings.slice(0, index).some((other) => other.finding_id === findingId)) throw new RuleRejection("duplicate_id", "finding_id is already recorded");
    const corrective = optionalString(item.corrective_work_id, `findings[${index}].corrective_work_id`);
    if (corrective !== null) {
      const correction = workById(state, corrective);
      if (correction.kind !== "correction" || correction.execution_epoch !== state.execution_epoch) throw new RuleRejection("invalid_correction", "corrective Work Item must be current correction work");
    }
    return {
      finding_id: findingId, revision: 1, review_id: reviewId, candidate: ref, criteria_revision: state.criteria_revision, resource_basis: basis,
      severity: enumValue(item.severity, ["blocking", "advisory"], `findings[${index}].severity`), summary: stringValue(item.summary, `findings[${index}].summary`),
      status: verdict === "disputed" ? "disputed" : "unresolved", corrective_work_id: corrective,
    };
  });
  const review: Review = { review_id: reviewId, revision, candidate: ref, reviewer_member_id: actor, verdict, criteria_revision: state.criteria_revision, resource_basis: basis, finding_ids: findings.map((item) => item.finding_id) };
  const next = { ...state, reviews: [...state.reviews, review], findings: [...state.findings, ...findings] };
  // Human-required work remains durable in the Review/Finding facts and is
  // surfaced by the participant projection and TUI. Core Attention Signals
  // launch Agent participants, so a Human coordinator must never be encoded
  // as their target.
  return { state: next, event: transitionEvent("candidate_review_recorded", { review_id: reviewId, revision, verdict }) };
}

function resolveFinding(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  requireOpen(state);
  const findingId = stringValue(payload.finding_id, "finding_id");
  const finding = state.findings.find((item) => item.finding_id === findingId);
  if (finding === undefined) throw new RuleRejection("unknown_finding", "Review Finding is not recorded");
  expectedRevision(finding.revision, payload, "expected_finding_revision", "stale_finding_revision");
  if (finding.status === "resolved") throw new RuleRejection("finding_resolved", "Review Finding is already resolved");
  const review = state.reviews.find((item) => item.review_id === finding.review_id &&
    item.candidate.candidate_id === finding.candidate.candidate_id && item.candidate.version === finding.candidate.version);
  const candidate = candidateByRef(state, finding.candidate);
  if (review === undefined) throw new RuleRejection("invalid_review", "Review Finding has no exact originating Review");
  const independentResolver = actor !== candidate.author_member_id && actor !== review.reviewer_member_id;
  if (actor !== state.human_coordinator_member_id && !independentResolver) {
    throw new RuleRejection("role_violation", "Finding resolution requires the Human coordinator or a participant independent of both author and reviewer");
  }
  const resolution = enumValue(payload.resolution, ["resolved", "disputed"], "resolution");
  const evidence = stringArray(payload.evidence_refs, "evidence_refs", 1);
  if (resolution === "resolved" && finding.corrective_work_id !== null && workById(state, finding.corrective_work_id).status !== "completed") {
    throw new RuleRejection("correction_incomplete", "corrective Work Item must complete before its Finding can resolve");
  }
  const replacement: Finding = { ...finding, revision: finding.revision + 1, status: resolution };
  const next = { ...state, findings: state.findings.map((item) => item.finding_id === findingId ? replacement : item) };
  return { state: next, event: transitionEvent("review_finding_resolved", { evidence_refs: evidence, finding_id: findingId, member_id: actor, resolution }) };
}

function acceptResult(state: SwarmState, payload: CanonicalObject, actor: string, input: CanonicalObject): Transition {
  requireOpen(state);
  expectedRevision(state.direction_revision, payload, "expected_direction_revision", "stale_direction_revision");
  const ref = candidateRefValue(payload.candidate, "candidate");
  const candidate = candidateByRef(state, ref);
  const candidateWork = workById(state, candidate.work_id);
  if (candidateWork.kind !== "integration") {
    throw new RuleRejection("validation_incomplete", "the accepted Candidate must be produced by an integration Work Item");
  }
  const invalidatedByDirection = state.directions.some((item) => item.direction_revision > candidate.direction_revision && item.affected_work_ids.includes(candidate.work_id));
  if (candidate.execution_epoch !== state.execution_epoch || candidate.criteria_revision !== state.criteria_revision || invalidatedByDirection) {
    throw new RuleRejection("stale_candidate", "Candidate is not based on the current execution, criteria and Direction revisions");
  }
  if (state.candidates.some((item) => item.candidate_id === ref.candidate_id && item.version > ref.version)) throw new RuleRejection("stale_candidate", "a newer Candidate version is recorded");
  currentResourceBasis(state, candidate.resource_basis);
  if (state.resource_conflicts.some((item) => item.status === "unresolved" && item.affected_work_ids.includes(candidate.work_id))) {
    throw new RuleRejection("resource_conflict", "Candidate work has an unresolved shared-resource conflict");
  }
  const required = requiredChecks(state);
  const checks = required.map((policy) => state.checks.filter((check) =>
    check.check_id === policy.check_id && check.candidate.candidate_id === ref.candidate_id &&
    check.candidate.version === ref.version && check.criteria_revision === state.criteria_revision));
  if (checks.some((matches) => matches.length !== 1) || checks.some((matches, index) => {
    const check = matches[0];
    const policy = required[index];
    return check === undefined || policy === undefined || check.criterion !== policy.criterion || check.status !== "passed" ||
      !sameVersionRefs(check.resource_basis, candidate.resource_basis);
  })) {
    throw new RuleRejection("validation_incomplete", "every canonical criterion Check must pass exactly once on the current Candidate and evidence basis");
  }
  const exactChecks = checks.map((matches) => matches[0]!);
  for (const check of exactChecks) {
    checkEvidenceRefs(state, check.evidence_refs as unknown as CanonicalJson, candidate, check.check_id, check.criterion);
  }
  const expectedCheckRefs = exactChecks.map((check) => ({ id: check.check_id, revision: check.revision }));
  const checkRefs = entityRefs(payload.check_refs, "check_refs");
  if (checkRefs.length !== expectedCheckRefs.length || checkRefs.some((item, index) => {
    const expected = expectedCheckRefs[index];
    return expected === undefined || item.id !== expected.id || item.revision !== expected.revision;
  })) {
    throw new RuleRejection("validation_incomplete", "check_refs must be the complete canonical criterion-ordered current Check set");
  }
  const reviewRefs = entityRefs(payload.review_refs, "review_refs");
  const reviews = reviewRefs.map((item) => state.reviews.find((review) => review.review_id === item.id && review.revision === item.revision));
  if (reviews.some((item) => item === undefined) || reviews.some((item) => item!.verdict !== "passed" || item!.candidate.candidate_id !== ref.candidate_id || item!.candidate.version !== ref.version || item!.reviewer_member_id === candidate.author_member_id || item!.criteria_revision !== state.criteria_revision || !sameVersionRefs(item!.resource_basis, candidate.resource_basis))) {
    throw new RuleRejection("review_incomplete", "an independent passing Review on the exact Candidate basis is required");
  }
  if (state.findings.some((item) => {
    if (item.severity !== "blocking" || item.status === "resolved") return false;
    const origin = state.candidates.find((recorded) =>
      recorded.candidate_id === item.candidate.candidate_id && recorded.version === item.candidate.version);
    return origin !== undefined && candidatesShareIntegrationLineage(state, candidate, origin);
  })) {
    throw new RuleRejection("blocking_finding", "an unresolved blocking Finding in the Candidate lineage prevents acceptance");
  }
  ensureCapacity(state.results, "Result");
  const resultId = stringValue(payload.result_id, "result_id");
  const version = state.results.filter((item) => item.result_id === resultId).length + 1;
  const result: Result = { result_id: resultId, version, execution_epoch: state.execution_epoch, candidate: ref, check_refs: expectedCheckRefs, review_refs: reviewRefs, criteria_revision: state.criteria_revision, direction_revision: state.direction_revision, resource_basis: candidate.resource_basis, accepted_by_member_id: actor };
  const next: SwarmState = {
    ...state, phase: "completed", outstanding_progress_reviews: [], results: [...state.results, result],
    work_items: state.work_items.map((item) => item.execution_epoch === state.execution_epoch && item.status !== "completed" ? { ...item, revision: item.revision + 1, status: "superseded" } : item),
    work_attempts: state.work_attempts.map((item) => item.execution_epoch === state.execution_epoch && (item.status === "active" || item.status === "blocked") ? { ...item, revision: item.revision + 1, status: "interruption_requested" } : item),
  };
  return { state: next, event: transitionEvent("swarm_result_accepted", { candidate: ref, result_id: resultId, version }), timerRequests: cancelProgressReview(input) };
}

function submitSuggestion(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  requireOpen(state);
  ensureCapacity(state.suggestions, "Suggestion");
  const suggestionId = stringValue(payload.suggestion_id, "suggestion_id");
  if (state.suggestions.some((item) => item.suggestion_id === suggestionId)) throw new RuleRejection("duplicate_id", "suggestion_id is already recorded");
  const scope = enumValue(payload.target_scope, ["goal", "work"], "target_scope");
  const targets = stringArray(payload.target_work_ids, "target_work_ids");
  if (scope === "work" && targets.length === 0) throw new RuleRejection("invalid_target", "work Suggestion requires at least one target Work Item");
  for (const target of targets) workById(state, target);
  const suggestion: Suggestion = { suggestion_id: suggestionId, revision: 1, target_scope: scope, target_work_ids: targets, summary: stringValue(payload.summary, "summary"), proposed_by_member_id: actor, disposition: "pending", disposition_reason: null };
  const next = { ...state, suggestions: [...state.suggestions, suggestion] };
  return { state: next, event: transitionEvent("suggestion_submitted", { suggestion_id: suggestionId }) };
}

function dispositionSuggestion(state: SwarmState, payload: CanonicalObject, actor: string, core: CanonicalObject): Transition {
  requireHuman(state, core, actor);
  requireOpen(state);
  const id = stringValue(payload.suggestion_id, "suggestion_id");
  const suggestion = state.suggestions.find((item) => item.suggestion_id === id);
  if (suggestion === undefined) throw new RuleRejection("unknown_suggestion", "Suggestion is not recorded");
  expectedRevision(suggestion.revision, payload, "expected_suggestion_revision", "stale_suggestion_revision");
  if (suggestion.disposition !== "pending") throw new RuleRejection("suggestion_disposed", "Suggestion already has a disposition");
  const replacement: Suggestion = { ...suggestion, revision: suggestion.revision + 1, disposition: enumValue(payload.disposition, ["accepted", "rejected"], "disposition"), disposition_reason: stringValue(payload.reason, "reason") };
  return { state: { ...state, suggestions: state.suggestions.map((item) => item.suggestion_id === id ? replacement : item) }, event: transitionEvent("suggestion_disposition_recorded", { disposition: replacement.disposition, suggestion_id: id }) };
}

function issueDirection(state: SwarmState, payload: CanonicalObject, actor: string, core: CanonicalObject, input: CanonicalObject): Transition {
  requireHuman(state, core, actor);
  requireOpen(state);
  expectedRevision(state.direction_revision, payload, "expected_direction_revision", "stale_direction_revision");
  ensureCapacity(state.directions, "Direction");
  ensureCapacity(state.blockers, "Blocker");
  const id = stringValue(payload.direction_id, "direction_id");
  if (state.directions.some((item) => item.direction_id === id)) throw new RuleRejection("duplicate_id", "direction_id is already recorded");
  const scope = enumValue(payload.target_scope, ["goal", "work"], "target_scope");
  const targets = scope === "goal"
    ? currentWork(state).filter((item) => item.kind !== "progress_review" && item.status !== "completed" && item.status !== "superseded").map((item) => item.work_id)
    : stringArray(payload.target_work_ids, "target_work_ids", 1);
  for (const target of targets) {
    if (workById(state, target).kind === "progress_review") throw new RuleRejection("invalid_target", "Directions target goal work, not the supervision obligation itself");
  }
  const affected = dependentClosure(state, targets);
  const directionRevision = state.direction_revision + 1;
  const direction: Direction = { direction_id: id, revision: 1, target_scope: scope, target_work_ids: targets, instruction: stringValue(payload.instruction, "instruction"), issued_by_member_id: actor, direction_revision: directionRevision, affected_work_ids: affected };
  const blockerId = `direction:${id}`;
  const blocker: Blocker = { blocker_id: blockerId, revision: 1, execution_epoch: state.execution_epoch, scope, work_id: scope === "work" && targets.length === 1 ? targets[0]! : null, summary: direction.instruction, status: "unresolved", reported_by_member_id: actor, evidence_refs: [] };
  let next: SwarmState = {
    ...state, direction_revision: directionRevision, directions: [...state.directions, direction], blockers: [...state.blockers, blocker],
    work_items: state.work_items.map((item) => affected.includes(item.work_id) && item.status !== "completed" && item.status !== "superseded" ? { ...item, revision: item.revision + 1, status: "blocked", owner_member_id: null, active_attempt_id: null, blocker_ids: [...item.blocker_ids, blockerId], direction_revision: directionRevision } : item),
    work_attempts: state.work_attempts.map((item) => affected.some((workId) => workId === item.work_id) && (item.status === "active" || item.status === "blocked") ? { ...item, revision: item.revision + 1, status: "interruption_requested" } : item),
  };
  const obligation = createProgressObligation(next, scope, affected, "direction");
  next = obligation.state;
  return {
    state: next,
    event: transitionEvent("human_direction_issued", { affected_work_ids: affected, direction_id: id, direction_revision: directionRevision }),
    attention: obligation.review === null ? [] : progressAttention(next, core, obligation.review, `progress:direction:${id}`),
  };
}

function reportProgress(state: SwarmState, payload: CanonicalObject, actor: string, input: CanonicalObject, admittedAt: string): Transition {
  requireOpen(state);
  const reviewId = stringValue(payload.review_id, "review_id");
  const progress = state.outstanding_progress_reviews.find((item) => item.review_id === reviewId);
  if (progress === undefined) throw new RuleRejection("no_progress_review", "the identified Progress Review obligation is not outstanding");
  expectedRevision(progress.revision, payload, "expected_review_revision", "stale_progress_review");
  const work = workById(state, progress.work_id);
  if (work.owner_member_id !== actor || work.active_attempt_id === null) throw new RuleRejection("not_work_owner", "Progress Review must be reported by its authoritative owner");
  const attempt = attemptById(state, work.active_attempt_id);
  const assessment = enumValue(payload.assessment, ["aligned", "realignment_required", "blocked"], "assessment");
  const evidence = stringArray(payload.evidence_refs, "evidence_refs");
  const correctiveIds = stringArray(payload.corrective_work_ids, "corrective_work_ids");
  for (const id of correctiveIds) {
    const corrective = workById(state, id);
    if (corrective.kind !== "correction" || corrective.execution_epoch !== state.execution_epoch) throw new RuleRejection("invalid_correction", "Progress Review corrections must reference current correction Work Items");
  }
  let problems = state.problems;
  if (assessment !== "aligned") {
    const problemId = stringValue(payload.problem_id, "problem_id");
    if (state.problems.some((item) => item.problem_id === problemId)) throw new RuleRejection("duplicate_id", "problem_id is already recorded");
    const problemScope = enumValue(payload.problem_scope, ["goal", "work"], "problem_scope");
    if (problemScope === "work" && progress.target_work_ids.length === 0) throw new RuleRejection("invalid_target", "work-scoped Progress Problem requires identified target work");
    const affectedWorkIds = problemScope === "goal"
      ? currentWork(state).filter((item) => item.kind !== "progress_review").map((item) => item.work_id)
      : [...progress.target_work_ids];
    const repeatsActiveProblem = state.problems.some((item) =>
      item.execution_epoch === state.execution_epoch && item.status !== "resolved" && item.scope === problemScope &&
      (problemScope === "goal" || item.affected_work_ids.some((existingWorkId) =>
        affectedWorkIds.some((workId) => workSharesLineage(state, existingWorkId, workId)))));
    if (repeatsActiveProblem) {
      throw new RuleRejection("invalid_correction", "an unresolved Progress Problem already owns this scope and correction history");
    }
    ensureCapacity(state.problems, "Progress Problem");
    const problem: Problem = { problem_id: problemId, revision: 1, execution_epoch: state.execution_epoch, scope: problemScope, affected_work_ids: affectedWorkIds, summary: stringValue(payload.summary, "summary"), status: "unresolved", failed_corrections: 0, correction_work_ids: correctiveIds, recorded_correction_attempt_ids: [], evidence_refs: evidence };
    problems = [...problems, problem];
  }
  const next: SwarmState = {
    ...replaceWork(state, { ...work, revision: work.revision + 1, status: "completed" }), problems,
    outstanding_progress_reviews: state.outstanding_progress_reviews.filter((item) => item.review_id !== progress.review_id),
    work_attempts: state.work_attempts.map((item) => item.attempt_id === attempt.attempt_id ? { ...item, revision: item.revision + 1, status: "completed" } : item),
  };
  return {
    state: next,
    event: transitionEvent("progress_review_reported", { assessment, review_id: progress.review_id }),
    timerRequests: progress.scope === "goal" ? [scheduleProgressReview(input, next, admittedAt)] : [],
  };
}

type AuthoritativeEvidenceRef = {
  readonly kind: "blocker" | "candidate" | "check" | "contribution" | "finding";
  readonly id: string;
  readonly revision: number;
};

function authoritativeEvidenceRef(value: string): AuthoritativeEvidenceRef | null {
  const match = /^(blocker|candidate|check|contribution|finding):(.+):([1-9][0-9]*)$/u.exec(value);
  if (match === null) return null;
  const revision = Number(match[3]);
  if (!Number.isSafeInteger(revision)) return null;
  return { kind: match[1] as AuthoritativeEvidenceRef["kind"], id: match[2]!, revision };
}

function contributionFromAttempt(state: SwarmState, ref: ContributionRef, attempt: WorkAttempt): boolean {
  return state.contributions.some((item) =>
    item.contribution_id === ref.contribution_id && item.version === ref.version &&
    item.execution_epoch === attempt.execution_epoch && item.work_id === attempt.work_id && item.attempt_id === attempt.attempt_id);
}

function candidateIncludesAttemptContribution(state: SwarmState, candidate: Candidate, attempt: WorkAttempt): boolean {
  return candidate.execution_epoch === attempt.execution_epoch &&
    candidate.contribution_refs.some((ref) => contributionFromAttempt(state, ref, attempt));
}

function usefulCorrectionEvidenceIsAuthoritative(
  state: SwarmState,
  problem: Problem,
  attempt: WorkAttempt,
  evidence: readonly string[],
): boolean {
  if (evidence.length === 0) return false;
  const relevantWorkIds = new Set([...problem.affected_work_ids, ...problem.correction_work_ids]);
  return evidence.every((value) => {
    const ref = authoritativeEvidenceRef(value);
    if (ref === null) return false;
    switch (ref.kind) {
      case "contribution":
        return contributionFromAttempt(state, { contribution_id: ref.id, version: ref.revision }, attempt);
      case "candidate": {
        const candidate = state.candidates.find((item) => item.candidate_id === ref.id && item.version === ref.revision);
        return candidate !== undefined && candidateIncludesAttemptContribution(state, candidate, attempt);
      }
      case "check": {
        const check = state.checks.find((item) => item.check_id === ref.id && item.revision === ref.revision);
        if (check === undefined || check.status !== "passed") return false;
        const candidate = state.candidates.find((item) =>
          item.candidate_id === check.candidate.candidate_id && item.version === check.candidate.version);
        return candidate !== undefined && candidateIncludesAttemptContribution(state, candidate, attempt);
      }
      case "finding": {
        const finding = state.findings.find((item) => item.finding_id === ref.id && item.revision === ref.revision);
        if (finding === undefined || finding.status !== "resolved") return false;
        if (finding.corrective_work_id === attempt.work_id) return true;
        const candidate = state.candidates.find((item) =>
          item.candidate_id === finding.candidate.candidate_id && item.version === finding.candidate.version);
        return candidate !== undefined && candidateIncludesAttemptContribution(state, candidate, attempt);
      }
      case "blocker": {
        const blocker = state.blockers.find((item) => item.blocker_id === ref.id && item.revision === ref.revision);
        return blocker !== undefined && blocker.status === "resolved" && blocker.execution_epoch === problem.execution_epoch &&
          (blocker.scope === "goal" || (blocker.work_id !== null && relevantWorkIds.has(blocker.work_id)));
      }
    }
  });
}

function correctionOutcome(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  requireOpen(state);
  const problemId = stringValue(payload.problem_id, "problem_id");
  const problem = state.problems.find((item) => item.problem_id === problemId);
  if (problem === undefined) throw new RuleRejection("unknown_problem", "Progress Problem is not recorded");
  expectedRevision(problem.revision, payload, "expected_problem_revision", "stale_problem_revision");
  if (problem.status !== "unresolved") throw new RuleRejection("problem_closed", "Progress Problem is no longer accepting correction outcomes");
  const workId = stringValue(payload.work_id, "work_id");
  const work = workById(state, workId);
  if (work.kind !== "correction" || work.status !== "completed" || !problem.correction_work_ids.includes(workId)) throw new RuleRejection("invalid_correction", "outcome must reference completed corrective work assigned to this Problem");
  if (work.active_attempt_id === null) throw new RuleRejection("invalid_correction", "completed corrective work must retain its authoritative Work Attempt");
  const attempt = attemptById(state, work.active_attempt_id);
  if (attempt.work_id !== work.work_id || attempt.execution_epoch !== problem.execution_epoch || attempt.status !== "completed") {
    throw new RuleRejection("invalid_correction", "outcome must bind the exact completed corrective Work Attempt");
  }
  if (problem.recorded_correction_attempt_ids.includes(attempt.attempt_id)) {
    throw new RuleRejection("invalid_correction", "this corrective Work Attempt already has a recorded outcome");
  }
  const outcome = enumValue(payload.outcome, ["useful", "unsuccessful"], "outcome");
  const evidence = stringArray(payload.evidence_refs, "evidence_refs");
  if (outcome === "useful" && !usefulCorrectionEvidenceIsAuthoritative(state, problem, attempt, evidence)) {
    throw new RuleRejection("missing_evidence", "useful correction requires exact goal-relevant authoritative evidence from Pack state");
  }
  const failures = outcome === "unsuccessful" ? problem.failed_corrections + 1 : problem.failed_corrections;
  const status = outcome === "useful" ? "resolved" as const : failures >= state.correction_failure_limit ? "escalated" as const : "unresolved" as const;
  const affected = status === "escalated" ? (problem.scope === "goal" ? currentWork(state).filter((item) => item.kind !== "progress_review").map((item) => item.work_id) : dependentClosure(state, problem.affected_work_ids)) : problem.affected_work_ids;
  const replacement: Problem = {
    ...problem,
    revision: problem.revision + 1,
    failed_corrections: failures,
    status,
    affected_work_ids: affected,
    recorded_correction_attempt_ids: [...problem.recorded_correction_attempt_ids, attempt.attempt_id],
    evidence_refs: [...problem.evidence_refs, ...evidence],
  };
  const next: SwarmState = {
    ...state,
    problems: state.problems.map((item) => item.problem_id === problemId ? replacement : item),
    work_items: status === "escalated" ? state.work_items.map((item) =>
      affected.includes(item.work_id) && item.execution_epoch === state.execution_epoch && item.kind !== "progress_review" && item.status !== "completed" && item.status !== "superseded"
        ? { ...item, revision: item.revision + 1, status: "blocked", owner_member_id: null, active_attempt_id: null }
        : item) : state.work_items,
    work_attempts: status === "escalated" ? state.work_attempts.map((item) =>
      affected.includes(item.work_id) && item.execution_epoch === state.execution_epoch && (item.status === "active" || item.status === "blocked")
        ? { ...item, revision: item.revision + 1, status: "interruption_requested" }
        : item) : state.work_attempts,
  };
  return { state: next, event: transitionEvent("correction_outcome_recorded", { attempt_id: attempt.attempt_id, failed_corrections: failures, member_id: actor, outcome, problem_id: problemId, status }) };
}

function recordResourceVersion(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  if (state.phase === "awaiting_human_confirmation") throw new RuleRejection("invalid_phase", "resources may be recorded only after setup confirmation");
  ensureCapacity(state.resources, "Resource Version");
  const resourceId = stringValue(payload.resource_id, "resource_id");
  const expectedPrevious = integerValue(payload.expected_previous_version, "expected_previous_version");
  const version = integerValue(payload.version, "version", 1);
  const current = state.resources.filter((item) => item.resource_id === resourceId).sort((a, b) => b.version - a.version)[0];
  const currentVersion = current?.version ?? 0;
  if (version <= currentVersion || state.resources.some((item) => item.resource_id === resourceId && item.version === version)) throw new RuleRejection("stale_resource_version", "Resource Version must advance beyond the current recorded version");
  const affected = stringArray(payload.affected_work_ids, "affected_work_ids");
  for (const id of affected) workById(state, id);
  const resource: ResourceVersion = { resource_id: resourceId, version, digest: stringValue(payload.digest, "digest"), local_path: authorizedLocalPath(state, payload.local_path, "local_path"), recorded_by_member_id: actor };
  let conflicts = state.resource_conflicts;
  if (expectedPrevious !== currentVersion) {
    ensureCapacity(conflicts, "Resource Conflict");
    const conflictId = `resource-conflict:${resourceId}:${version}`;
    const conflict: ResourceConflict = { conflict_id: conflictId, revision: 1, resource_id: resourceId, expected_version: expectedPrevious, actual_version: currentVersion, affected_work_ids: dependentClosure(state, affected), evidence_refs: [], status: "unresolved" };
    conflicts = [...conflicts, conflict];
  }
  const next: SwarmState = { ...state, resources: [...state.resources, resource], resource_conflicts: conflicts };
  return { state: next, event: transitionEvent("resource_version_recorded", { conflict: expectedPrevious !== currentVersion, resource_id: resourceId, version }) };
}

function requestWriteback(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  requireOpen(state);
  ensureCapacity(state.writebacks, "Writeback");
  const operationId = stringValue(payload.operation_id, "operation_id");
  if (state.writebacks.some((item) => item.operation_id === operationId)) throw new RuleRejection("duplicate_id", "operation_id is already recorded");
  const candidateRef = candidateRefValue(payload.candidate, "candidate");
  const candidate = candidateByRef(state, candidateRef);
  if (candidate.execution_epoch !== state.execution_epoch) throw new RuleRejection("stale_candidate", "Writeback Candidate belongs to a previous execution epoch");
  const resourceId = stringValue(payload.resource_id, "resource_id");
  const expected = integerValue(payload.expected_resource_version, "expected_resource_version");
  const current = state.resources.filter((item) => item.resource_id === resourceId).sort((a, b) => b.version - a.version)[0]?.version ?? 0;
  if (current !== expected) throw new RuleRejection("stale_resource_basis", "Writeback precondition differs from current Resource Version");
  const writeback: Writeback = { operation_id: operationId, revision: 1, candidate: candidateRef, resource_id: resourceId, expected_version: expected, status: "requested", resulting_version: null, evidence_refs: [] };
  return { state: { ...state, writebacks: [...state.writebacks, writeback] }, event: transitionEvent("writeback_requested", { member_id: actor, operation_id: operationId, resource_id: resourceId }) };
}

function recordWritebackOutcome(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  requireOpen(state);
  const operationId = stringValue(payload.operation_id, "operation_id");
  const writeback = [...state.writebacks].reverse().find((item) => item.operation_id === operationId);
  if (writeback === undefined) throw new RuleRejection("unknown_writeback", "Writeback operation is not recorded");
  expectedRevision(writeback.revision, payload, "expected_writeback_revision", "stale_writeback_revision");
  if (writeback.status !== "requested") throw new RuleRejection("writeback_closed", "Writeback operation already has an outcome");
  ensureCapacity(state.writebacks, "Writeback");
  const status = enumValue(payload.status, ["applied", "conflicted", "uncertain"], "status");
  const resulting = payload.resulting_version === undefined ? null : integerValue(payload.resulting_version, "resulting_version", 1);
  if (status === "applied" && resulting === null) throw new RuleRejection("invalid_writeback", "applied Writeback must record its resulting Resource Version");
  const evidence = stringArray(payload.evidence_refs, "evidence_refs");
  const outcome: Writeback = { ...writeback, revision: writeback.revision + 1, status, resulting_version: resulting, evidence_refs: evidence };
  let conflicts = state.resource_conflicts;
  let blockers = state.blockers;
  const candidate = candidateByRef(state, writeback.candidate);
  if (status === "conflicted") {
    ensureCapacity(conflicts, "Resource Conflict");
    const conflictId = `writeback-conflict:${operationId}`;
    conflicts = [...conflicts, { conflict_id: conflictId, revision: 1, resource_id: writeback.resource_id, expected_version: writeback.expected_version, actual_version: resulting ?? writeback.expected_version, affected_work_ids: dependentClosure(state, [candidate.work_id]), evidence_refs: evidence, status: "unresolved" }];
  } else if (status === "uncertain") {
    ensureCapacity(blockers, "Blocker");
    const blockerId = `writeback-uncertain:${operationId}`;
    blockers = [...blockers, { blocker_id: blockerId, revision: 1, execution_epoch: state.execution_epoch, scope: "work", work_id: candidate.work_id, summary: "Writeback outcome is uncertain and must be reconciled before retry.", status: "unresolved", reported_by_member_id: actor, evidence_refs: evidence }];
  }
  const next: SwarmState = { ...state, blockers, resource_conflicts: conflicts, writebacks: [...state.writebacks, outcome] };
  return { state: next, event: transitionEvent("writeback_outcome_recorded", { operation_id: operationId, status }) };
}

function recordLateContribution(state: SwarmState, payload: CanonicalObject, actor: string): Transition {
  if (state.phase !== "completed") throw new RuleRejection("invalid_phase", "late output is recorded only after completion");
  ensureCapacity(state.late_contributions, "Late Contribution");
  const work = workById(state, stringValue(payload.work_id, "work_id"));
  expectedRevision(work.revision, payload, "expected_work_revision", "stale_work_revision");
  const attempt = attemptById(state, stringValue(payload.attempt_id, "attempt_id"));
  if (attempt.work_id !== work.work_id || attempt.owner_member_id !== actor) throw new RuleRejection("not_work_owner", "late output must retain its original Work Attempt attribution");
  const lateId = stringValue(payload.late_id, "late_id");
  if (state.late_contributions.some((item) => item.late_id === lateId)) throw new RuleRejection("duplicate_id", "late_id is already recorded");
  const late = { late_id: lateId, execution_epoch: attempt.execution_epoch, work_id: work.work_id, attempt_id: attempt.attempt_id, author_member_id: actor, summary: stringValue(payload.summary, "summary"), artifact: authorizedArtifact(state, payload.artifact, "artifact") };
  return { state: { ...state, late_contributions: [...state.late_contributions, late] }, event: transitionEvent("late_contribution_recorded", { late_id: lateId, work_id: work.work_id }) };
}

function reopenSwarm(state: SwarmState, payload: CanonicalObject, actor: string, core: CanonicalObject, input: CanonicalObject, admittedAt: string): Transition {
  requireHuman(state, core, actor);
  if (state.phase !== "completed") throw new RuleRejection("invalid_phase", "only a completed Swarm can be explicitly reopened");
  expectedRevision(state.execution_epoch, payload, "expected_execution_epoch", "stale_execution_epoch");
  const next: SwarmState = { ...state, phase: "open", execution_epoch: state.execution_epoch + 1, outstanding_progress_reviews: [] };
  return { state: next, event: transitionEvent("swarm_reopened", { execution_epoch: next.execution_epoch, reason: stringValue(payload.reason, "reason") }), timerRequests: [scheduleProgressReview(input, next, admittedAt)] };
}

function participantTransition(action: ActionType, state: SwarmState, payload: CanonicalObject, actor: string, core: CanonicalObject, input: CanonicalObject, admittedAt: string): Transition {
  const ACTIONS = actionNames();
  switch (action) {
    case ACTIONS.confirm: return confirmSetup(state, payload, actor, core, input, admittedAt);
    case ACTIONS.proposeWork: return proposeWork(state, payload, actor);
    case ACTIONS.reviseDependencies: return reviseDependencies(state, payload, actor);
    case ACTIONS.claimWork: return claimWork(state, payload, actor, core);
    case ACTIONS.handoffWork: return handoffWork(state, payload, actor, core);
    case ACTIONS.reportBlocker: return reportBlocker(state, payload, actor, input, core);
    case ACTIONS.resolveBlocker: return resolveBlocker(state, payload, actor, core);
    case ACTIONS.submitContribution: return submitContribution(state, payload, actor);
    case ACTIONS.submitCandidate: return submitCandidate(state, payload, actor);
    case ACTIONS.recordCheck: return recordCheck(state, payload, actor);
    case ACTIONS.recordReview: return recordReview(state, payload, actor);
    case ACTIONS.resolveFinding: return resolveFinding(state, payload, actor);
    case ACTIONS.acceptResult: return acceptResult(state, payload, actor, input);
    case ACTIONS.submitSuggestion: return submitSuggestion(state, payload, actor);
    case ACTIONS.dispositionSuggestion: return dispositionSuggestion(state, payload, actor, core);
    case ACTIONS.issueDirection: return issueDirection(state, payload, actor, core, input);
    case ACTIONS.reportProgress: return reportProgress(state, payload, actor, input, admittedAt);
    case ACTIONS.correctionOutcome: return correctionOutcome(state, payload, actor);
    case ACTIONS.updateMemberConfiguration: return updateMemberConfiguration(state, payload, actor, core);
    case ACTIONS.resourceVersion: return recordResourceVersion(state, payload, actor);
    case ACTIONS.requestWriteback: return requestWriteback(state, payload, actor);
    case ACTIONS.writebackOutcome: return recordWritebackOutcome(state, payload, actor);
    case ACTIONS.lateContribution: return recordLateContribution(state, payload, actor);
    case ACTIONS.reopen: return reopenSwarm(state, payload, actor, core, input, admittedAt);
  }
}

function isActionType(value: string): value is ActionType {
  return (Object.values(actionNames()) as readonly string[]).includes(value);
}

function timerTransition(state: SwarmState, stimulus: CanonicalObject, core: CanonicalObject): Transition {
  if (stimulus.timer_id !== PROGRESS_REVIEW_TIMER_ID) throw new RuleRejection("unsupported_stimulus", "unknown Agent Swarm Timer");
  const payload = record(stimulus.canonical_payload, "Timer payload");
  if (payload.timer !== "progress_review") throw new RuleRejection("unsupported_stimulus", "Timer payload does not name progress_review");
  const epoch = integerValue(payload.execution_epoch, "Timer payload execution_epoch", 1);
  if (state.phase !== "open" || epoch !== state.execution_epoch || progressReviewForScope(state, "goal", []) !== undefined) {
    return { state, event: transitionEvent("progress_review_timer_suppressed", { execution_epoch: epoch }) };
  }
  const obligation = createProgressObligation(state, "goal", [], "interval");
  if (obligation.review === null) return { state, event: transitionEvent("progress_review_timer_suppressed", { execution_epoch: epoch }) };
  return {
    state: obligation.state,
    event: transitionEvent("progress_review_due", { review_id: obligation.review.review_id }),
    attention: progressAttention(obligation.state, core, obligation.review, `progress:timer:${obligation.review.review_id}`),
  };
}

export function reduceSwarm(input: CanonicalObject): PackReduceOutput {
  const before = stateValue(input.prior_activity_state);
  const stimulus = record(input.recorded_stimulus, "recorded_stimulus");
  const coreBefore = coreRecord(input.core_before);
  const coreAfter = coreRecord(input.proposed_core_after);
  const nextRoomSeq = integerValue(input.next_room_seq, "next_room_seq", 1);
  try {
    const beforeMembers = enabledParticipants(coreBefore);
    const afterMembers = enabledParticipants(coreAfter);
    const expectedWorkers = before.roster.map((item) => item.member_id).sort();
    if (beforeMembers.coordinator !== before.human_coordinator_member_id || afterMembers.coordinator !== before.human_coordinator_member_id ||
      beforeMembers.workers.join("|") !== expectedWorkers.join("|") || afterMembers.workers.join("|") !== expectedWorkers.join("|")) {
      throw new RuleRejection("core_role_invariant", "selected Human and Worker Membership identities are immutable for this Swarm Room");
    }
    const stimulusType = stringValue(stimulus.stimulus_type, "stimulus_type");
    if (stimulusType === "core_proposed") {
      const next = { ...before, room_seq: nextRoomSeq };
      return { activity_disposition_type: "apply", next_activity_state: stateCanonical(next), ordered_domain_events: [], timer_requests: [], ordered_attention_signals: [] };
    }
    let transition: Transition;
    if (stimulusType === "timer_fired") {
      transition = timerTransition(before, stimulus, coreAfter);
    } else if (stimulusType === "participant_action") {
      ensureCurrentBasis(stimulus, before);
      const actor = stringValue(stimulus.member_id, "member_id");
      actorRole(coreBefore, actor);
      const actionText = stringValue(stimulus.action_type, "action_type");
      if (!isActionType(actionText)) throw new RuleRejection("unsupported_action", "Action type is not declared by Agent Swarm 0.2.0");
      transition = participantTransition(actionText, before, record(stimulus.canonical_payload, "Action payload"), actor, coreBefore, input, stringValue(stimulus.admitted_at, "admitted_at"));
    } else {
      throw new RuleRejection("unsupported_stimulus", "Agent Swarm accepts participant Actions, Core proposals and its progress Timer");
    }
    const next = { ...transition.state, room_seq: nextRoomSeq };
    validateStateSize(next);
    return {
      activity_disposition_type: "apply",
      next_activity_state: stateCanonical(next),
      ordered_domain_events: [transition.event],
      timer_requests: transition.timerRequests ?? [],
      ordered_attention_signals: transition.attention ?? [],
    };
  } catch (error) {
    if (error instanceof RuleRejection) return reject(error);
    throw error;
  }
}
