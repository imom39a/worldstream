import { describe, expect, it } from "vitest";

import type { AuthorizedRoomDeliveryBatch } from "@worldstream/client";

import {
  NEGOTIATE_REVISION_0_1,
  NEGOTIATE_REVISION_0_2,
  initialNegotiateLiveState,
  reduceNegotiateObservation,
} from "./liveAdapter";

const digest = (character: string) => `blake3:${character.repeat(64)}`;

function batch({
  accessMode = "participant",
  persona = "buyer_agent",
  projection = {},
  offers = [{ action_type: "submit_proposal", payload_schema_digest: digest("a") }],
  packId = "worldstream.negotiate",
  packVersion = "0.1.0",
  packDigest = NEGOTIATE_REVISION_0_1,
  headDigest = packDigest,
  roomSequence = 12,
}: {
  accessMode?: "participant" | "spectator";
  persona?: string;
  projection?: Record<string, unknown>;
  offers?: unknown[];
  packId?: string;
  packVersion?: string;
  packDigest?: string;
  headDigest?: string;
  roomSequence?: number;
} = {}): AuthorizedRoomDeliveryBatch {
  return {
    pack: { id: packId, version: packVersion, digest: packDigest },
    room_head: {
      room_seq: roomSequence,
      genesis_or_transition_hash: digest("1"),
      core_schema_version: "worldstream.core-room-state.v1",
      pack_digest: headDigest,
      core_state_hash: digest("2"),
      activity_state_hash: digest("3"),
      authoritative_state_hash: digest("4"),
    },
    frame_head: 14,
    delivery: [{
      kind: "projection_reset",
      body: {
        projection: {
          core: {
            access_mode: accessMode,
            standing: "enabled",
            role: accessMode === "participant" ? persona : null,
            viewer_class: accessMode === "participant" ? "participant" : "public",
          },
          activity: {
            persona,
            phase: "formation_open",
            aggregate_state: "open",
            private_quote: persona === "spectator" ? undefined : "authorized-only",
            ...projection,
          },
          action_offers: accessMode === "spectator" ? [] : offers,
        },
      },
    }],
  };
}

describe("Negotiate retained live adapter", () => {
  it("starts empty and installs only an exact authorized Reset", () => {
    const empty = initialNegotiateLiveState();
    expect(empty).toEqual({ kind: "awaiting" });
    expect(JSON.stringify(empty)).not.toContain("authorized-only");

    const state = reduceNegotiateObservation(empty, batch());
    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.authorization).toEqual({ accessMode: "participant", role: "buyer_agent" });
    expect(state.session.projection.private_quote).toBe("authorized-only");
    expect(state.offerBindings).toEqual([{
      offerId: "12:submit_proposal:0",
      actionType: "submit_proposal",
      schemaDigest: digest("a"),
    }]);
  });

  it("supports a public spectator Reset without participant Actions", () => {
    const state = reduceNegotiateObservation(
      initialNegotiateLiveState(),
      batch({ accessMode: "spectator", persona: "spectator", offers: [] }),
    );

    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.authorization).toEqual({ accessMode: "spectator", role: null });
    expect(state.session.persona).toBe("spectator");
    expect(state.session.action_offers).toEqual([]);
    expect(state.offerBindings).toEqual([]);
  });

  it("accepts the corrected immutable 0.2.0 Pack Revision", () => {
    const state = reduceNegotiateObservation(
      initialNegotiateLiveState(),
      batch({ packVersion: "0.2.0", packDigest: NEGOTIATE_REVISION_0_2 }),
    );

    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.pack).toEqual({
      id: "worldstream.negotiate",
      version: "0.2.0",
      digest: NEGOTIATE_REVISION_0_2,
    });
  });

  it("replaces the complete authorized activity view on Reset", () => {
    const first = reduceNegotiateObservation(initialNegotiateLiveState(), batch());
    const second = reduceNegotiateObservation(first, batch({
      roomSequence: 13,
      projection: { private_quote: null },
      offers: [],
    }));

    expect(second.kind).toBe("ready");
    if (second.kind !== "ready") throw new Error("expected ready state");
    expect(second.session.projection.private_quote).toBeNull();
    expect(second.session.action_offers).toEqual([]);
    expect(JSON.stringify(second)).not.toContain("authorized-only");
  });

  it("installs the Pack's real projection-replaced Observation wrapper", () => {
    const reset = reduceNegotiateObservation(initialNegotiateLiveState(), batch());
    const deltaBase = batch({ roomSequence: 13 });
    const delta: AuthorizedRoomDeliveryBatch = {
      ...deltaBase,
      delivery: [{
        kind: "observation",
        body: {
          observation: {
            change_type: "projection_replaced",
            projection: {
              persona: "buyer_agent",
              phase: "approval_pending",
              aggregate_state: "open",
              private_quote: "replacement-only",
            },
            action_offers: [{
              action_type: "approve_proposal",
              payload_schema_digest: digest("b"),
            }],
          },
        },
      }],
    };

    const state = reduceNegotiateObservation(reset, delta);

    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.session.projection.private_quote).toBe("replacement-only");
    expect(state.session.action_offers).toEqual([{
      action_type: "approve_proposal",
      payload_schema_digest: digest("b"),
    }]);
  });

  it("fails closed for the wrong Pack, revision, head, or access/persona pair", () => {
    const candidates = [
      batch({ packId: "worldstream.agent-heist" }),
      batch({ packDigest: digest("e"), headDigest: digest("e") }),
      batch({ headDigest: digest("f") }),
      batch({ accessMode: "spectator", persona: "buyer_agent" }),
    ];
    for (const candidate of candidates) {
      expect(reduceNegotiateObservation(initialNegotiateLiveState(), candidate).kind)
        .toBe("incompatible");
    }
  });
});
