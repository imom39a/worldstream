import { describe, expect, it } from "vitest";

import {
  ELEVEN_TURN_POWERED_AGREEMENT_ROUTE,
  FIFTEEN_TURN_BOTH_OPTIONALS_ROUTE,
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

  it("accepts only Pack-sourced observed evidence and a supported recommendation", () => {
    const base = rawProjection();
    const candidates = base.candidates as Array<Record<string, unknown>>;
    const observed = candidates.map((candidate) => ({
      ...candidate,
      evidence_assessment: candidate.candidate_id === "ledger-violet" ? "recommended" : "observed",
      observed_evidence: [
        {
          source_id: "records",
          source_label: "Records intake card",
          attribute_label: "Binding",
          observed_value: "calfskin",
          candidate_value: candidate.candidate_id === "ledger-cobalt" ? "linen" : "calfskin",
          relation: candidate.candidate_id === "ledger-cobalt" ? "does_not_match" : "matches",
        },
        {
          source_id: "conservation",
          source_label: "Conservation restoration note",
          attribute_label: "Marking",
          observed_value: "split_star",
          candidate_value: candidate.candidate_id === "ledger-amber" ? "compass_rose" : "split_star",
          relation: candidate.candidate_id === "ledger-amber" ? "does_not_match" : "matches",
        },
      ],
    }));
    expect(readMidnightArchiveProjection({ ...base, candidates: observed })).not.toBeNull();
    expect(readMidnightArchiveProjection({
      ...base,
      candidates: [{ ...observed[0], evidence_assessment: "recommended" }, ...observed.slice(1)],
    })).toBeNull();
    expect(readMidnightArchiveProjection({
      ...base,
      candidates: observed.map((candidate, index) => index === 0 ? {
        ...candidate,
        observed_evidence: [
          (candidate.observed_evidence as unknown[])[0],
          (candidate.observed_evidence as unknown[])[0],
        ],
      } : candidate),
    })).toBeNull();
    expect(readMidnightArchiveProjection({
      ...base,
      candidates: observed.map((candidate, index) => index === 0 ? {
        ...candidate,
        observed_evidence: (candidate.observed_evidence as Array<Record<string, unknown>>).map((evidence, evidenceIndex) => (
          evidenceIndex === 0 ? { ...evidence, relation: "does_not_match" } : evidence
        )),
      } : candidate),
    })).toBeNull();
    expect(readMidnightArchiveProjection({
      ...base,
      candidates: observed.map((candidate, index) => index === 1 ? {
        ...candidate,
        observed_evidence: (candidate.observed_evidence as Array<Record<string, unknown>>).map((evidence, evidenceIndex) => (
          evidenceIndex === 0 ? { ...evidence, observed_value: "linen", relation: "matches" } : evidence
        )),
      } : candidate),
    })).toBeNull();
    expect(readMidnightArchiveProjection({
      ...base,
      phase: "complete",
      candidates: observed,
      debrief: { evidence_status: "partial", message: "Incomplete evidence." },
      outcome: { kind: "success" },
    })).toBeNull();
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

  it("fails closed on agreement text, ordering, costs, status, and cross-field drift", () => {
    const base = rawProjection();
    const agreement = base.preservation_agreement as Record<string, unknown>;
    const conditions = agreement.conditions as Array<Record<string, unknown>>;
    const objectives = base.optional_objectives as Record<string, unknown>;
    const collection = objectives.collection_preserved as Record<string, unknown>;
    const source = objectives.source_record_protected as Record<string, unknown>;
    const terminal = rawProjection({
      phase: "complete",
      turns_used: 10,
      turns_remaining: 6,
      outcome: { kind: "no_ledger" },
    });
    const terminalDebrief = terminal.debrief as Record<string, unknown>;
    const invalid = [
      { ...base, preservation_agreement: { ...agreement, speaker: "Curator" } },
      { ...base, preservation_agreement: { ...agreement, statement: `${String(agreement.statement)} Negotiate freely.` } },
      { ...base, preservation_agreement: { ...agreement, conditions: [conditions[1], conditions[0], conditions[2]] } },
      { ...base, preservation_agreement: { ...agreement, conditions: [{ ...conditions[0], power_cost: 1 }, ...conditions.slice(1)] } },
      { ...base, preservation_agreement: { ...agreement, conditions: [{ ...conditions[0], status: "blocked" }, ...conditions.slice(1)] } },
      { ...base, gates: { archive_gate: "open", service_hatch: "closed" } },
      { ...base, optional_objectives: { ...objectives, collection_preserved: { ...collection, status: "complete" } } },
      { ...base, optional_objectives: { ...objectives, source_record_protected: { ...source, status: "complete" } } },
      { ...base, carried_candidate: "ledger-amber" },
      { ...base, staged_action: { action_type: "stage_energize_preservation_equipment", turn_cost: 1, power_cost: 1 }, location: "conservation" },
      { ...base, staged_action: { action_type: "stage_protect_source_record", turn_cost: 1, power_cost: 1 }, location: "plant" },
      { ...terminal, debrief: { ...terminalDebrief, message: "No evidence." } },
      { ...terminal, debrief: { ...terminalDebrief, agreement_commitment: "accepted" } },
      {
        ...terminal,
        debrief: {
          ...terminalDebrief,
          optional_objectives: { collection_preserved: true, source_record_protected: false },
        },
      },
    ];
    for (const value of invalid) expect(readMidnightArchiveProjection(value)).toBeNull();
  });

  it("accepts coherent accepted and honored agreement states", () => {
    const accepted = agreementState("accepted", ["complete", "complete", "pending"], "prepared");
    expect(readMidnightArchiveProjection(accepted)).not.toBeNull();
    const honored = agreementState("honored", ["complete", "complete", "complete"], "complete", "open");
    expect(readMidnightArchiveProjection(honored)).not.toBeNull();
    const base = rawProjection();
    const objectives = base.optional_objectives as Record<string, unknown>;
    const source = objectives.source_record_protected as Record<string, unknown>;
    expect(readMidnightArchiveProjection({
      ...base,
      carried_candidate: "ledger-amber",
      optional_objectives: {
        ...objectives,
        source_record_protected: { ...source, status: "available" },
      },
    })).not.toBeNull();
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

  it("models the eleven-turn powered agreement route and fifteen-turn route with both optionals", () => {
    expect(routeCost(ELEVEN_TURN_POWERED_AGREEMENT_ROUTE)).toEqual({ turns: 11, power: 2 });
    expect(ELEVEN_TURN_POWERED_AGREEMENT_ROUTE.map((intent) => intent.action)).toContain("stage_accept_preservation_agreement");
    expect(ELEVEN_TURN_POWERED_AGREEMENT_ROUTE.map((intent) => intent.action)).toContain("stage_energize_preservation_equipment");

    expect(routeCost(FIFTEEN_TURN_BOTH_OPTIONALS_ROUTE)).toEqual({ turns: 15, power: 3 });
    expect(FIFTEEN_TURN_BOTH_OPTIONALS_ROUTE.map((intent) => intent.action)).toContain("stage_protect_source_record");
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

function routeCost(route: readonly { readonly action: Parameters<typeof actionCost>[0] }[]) {
  return route.reduce((sum, intent) => {
    const cost = actionCost(intent.action);
    return { turns: sum.turns + cost.turns, power: sum.power + cost.power };
  }, { turns: 0, power: 0 });
}

function agreementState(
  commitment: "accepted" | "honored",
  statuses: readonly ["complete", "complete", "pending" | "complete"],
  collectionStatus: "prepared" | "complete",
  archiveGate: "closed" | "open" = "closed",
) {
  const base = rawProjection();
  const agreement = base.preservation_agreement as Record<string, unknown>;
  const conditions = agreement.conditions as Array<Record<string, unknown>>;
  const objectives = base.optional_objectives as Record<string, unknown>;
  const collection = objectives.collection_preserved as Record<string, unknown>;
  return {
    ...base,
    gates: { archive_gate: archiveGate, service_hatch: "closed" },
    preservation_agreement: {
      ...agreement,
      commitment,
      conditions: conditions.map((condition, index) => ({ ...condition, status: statuses[index] })),
    },
    optional_objectives: {
      ...objectives,
      collection_preserved: { ...collection, status: collectionStatus },
    },
  };
}
