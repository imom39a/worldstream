import type { CanonicalObject } from "@worldstream/pack-sdk";

function schemaParts() {
  const location = { enum: ["atrium", "records", "conservation", "plant", "vault"] };
  const locationOrNone = { enum: ["none", "atrium", "records", "conservation", "plant", "vault"] };
  const candidate = { enum: ["ledger-amber", "ledger-cobalt", "ledger-violet"] };
  const candidateOrNone = { enum: ["none", "ledger-amber", "ledger-cobalt", "ledger-violet"] };
  const candidateOrNull = { enum: [null, "ledger-amber", "ledger-cobalt", "ledger-violet"] };
  const stagedAction = {
    additionalProperties: false,
    properties: {
      candidate_id: candidateOrNone,
      destination: locationOrNone,
      kind: { enum: ["none", "move", "use_verifier", "open_service_hatch", "recover_candidate", "extract", "wait"] },
      power_cost: { maximum: 2, minimum: 0, type: "integer" },
      turn_cost: { maximum: 1, minimum: 0, type: "integer" },
    },
    required: ["kind", "destination", "candidate_id", "turn_cost", "power_cost"],
    type: "object",
  };
  const stagedActionProjection = {
    enum: [
      null,
      { action_type: "stage_move", destination: "atrium", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_move", destination: "records", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_move", destination: "conservation", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_move", destination: "plant", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_move", destination: "vault", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_use_verifier", turn_cost: 1, power_cost: 1 },
      { action_type: "stage_open_service_hatch", turn_cost: 1, power_cost: 2 },
      { action_type: "stage_recover_candidate", candidate_id: "ledger-amber", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_recover_candidate", candidate_id: "ledger-cobalt", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_recover_candidate", candidate_id: "ledger-violet", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_extract", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_wait", turn_cost: 1, power_cost: 0 },
    ],
  };
  const verifierResultProjection = {
    enum: [
      null,
      { candidate_id: "ledger-amber", confidence: "verified" },
      { candidate_id: "ledger-cobalt", confidence: "verified" },
      { candidate_id: "ledger-violet", confidence: "verified" },
    ],
  };
  const outcomeProjection = {
    enum: [
      null,
      { kind: "success" },
      { kind: "wrong_ledger" },
      { kind: "no_ledger" },
      { kind: "exhausted_inside" },
    ],
  };
  const outcome = {
    additionalProperties: false,
    properties: {
      extracted_candidate_id: candidateOrNone,
      factual_reason: { maxLength: 256, type: "string" },
      kind: { enum: ["pending", "success", "wrong_ledger", "no_ledger", "exhausted_inside"] },
    },
    required: ["kind", "factual_reason", "extracted_candidate_id"],
    type: "object",
  };
  const visibleCandidate = {
    additionalProperties: false,
    properties: {
      binding: { enum: ["calfskin", "linen"] },
      candidate_id: candidate,
      marking: { enum: ["compass_rose", "split_star"] },
      year: { enum: [1891, 1904] },
    },
    required: ["candidate_id", "binding", "marking", "year"],
    type: "object",
  };
  const gates = {
    additionalProperties: false,
    properties: {
      conservation_vault_open: { type: "boolean" },
      plant_vault_open: { type: "boolean" },
    },
    required: ["conservation_vault_open", "plant_vault_open"],
    type: "object",
  };
  const roleNotes = {
    additionalProperties: false,
    properties: {
      jonah: { maxLength: 64, type: "string" },
      lead: { maxLength: 64, type: "string" },
      mira: { maxLength: 64, type: "string" },
    },
    required: ["lead", "mira", "jonah"],
    type: "object",
  };
  const mapLocation = {
    additionalProperties: false,
    properties: {
      id: location,
      description: { maxLength: 256, type: "string" },
      name: { maxLength: 64, type: "string" },
    },
    required: ["id", "name", "description"],
    type: "object",
  };
  const mapEdge = {
    additionalProperties: false,
    properties: {
      from: location,
      gate: { enum: [null, "archive_gate", "service_hatch"] },
      to: location,
    },
    required: ["from", "to", "gate"],
    type: "object",
  };
  return {
    candidate,
    candidateOrNull,
    gates,
    location,
    mapEdge,
    mapLocation,
    outcome,
    outcomeProjection,
    roleNotes,
    stagedAction,
    stagedActionProjection,
    verifierResultProjection,
    visibleCandidate,
  };
}

export function stateSchema(): CanonicalObject {
  const part = schemaParts();
  return {
    additionalProperties: false,
    properties: {
      candidates: { items: part.visibleCandidate, maxItems: 3, minItems: 3, type: "array" },
      carried_candidate_id: { enum: ["none", "ledger-amber", "ledger-cobalt", "ledger-violet"] },
      carried_confidence: { enum: ["none", "unverified", "verified"] },
      gates: part.gates,
      location: part.location,
      objective: { maxLength: 256, minLength: 1, type: "string" },
      outcome: part.outcome,
      phase: { enum: ["briefing", "active", "complete"] },
      power_remaining: { maximum: 3, minimum: 0, type: "integer" },
      role_notes: part.roleNotes,
      scenario_id: { const: "standard-v1" },
      staged_action: part.stagedAction,
      turn_limit: { const: 16 },
      turns_used: { maximum: 16, minimum: 0, type: "integer" },
      verifier_result: { enum: ["none", "ledger-amber", "ledger-cobalt", "ledger-violet"] },
      truth_marker: part.candidate,
    },
    required: [
      "phase", "scenario_id", "objective", "location", "turn_limit", "turns_used",
      "power_remaining", "gates", "candidates", "truth_marker", "verifier_result",
      "carried_candidate_id", "carried_confidence", "staged_action", "outcome", "role_notes",
    ],
    type: "object",
  } as CanonicalObject;
}

export function participantProjectionSchema(): CanonicalObject {
  const part = schemaParts();
  return {
    additionalProperties: false,
    properties: {
      candidates: {
        items: {
          additionalProperties: false,
          properties: {
            candidate_id: part.candidate,
            label: { maxLength: 64, type: "string" },
            visible_attributes: {
              items: {
                additionalProperties: false,
                properties: {
                  label: { maxLength: 64, type: "string" },
                  value: { maxLength: 64, type: "string" },
                },
                required: ["label", "value"],
                type: "object",
              },
              maxItems: 3,
              minItems: 3,
              type: "array",
            },
          },
          required: ["candidate_id", "label", "visible_attributes"],
          type: "object",
        },
        maxItems: 3,
        minItems: 3,
        type: "array",
      },
      carried_candidate: part.candidateOrNull,
      gates: {
        additionalProperties: false,
        properties: {
          archive_gate: { enum: ["closed", "open"] },
          service_hatch: { enum: ["closed", "open"] },
        },
        required: ["archive_gate", "service_hatch"],
        type: "object",
      },
      location: part.location,
      map: {
        additionalProperties: false,
        properties: {
          connections: { items: part.mapEdge, maxItems: 6, minItems: 6, type: "array" },
          locations: { items: part.mapLocation, maxItems: 5, minItems: 5, type: "array" },
        },
        required: ["locations", "connections"],
        type: "object",
      },
      objective: { maxLength: 384, minLength: 1, type: "string" },
      outcome: part.outcomeProjection,
      phase: { enum: ["briefing", "active", "complete"] },
      power: { maximum: 3, minimum: 0, type: "integer" },
      staged_action: part.stagedActionProjection,
      turns_remaining: { maximum: 16, minimum: 0, type: "integer" },
      turns_used: { maximum: 16, minimum: 0, type: "integer" },
      verifier_result: part.verifierResultProjection,
    },
    required: [
      "phase", "objective", "location", "turns_used", "turns_remaining", "power",
      "gates", "map", "candidates", "staged_action", "carried_candidate",
      "verifier_result", "outcome",
    ],
    type: "object",
  } as CanonicalObject;
}

export function publicProjectionSchema(): CanonicalObject {
  const part = schemaParts();
  return {
    additionalProperties: false,
    properties: {
      location: part.location,
      outcome: part.outcomeProjection,
      phase: { enum: ["briefing", "active", "complete"] },
      turns_used: { maximum: 16, minimum: 0, type: "integer" },
    },
    required: ["phase", "location", "turns_used", "outcome"],
    type: "object",
  } as CanonicalObject;
}

export function emptyPayloadSchema(): CanonicalObject {
  return {
    additionalProperties: false,
    properties: {},
    required: [],
    type: "object",
  };
}
