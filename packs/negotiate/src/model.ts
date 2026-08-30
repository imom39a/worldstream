import { canonicalStringify, type CanonicalJson, type CanonicalObject } from "@worldstream/pack-sdk";

export type Role = "buyer_agent" | "seller_agent" | "buyer_approver" | "venue_signer";
export type Persona = Role | "operator" | "spectator";
export type Phase =
  | "formation_open"
  | "approval_pending"
  | "offer_accepted"
  | "agreement_pending"
  | "withdrawn_pending_expiry"
  | "deadline_resolution"
  | "complete"
  | "expired";
export type DeadlineProgress =
  | "none"
  | "awaiting_session_expiry"
  | "awaiting_transaction_expiry"
  | "awaiting_session_close"
  | "resolved";
export type ActionKind =
  | "submit_proposal_revision"
  | "withdraw_live_proposal"
  | "request_exact_approval"
  | "record_exact_approval"
  | "accept_current_proposal"
  | "select_accepted_proposal"
  | "record_agreement_signature"
  | "commit_agreement"
  | "record_session_deadline_elapsed"
  | "record_transaction_deadline_elapsed"
  | "close_transaction_expired_session";
export type AttentionReason =
  | "proposal_received"
  | "agreement_signature_required"
  | "agreement_ready"
  | "venue_signature_required";
export type RejectionCode =
  | "stale_room_head"
  | "stale_transaction_head"
  | "stale_session_head"
  | "invalid_phase"
  | "role_violation"
  | "wrong_signer"
  | "invalid_signature"
  | "non_canonical_a202_bytes"
  | "byte_mutation"
  | "approval_binding_mismatch"
  | "approval_expired"
  | "proposal_expired"
  | "deadline_reached"
  | "deadline_not_passed"
  | "resource_limit"
  | "core_role_invariant";

export interface RoomHead {
  digest: string;
  sequence: number;
}

export interface LogicalHead {
  event_hash: string;
  sequence: number;
}

export interface ActionBasis {
  room: RoomHead;
  transaction: LogicalHead;
  session: LogicalHead;
}

export interface FixtureSignature {
  signer_id: string;
  signer_role: Role;
  purpose: string;
  signed_wire_digest: string;
  proof: string;
}

export interface ExactA202Object {
  object_id: string;
  object_type: string;
  declared_content_hash: string;
  canonical_json: string;
  wire_digest: string;
  signature: FixtureSignature;
}

export interface Proposal {
  author: Role;
  offer: ExactA202Object;
  session_event: ExactA202Object;
  valid_until: number;
  supersedes_offer_id: string | null;
}

export interface ApprovalBinding {
  candidate_canonical_json: string;
  candidate_wire_digest: string;
  transaction_id: string;
  proposal_id: string;
  proposal_content_hash: string;
  room_head_at_request: RoomHead;
  transaction_head_at_request: LogicalHead;
  session_head_at_request: LogicalHead;
  approver_id: string;
  expires_at: number;
}

export interface ExactApproval {
  binding: ApprovalBinding;
  decision: "approved" | "rejected";
  approval: ExactA202Object;
}

export interface AgreementSignature {
  agreement_id: string;
  agreement_content_hash: string;
  agreement_canonical_json: string;
  agreement_wire_digest: string;
  signer_id: string;
  signer_role: Role;
  proof: string;
}

export type Outcome =
  | {
      kind: "agreement_committed";
      transaction_id: string;
      agreement_id: string;
      agreement_hash: string;
    }
  | {
      kind: "formation_expired";
      transaction_id: string;
      final_session_state: string;
    };

export interface EvidenceLink {
  object_id: string;
  object_type: string;
  content_hash: string;
  wire_digest: string;
  action_id: string;
  room_sequence: number;
  room_digest: string;
  transaction_head: LogicalHead;
  session_head: LogicalHead;
}

export interface NegotiateState {
  oracle_id: string;
  a202_revision: string;
  transaction_id: string;
  session_id: string;
  phase: Phase;
  aggregate_state: string;
  session_state: string;
  room_head: RoomHead;
  transaction_head: LogicalHead;
  session_head: LogicalHead;
  formation_deadline: number;
  proposal_expired: boolean;
  current_proposal: Proposal | null;
  pending_approval: ApprovalBinding | null;
  recorded_approval: ExactApproval | null;
  accepted_offer_id: string | null;
  accepted_offer_hash: string | null;
  agreement_id: string | null;
  agreement_content_hash: string | null;
  agreement_canonical_json: string | null;
  agreement_signatures: Partial<Record<Role, AgreementSignature>>;
  deadline_progress: DeadlineProgress;
  outcome: Outcome | null;
  evidence: EvidenceLink[];
}

export interface Attention {
  target: Role;
  reason: AttentionReason;
}

export interface RuleTransition {
  actionId: string;
  actionKind: ActionKind | null;
  attention: Attention[];
  eventType: string;
  state: NegotiateState;
}

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

export function record(value: CanonicalJson | undefined, label: string): Record<string, CanonicalJson> {
  if (value === null || value === undefined || Array.isArray(value) || typeof value !== "object") {
    throw new TypeError(`${label} must be an object`);
  }
  return value as Record<string, CanonicalJson>;
}

export function stringValue(value: CanonicalJson | undefined, label: string): string {
  if (typeof value !== "string") throw new TypeError(`${label} must be a string`);
  return value;
}

export function integerValue(value: CanonicalJson | undefined, label: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw new TypeError(`${label} must be a safe integer`);
  }
  return value;
}

export function optionalString(value: CanonicalJson | undefined, label: string): string | null {
  if (value === null) return null;
  return stringValue(value, label);
}

export function roleValue(value: CanonicalJson | undefined, label: string): Role {
  const role = stringValue(value, label);
  if (!isRole(role)) throw new TypeError(`${label} is not a Negotiate Role`);
  return role;
}

export function isRole(value: string): value is Role {
  return (
    value === "buyer_agent" ||
    value === "seller_agent" ||
    value === "buyer_approver" ||
    value === "venue_signer"
  );
}

export function stateValue(value: CanonicalJson | undefined): NegotiateState {
  return record(value, "activity state") as unknown as NegotiateState;
}

export function cloneState(state: NegotiateState): NegotiateState {
  return JSON.parse(canonicalStringify(state as unknown as CanonicalJson)) as NegotiateState;
}

export function sameCanonical(left: unknown, right: unknown): boolean {
  return (
    canonicalStringify(left as CanonicalJson) === canonicalStringify(right as CanonicalJson)
  );
}

export function asCanonical(value: unknown): CanonicalObject {
  return value as CanonicalObject;
}
