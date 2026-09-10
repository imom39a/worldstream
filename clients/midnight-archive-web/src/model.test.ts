import { describe, expect, it } from "vitest";

import {
  actionCost,
  actionPayload,
  candidateConfidence,
  moveOptions,
  readMidnightArchiveProjection,
} from "./model";
import {
  ELEVEN_TURN_POWERED_AGREEMENT_ROUTE,
  FIFTEEN_TURN_BOTH_OPTIONALS_ROUTE,
  TEN_TURN_TECHNICAL_ROUTE,
  projection,
  rawJonah,
  rawMira,
  rawProjection,
} from "./testFixtures";

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

  it("decodes the v2 specialist, turn-resolution, extraction, and crew-debrief contract", () => {
    const parsed = readMidnightArchiveProjection(rawProjection({
      jonah: rawJonah({ presence: "active", location: "plant", mode: "following" }),
      crew_debrief: {
        starting_roles: ["lead", "jonah"],
        extracted_roles: [],
        left_behind_roles: [],
        completed_work: [],
      },
    }));
    expect(parsed?.jonah.location).toBe("plant");
    expect(parsed?.jonah.fieldAssay).toEqual({ stepsCompleted: 0, result: null });
    expect(parsed?.turnResolution.status).toBe("clear");
    expect(parsed?.extraction.status).toBe("none");
    expect(parsed?.crewDebrief.startingRoles).toEqual(["lead", "jonah"]);
  });

  it("fails closed on specialist privacy, conflicting reservations, and stale extraction acknowledgement", () => {
    const invalid = [
      rawProjection({ jonah: rawJonah({ private_plan: [{ step_type: "move" }] }) }),
      rawProjection({
        turn_resolution: {
          status: "clear", power_reserved: 4,
          prepared_roles: ["mira", "jonah"], deferred_roles: [], unprepared_roles: [],
          reservations: [
            { role: "mira", power: 2, interaction: "service_hatch" },
            { role: "jonah", power: 1, interaction: "service_hatch" },
          ],
          conflicts: [],
        },
      }),
      rawProjection({
        staged_action: { action_type: "stage_extract", turn_cost: 1, power_cost: 0 },
        location: "atrium",
        extraction: {
          status: "acknowledged", revision: 4, for_turn: 1,
          extracted_roles: ["lead"], left_behind_roles: ["mira"],
        },
        crew_debrief: {
          starting_roles: ["lead"], extracted_roles: [], left_behind_roles: [], completed_work: [],
        },
      }),
    ];
    for (const value of invalid) expect(readMidnightArchiveProjection(value)).toBeNull();
  });

  it("accepts an all-three acknowledged extraction only in canonical role order", () => {
    const allThree = rawProjection({
      location: "atrium",
      staged_action: { action_type: "stage_extract", turn_cost: 1, power_cost: 0 },
      mira: rawMira({ presence: "active", location: "atrium", mode: "following" }),
      jonah: rawJonah({ presence: "active", location: "atrium", mode: "following" }),
      extraction: {
        status: "acknowledged", revision: 3, for_turn: 2,
        extracted_roles: ["lead", "mira", "jonah"], left_behind_roles: [],
      },
      crew_debrief: {
        starting_roles: ["lead", "mira", "jonah"], extracted_roles: [], left_behind_roles: [], completed_work: [],
      },
    });
    expect(readMidnightArchiveProjection(allThree)?.extraction.extractedRoles).toEqual(["lead", "mira", "jonah"]);
    expect(readMidnightArchiveProjection({
      ...allThree,
      extraction: { ...(allThree.extraction as Record<string, unknown>), extracted_roles: ["lead", "jonah", "mira"] },
    })).toBeNull();
  });

  it("rejects a non-positive or conflicted extraction preview", () => {
    const staged = {
      location: "atrium",
      staged_action: { action_type: "stage_extract", turn_cost: 1, power_cost: 0 },
      extraction: { status: "prepared", revision: 0, for_turn: 2, extracted_roles: ["lead"], left_behind_roles: [] },
    };
    expect(readMidnightArchiveProjection(rawProjection(staged))).toBeNull();
    expect(readMidnightArchiveProjection(rawProjection({
      ...staged,
      extraction: { ...(staged.extraction as Record<string, unknown>), revision: 1 },
      turn_resolution: {
        status: "conflict", power_reserved: 0, prepared_roles: [], deferred_roles: [], unprepared_roles: [], reservations: [],
        conflicts: [{ code: "ineligible_contribution", roles: ["mira"] }],
      },
    }))).toBeNull();
  });

  it("rejects absent assay facts, stale terminal turns, unordered work, and invalid conflict provenance", () => {
    const terminal = rawProjection({ phase: "complete", turns_used: 8, turns_remaining: 8, outcome: { kind: "success" }, carried_candidate: "ledger-violet" });
    const crew = terminal.crew_debrief as Record<string, unknown>;
    const invalid = [
      rawProjection({ mira: rawMira({ field_assay: { steps_completed: 1, result: null } }) }),
      { ...terminal, extraction: { ...(terminal.extraction as Record<string, unknown>), for_turn: 7 } },
      rawProjection({
        mira: rawMira({ presence: "active", location: "atrium", mode: "following" }),
        jonah: rawJonah({ presence: "active", location: "atrium", mode: "following" }),
        crew_debrief: {
          starting_roles: ["lead", "mira", "jonah"], extracted_roles: [], left_behind_roles: [],
          completed_work: [
            { role: "mira", kind: "move", turn: 2 },
            { role: "jonah", kind: "move", turn: 1 },
          ],
        },
      }),
      rawProjection({
        turn_resolution: {
          status: "conflict", power_reserved: 0, prepared_roles: [], deferred_roles: [], unprepared_roles: [], reservations: [],
          conflicts: [
            { code: "ineligible_contribution", roles: ["mira"] },
            { code: "ineligible_contribution", roles: ["mira"] },
          ],
        },
      }),
      { ...terminal, crew_debrief: { ...crew, starting_roles: ["lead", "jonah", "mira"] } },
    ];
    for (const value of invalid) expect(readMidnightArchiveProjection(value)).toBeNull();
  });

  it("requires terminal outcomes to match ledger and starting-crew extraction facts", () => {
    const mira = rawMira({ presence: "suspended", location: "atrium", mode: "unavailable" });
    const partialCrew = {
      starting_roles: ["lead", "mira"], extracted_roles: ["lead"], left_behind_roles: ["mira"], completed_work: [],
    };
    const extraction = {
      status: "acknowledged", revision: 2, for_turn: 8, extracted_roles: ["lead"], left_behind_roles: ["mira"],
    };
    expect(readMidnightArchiveProjection(rawProjection({
      phase: "complete", turns_used: 8, turns_remaining: 8, outcome: { kind: "success" },
      carried_candidate: "ledger-violet", mira, extraction, crew_debrief: partialCrew,
    }))).toBeNull();
    expect(readMidnightArchiveProjection(rawProjection({
      phase: "complete", turns_used: 8, turns_remaining: 8, outcome: { kind: "no_ledger" },
      carried_candidate: "ledger-violet",
    }))).toBeNull();
  });

  it("accepts terminal retained tasked state with an intentionally empty turn resolution", () => {
    const mira = rawMira({
      presence: "active", location: "atrium", mode: "tasked",
      task: { status: "assigned", revision: 2, kind: "field_assay", power_allowance: 0, power_spent: 0 },
      planning: { status: "ready", opportunity_revision: 3, plan_revision: 3, steps_total: 2, steps_completed: 1, deadline: "none" },
    });
    expect(readMidnightArchiveProjection(rawProjection({
      phase: "complete", turns_used: 8, turns_remaining: 8, outcome: { kind: "success" },
      mira,
      turn_resolution: {
        status: "clear", power_reserved: 0, prepared_roles: [], deferred_roles: [], unprepared_roles: [], reservations: [], conflicts: [],
      },
      extraction: { status: "acknowledged", revision: 2, for_turn: 8, extracted_roles: ["lead", "mira"], left_behind_roles: [] },
      crew_debrief: { starting_roles: ["lead", "mira"], extracted_roles: ["lead", "mira"], left_behind_roles: [], completed_work: [] },
    }))).not.toBeNull();
  });

  it("decodes Mira's two-step Vault assay without revealing a result after the first step", () => {
    const mira = rawMira({
      presence: "active", location: "vault", mode: "tasked",
      task: { status: "assigned", revision: 2, kind: "field_assay", power_allowance: 0, power_spent: 0 },
      planning: { status: "ready", opportunity_revision: 3, plan_revision: 3, steps_total: 2, steps_completed: 1, deadline: "none" },
      field_assay: { steps_completed: 1, result: null },
      last_contribution: { turn: 1, kind: "collect_assay_sample", summary: "Mira collected a Vault assay sample; no result is available yet." },
    });
    const parsed = readMidnightArchiveProjection(rawProjection({
      mira,
      crew_debrief: {
        starting_roles: ["lead", "mira"], extracted_roles: [], left_behind_roles: [],
        completed_work: [{ role: "mira", kind: "collect_assay_sample", turn: 1 }],
      },
    }));
    expect(parsed?.mira.fieldAssay).toEqual({ stepsCompleted: 1, result: null });
    expect(readMidnightArchiveProjection(rawProjection({
      turns_used: 2,
      turns_remaining: 14,
      mira,
      crew_debrief: {
        starting_roles: ["lead", "mira"], extracted_roles: [], left_behind_roles: [],
        completed_work: [{ role: "mira", kind: "collect_assay_sample", turn: 1 }],
      },
    }))).toBeNull();
    expect(readMidnightArchiveProjection(rawProjection({
      mira: rawMira({ ...mira, field_assay: { steps_completed: 1, result: { candidate_id: "ledger-violet", confidence: "verified" } } }),
      crew_debrief: {
        starting_roles: ["lead", "mira"], extracted_roles: [], left_behind_roles: [],
        completed_work: [{ role: "mira", kind: "collect_assay_sample", turn: 1 }],
      },
    }))).toBeNull();
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

  it("decodes bounded Mira task, waiting, ready, and prepared progress without private plan steps", () => {
    const task = {
      status: "assigned",
      revision: 4,
      kind: "investigate_records",
      power_allowance: 1,
      power_spent: 0,
    };
    const waiting = readMidnightArchiveProjection(rawProjection({
      mira: rawMira({
        presence: "active",
        location: "atrium",
        mode: "tasked",
        task,
        planning: {
          status: "waiting",
          opportunity_revision: 6,
          plan_revision: 0,
          steps_total: 0,
          steps_completed: 0,
          deadline: "2026-09-09T12:34:56.789Z",
        },
      }),
    }));
    expect(waiting?.mira.planning.status).toBe("waiting");
    expect(waiting?.mira).not.toHaveProperty("steps");

    const ready = readMidnightArchiveProjection(rawProjection({
      mira: rawMira({
        presence: "active",
        location: "records",
        mode: "tasked",
        task,
        planning: {
          status: "ready",
          opportunity_revision: 6,
          plan_revision: 6,
          steps_total: 3,
          steps_completed: 1,
          deadline: "none",
        },
        preparation: {
          status: "prepared",
          for_turn: 2,
          summary: "Mira will inspect the assigned source.",
        },
      }),
    }));
    expect(ready?.mira.planning).toMatchObject({ stepsTotal: 3, stepsCompleted: 1 });
    expect(ready?.mira.preparation.summary).toBe("Mira will inspect the assigned source.");

    const exhausted = readMidnightArchiveProjection(rawProjection({
      turns_used: 1,
      turns_remaining: 15,
      mira: rawMira({
        presence: "active",
        location: "records",
        mode: "tasked",
        task,
        planning: {
          status: "complete",
          opportunity_revision: 6,
          plan_revision: 6,
          steps_total: 1,
          steps_completed: 1,
          deadline: "none",
        },
        last_contribution: {
          turn: 1,
          kind: "move",
          summary: "Mira moved one open passage.",
        },
      }),
    }));
    expect(exhausted?.mira.task.status).toBe("assigned");
    expect(exhausted?.mira.planning.status).toBe("complete");
  });

  it("accepts Core-canonical UTC planning deadlines without truncating fractional precision", () => {
    const task = {
      status: "assigned",
      revision: 4,
      kind: "investigate_records",
      power_allowance: 1,
      power_spent: 0,
    };
    const waiting = (deadline: string) => rawProjection({
      mira: rawMira({
        presence: "active",
        location: "atrium",
        mode: "tasked",
        task,
        planning: {
          status: "waiting",
          opportunity_revision: 6,
          plan_revision: 0,
          steps_total: 0,
          steps_completed: 0,
          deadline,
        },
      }),
    });
    for (const deadline of [
      "2026-09-09T12:34:56Z",
      "2026-09-09T12:34:56.1Z",
      "2026-09-09T12:34:56.12Z",
      "2026-09-09T12:34:56.123Z",
      "2026-09-09T12:34:56.123456789Z",
      "2024-02-29T12:34:56.123456789Z",
    ]) expect(readMidnightArchiveProjection(waiting(deadline))?.mira.planning.deadline).toBe(deadline);

    for (const deadline of [
      "2026-09-09T12:34:56.10Z",
      "0000-01-01T00:00:00Z",
      "2026-02-29T12:34:56Z",
      "2024-02-30T12:34:56Z",
      "2026-09-09T24:00:00Z",
      "2026-09-09T12:60:00Z",
      "2026-09-09T12:34:60Z",
      "2026-09-09T12:34:56+00:00",
      "2026-09-09T12:34:56.1234567891Z",
    ]) expect(readMidnightArchiveProjection(waiting(deadline))).toBeNull();
  });

  it("fails closed on private payloads, stale preparation, forged provenance, and impossible Mira spend", () => {
    const assigned = {
      status: "assigned",
      revision: 2,
      kind: "investigate_records",
      power_allowance: 1,
      power_spent: 0,
    };
    const planning = {
      status: "ready",
      opportunity_revision: 3,
      plan_revision: 3,
      steps_total: 2,
      steps_completed: 0,
      deadline: "none",
    };
    const invalid = [
      rawMira({ presence: "active", location: "records", mode: "tasked", task: assigned, planning: { ...planning, steps: [] } }),
      rawMira({
        presence: "active", location: "records", mode: "tasked", task: assigned, planning,
        preparation: { status: "prepared", for_turn: 1, summary: "Mira will inspect the assigned source." },
      }),
      rawMira({ presence: "active", location: "records", mode: "tasked", task: assigned, planning: { ...planning, plan_revision: 2 } }),
      rawMira({
        presence: "active", location: "records", mode: "tasked", task: assigned,
        planning: { ...planning, status: "waiting", plan_revision: 0, steps_total: 0, deadline: "2026-99-09T12:34:56.789Z" },
      }),
      rawMira({
        presence: "active", location: "records", mode: "tasked",
        task: { ...assigned, power_spent: 1 }, planning,
      }),
      rawMira({
        presence: "active", location: "records", mode: "tasked", task: assigned, planning,
        last_contribution: { turn: 1, kind: "inspect_source", summary: "Found calfskin." },
      }),
    ];
    for (const mira of invalid) {
      expect(readMidnightArchiveProjection(rawProjection({ mira }))).toBeNull();
    }
  });

  it("accepts only disclosed Mira evidence in the lead Projection", () => {
    const base = rawProjection();
    const candidates = (base.candidates as Array<Record<string, unknown>>).map((candidate) => ({
      ...candidate,
      evidence_assessment: "observed",
      observed_evidence: [{
        source_id: "records",
        source_label: "Records intake card",
        attribute_label: "Binding",
        observed_value: "calfskin",
        candidate_value: (candidate.visible_attributes as Array<Record<string, unknown>>)[0]?.value,
        relation: candidate.candidate_id === "ledger-cobalt" ? "does_not_match" : "matches",
      }],
    }));
    const privateState = readMidnightArchiveProjection(rawProjection({
      mira: rawMira({
        presence: "active", location: "records", mode: "tasked",
        task: { status: "assigned", revision: 1, kind: "investigate_records", power_allowance: 0, power_spent: 0 },
        knowledge: { records: "private", conservation: "unknown", verifier_result: null },
      }),
    }));
    expect(privateState?.mira.knowledge.records).toBe("private");
    expect(privateState?.candidates.every((candidate) => candidate.observedEvidence.length === 0)).toBe(true);
    expect(readMidnightArchiveProjection(rawProjection({
      candidates,
      mira: rawMira({
        presence: "active", location: "records", mode: "tasked",
        task: { status: "assigned", revision: 1, kind: "investigate_records", power_allowance: 0, power_spent: 0 },
        knowledge: { records: "shared", conservation: "unknown", verifier_result: null },
      }),
    }))).not.toBeNull();
  });
});

describe("technical route semantics", () => {
  it("models the documented ten-turn route using all three power charges", () => {
    expect(TEN_TURN_TECHNICAL_ROUTE).toHaveLength(10);
    const totals = TEN_TURN_TECHNICAL_ROUTE.reduce((sum, intent) => {
      const cost = actionCost(projection(), intent.action);
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
    expect(routeCost(projection(), ELEVEN_TURN_POWERED_AGREEMENT_ROUTE)).toEqual({ turns: 11, power: 2 });
    expect(ELEVEN_TURN_POWERED_AGREEMENT_ROUTE.map((intent) => intent.action)).toContain("stage_accept_preservation_agreement");
    expect(ELEVEN_TURN_POWERED_AGREEMENT_ROUTE.map((intent) => intent.action)).toContain("stage_energize_preservation_equipment");

    expect(routeCost(projection(), FIFTEEN_TURN_BOTH_OPTIONALS_ROUTE)).toEqual({ turns: 15, power: 3 });
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
    expect(actionPayload({
      action: "assign_mira_task",
      task_kind: "investigate_records",
      power_allowance: 1,
    })).toEqual({ task_kind: "investigate_records", power_allowance: 1 });
    expect(actionCost(current, "request_mira_plan")).toEqual({ turns: 0, power: 0 });
    expect(actionPayload({
      action: "assign_jonah_task",
      task_kind: "open_service_hatch",
      power_allowance: 1,
    })).toEqual({ task_kind: "open_service_hatch", power_allowance: 1 });
    expect(actionPayload({
      action: "acknowledge_extraction",
      preview_revision: 7,
      left_behind_roles: ["mira", "jonah"],
    })).toEqual({ preview_revision: 7, left_behind_roles: ["mira", "jonah"] });
    expect(() => actionPayload({
      action: "acknowledge_extraction",
      preview_revision: 7,
      left_behind_roles: ["jonah", "mira"],
    })).toThrow(/canonical role order/u);
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

function routeCost(
  current: Parameters<typeof actionCost>[0],
  route: readonly { readonly action: Parameters<typeof actionCost>[1] }[],
) {
  return route.reduce((sum, intent) => {
    const cost = actionCost(current, intent.action);
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
