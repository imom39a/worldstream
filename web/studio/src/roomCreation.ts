export type RoomCreationState =
  | "waiting"
  | "retrying"
  | "succeeded"
  | "needs_attention";

export interface RoomCreationAttention {
  code: string;
  message: string;
  retryable: boolean;
}

export interface RoomCreationStatus {
  version: "studio_room_creation.v1";
  draft_id: string;
  operation_id: string;
  idempotency_key: string;
  review_hash: string;
  intent_hash: string;
  state: RoomCreationState;
  attempts: number;
  room_id: string | null;
  attention: RoomCreationAttention | null;
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

export type RoomCreationLoadResult =
  | { availability: "available"; operation: RoomCreationStatus | null }
  | { availability: "unavailable"; operation: null };

export async function loadRoomCreation(
  draftId: string,
  fetcher: Fetcher = fetch,
): Promise<RoomCreationLoadResult> {
  if (!isIdentifier(draftId)) return { availability: "unavailable", operation: null };
  try {
    const response = await fetcher(`/api/v1/room-creations/${draftId}`, {
      headers: { accept: "application/json" },
    });
    if (response.status === 404) return { availability: "available", operation: null };
    if (!response.ok) return { availability: "unavailable", operation: null };
    const value: unknown = await response.json();
    return isRoomCreationStatus(value) && value.draft_id === draftId
      ? { availability: "available", operation: value }
      : { availability: "unavailable", operation: null };
  } catch {
    return { availability: "unavailable", operation: null };
  }
}

export async function requestRoomCreation(
  draftId: string,
  action: "start" | "retry",
  fetcher: Fetcher = fetch,
): Promise<RoomCreationStatus | null> {
  if (!isIdentifier(draftId)) return null;
  try {
    const response = await fetcher(`/api/v1/room-creations/${draftId}:${action}`, {
      method: "POST",
      headers: { accept: "application/json" },
    });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isRoomCreationStatus(value) && value.draft_id === draftId ? value : null;
  } catch {
    return null;
  }
}

function isRoomCreationStatus(value: unknown): value is RoomCreationStatus {
  if (!isRecordWithKeys(value, [
    "version", "draft_id", "operation_id", "idempotency_key", "review_hash", "intent_hash",
    "state", "attempts", "room_id", "attention",
  ])) return false;
  if (
    value.version !== "studio_room_creation.v1" ||
    typeof value.draft_id !== "string" || !isIdentifier(value.draft_id) ||
    !isBoundedText(value.operation_id) ||
    !isBoundedText(value.idempotency_key) ||
    !isDigest(value.review_hash) ||
    !isDigest(value.intent_hash) ||
    !isState(value.state) ||
    !Number.isSafeInteger(value.attempts) || Number(value.attempts) < 0 ||
    !(value.room_id === null || (typeof value.room_id === "string" && isUlid(value.room_id))) ||
    !(value.attention === null || isAttention(value.attention))
  ) return false;
  if (value.state === "succeeded") return value.room_id !== null && value.attention === null;
  if (value.room_id !== null) return false;
  if (value.state === "waiting") return value.attention === null;
  return value.attention !== null;
}

function isAttention(value: unknown): value is RoomCreationAttention {
  return isRecordWithKeys(value, ["code", "message", "retryable"]) &&
    isBoundedText(value.code) && typeof value.message === "string" &&
    value.message.length > 0 && value.message.length <= 256 &&
    typeof value.retryable === "boolean";
}

function isState(value: unknown): value is RoomCreationState {
  return value === "waiting" || value === "retrying" || value === "succeeded" ||
    value === "needs_attention";
}

function isDigest(value: unknown): value is string {
  return typeof value === "string" && /^blake3:[0-9a-f]{64}$/.test(value);
}

function isUlid(value: string): boolean {
  return /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/.test(value);
}

function isIdentifier(value: string): boolean {
  return /^[a-z0-9][a-z0-9-]{0,63}$/.test(value);
}

function isBoundedText(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 128;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isRecordWithKeys(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return isRecord(value) && Object.keys(value).length === keys.length && keys.every((key) => key in value);
}
