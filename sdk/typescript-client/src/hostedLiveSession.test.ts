import { describe, expect, it, vi } from "vitest";

import {
  ActivityClientHandoffError,
  HostedLiveSessionController,
  WORLDSTREAM_PROTOCOL,
  WORLDSTREAM_WS_SUBPROTOCOL,
  type ActivityClientSessionStatus,
  type ProtocolMessage,
  type WebSocketLike,
} from "./index";

const id = (suffix: string) =>
  `01J0000000000000000000000${suffix}`.slice(0, 26);
const digest = (character: string) =>
  `blake3:${character.repeat(64)}`;
const TICKET = `wst1:${"ab".repeat(32)}`;
const USABLE: ActivityClientSessionStatus = {
  version: "participant_console_session.v1",
  state: "usable",
  nextAction: "continue",
};

class FakeSocket implements WebSocketLike {
  readonly readyState = 1;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent<unknown>) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  readonly sent: string[] = [];
  closed = false;

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.closed = true;
  }

  open(): void {
    this.onopen?.(new Event("open"));
  }

  receive(message: ProtocolMessage): void {
    this.onmessage?.({ data: JSON.stringify(message) } as MessageEvent<string>);
  }

  receiveRaw(value: unknown): void {
    this.onmessage?.({ data: value } as MessageEvent<unknown>);
  }

  remoteClose(): void {
    this.onclose?.({} as CloseEvent);
  }
}

function envelope(
  type: ProtocolMessage["type"],
  body: Record<string, unknown>,
  requestId?: string,
): ProtocolMessage {
  return {
    protocol: WORLDSTREAM_PROTOCOL,
    type,
    message_id: id("90"),
    ...(requestId === undefined ? {} : { request_id: requestId }),
    body,
  } as ProtocolMessage;
}

function roomHead(roomSequence: number): Record<string, unknown> {
  return {
    room_id: id("01"),
    room_seq: roomSequence,
    genesis_or_transition_hash: digest("1"),
    core_schema_version: "worldstream.core-room-state.v1",
    pack_digest: digest("2"),
    core_state_hash: digest("3"),
    activity_state_hash: digest("4"),
    authoritative_state_hash: digest("5"),
  };
}

function welcome(): ProtocolMessage {
  return envelope("server.welcome", {
    session_id: id("03"),
    selected_protocol: WORLDSTREAM_PROTOCOL,
    server_version: "0.1.0",
    heartbeat_interval_ms: 60_000,
    maximum_message_bytes: 524_288,
    authenticated_principal: { principal_id: "principal", kind: "human" },
  });
}

function attached(
  sync: Record<string, unknown>,
  cursor: number | null = null,
  accessMode: "participant" | "spectator" = "participant",
): ProtocolMessage {
  const through =
    sync.kind === "projection_reset"
      ? sync.baseline_frame_head
      : sync.through_frame_head;
  return envelope("room.attached", {
    room_id: id("01"),
    member_id: id("02"),
    principal_kind: "human",
    access_mode: accessMode,
    role: accessMode === "participant" ? "navigator" : null,
    membership_status: "enabled",
    room_status: "active",
    room_health: "healthy",
    integrity_generation: 1,
    room_head: roomHead(7),
    cursor,
    frame_head: through,
    retained_floor: 0,
    sync_token: "opaque-sync-token",
    sync,
    pack: {
      id: "worldstream.agent-heist",
      version: "0.2.0",
      digest: digest("2"),
    },
  });
}

function reset(frame = 9): ProtocolMessage {
  return envelope("projection.reset", {
    room_id: id("01"),
    member_id: id("02"),
    room_head: roomHead(7),
    room_health: "healthy",
    integrity_generation: 1,
    baseline_frame_head: frame,
    reset_reason: "retention_gap",
    projection_schema: "agent-heist.participant.v1",
    projection: {
      core: {
        access_mode: "participant",
        standing: "enabled",
        role: "navigator",
        room_status: "active",
        viewer_class: "participant",
      },
      activity: { phase: "lobby" },
      action_offers: [],
    },
    projection_hash: digest("6"),
  });
}

function observation(
  frame: number,
  roomSequence: number,
  hashCharacter = "7",
): ProtocolMessage {
  return envelope("observation.deliver", {
    room_id: id("01"),
    member_id: id("02"),
    frame_seq: frame,
    cause_room_seq: roomSequence,
    frame_kind: "observation",
    observation_schema: "agent-heist.participant.v1",
    observation: { phase: "negotiation", marker: frame },
    frame_payload_hash: digest(hashCharacter),
  });
}

function sentMessages(socket: FakeSocket): Array<Record<string, unknown>> {
  return socket.sent
    .filter((value) => value.startsWith("{"))
    .map((value) => JSON.parse(value) as Record<string, unknown>);
}

function fixture() {
  const sockets: FakeSocket[] = [];
  const ticketCursors: Array<number | null> = [];
  let sequence = 10;
  const authority = {
    redeem: vi.fn().mockResolvedValue(USABLE),
    resume: vi.fn().mockResolvedValue(USABLE),
    issueStreamTicket: vi.fn(async (cursor: number | null) => {
      ticketCursors.push(cursor);
      return { ticket: TICKET, expiresInMs: 15_000 };
    }),
  };
  const controller = new HostedLiveSessionController({
    streamUrl: "wss://stream.example/v1/hosted/browser-stream",
    authority,
    webSocketFactory: (url, protocols, options) => {
      expect(url).toBe("wss://stream.example/v1/hosted/browser-stream");
      expect(protocols).toEqual([WORLDSTREAM_WS_SUBPROTOCOL]);
      expect(options.headers).toEqual({});
      const socket = new FakeSocket();
      sockets.push(socket);
      return socket;
    },
    createMessageId: () => id(String(sequence++).padStart(2, "0")),
    connectTimeoutMs: 1_000,
    actionTimeoutMs: 1_000,
  });
  return { authority, controller, sockets, ticketCursors };
}

async function openResetSession(
  setup: ReturnType<typeof fixture>,
  accessMode: "participant" | "spectator" = "participant",
) {
  const starting = setup.controller.start({ kind: "retained", status: USABLE });
  await vi.waitFor(() => expect(setup.sockets).toHaveLength(1));
  const socket = setup.sockets[0] as FakeSocket;
  socket.open();
  expect(socket.sent).toEqual([TICKET]);
  socket.receive(welcome());
  socket.receive(
    attached(
      { kind: "projection_reset", baseline_frame_head: 9, reason: "retention_gap" },
      null,
      accessMode,
    ),
  );
  const resetMessage = reset();
  if (accessMode === "spectator") {
    const body = resetMessage.body as Record<string, unknown>;
    body.projection = {
      core: {
        access_mode: "spectator",
        standing: "enabled",
        role: null,
        room_status: "active",
        viewer_class: "public",
      },
      activity: { phase: "lobby" },
      action_offers: [],
    };
  }
  socket.receive(resetMessage);
  const sync = sentMessages(socket).find(
    (message) => message.type === "room.sync_ack",
  );
  expect(sync).toBeDefined();
  socket.receive(
    envelope(
      "room.sync_acked",
      { through_frame_head: 9 },
      String(sync?.message_id),
    ),
  );
  await starting;
  return socket;
}

describe("HostedLiveSessionController", () => {
  it("starts empty and installs only the authorized Reset before enabling Actions", async () => {
    const setup = fixture();
    expect(setup.controller.state.deliveryBatch).toBeNull();

    const socket = await openResetSession(setup);
    expect(setup.ticketCursors).toEqual([null]);
    expect(setup.controller.state).toMatchObject({
      status: "live",
      synchronized: true,
      canAct: true,
    });
    expect(setup.controller.state.deliveryBatch?.delivery).toHaveLength(1);
    expect(setup.controller.state.deliveryBatch?.delivery[0]?.kind).toBe(
      "projection_reset",
    );
    expect(JSON.stringify(setup.controller.state.deliveryBatch)).not.toMatch(
      /room_id|member_id|principal_id/u,
    );
    const sync = sentMessages(socket).find(
      (message) => message.type === "room.sync_ack",
    );
    expect(sync?.body).toEqual({
      through_frame_head: 9,
      sync_token: "opaque-sync-token",
    });
  });

  it.each(["usable", "disconnected"] as const)("reconnects from the acknowledged Cursor when retained transport health is %s", async (state) => {
    const setup = fixture();
    const first = await openResetSession(setup);
    first.receive(observation(10, 8));
    const observationAck = sentMessages(first).find(
      (message) => message.type === "observation.ack",
    );
    expect(observationAck?.body).toEqual({ through_frame_seq: 10 });
    first.receive(
      envelope(
        "observation.acked",
        { room_id: id("01"), member_id: id("02"), cursor: 10 },
        String(observationAck?.message_id),
      ),
    );
    expect(setup.controller.state.lastAcknowledgedFrameSeq).toBe(10);
    first.remoteClose();
    expect(setup.controller.state.canAct).toBe(false);
    setup.authority.resume.mockResolvedValue({
      ...USABLE, state, nextAction: state === "disconnected" ? "reconnect" : "continue",
    });

    const reconnecting = setup.controller.reconnect();
    await vi.waitFor(() => expect(setup.sockets).toHaveLength(2));
    const second = setup.sockets[1] as FakeSocket;
    second.open();
    second.receive(welcome());
    second.receive(
      attached(
        {
          kind: "retained_frames",
          cursor_exclusive: 10,
          through_frame_head: 12,
        },
        10,
      ),
    );
    second.receive(observation(11, 9, "8"));
    expect(
      sentMessages(second).some((message) => message.type === "room.sync_ack"),
    ).toBe(false);
    second.receive(observation(12, 10, "9"));
    const sync = sentMessages(second).find(
      (message) => message.type === "room.sync_ack",
    );
    expect(sync).toBeDefined();
    second.receive(
      envelope(
        "room.sync_acked",
        { through_frame_head: 12 },
        String(sync?.message_id),
      ),
    );
    await reconnecting;

    expect(setup.ticketCursors).toEqual([null, 10]);
    expect(setup.controller.state.status).toBe("live");
    expect(setup.controller.state.canAct).toBe(true);
    expect(setup.controller.state.deliveryBatch?.frame_head).toBe(12);
  });

  it("ignores an exact duplicate and fails closed on conflicting order", async () => {
    const setup = fixture();
    const socket = await openResetSession(setup);
    socket.receive(observation(10, 8));
    const before = setup.controller.state.deliveryBatch;
    socket.receive(observation(10, 8));
    expect(setup.controller.state.deliveryBatch).toBe(before);

    socket.receive(observation(10, 8, "f"));
    expect(setup.controller.state.status).toBe("disconnected");
    expect(setup.controller.state.canAct).toBe(false);
  });

  it.each([false, true])("adopts a recovery Cursor only after Reset installation (interrupted: %s)", async (interrupted) => {
    const setup = fixture();
    const first = await openResetSession(setup);
    first.receive(observation(10, 8));
    // The ACK reached the Runtime, but its receipt did not reach this client.
    first.remoteClose();
    let reconnecting = setup.controller.reconnect();
    await vi.waitFor(() => expect(setup.sockets).toHaveLength(2));
    let second = setup.sockets[1] as FakeSocket;
    second.open();
    second.receive(welcome());
    second.receive(attached({
      kind: "projection_reset", baseline_frame_head: 10, reason: "client_cursor_behind",
    }, 10));
    expect(setup.ticketCursors).toEqual([null, null]);
    expect(setup.controller.state.canAct).toBe(false);
    expect(setup.controller.state.lastAcknowledgedFrameSeq).toBeNull();
    if (interrupted) {
      second.remoteClose();
      await reconnecting;
      reconnecting = setup.controller.reconnect();
      await vi.waitFor(() => expect(setup.sockets).toHaveLength(3));
      second = setup.sockets[2] as FakeSocket;
      second.open();
      second.receive(welcome());
      second.receive(attached({
        kind: "projection_reset", baseline_frame_head: 10, reason: "client_cursor_behind",
      }, 10));
      expect(setup.ticketCursors).toEqual([null, null, null]);
      expect(setup.controller.state.lastAcknowledgedFrameSeq).toBeNull();
    }
    second.receive(reset(10));
    expect(setup.controller.state.lastAcknowledgedFrameSeq).toBe(10);
    const sync = sentMessages(second).find((message) => message.type === "room.sync_ack");
    second.receive(envelope("room.sync_acked", { through_frame_head: 10 }, String(sync?.message_id)));
    await reconnecting;
    expect(setup.controller.state.canAct).toBe(true);
    setup.controller.close();
  });

  it("rejects Catch-up that would skip its confirmed Cursor without a Reset", async () => {
    const setup = fixture();
    const first = await openResetSession(setup);
    first.remoteClose();
    const reconnecting = setup.controller.reconnect();
    await vi.waitFor(() => expect(setup.sockets).toHaveLength(2));
    const second = setup.sockets[1] as FakeSocket;
    second.open();
    second.receive(welcome());
    second.receive(attached({
      kind: "retained_frames", cursor_exclusive: 10, through_frame_head: 10,
    }, 10));
    await reconnecting;
    expect(setup.controller.state.status).toBe("disconnected");
    expect(setup.controller.state.lastAcknowledgedFrameSeq).toBeNull();
    expect(setup.controller.state.canAct).toBe(false);
    setup.controller.close();
  });

  it("returns matched Action receipts and never sends a routing selector", async () => {
    const setup = fixture();
    const socket = await openResetSession(setup);
    const receipt = setup.controller.submitAction({
      actionId: id("30"),
      basedOnRoomSeq: 7,
      actionType: "commit_move",
      payload: { selected_plan_id: "plan-a" },
    });
    const action = sentMessages(socket).find(
      (message) => message.type === "action.submit",
    );
    expect(action?.body).toEqual({
      action_id: id("30"),
      based_on_room_seq: 7,
      action_type: "commit_move",
      payload: { selected_plan_id: "plan-a" },
    });
    expect(JSON.stringify(action)).not.toMatch(
      /room_id|member_id|membership_id|principal_id/u,
    );
    socket.receive(
      envelope(
        "action.accepted",
        {
          room_id: id("01"),
          member_id: id("02"),
          action_id: id("30"),
          transition_id: id("31"),
          admitted_at: "2026-09-05T12:00:00Z",
          room_head: roomHead(8),
          duplicate: false,
        },
        String(action?.message_id),
      ),
    );
    await expect(receipt).resolves.toMatchObject({
      state: "accepted",
      actionId: id("30"),
      duplicate: false,
    });
    expect(setup.controller.state.canAct).toBe(false);
    socket.receive(observation(10, 8));
    expect(setup.controller.state.canAct).toBe(true);
  });

  it("never resubmits an uncertain Action after reconnect", async () => {
    const setup = fixture();
    const first = await openResetSession(setup);
    const uncertain = setup.controller.submitAction({
      actionId: id("35"),
      basedOnRoomSeq: 7,
      actionType: "commit_move",
      payload: { selected_plan_id: "plan-a" },
    });

    await expect(uncertain).rejects.toThrow(/receipt timed out/u);
    expect(setup.controller.state.message).toMatch(/outcome is uncertain/u);
    expect(sentMessages(first).filter((message) => message.type === "action.submit")).toHaveLength(1);

    const reconnecting = setup.controller.reconnect();
    await vi.waitFor(() => expect(setup.sockets).toHaveLength(2));
    const second = setup.sockets[1] as FakeSocket;
    second.open();
    second.receive(welcome());
    second.receive(attached({
      kind: "projection_reset",
      baseline_frame_head: 10,
      reason: "client_cursor_behind",
    }, null));
    second.receive(reset(10));
    const sync = sentMessages(second).find((message) => message.type === "room.sync_ack");
    second.receive(
      envelope(
        "room.sync_acked",
        { through_frame_head: 10 },
        String(sync?.message_id),
      ),
    );
    await reconnecting;

    expect(setup.controller.state).toMatchObject({ status: "live", synchronized: true });
    expect(sentMessages(second).filter((message) => message.type === "action.submit")).toHaveLength(0);
  });

  it("keeps spectators read-only and answers heartbeat without authority fields", async () => {
    const setup = fixture();
    const socket = await openResetSession(setup, "spectator");
    expect(setup.controller.state.canAct).toBe(false);
    await expect(
      setup.controller.submitAction({
        actionId: id("40"),
        basedOnRoomSeq: 7,
        actionType: "commit_move",
        payload: {},
      }),
    ).rejects.toThrow(/not ready/u);
    socket.receive(envelope("server.ping", {}));
    const pong = sentMessages(socket).find(
      (message) => message.type === "client.pong",
    );
    expect(pong?.body).toEqual({});
  });

  it("supports abortable bounded waits without polling", async () => {
    const setup = fixture();
    const abort = new AbortController();
    const waiting = setup.controller.waitFor(
      (snapshot) => snapshot.status === "live",
      { signal: abort.signal, timeoutMs: 1_000 },
    );
    abort.abort();
    await expect(waiting).rejects.toMatchObject({ name: "AbortError" });
  });

  it("clears authorized state when the retained session is invalidated", async () => {
    const setup = fixture();
    await openResetSession(setup);
    setup.sockets[0]?.remoteClose();
    setup.authority.issueStreamTicket.mockRejectedValueOnce(
      new ActivityClientHandoffError(
        "participant_session_authority_invalid",
        "The retained authority is no longer valid.",
        "return_to_task_setup",
        false,
      ),
    );

    await expect(setup.controller.reconnect()).resolves.toMatchObject({
      status: "setup_required",
      deliveryBatch: null,
      canAct: false,
    });
  });

  it("turns server slow-consumer closure and malformed frames into reconnectable state", async () => {
    const setup = fixture();
    const socket = await openResetSession(setup);
    socket.remoteClose();
    expect(setup.controller.state).toMatchObject({
      status: "disconnected",
      canAct: false,
    });

    const secondSetup = fixture();
    const second = await openResetSession(secondSetup);
    second.receiveRaw(new Uint8Array([1, 2, 3]));
    expect(secondSetup.controller.state.status).toBe("disconnected");
  });
});
