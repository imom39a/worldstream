import type { CanonicalJson, CanonicalObject } from "@worldstream/pack-sdk";

export type Role = "analyst" | "reviewer";
export type WorkStatus = "needs_assessment" | "resolved";
export type AssessmentValidity = "current" | "superseded";
export type AssessmentClaim = "connection_feasible" | "connection_at_risk";

export interface SourceFact {
  readonly connection_id: string;
  readonly arrival_version: number;
  readonly arrival_minute: number;
  readonly departure_version: number;
  readonly departure_minute: number;
  readonly minimum_transfer_minutes: number;
}

export interface Assessment {
  readonly assessor_member_id: string;
  readonly claim: AssessmentClaim;
  readonly validity: AssessmentValidity;
  readonly arrival_version: number;
  readonly departure_version: number;
  readonly reason_code: string;
}

export interface WorkItem {
  readonly work_id: string;
  readonly revision: number;
  readonly connection_id: string;
  readonly status: WorkStatus;
  readonly reason: "initial" | "arrival_changed" | "departure_changed";
  /** Omitted until the first assessment; once present it may be superseded. */
  readonly last_assessment?: Assessment;
}

export interface ReferenceState {
  readonly phase: "monitoring";
  readonly room_seq: number;
  readonly objective: string;
  readonly max_open_work: number;
  readonly connections: readonly SourceFact[];
  readonly open_work: readonly WorkItem[];
  readonly role_guidance: Readonly<Record<Role, string>>;
}

export function asRecord(value: CanonicalJson | undefined, label: string): CanonicalObject {
  if (value === undefined || value === null || Array.isArray(value) || typeof value !== "object") {
    throw new TypeError(`${label} must be an object`);
  }
  return value as CanonicalObject;
}

export function stringValue(value: CanonicalJson | undefined, label: string): string {
  if (typeof value !== "string" || value.length === 0) throw new TypeError(`${label} must be a non-empty string`);
  return value;
}

export function integerValue(value: CanonicalJson | undefined, label: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) throw new TypeError(`${label} must be a safe integer`);
  return value;
}

export function stateValue(value: CanonicalJson | undefined): ReferenceState {
  const state = asRecord(value, "Activity State");
  return state as unknown as ReferenceState;
}

export function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

export function connectionValue(value: CanonicalJson | undefined, label: string): SourceFact {
  const source = asRecord(value, label);
  return {
    arrival_minute: integerValue(source.arrival_minute, `${label}.arrival_minute`),
    arrival_version: integerValue(source.arrival_version, `${label}.arrival_version`),
    connection_id: stringValue(source.connection_id, `${label}.connection_id`),
    departure_minute: integerValue(source.departure_minute, `${label}.departure_minute`),
    departure_version: integerValue(source.departure_version, `${label}.departure_version`),
    minimum_transfer_minutes: integerValue(source.minimum_transfer_minutes, `${label}.minimum_transfer_minutes`),
  };
}

export function assessmentValue(value: CanonicalJson | undefined, label: string): Assessment | null {
  if (value === null || value === undefined) return null;
  const assessment = asRecord(value, label);
  const claim = stringValue(assessment.claim, `${label}.claim`);
  const validity = stringValue(assessment.validity, `${label}.validity`);
  if (claim !== "connection_feasible" && claim !== "connection_at_risk") throw new TypeError(`${label}.claim is unsupported`);
  if (validity !== "current" && validity !== "superseded") throw new TypeError(`${label}.validity is unsupported`);
  return {
    assessor_member_id: stringValue(assessment.assessor_member_id, `${label}.assessor_member_id`),
    arrival_version: integerValue(assessment.arrival_version, `${label}.arrival_version`),
    claim,
    departure_version: integerValue(assessment.departure_version, `${label}.departure_version`),
    reason_code: stringValue(assessment.reason_code, `${label}.reason_code`),
    validity,
  };
}
