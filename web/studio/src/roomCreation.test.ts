import { describe, expect, it, vi } from "vitest";

import {
  loadRoomCreation,
  requestRoomCreation,
  type RoomCreationStatus,
} from "./roomCreation";

const status: RoomCreationStatus = {
  version: "studio_room_creation.v1",
  draft_id: "new-room",
  operation_id: "room-create-operation-0123456789abcdef",
  idempotency_key: "studio-room-create-0123456789abcdef",
  review_hash: `blake3:${"a".repeat(64)}`,
  intent_hash: `blake3:${"b".repeat(64)}`,
  state: "retrying",
  attempts: 2,
  room_id: null,
  attention: {
    code: "daemon_result_ambiguous",
    message: "The original Room creation result is not resolved yet; retrying will reuse the exact persisted intent.",
    retryable: true,
  },
};

describe("Room creation operation client", () => {
  it("loads durable ambiguous state without fabricating a Room", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify(status), { status: 200 }));

    await expect(loadRoomCreation("new-room", fetcher)).resolves.toEqual({
      availability: "available",
      operation: status,
    });
    expect(fetcher).toHaveBeenCalledWith("/api/v1/room-creations/new-room", {
      headers: { accept: "application/json" },
    });
  });

  it("starts and retries only the same draft-bound operation routes", async () => {
    const fetcher = vi.fn(async (
      _input: RequestInfo | URL,
      _init?: RequestInit,
    ) => new Response(JSON.stringify(status), { status: 200 }));

    await requestRoomCreation("new-room", "start", fetcher);
    await requestRoomCreation("new-room", "retry", fetcher);

    expect(fetcher.mock.calls.map(([input]) => input)).toEqual([
      "/api/v1/room-creations/new-room:start",
      "/api/v1/room-creations/new-room:retry",
    ]);
    expect(fetcher.mock.calls.every(([, init]) => init?.method === "POST" && init.body === undefined)).toBe(true);
  });

  it("fails closed for malformed identity or response state", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify({ ...status, room_id: "different-room" }), { status: 200 }));

    await expect(loadRoomCreation("../new-room", fetcher)).resolves.toEqual({ availability: "unavailable", operation: null });
    await expect(loadRoomCreation("new-room", fetcher)).resolves.toEqual({ availability: "unavailable", operation: null });
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("distinguishes no operation from unavailable refresh state", async () => {
    const missing = vi.fn(async () => new Response(null, { status: 404 }));
    const unavailable = vi.fn(async () => new Response(null, { status: 503 }));

    await expect(loadRoomCreation("new-room", missing)).resolves.toEqual({
      availability: "available",
      operation: null,
    });
    await expect(loadRoomCreation("new-room", unavailable)).resolves.toEqual({
      availability: "unavailable",
      operation: null,
    });
  });
});
