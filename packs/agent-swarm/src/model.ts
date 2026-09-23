import {
  encodeCanonical,
  type CanonicalJson,
  type CanonicalObject,
} from "@worldstream/pack-sdk";

export const MAX_WORKERS = 16;
export const MAX_ITEMS = 64;
export const MAX_TEXT = 4096;
export const MAX_STATE_BYTES = 262_144;
export const SETUP_REVISION = 2;
export const DEFAULT_PROGRESS_INTERVAL_SECONDS = 300;
export const DEFAULT_CORRECTION_FAILURE_LIMIT = 3;
export const CHECK_EVIDENCE_MEDIA_TYPE = "application/vnd.worldstream.agent-swarm-check-evidence+json;version=1";

export type SwarmPhase = "awaiting_human_confirmation" | "open" | "completed";
export type ConfigStatus = "fixture_unavailable" | "resolution_unreported";
export type WorkKind = "goal" | "integration" | "correction" | "progress_review";
export type WorkStatus = "open" | "claimed" | "blocked" | "completed" | "superseded";

export type RosterEntry = {
  readonly member_key: string;
  readonly label: string;
  readonly provider: string;
  readonly requested_model: string;
  readonly requested_effort: string | null;
  readonly configuration_state: ConfigStatus;
  readonly moving_alias_acknowledged: boolean;
};

export type BoundRosterEntry = RosterEntry & {
  readonly member_id: string;
  readonly configuration_revision: number;
};

export type WorkItem = {
  readonly work_id: string;
  readonly revision: number;
  readonly execution_epoch: number;
  readonly kind: WorkKind;
  readonly title: string;
  readonly description: string;
  readonly dependency_ids: readonly string[];
  readonly status: WorkStatus;
  readonly owner_member_id: string | null;
  readonly active_attempt_id: string | null;
  readonly blocker_ids: readonly string[];
  readonly direction_revision: number;
  readonly created_by_member_id: string;
  readonly supersedes_work_id: string | null;
};

export type WorkAttempt = {
  readonly attempt_id: string;
  readonly revision: number;
  readonly work_id: string;
  readonly execution_epoch: number;
  readonly owner_member_id: string;
  readonly status: "active" | "blocked" | "completed" | "interruption_requested" | "fenced";
};

export type WorkHandoff = {
  readonly handoff_id: string;
  readonly revision: number;
  readonly execution_epoch: number;
  readonly work_id: string;
  readonly work_revision: number;
  readonly from_attempt_id: string;
  readonly from_member_id: string;
  readonly to_attempt_id: string;
  readonly to_member_id: string;
  readonly authorized_by_member_id: string;
  readonly reconciliation: "confirmed_terminal" | "effects_reconciled";
  readonly evidence_refs: readonly string[];
  readonly reason: string;
  readonly direction_revision: number;
};

export type Blocker = {
  readonly blocker_id: string;
  readonly revision: number;
  readonly execution_epoch: number;
  readonly scope: "goal" | "work";
  readonly work_id: string | null;
  readonly summary: string;
  readonly status: "unresolved" | "resolved";
  readonly reported_by_member_id: string;
  readonly evidence_refs: readonly string[];
};

export type Contribution = {
  readonly contribution_id: string;
  readonly version: number;
  readonly execution_epoch: number;
  readonly work_id: string;
  readonly work_revision: number;
  readonly attempt_id: string;
  readonly author_member_id: string;
  readonly summary: string;
  readonly source_refs: readonly string[];
  readonly resource_basis: readonly VersionRef[];
  readonly artifact: ArtifactRef;
  readonly direction_revision: number;
};

export type VersionRef = { readonly resource_id: string; readonly version: number };
export type ContributionRef = { readonly contribution_id: string; readonly version: number };
export type CandidateRef = { readonly candidate_id: string; readonly version: number };

export type ArtifactRef = {
  readonly artifact_id: string;
  readonly media_type: string;
  readonly local_path: string;
  readonly digest: string;
};

export type CheckEvidenceRef = {
  readonly artifact: ArtifactRef;
  readonly candidate: CandidateRef;
  readonly candidate_artifact: ArtifactRef;
  readonly check_id: string;
  readonly criterion: string;
  readonly criteria_revision: number;
  readonly resource_basis: readonly VersionRef[];
};

export type Candidate = {
  readonly candidate_id: string;
  readonly version: number;
  readonly execution_epoch: number;
  readonly work_id: string;
  readonly work_revision: number;
  readonly author_member_id: string;
  readonly contribution_refs: readonly ContributionRef[];
  readonly resource_basis: readonly VersionRef[];
  readonly artifact: ArtifactRef;
  readonly criteria_revision: number;
  readonly direction_revision: number;
};

export type Check = {
  readonly check_id: string;
  readonly revision: number;
  readonly criterion: string;
  readonly candidate: CandidateRef;
  readonly criteria_revision: number;
  readonly resource_basis: readonly VersionRef[];
  readonly recorded_by_member_id: string;
  readonly status: "passed" | "failed";
  readonly evidence_refs: readonly CheckEvidenceRef[];
};

export type Finding = {
  readonly finding_id: string;
  readonly revision: number;
  readonly review_id: string;
  readonly candidate: CandidateRef;
  readonly criteria_revision: number;
  readonly resource_basis: readonly VersionRef[];
  readonly severity: "blocking" | "advisory";
  readonly summary: string;
  readonly status: "unresolved" | "resolved" | "disputed";
  readonly corrective_work_id: string | null;
};

export type Review = {
  readonly review_id: string;
  readonly revision: number;
  readonly candidate: CandidateRef;
  readonly reviewer_member_id: string;
  readonly verdict: "passed" | "changes_requested" | "disputed";
  readonly criteria_revision: number;
  readonly resource_basis: readonly VersionRef[];
  readonly finding_ids: readonly string[];
};

export type Result = {
  readonly result_id: string;
  readonly version: number;
  readonly execution_epoch: number;
  readonly candidate: CandidateRef;
  readonly check_refs: readonly EntityRevisionRef[];
  readonly review_refs: readonly EntityRevisionRef[];
  readonly criteria_revision: number;
  readonly direction_revision: number;
  readonly resource_basis: readonly VersionRef[];
  readonly accepted_by_member_id: string;
};

export type EntityRevisionRef = { readonly id: string; readonly revision: number };

export type Suggestion = {
  readonly suggestion_id: string;
  readonly revision: number;
  readonly target_scope: "goal" | "work";
  readonly target_work_ids: readonly string[];
  readonly summary: string;
  readonly proposed_by_member_id: string;
  readonly disposition: "pending" | "accepted" | "rejected";
  readonly disposition_reason: string | null;
};

export type Direction = {
  readonly direction_id: string;
  readonly revision: number;
  readonly target_scope: "goal" | "work";
  readonly target_work_ids: readonly string[];
  readonly instruction: string;
  readonly issued_by_member_id: string;
  readonly direction_revision: number;
  readonly affected_work_ids: readonly string[];
};

export type ProgressReview = {
  readonly review_id: string;
  readonly revision: number;
  readonly execution_epoch: number;
  readonly scope: "goal" | "work";
  readonly target_work_ids: readonly string[];
  readonly work_id: string;
  readonly status: "due" | "claimed" | "blocked";
  readonly reason: "interval" | "blocker" | "direction";
};

export type Problem = {
  readonly problem_id: string;
  readonly revision: number;
  readonly execution_epoch: number;
  readonly scope: "goal" | "work";
  readonly affected_work_ids: readonly string[];
  readonly summary: string;
  readonly status: "unresolved" | "resolved" | "escalated";
  readonly failed_corrections: number;
  readonly correction_work_ids: readonly string[];
  readonly recorded_correction_attempt_ids: readonly string[];
  readonly evidence_refs: readonly string[];
};

export type ResourceVersion = {
  readonly resource_id: string;
  readonly version: number;
  readonly digest: string;
  readonly local_path: string;
  readonly recorded_by_member_id: string;
};

export type ResourceConflict = {
  readonly conflict_id: string;
  readonly revision: number;
  readonly resource_id: string;
  readonly expected_version: number;
  readonly actual_version: number;
  readonly affected_work_ids: readonly string[];
  readonly evidence_refs: readonly string[];
  readonly status: "unresolved" | "resolved";
};

export type Writeback = {
  readonly operation_id: string;
  readonly revision: number;
  readonly candidate: CandidateRef;
  readonly resource_id: string;
  readonly expected_version: number;
  readonly status: "requested" | "applied" | "conflicted" | "uncertain";
  readonly resulting_version: number | null;
  readonly evidence_refs: readonly string[];
};

export type LateContribution = {
  readonly late_id: string;
  readonly execution_epoch: number;
  readonly work_id: string;
  readonly attempt_id: string;
  readonly author_member_id: string;
  readonly summary: string;
  readonly artifact: ArtifactRef;
};

export type SwarmState = {
  readonly phase: SwarmPhase;
  readonly room_seq: number;
  readonly execution_epoch: number;
  readonly goal: string;
  readonly goal_revision: number;
  readonly constraints: readonly string[];
  readonly acceptance_criteria: readonly string[];
  readonly criteria_revision: number;
  readonly direction_revision: number;
  readonly approved_working_area: { readonly root: string; readonly resource_paths: readonly string[] };
  readonly human_coordinator_member_id: string;
  readonly roster: readonly BoundRosterEntry[];
  readonly member_notices: Readonly<Record<string, string>>;
  readonly setup_revision: number;
  readonly confirmed_by_member_id: string | null;
  readonly confirmed_at_room_seq: number | null;
  readonly progress_review_interval_seconds: number;
  readonly correction_failure_limit: number;
  readonly progress_review_sequence: number;
  readonly outstanding_progress_reviews: readonly ProgressReview[];
  readonly work_items: readonly WorkItem[];
  readonly work_attempts: readonly WorkAttempt[];
  readonly handoffs: readonly WorkHandoff[];
  readonly blockers: readonly Blocker[];
  readonly contributions: readonly Contribution[];
  readonly candidates: readonly Candidate[];
  readonly checks: readonly Check[];
  readonly reviews: readonly Review[];
  readonly findings: readonly Finding[];
  readonly results: readonly Result[];
  readonly suggestions: readonly Suggestion[];
  readonly directions: readonly Direction[];
  readonly problems: readonly Problem[];
  readonly resources: readonly ResourceVersion[];
  readonly resource_conflicts: readonly ResourceConflict[];
  readonly writebacks: readonly Writeback[];
  readonly late_contributions: readonly LateContribution[];
};

export type SwarmConfiguration = {
  readonly goal: string;
  readonly constraints: readonly string[];
  readonly acceptance_criteria: readonly string[];
  readonly approved_working_area: { readonly root: string; readonly resource_paths: readonly string[] };
  readonly roster: readonly RosterEntry[];
  readonly progress_review_interval_seconds: number;
  readonly correction_failure_limit: number;
};

export class RuleRejection extends Error {
  constructor(readonly code: string, message: string) { super(message); }
}

export function record(value: CanonicalJson | undefined, label: string): CanonicalObject {
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new TypeError(`${label} must be an object`);
  return value as CanonicalObject;
}

export function stringValue(value: CanonicalJson | undefined, label: string): string {
  if (typeof value !== "string" || value.length < 1 || value.length > MAX_TEXT) throw new TypeError(`${label} must be a non-empty bounded string`);
  return value;
}

export function integerValue(value: CanonicalJson | undefined, label: string, minimum = 0): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < minimum) throw new TypeError(`${label} must be a safe integer of at least ${minimum}`);
  return value;
}

export function booleanValue(value: CanonicalJson | undefined, label: string): boolean {
  if (typeof value !== "boolean") throw new TypeError(`${label} must be a boolean`);
  return value;
}

export function stringArray(value: CanonicalJson | undefined, label: string, minimum = 0): string[] {
  if (!Array.isArray(value) || value.length < minimum || value.length > MAX_ITEMS) throw new TypeError(`${label} must be a bounded array`);
  const result = value.map((item, index) => stringValue(item, `${label}[${index}]`));
  if (new Set(result).size !== result.length) throw new TypeError(`${label} must not contain duplicates`);
  return result;
}

export function objectArray(value: CanonicalJson | undefined, label: string): CanonicalObject[] {
  if (!Array.isArray(value) || value.length > MAX_ITEMS) throw new TypeError(`${label} must be a bounded array`);
  return value.map((item, index) => record(item, `${label}[${index}]`));
}

export function optionalString(value: CanonicalJson | undefined, label: string): string | null {
  return value === undefined || value === null ? null : stringValue(value, label);
}

export function enumValue<T extends string>(value: CanonicalJson | undefined, choices: readonly T[], label: string): T {
  if (typeof value !== "string" || !choices.includes(value as T)) throw new TypeError(`${label} is invalid`);
  return value as T;
}

function configStatus(value: CanonicalJson | undefined, label: string): ConfigStatus {
  return enumValue(value, ["fixture_unavailable", "resolution_unreported"], label);
}

export function modelNeedsMovingAliasAcknowledgement(model: string): boolean {
  const normalized = model.toLowerCase();
  if (/(?:^|[-_.:])(auto|default|latest)(?:$|[-_.:])/u.test(normalized)) return true;
  return !/(?:^|[-_.:])v?\d+(?:[._-]\d+)*(?:$|[-_.:])/u.test(normalized);
}

function rosterEntry(value: CanonicalJson | undefined, label: string): RosterEntry {
  const item = record(value, label);
  const requestedModel = stringValue(item.requested_model, `${label}.requested_model`);
  const aliasAcknowledged = booleanValue(item.moving_alias_acknowledged, `${label}.moving_alias_acknowledged`);
  if (modelNeedsMovingAliasAcknowledgement(requestedModel) && !aliasAcknowledged) {
    throw new TypeError(`${label}.moving_alias_acknowledged must explicitly acknowledge a moving or unpinned requested_model`);
  }
  return {
    member_key: stringValue(item.member_key, `${label}.member_key`),
    label: stringValue(item.label, `${label}.label`),
    provider: stringValue(item.provider, `${label}.provider`),
    requested_model: requestedModel,
    requested_effort: optionalString(item.requested_effort, `${label}.requested_effort`),
    configuration_state: configStatus(item.configuration_state, `${label}.configuration_state`),
    moving_alias_acknowledged: aliasAcknowledged,
  };
}

export function configurationValue(value: CanonicalJson | undefined): SwarmConfiguration {
  const configuration = record(value, "configuration");
  const rosterValue = configuration.roster;
  if (!Array.isArray(rosterValue) || rosterValue.length < 1 || rosterValue.length > MAX_WORKERS) throw new TypeError("configuration.roster must contain one to sixteen Workers");
  const roster = rosterValue.map((entry, index) => rosterEntry(entry, `configuration.roster[${index}]`));
  if (new Set(roster.map((entry) => entry.member_key)).size !== roster.length) throw new TypeError("configuration roster member_key values must be unique");
  const area = record(configuration.approved_working_area, "configuration.approved_working_area");
  const interval = configuration.progress_review_interval_seconds === undefined
    ? DEFAULT_PROGRESS_INTERVAL_SECONDS
    : integerValue(configuration.progress_review_interval_seconds, "configuration.progress_review_interval_seconds", 1);
  const limit = configuration.correction_failure_limit === undefined
    ? DEFAULT_CORRECTION_FAILURE_LIMIT
    : integerValue(configuration.correction_failure_limit, "configuration.correction_failure_limit", 1);
  return {
    goal: stringValue(configuration.goal, "configuration.goal"),
    constraints: stringArray(configuration.constraints, "configuration.constraints"),
    acceptance_criteria: stringArray(configuration.acceptance_criteria, "configuration.acceptance_criteria", 1),
    approved_working_area: {
      root: stringValue(area.root, "configuration.approved_working_area.root"),
      resource_paths: stringArray(area.resource_paths, "configuration.approved_working_area.resource_paths"),
    },
    roster,
    progress_review_interval_seconds: interval,
    correction_failure_limit: limit,
  };
}

export function stateValue(value: CanonicalJson | undefined): SwarmState {
  const state = record(value, "Activity State");
  const phase = enumValue(state.phase, ["awaiting_human_confirmation", "open", "completed"], "Activity State phase");
  for (const key of ["roster", "outstanding_progress_reviews", "work_items", "work_attempts", "handoffs", "blockers", "contributions", "candidates", "checks", "reviews", "findings", "results", "suggestions", "directions", "problems", "resources", "resource_conflicts", "writebacks", "late_contributions"] as const) {
    if (!Array.isArray(state[key]) || state[key].length > MAX_ITEMS) throw new TypeError(`Activity State ${key} must be a bounded array`);
  }
  if (state.setup_revision !== SETUP_REVISION) throw new TypeError(`Activity State setup_revision must be ${SETUP_REVISION}`);
  const parsed = {
    ...cloneCanonical(state),
    confirmed_at_room_seq: state.confirmed_at_room_seq ?? null,
    confirmed_by_member_id: state.confirmed_by_member_id ?? null,
  } as unknown as SwarmState;
  if (phase === "completed" && parsed.results.length === 0) throw new TypeError("completed Activity State must retain an accepted result");
  validateStateSize(parsed);
  return parsed;
}

export function stateCanonical(state: SwarmState): CanonicalObject {
  const result = cloneCanonical(state as unknown as CanonicalObject) as Record<string, CanonicalJson>;
  if (state.confirmed_at_room_seq === null) delete result.confirmed_at_room_seq;
  if (state.confirmed_by_member_id === null) delete result.confirmed_by_member_id;
  result.roster = state.roster.map((entry) => ({
    configuration_revision: entry.configuration_revision,
    configuration_state: entry.configuration_state,
    label: entry.label,
    member_id: entry.member_id,
    member_key: entry.member_key,
    moving_alias_acknowledged: entry.moving_alias_acknowledged,
    provider: entry.provider,
    ...(entry.requested_effort === null ? {} : { requested_effort: entry.requested_effort }),
    requested_model: entry.requested_model,
  }));
  return result;
}

/** Clones canonical data without relying on host-only structured-clone globals. */
export function cloneCanonical<T extends CanonicalJson>(value: T): T {
  if (Array.isArray(value)) return value.map((item) => cloneCanonical(item)) as unknown as T;
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, cloneCanonical(item)])) as T;
  }
  return value;
}

export function validateStateSize(state: SwarmState): void {
  if (encodeCanonical(state as unknown as CanonicalJson).length > MAX_STATE_BYTES) {
    throw new RuleRejection("capacity_exhausted", "bounded Agent Swarm Activity State exceeds the Pack limit");
  }
}

export function ensureCapacity(items: readonly unknown[], label: string): void {
  if (items.length >= MAX_ITEMS) throw new RuleRejection("capacity_exhausted", `${label} capacity is exhausted`);
}

export function coreRecord(value: CanonicalJson | undefined): CanonicalObject {
  return record(value, "Core Room State");
}

export function enabledParticipants(core: CanonicalObject, allowNoWorkers = false): { coordinator: string; workers: string[] } {
  const coordinator: string[] = [];
  const workers: string[] = [];
  for (const [memberId, raw] of Object.entries(record(core.memberships, "Core memberships"))) {
    const membership = record(raw, "Core membership");
    if (membership.standing !== "enabled" || membership.access_mode !== "participant") continue;
    if (membership.role === "human_coordinator" && membership.principal_kind === "human") coordinator.push(memberId);
    else if (membership.role === "worker" && membership.principal_kind === "agent") workers.push(memberId);
    else if (membership.role === "human_coordinator" || membership.role === "worker") {
      throw new RuleRejection("core_role_invariant", "Swarm Roles must retain their declared Principal kind and participant access");
    }
  }
  coordinator.sort();
  workers.sort();
  if (coordinator.length !== 1 || (!allowNoWorkers && workers.length < 1) || workers.length > MAX_WORKERS) {
    throw new RuleRejection("core_role_invariant", "Agent Swarm requires exactly one Human coordinator and no more than sixteen Agent Workers");
  }
  return { coordinator: coordinator[0]!, workers };
}

export function roleFor(core: CanonicalObject, memberId: string): "human_coordinator" | "worker" | null {
  const value = record(core.memberships, "Core memberships")[memberId];
  if (value === undefined) return null;
  const membership = record(value, "Core membership");
  if (membership.standing !== "enabled" || membership.access_mode !== "participant") return null;
  return membership.role === "human_coordinator" || membership.role === "worker" ? membership.role : null;
}

export function ensureCurrentBasis(stimulus: CanonicalObject, state: SwarmState): void {
  if (stimulus.exact_basis_head === undefined) throw new RuleRejection("stale_room_head", "Action must bind the current exact Room Head");
  const roomSeq = integerValue(record(stimulus.exact_basis_head, "exact_basis_head").room_seq, "exact_basis_head.room_seq");
  if (roomSeq !== state.room_seq) throw new RuleRejection("stale_room_head", "Action basis does not match the current exact Room Head");
}
