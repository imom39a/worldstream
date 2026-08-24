import { describe, expect, it, vi } from "vitest";

import { ParticipantHandoffClient, ParticipantHandoffError } from "./participantHandoff";
import { ParticipantConsoleSession } from "./participantSession";

const HANDOFF = `wsh1:${"ab".repeat(32)}`;
const OBSERVATION = {
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

describe("Participant Console handed-off session", () => {
  it("reconnects and refreshes through the retained opaque cookie", async () => {
    const client = {
      redeem: vi.fn().mockResolvedValue({ version: "participant_console_session.v1", state: "usable", nextAction: "continue" }),
      resume: vi.fn()
        .mockResolvedValueOnce({ version: "participant_console_session.v1", state: "disconnected", nextAction: "reconnect" })
        .mockResolvedValueOnce({ version: "participant_console_session.v1", state: "usable", nextAction: "continue" }),
      observe: vi.fn().mockResolvedValue(OBSERVATION),
      act: vi.fn(),
    } satisfies Pick<ParticipantHandoffClient, "redeem" | "resume" | "observe" | "act">;

    const opened = new ParticipantConsoleSession(client);
    await expect(opened.start({ kind: "handoff", handoff: HANDOFF })).resolves.toMatchObject({ state: "live" });
    expect(client.redeem).toHaveBeenCalledWith(HANDOFF);

    const refreshed = new ParticipantConsoleSession(client);
    await expect(refreshed.start({ kind: "direct" })).resolves.toMatchObject({ state: "disconnected", action: "reconnect" });
    await expect(refreshed.reconnect()).resolves.toMatchObject({ state: "live" });
    expect(client.observe).toHaveBeenCalledWith(null);
  });

  it("returns missing or invalid retained authority to actionable setup", async () => {
    const client = {
      redeem: vi.fn(),
      resume: vi.fn().mockRejectedValue(new ParticipantHandoffError(
        "participant_session_authority_invalid",
        "The retained Participant authority is no longer valid.",
        "return_to_task_setup",
        false,
      )),
      observe: vi.fn(),
      act: vi.fn(),
    } satisfies Pick<ParticipantHandoffClient, "redeem" | "resume" | "observe" | "act">;
    const session = new ParticipantConsoleSession(client);
    await expect(session.start({ kind: "direct" })).resolves.toEqual({
      state: "setup_required",
      action: "return_to_task_setup",
      message: "The retained Participant authority is no longer valid.",
    });
  });

  it("submits no routing or authority values from Console state", async () => {
    const client = {
      redeem: vi.fn().mockResolvedValue({ version: "participant_console_session.v1", state: "usable", nextAction: "continue" }),
      resume: vi.fn(),
      observe: vi.fn().mockResolvedValue(OBSERVATION),
      act: vi.fn().mockResolvedValue({ state: "accepted", room_seq: 1 }),
    } satisfies Pick<ParticipantHandoffClient, "redeem" | "resume" | "observe" | "act">;
    const session = new ParticipantConsoleSession(client);
    await session.start({ kind: "handoff", handoff: HANDOFF });
    await session.act({
      actionId: "01ARZ3NDEKTSV4RRFFQ69G5FAY",
      basedOnRoomSeq: 0,
      offerId: "0:ready:0",
      schemaDigest: `blake3:${"a".repeat(64)}`,
      actionType: "ready",
      payload: {},
    });
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
});
