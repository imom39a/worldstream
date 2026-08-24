export type TaskSetupState = "waiting" | "provisioning" | "ready" | "needs_attention";

export type TaskSetupStage =
  | { kind: "member_capability"; seat_id: string }
  | { kind: "runner_capability"; seat_id: string };

export interface TaskSetupAttention {
  code: string;
  message: string;
  retryable: boolean;
}

export interface TaskSetupSeatStatus {
  seat_id: string;
  role: string;
  required: boolean;
  display_name: string;
  principal_id: string | null;
  principal_kind: "human" | "agent" | null;
  agent_assignment: "external" | "managed" | null;
  member_id: string | null;
  member_authority: "pending" | "provisioned" | "unfilled_optional";
  runner_authority: "pending" | "provisioned" | "not_applicable";
}

export interface TaskSetupStatus {
  version: "studio_task_setup.v1";
  draft_id: string;
  operation_id: string;
  room_id: string;
  state: TaskSetupState;
  attempts: number;
  completed_stages: number;
  total_stages: number;
  active_stage: TaskSetupStage | null;
  attention: TaskSetupAttention | null;
  seats: TaskSetupSeatStatus[];
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const maximumSeats = 64;
const maximumStages = maximumSeats * 2;
const maximumU32 = 4_294_967_295;
const statusKeys = [
  "version", "draft_id", "operation_id", "room_id", "state", "attempts",
  "completed_stages", "total_stages", "active_stage", "attention", "seats",
] as const;
const seatKeys = [
  "seat_id", "role", "required", "display_name", "principal_id", "principal_kind",
  "agent_assignment", "member_id", "member_authority", "runner_authority",
] as const;

export async function loadTaskSetup(
  draftId: string,
  fetcher: Fetcher = fetch,
): Promise<{ availability: "available" | "unavailable"; setup: TaskSetupStatus | null }> {
  if (!isIdentifier(draftId)) return { availability: "unavailable", setup: null };
  try {
    const response = await fetcher(`/api/v1/task-setups/${encodeURIComponent(draftId)}`, {
      headers: { Accept: "application/json" },
    });
    if (response.status === 404) return { availability: "available", setup: null };
    if (!response.ok) return { availability: "unavailable", setup: null };
    const value: unknown = await response.json();
    return isTaskSetupStatus(value) && value.draft_id === draftId
      ? { availability: "available", setup: value }
      : { availability: "unavailable", setup: null };
  } catch {
    return { availability: "unavailable", setup: null };
  }
}

export async function requestTaskSetup(
  draftId: string,
  action: "start" | "retry",
  fetcher: Fetcher = fetch,
): Promise<TaskSetupStatus | null> {
  if (!isIdentifier(draftId)) return null;
  try {
    const response = await fetcher(
      `/api/v1/task-setups/${encodeURIComponent(draftId)}:${action}`,
      { method: "POST", headers: { Accept: "application/json" } },
    );
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isTaskSetupStatus(value) && value.draft_id === draftId ? value : null;
  } catch {
    return null;
  }
}

export function isTaskSetupStatus(value: unknown): value is TaskSetupStatus {
  if (containsCredentialField(value) || !isRecordWithKeys(value, statusKeys) ||
    value.version !== "studio_task_setup.v1" || !isIdentifier(value.draft_id) ||
    !isUlid(value.operation_id) || !isUlid(value.room_id) || !isState(value.state) ||
    !isU32(value.attempts) || !isBoundedCount(value.completed_stages) ||
    !isBoundedCount(value.total_stages) || !Array.isArray(value.seats) ||
    value.seats.length > maximumSeats || !value.seats.every(isSeat) ||
    !(value.active_stage === null || isStage(value.active_stage)) ||
    !(value.attention === null || isAttention(value.attention))) return false;

  const seats = value.seats as TaskSetupSeatStatus[];
  if (new Set(seats.map((seat) => seat.seat_id)).size !== seats.length) return false;
  const totalStages = seats.reduce((total, seat) => total + seatStageCount(seat), 0);
  const completedStages = seats.reduce((total, seat) => total + completedSeatStageCount(seat), 0);
  if (value.total_stages !== totalStages || value.completed_stages !== completedStages ||
    completedStages > totalStages) return false;

  const expectedStage = nextStage(seats);
  if (value.active_stage !== null && !sameStage(value.active_stage, expectedStage)) return false;
  if (value.state === "waiting") {
    return value.attempts === 0 && completedStages === 0 && value.active_stage === null &&
      value.attention === null;
  }
  if (value.state === "ready") {
    return value.attempts > 0 && completedStages === totalStages && expectedStage === null &&
      value.active_stage === null && value.attention === null;
  }
  if (value.state === "provisioning") {
    return value.attempts > 0 && completedStages < totalStages && expectedStage !== null &&
      value.active_stage !== null && value.attention === null;
  }
  return value.attempts > 0 && completedStages < totalStages && expectedStage !== null &&
    value.active_stage !== null && value.attention !== null;
}

function isSeat(value: unknown): value is TaskSetupSeatStatus {
  if (!isRecordWithKeys(value, seatKeys) || !isIdentifier(value.seat_id) || !isText(value.role) ||
    typeof value.required !== "boolean" || !isText(value.display_name) ||
    !(value.principal_id === null || isUlid(value.principal_id)) ||
    !(value.principal_kind === null || value.principal_kind === "human" || value.principal_kind === "agent") ||
    !(value.agent_assignment === null || value.agent_assignment === "external" || value.agent_assignment === "managed") ||
    !(value.member_id === null || isUlid(value.member_id)) ||
    !["pending", "provisioned", "unfilled_optional"].includes(String(value.member_authority)) ||
    !["pending", "provisioned", "not_applicable"].includes(String(value.runner_authority))) {
    return false;
  }
  const seat = value as unknown as TaskSetupSeatStatus;
  const unfilled = seat.principal_id === null && seat.principal_kind === null &&
    seat.agent_assignment === null && seat.member_id === null;
  if (unfilled) {
    return !seat.required && seat.member_authority === "unfilled_optional" &&
      seat.runner_authority === "not_applicable";
  }
  if (seat.principal_id === null || seat.principal_kind === null || seat.member_id === null ||
    seat.member_authority === "unfilled_optional") return false;
  if (seat.principal_kind === "human") {
    return seat.agent_assignment === null && seat.runner_authority === "not_applicable";
  }
  return (seat.agent_assignment === "external" || seat.agent_assignment === "managed") &&
    seat.runner_authority !== "not_applicable" &&
    !(seat.member_authority === "pending" && seat.runner_authority === "provisioned");
}

function isStage(value: unknown): value is TaskSetupStage {
  return isRecordWithKeys(value, ["kind", "seat_id"]) &&
    (value.kind === "member_capability" || value.kind === "runner_capability") &&
    isIdentifier(value.seat_id);
}

function isAttention(value: unknown): value is TaskSetupAttention {
  return isRecordWithKeys(value, ["code", "message", "retryable"]) &&
    typeof value.code === "string" && /^[a-z0-9][a-z0-9_]{0,63}$/.test(value.code) &&
    typeof value.message === "string" && value.message.length > 0 && value.message.length <= 512 &&
    typeof value.retryable === "boolean";
}

function nextStage(seats: TaskSetupSeatStatus[]): TaskSetupStage | null {
  for (const seat of seats) {
    if (seat.member_authority === "pending") {
      return { kind: "member_capability", seat_id: seat.seat_id };
    }
    if (seat.runner_authority === "pending") {
      return { kind: "runner_capability", seat_id: seat.seat_id };
    }
  }
  return null;
}

function sameStage(left: TaskSetupStage, right: TaskSetupStage | null): boolean {
  return right !== null && left.kind === right.kind && left.seat_id === right.seat_id;
}

function seatStageCount(seat: TaskSetupSeatStatus): number {
  return Number(seat.member_authority !== "unfilled_optional") +
    Number(seat.runner_authority !== "not_applicable");
}

function completedSeatStageCount(seat: TaskSetupSeatStatus): number {
  return Number(seat.member_authority === "provisioned") +
    Number(seat.runner_authority === "provisioned");
}

function containsCredentialField(value: unknown, seen = new Set<object>()): boolean {
  if (Array.isArray(value)) return value.some((item) => containsCredentialField(item, seen));
  if (!isRecord(value) || seen.has(value)) return false;
  seen.add(value);
  return Object.entries(value).some(([key, child]) =>
    isCredentialKey(key) || containsCredentialField(child, seen));
}

function isCredentialKey(key: string): boolean {
  const normalized = key.replaceAll("-", "_").toLowerCase();
  if (normalized.includes("bearer") || normalized.includes("token_hash") ||
    normalized.includes("tokenhash") || normalized.includes("secret_reference") ||
    normalized.includes("secretreference") || normalized.includes("secret_ref")) return true;
  return /(^|_)(path|paths|request|receipt|hash)($|_)/.test(normalized);
}

function isRecordWithKeys(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return isRecord(value) && Object.keys(value).length === keys.length && keys.every((key) => key in value);
}

function isIdentifier(value: unknown): value is string {
  return typeof value === "string" && /^[a-z0-9][a-z0-9-]{0,63}$/.test(value);
}

function isUlid(value: unknown): value is string {
  return typeof value === "string" && /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/.test(value);
}

function isText(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 256;
}

function isU32(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) >= 0 && Number(value) <= maximumU32;
}

function isBoundedCount(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) >= 0 && Number(value) <= maximumStages;
}

function isState(value: unknown): value is TaskSetupState {
  return value === "waiting" || value === "provisioning" || value === "ready" ||
    value === "needs_attention";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
