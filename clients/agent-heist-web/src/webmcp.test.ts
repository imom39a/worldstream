import { describe, expect, it, vi } from "vitest";

import {
  type HostedLiveSessionController,
  type HostedLiveSessionSnapshot,
} from "@worldstream/client";

import {
  AgentHeistWebMcpBridge,
  registerAgentHeistWebMcp,
  type AgentHeistWebMcpTool,
} from "./webmcp";
import type { AgentHeistLiveState, AgentHeistReadyState } from "./liveAdapter";

const digest = (character: string) => `blake3:${character.repeat(64)}`;
const ACTION_ID = "01J00000000000000000000099";

function ready(
  accessMode: "participant" | "spectator" = "participant",
  overrides: Partial<AgentHeistReadyState> = {},
): AgentHeistReadyState {
  return {
    kind: "ready",
    pack: {
      id: "worldstream.agent-heist",
      version: "0.2.0",
      digest: digest("1"),
    },
    roomHead: {
      genesisOrTransitionHash: digest("2"),
      authoritativeStateHash: digest("3"),
    },
    roomSequence: 18,
    frameHead: 41,
    authorization: {
      accessMode,
      role: accessMode === "participant" ? "navigator" : null,
    },
    projection: {
      phase: "commitment",
      phaseGeneration: 3,
      phaseStart: "2026-09-05T12:00:00Z",
      phaseDeadline: "2026-09-05T12:01:00Z",
      seats: [
        { role: "navigator", present: true },
        { role: "insider", present: true },
        { role: "broker", present: true },
      ],
      publicClaims: [{ clueId: "route", claimCode: "service_stairs" }],
      plans: [{
        planId: "plan-ember",
        proposerRole: "broker",
        route: "service_stairs",
        entryWindow: "camera_pause",
        requiredTool: "brass_token",
        extraction: "gallery_c",
        endorsements: 2,
        challenges: 0,
      }],
      challenges: [],
      commitmentCount: 1,
      outcome: null,
      privateClues: accessMode === "participant"
        ? [{ clueId: "camera", ownerRole: "navigator", claimCode: "eleven_second_pause" }]
        : [],
      ownCommitment: null,
      addressedOffers: accessMode === "participant"
        ? [{
            offerId: "exchange-1",
            senderRole: "broker",
            offeredClueId: "camera",
            considerationKind: "plan_endorsement",
            considerationId: "plan-ember",
            status: "open",
          }]
        : [],
    },
    offers: accessMode === "participant"
      ? [{
          offerId: "18:commit_move:0",
          actionType: "commit_move",
          schemaDigest: digest("4"),
          eligibility: "Until 2026-09-05T12:01:00Z",
        }]
      : [],
    ...overrides,
  };
}

function liveSnapshot(overrides: Partial<HostedLiveSessionSnapshot> = {}): HostedLiveSessionSnapshot {
  return {
    status: "live",
    synchronized: true,
    canAct: true,
    deliveryBatch: null,
    lastAcknowledgedFrameSeq: 41,
    actionReceipt: null,
    message: null,
    ...overrides,
  };
}

function fixture(accessMode: "participant" | "spectator" = "participant") {
  let live: AgentHeistLiveState = ready(accessMode);
  let session = liveSnapshot({ canAct: accessMode === "participant" });
  let tokenSequence = 0;
  const submitAction = vi.fn().mockResolvedValue({
    state: "accepted",
    actionId: ACTION_ID,
    duplicate: false,
    roomHead: {
      room_seq: 19,
      genesis_or_transition_hash: digest("5"),
      core_schema_version: "worldstream.core-room-state.v1",
      pack_digest: digest("1"),
      core_state_hash: digest("6"),
      activity_state_hash: digest("7"),
      authoritative_state_hash: digest("8"),
    },
  });
  const controller = {
    get state() { return session; },
    submitAction,
    waitFor: vi.fn((
      _predicate: (snapshot: HostedLiveSessionSnapshot) => boolean,
      options?: { signal?: AbortSignal },
    ) => new Promise<HostedLiveSessionSnapshot>((_resolve, reject) => {
      const abort = () => {
        const error = new Error("cancelled");
        error.name = "AbortError";
        reject(error);
      };
      if (options?.signal?.aborted === true) abort();
      else options?.signal?.addEventListener("abort", abort, { once: true });
    })),
  } as unknown as Pick<
    HostedLiveSessionController,
    "state" | "waitFor" | "submitAction"
  >;
  const bridge = new AgentHeistWebMcpBridge({
    controller,
    readLiveState: () => live,
    tokenFactory: (prefix) => `${prefix}_${String(++tokenSequence).padStart(20, "0")}`,
    actionIdFactory: () => ACTION_ID,
  });
  return {
    bridge,
    controller,
    submitAction,
    setLive: (value: AgentHeistLiveState) => { live = value; },
    setSession: (value: HostedLiveSessionSnapshot) => { session = value; },
  };
}

describe("AgentHeistWebMcpBridge", () => {
  it("exposes three participant tools but only read and wait to a spectator", () => {
    expect(fixture().bridge.tools("participant").map((tool) => tool.name)).toEqual([
      "heist_read_state",
      "heist_wait_for_update",
      "heist_commit_plan",
    ]);
    expect(fixture("spectator").bridge.tools("spectator").map((tool) => tool.name)).toEqual([
      "heist_read_state",
      "heist_wait_for_update",
    ]);
  });

  it("returns bounded authorized participant content and preserves exchange consideration", async () => {
    const { bridge } = fixture();
    const output = await bridge.read({});

    expect(JSON.stringify(output).length).toBeLessThanOrEqual(1_500);
    expect(output).toMatchObject({
      ok: true,
      untrusted_content: true,
      access_mode: "participant",
      role: "navigator",
      incoming_exchanges: [{
        from: "broker",
        consideration: { kind: "plan_endorsement", id: "plan-ember" },
      }],
    });
  });

  it("never exposes participant-private content or a mutation to spectators", async () => {
    const { bridge } = fixture("spectator");
    const output = await bridge.read({});
    const serialized = JSON.stringify(output);

    expect(serialized).not.toContain("private_clues");
    expect(serialized).not.toContain("incoming_exchanges");
    expect(serialized).not.toContain("eleven_second_pause");
    expect(output.available_moves).toEqual([]);
  });

  it("binds a stable internal Action ID and refuses to rebind a stale token", async () => {
    const setup = fixture();
    const first = await setup.bridge.read({});
    const moves = first.available_moves as Array<Record<string, unknown>>;
    const actionToken = String(moves[0]?.action_token);
    const input = {
      action_token: actionToken,
      plan_id: "plan-ember",
      contribute_required_resource: true,
    };

    await expect(setup.bridge.commit(input)).resolves.toMatchObject({
      ok: true,
      status: "accepted",
      room_sequence: 19,
    });
    await expect(setup.bridge.commit(input)).resolves.toMatchObject({ ok: true });
    expect(setup.submitAction).toHaveBeenCalledTimes(1);
    expect(setup.submitAction).toHaveBeenCalledWith(expect.objectContaining({
      actionId: ACTION_ID,
      basedOnRoomSeq: 18,
      actionType: "commit_move",
    }));

    setup.setLive(ready("participant", {
      roomSequence: 19,
      offers: [{
        offerId: "19:commit_move:0",
        actionType: "commit_move",
        schemaDigest: digest("4"),
        eligibility: "Until 2026-09-05T12:01:00Z",
      }],
    }));
    await expect(setup.bridge.commit(input)).resolves.toMatchObject({
      ok: false,
      code: "refresh_required",
    });
    expect(setup.submitAction).toHaveBeenCalledTimes(1);
  });

  it("rejects malformed or unknown fields before any Action", async () => {
    const { bridge, submitAction } = fixture();
    await expect(bridge.read({ extra: true })).resolves.toMatchObject({ code: "invalid_input" });
    await expect(bridge.commit({ action_token: "bad" })).resolves.toMatchObject({ code: "invalid_input" });
    expect(submitAction).not.toHaveBeenCalled();
  });

  it("honors cancellation on a bounded wait", async () => {
    const { bridge } = fixture();
    const current = await bridge.read({});
    const abort = new AbortController();
    const waiting = bridge.wait(
      { after_state_token: current.state_token },
      { signal: abort.signal },
    );
    abort.abort();
    await expect(waiting).rejects.toMatchObject({ name: "AbortError" });
  });

  it("falls back to a compact projection before exceeding the output ceiling", async () => {
    const setup = fixture();
    const large = "x".repeat(2_000);
    setup.setLive(ready("participant", {
      projection: {
        ...ready().projection,
        plans: Array.from({ length: 8 }, (_, index) => ({
          ...ready().projection.plans[0]!,
          planId: `plan-${index}-${large}`,
          route: large,
        })),
      },
    }));
    const output = await setup.bridge.read({});
    expect(JSON.stringify(output).length).toBeLessThanOrEqual(1_500);
    expect(output).toMatchObject({ ok: true, truncated: true });
  });
});

describe("registerAgentHeistWebMcp", () => {
  it("does nothing when the browser does not implement WebMCP", async () => {
    const registration = registerAgentHeistWebMcp({}, fixture().bridge, "participant");
    expect(registration.supported).toBe(false);
    await expect(registration.ready).resolves.toBe(false);
    registration.dispose();
  });

  it("registers exact tools and aborts all registrations on disposal", async () => {
    const registered: AgentHeistWebMcpTool[] = [];
    const signals: AbortSignal[] = [];
    const registration = registerAgentHeistWebMcp(
      {
        modelContext: {
          registerTool: (tool, options) => {
            registered.push(tool);
            signals.push(options.signal);
          },
        },
      },
      fixture().bridge,
      "participant",
    );
    await expect(registration.ready).resolves.toBe(true);
    expect(registered.map((tool) => tool.name)).toEqual([
      "heist_read_state",
      "heist_wait_for_update",
      "heist_commit_plan",
    ]);
    expect(registered.every((tool) => tool.inputSchema.additionalProperties === false)).toBe(true);
    registration.dispose();
    expect(signals.every((signal) => signal.aborted)).toBe(true);
  });
});
