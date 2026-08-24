export interface RoomHead {
  room_id: string;
  room_seq: number;
  genesis_or_transition_hash: string;
  core_schema_version: string;
  pack_digest: string;
  core_state_hash: string;
  activity_state_hash: string;
  authoritative_state_hash: string;
}

export interface PackReference {
  id: string;
  version: string;
  digest: string;
}

export type RoomSetupProgress =
  | { status: "complete"; completed_steps: number; total_steps: number }
  | { status: "partially_provisioned"; completed_steps: number; total_steps: number; reason: string }
  | { status: "unavailable"; reason: string };

export type ParticipantReadiness =
  | { status: "available"; ready: number; total: number }
  | { status: "unavailable"; reason: string };

export type ActivityPhase =
  | { status: "available"; value: string }
  | { status: "unavailable"; reason: string };

export type RoomFreshness =
  | { status: "fresh"; observed_at: string }
  | { status: "stale"; observed_at: string; reason: string }
  | { status: "unavailable"; reason: string };

export interface OperatorRoom {
  room_id: string;
  room_head: RoomHead;
  pack: PackReference;
  setup_progress: RoomSetupProgress;
  participant_readiness: ParticipantReadiness;
  integrity: { status: "healthy" | "faulted" | "quarantined"; generation: number };
  activity_phase: ActivityPhase;
  freshness: RoomFreshness;
}

export interface RoomInventoryPage {
  schema: "worldstream/studio-room-inventory/v1";
  rooms: OperatorRoom[];
  next_after_room_id: string | null;
}

export type RoomInventoryState =
  | { status: "loading" }
  | { status: "available"; page: RoomInventoryPage }
  | { status: "unavailable"; reason: "connection_failed" | "invalid_response" };

const ROOM_PAGE_LIMIT = 50;

export async function loadRoomInventory(
  fetcher: typeof fetch = fetch,
): Promise<RoomInventoryState> {
  try {
    const response = await fetcher(`/api/v1/rooms?limit=${ROOM_PAGE_LIMIT}`, {
      cache: "no-store",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) return { status: "unavailable", reason: "connection_failed" };
    const value: unknown = await response.json();
    if (!isRoomInventoryPage(value)) {
      return { status: "unavailable", reason: "invalid_response" };
    }
    return { status: "available", page: value };
  } catch {
    return { status: "unavailable", reason: "connection_failed" };
  }
}

export async function loadRoomDetail(
  roomId: string,
  fetcher: typeof fetch = fetch,
): Promise<OperatorRoom | null> {
  if (!isStableRoomId(roomId)) return null;
  try {
    const response = await fetcher(`/api/v1/rooms/${encodeURIComponent(roomId)}`, {
      cache: "no-store",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isOperatorRoom(value) ? value : null;
  } catch {
    return null;
  }
}

export function staleAfterFailedRefresh(
  previous: RoomInventoryState,
  next: RoomInventoryState,
): RoomInventoryState {
  if (next.status !== "unavailable" || previous.status !== "available") return next;
  return {
    status: "available",
    page: {
      ...previous.page,
      rooms: previous.page.rooms.map((room) => ({
        ...room,
        freshness: room.freshness.status === "unavailable"
          ? room.freshness
          : {
              status: "stale",
              observed_at: room.freshness.observed_at,
              reason: "refresh_failed",
            },
      })),
    },
  };
}

function isRoomInventoryPage(value: unknown): value is RoomInventoryPage {
  if (!isRecord(value)) return false;
  return value.schema === "worldstream/studio-room-inventory/v1"
    && Array.isArray(value.rooms)
    && value.rooms.length <= ROOM_PAGE_LIMIT
    && value.rooms.every(isOperatorRoom)
    && (value.next_after_room_id === null || isStableRoomId(value.next_after_room_id));
}

function isOperatorRoom(value: unknown): value is OperatorRoom {
  if (!isRecord(value) || !isStableRoomId(value.room_id)) return false;
  if (!hasOnlyKeys(value, [
    "room_id", "room_head", "pack", "setup_progress", "participant_readiness",
    "integrity", "activity_phase", "freshness",
  ])) return false;
  if (!isRecord(value.room_head) || value.room_head.room_id !== value.room_id) return false;
  if (!isRecord(value.pack) || value.room_head.pack_digest !== value.pack.digest) return false;
  if (!isRecord(value.integrity) || !["healthy", "faulted", "quarantined"].includes(String(value.integrity.status))) return false;
  if (!isRecord(value.setup_progress) || !["complete", "partially_provisioned", "unavailable"].includes(String(value.setup_progress.status))) return false;
  if (!isRecord(value.participant_readiness) || !["available", "unavailable"].includes(String(value.participant_readiness.status))) return false;
  if (!isRecord(value.activity_phase) || !["available", "unavailable"].includes(String(value.activity_phase.status))) return false;
  return isRecord(value.freshness) && ["fresh", "stale", "unavailable"].includes(String(value.freshness.status));
}

function isStableRoomId(value: unknown): value is string {
  return typeof value === "string" && /^[0-9A-HJKMNP-TV-Z]{26}$/.test(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasOnlyKeys(value: Record<string, unknown>, allowed: string[]) {
  return Object.keys(value).every((key) => allowed.includes(key));
}
