import { describe, expect, it, vi } from "vitest";

import {
  MAX_MESSAGE_BYTES,
  InMemoryWorldStreamTransport,
  WebSocketWorldStreamTransport,
  WORLDSTREAM_WS_SUBPROTOCOL,
  ProtocolValidationError,
  WORLDSTREAM_PROTOCOL,
  createActionSubmitRequest,
  createRequest,
  decodeMessage,
  encodeMessage,
  validateServerWelcome,
  type ProtocolMessage,
  type ServerWelcomeMessage,
  type WebSocketLike,
} from "./transport";

const ROOM_ID = "01J00000000000000000000001";
const MEMBER_ID = "01J00000000000000000000002";
const ACTION_ID = "01J00000000000000000000003";
const MESSAGE_ID = "01J00000000000000000000004";

const welcome: ServerWelcomeMessage = {
  protocol: WORLDSTREAM_PROTOCOL,
  type: "server.welcome",
  message_id: MESSAGE_ID,
  request_id: MESSAGE_ID,
  body: {
    session_id: "01J00000000000000000000005",
    selected_protocol: WORLDSTREAM_PROTOCOL,
    server_version: "0.1.0-dev",
    heartbeat_interval_ms: 15_000,
    maximum_message_bytes: MAX_MESSAGE_BYTES,
    authenticated_principal: {
      principal_id: "principal",
      kind: "agent",
    },
  },
};

describe("WorldStream console protocol boundary", () => {
  it("round-trips and validates the documented server welcome session", () => {
    const encoded = encodeMessage(welcome);
    const decoded = decodeMessage(encoded);

    expect(decoded).toEqual(welcome);
    expect(validateServerWelcome(decoded).body.session_id).toBe("01J00000000000000000000005");
  });

  it("rejects unknown fields, invalid ULIDs, and oversized JSON before use", () => {
    expect(() =>
      decodeMessage(
        JSON.stringify({
          ...welcome,
          body: { ...welcome.body, unexpected: true },
        }),
      ),
    ).toThrow(ProtocolValidationError);

    expect(() =>
      decodeMessage(
        JSON.stringify({
          ...welcome,
          body: { ...welcome.body, session_id: "not-a-session" },
        }),
      ),
    ).toThrow("session_id must be a 26-character Crockford ULID");

    expect(() => decodeMessage(`{"x":"${"a".repeat(MAX_MESSAGE_BYTES)}"}`)).toThrow(
      `message exceeds ${MAX_MESSAGE_BYTES}-byte limit`,
    );
  });

  it("keeps attach, sync, observation ack, and action requests typed and strict", () => {
    const requests: ProtocolMessage[] = [
      createRequest("room.attach", MESSAGE_ID, {
        room_id: ROOM_ID,
        member_id: MEMBER_ID,
        after_frame_seq: null,
      }),
      createRequest("room.sync_ack", MESSAGE_ID, {
        room_id: ROOM_ID,
        member_id: MEMBER_ID,
        through_frame_head: 191,
        sync_token: "opaque-session-bound-token",
      }),
      createRequest("observation.ack", MESSAGE_ID, {
        room_id: ROOM_ID,
        member_id: MEMBER_ID,
        through_frame_seq: 192,
      }),
      createRequest("action.submit", MESSAGE_ID, {
        room_id: ROOM_ID,
        member_id: MEMBER_ID,
        action_id: ACTION_ID,
        based_on_room_seq: 91,
        action_type: "commit_move",
        payload: { selected_plan_id: ROOM_ID, contribute_required_resource: true },
      }),
    ];

    expect(requests.map((request) => decodeMessage(encodeMessage(request)))).toEqual(requests);
    expect(() =>
      createRequest("room.attach", MESSAGE_ID, {
        room_id: ROOM_ID,
        member_id: MEMBER_ID,
        after_frame_seq: -1,
      }),
    ).toThrow("after_frame_seq must be an integer in range");
    expect(
      createRequest("action.submit", MESSAGE_ID, {
        room_id: ROOM_ID,
        member_id: MEMBER_ID,
        action_id: ACTION_ID,
        based_on_room_seq: 91,
        action_type: "commit_move",
        payload: [ROOM_ID, true],
      }).body.payload,
    ).toEqual([ROOM_ID, true]);
  });

  it("builds action submissions from an exact head without implying acceptance", () => {
    const request = createActionSubmitRequest({
      message_id: MESSAGE_ID,
      room_id: ROOM_ID,
      member_id: MEMBER_ID,
      action_id: ACTION_ID,
      based_on_room_seq: 91,
      action_type: "propose_plan",
      payload: { route: "service", entry_window: "early" },
    });

    expect(request.type).toBe("action.submit");
    expect(request.body.based_on_room_seq).toBe(91);
    expect(request.body.action_id).toBe(ACTION_ID);
    expect(request.body.payload).toEqual({ route: "service", entry_window: "early" });
    expect(request).not.toHaveProperty("accepted");
  });

  it("provides a controllable fake transport without opening a socket", async () => {
    const transport = new InMemoryWorldStreamTransport(welcome);
    const received: ProtocolMessage[] = [];
    const unsubscribe = transport.subscribe((message) => received.push(message));
    const attach = createRequest("room.attach", MESSAGE_ID, {
      room_id: ROOM_ID,
      member_id: MEMBER_ID,
      after_frame_seq: 184,
    });

    await expect(transport.send(attach)).rejects.toThrow("transport is not open");
    await expect(transport.connect()).resolves.toEqual(welcome);
    await transport.send(attach);
    transport.receive(welcome);

    expect(transport.state).toBe("open");
    expect(transport.sentMessages).toEqual([attach]);
    expect(received).toEqual([welcome]);

    unsubscribe();
    transport.close();
    expect(transport.state).toBe("closed");
    expect(() => transport.receive(welcome)).toThrow("transport is not open");
  });
});

class FakeWebSocket implements WebSocketLike {
  readonly readyState = 1;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent<unknown>) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  readonly sent: string[] = [];
  closed = false;

  send(data: string): void { this.sent.push(data); }
  close(): void { this.closed = true; }
  open(): void { this.onopen?.(new Event("open")); }
  receive(data: string): void { this.onmessage?.({ data } as MessageEvent<string>); }
  fail(): void { this.onerror?.(new Event("error")); }
}

describe("WebSocketWorldStreamTransport", () => {
  it("uses the configured URL and subprotocol, and passes bearer auth only to a header-capable adapter", async () => {
    const socket = new FakeWebSocket();
    let observed: { url: string; protocols: readonly string[]; headers: Readonly<Record<string, string>> } | undefined;
    const transport = new WebSocketWorldStreamTransport({
      url: "wss://worldstream.test/v1/rooms/room",
      clientName: "console",
      clientVersion: "test",
      bearer: "wsb1:secret",
      webSocketFactory: (url, protocols, options) => {
        observed = { url, protocols, headers: options.headers };
        return socket;
      },
    });
    const connecting = transport.connect();
    expect(observed).toEqual({
      url: "wss://worldstream.test/v1/rooms/room",
      protocols: [WORLDSTREAM_WS_SUBPROTOCOL],
      headers: { Authorization: "Bearer wsb1:secret" },
    });
    expect(observed?.url).not.toContain("secret");
    socket.open();
    expect(JSON.parse(socket.sent[0])).toMatchObject({ type: "client.hello", body: { mode: "participant" } });
    socket.receive(JSON.stringify(welcome));
    await expect(connecting).resolves.toEqual(welcome);
    transport.close();
  });

  it("issues a native browser ticket without leaking the bearer", async () => {
    let socket: FakeWebSocket | undefined;
    class NativeWebSocket extends FakeWebSocket {
      constructor(readonly url: string, readonly protocols: readonly string[]) {
        super();
        socket = this;
      }
    }
    vi.stubGlobal("WebSocket", NativeWebSocket);
    const secret = "wsb1:long-lived-browser-secret";
    const ticket = `wst1:${"ab".repeat(32)}`;
    let request: { url: string; init?: RequestInit } | undefined;
    const transport = new WebSocketWorldStreamTransport({
      url: "wss://worldstream.test/v1/stream",
      clientName: "console",
      clientVersion: "test",
      bearer: secret,
      fetch: async (url, init) => {
        request = { url, init };
        return { ok: true, json: async () => ({ version: "browser_ws_ticket.v1", ticket, expires_in_ms: 15_000 }) };
      },
    });
    const connecting = transport.connect();
    await vi.waitFor(() => expect(request?.url).toBe("https://worldstream.test/v1/stream/ticket"));
    expect(request?.init?.headers).toMatchObject({ Authorization: `Bearer ${secret}` });
    expect(request?.url).not.toContain(secret);
    await vi.waitFor(() => expect(socket).toBeDefined());
    socket?.open();
    expect(socket?.sent[0]).toBe(ticket);
    expect(socket?.sent[0]).not.toContain(secret);
    expect(JSON.parse(socket?.sent[1] ?? "{}")).toMatchObject({ type: "client.hello" });
    socket?.receive(JSON.stringify(welcome));
    await expect(connecting).resolves.toEqual(welcome);
    transport.close();
    vi.unstubAllGlobals();
  });

  it("fails closed on ticket issuance without returning the bearer or ticket detail", async () => {
    const secret = "wsb1:long-lived-browser-secret";
    const transport = new WebSocketWorldStreamTransport({
      url: "wss://worldstream.test/v1/stream",
      clientName: "console",
      clientVersion: "test",
      bearer: secret,
      fetch: async () => ({ ok: false, json: async () => ({ error: secret }) }),
    });
    await expect(transport.connect()).rejects.toThrow("browser authorization failed");
    expect(transport.state).toBe("closed");
  });

  it("validates server delivery, answers ping, and closes safely on malformed input", async () => {
    const socket = new FakeWebSocket();
    const transport = new WebSocketWorldStreamTransport({
      url: "ws://localhost",
      clientName: "console",
      clientVersion: "test",
      webSocketFactory: () => socket,
    });
    const received: ProtocolMessage[] = [];
    transport.subscribe((message) => received.push(message));
    const connecting = transport.connect();
    socket.open();
    socket.receive(JSON.stringify(welcome));
    await connecting;

    socket.receive(JSON.stringify({
      protocol: WORLDSTREAM_PROTOCOL,
      type: "server.ping",
      message_id: "01J00000000000000000000007",
      body: {},
    }));
    expect(JSON.parse(socket.sent.at(-1) ?? "{}" )).toMatchObject({ type: "client.pong", body: {} });
    socket.receive(JSON.stringify({
      protocol: WORLDSTREAM_PROTOCOL,
      type: "observation.deliver",
      message_id: "01J00000000000000000000008",
      body: {
        room_id: ROOM_ID,
        member_id: MEMBER_ID,
        frame_seq: 3,
        cause_room_seq: 2,
        frame_kind: "observation",
        observation_schema: "test/v1",
        observation: { public: true },
        frame_payload_hash: "sha256:test",
      },
    }));
    expect(received.at(-1)?.type).toBe("observation.deliver");
    socket.receive(JSON.stringify({ ...welcome, body: { ...welcome.body, unexpected: true } }));
    expect(transport.state).toBe("closed");
    expect(socket.closed).toBe(true);
  });
});
