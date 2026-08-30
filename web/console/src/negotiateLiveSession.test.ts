import { describe, expect, it } from "vitest";

import { NEGOTIATE_CONSOLE_BOOTSTRAP_VERSION, type NegotiateConsoleBootstrap } from "./negotiate";
import {
  initialNegotiateLiveView,
  latestNegotiateObservationFrameToAck,
  projectNegotiateMessage,
} from "./negotiateLiveSession";
import { MAX_MESSAGE_BYTES, WORLDSTREAM_PROTOCOL, type ProtocolMessage } from "./transport";

const room = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const member = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const target = { roomId: room, memberId: member };
const messageId = "01J00000000000000000000001";
const hash = `blake3:${"1".repeat(64)}`;

function initial(): NegotiateConsoleBootstrap {
  return {
    version: NEGOTIATE_CONSOLE_BOOTSTRAP_VERSION,
    room_id: room,
    member_id: member,
    room_sequence: 0,
    persona: "buyer_agent",
    projection: {
      aggregate_state: "formation",
      outcome: null,
      persona: "buyer_agent",
      phase: "formation_open",
      session_state: "open",
    },
    action_offers: [],
    connection: "connecting",
    replay: "pending",
    evidence: "pending",
  };
}

function message(type: ProtocolMessage["type"], body: Record<string, unknown>): ProtocolMessage {
  return {
    protocol: WORLDSTREAM_PROTOCOL,
    type,
    message_id: messageId,
    body,
  } as ProtocolMessage;
}

describe("Negotiate live Session projection", () => {
  it("installs only the authorized projection and enables Actions after the sync barrier", () => {
    let view = initialNegotiateLiveView(initial());
    view = projectNegotiateMessage(view, message("room.attached", {
      room_id: room,
      member_id: member,
      pack: { id: "worldstream.negotiate", version: "0.1.0", digest: hash },
      room_head: { room_seq: 4 },
      sync: { kind: "projection_reset", baseline_frame_head: 2 },
      sync_token: "opaque",
    }), target);
    expect(view.session.connection).toBe("catching_up");

    view = projectNegotiateMessage(view, message("projection.reset", {
      room_id: room,
      member_id: member,
      room_head: { room_seq: 4 },
      projection: {
        core: { private_authority: "ignored" },
        activity: {
          aggregate_state: "formation",
          outcome: null,
          persona: "buyer_agent",
          phase: "approval_pending",
          session_state: "open",
          current_proposal: { object_id: "offer-2", declared_content_hash: hash },
        },
        action_offers: [{ domain: "worldstream/action-offer/v1", action_type: "accept_current_proposal", payload_schema_digest: hash, eligibility_window: null }],
      },
    }), target);
    expect(view.session.projection.phase).toBe("approval_pending");
    expect(view.session.projection).not.toHaveProperty("private_authority");
    expect(view.session.action_offers).toEqual([{ action_type: "accept_current_proposal", payload_schema_digest: hash }]);

    view = projectNegotiateMessage(view, message("room.sync_acked", {
      through_frame_head: 2,
    }), target);
    expect(view.session.connection).toBe("live");
  });

  it("turns stale rejection into a catch-up gate without rendering server details", () => {
    const current = {
      ...initialNegotiateLiveView({ ...initial(), connection: "live", room_sequence: 7 }),
      session: { ...initial(), connection: "live" as const, room_sequence: 7 },
      receipt: { state: "submitting" as const, action_id: messageId, code: null, message: null },
    };
    const next = projectNegotiateMessage(current, message("action.rejected", {
      room_id: room,
      member_id: member,
      action_id: messageId,
      code: "stale_room_head",
      message: "secret server detail",
      current_room_seq: 8,
      action_offers: [],
    }), target);
    expect(next.session.connection).toBe("catching_up");
    expect(next.receipt.state).toBe("stale");
    expect(JSON.stringify(next)).not.toContain("secret server detail");
  });

  it("requires rebuilt signed bytes when the A202 session head changes without inventing catch-up", () => {
    const current = {
      ...initialNegotiateLiveView({ ...initial(), connection: "live", room_sequence: 7 }),
      session: { ...initial(), connection: "live" as const, room_sequence: 7 },
      receipt: { state: "submitting" as const, action_id: messageId, code: null, message: null },
    };
    const next = projectNegotiateMessage(current, message("action.rejected", {
      room_id: room,
      member_id: member,
      action_id: messageId,
      code: "stale_session_head",
      message: "private verifier details",
      current_room_seq: 7,
      action_offers: [{ action_type: "accept_current_proposal", payload_schema_digest: hash }],
    }), target);
    expect(next.session.connection).toBe("live");
    expect(next.receipt.state).toBe("stale");
    expect(next.receipt.message).toContain("Rebuild and re-sign");
    expect(JSON.stringify(next)).not.toContain("private verifier details");
  });

  it("keeps expiry as a typed no-transition rejection with server-supplied offers only", () => {
    const current = {
      ...initialNegotiateLiveView({ ...initial(), connection: "live", room_sequence: 7 }),
      session: { ...initial(), connection: "live" as const, room_sequence: 7 },
      receipt: { state: "submitting" as const, action_id: messageId, code: null, message: null },
    };
    const next = projectNegotiateMessage(current, message("action.rejected", {
      room_id: room,
      member_id: member,
      action_id: messageId,
      code: "approval_expired",
      message: "approval bytes and verifier trace",
      current_room_seq: 7,
      action_offers: [],
    }), target);
    expect(next.receipt.state).toBe("rejected");
    expect(next.receipt.code).toBe("approval_expired");
    expect(next.session.action_offers).toEqual([]);
    expect(JSON.stringify(next)).not.toContain("verifier trace");
  });

  it("clears the consumed offer immediately after acceptance", () => {
    const current = {
      ...initialNegotiateLiveView({
        ...initial(),
        connection: "live",
        room_sequence: 7,
        action_offers: [{ action_type: "accept_current_proposal", payload_schema_digest: hash }],
      }),
      session: {
        ...initial(),
        connection: "live" as const,
        room_sequence: 7,
        action_offers: [{ action_type: "accept_current_proposal", payload_schema_digest: hash }],
      },
      receipt: { state: "submitting" as const, action_id: messageId, code: null, message: null },
    };
    const next = projectNegotiateMessage(current, message("action.accepted", {
      room_id: room,
      member_id: member,
      action_id: messageId,
      cause_room_seq: 8,
      duplicate: false,
    }), target);
    expect(next.session.room_sequence).toBe(8);
    expect(next.session.action_offers).toEqual([]);
  });

  it("fails closed on cross-Room traffic and wrong Pack attachment", () => {
    const current = initialNegotiateLiveView(initial());
    const wrongRoom = projectNegotiateMessage(current, message("projection.reset", {
      room_id: "01ARZ3NDEKTSV4RRFFQ69G5FA0",
      member_id: member,
    }), target);
    expect(wrongRoom.session.connection).toBe("disconnected");

    const wrongPack = projectNegotiateMessage(current, message("room.attached", {
      room_id: room,
      member_id: member,
      pack: { id: "worldstream.counter" },
    }), target);
    expect(wrongPack.error).toContain("not a WorldStream Negotiate Room");
  });

  it("retains the frozen transport bounds in the test lane", () => {
    expect(MAX_MESSAGE_BYTES).toBe(524_288);
  });

  it("advances the durable observation Cursor only after the sync barrier", () => {
    expect(latestNegotiateObservationFrameToAck([4, 5, 6], false)).toBeNull();
    expect(latestNegotiateObservationFrameToAck([4, 6, 5], true)).toBe(6);
    expect(latestNegotiateObservationFrameToAck([], true)).toBeNull();
  });
});
