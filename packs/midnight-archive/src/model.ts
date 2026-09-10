import {
  canonicalStringify,
  type CanonicalJson,
  type CanonicalObject,
} from "@worldstream/pack-sdk";

export type Location = "atrium" | "records" | "conservation" | "plant" | "vault";

export type CandidateId = "ledger-amber" | "ledger-cobalt" | "ledger-violet";
export type CandidateReference = CandidateId | "none";
export type EvidenceSourceId = "records" | "conservation";

export type Role = "lead" | "mira" | "jonah";
export type CompanionRole = "mira" | "jonah";
export type Phase = "briefing" | "active" | "complete";
export type OutcomeKind = "pending" | "success" | "partial_extraction" | "wrong_ledger" | "no_ledger" | "exhausted_inside";
export type StagedKind =
  | "none"
  | "move"
  | "inspect_records"
  | "inspect_conservation"
  | "use_verifier"
  | "accept_preservation_agreement"
  | "prepare_collection"
  | "energize_preservation_equipment"
  | "open_service_hatch"
  | "recover_candidate"
  | "protect_source_record"
  | "extract"
  | "wait";

export type PreservationAgreementStatus = "offered" | "accepted";
export type CollectionPreservationStatus = "unprepared" | "prepared" | "preserved";
export type MiraPresence = "absent" | "active" | "suspended";
export type MiraMode = "unavailable" | "following" | "holding" | "tasked" | "regrouping";
export type MiraTaskKind = "none" | "investigate_records" | "investigate_conservation" | "field_assay" | "open_service_hatch";
export type MiraTaskStatus = "none" | "assigned" | "complete" | "cancelled";
export type MiraPlanningStatus = "not_requested" | "waiting" | "ready" | "expired" | "complete";
export type MiraKnowledgeStatus = "unknown" | "private" | "shared";
export type MiraStepType = "move" | "inspect_source" | "share_source" | "use_verifier" | "collect_assay_sample" | "complete_field_assay" | "open_service_hatch";
export type MiraContributionKind = MiraStepType | "follow_move" | "regroup_move" | "none";

export interface MiraPlanStep {
  readonly step_type: MiraStepType;
  readonly destination: Location | "none";
  readonly source_id: EvidenceSourceId | "none";
  readonly power_cost: 0 | 1 | 2;
}

export interface MiraState {
  readonly presence: MiraPresence;
  readonly member_id: string;
  readonly location: Location | "none";
  readonly mode: MiraMode;
  readonly task: {
    readonly status: MiraTaskStatus;
    readonly revision: number;
    readonly kind: MiraTaskKind;
    readonly power_allowance: 0 | 1 | 2;
    readonly power_spent: 0 | 1 | 2;
  };
  readonly opportunity: {
    readonly status: "none" | "open" | "expired" | "fulfilled";
    readonly revision: number;
    readonly task_revision: number;
    readonly opened_at: string;
    readonly deadline: string;
  };
  readonly plan: {
    readonly status: "none" | "active" | "complete";
    readonly revision: number;
    readonly origin_member_id: string;
    readonly origin_task_revision: number;
    readonly origin_opportunity_revision: number;
    readonly steps: readonly MiraPlanStep[];
    readonly next_step_index: number;
  };
  readonly preparation: {
    readonly status: "none" | "prepared" | "deferred";
    readonly for_turn: number;
    readonly task_revision: number;
    readonly plan_revision: number;
    readonly step_index: number;
    readonly summary: string;
  };
  readonly knowledge: {
    readonly records: MiraKnowledgeStatus;
    readonly conservation: MiraKnowledgeStatus;
    readonly verifier_result: CandidateReference;
  };
  readonly last_contribution: {
    readonly turn: number;
    readonly kind: MiraContributionKind;
    readonly summary: string;
  };
  readonly field_assay: { readonly steps_completed: number; readonly result: CandidateReference };
}

export interface ExtractionPreview {
  readonly status: "none" | "prepared" | "acknowledged";
  readonly revision: number;
  readonly for_turn: number;
  readonly contribution_fingerprint: string;
  readonly extracted_roles: readonly Role[];
  readonly left_behind_roles: readonly CompanionRole[];
}

export interface CompletedCrewWork {
  readonly role: CompanionRole;
  readonly kind: MiraContributionKind;
  readonly turn: number;
}

export interface VisibleCandidate {
  readonly candidate_id: CandidateId;
  readonly binding: "calfskin" | "linen";
  readonly marking: "compass_rose" | "split_star";
  readonly year: 1891 | 1904;
}

export interface StagedAction {
  readonly kind: StagedKind;
  readonly destination: Location | "none";
  readonly candidate_id: CandidateReference;
  readonly turn_cost: 0 | 1;
  readonly power_cost: 0 | 1 | 2;
}

export interface ArchiveOutcome {
  readonly kind: OutcomeKind;
  readonly factual_reason: string;
  readonly extracted_candidate_id: CandidateReference;
}

export interface ArchiveState {
  readonly phase: Phase;
  readonly scenario_id: "standard-v1";
  readonly objective: string;
  readonly location: Location;
  readonly turn_limit: 16;
  readonly turns_used: number;
  readonly power_remaining: number;
  readonly gates: {
    readonly conservation_vault_open: boolean;
    readonly plant_vault_open: boolean;
  };
  readonly candidates: readonly VisibleCandidate[];
  readonly evidence: Readonly<Record<EvidenceSourceId, "unknown" | "observed">>;
  readonly preservation_agreement: PreservationAgreementStatus;
  readonly collection_preservation: CollectionPreservationStatus;
  readonly source_record_protected: boolean;
  readonly mira: MiraState;
  readonly jonah: MiraState;
  readonly starting_crew: readonly { readonly role: Role; readonly member_id: string }[];
  readonly extraction: ExtractionPreview;
  readonly completed_crew_work: readonly CompletedCrewWork[];
  readonly truth_marker: CandidateId;
  readonly verifier_result: CandidateReference;
  readonly carried_candidate_id: CandidateReference;
  readonly carried_confidence: "none" | "unverified" | "verified";
  readonly staged_action: StagedAction;
  readonly outcome: ArchiveOutcome;
  readonly role_notes: Readonly<Record<Role, string>>;
}

export type RejectionCode =
  | "inactive"
  | "role_violation"
  | "invalid_payload"
  | "illegal_action"
  | "nothing_staged"
  | "insufficient_power"
  | "gate_closed"
  | "unknown_candidate"
  | "companion_unavailable"
  | "stale_plan"
  | "task_violation"
  | "plan_invalid"
  | "preparation_required"
  | "crew_not_regrouped"
  | "turn_conflict"
  | "extraction_preview_required"
  | "extraction_preview_stale"
  | "core_role_invariant";

export class RuleRejection extends Error {
  readonly code: RejectionCode;

  constructor(code: RejectionCode, message: string) {
    super(message);
    this.name = "RuleRejection";
    this.code = code;
  }
}

export function reject(code: RejectionCode, message: string): never {
  throw new RuleRejection(code, message);
}

export function record(value: CanonicalJson | undefined, label: string): CanonicalObject {
  if (value === null || value === undefined || Array.isArray(value) || typeof value !== "object") {
    throw new TypeError(`${label} must be an object`);
  }
  return value as CanonicalObject;
}

export function stateValue(value: CanonicalJson | undefined): ArchiveState {
  return record(value, "Activity State") as unknown as ArchiveState;
}

export function cloneState(state: ArchiveState): ArchiveState {
  return JSON.parse(canonicalStringify(state as unknown as CanonicalJson)) as ArchiveState;
}

export function stringValue(value: CanonicalJson | undefined, label: string): string {
  if (typeof value !== "string") throw new TypeError(`${label} must be a string`);
  return value;
}

export function isLocation(value: string): value is Location {
  return value === "atrium" || value === "records" || value === "conservation" ||
    value === "plant" || value === "vault";
}

export function isCandidateId(value: string): value is CandidateId {
  return value === "ledger-amber" || value === "ledger-cobalt" || value === "ledger-violet";
}

export function asCanonical(value: unknown): CanonicalObject {
  return value as CanonicalObject;
}
