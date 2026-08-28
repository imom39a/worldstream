export type RoomOperatorView =
  | { schema: "worldstream/studio-room-operator-view/v1"; room_id: string; state: "available"; counter: { value: number }; room_head: { room_seq: number } }
  | { schema: "worldstream/studio-room-operator-view/v1"; room_id: string; state: "unavailable"; unavailable_reason: "operator_membership_required" | "operator_view_unavailable" | "unsupported_projection" };

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

export async function loadRoomOperatorView(roomId: string, fetcher: Fetcher = fetch): Promise<RoomOperatorView | null> {
  return request(roomId, undefined, fetcher);
}

export async function enableRoomOperatorView(roomId: string, fetcher: Fetcher = fetch): Promise<RoomOperatorView | null> {
  return request(roomId, { method: "POST", headers: { Accept: "application/json", "Content-Type": "application/json" }, body: JSON.stringify({ schema: "worldstream/studio-room-operator-view-enable/v1" }) }, fetcher);
}

async function request(roomId: string, init: RequestInit | undefined, fetcher: Fetcher): Promise<RoomOperatorView | null> {
  if (!isRoomId(roomId)) return null;
  try {
    const response = await fetcher(`/api/v1/rooms/${encodeURIComponent(roomId)}/operator-view`, init);
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isView(value, roomId) ? value : null;
  } catch { return null; }
}

function isView(value: unknown, roomId: string): value is RoomOperatorView {
  if (!record(value) || value.schema !== "worldstream/studio-room-operator-view/v1" || value.room_id !== roomId) return false;
  if (value.state === "available") return exact(value, ["schema", "room_id", "state", "counter", "room_head"])
    && record(value.counter) && exact(value.counter, ["value"]) && count(value.counter.value) && value.counter.value <= 16
    && record(value.room_head) && exact(value.room_head, ["room_seq"]) && count(value.room_head.room_seq);
  return value.state === "unavailable" && exact(value, ["schema", "room_id", "state", "unavailable_reason"])
    && ["operator_membership_required", "operator_view_unavailable", "unsupported_projection"].includes(String(value.unavailable_reason));
}
function record(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value); }
function exact(value: Record<string, unknown>, keys: string[]): boolean { return Object.keys(value).length === keys.length && keys.every((key) => key in value); }
function count(value: unknown): value is number { return typeof value === "number" && Number.isSafeInteger(value) && value >= 0; }
function isRoomId(value: string): boolean { return /^[0-9A-HJKMNP-TV-Z]{26}$/.test(value); }
