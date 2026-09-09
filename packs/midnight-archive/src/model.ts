import {
  canonicalStringify,
  type CanonicalJson,
  type CanonicalObject,
} from "@worldstream/pack-sdk";

export type Location = "atrium" | "records" | "conservation" | "plant" | "vault";

export type CandidateId = "ledger-amber" | "ledger-cobalt" | "ledger-violet";
export type CandidateReference = CandidateId | "none";

export type Role = "lead" | "mira" | "jonah";
export type Phase = "briefing" | "active" | "complete";
export type OutcomeKind = "pending" | "success" | "wrong_ledger" | "no_ledger" | "exhausted_inside";
export type StagedKind =
  | "none"
  | "move"
  | "use_verifier"
  | "open_service_hatch"
  | "recover_candidate"
  | "extract"
  | "wait";

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
