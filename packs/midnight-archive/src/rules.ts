import type { CanonicalJson, CanonicalObject } from "@worldstream/pack-sdk";

import {
  type ArchiveOutcome,
  type ArchiveOperationCosts,
  type ArchiveState,
  type CandidateId,
  type EvidenceSourceId,
  type Location,
  type StagedAction,
  type VisibleCandidate,
  asCanonical,
  cloneState,
  isCandidateId,
  isLocation,
  record,
  reject,
  stringValue,
} from "./model.js";
import {
  cancelMiraPlanningTimer,
  closeMiraAtTerminal,
  initialMiraState,
} from "./companions.js";
import { legalDestinationsFrom } from "./world.js";
import { acknowledgeExtraction, emptyExtraction, invalidateExtraction, prepareExtraction, prepareTurn, resolveCompanions, validateExtractionCommit } from "./turn-resolution.js";

export const OBJECTIVE =
  "Recover the authentic ledger, return to the Atrium, and extract before turn 16 ends.";

export interface AuthoredEvidenceSource {
  readonly source_id: EvidenceSourceId;
  readonly source_label: string;
  readonly attribute: "binding" | "marking" | "year";
  readonly value: string | number;
}

export interface AuthoredScenario {
  readonly scenario_id: "standard-v1" | "low-reserve-v1";
  readonly scenario_label: "Standard" | "Low Reserve";
  readonly initial_power: 2 | 3;
  readonly method_costs: { readonly verifier: 1; readonly ordinary_service_hatch: 2 };
  readonly operation_costs: ArchiveOperationCosts;
  readonly candidates: readonly VisibleCandidate[];
  readonly authentic_candidate_id: CandidateId;
  readonly evidence_sources: readonly AuthoredEvidenceSource[];
}

function closedOperationCosts(): ArchiveOperationCosts {
  return {
  move: { turn_cost: 1, power_cost: 0 },
  inspect_records: { turn_cost: 1, power_cost: 0 },
  inspect_conservation: { turn_cost: 1, power_cost: 0 },
  use_verifier: { turn_cost: 1, power_cost: 1 },
  accept_preservation_agreement: { turn_cost: 1, power_cost: 0 },
  prepare_collection: { turn_cost: 1, power_cost: 0 },
  energize_preservation_equipment: { turn_cost: 1, power_cost: 1 },
  open_service_hatch: { turn_cost: 1, power_cost: 2 },
  recover_candidate: { turn_cost: 1, power_cost: 0 },
  protect_source_record: { turn_cost: 1, power_cost: 1 },
  extract: { turn_cost: 1, power_cost: 0 },
  wait: { turn_cost: 1, power_cost: 0 },
  companion_move: { turn_cost: 0, power_cost: 0 },
  companion_inspect_source: { turn_cost: 0, power_cost: 0 },
  companion_share_source: { turn_cost: 0, power_cost: 0 },
  mira_field_assay: { turn_cost: 0, power_cost: 0 },
  companion_verifier: { turn_cost: 0, power_cost: 1 },
  mira_open_service_hatch: { turn_cost: 0, power_cost: 2 },
  jonah_open_service_hatch: { turn_cost: 0, power_cost: 1 },
  };
}

function standardScenario(): AuthoredScenario {
  return {
    scenario_id: "standard-v1",
    scenario_label: "Standard",
    initial_power: 3,
    method_costs: { verifier: 1, ordinary_service_hatch: 2 },
    operation_costs: closedOperationCosts(),
    candidates: [
      { candidate_id: "ledger-amber", binding: "calfskin", marking: "compass_rose", year: 1891 },
      { candidate_id: "ledger-cobalt", binding: "linen", marking: "split_star", year: 1891 },
      { candidate_id: "ledger-violet", binding: "calfskin", marking: "split_star", year: 1904 },
    ],
    authentic_candidate_id: "ledger-violet",
    evidence_sources: [
      { source_id: "records", source_label: "Records intake card", attribute: "binding", value: "calfskin" },
      { source_id: "conservation", source_label: "Conservation restoration note", attribute: "marking", value: "split_star" },
    ],
  };
}

function lowReserveScenario(): AuthoredScenario {
  return {
    scenario_id: "low-reserve-v1",
    scenario_label: "Low Reserve",
    initial_power: 2,
    method_costs: { verifier: 1, ordinary_service_hatch: 2 },
    operation_costs: closedOperationCosts(),
    candidates: [
      { candidate_id: "ledger-amber", binding: "calfskin", marking: "split_star", year: 1904 },
      { candidate_id: "ledger-cobalt", binding: "calfskin", marking: "compass_rose", year: 1891 },
      { candidate_id: "ledger-violet", binding: "linen", marking: "compass_rose", year: 1904 },
    ],
    authentic_candidate_id: "ledger-cobalt",
    evidence_sources: [
      { source_id: "records", source_label: "Records intake card", attribute: "binding", value: "calfskin" },
      { source_id: "conservation", source_label: "Conservation restoration note", attribute: "marking", value: "compass_rose" },
    ],
  };
}

function resolveScenario(id: string): AuthoredScenario {
  if (id === "standard-v1") return standardScenario();
  if (id === "low-reserve-v1") return lowReserveScenario();
  throw new TypeError("configuration must select a closed authored scenario");
}

export function validateAuthoredScenario(scenario: AuthoredScenario): void {
  const expectedScenario = scenario.scenario_id === "standard-v1"
    ? {
      label: "Standard", initialPower: 3, authenticCandidate: "ledger-violet",
      candidates: {
        "ledger-amber": { binding: "calfskin", marking: "compass_rose", year: 1891 },
        "ledger-cobalt": { binding: "linen", marking: "split_star", year: 1891 },
        "ledger-violet": { binding: "calfskin", marking: "split_star", year: 1904 },
      },
      conservationMarking: "split_star",
    }
    : scenario.scenario_id === "low-reserve-v1"
      ? {
        label: "Low Reserve", initialPower: 2, authenticCandidate: "ledger-cobalt",
        candidates: {
          "ledger-amber": { binding: "calfskin", marking: "split_star", year: 1904 },
          "ledger-cobalt": { binding: "calfskin", marking: "compass_rose", year: 1891 },
          "ledger-violet": { binding: "linen", marking: "compass_rose", year: 1904 },
        },
        conservationMarking: "compass_rose",
      }
      : null;
  const ids = new Set(scenario.candidates.map((candidate) => candidate.candidate_id));
  const expectedIds = ["ledger-amber", "ledger-cobalt", "ledger-violet"] as const;
  if (
    !expectedScenario ||
    scenario.scenario_label !== expectedScenario.label ||
    scenario.initial_power !== expectedScenario.initialPower ||
    scenario.authentic_candidate_id !== expectedScenario.authenticCandidate ||
    scenario.method_costs.verifier !== scenario.operation_costs.use_verifier.power_cost ||
    scenario.method_costs.ordinary_service_hatch !== scenario.operation_costs.open_service_hatch.power_cost ||
    !hasExactKeys(scenario, ["scenario_id", "scenario_label", "initial_power", "method_costs", "operation_costs", "candidates", "authentic_candidate_id", "evidence_sources"]) ||
    !hasExactKeys(scenario.method_costs, ["verifier", "ordinary_service_hatch"]) ||
    !hasClosedOperationCosts(scenario.operation_costs) ||
    scenario.candidates.length !== expectedIds.length ||
    ids.size !== scenario.candidates.length ||
    expectedIds.some((candidateId) => !ids.has(candidateId)) ||
    scenario.candidates.some((candidate) =>
      !hasExactKeys(candidate, ["candidate_id", "binding", "marking", "year"]) ||
      !["calfskin", "linen"].includes(candidate.binding) ||
      !["compass_rose", "split_star"].includes(candidate.marking) ||
      ![1891, 1904].includes(candidate.year) ||
      candidate.binding !== expectedScenario.candidates[candidate.candidate_id].binding ||
      candidate.marking !== expectedScenario.candidates[candidate.candidate_id].marking ||
      candidate.year !== expectedScenario.candidates[candidate.candidate_id].year,
    )
  ) {
    throw new TypeError("an authored scenario requires exactly three distinct candidates");
  }
  if (!ids.has(scenario.authentic_candidate_id)) {
    throw new TypeError("an authored scenario requires exactly one authentic candidate");
  }
  const sourceIds = new Set(scenario.evidence_sources.map((source) => source.source_id));
  if (
    scenario.evidence_sources.length !== 2 ||
    sourceIds.size !== 2 ||
    !sourceIds.has("records") ||
    !sourceIds.has("conservation") ||
    scenario.evidence_sources.some((source) =>
      !hasExactKeys(source, ["source_id", "source_label", "attribute", "value"]) ||
      (source.source_id === "records" &&
        (source.source_label !== "Records intake card" || source.attribute !== "binding" || source.value !== "calfskin")) ||
      (source.source_id === "conservation" &&
        (source.source_label !== "Conservation restoration note" || source.attribute !== "marking" || source.value !== expectedScenario.conservationMarking)),
    )
  ) {
    throw new TypeError("an authored scenario requires Records and Conservation sources");
  }
  const matches = scenario.evidence_sources.map((source) => {
    const compatible = scenario.candidates.filter((candidate) => candidate[source.attribute] === source.value);
    if (compatible.length <= 1) {
      throw new TypeError(`${source.source_id} evidence must leave more than one candidate possible`);
    }
    return compatible;
  });
  const intersection = matches[0]!.filter((candidate) => matches[1]!.some((other) => other.candidate_id === candidate.candidate_id));
  if (intersection.length !== 1 || intersection[0]!.candidate_id !== scenario.authentic_candidate_id) {
    throw new TypeError("authored evidence must have one authentic candidate at its unique intersection");
  }
}

function hasExactKeys(value: object, expected: readonly string[]): boolean {
  const actual = Object.keys(value).sort();
  return actual.length === expected.length && actual.every((key, index) => key === [...expected].sort()[index]);
}

function hasClosedOperationCosts(costs: ArchiveOperationCosts): boolean {
  const expectedCosts = closedOperationCosts();
  if (!hasExactKeys(costs, Object.keys(expectedCosts))) return false;
  return (Object.keys(expectedCosts) as Array<keyof ArchiveOperationCosts>).every((operation) => {
    const expected = expectedCosts[operation];
    const actual = costs[operation];
    return hasExactKeys(actual, ["turn_cost", "power_cost"]) &&
      actual.turn_cost === expected.turn_cost && actual.power_cost === expected.power_cost;
  });
}

export function authoredEvidenceSources(state: ArchiveState): readonly AuthoredEvidenceSource[] {
  return state.evidence_records;
}

function emptyStage(): StagedAction {
  return {
    kind: "none",
    destination: "none",
    candidate_id: "none",
    turn_cost: 0,
    power_cost: 0,
  };
}

function pendingOutcome(): ArchiveOutcome {
  return {
    kind: "pending",
    factual_reason: "The expedition is still inside the archive.",
    extracted_candidate_id: "none",
  };
}

export function initializeArchiveState(configuration: CanonicalObject): ArchiveState {
  if (
    Object.keys(configuration).length !== 1 ||
    typeof configuration.scenario_id !== "string"
  ) {
    throw new TypeError("configuration must select one closed authored scenario");
  }
  const scenario = resolveScenario(configuration.scenario_id);
  validateAuthoredScenario(scenario);
  return {
    phase: "briefing",
    scenario_id: scenario.scenario_id,
    scenario_label: scenario.scenario_label,
    initial_power: scenario.initial_power,
    evidence_records: scenario.evidence_sources,
    method_costs: scenario.method_costs,
    operation_costs: scenario.operation_costs,
    objective: OBJECTIVE,
    location: "atrium",
    turn_limit: 16,
    turns_used: 0,
    power_remaining: scenario.initial_power,
    gates: {
      conservation_vault_open: false,
      plant_vault_open: false,
    },
    candidates: scenario.candidates,
    evidence: { records: "unknown", conservation: "unknown" },
    preservation_agreement: "offered",
    collection_preservation: "unprepared",
    source_record_protected: false,
    mira: initialMiraState(null),
    jonah: initialMiraState(null, "jonah"),
    companion_dialogue: [],
    starting_crew: [{ role: "lead", member_id: "none" }],
    extraction: emptyExtraction(),
    completed_crew_work: [],
    truth_marker: scenario.authentic_candidate_id,
    verifier_result: "none",
    carried_candidate_id: "none",
    carried_confidence: "none",
    staged_action: emptyStage(),
    outcome: pendingOutcome(),
    role_notes: {
      lead: "You decide which ledger to recover and when to extract.",
      mira: "Mira wants every claim grounded in visible evidence.",
      jonah: "Jonah wants the source record protected.",
    },
  };
}

export function startArchive(state: ArchiveState): ArchiveState {
  if (state.phase !== "briefing") reject("inactive", "the archive briefing is already closed");
  return { ...cloneState(state), phase: "active" };
}

export interface AppliedLeadAction {
  readonly state: ArchiveState;
  readonly event: CanonicalObject;
  readonly timerRequests: readonly CanonicalJson[];
}

export function applyLeadAction(
  current: ArchiveState,
  actionType: string,
  payload: CanonicalObject,
  core?: CanonicalObject,
  scheduled: CanonicalObject = {},
): AppliedLeadAction {
  if (current.phase !== "active") reject("inactive", "the expedition is not active");
  if (actionType === "commit_turn") return commitTurn(current, payload, core, scheduled);
  if (actionType === "prepare_extraction" || actionType === "acknowledge_extraction") {
    if (actionType === "prepare_extraction") exactKeys(payload, []);
    const state = actionType === "prepare_extraction" ? prepareExtraction(current, core) : acknowledgeExtraction(current, payload, core);
    return { state, event: asCanonical({ event_type: "extraction_updated", status: state.extraction.status, preview_revision: state.extraction.revision }), timerRequests: [] };
  }
  const staged = stagedAction(current, actionType, payload);
  const state = invalidateExtraction({ ...cloneState(current), staged_action: staged });
  return {
    state,
    event: asCanonical({
      event_type: "action_staged",
      staged_action_type: publicActionType(staged),
      turn_cost: staged.turn_cost,
      power_cost: staged.power_cost,
    }),
    timerRequests: [],
  };
}

function stagedAction(
  state: ArchiveState,
  actionType: string,
  payload: CanonicalObject,
): StagedAction {
  if (actionType === "stage_move") {
    exactKeys(payload, ["destination"]);
    const destination = stringValue(payload.destination, "destination");
    if (!isLocation(destination)) reject("invalid_payload", "destination is not a Location");
    if (!legalDestinations(state).includes(destination)) {
      if (isVaultEdge(state.location, destination)) reject("gate_closed", "the selected vault gate is closed");
      reject("illegal_action", "destination is not adjacent to the current Location");
    }
    return stagedFromCost(state, "move", destination);
  }
  if (actionType === "stage_use_verifier") {
    exactKeys(payload, []);
    if (state.location !== "records") reject("illegal_action", "the verifier is in Records");
    if (state.verifier_result !== "none") {
      reject("illegal_action", "the catalog verifier result is already recorded");
    }
    requirePower(state, state.operation_costs.use_verifier.power_cost);
    return stagedFromCost(state, "use_verifier");
  }
  if (actionType === "stage_inspect_records") {
    exactKeys(payload, []);
    if (state.location !== "records") reject("illegal_action", "the intake evidence is in Records");
    if (state.evidence.records === "observed") reject("illegal_action", "the Records source has already been inspected");
    return stagedFromCost(state, "inspect_records");
  }
  if (actionType === "stage_inspect_conservation") {
    exactKeys(payload, []);
    if (state.location !== "conservation") reject("illegal_action", "the restoration evidence is in Conservation");
    if (state.evidence.conservation === "observed") reject("illegal_action", "the Conservation source has already been inspected");
    return stagedFromCost(state, "inspect_conservation");
  }
  if (actionType === "stage_accept_preservation_agreement") {
    exactKeys(payload, []);
    if (state.location !== "conservation") {
      reject("illegal_action", "the archivist offers the preservation agreement in Conservation");
    }
    if (state.preservation_agreement === "accepted") {
      reject("illegal_action", "the preservation agreement is already accepted");
    }
    return stagedFromCost(state, "accept_preservation_agreement");
  }
  if (actionType === "stage_prepare_collection") {
    exactKeys(payload, []);
    if (state.location !== "conservation") {
      reject("illegal_action", "the threatened collection is in Conservation");
    }
    if (state.collection_preservation !== "unprepared") {
      reject("illegal_action", "the collection is already prepared or preserved");
    }
    return stagedFromCost(state, "prepare_collection");
  }
  if (actionType === "stage_energize_preservation_equipment") {
    exactKeys(payload, []);
    if (state.location !== "conservation") {
      reject("illegal_action", "the preservation equipment is in Conservation");
    }
    if (state.collection_preservation !== "prepared") {
      reject("illegal_action", "the collection must be prepared before energizing the equipment");
    }
    requirePower(state, state.operation_costs.energize_preservation_equipment.power_cost);
    return stagedFromCost(state, "energize_preservation_equipment");
  }
  if (actionType === "stage_open_service_hatch") {
    exactKeys(payload, []);
    if (state.location !== "plant") reject("illegal_action", "the service hatch controls are in Plant");
    if (state.gates.plant_vault_open) reject("illegal_action", "the service hatch is already open");
    requirePower(state, state.operation_costs.open_service_hatch.power_cost);
    return stagedFromCost(state, "open_service_hatch");
  }
  if (actionType === "stage_recover_candidate") {
    exactKeys(payload, ["candidate_id"]);
    if (state.location !== "vault") reject("illegal_action", "candidate ledgers are in the Vault");
    const candidateId = stringValue(payload.candidate_id, "candidate_id");
    if (!isCandidateId(candidateId)) {
      reject("unknown_candidate", "candidate_id is not present in this scenario");
    }
    return stagedFromCost(state, "recover_candidate", "none", candidateId);
  }
  if (actionType === "stage_protect_source_record") {
    exactKeys(payload, []);
    if (state.location !== "plant") {
      reject("illegal_action", "the source record controls are in Plant");
    }
    if (state.carried_candidate_id === "none") {
      reject("illegal_action", "a ledger must be recovered before protecting its source record");
    }
    if (state.source_record_protected) {
      reject("illegal_action", "the source record is already protected");
    }
    requirePower(state, state.operation_costs.protect_source_record.power_cost);
    return stagedFromCost(state, "protect_source_record");
  }
  if (actionType === "stage_extract") {
    exactKeys(payload, []);
    if (state.location !== "atrium") reject("illegal_action", "extraction is available only in the Atrium");
    return stagedFromCost(state, "extract");
  }
  if (actionType === "stage_wait") {
    exactKeys(payload, []);
    return stagedFromCost(state, "wait");
  }
  reject("invalid_payload", "Action type is not declared by Midnight Archive");
}

function stagedFromCost(
  state: ArchiveState,
  kind: Exclude<StagedAction["kind"], "none">,
  destination: Location | "none" = "none",
  candidateId: CandidateId | "none" = "none",
): StagedAction {
  return { kind, destination, candidate_id: candidateId, ...state.operation_costs[kind] };
}

function commitTurn(
  current: ArchiveState,
  payload: CanonicalObject,
  core?: CanonicalObject,
  scheduled: CanonicalObject = {},
): AppliedLeadAction {
  exactKeys(payload, []);
  if (current.staged_action.kind === "none") {
    reject("nothing_staged", "stage one personal action before committing the turn");
  }
  const staged = current.staged_action;
  validateStagedAtTurnStart(current, staged);
  const turnWork = prepareTurn(current, core);
  if (turnWork.view.status === "conflict") reject("turn_conflict", "resolve or defer conflicting prepared contributions before committing");
  if (staged.kind === "extract") validateExtractionCommit(current, core);
  let next = cloneState(current) as MutableArchiveState;
  const kind = staged.kind;
  if (kind === "move") {
    next.location = staged.destination as Location;
  } else if (kind === "use_verifier") {
    next.power_remaining -= next.operation_costs.use_verifier.power_cost;
    next.verifier_result = next.truth_marker;
    if (next.carried_candidate_id !== "none") {
      next.carried_confidence = next.carried_candidate_id === next.verifier_result
        ? "verified"
        : "unverified";
    }
  } else if (kind === "inspect_records") {
    next.evidence = { ...next.evidence, records: "observed" };
  } else if (kind === "inspect_conservation") {
    next.evidence = { ...next.evidence, conservation: "observed" };
  } else if (kind === "accept_preservation_agreement") {
    next.preservation_agreement = "accepted";
  } else if (kind === "prepare_collection") {
    next.collection_preservation = "prepared";
  } else if (kind === "energize_preservation_equipment") {
    next.power_remaining -= next.operation_costs.energize_preservation_equipment.power_cost;
    next.collection_preservation = "preserved";
  } else if (kind === "open_service_hatch") {
    next.power_remaining -= next.operation_costs.open_service_hatch.power_cost;
    next.gates = { ...next.gates, plant_vault_open: true };
  } else if (kind === "recover_candidate") {
    next.carried_candidate_id = staged.candidate_id;
    next.carried_confidence = staged.candidate_id === next.verifier_result
      ? "verified"
      : "unverified";
  } else if (kind === "protect_source_record") {
    next.power_remaining -= next.operation_costs.protect_source_record.power_cost;
    next.source_record_protected = true;
  }

  next.gates = {
    ...next.gates,
    conservation_vault_open: next.preservation_agreement === "accepted" &&
      next.collection_preservation === "preserved",
  };

  next = resolveCompanions(current, next, turnWork) as MutableArchiveState;

  next.turns_used += staged.turn_cost;
  next.staged_action = emptyStage();
  if (kind === "extract") {
    next.phase = "complete";
    next.outcome = extractionOutcome(next);
  } else if (next.turns_used >= next.turn_limit) {
    next.phase = "complete";
    next.outcome = {
      kind: "exhausted_inside",
      factual_reason: "The turn budget ended with the crew inside the archive.",
      extracted_candidate_id: "none",
    };
  }
  const timerRequests = next.phase === "complete"
    ? [...cancelMiraPlanningTimer(scheduled), ...cancelMiraPlanningTimer(scheduled, "jonah")]
    : [];
  if (next.phase === "complete") next = closeMiraAtTerminal(closeMiraAtTerminal(next), "jonah") as MutableArchiveState;
  else next = invalidateExtraction(next) as MutableArchiveState;
  return {
    state: next,
    event: asCanonical({
      event_type: "turn_committed",
      committed_action_type: publicActionType(staged),
      turn: next.turns_used,
      location: next.location,
      power: next.power_remaining,
      outcome_kind: next.outcome.kind,
      mira_contribution: next.mira.last_contribution.turn === next.turns_used
        ? next.mira.last_contribution.kind
        : "none",
      mira_plan_revision: next.mira.plan.revision,
      jonah_contribution: next.jonah.last_contribution.turn === next.turns_used ? next.jonah.last_contribution.kind : "none",
      jonah_plan_revision: next.jonah.plan.revision,
    }),
    timerRequests,
  };
}

function validateStagedAtTurnStart(state: ArchiveState, staged: StagedAction): void {
  if (staged.kind === "move") {
    if (!isLocation(staged.destination) || !legalDestinations(state).includes(staged.destination)) {
      if (isLocation(staged.destination) && isVaultEdge(state.location, staged.destination)) {
        reject("gate_closed", "the selected vault gate was closed at the beginning of the turn");
      }
      reject("illegal_action", "the staged destination is no longer adjacent");
    }
  } else if (staged.kind === "use_verifier") {
    if (state.location !== "records" || state.verifier_result !== "none") {
      reject("illegal_action", "the verifier is no longer eligible");
    }
    requirePower(state, state.operation_costs.use_verifier.power_cost);
  } else if (staged.kind === "inspect_records") {
    if (state.location !== "records" || state.evidence.records === "observed") {
      reject("illegal_action", "the Records evidence is no longer eligible");
    }
  } else if (staged.kind === "inspect_conservation") {
    if (state.location !== "conservation" || state.evidence.conservation === "observed") {
      reject("illegal_action", "the Conservation evidence is no longer eligible");
    }
  } else if (staged.kind === "accept_preservation_agreement") {
    if (state.location !== "conservation" || state.preservation_agreement === "accepted") {
      reject("illegal_action", "the preservation agreement is no longer eligible for acceptance");
    }
  } else if (staged.kind === "prepare_collection") {
    if (state.location !== "conservation" || state.collection_preservation !== "unprepared") {
      reject("illegal_action", "the collection can no longer be prepared by this action");
    }
  } else if (staged.kind === "energize_preservation_equipment") {
    if (state.location !== "conservation" || state.collection_preservation !== "prepared") {
      reject("illegal_action", "the preservation equipment is no longer eligible to run");
    }
    requirePower(state, state.operation_costs.energize_preservation_equipment.power_cost);
  } else if (staged.kind === "open_service_hatch") {
    if (state.location !== "plant" || state.gates.plant_vault_open) {
      reject("illegal_action", "the service hatch action is no longer eligible");
    }
    requirePower(state, state.operation_costs.open_service_hatch.power_cost);
  } else if (staged.kind === "recover_candidate") {
    if (state.location !== "vault") reject("illegal_action", "the Vault is no longer occupied");
    if (!isCandidateId(staged.candidate_id)) reject("unknown_candidate", "the staged candidate is absent");
  } else if (staged.kind === "protect_source_record") {
    if (
      state.location !== "plant" ||
      state.carried_candidate_id === "none" ||
      state.source_record_protected
    ) {
      reject("illegal_action", "the source record protection action is no longer eligible");
    }
    requirePower(state, state.operation_costs.protect_source_record.power_cost);
  } else if (staged.kind === "extract" && state.location !== "atrium") {
    reject("illegal_action", "the crew is no longer at the Atrium");
  }
}

function extractionOutcome(state: ArchiveState): ArchiveOutcome {
  if (state.carried_candidate_id === "none") {
    return {
      kind: "no_ledger",
      factual_reason: "The crew extracted without a ledger.",
      extracted_candidate_id: "none",
    };
  }
  if (state.carried_candidate_id === state.truth_marker) {
    return {
      kind: state.extraction.left_behind_roles.length === 0 ? "success" : "partial_extraction",
      factual_reason: state.extraction.left_behind_roles.length === 0
        ? "The authentic ledger and the entire starting crew were extracted."
        : "The authentic ledger was extracted, but some starting crew were left behind.",
      extracted_candidate_id: state.carried_candidate_id,
    };
  }
  return {
    kind: "wrong_ledger",
    factual_reason: "The extracted ledger does not match the archive's authentic record.",
    extracted_candidate_id: state.carried_candidate_id,
  };
}

export function legalDestinations(state: ArchiveState): Location[] {
  return legalDestinationsFrom(state, state.location);
}

function isVaultEdge(left: Location, right: Location): boolean {
  return (
    (left === "conservation" && right === "vault") ||
    (left === "vault" && right === "conservation") ||
    (left === "plant" && right === "vault") ||
    (left === "vault" && right === "plant")
  );
}

function requirePower(state: ArchiveState, cost: number): void {
  if (state.power_remaining < cost) {
    reject("insufficient_power", `the staged action requires ${cost} power`);
  }
}

function exactKeys(payload: CanonicalObject, expected: readonly string[]): void {
  const actual = Object.keys(payload).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((value, index) => value !== wanted[index])) {
    reject("invalid_payload", "Action payload fields do not match the declared action");
  }
}

function publicActionType(staged: StagedAction): string {
  if (staged.kind === "move") return "stage_move";
  if (staged.kind === "inspect_records") return "stage_inspect_records";
  if (staged.kind === "inspect_conservation") return "stage_inspect_conservation";
  if (staged.kind === "use_verifier") return "stage_use_verifier";
  if (staged.kind === "accept_preservation_agreement") return "stage_accept_preservation_agreement";
  if (staged.kind === "prepare_collection") return "stage_prepare_collection";
  if (staged.kind === "energize_preservation_equipment") return "stage_energize_preservation_equipment";
  if (staged.kind === "open_service_hatch") return "stage_open_service_hatch";
  if (staged.kind === "recover_candidate") return "stage_recover_candidate";
  if (staged.kind === "protect_source_record") return "stage_protect_source_record";
  if (staged.kind === "extract") return "stage_extract";
  if (staged.kind === "wait") return "stage_wait";
  return "none";
}

type MutableArchiveState = {
  -readonly [Key in keyof ArchiveState]: ArchiveState[Key];
};

export function stateAsCanonical(state: ArchiveState): CanonicalObject {
  return state as unknown as CanonicalObject;
}

export function payload(value: CanonicalJson | undefined): CanonicalObject {
  return record(value, "Action payload");
}
