import { describe, expect, it, vi } from "vitest";

import {
  buildSeatPolicy,
  createRoomDraft,
  invalidateRoomDraftReview,
  loadRoomDraft,
  saveRoomDraft,
  validateConfiguration,
  type RoomDraft,
} from "./roomDrafts";

const digest = `blake3:${"a".repeat(64)}`;

function draft(): RoomDraft {
  return {
    schema: "worldstream/studio-room-draft/v1",
    draft_id: "launch-alpha",
    pack: { id: "counter", version: "1.0.0", digest },
    configuration: { initial_value: 0, maximum_value: 8 },
    seats: [
      { seat_id: "role-1-seat-1", role: "player", required: true, display_name: "Player 1", principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV", principal_kind: "human" },
    ],
    readiness: [
      { seat_id: "role-1-seat-1", role: "player", required: true },
    ],
    last_valid_step: "readiness",
  };
}

describe("Studio Room drafts", () => {
  it("creates stable required and optional seats from declared Role cardinality", () => {
    const policy = buildSeatPolicy([
      { role: "player", minimum: 2, maximum: 3 },
      { role: "observer", minimum: 0, maximum: 1 },
    ]);

    expect(policy.seats.map((seat) => [seat.seat_id, seat.role, seat.required])).toEqual([
      ["role-1-seat-1", "player", true],
      ["role-1-seat-2", "player", true],
      ["role-1-seat-3", "player", false],
      ["role-2-seat-1", "observer", false],
    ]);
    expect(policy.readiness).toEqual(
      policy.seats.map(({ seat_id, role, required }) => ({ seat_id, role, required })),
    );
    expect(policy.readiness).not.toHaveProperty("ready");
  });

  it("returns bounded JSON Pointer errors for exact configuration fields", () => {
    const errors = validateConfiguration(
      {
        type: "object",
        properties: {
          title: { type: "string", minLength: 3 },
          rounds: { type: "integer", minimum: 1, maximum: 10 },
        },
        required: ["title", "rounds"],
        additionalProperties: false,
      },
      { title: "x", rounds: 11, extra: true },
    );

    expect(errors).toEqual([
      { path: "/configuration/extra", code: "additional_property", message: "The field is not declared by this exact revision." },
      { path: "/configuration/title", code: "min_length", message: "The field is shorter than allowed." },
      { path: "/configuration/rounds", code: "maximum", message: "The field is above the declared maximum." },
    ]);
  });

  it("resumes the exact persisted draft and never migrates its unavailable revision", async () => {
    const expected = draft();
    const fetcher = vi.fn(async () => new Response(JSON.stringify({
      version: "studio_room_draft.v1",
      draft: expected,
      review: {
        pack: expected.pack,
        configuration: expected.configuration,
        seats: expected.seats,
        readiness: expected.readiness,
      },
    }), { status: 200 }));

    const resumed = await loadRoomDraft("launch-alpha", fetcher);
    expect(resumed?.draft.pack).toEqual(expected.pack);
    expect(resumed?.draft.last_valid_step).toBe("readiness");
    expect(fetcher).toHaveBeenCalledWith("/api/v1/room-drafts/launch-alpha", {
      headers: { accept: "application/json" },
    });
  });

  it("saves planning state only through the bounded draft route", async () => {
    const expected = draft();
    const fetcher = vi.fn(async (_input, init) => new Response(JSON.stringify({
      version: "studio_room_draft.v1",
      draft: JSON.parse(String(init?.body)),
      review: {
        pack: expected.pack,
        configuration: expected.configuration,
        seats: expected.seats,
        readiness: expected.readiness,
      },
    }), { status: 200 }));

    await expect(saveRoomDraft(expected, fetcher)).resolves.toMatchObject({ draft: expected });
    expect(fetcher).toHaveBeenCalledWith("/api/v1/room-drafts/launch-alpha", expect.objectContaining({
      method: "PUT",
    }));
    expect(String(fetcher.mock.calls[0]?.[0])).not.toContain("/rooms");
    expect(String(fetcher.mock.calls[0]?.[0])).not.toContain("authority");
  });

  it("starts an empty draft at Activity without inventing a pack", () => {
    expect(createRoomDraft("launch-alpha")).toEqual({
      schema: "worldstream/studio-room-draft/v1",
      draft_id: "launch-alpha",
      pack: null,
      configuration: {},
      seats: [],
      readiness: [],
      last_valid_step: null,
    });
  });

  it.each([
    ["activity", (value: RoomDraft) => ({ ...value, pack: { id: "counter", version: "2.0.0", digest: `blake3:${"b".repeat(64)}` } }), null],
    ["configuration", (value: RoomDraft) => ({ ...value, configuration: { initial_value: 1, maximum_value: 8 } }), "activity"],
    ["seats", (value: RoomDraft) => ({ ...value, seats: value.seats.map((seat) => ({ ...seat, display_name: "Changed" })) }), "configuration"],
    ["readiness", (value: RoomDraft) => ({ ...value, readiness: value.readiness.map((policy) => ({ ...policy, required: false })) }), "seats"],
  ] as const)("invalidates persisted review after a %s edit", (_name, edit, expectedStep) => {
    const reviewed = { ...draft(), last_valid_step: "review" as const };
    expect(invalidateRoomDraftReview(reviewed, edit(reviewed)).last_valid_step).toBe(expectedStep);
  });
});
