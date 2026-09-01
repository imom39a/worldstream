import { describe, expect, it, vi } from "vitest";

import {
  ActivityClientHandoffClient,
  ActivityClientHandoffError,
  ActivityClientSession,
} from "./index";

const HANDOFF = `wsh1:${"ab".repeat(32)}`;
const DELIVERY_BATCH = {
  pack: { id: "worldstream.counter", version: "0.1.0", digest: `blake3:${"2".repeat(64)}` },
  room_head: {
    room_seq: 7,
    genesis_or_transition_hash: `blake3:${"1".repeat(64)}`,
    core_schema_version: "core.v1",
    pack_digest: `blake3:${"2".repeat(64)}`,
    core_state_hash: `blake3:${"3".repeat(64)}`,
    activity_state_hash: `blake3:${"4".repeat(64)}`,
    authoritative_state_hash: `blake3:${"5".repeat(64)}`,
  },
  frame_head: 7,
  delivery: [{ kind: "projection_reset" as const, body: { projection: { phase: "Lobby", action_offers: [] } } }],
};

describe("Activity Client handed-off session", () => {
  it("reconnects and refreshes through the retained opaque cookie", async () => {
    const client = {
      redeem: vi.fn().mockResolvedValue({ version: "participant_console_session.v1", state: "usable", nextAction: "continue" }),
      resume: vi.fn()
        .mockResolvedValueOnce({ version: "participant_console_session.v1", state: "disconnected", nextAction: "reconnect" })
        .mockResolvedValueOnce({ version: "participant_console_session.v1", state: "usable", nextAction: "continue" }),
      observe: vi.fn().mockResolvedValue(DELIVERY_BATCH),
      act: vi.fn(),
      replay: vi.fn(),
    } satisfies Pick<ActivityClientHandoffClient, "redeem" | "resume" | "observe" | "act" | "replay">;

    const opened = new ActivityClientSession(client);
    await expect(opened.start({ kind: "handoff", handoff: HANDOFF })).resolves.toMatchObject({ state: "live" });
    expect(client.redeem).toHaveBeenCalledWith(HANDOFF);

    const refreshed = new ActivityClientSession(client);
    await expect(refreshed.start({ kind: "direct" })).resolves.toMatchObject({ state: "disconnected", action: "reconnect" });
    await expect(refreshed.reconnect()).resolves.toMatchObject({ state: "live" });
    expect(client.observe).toHaveBeenCalledWith(null);
  });

  it("deduplicates a repeated handoff start so Strict Mode cannot redeem it twice", async () => {
    const client = {
      redeem: vi.fn().mockResolvedValue({ version: "participant_console_session.v1", state: "usable", nextAction: "continue" }),
      resume: vi.fn(),
      observe: vi.fn().mockResolvedValue(DELIVERY_BATCH),
      act: vi.fn(),
      replay: vi.fn(),
    } satisfies Pick<ActivityClientHandoffClient, "redeem" | "resume" | "observe" | "act" | "replay">;
    const session = new ActivityClientSession(client);
    await Promise.all([session.start({ kind: "handoff", handoff: HANDOFF }), session.start({ kind: "handoff", handoff: HANDOFF })]);
    expect(client.redeem).toHaveBeenCalledTimes(1);
    expect(client.observe).toHaveBeenCalledTimes(1);
  });

  it("returns missing or invalid retained authority to actionable setup", async () => {
    const client = {
      redeem: vi.fn(),
      resume: vi.fn().mockRejectedValue(new ActivityClientHandoffError(
        "participant_session_authority_invalid",
        "The retained Participant authority is no longer valid.",
        "return_to_task_setup",
        false,
      )),
      observe: vi.fn(),
      act: vi.fn(),
      replay: vi.fn(),
    } satisfies Pick<ActivityClientHandoffClient, "redeem" | "resume" | "observe" | "act" | "replay">;
    const session = new ActivityClientSession(client);
    await expect(session.start({ kind: "direct" })).resolves.toEqual({
      state: "setup_required",
      action: "return_to_task_setup",
      message: "The retained Participant authority is no longer valid.",
    });
  });

  it("keeps a redeemed session reconnectable when its first Room delivery batch is transiently unavailable", async () => {
    const client = {
      redeem: vi.fn().mockResolvedValue({ version: "participant_console_session.v1", state: "usable", nextAction: "continue" }),
      resume: vi.fn(),
      observe: vi.fn().mockRejectedValue(new ActivityClientHandoffError(
        "participant_session_unavailable",
        "The Participant View cannot reach the Room service safely.",
        "reconnect",
        true,
      )),
      act: vi.fn(),
      replay: vi.fn(),
    } satisfies Pick<ActivityClientHandoffClient, "redeem" | "resume" | "observe" | "act" | "replay">;
    const session = new ActivityClientSession(client);
    await expect(session.start({ kind: "handoff", handoff: HANDOFF })).resolves.toEqual({
      state: "disconnected",
      action: "reconnect",
      message: "The Participant View cannot reach the Room service safely.",
    });
    expect(client.redeem).toHaveBeenCalledOnce();
  });

  it("submits no routing or authority values from Console state", async () => {
    const client = {
      redeem: vi.fn().mockResolvedValue({ version: "participant_console_session.v1", state: "usable", nextAction: "continue" }),
      resume: vi.fn(),
      observe: vi.fn().mockResolvedValue(DELIVERY_BATCH),
      act: vi.fn().mockResolvedValue({ state: "accepted", room_seq: 1 }),
      replay: vi.fn(),
    } satisfies Pick<ActivityClientHandoffClient, "redeem" | "resume" | "observe" | "act" | "replay">;
    const session = new ActivityClientSession(client);
    await session.start({ kind: "handoff", handoff: HANDOFF });
    await expect(session.act({
      actionId: "01ARZ3NDEKTSV4RRFFQ69G5FAY",
      basedOnRoomSeq: 0,
      offerId: "0:ready:0",
      schemaDigest: `blake3:${"a".repeat(64)}`,
      actionType: "ready",
      payload: {},
    })).resolves.toEqual({ state: "accepted", room_seq: 1 });
    expect(client.act).toHaveBeenCalledWith({
      action_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY",
      based_on_room_seq: 0,
      offer_id: "0:ready:0",
      schema_digest: `blake3:${"a".repeat(64)}`,
      action_type: "ready",
      payload: {},
    });
    expect(JSON.stringify(client.act.mock.calls)).not.toMatch(/room_id|member_id|bearer|credential/);
  });

  it("names the installed live transport value as a delivery batch", async () => {
    const client = {
      redeem: vi.fn(),
      resume: vi.fn().mockResolvedValue({ version: "participant_console_session.v1", state: "usable", nextAction: "continue" }),
      observe: vi.fn().mockResolvedValue(DELIVERY_BATCH),
      act: vi.fn(),
      replay: vi.fn(),
    } satisfies Pick<ActivityClientHandoffClient, "redeem" | "resume" | "observe" | "act" | "replay">;
    const session = new ActivityClientSession(client);

    const state = await session.start({ kind: "direct" });

    expect(state).toEqual({ state: "live", action: "continue", message: null, deliveryBatch: DELIVERY_BATCH });
    expect(state).not.toHaveProperty("observation");
  });

  it("retains current Action Offers when a later non-empty batch omits them", async () => {
    const offered = [{ action_type: "ready", payload_schema_digest: `blake3:${"a".repeat(64)}` }];
    const client = {
      redeem: vi.fn(),
      resume: vi.fn().mockResolvedValue({ version: "participant_console_session.v1", state: "usable", nextAction: "continue" }),
      observe: vi.fn()
        .mockResolvedValueOnce({
          ...DELIVERY_BATCH,
          delivery: [{ kind: "projection_reset", body: { projection: { phase: "Lobby", action_offers: offered } } }],
        })
        .mockResolvedValueOnce({
          ...DELIVERY_BATCH,
          room_head: { ...DELIVERY_BATCH.room_head, room_seq: 8 },
          frame_head: 8,
          delivery: [{ kind: "observation", body: { observation: { notice: "Phase advanced" } } }],
        }),
      act: vi.fn(),
      replay: vi.fn(),
    } satisfies Pick<ActivityClientHandoffClient, "redeem" | "resume" | "observe" | "act" | "replay">;
    const session = new ActivityClientSession(client);

    await session.start({ kind: "direct" });
    await session.refresh();

    expect(session.actionOffers).toEqual(offered);
  });

  it("replaces current Action Offers when a later batch explicitly supplies an empty set", async () => {
    const client = {
      redeem: vi.fn(),
      resume: vi.fn().mockResolvedValue({ version: "participant_console_session.v1", state: "usable", nextAction: "continue" }),
      observe: vi.fn()
        .mockResolvedValueOnce({
          ...DELIVERY_BATCH,
          delivery: [{
            kind: "projection_reset",
            body: { projection: { action_offers: [{ action_type: "ready", payload_schema_digest: `blake3:${"a".repeat(64)}` }] } },
          }],
        })
        .mockResolvedValueOnce({
          ...DELIVERY_BATCH,
          room_head: { ...DELIVERY_BATCH.room_head, room_seq: 8 },
          frame_head: 8,
          delivery: [{ kind: "observation", body: { observation: { action_offers: [] } } }],
        }),
      act: vi.fn(),
      replay: vi.fn(),
    } satisfies Pick<ActivityClientHandoffClient, "redeem" | "resume" | "observe" | "act" | "replay">;
    const session = new ActivityClientSession(client);

    await session.start({ kind: "direct" });
    await session.refresh();

    expect(session.actionOffers).toEqual([]);
  });
});
