import { describe, expect, it } from "vitest";

import type { AuthorizedRoomDeliveryBatch } from "@worldstream/client";

import {
  AGENT_HEIST_REVISION_0_1,
  AGENT_HEIST_REVISION_0_2,
  initialAgentHeistLiveState,
  reduceAgentHeistObservation,
} from "./liveAdapter";

const hash = (character: string) => `blake3:${character.repeat(64)}`;

function activity(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    phase: "negotiation",
    phase_generation: 2,
    phase_start: "2026-08-15T12:00:30Z",
    phase_deadline: "2026-08-15T12:02:00Z",
    seats: [
      { role: "navigator", present: true },
      { role: "insider", present: true },
      { role: "broker", present: false },
    ],
    public_claims: [{ clue_id: "route", claim_code: "route_service" }],
    plans: [{
      plan_id: "plan-a",
      proposer_role: "navigator",
      created_room_seq: 4,
      route: "service",
      entry_window: "early",
      required_tool: "thermal_key",
      extraction: "boat",
    }],
    endorsements: { navigator: "plan-a" },
    challenges: [],
    commitment_count: 0,
    outcome: null,
    private_clues: [{
      clue_id: "route",
      known: true,
      owner_role: "navigator",
      claim_code: "route_service",
    }],
    own_commitment: null,
    addressed_offers: [],
    ...overrides,
  };
}

function observation({
  digest = AGENT_HEIST_REVISION_0_1,
  version = digest === AGENT_HEIST_REVISION_0_2 ? "0.2.0" : "0.1.0",
  roomDigest = digest,
  roomSequence = 7,
  delivery,
}: {
  digest?: string;
  version?: string;
  roomDigest?: string;
  roomSequence?: number;
  delivery?: AuthorizedRoomDeliveryBatch["delivery"];
} = {}): AuthorizedRoomDeliveryBatch {
  return {
    pack: {
      id: "worldstream.agent-heist",
      version,
      digest,
    },
    room_head: {
      room_seq: roomSequence,
      genesis_or_transition_hash: hash("1"),
      core_schema_version: "worldstream.core-room-state.v1",
      pack_digest: roomDigest,
      core_state_hash: hash("2"),
      activity_state_hash: hash("3"),
      authoritative_state_hash: hash("4"),
    },
    frame_head: 9,
    delivery: delivery ?? [{
      kind: "projection_reset",
      body: {
        projection: {
          core: {
            access_mode: "participant",
            standing: "enabled",
            role: "navigator",
            room_status: "active",
            viewer_class: "participant",
          },
          activity: activity(),
          action_offers: [{
            action_type: "propose_plan",
            payload_schema_digest: hash("a"),
          }],
        },
      },
    }],
  };
}

describe("Agent Heist retained live adapter", () => {
  it("accepts only the exact schema-safe revision/version pair", () => {
    const digest = "blake3:56449d0830d1137d69b1b7c11ed25e8f0d9b7188d40e8290c58e5a2caff2bef9";
    expect(reduceAgentHeistObservation(initialAgentHeistLiveState(), observation({ digest, version: "0.5.0" })).kind).toBe("ready");
    expect(reduceAgentHeistObservation(initialAgentHeistLiveState(), observation({ digest, version: "0.4.0" })).kind).toBe("incompatible");
  });
  it("accepts only the exact agent-ready revision/version pair", () => {
    const digest = "blake3:4455e4302bda695a5fc4aca150b5a8dac944775474930dafaef1539acb86c96e";
    expect(reduceAgentHeistObservation(initialAgentHeistLiveState(), observation({ digest, version: "0.4.0" })).kind).toBe("ready");
    expect(reduceAgentHeistObservation(initialAgentHeistLiveState(), observation({ digest, version: "0.3.0" })).kind).toBe("incompatible");
  });
  it("accepts the exact clock-safe revision and rejects a mislabeled version", () => {
    const digest = "blake3:4e4c970403f29a8448a1a3bcf7a96c030df713499730288f324c7e200d160b2d";
    const ready = reduceAgentHeistObservation(initialAgentHeistLiveState(), observation({ digest, version: "0.3.0" }));
    expect(ready.kind).toBe("ready");
    const rejected = reduceAgentHeistObservation(initialAgentHeistLiveState(), observation({ digest, version: "0.2.0" }));
    expect(rejected.kind).toBe("incompatible");
  });

  it("starts without any fixture or activity data", () => {
    const state = initialAgentHeistLiveState();

    expect(state).toEqual({ kind: "awaiting" });
    expect(JSON.stringify(state)).not.toMatch(/Canal Shift|route_service|fixture/i);
  });

  it("installs an authorized Reset from empty state with exact offers", () => {
    const state = reduceAgentHeistObservation(initialAgentHeistLiveState(), observation());

    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.authorization).toEqual({ accessMode: "participant", role: "navigator" });
    expect(state.projection.privateClues).toEqual([{
      clueId: "route",
      ownerRole: "navigator",
      claimCode: "route_service",
    }]);
    expect(state.offers).toEqual([expect.objectContaining({
      offerId: "7:propose_plan:0",
      actionType: "propose_plan",
      schemaDigest: hash("a"),
    })]);
  });

  it("preserves the consideration on an incoming exchange", () => {
    const state = reduceAgentHeistObservation(
      initialAgentHeistLiveState(),
      observation({
        delivery: [{
          kind: "projection_reset",
          body: {
            projection: {
              core: {
                access_mode: "participant",
                standing: "enabled",
                role: "navigator",
                room_status: "active",
                viewer_class: "participant",
              },
              activity: activity({
                addressed_offers: [{
                  offer_id: "exchange-1",
                  sender_role: "broker",
                  recipient_role: "navigator",
                  offered_clue_id: "route",
                  consideration_kind: "plan_endorsement",
                  consideration_id: "plan-a",
                  status: "open",
                }],
              }),
              action_offers: [],
            },
          },
        }],
      }),
    );

    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.projection.addressedOffers).toEqual([{
      offerId: "exchange-1",
      senderRole: "broker",
      offeredClueId: "route",
      considerationKind: "plan_endorsement",
      considerationId: "plan-a",
      status: "open",
    }]);
  });

  it("replaces every activity field on Reset instead of retaining a prior private value", () => {
    const first = reduceAgentHeistObservation(initialAgentHeistLiveState(), observation());
    const replacement = observation({
      roomSequence: 8,
      delivery: [{
        kind: "projection_reset",
        body: {
          projection: {
            core: {
              access_mode: "participant",
              standing: "enabled",
              role: "navigator",
              room_status: "active",
              viewer_class: "participant",
            },
            activity: activity({ private_clues: [], public_claims: [], plans: [] }),
            action_offers: [],
          },
        },
      }],
    });
    const second = reduceAgentHeistObservation(first, replacement);

    expect(second.kind).toBe("ready");
    if (second.kind !== "ready") throw new Error("expected ready state");
    expect(second.projection.privateClues).toEqual([]);
    expect(second.projection.publicClaims).toEqual([]);
    expect(second.projection.plans).toEqual([]);
    expect(second.offers).toEqual([]);
    expect(JSON.stringify(second)).not.toContain("route_service");
  });

  it("installs a read-only spectator Reset without participant-private state or Actions", () => {
    const state = reduceAgentHeistObservation(initialAgentHeistLiveState(), observation({
      delivery: [{
        kind: "projection_reset",
        body: {
          projection: {
            core: {
              access_mode: "spectator",
              standing: "enabled",
              role: null,
              room_status: "active",
              viewer_class: "public",
            },
            activity: activity({ private_clues: [], own_commitment: null, addressed_offers: [] }),
            action_offers: [],
          },
        },
      }],
    }));

    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.authorization).toEqual({ accessMode: "spectator", role: null });
    expect(state.offers).toEqual([]);
    expect(state.projection.privateClues).toEqual([]);
  });

  it("uses a full Observation replacement while preserving only unchanged offers", () => {
    const first = reduceAgentHeistObservation(initialAgentHeistLiveState(), observation());
    const next = observation({
      digest: AGENT_HEIST_REVISION_0_2,
      roomSequence: 10,
      delivery: [{
        kind: "observation",
        body: {
          observation: activity({
            phase: "commitment",
            phase_generation: 3,
            public_claims: [],
            plans: [],
            private_clues: [],
            commitment_count: 1,
          }),
        },
      }],
    });
    const compatibleFirst = reduceAgentHeistObservation(initialAgentHeistLiveState(), {
      ...observation(),
      pack: next.pack,
      room_head: { ...observation().room_head, pack_digest: next.pack.digest },
    });
    const state = reduceAgentHeistObservation(compatibleFirst, next);

    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.projection.phase).toBe("commitment");
    expect(state.projection.privateClues).toEqual([]);
    expect(state.projection.publicClaims).toEqual([]);
    expect(state.offers.map((offer) => offer.actionType)).toEqual(["propose_plan"]);
  });

  it("accepts the exact revision 0.2 Lobby with no participant Action Offer", () => {
    const state = reduceAgentHeistObservation(initialAgentHeistLiveState(), observation({
      digest: AGENT_HEIST_REVISION_0_2,
      delivery: [{
        kind: "projection_reset",
        body: {
          projection_schema: "agent-heist.participant.v1",
          projection: {
            core: {
              access_mode: "participant",
              standing: "enabled",
              role: "navigator",
              room_status: "active",
              viewer_class: "participant",
            },
            activity: activity({
              phase: "lobby",
              phase_generation: 0,
              phase_deadline: null,
              public_claims: [],
              plans: [],
              private_clues: [],
            }),
            action_offers: [],
          },
        },
      }],
    }));

    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.projection.phase).toBe("lobby");
    expect(state.offers).toEqual([]);
  });

  it("preserves the installed Projection across an empty delivery and advances its exact Head", () => {
    const first = reduceAgentHeistObservation(initialAgentHeistLiveState(), observation());
    const state = reduceAgentHeistObservation(first, observation({ roomSequence: 11, delivery: [] }));

    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.roomSequence).toBe(11);
    expect(state.projection.privateClues[0]?.claimCode).toBe("route_service");
    expect(state.offers[0]?.offerId).toBe("11:propose_plan:0");
  });

  it("fails closed for a wrong Pack, mismatched Room Head, unsupported revision, or non-exact participant Reset", () => {
    const wrongPack = { ...observation(), pack: { ...observation().pack, id: "worldstream.counter" } };
    const wrongHead = observation({ roomDigest: hash("f") });
    const unsupported = observation({ digest: hash("e") });
    const mismatchedVersion = observation({
      digest: AGENT_HEIST_REVISION_0_2,
      version: "0.1.0",
    });
    const nestedLegacyCore = observation({
      delivery: [{
        kind: "projection_reset",
        body: {
          projection: {
            core: {
              room_status: "active",
              membership: { status: "enabled", access_mode: "participant", role: "navigator" },
            },
            activity: activity(),
            action_offers: [],
          },
        },
      }],
    });

    for (const candidate of [wrongPack, wrongHead, unsupported, mismatchedVersion, nestedLegacyCore]) {
      const state = reduceAgentHeistObservation(initialAgentHeistLiveState(), candidate);
      expect(state.kind).toBe("incompatible");
      if (state.kind === "incompatible") expect(state.reason.length).toBeGreaterThan(0);
    }
  });
});
