import { describe, expect, it, vi } from "vitest";

import {
  loadRoomDetail,
  loadRoomInventory,
  staleAfterFailedRefresh,
  type OperatorRoom,
  type RoomInventoryState,
} from "./roomInventory";

const room: OperatorRoom = {
  room_id: "01ARZ3NDEKTSV4RRFFQ69G5FQ0",
  room_head: {
    room_id: "01ARZ3NDEKTSV4RRFFQ69G5FQ0",
    room_seq: 9,
    genesis_or_transition_hash: "transition",
    core_schema_version: "core.v1",
    pack_digest: "pack-digest",
    core_state_hash: "core",
    activity_state_hash: "activity",
    authoritative_state_hash: "authoritative",
  },
  pack: { id: "counter", version: "1.0.0", digest: "pack-digest" },
  setup_progress: { status: "complete", completed_steps: 3, total_steps: 3 },
  participant_readiness: { status: "unavailable", reason: "operator_membership_required" },
  integrity: { status: "healthy", generation: 2 },
  activity_phase: { status: "unavailable", reason: "operator_membership_required" },
  freshness: { status: "fresh", observed_at: "2026-08-23T12:00:00Z" },
};

describe("Studio Room inventory client", () => {
  it("loads a bounded page and stable-identity detail without browser credentials", async () => {
    const fetcher = vi.fn(async (input: string | URL | Request) => new Response(
      JSON.stringify(String(input).includes(room.room_id)
        ? room
        : { schema: "worldstream/studio-room-inventory/v1", rooms: [room], next_after_room_id: null }),
      { status: 200, headers: { "Content-Type": "application/json" } },
    ));

    const inventory = await loadRoomInventory(fetcher as typeof fetch);
    const detail = await loadRoomDetail(room.room_id, fetcher as typeof fetch);

    expect(inventory.status).toBe("available");
    expect(detail?.room_id).toBe(room.room_id);
    expect(fetcher.mock.calls[0]?.[0]).toBe("/api/v1/rooms?limit=50");
    expect(JSON.stringify(fetcher.mock.calls)).not.toContain("Authorization");
  });

  it("retains safe metadata as explicitly stale after a failed refresh", () => {
    const previous: RoomInventoryState = {
      status: "available",
      page: { schema: "worldstream/studio-room-inventory/v1", rooms: [room], next_after_room_id: null },
    };

    const reconciled = staleAfterFailedRefresh(previous, {
      status: "unavailable",
      reason: "connection_failed",
    });

    expect(reconciled.status).toBe("available");
    if (reconciled.status === "available") {
      expect(reconciled.page.rooms[0]?.freshness).toEqual({
        status: "stale",
        observed_at: "2026-08-23T12:00:00Z",
        reason: "refresh_failed",
      });
      expect(reconciled.page.rooms[0]?.activity_phase.status).toBe("unavailable");
      expect(reconciled.page.rooms[0]?.integrity.status).toBe("healthy");
    }
  });

  it("rejects malformed or privacy-expanded responses", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify({
      schema: "worldstream/studio-room-inventory/v1",
      rooms: [{ ...room, invocation: { prompt: "private" } }],
      next_after_room_id: null,
    }), { status: 200 }));

    expect(await loadRoomInventory(fetcher as typeof fetch)).toEqual({
      status: "unavailable",
      reason: "invalid_response",
    });
  });
});
