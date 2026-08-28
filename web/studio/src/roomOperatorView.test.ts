import { describe, expect, it, vi } from "vitest";

import { enableRoomOperatorView, loadRoomOperatorView } from "./roomOperatorView";

const roomId = "01ARZ3NDEKTSV4RRFFQ69G5FB0";

describe("Room operator view", () => {
  it("accepts only the bounded public counter projection", async () => {
    const view = {
      schema: "worldstream/studio-room-operator-view/v1",
      room_id: roomId,
      state: "available",
      counter: { value: 3 },
      room_head: { room_seq: 7 },
    };
    const fetcher = vi.fn(async () => new Response(JSON.stringify(view), { status: 200 }));
    await expect(loadRoomOperatorView(roomId, fetcher)).resolves.toEqual(view);
    expect(fetcher).toHaveBeenCalledWith(`/api/v1/rooms/${roomId}/operator-view`, undefined);

    const leaked = { ...view, member_id: "01ARZ3NDEKTSV4RRFFQ69G5FB1" };
    await expect(loadRoomOperatorView(roomId, async () => new Response(JSON.stringify(leaked), { status: 200 }))).resolves.toBeNull();
  });

  it("uses the explicit reviewed operator-view enable action", async () => {
    const unavailable = {
      schema: "worldstream/studio-room-operator-view/v1",
      room_id: roomId,
      state: "unavailable",
      unavailable_reason: "operator_membership_required",
    };
    const fetcher = vi.fn(async () => new Response(JSON.stringify(unavailable), { status: 200 }));
    await expect(enableRoomOperatorView(roomId, fetcher)).resolves.toEqual(unavailable);
    expect(fetcher).toHaveBeenCalledWith(`/api/v1/rooms/${roomId}/operator-view`, expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ schema: "worldstream/studio-room-operator-view-enable/v1" }),
    }));
  });
});
