import { describe, expect, it } from "vitest";

import {
  TEN_TURN_TECHNICAL_ROUTE,
  actionCost,
  actionPayload,
  candidateConfidence,
  moveOptions,
  readMidnightArchiveProjection,
} from "./model";
import { projection, rawProjection } from "./testFixtures";

describe("Midnight Archive strict participant Projection", () => {
  it("accepts the exact five-location, finite-resource contract", () => {
    const parsed = readMidnightArchiveProjection(rawProjection());
    expect(parsed).not.toBeNull();
    if (parsed === null) throw new Error("expected parsed Projection");
    expect(parsed.map.locations.map((location) => location.id).sort()).toEqual([
      "atrium", "conservation", "plant", "records", "vault",
    ]);
    expect(parsed.turnsUsed + parsed.turnsRemaining).toBe(16);
  });

  it("fails closed for drift, malformed resources, hidden truth, bad costs, and dangling references", () => {
    const base = rawProjection();
    const candidates = base.candidates as Array<Record<string, unknown>>;
    const firstCandidate = candidates[0] as Record<string, unknown>;
    const invalid = [
      { ...base, extra: true },
      { ...base, turns_remaining: 14 },
      { ...base, power: 4 },
      { ...base, outcome: { kind: "partial" }, phase: "complete" },
      { ...base, candidates: [{ ...firstCandidate, authentic: true }, ...candidates.slice(1)] },
      { ...base, carried_candidate: "missing-ledger" },
      { ...base, verifier_result: { candidate_id: "ledger-amber", confidence: "likely" } },
      { ...base, staged_action: { action_type: "stage_use_verifier", turn_cost: 1, power_cost: 0 } },
      { ...base, staged_action: { action_type: "stage_move", destination: "vault", turn_cost: 1, power_cost: 0 } },
    ];
    for (const value of invalid) expect(readMidnightArchiveProjection(value)).toBeNull();
  });

  it("accepts only the four exact factual outcomes", () => {
    for (const kind of ["success", "wrong_ledger", "no_ledger", "exhausted_inside"]) {
      const remaining = kind === "exhausted_inside" ? 0 : 6;
      expect(readMidnightArchiveProjection(rawProjection({
        phase: "complete",
        turns_used: 16 - remaining,
        turns_remaining: remaining,
        outcome: { kind },
      }))).not.toBeNull();
    }
    expect(readMidnightArchiveProjection(rawProjection({
      phase: "complete",
      outcome: { kind: "success", score: 100 },
    }))).toBeNull();
  });
});

describe("technical route semantics", () => {
  it("models the documented ten-turn route using all three power charges", () => {
    expect(TEN_TURN_TECHNICAL_ROUTE).toHaveLength(10);
    const totals = TEN_TURN_TECHNICAL_ROUTE.reduce((sum, intent) => {
      const cost = actionCost(intent.action);
      return { turns: sum.turns + cost.turns, power: sum.power + cost.power };
    }, { turns: 0, power: 0 });
    expect(totals).toEqual({ turns: 10, power: 3 });
    expect(TEN_TURN_TECHNICAL_ROUTE.map((intent) => intent.action)).toEqual([
      "stage_move",
      "stage_use_verifier",
      "stage_move",
      "stage_open_service_hatch",
      "stage_move",
      "stage_recover_candidate",
      "stage_move",
      "stage_move",
      "stage_move",
      "stage_extract",
    ]);
  });

  it("builds exact Pack payloads without mutating projected resources", () => {
    const current = projection();
    const before = JSON.stringify(current);
    expect(actionPayload({ action: "stage_use_verifier" })).toEqual({});
    expect(actionPayload({ action: "stage_move", destination: "plant" })).toEqual({
      destination: "plant",
    });
    expect(actionPayload({ action: "stage_recover_candidate", candidate_id: "ledger-amber" })).toEqual({
      candidate_id: "ledger-amber",
    });
    expect(moveOptions(current).map((option) => option.destination).sort()).toEqual([
      "atrium", "conservation", "plant",
    ]);
    expect(JSON.stringify(current)).toBe(before);
    expect(current.power).toBe(3);
    expect(current.turnsRemaining).toBe(15);
  });

  it("derives confidence solely from the authorized verifier result", () => {
    const current = projection({
      verifier_result: { candidate_id: "ledger-amber", confidence: "verified" },
      carried_candidate: "ledger-cobalt",
    });
    expect(candidateConfidence(current, "ledger-amber")).toBe("verified");
    expect(candidateConfidence(current, "ledger-cobalt")).toBe("unverified");
  });
});
