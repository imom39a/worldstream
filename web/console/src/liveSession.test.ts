import { describe, expect, it, vi } from "vitest";

import { resultFixture } from "./fixture";
import {
  initialLiveSessionView,
  isCurrentActionOffer,
  latestObservationFrameToAck,
  createLiveSessionTransport,
  consumeLiveSessionBootstrap,
  projectLiveSessionMessage,
  projectLiveSessionMessageForSession,
  readLiveSessionConfig,
  replaceLiveTransport,
} from "./liveSession";
import {
  MAX_MESSAGE_BYTES,
  InMemoryWorldStreamTransport,
  WebSocketWorldStreamTransport,
  WORLDSTREAM_PROTOCOL,
  type ServerWelcomeMessage,
  type WebSocketLike,
} from "./transport";

const id = (last: string) => `01J0000000000000000000000${last}`.slice(0, 26);

class FakeSocket implements WebSocketLike {
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
}

const welcome: ServerWelcomeMessage = {
  protocol: WORLDSTREAM_PROTOCOL,
  type: "server.welcome",
  message_id: id("01"),
  body: {
    session_id: id("02"),
    selected_protocol: WORLDSTREAM_PROTOCOL,
    server_version: "test",
    heartbeat_interval_ms: 60_000,
    maximum_message_bytes: MAX_MESSAGE_BYTES,
    authenticated_principal: { principal_id: "principal", kind: "agent" },
  },
};

describe("opt-in live console session", () => {
  it("accepts only the one-shot in-memory live bootstrap shape", () => {
    const config = readLiveSessionConfig({
      version: "worldstream.console.live.v1",
      endpoint: "ws://127.0.0.1:9410/v1/stream",
      clientName: "browser-smoke",
      clientVersion: "0.1.0",
      roomId: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
      memberId: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
      bearer: `wsb1:${"ab".repeat(32)}`,
    });
    expect(config?.endpoint).toBe("ws://127.0.0.1:9410/v1/stream");
    expect(readLiveSessionConfig({ ...config, version: "wrong" })).toBeNull();
    expect(readLiveSessionConfig({ ...config, endpoint: "https://127.0.0.1/stream" })).toBeNull();
    expect(readLiveSessionConfig({ ...config, bearer: "secret" })).toBeNull();
  });

  it("consumes and deletes the bootstrap before React receives long-lived config", () => {
    const target = {
      __WORLDSTREAM_LIVE_SESSION__: {
        version: "worldstream.console.live.v1",
        endpoint: "ws://127.0.0.1:9410/v1/stream",
        clientName: "browser-smoke",
        clientVersion: "0.1.0",
        roomId: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        memberId: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
        bearer: `wsb1:${"cd".repeat(32)}`,
      },
    };
    const consumed = consumeLiveSessionBootstrap(target);
    expect(target.__WORLDSTREAM_LIVE_SESSION__).toBeUndefined();
    expect(consumed.config?.bearer).toBeUndefined();
    expect(consumed.transport).not.toBeNull();
    consumed.transport?.close();
  });

  it("constructs native browser sessions through the short-lived ticket seam", async () => {
    let socket!: FakeSocket;
    class NativeWebSocket extends FakeSocket {
      constructor(readonly url: string, readonly protocols: readonly string[]) {
        super();
        socket = this;
      }
    }
    vi.stubGlobal("WebSocket", NativeWebSocket);
    const bearer = "wsb1:live-session-secret";
    const ticket = `wst1:${"cd".repeat(32)}`;
    let authorization: string | undefined;
    const transport = createLiveSessionTransport({
      endpoint: "wss://worldstream.test/v1/stream",
      clientName: "console",
      clientVersion: "test",
      roomId: id("04"),
      memberId: id("05"),
      bearer,
      fetch: async (_url, init) => {
        authorization = (init?.headers as Record<string, string>).Authorization;
        return { ok: true, json: async () => ({ version: "browser_ws_ticket.v1", ticket, expires_in_ms: 15_000 }) };
      },
    });
    const connecting = transport.connect();
    await vi.waitFor(() => expect(authorization).toBe(`Bearer ${bearer}`));
    await vi.waitFor(() => expect(socket).toBeDefined());
    socket.open();
    expect(socket.sent[0]).toBe(ticket);
    expect(socket.sent[0]).not.toContain(bearer);
    socket.receive(JSON.stringify(welcome));
    await expect(connecting).resolves.toEqual(welcome);
    transport.close();
    vi.unstubAllGlobals();
  });

  it("flushes retained observation ACK eligibility only after the sync barrier", () => {
    expect(latestObservationFrameToAck([10, 11, 12], false)).toBeNull();
    expect(latestObservationFrameToAck([10, 11, 12], true)).toBe(12);
    expect(latestObservationFrameToAck([], true)).toBeNull();
  });

  it("installs a replacement even when the old hook transport throws or closes late", () => {
    const replacement = new InMemoryWorldStreamTransport(welcome);
    const throwing = {
      state: "open" as const,
      connect: async () => welcome,
      send: async () => undefined,
      subscribe: () => () => undefined,
      close: () => { throw new Error("late adapter close"); },
    };
    expect(replaceLiveTransport(throwing, () => replacement)).toBe(replacement);

    let closeObserved = false;
    const slow = {
      ...throwing,
      close: () => { closeObserved = true; },
    };
    const secondReplacement = new InMemoryWorldStreamTransport(welcome);
    expect(replaceLiveTransport(slow, () => secondReplacement)).toBe(secondReplacement);
    expect(closeObserved).toBe(true);
  });

  it("accepts only the exact installed Action Offer identity and schema", () => {
    const offer = resultFixture.participant.offers[0];
    expect(offer).toBeDefined();
    if (!offer) throw new Error("fixture must include an Action Offer");
    expect(isCurrentActionOffer(resultFixture, { offerId: offer.id, schemaDigest: offer.schemaDigest, actionType: offer.actionType })).toBe(true);
    expect(isCurrentActionOffer(resultFixture, { offerId: offer.id, schemaDigest: "blake3:stale", actionType: offer.actionType })).toBe(false);
    expect(isCurrentActionOffer(resultFixture, { offerId: offer.id, schemaDigest: offer.schemaDigest, actionType: "commit_move" })).toBe(false);
    expect(isCurrentActionOffer(resultFixture, { offerId: "15:propose_plan:0", schemaDigest: offer.schemaDigest, actionType: offer.actionType })).toBe(false);
  });

  it("connects through the injectable socket and reaches the attached lifecycle state", async () => {
    let socket!: FakeSocket;
    const transport = new WebSocketWorldStreamTransport({
      url: "wss://worldstream.test/session",
      clientName: "console-test",
      clientVersion: "0.1.0",
      webSocketFactory: () => {
        socket = new FakeSocket();
        return socket;
      },
      createMessageId: () => id("03"),
    });

    const connection = transport.connect();
    socket.open();
    expect(JSON.parse(socket.sent[0])).toMatchObject({ type: "client.hello" });
    socket.receive(JSON.stringify(welcome));
    await expect(connection).resolves.toEqual(welcome);
    expect(transport.state).toBe("open");

    const attached = projectLiveSessionMessage(
      projectLiveSessionMessage(initialLiveSessionView(resultFixture), welcome, resultFixture),
      { ...welcome, type: "room.attached", body: {
        room_id: id("04"), member_id: id("05"), frame_head: 9, sync_token: "opaque",
        sync: { kind: "projection_reset", baseline_frame_head: 9, reason: "retention_gap" },
      } },
      resultFixture,
    );
    expect(attached.status).toBe("attached");
    expect(attached.fixture.runtime.frame.head).toBe(9);
    transport.close();
  });

  it("maps live delivery and server errors without exposing protocol details", () => {
    const attached = projectLiveSessionMessage(
      projectLiveSessionMessage(initialLiveSessionView(resultFixture), welcome, resultFixture),
      { ...welcome, type: "room.attached", body: {
        room_id: id("04"), member_id: id("05"), frame_head: 9, sync_token: "opaque",
        sync: { kind: "projection_reset", baseline_frame_head: 9, reason: "retention_gap" },
      } },
      resultFixture,
    );
    const reset = projectLiveSessionMessage(
      attached,
      { ...welcome, type: "projection.reset", body: {
        baseline_frame_head: 9,
        room_head: { room_seq: 15, genesis_or_transition_hash: "blake3:head" },
        projection: {
          action_offers: [
            { action_type: "commit_move", payload_schema_digest: "blake3:offer", eligibility_window: { opens_at: "2026-01-01T00:00:00Z", deadline: "2026-01-01T00:01:00Z" } },
            { action_type: "private_internal_action", payload_schema_digest: "blake3:secret" },
          ],
        },
      } },
      resultFixture,
    );
    expect(reset.status).toBe("attached");
    expect(reset.fixture.runtime.frame.reset).toBe("Installed");
    expect(reset.fixture.participant.offers.map((offer) => offer.actionType)).toEqual(["commit_move"]);
    expect(reset.fixture.participant.exactHead).toBe("blake3:head");

    const live = projectLiveSessionMessage(
      reset,
      { ...welcome, type: "room.sync_acked", body: { through_frame_head: 9 } },
      resultFixture,
    );
    expect(live.status).toBe("live");
    expect(live.fixture.runtime.transport).toBe("Live");
    expect(live.fixture.runtime.frame.delivery).toBe("Live");

    const delivered = projectLiveSessionMessage(
      live,
      { ...welcome, type: "observation.deliver", body: { frame_seq: 10, cursor: 10 } },
      resultFixture,
    );
    expect(delivered.status).toBe("live");

    const transitioned = projectLiveSessionMessage(
      delivered,
      { ...welcome, type: "observation.deliver", body: {
        frame_seq: 11,
        cursor: 11,
        observation: {
          activity: {
            phase: "negotiation",
            phase_generation: 2,
            action_offers: [{ action_type: "publish_clue", payload_schema_digest: "blake3:publish" }],
            private_clues: [{ clue_id: "route", claim_code: "authorized-claim" }],
          },
        },
      } },
      resultFixture,
    );
    expect(transitioned.fixture.phase).toBe("Negotiation");
    expect(transitioned.fixture.participant.offers.map((offer) => offer.actionType)).toEqual(["publish_clue"]);
    expect(transitioned.fixture.participant.privateClues).toEqual([{ clueId: "route", claimCode: "authorized-claim" }]);

    const briefingFrame = projectLiveSessionMessage(
      live,
      { ...welcome, type: "observation.deliver", body: {
        frame_seq: 2,
        cursor: 2,
        room_head: { room_seq: 2, genesis_or_transition_hash: "blake3:seq2" },
        observation: { activity: { phase: "briefing", phase_generation: 1, action_offers: [{ action_type: "inspect_clue", payload_schema_digest: "blake3:inspect" }] } },
      } },
      resultFixture,
    );
    const negotiationFrame = projectLiveSessionMessage(
      briefingFrame,
      { ...welcome, type: "observation.deliver", body: {
        frame_seq: 3,
        cursor: 3,
        room_head: { room_seq: 3, genesis_or_transition_hash: "blake3:seq3" },
        observation: { activity: { phase: "negotiation", phase_generation: 2, action_offers: [{ action_type: "publish_clue", payload_schema_digest: "blake3:publish" }] } },
      } },
      resultFixture,
    );
    expect(briefingFrame.fixture.roomSequence).toBe(2);
    expect(briefingFrame.fixture.phase).toBe("Briefing");
    expect(negotiationFrame.fixture.roomSequence).toBe(3);
    expect(negotiationFrame.fixture.phase).toBe("Negotiation");
    expect(negotiationFrame.fixture.participant.offers.map((offer) => offer.actionType)).toEqual(["publish_clue"]);

    const failed = projectLiveSessionMessage(delivered, { ...welcome, type: "error", body: {} }, resultFixture);
    expect(failed.status).toBe("error");
    expect(failed.error).toBe("The live WorldStream session reported an error.");
    expect(failed.error).not.toContain("Bearer");
    expect(failed.fixture.runtime.transport).toBe("Disconnected");
  });

  it("projects only public activity fields and preserves offers across offer-less frames", () => {
    const reset = projectLiveSessionMessage(
      initialLiveSessionView(resultFixture),
      { ...welcome, type: "projection.reset", body: {
        room_head: { room_seq: 21, genesis_or_transition_hash: "blake3:head", core_state_hash: "blake3:core", activity_state_hash: "blake3:activity", authoritative_state_hash: "blake3:aggregate" },
        room_health: "healthy",
        integrity_generation: 9,
        projection: {
          activity: {
            phase: "Commitment",
            phase_generation: 8,
            phase_deadline: "2026-08-21T12:00:00Z",
            seats: [{ role: "navigator", present: true }, { role: "insider", present: true }, { role: "broker", present: false }],
            public_claims: [{ clue_id: "route", claim_code: "route_public" }],
            commitment_count: 1,
            private_clues: ["secret-clue"],
            own_commitment: "sealed-value",
          },
          action_offers: [{ action_type: "commit_move", payload_schema_digest: "blake3:offer" }],
        },
      } },
      resultFixture,
    );

    expect(reset.fixture.phase).toBe("Commitment");
    expect(reset.fixture.phaseDeadline).toBe("2026-08-21T12:00:00Z");
    expect(reset.fixture.deadline).toBe("2026-08-21T12:00:00Z");
    expect(reset.fixture.roomSequence).toBe(21);
    expect(reset.fixture.roles.find((role) => role.role === "Broker")?.presence).toBe("Awaiting");
    expect(reset.fixture.participant.offers.map((offer) => offer.actionType)).toEqual(["commit_move"]);
    expect(JSON.stringify(reset.fixture)).not.toContain("secret-clue");
    expect(JSON.stringify(reset.fixture)).not.toContain("sealed-value");

    const delivered = projectLiveSessionMessage(
      reset,
      { ...welcome, type: "observation.deliver", body: { frame_seq: 22, cause_room_seq: 22, observation: { reason: "timer_fired" } } },
      resultFixture,
    );
    expect(delivered.fixture.participant.offers.map((offer) => offer.actionType)).toEqual(["commit_move"]);
    expect(delivered.fixture.phase).toBe("Commitment");
  });

  it("surfaces stale exact-head rejection and only clears it after synchronization", () => {
    const pending = {
      ...initialLiveSessionView(resultFixture),
      status: "live" as const,
      action: { state: "submitting" as const, actionId: id("10"), requestId: id("11"), basedOnRoomSeq: 15, message: "pending" },
      fixture: { ...resultFixture, runtime: { ...resultFixture.runtime, transport: "Live" as const, recovery: "Active" as const, frame: { ...resultFixture.runtime.frame, delivery: "Live" as const } } },
    };
    const stale = projectLiveSessionMessage(
      pending,
      { ...welcome, type: "action.rejected", request_id: id("11"), body: {
        action_id: id("10"), code: "stale_room_state", message: "stale", current_room_seq: 16,
        action_offers: [{ action_type: "commit_move", payload_schema_digest: "blake3:new" }],
      } },
      resultFixture,
    );
    expect(stale.action.state).toBe("stale");
    expect(stale.fixture.roomSequence).toBe(16);
    expect(stale.fixture.participant.offers[0]?.schemaDigest).toBe("blake3:new");

    const synchronized = projectLiveSessionMessage(
      stale,
      { ...welcome, type: "room.sync_acked", body: { through_frame_head: 22 } },
      resultFixture,
    );
    expect(synchronized.action.state).toBe("idle");
    expect(synchronized.action.actionId).toBeNull();
    expect(synchronized.fixture.runtime.frame.delivery).toBe("Live");
  });

  it("keeps faulted and quarantined Room health distinct from transport recovery", () => {
    const faulted = projectLiveSessionMessage(initialLiveSessionView(resultFixture), { ...welcome, type: "error", body: { code: "room_faulted", message: "safe", retryable: false } }, resultFixture);
    expect(faulted.fixture.runtime.roomHealth).toBe("Faulted");
    expect(faulted.fixture.runtime.recovery).toBe("Inactive");

    const quarantined = projectLiveSessionMessage(initialLiveSessionView(resultFixture), { ...welcome, type: "error", body: { code: "room_quarantined", message: "safe", retryable: false } }, resultFixture);
    expect(quarantined.fixture.runtime.roomHealth).toBe("Quarantined");
    expect(quarantined.fixture.runtime.recovery).toBe("Inactive");
  });

  it("fails closed when a server message targets another Room membership", () => {
    const initial = initialLiveSessionView(resultFixture);
    const rejected = projectLiveSessionMessageForSession(
      initial,
      { ...welcome, type: "projection.reset", body: {
        room_id: id("99"), member_id: id("98"), room_head: { room_seq: 16, genesis_or_transition_hash: "blake3:wrong" },
        room_health: "healthy", integrity_generation: 1, baseline_frame_head: 1, reset_reason: "retention_gap",
        projection_schema: "fixture/v1", projection: { action_offers: [] }, projection_hash: "blake3:projection",
      } },
      resultFixture,
      { roomId: id("04"), memberId: id("05") },
    );
    expect(rejected.status).toBe("error");
    expect(rejected.error).toBe("Live session received a response for a different Room membership.");
    expect(JSON.stringify(rejected)).not.toContain(id("99"));
    expect(JSON.stringify(rejected)).not.toContain(id("98"));
  });

  it("does not enter Live for a sync acknowledgement ahead of the attached frame head", () => {
    const attached = projectLiveSessionMessage(
      projectLiveSessionMessage(initialLiveSessionView(resultFixture), welcome, resultFixture),
      { ...welcome, type: "room.attached", body: {
        room_id: id("04"), member_id: id("05"), frame_head: 9, sync_token: "opaque",
        sync: { kind: "projection_reset", baseline_frame_head: 9, reason: "retention_gap" },
      } },
      resultFixture,
    );
    const early = projectLiveSessionMessage(attached, { ...welcome, type: "room.sync_acked", body: { through_frame_head: 8 } }, resultFixture);
    expect(early.status).toBe("attached");
    expect(early.fixture.runtime.recovery).toBe("CatchingUp");
    expect(early.fixture.runtime.frame.delivery).toBe("Catching up");
  });
});
