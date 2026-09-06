import { describe, expect, it, vi } from "vitest";

import {
  PUBLIC_PROJECTION_STREAM_VERSION,
  PUBLIC_PROJECTION_WS_SUBPROTOCOL,
  PublicProjectionSessionController,
  type WebSocketLike,
} from "./index";

const PUBLIC_ID = "a".repeat(32);
const STREAM_URL = `wss://stream.example/v1/hosted/public-runs/${PUBLIC_ID}/stream`;
const digest = (character: string) => `blake3:${character.repeat(64)}`;

class FakeSocket implements WebSocketLike {
  readonly readyState = 1;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent<unknown>) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  readonly sent: string[] = [];
  closed = false;

  send(value: string): void { this.sent.push(value); }
  close(): void { this.closed = true; }
  open(): void { this.onopen?.(new Event("open")); }
  receive(value: unknown): void {
    this.onmessage?.({ data: typeof value === "string" ? value : JSON.stringify(value) } as MessageEvent);
  }
  remoteClose(): void { this.onclose?.({} as CloseEvent); }
}

function roomHead(roomSequence: number) {
  return {
    room_seq: roomSequence,
    genesis_or_transition_hash: digest("1"),
    core_schema_version: "worldstream/core-room-state/v1",
    pack_digest: digest("2"),
    core_state_hash: digest("3"),
    activity_state_hash: digest("4"),
    authoritative_state_hash: digest("5"),
  };
}

function frame(
  kind: "projection_reset" | "observation",
  frameHead: number,
  roomSequence: number,
  bodyOverride: Record<string, unknown> = {},
) {
  const head = roomHead(roomSequence);
  const body = kind === "projection_reset"
    ? {
        room_head: head,
        room_health: "healthy",
        integrity_generation: 0,
        baseline_frame_head: frameHead,
        reset_reason: "first_attach",
        projection_schema: "agent-heist/projection/v1",
        projection: {
          core: {
            access_mode: "spectator",
            standing: "enabled",
            role: null,
            room_status: "active",
            viewer_class: "public",
          },
          activity: { phase: "lobby" },
          action_offers: [],
        },
        projection_hash: digest("6"),
        ...bodyOverride,
      }
    : {
        frame_seq: frameHead,
        cause_room_seq: roomSequence,
        frame_kind: "activity",
        observation_schema: "agent-heist/observation/v1",
        observation: { phase: "negotiation", marker: frameHead },
        frame_payload_hash: digest("7"),
        ...bodyOverride,
      };
  return {
    version: PUBLIC_PROJECTION_STREAM_VERSION,
    batch: {
      pack: {
        id: "worldstream.agent-heist",
        version: "0.2.0",
        digest: digest("2"),
      },
      room_head: head,
      frame_head: frameHead,
      delivery: [{ kind, body }],
    },
  };
}

function fixture() {
  const sockets: FakeSocket[] = [];
  const controller = new PublicProjectionSessionController({
    streamUrl: STREAM_URL,
    connectTimeoutMs: 1_000,
    webSocketFactory: (url, protocols, options) => {
      expect(url).toBe(STREAM_URL);
      expect(protocols).toEqual([PUBLIC_PROJECTION_WS_SUBPROTOCOL]);
      expect(options.headers).toEqual({});
      const socket = new FakeSocket();
      sockets.push(socket);
      return socket;
    },
  });
  return { controller, sockets };
}

async function start(fixtureValue: ReturnType<typeof fixture>): Promise<FakeSocket> {
  const starting = fixtureValue.controller.start({ kind: "direct" });
  await vi.waitFor(() => expect(fixtureValue.sockets).toHaveLength(1));
  const socket = fixtureValue.sockets[0] as FakeSocket;
  socket.open();
  socket.receive(frame("projection_reset", 4, 7));
  await starting;
  return socket;
}

describe("PublicProjectionSessionController", () => {
  it("starts empty, sends nothing, and installs only a public spectator Reset", async () => {
    const setup = fixture();
    expect(setup.controller.state.deliveryBatch).toBeNull();
    const socket = await start(setup);
    expect(socket.sent).toEqual([]);
    expect(setup.controller.state).toMatchObject({
      status: "live",
      synchronized: true,
      canAct: false,
    });
    expect(setup.controller.state.deliveryBatch?.delivery[0]?.kind).toBe("projection_reset");
    expect(JSON.stringify(setup.controller.state.deliveryBatch)).not.toMatch(
      /room_id|member_id|principal_id|ticket|replay/u,
    );
    await expect(setup.controller.submitAction({
      actionId: "01J00000000000000000000001",
      basedOnRoomSeq: 7,
      actionType: "commit_move",
      payload: {},
    })).rejects.toThrow(/cannot submit Actions/u);
  });

  it("accepts only contiguous observations for the pinned Pack", async () => {
    const setup = fixture();
    const socket = await start(setup);
    socket.receive(frame("observation", 5, 8));
    expect(setup.controller.state.deliveryBatch?.frame_head).toBe(5);
    expect(setup.controller.state.canAct).toBe(false);
    socket.receive(frame("observation", 7, 9));
    expect(setup.controller.state).toMatchObject({
      status: "disconnected",
      deliveryBatch: null,
      canAct: false,
    });
  });

  it("fails closed on private routing material or a non-public Reset", async () => {
    for (const replacement of [
      { principal_id: "private" },
      {
        projection: {
          core: {
            access_mode: "participant",
            standing: "enabled",
            role: "navigator",
            viewer_class: "participant",
          },
          activity: {},
          action_offers: [],
        },
      },
    ]) {
      const setup = fixture();
      const starting = setup.controller.start({ kind: "direct" });
      await vi.waitFor(() => expect(setup.sockets).toHaveLength(1));
      setup.sockets[0]?.open();
      setup.sockets[0]?.receive(frame("projection_reset", 0, 0, replacement));
      await starting;
      expect(setup.controller.state.status).toBe("disconnected");
    }
  });

  it("reconnects from empty state and requires a fresh Reset", async () => {
    const setup = fixture();
    const first = await start(setup);
    first.remoteClose();
    expect(setup.controller.state.deliveryBatch).toBeNull();
    const reconnecting = setup.controller.reconnect();
    await vi.waitFor(() => expect(setup.sockets).toHaveLength(2));
    const second = setup.sockets[1] as FakeSocket;
    second.open();
    second.receive(frame("projection_reset", 10, 12));
    await reconnecting;
    expect(setup.controller.state.deliveryBatch?.frame_head).toBe(10);
  });

  it("rejects credential-bearing URLs before opening a socket", () => {
    expect(() => new PublicProjectionSessionController({
      streamUrl: `${STREAM_URL}?ticket=secret`,
    })).toThrow(/URL is invalid/u);
    expect(() => new PublicProjectionSessionController({
      streamUrl: STREAM_URL.replace("wss:", "http:"),
    })).toThrow(/URL is invalid/u);
  });
});
