import { describe, expect, it } from "vitest";

import {
  initialMidnightArchiveLiveState,
  prepareMidnightArchiveAction,
  reduceMidnightArchiveObservation,
} from "./liveAdapter";
import { authorizedBatch, rawProjection } from "./testFixtures";

describe("Midnight Archive authorized live adapter", () => {
  it("installs only the exact lead participant Reset and exact Action Offers", () => {
    const state = reduceMidnightArchiveObservation(
      initialMidnightArchiveLiveState(),
      authorizedBatch(),
    );
    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.authorization).toEqual({ accessMode: "participant", role: "lead" });
    expect(state.projection.location).toBe("records");
    expect(state.offers.map((offer) => offer.actionType)).toEqual([
      "stage_move", "stage_use_verifier", "stage_wait",
    ]);
  });

  it("keeps briefing valid and actionless until Activity Start", () => {
    const state = reduceMidnightArchiveObservation(initialMidnightArchiveLiveState(), authorizedBatch({
      projectionValue: rawProjection({
        phase: "briefing",
        location: "atrium",
        turns_used: 0,
        turns_remaining: 16,
      }),
      actions: [],
    }));
    expect(state.kind).toBe("ready");
    if (state.kind !== "ready") throw new Error("expected ready state");
    expect(state.projection.phase).toBe("briefing");
    expect(() => prepareMidnightArchiveAction(state, { action: "stage_wait" })).toThrow(/not accepting/u);
  });

  it("binds staging and commit to the current offer without local state mutation", () => {
    const initial = reduceMidnightArchiveObservation(initialMidnightArchiveLiveState(), authorizedBatch());
    if (initial.kind !== "ready") throw new Error("expected ready state");
    const before = JSON.stringify(initial.projection);
    const staged = prepareMidnightArchiveAction(
      initial,
      { action: "stage_use_verifier" },
      "01J00000000000000000000001",
    );
    expect(staged).toMatchObject({
      actionId: "01J00000000000000000000001",
      basedOnRoomSeq: 7,
      actionType: "stage_use_verifier",
      payload: {},
    });
    expect(JSON.stringify(initial.projection)).toBe(before);

    const stagedState = reduceMidnightArchiveObservation(initial, authorizedBatch({
      kind: "observation",
      roomSequence: 8,
      projectionValue: rawProjection({
        staged_action: { action_type: "stage_use_verifier", turn_cost: 1, power_cost: 1 },
      }),
      actions: ["stage_move", "stage_use_verifier", "stage_wait", "commit_turn"],
    }));
    expect(stagedState.kind).toBe("ready");
    if (stagedState.kind !== "ready") throw new Error("expected staged state");
    expect(stagedState.projection.power).toBe(3);
    expect(stagedState.projection.turnsRemaining).toBe(15);
    expect(prepareMidnightArchiveAction(
      stagedState,
      { action: "commit_turn" },
      "01J00000000000000000000002",
    )).toMatchObject({
      basedOnRoomSeq: 8,
      actionType: "commit_turn",
      payload: {},
    });
  });

  it("replaces a complete Projection and clears superseded private facts", () => {
    const first = reduceMidnightArchiveObservation(initialMidnightArchiveLiveState(), authorizedBatch({
      projectionValue: rawProjection({
        verifier_result: { candidate_id: "ledger-amber", confidence: "verified" },
      }),
    }));
    const second = reduceMidnightArchiveObservation(first, authorizedBatch({
      kind: "observation",
      roomSequence: 8,
      projectionValue: rawProjection({ verifier_result: null }),
    }));
    expect(second.kind).toBe("ready");
    if (second.kind !== "ready") throw new Error("expected ready state");
    expect(second.projection.verifierResult).toBeNull();
    expect(JSON.stringify(second)).not.toContain('"confidence":"verified"');
  });

  it("fails closed for wrong identity, wrong authorization, and malformed projections", () => {
    const wrongPack = authorizedBatch();
    wrongPack.pack.id = "worldstream.agent-heist";
    const wrongAuthorization = authorizedBatch();
    const delivery = wrongAuthorization.delivery[0];
    if (delivery?.kind === "projection_reset") {
      ((delivery.body.projection as Record<string, unknown>).core as Record<string, unknown>).role = "mira";
    }
    const malformed = authorizedBatch({ projectionValue: rawProjection({ power: -1 }) });
    for (const batch of [wrongPack, wrongAuthorization, malformed]) {
      expect(reduceMidnightArchiveObservation(initialMidnightArchiveLiveState(), batch).kind).toBe("incompatible");
    }
  });
});
