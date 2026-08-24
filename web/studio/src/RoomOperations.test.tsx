import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { RoomOperations } from "./RoomOperations";
import type { OperatorRoom } from "./roomInventory";

const base: OperatorRoom = {
  room_id: "01ARZ3NDEKTSV4RRFFQ69G5FQ0",
  room_head: {
    room_id: "01ARZ3NDEKTSV4RRFFQ69G5FQ0", room_seq: 7,
    genesis_or_transition_hash: "transition", core_schema_version: "core.v1",
    pack_digest: "digest", core_state_hash: "core", activity_state_hash: "activity",
    authoritative_state_hash: "authoritative",
  },
  pack: { id: "counter", version: "1.0.0", digest: "digest" },
  setup_progress: { status: "complete", completed_steps: 3, total_steps: 3 },
  participant_readiness: { status: "unavailable", reason: "operator_membership_required" },
  integrity: { status: "healthy", generation: 4 },
  activity_phase: { status: "unavailable", reason: "operator_membership_required" },
  freshness: { status: "fresh", observed_at: "2026-08-23T12:00:00Z" },
};

describe("Room Operations", () => {
  it("lists multiple Rooms and opens one stable-identity detail with independent axes", () => {
    const partial: OperatorRoom = {
      ...base,
      room_id: "01ARZ3NDEKTSV4RRFFQ69G5FQ1",
      room_head: { ...base.room_head, room_id: "01ARZ3NDEKTSV4RRFFQ69G5FQ1" },
      setup_progress: { status: "partially_provisioned", completed_steps: 1, total_steps: 3, reason: "awaiting_membership_setup" },
      integrity: { status: "faulted", generation: 5 },
      freshness: { status: "stale", observed_at: "2026-08-23T11:55:00Z", reason: "refresh_failed" },
    };
    const dom = renderToStaticMarkup(<RoomOperations inventory={{
      status: "available",
      page: { schema: "worldstream/studio-room-inventory/v1", rooms: [base, partial], next_after_room_id: null },
    }} selectedRoomId={partial.room_id} />);

    expect(dom).toContain(base.room_id);
    expect(dom).toContain(partial.room_id);
    expect(dom).toContain("Partially provisioned · 1/3");
    expect(dom).toContain("Unavailable · operator membership required");
    expect(dom).toContain("Faulted");
    expect(dom).toContain("Integrity generation");
    expect(dom).toContain("Stale · last observed");
    expect(dom).not.toContain("prompt");
    expect(dom).not.toContain("seat_id");
  });

  it("renders explicit loading, empty, and unavailable states", () => {
    expect(renderToStaticMarkup(<RoomOperations inventory={{ status: "loading" }} />)).toContain("Loading Rooms");
    expect(renderToStaticMarkup(<RoomOperations inventory={{
      status: "available", page: { schema: "worldstream/studio-room-inventory/v1", rooms: [], next_after_room_id: null },
    }} />)).toContain("No Rooms yet");
    expect(renderToStaticMarkup(<RoomOperations inventory={{
      status: "unavailable", reason: "connection_failed",
    }} />)).toContain("Rooms unavailable");
  });
});
