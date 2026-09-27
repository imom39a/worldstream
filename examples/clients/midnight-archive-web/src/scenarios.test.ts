import { describe, expect, it } from "vitest";

import { readMidnightArchiveProjection } from "./model";
import { rawProjection } from "./testFixtures";

const standard = {
  scenario: { id: "standard-v1", label: "Standard" },
  initial_power: 3,
  method_costs: { verifier: 1, ordinary_service_hatch: 2 },
};
const lowReserve = {
  ...standard,
  scenario: { id: "low-reserve-v1", label: "Low Reserve" },
  initial_power: 2,
  power: 2,
};

describe("authored scenario Projection", () => {
  it.each([standard, lowReserve])("reads the fixed authored identity and initial budget: $scenario.label", (scenario) => {
    const result = readMidnightArchiveProjection(rawProjection(scenario));
    expect(result).toMatchObject({
      scenario: scenario.scenario,
      initialPower: scenario.initial_power,
      methodCosts: { verifier: 1, ordinaryServiceHatch: 2 },
    });
    expect(result?.candidates.every((candidate) => candidate.observedEvidence.length === 0)).toBe(true);
    expect(result?.verifierResult).toBeNull();
  });

  it.each([0, 1, 2])("keeps Low Reserve's initial budget when current power is %i", (power) => {
    expect(readMidnightArchiveProjection(rawProjection({ ...lowReserve, power }))).toMatchObject({
      power, initialPower: 2,
    });
  });

  it("accepts changed visible candidate values without inferring evidence or authenticity from the scenario", () => {
    const raw = rawProjection(lowReserve);
    const candidates = raw.candidates as Array<Record<string, unknown>>;
    const firstAttributes = candidates[0]!.visible_attributes;
    candidates[0]!.visible_attributes = candidates[2]!.visible_attributes;
    candidates[2]!.visible_attributes = firstAttributes;
    const result = readMidnightArchiveProjection(raw);
    expect(result).not.toBeNull();
    expect(result?.candidates[0]?.visibleAttributes).not.toEqual(
      readMidnightArchiveProjection(rawProjection())?.candidates[0]?.visibleAttributes,
    );
    expect(result?.candidates.every((candidate) => (
      candidate.evidenceAssessment === "unknown" && candidate.observedEvidence.length === 0
    ))).toBe(true);
    expect(result?.verifierResult).toBeNull();
  });

  it.each([
    { scenario: { id: "unknown-v1", label: "Standard" } },
    { scenario: { id: "standard-v1", label: "Low Reserve" } },
    { scenario: { id: "low-reserve-v1", label: "Standard" } },
    { initial_power: 3 },
    { initial_power: 1 },
    { power: 3 },
    { power: -1 },
    { method_costs: { verifier: 0, ordinary_service_hatch: 2 } },
    { method_costs: { verifier: 1, ordinary_service_hatch: 1 } },
    { method_costs: { verifier: 1, ordinary_service_hatch: 2, free_access: true } },
    { scenario: { id: "low-reserve-v1", label: "Low Reserve", truth_marker: "ledger-amber" } },
  ])("rejects unknown, inconsistent, or private scenario fields: %j", (invalid) => {
    expect(readMidnightArchiveProjection(rawProjection({ ...lowReserve, ...invalid }))).toBeNull();
  });

  it.each(["scenario", "initial_power", "method_costs", "operation_costs"])("requires %s from the authorized Projection", (key) => {
    const raw = rawProjection(lowReserve);
    delete raw[key];
    expect(readMidnightArchiveProjection(raw)).toBeNull();
  });

  it("retains the complete authored lead and specialist operation costs", () => {
    const result = readMidnightArchiveProjection(rawProjection(lowReserve));
    expect(Object.keys(result!.operationCosts)).toHaveLength(19);
    expect(result?.operationCosts).toMatchObject({
      use_verifier: { turns: 1, power: 1 },
      open_service_hatch: { turns: 1, power: 2 },
      mira_field_assay: { turns: 0, power: 0 },
      companion_verifier: { turns: 0, power: 1 },
      mira_open_service_hatch: { turns: 0, power: 2 },
      jonah_open_service_hatch: { turns: 0, power: 1 },
    });
  });

  it.each([
    { mira_field_assay: { turn_cost: 0, power_cost: 1 } },
    { companion_verifier: { turn_cost: 0, power_cost: 2 } },
    { mira_open_service_hatch: { turn_cost: 0, power_cost: 1 } },
    { jonah_open_service_hatch: { turn_cost: 1, power_cost: 1 } },
    { use_verifier: { turn_cost: 1, power_cost: 1, truth_marker: "ledger-amber" } },
    { reroll: { turn_cost: 0, power_cost: 0 } },
    { move: undefined },
  ])("rejects missing, altered or extra operation costs: %j", (overrides) => {
    const raw = rawProjection(lowReserve);
    raw.operation_costs = { ...raw.operation_costs as Record<string, unknown>, ...overrides };
    expect(readMidnightArchiveProjection(raw)).toBeNull();
  });
});
