import type { CanonicalObject } from "@worldstream/pack-sdk";

function debriefValues(): unknown[] {
  const evidence = [
    { evidence_status: "none", message: "No authored source was inspected." },
    {
      evidence_status: "partial",
      message: "Only one authored source was inspected; it did not uniquely identify a candidate.",
    },
    {
      evidence_status: "complete",
      message: "Both authored sources were inspected and their intersection informed the recommendation.",
    },
  ] as const;
  const commitments = ["not_accepted", "accepted", "honored"] as const;
  const values: unknown[] = [null];
  for (const evidenceDebrief of evidence) {
    for (const agreementCommitment of commitments) {
      for (const collectionPreserved of [false, true]) {
        for (const sourceRecordProtected of [false, true]) {
          values.push({
            ...evidenceDebrief,
            agreement_commitment: agreementCommitment,
            optional_objectives: {
              collection_preserved: collectionPreserved,
              source_record_protected: sourceRecordProtected,
            },
          });
        }
      }
    }
  }
  return values;
}

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
      kind: { enum: [
        "none", "move", "inspect_records", "inspect_conservation", "use_verifier",
        "accept_preservation_agreement", "prepare_collection", "energize_preservation_equipment",
        "open_service_hatch", "recover_candidate", "protect_source_record", "extract", "wait",
      ] },
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
      { action_type: "stage_inspect_records", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_inspect_conservation", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_use_verifier", turn_cost: 1, power_cost: 1 },
      { action_type: "stage_accept_preservation_agreement", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_prepare_collection", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_energize_preservation_equipment", turn_cost: 1, power_cost: 1 },
      { action_type: "stage_open_service_hatch", turn_cost: 1, power_cost: 2 },
      { action_type: "stage_recover_candidate", candidate_id: "ledger-amber", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_recover_candidate", candidate_id: "ledger-cobalt", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_recover_candidate", candidate_id: "ledger-violet", turn_cost: 1, power_cost: 0 },
      { action_type: "stage_protect_source_record", turn_cost: 1, power_cost: 1 },
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
      { kind: "partial_extraction" },
      { kind: "wrong_ledger" },
      { kind: "no_ledger" },
      { kind: "exhausted_inside" },
    ],
  };
  const debriefProjection = {
    enum: debriefValues(),
  };
  const agreementCondition = {
    additionalProperties: false,
    properties: {
      condition_id: { enum: ["lead_acceptance", "collection_preparation", "equipment_energized"] },
      label: { maxLength: 96, type: "string" },
      power_cost: { maximum: 1, minimum: 0, type: "integer" },
      status: { enum: ["blocked", "pending", "complete"] },
      turn_cost: { const: 1 },
    },
    required: ["condition_id", "label", "status", "turn_cost", "power_cost"],
    type: "object",
  };
  const preservationAgreementProjection = {
    additionalProperties: false,
    properties: {
      commitment: { enum: ["not_accepted", "accepted", "honored"] },
      conditions: { items: agreementCondition, maxItems: 3, minItems: 3, type: "array" },
      speaker: { const: "Archivist" },
      statement: { const: "Preserve the threatened collection and I will open the Conservation–Vault gate." },
    },
    required: ["speaker", "statement", "commitment", "conditions"],
    type: "object",
  };
  const optionalObjectivesProjection = {
    additionalProperties: false,
    properties: {
      collection_preserved: {
        additionalProperties: false,
        properties: {
          label: { const: "Preserve the threatened collection" },
          power_cost: { const: 1 },
          status: { enum: ["not_started", "prepared", "complete"] },
          turn_cost: { const: 2 },
        },
        required: ["label", "status", "turn_cost", "power_cost"],
        type: "object",
      },
      source_record_protected: {
        additionalProperties: false,
        properties: {
          label: { const: "Protect the source's identifying record" },
          power_cost: { const: 1 },
          status: { enum: ["locked", "available", "complete"] },
          turn_cost: { const: 1 },
        },
        required: ["label", "status", "turn_cost", "power_cost"],
        type: "object",
      },
    },
    required: ["collection_preserved", "source_record_protected"],
    type: "object",
  };
  const outcome = {
    additionalProperties: false,
    properties: {
      extracted_candidate_id: candidateOrNone,
      factual_reason: { maxLength: 256, type: "string" },
      kind: { enum: ["pending", "success", "partial_extraction", "wrong_ledger", "no_ledger", "exhausted_inside"] },
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
  const evidence = {
    additionalProperties: false,
    properties: {
      conservation: { enum: ["unknown", "observed"] },
      records: { enum: ["unknown", "observed"] },
    },
    required: ["records", "conservation"],
    type: "object",
  };
  const observedEvidence = {
    additionalProperties: false,
    properties: {
      attribute_label: { enum: ["Binding", "Marking", "Year"] },
      candidate_value: { maxLength: 64, type: "string" },
      observed_value: { maxLength: 64, type: "string" },
      relation: { enum: ["matches", "does_not_match"] },
      source_id: { enum: ["records", "conservation"] },
      source_label: { maxLength: 64, type: "string" },
    },
    required: ["source_id", "source_label", "attribute_label", "observed_value", "candidate_value", "relation"],
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
  const miraPlanStep = {
    additionalProperties: false,
    properties: {
      destination: locationOrNone,
      power_cost: { enum: [0, 1, 2] },
      source_id: { enum: ["none", "records", "conservation"] },
      step_type: { enum: ["move", "inspect_source", "share_source", "use_verifier", "open_service_hatch", "collect_assay_sample", "complete_field_assay"] },
    },
    required: ["step_type", "destination", "source_id", "power_cost"],
    type: "object",
  };
  const miraTask = {
    additionalProperties: false,
    properties: {
      kind: { enum: ["none", "investigate_records", "investigate_conservation", "open_service_hatch", "field_assay"] },
      power_allowance: { enum: [0, 1, 2] },
      power_spent: { enum: [0, 1, 2] },
      revision: { maximum: 65_535, minimum: 0, type: "integer" },
      status: { enum: ["none", "assigned", "complete", "cancelled"] },
    },
    required: ["status", "revision", "kind", "power_allowance", "power_spent"],
    type: "object",
  };
  const contributionKind = { enum: [
    "none", "move", "inspect_source", "share_source", "use_verifier", "open_service_hatch", "collect_assay_sample", "complete_field_assay", "follow_move", "regroup_move",
  ] };
  const miraKnowledge = {
    additionalProperties: false,
    properties: {
      conservation: { enum: ["unknown", "private", "shared"] },
      records: { enum: ["unknown", "private", "shared"] },
      verifier_result: candidateOrNone,
    },
    required: ["records", "conservation", "verifier_result"],
    type: "object",
  };
  const miraState = {
    additionalProperties: false,
    properties: {
      knowledge: miraKnowledge,
      field_assay: { additionalProperties: false, type: "object", properties: { steps_completed: { type: "integer", minimum: 0, maximum: 2 }, result: candidateOrNone }, required: ["steps_completed", "result"] },
      last_contribution: {
        additionalProperties: false,
        properties: {
          kind: contributionKind,
          summary: { maxLength: 96, type: "string" },
          turn: { maximum: 16, minimum: 0, type: "integer" },
        },
        required: ["turn", "kind", "summary"],
        type: "object",
      },
      location: locationOrNone,
      member_id: { maxLength: 64, minLength: 1, type: "string" },
      mode: { enum: ["unavailable", "following", "holding", "tasked", "regrouping"] },
      opportunity: {
        additionalProperties: false,
        properties: {
          deadline: { maxLength: 64, minLength: 4, type: "string" },
          opened_at: { maxLength: 64, minLength: 4, type: "string" },
          revision: { maximum: 65_535, minimum: 0, type: "integer" },
          status: { enum: ["none", "open", "expired", "fulfilled"] },
          task_revision: { maximum: 65_535, minimum: 0, type: "integer" },
        },
        required: ["status", "revision", "task_revision", "opened_at", "deadline"],
        type: "object",
      },
      plan: {
        additionalProperties: false,
        properties: {
          next_step_index: { maximum: 3, minimum: 0, type: "integer" },
          origin_member_id: { maxLength: 64, minLength: 1, type: "string" },
          origin_opportunity_revision: { maximum: 65_535, minimum: 0, type: "integer" },
          origin_task_revision: { maximum: 65_535, minimum: 0, type: "integer" },
          revision: { maximum: 65_535, minimum: 0, type: "integer" },
          status: { enum: ["none", "active", "complete"] },
          steps: { items: miraPlanStep, maxItems: 3, minItems: 0, type: "array" },
        },
        required: [
          "status", "revision", "origin_member_id", "origin_task_revision",
          "origin_opportunity_revision", "steps", "next_step_index",
        ],
        type: "object",
      },
      preparation: {
        additionalProperties: false,
        properties: {
          for_turn: { maximum: 16, minimum: 0, type: "integer" },
          plan_revision: { maximum: 65_535, minimum: 0, type: "integer" },
          status: { enum: ["none", "prepared", "deferred"] },
          step_index: { maximum: 3, minimum: 0, type: "integer" },
          summary: { maxLength: 64, minLength: 4, type: "string" },
          task_revision: { maximum: 65_535, minimum: 0, type: "integer" },
        },
        required: ["status", "for_turn", "task_revision", "plan_revision", "step_index", "summary"],
        type: "object",
      },
      presence: { enum: ["absent", "active", "suspended"] },
      task: miraTask,
    },
    required: [
      "presence", "member_id", "location", "mode", "task", "opportunity", "plan",
      "preparation", "knowledge", "last_contribution", "field_assay",
    ],
    type: "object",
  };
  const miraProjection = {
    additionalProperties: false,
    properties: {
      knowledge: {
        additionalProperties: false,
        properties: {
          conservation: { enum: ["unknown", "private", "shared"] },
          records: { enum: ["unknown", "private", "shared"] },
          verifier_result: verifierResultProjection,
        },
        required: ["records", "conservation", "verifier_result"],
        type: "object",
      },
      last_contribution: miraState.properties.last_contribution,
      field_assay: { additionalProperties: false, type: "object", properties: { steps_completed: { type: "integer", minimum: 0, maximum: 2 }, result: verifierResultProjection }, required: ["steps_completed", "result"] },
      location: locationOrNone,
      mode: miraState.properties.mode,
      planning: {
        additionalProperties: false,
        properties: {
          deadline: { maxLength: 64, minLength: 4, type: "string" },
          opportunity_revision: { maximum: 65_535, minimum: 0, type: "integer" },
          plan_revision: { maximum: 65_535, minimum: 0, type: "integer" },
          status: { enum: ["not_requested", "waiting", "ready", "expired", "complete"] },
          steps_completed: { maximum: 3, minimum: 0, type: "integer" },
          steps_total: { maximum: 3, minimum: 0, type: "integer" },
        },
        required: ["status", "opportunity_revision", "plan_revision", "steps_total", "steps_completed", "deadline"],
        type: "object",
      },
      preparation: {
        additionalProperties: false,
        properties: {
          for_turn: { maximum: 16, minimum: 0, type: "integer" },
          status: { enum: ["none", "prepared", "deferred"] },
          summary: { maxLength: 64, minLength: 4, type: "string" },
        },
        required: ["status", "for_turn", "summary"],
        type: "object",
      },
      presence: miraState.properties.presence,
      task: miraTask,
    },
    required: ["presence", "location", "mode", "task", "planning", "preparation", "knowledge", "last_contribution", "field_assay"],
    type: "object",
  };
  const role = { enum: ["lead", "mira", "jonah"] };
  const companionRole = { enum: ["mira", "jonah"] };
  const roles = arrayOf(role, 3);
  const companions = arrayOf(companionRole, 2);
  const revision = { type: "integer", minimum: 0, maximum: 65535 };
  const completedWork = arrayOf(closed({ role: companionRole, kind: contributionKind, turn: { type: "integer", minimum: 1, maximum: 16 } }), 32);
  const extraction = closed({ status: { enum: ["none", "prepared", "acknowledged"] }, revision,
    for_turn: { type: "integer", minimum: 0, maximum: 16 }, extracted_roles: roles, left_behind_roles: companions });
  const extractionState = closed({ ...extraction.properties,
    contribution_fingerprint: { type: "string", minLength: 4, maxLength: 71 } });
  const turnResolution = closed({ status: { enum: ["clear", "conflict"] },
    power_reserved: { type: "integer", minimum: 0, maximum: 5 },
    prepared_roles: companions, deferred_roles: companions, unprepared_roles: companions,
    reservations: arrayOf(closed({ role, power: { type: "integer", minimum: 0, maximum: 2 },
      interaction: { enum: ["none", "service_hatch", "catalog_verifier"] } }), 3),
    conflicts: arrayOf(closed({ code: { enum: ["shared_power", "service_hatch", "catalog_verifier", "ineligible_contribution"] }, roles }), 5) });
  const crewDebrief = closed({ starting_roles: roles, extracted_roles: roles, left_behind_roles: roles, completed_work: completedWork });
  const startingCrew = arrayOf(closed({ role, member_id: { type: "string", minLength: 1, maxLength: 64 } }), 3);
  return {
    extraction, extractionState, turnResolution, crewDebrief, startingCrew, completedWork,
    candidate,
    candidateOrNull,
    debriefProjection,
    evidence,
    gates,
    location,
    mapEdge,
    mapLocation,
    miraProjection,
    miraState,
    outcome,
    outcomeProjection,
    observedEvidence,
    optionalObjectivesProjection,
    preservationAgreementProjection,
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
      evidence: part.evidence,
      collection_preservation: { enum: ["unprepared", "prepared", "preserved"] },
      gates: part.gates,
      location: part.location,
      mira: part.miraState,
      jonah: part.miraState,
      starting_crew: part.startingCrew,
      extraction: part.extractionState,
      completed_crew_work: part.completedWork,
      objective: { maxLength: 256, minLength: 1, type: "string" },
      outcome: part.outcome,
      phase: { enum: ["briefing", "active", "complete"] },
      power_remaining: { maximum: 3, minimum: 0, type: "integer" },
      preservation_agreement: { enum: ["offered", "accepted"] },
      role_notes: part.roleNotes,
      scenario_id: { const: "standard-v1" },
      staged_action: part.stagedAction,
      source_record_protected: { type: "boolean" },
      turn_limit: { const: 16 },
      turns_used: { maximum: 16, minimum: 0, type: "integer" },
      verifier_result: { enum: ["none", "ledger-amber", "ledger-cobalt", "ledger-violet"] },
      truth_marker: part.candidate,
    },
    required: [
      "phase", "scenario_id", "objective", "location", "turn_limit", "turns_used",
      "power_remaining", "gates", "candidates", "evidence", "preservation_agreement",
      "collection_preservation", "source_record_protected", "mira", "jonah", "starting_crew", "extraction", "completed_crew_work", "truth_marker", "verifier_result",
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
            evidence_assessment: { enum: ["unknown", "observed", "recommended"] },
            observed_evidence: { items: part.observedEvidence, maxItems: 2, minItems: 0, type: "array" },
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
          required: ["candidate_id", "label", "visible_attributes", "evidence_assessment", "observed_evidence"],
          type: "object",
        },
        maxItems: 3,
        minItems: 3,
        type: "array",
      },
      debrief: part.debriefProjection,
      optional_objectives: part.optionalObjectivesProjection,
      preservation_agreement: part.preservationAgreementProjection,
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
      mira: part.miraProjection,
      jonah: part.miraProjection,
      turn_resolution: part.turnResolution,
      extraction: part.extraction,
      crew_debrief: part.crewDebrief,
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
      "gates", "map", "candidates", "staged_action", "carried_candidate", "debrief",
      "preservation_agreement", "optional_objectives", "mira", "jonah", "turn_resolution", "extraction", "crew_debrief",
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

function closed(properties: Record<string, unknown>) {
  return { additionalProperties: false, type: "object", properties, required: Object.keys(properties) };
}

function arrayOf(items: unknown, maximum: number) {
  return { type: "array", items, minItems: 0, maxItems: maximum };
}
