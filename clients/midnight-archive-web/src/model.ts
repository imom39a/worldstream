import type { JsonValue } from "@worldstream/client";

import {
  isMidnightArchiveActionType,
  type MidnightArchiveActionType,
} from "./actionContract";

export const ARCHIVE_LOCATIONS = [
  "atrium",
  "records",
  "conservation",
  "plant",
  "vault",
] as const;

export type ArchiveLocation = typeof ARCHIVE_LOCATIONS[number];
export type ArchiveGate = "archive_gate" | "service_hatch";
export type GateState = "closed" | "open";
export type ArchivePhase = "briefing" | "active" | "complete";
export type ArchiveCandidateId = "ledger-amber" | "ledger-cobalt" | "ledger-violet";
export type ArchiveOutcomeKind =
  | "success"
  | "partial_extraction"
  | "wrong_ledger"
  | "no_ledger"
  | "exhausted_inside";
export type AgreementCommitment = "not_accepted" | "accepted" | "honored";
export type AgreementConditionId =
  | "lead_acceptance"
  | "collection_preparation"
  | "equipment_energized";
export type MiraPresence = "absent" | "active" | "suspended";
export type MiraMode = "unavailable" | "following" | "holding" | "tasked" | "regrouping";
export type ArchiveRole = "lead" | "mira" | "jonah";
export type CompanionRole = Exclude<ArchiveRole, "lead">;
export type SpecialistTaskKind =
  | "none"
  | "investigate_records"
  | "investigate_conservation"
  | "open_service_hatch"
  | "field_assay";
export type MiraTaskKind = SpecialistTaskKind;
export type MiraTaskStatus = "none" | "assigned" | "complete" | "cancelled";
export type MiraPlanningStatus = "not_requested" | "waiting" | "ready" | "expired" | "complete";
export type MiraPreparationStatus = "none" | "prepared" | "deferred";
export type MiraKnowledgeStatus = "unknown" | "private" | "shared";
export type MiraContributionKind =
  | "none"
  | "move"
  | "inspect_source"
  | "share_source"
  | "use_verifier"
  | "follow_move"
  | "regroup_move"
  | "open_service_hatch"
  | "collect_assay_sample"
  | "complete_field_assay";

export interface SpecialistCrewState {
  readonly dialogueAllowed: boolean;
  readonly presence: MiraPresence;
  readonly location: ArchiveLocation | "none";
  readonly mode: MiraMode;
  readonly task: {
    readonly status: MiraTaskStatus;
    readonly revision: number;
    readonly kind: MiraTaskKind;
    readonly powerAllowance: 0 | 1 | 2;
    readonly powerSpent: 0 | 1 | 2;
  };
  readonly planning: {
    readonly status: MiraPlanningStatus;
    readonly opportunityRevision: number;
    readonly planRevision: number;
    readonly stepsTotal: number;
    readonly stepsCompleted: number;
    readonly deadline: "none" | string;
  };
  readonly preparation: {
    readonly status: MiraPreparationStatus;
    readonly forTurn: number;
    readonly summary: string;
  };
  readonly knowledge: {
    readonly records: MiraKnowledgeStatus;
    readonly conservation: MiraKnowledgeStatus;
    readonly verifierResult: null | {
      readonly candidateId: ArchiveCandidateId;
      readonly confidence: "verified";
    };
  };
  readonly fieldAssay: {
    readonly stepsCompleted: 0 | 1 | 2;
    readonly result: null | {
      readonly candidateId: ArchiveCandidateId;
      readonly confidence: "verified";
    };
  };
  readonly lastContribution: {
    readonly turn: number;
    readonly kind: MiraContributionKind;
    readonly summary: string;
  };
}

export type MiraCrewState = SpecialistCrewState;

export interface ArchiveTurnResolution {
  readonly status: "clear" | "conflict";
  readonly powerReserved: number;
  readonly preparedRoles: readonly CompanionRole[];
  readonly deferredRoles: readonly CompanionRole[];
  readonly unpreparedRoles: readonly CompanionRole[];
  readonly reservations: readonly {
    readonly role: ArchiveRole;
    readonly power: 0 | 1 | 2;
    readonly interaction: "none" | "service_hatch" | "catalog_verifier";
  }[];
  readonly conflicts: readonly {
    readonly code: "shared_power" | "service_hatch" | "catalog_verifier" | "ineligible_contribution";
    readonly roles: readonly ArchiveRole[];
  }[];
}

export interface ArchiveExtractionPreview {
  readonly status: "none" | "prepared" | "acknowledged";
  readonly revision: number;
  readonly forTurn: number;
  readonly extractedRoles: readonly ArchiveRole[];
  readonly leftBehindRoles: readonly CompanionRole[];
}

export interface ArchiveCrewDebrief {
  readonly startingRoles: readonly ArchiveRole[];
  readonly extractedRoles: readonly ArchiveRole[];
  readonly leftBehindRoles: readonly ArchiveRole[];
  readonly completedWork: readonly {
    readonly role: CompanionRole;
    readonly kind: MiraContributionKind;
    readonly turn: number;
  }[];
}

export interface PreservationAgreementCondition {
  readonly conditionId: AgreementConditionId;
  readonly label: string;
  readonly status: "blocked" | "pending" | "complete";
  readonly turnCost: 1;
  readonly powerCost: 0 | 1;
}

export interface PreservationAgreement {
  readonly speaker: "Archivist";
  readonly statement: "Preserve the threatened collection and I will open the Conservation–Vault gate.";
  readonly commitment: AgreementCommitment;
  readonly conditions: readonly PreservationAgreementCondition[];
}

export interface ArchiveOptionalObjectives {
  readonly collectionPreserved: {
    readonly label: "Preserve the threatened collection";
    readonly status: "not_started" | "prepared" | "complete";
    readonly turnCost: 2;
    readonly powerCost: 1;
  };
  readonly sourceRecordProtected: {
    readonly label: "Protect the source's identifying record";
    readonly status: "locked" | "available" | "complete";
    readonly turnCost: 1;
    readonly powerCost: 1;
  };
}

export interface ArchiveMapLocation {
  readonly id: ArchiveLocation;
  readonly name: string;
  readonly description: string;
}

export interface ArchiveMapConnection {
  readonly from: ArchiveLocation;
  readonly to: ArchiveLocation;
  readonly gate: ArchiveGate | null;
}

export interface ArchiveCandidate {
  readonly candidateId: ArchiveCandidateId;
  readonly label: string;
  readonly visibleAttributes: readonly {
    readonly label: string;
    readonly value: string;
  }[];
  readonly evidenceAssessment: "unknown" | "observed" | "recommended";
  readonly observedEvidence: readonly {
    readonly sourceId: "records" | "conservation";
    readonly sourceLabel: string;
    readonly attributeLabel: "Binding" | "Marking" | "Year";
    readonly observedValue: string;
    readonly candidateValue: string;
    readonly relation: "matches" | "does_not_match";
  }[];
}

export type ArchiveStagedAction =
  | {
      readonly actionType: "stage_inspect_records" | "stage_inspect_conservation";
      readonly turnCost: 1;
      readonly powerCost: 0;
    }
  | {
      readonly actionType: "stage_move";
      readonly destination: ArchiveLocation;
      readonly turnCost: 1;
      readonly powerCost: 0;
    }
  | {
      readonly actionType: "stage_use_verifier";
      readonly turnCost: 1;
      readonly powerCost: 1;
    }
  | {
      readonly actionType:
        | "stage_accept_preservation_agreement"
        | "stage_prepare_collection";
      readonly turnCost: 1;
      readonly powerCost: 0;
    }
  | {
      readonly actionType:
        | "stage_energize_preservation_equipment"
        | "stage_protect_source_record";
      readonly turnCost: 1;
      readonly powerCost: 1;
    }
  | {
      readonly actionType: "stage_open_service_hatch";
      readonly turnCost: 1;
      readonly powerCost: 2;
    }
  | {
      readonly actionType: "stage_recover_candidate";
      readonly candidateId: ArchiveCandidateId;
      readonly turnCost: 1;
      readonly powerCost: 0;
    }
  | {
      readonly actionType: "stage_extract" | "stage_wait";
      readonly turnCost: 1;
      readonly powerCost: 0;
    };

const OPERATION_COSTS = {
  move: [1, 0], inspect_records: [1, 0], inspect_conservation: [1, 0],
  use_verifier: [1, 1], accept_preservation_agreement: [1, 0],
  prepare_collection: [1, 0], energize_preservation_equipment: [1, 1],
  open_service_hatch: [1, 2], recover_candidate: [1, 0],
  protect_source_record: [1, 1], extract: [1, 0], wait: [1, 0],
  companion_move: [0, 0], companion_inspect_source: [0, 0], companion_share_source: [0, 0],
  mira_field_assay: [0, 0], companion_verifier: [0, 1], mira_open_service_hatch: [0, 2],
  jonah_open_service_hatch: [0, 1],
} as const;
export type ArchiveOperation = keyof typeof OPERATION_COSTS;

export interface MidnightArchiveProjection {
  readonly companionDialogue: readonly { readonly speaker: CompanionRole; readonly turn: number; readonly text: string }[];
  readonly scenario:
    | { readonly id: "standard-v1"; readonly label: "Standard" }
    | { readonly id: "low-reserve-v1"; readonly label: "Low Reserve" };
  readonly initialPower: 2 | 3;
  readonly methodCosts: { readonly verifier: 1; readonly ordinaryServiceHatch: 2 };
  readonly operationCosts: Readonly<Record<ArchiveOperation, ArchiveActionCost>>;
  readonly phase: ArchivePhase;
  readonly objective: string;
  readonly location: ArchiveLocation;
  readonly turnsUsed: number;
  readonly turnsRemaining: number;
  readonly power: number;
  readonly gates: Readonly<Record<ArchiveGate, GateState>>;
  readonly map: {
    readonly locations: readonly ArchiveMapLocation[];
    readonly connections: readonly ArchiveMapConnection[];
  };
  readonly candidates: readonly ArchiveCandidate[];
  readonly preservationAgreement: PreservationAgreement;
  readonly optionalObjectives: ArchiveOptionalObjectives;
  readonly mira: MiraCrewState;
  readonly jonah: SpecialistCrewState;
  readonly turnResolution: ArchiveTurnResolution;
  readonly extraction: ArchiveExtractionPreview;
  readonly crewDebrief: ArchiveCrewDebrief;
  readonly debrief: null | {
    readonly evidenceStatus: "none" | "partial" | "complete";
    readonly message: string;
    readonly agreementCommitment: AgreementCommitment;
    readonly optionalObjectives: {
      readonly collectionPreserved: boolean;
      readonly sourceRecordProtected: boolean;
    };
  };
  readonly stagedAction: ArchiveStagedAction | null;
  readonly carriedCandidate: ArchiveCandidateId | null;
  readonly verifierResult: null | {
    readonly candidateId: ArchiveCandidateId;
    readonly confidence: "verified";
  };
  readonly outcome: null | { readonly kind: ArchiveOutcomeKind };
}

export type MidnightArchiveActionIntent =
  | { readonly action: "stage_move"; readonly destination: ArchiveLocation }
  | { readonly action: "stage_inspect_records" }
  | { readonly action: "stage_inspect_conservation" }
  | { readonly action: "stage_use_verifier" }
  | { readonly action: "stage_accept_preservation_agreement" }
  | { readonly action: "stage_prepare_collection" }
  | { readonly action: "stage_energize_preservation_equipment" }
  | { readonly action: "stage_open_service_hatch" }
  | { readonly action: "stage_recover_candidate"; readonly candidate_id: ArchiveCandidateId }
  | { readonly action: "stage_protect_source_record" }
  | { readonly action: "stage_extract" }
  | { readonly action: "stage_wait" }
  | { readonly action: "commit_turn" }
  | {
      readonly action: "assign_mira_task";
      readonly task_kind: Exclude<MiraTaskKind, "none">;
      readonly power_allowance: 0 | 1 | 2;
    }
  | { readonly action: "cancel_mira_task" }
  | { readonly action: "set_mira_follow" }
  | { readonly action: "set_mira_hold" }
  | { readonly action: "set_mira_regroup" }
  | { readonly action: "request_mira_plan" }
  | { readonly action: "prepare_mira_contribution" }
  | { readonly action: "defer_mira_contribution" }
  | {
      readonly action: "assign_jonah_task";
      readonly task_kind: Exclude<SpecialistTaskKind, "none" | "field_assay">;
      readonly power_allowance: 0 | 1;
    }
  | { readonly action: "cancel_jonah_task" }
  | { readonly action: "set_jonah_follow" }
  | { readonly action: "set_jonah_hold" }
  | { readonly action: "set_jonah_regroup" }
  | { readonly action: "request_jonah_plan" }
  | { readonly action: "prepare_jonah_contribution" }
  | { readonly action: "defer_jonah_contribution" }
  | { readonly action: "prepare_extraction" }
  | {
      readonly action: "acknowledge_extraction";
      readonly preview_revision: number;
      readonly left_behind_roles: readonly CompanionRole[];
    };

export interface ArchiveActionCost {
  readonly turns: 0 | 1;
  readonly power: 0 | 1 | 2;
}

export interface ArchiveMoveOption {
  readonly destination: ArchiveLocation;
  readonly gate: ArchiveGate | null;
  readonly open: boolean;
}

const MAX_OBJECTIVE_BYTES = 384;
const MAX_LABEL_BYTES = 64;
const MAX_DESCRIPTION_BYTES = 256;
const ROOT_KEYS = [
  "companion_dialogue",
  "scenario",
  "initial_power",
  "method_costs",
  "operation_costs",
  "candidates",
  "crew_debrief",
  "debrief",
  "carried_candidate",
  "extraction",
  "gates",
  "location",
  "map",
  "mira",
  "jonah",
  "objective",
  "outcome",
  "optional_objectives",
  "phase",
  "power",
  "preservation_agreement",
  "staged_action",
  "turns_remaining",
  "turn_resolution",
  "turns_used",
  "verifier_result",
] as const;
const EXPECTED_CONNECTIONS = new Map<string, ArchiveGate | null>([
  ["atrium|records", null],
  ["atrium|conservation", null],
  ["conservation|records", null],
  ["plant|records", null],
  ["conservation|vault", "archive_gate"],
  ["plant|vault", "service_hatch"],
]);

/** Reads the complete authorized participant Projection and rejects any drift. */
export function readMidnightArchiveProjection(
  value: unknown,
): MidnightArchiveProjection | null {
  const source = exactRecord(value, ROOT_KEYS);
  if (source === null) return null;

  const companionDialogue = readCompanionDialogue(source.companion_dialogue);
  const scenario = readScenario(source.scenario);
  const initialPower = source.initial_power === 2 || source.initial_power === 3
    ? source.initial_power : null;
  const methodCosts = readMethodCosts(source.method_costs);
  const operationCosts = readOperationCosts(source.operation_costs);
  const phase = readPhase(source.phase);
  const objective = boundedText(source.objective, MAX_OBJECTIVE_BYTES);
  const location = readLocation(source.location);
  const turnsUsed = integerInRange(source.turns_used, 0, 16);
  const turnsRemaining = integerInRange(source.turns_remaining, 0, 16);
  const power = integerInRange(source.power, 0, 3);
  const gates = readGates(source.gates);
  const map = readMap(source.map);
  const candidates = readCandidates(source.candidates);
  const mira = readSpecialist(source.mira, "mira");
  const jonah = readSpecialist(source.jonah, "jonah");
  const turnResolution = readTurnResolution(source.turn_resolution);
  const extraction = readExtraction(source.extraction);
  const crewDebrief = readCrewDebrief(source.crew_debrief);
  const preservationAgreement = readPreservationAgreement(source.preservation_agreement);
  const optionalObjectives = readOptionalObjectives(source.optional_objectives);
  const debrief = readDebrief(source.debrief);
  const stagedAction = operationCosts === null
    ? undefined
    : readStagedAction(source.staged_action, operationCosts);
  const carriedCandidate = source.carried_candidate === null
    ? null
    : candidateId(source.carried_candidate);
  const verifierResult = readVerifierResult(source.verifier_result);
  const outcome = readOutcome(source.outcome);

  if (
    companionDialogue === null || scenario === null || initialPower === null || methodCosts === null || operationCosts === null
    || initialPower !== (scenario.id === "standard-v1" ? 3 : 2)
    || phase === null || objective === null || location === null
    || turnsUsed === null || turnsRemaining === null || power === null
    || power > initialPower
    || gates === null || map === null || candidates === null || mira === null || jonah === null
    || turnResolution === null || extraction === null || crewDebrief === null
    || preservationAgreement === null || optionalObjectives === null
    || debrief === undefined
    || stagedAction === undefined || carriedCandidate === undefined
    || verifierResult === undefined || outcome === undefined
    || turnsUsed + turnsRemaining !== 16
  ) return null;

  const candidateIds = new Set(candidates.map((candidate) => candidate.candidateId));
  const expectedDebriefStatus = (["none", "partial", "complete"] as const)[
    candidates[0]!.observedEvidence.length
  ];
  if (
    (carriedCandidate !== null && !candidateIds.has(carriedCandidate))
    || (verifierResult !== null && !candidateIds.has(verifierResult.candidateId))
    || (stagedAction?.actionType === "stage_recover_candidate"
      && !candidateIds.has(stagedAction.candidateId))
    || !agreementStateIsConsistent(preservationAgreement, optionalObjectives, gates)
    || !specialistStateFitsProjection(mira, "mira", turnsUsed, power, candidates, verifierResult)
    || !specialistStateFitsProjection(jonah, "jonah", turnsUsed, power, candidates, verifierResult)
    || !crewStateIsConsistent(
      mira, jonah, turnResolution, extraction, crewDebrief, phase, turnsUsed, power,
      stagedAction, operationCosts,
    )
    || !stagedActionFitsProjection(
      stagedAction,
      location,
      power,
      gates,
      map,
      preservationAgreement,
      optionalObjectives,
      verifierResult,
      carriedCandidate,
      operationCosts,
    )
    || (phase === "briefing" && (turnsUsed !== 0 || stagedAction !== null || outcome !== null))
    || (phase === "active" && (outcome !== null || debrief !== null))
    || (phase === "complete" && (outcome === null || debrief === null))
    || (phase === "complete" && debrief?.evidenceStatus !== expectedDebriefStatus)
    || (optionalObjectives.sourceRecordProtected.status !== "locked" && carriedCandidate === null)
    || (optionalObjectives.sourceRecordProtected.status === "locked" && carriedCandidate !== null)
    || (debrief !== null && (
      debrief.agreementCommitment !== preservationAgreement.commitment
      || debrief.optionalObjectives.collectionPreserved !==
        (optionalObjectives.collectionPreserved.status === "complete")
      || debrief.optionalObjectives.sourceRecordProtected !==
        (optionalObjectives.sourceRecordProtected.status === "complete")
    ))
    || (outcome?.kind === "exhausted_inside" && turnsRemaining !== 0)
    || (phase === "complete" && outcome !== null && !terminalOutcomeIsConsistent(
      outcome.kind, carriedCandidate, extraction, crewDebrief,
    ))
  ) return null;

  return {
    companionDialogue,
    scenario,
    initialPower,
    methodCosts,
    operationCosts,
    phase,
    objective,
    location,
    turnsUsed,
    turnsRemaining,
    power,
    gates,
    map,
    candidates,
    mira,
    jonah,
    turnResolution,
    extraction,
    crewDebrief,
    preservationAgreement,
    optionalObjectives,
    debrief,
    stagedAction,
    carriedCandidate,
    verifierResult,
    outcome,
  };
}

function readScenario(value: unknown): MidnightArchiveProjection["scenario"] | null {
  const source = exactRecord(value, ["id", "label"]);
  if (source?.id === "standard-v1" && source.label === "Standard") {
    return { id: "standard-v1", label: "Standard" };
  }
  if (source?.id === "low-reserve-v1" && source.label === "Low Reserve") {
    return { id: "low-reserve-v1", label: "Low Reserve" };
  }
  return null;
}

function readMethodCosts(value: unknown): MidnightArchiveProjection["methodCosts"] | null {
  const source = exactRecord(value, ["verifier", "ordinary_service_hatch"]);
  return source?.verifier === 1 && source.ordinary_service_hatch === 2
    ? { verifier: 1, ordinaryServiceHatch: 2 } : null;
}

function readOperationCosts(value: unknown): MidnightArchiveProjection["operationCosts"] | null {
  const source = exactRecord(value, Object.keys(OPERATION_COSTS));
  if (source === null) return null;
  const entries: Array<[string, ArchiveActionCost]> = [];
  for (const [operation, [turns, power]] of Object.entries(OPERATION_COSTS)) {
    const cost = exactRecord(source[operation], ["turn_cost", "power_cost"]);
    if (cost?.turn_cost !== turns || cost.power_cost !== power) return null;
    entries.push([operation, { turns, power }]);
  }
  return Object.fromEntries(entries) as Record<ArchiveOperation, ArchiveActionCost>;
}

export function actionCost(
  projection: Pick<MidnightArchiveProjection, "operationCosts">,
  action: MidnightArchiveActionType,
): ArchiveActionCost {
  return actionCostFromOperations(projection.operationCosts, action);
}

const STAGED_ACTION_OPERATIONS: Readonly<Partial<Record<MidnightArchiveActionType, ArchiveOperation>>> = {
  stage_move: "move",
  stage_inspect_records: "inspect_records",
  stage_inspect_conservation: "inspect_conservation",
  stage_use_verifier: "use_verifier",
  stage_accept_preservation_agreement: "accept_preservation_agreement",
  stage_prepare_collection: "prepare_collection",
  stage_energize_preservation_equipment: "energize_preservation_equipment",
  stage_open_service_hatch: "open_service_hatch",
  stage_recover_candidate: "recover_candidate",
  stage_protect_source_record: "protect_source_record",
  stage_extract: "extract",
  stage_wait: "wait",
};

function actionCostFromOperations(
  operationCosts: MidnightArchiveProjection["operationCosts"],
  action: MidnightArchiveActionType,
): ArchiveActionCost {
  const operation = STAGED_ACTION_OPERATIONS[action];
  return operation === undefined ? { turns: 0, power: 0 } : operationCosts[operation];
}

export function actionPayload(intent: MidnightArchiveActionIntent): JsonValue {
  switch (intent.action) {
    case "stage_move":
      return { destination: intent.destination };
    case "stage_recover_candidate":
      return { candidate_id: intent.candidate_id };
    case "assign_mira_task":
    case "assign_jonah_task":
      return {
        task_kind: intent.task_kind,
        power_allowance: intent.power_allowance,
      };
    case "acknowledge_extraction":
      if (!Number.isSafeInteger(intent.preview_revision) || intent.preview_revision < 1
        || !isCanonicalRoleOrder(intent.left_behind_roles, COMPANION_ORDER)
        || new Set(intent.left_behind_roles).size !== intent.left_behind_roles.length) {
        throw new Error("Extraction acknowledgement must use the canonical role order and current positive preview revision.");
      }
      return {
        preview_revision: intent.preview_revision,
        left_behind_roles: [...intent.left_behind_roles],
      };
    default:
      return {};
  }
}

export function moveOptions(
  projection: MidnightArchiveProjection,
): readonly ArchiveMoveOption[] {
  return projection.map.connections.flatMap((connection) => {
    const destination = connection.from === projection.location
      ? connection.to
      : connection.to === projection.location
        ? connection.from
        : null;
    if (destination === null) return [];
    return [{
      destination,
      gate: connection.gate,
      open: connection.gate === null || projection.gates[connection.gate] === "open",
    }];
  });
}

export function candidateConfidence(
  projection: MidnightArchiveProjection,
  candidateId: ArchiveCandidateId,
): "verified" | "unverified" {
  return projection.verifierResult?.candidateId === candidateId
    ? "verified"
    : "unverified";
}

export function candidateById(
  projection: MidnightArchiveProjection,
  candidateId: ArchiveCandidateId | null,
): ArchiveCandidate | null {
  if (candidateId === null) return null;
  return projection.candidates.find((candidate) => candidate.candidateId === candidateId) ?? null;
}

function readGates(value: unknown): Readonly<Record<ArchiveGate, GateState>> | null {
  const source = exactRecord(value, ["archive_gate", "service_hatch"]);
  if (source === null) return null;
  const archiveGate = readGateState(source.archive_gate);
  const serviceHatch = readGateState(source.service_hatch);
  return archiveGate === null || serviceHatch === null
    ? null
    : { archive_gate: archiveGate, service_hatch: serviceHatch };
}

function readMap(value: unknown): MidnightArchiveProjection["map"] | null {
  const source = exactRecord(value, ["connections", "locations"]);
  if (source === null || !Array.isArray(source.locations) || !Array.isArray(source.connections)) {
    return null;
  }
  if (source.locations.length !== ARCHIVE_LOCATIONS.length || source.connections.length !== EXPECTED_CONNECTIONS.size) {
    return null;
  }
  const locations: ArchiveMapLocation[] = [];
  const seenLocations = new Set<ArchiveLocation>();
  for (const value of source.locations) {
    const item = exactRecord(value, ["description", "id", "name"]);
    if (item === null) return null;
    const id = readLocation(item.id);
    const name = boundedText(item.name, MAX_LABEL_BYTES);
    const description = boundedText(item.description, MAX_DESCRIPTION_BYTES);
    if (id === null || name === null || description === null || seenLocations.has(id)) return null;
    seenLocations.add(id);
    locations.push({ id, name, description });
  }
  if (ARCHIVE_LOCATIONS.some((location) => !seenLocations.has(location))) return null;

  const connections: ArchiveMapConnection[] = [];
  const seenConnections = new Set<string>();
  for (const value of source.connections) {
    const item = exactRecord(value, ["from", "gate", "to"]);
    if (item === null) return null;
    const from = readLocation(item.from);
    const to = readLocation(item.to);
    const gate = item.gate === null ? null : readGate(item.gate);
    if (from === null || to === null || from === to || gate === undefined) return null;
    const key = connectionKey(from, to);
    if (!EXPECTED_CONNECTIONS.has(key) || EXPECTED_CONNECTIONS.get(key) !== gate || seenConnections.has(key)) return null;
    seenConnections.add(key);
    connections.push({ from, to, gate });
  }
  return seenConnections.size === EXPECTED_CONNECTIONS.size
    ? { locations, connections }
    : null;
}

function readCandidates(value: unknown): readonly ArchiveCandidate[] | null {
  if (!Array.isArray(value) || value.length !== 3) return null;
  const result: ArchiveCandidate[] = [];
  const seen = new Set<string>();
  const evidenceDefinitions = new Map<string, string>();
  let expectedEvidenceSources: string | null = null;
  for (const candidateValue of value) {
    const item = exactRecord(candidateValue, ["candidate_id", "evidence_assessment", "label", "observed_evidence", "visible_attributes"]);
    if (item === null || !Array.isArray(item.visible_attributes)
      || item.visible_attributes.length !== 3 || !Array.isArray(item.observed_evidence)
      || item.observed_evidence.length > 2) return null;
    const id = candidateId(item.candidate_id);
    const label = boundedText(item.label, MAX_LABEL_BYTES);
    if (id === undefined || label === null || seen.has(id)) return null;
    const visibleAttributes: Array<{ label: string; value: string }> = [];
    const visibleAttributeValues = new Map<string, string>();
    for (const value of item.visible_attributes) {
      const attribute = exactRecord(value, ["label", "value"]);
      if (attribute === null) return null;
      const attributeLabel = boundedText(attribute.label, MAX_LABEL_BYTES);
      const attributeValue = boundedText(attribute.value, MAX_LABEL_BYTES);
      if (
        attributeLabel === null || attributeValue === null ||
        !["Binding", "Marking", "Year"].includes(attributeLabel) ||
        visibleAttributeValues.has(attributeLabel)
      ) return null;
      visibleAttributeValues.set(attributeLabel, attributeValue);
      visibleAttributes.push({ label: attributeLabel, value: attributeValue });
    }
    const evidenceAssessment = item.evidence_assessment;
    if (evidenceAssessment !== "unknown" && evidenceAssessment !== "observed" && evidenceAssessment !== "recommended") return null;
    const observedEvidence: Array<ArchiveCandidate["observedEvidence"][number]> = [];
    const seenEvidenceSources = new Set<string>();
    for (const value of item.observed_evidence) {
      const evidence = exactRecord(value, ["attribute_label", "candidate_value", "observed_value", "relation", "source_id", "source_label"]);
      if (evidence === null || (evidence.source_id !== "records" && evidence.source_id !== "conservation")
        || (evidence.attribute_label !== "Binding" && evidence.attribute_label !== "Marking" && evidence.attribute_label !== "Year")
        || (evidence.relation !== "matches" && evidence.relation !== "does_not_match")) return null;
      const sourceLabel = boundedText(evidence.source_label, MAX_LABEL_BYTES);
      const observedValue = boundedText(evidence.observed_value, MAX_LABEL_BYTES);
      const candidateValue = boundedText(evidence.candidate_value, MAX_LABEL_BYTES);
      const expectedSource = evidence.source_id === "records"
        ? { label: "Records intake card", attribute: "Binding" as const }
        : { label: "Conservation restoration note", attribute: "Marking" as const };
      if (
        sourceLabel === null || observedValue === null || candidateValue === null ||
        seenEvidenceSources.has(evidence.source_id) ||
        sourceLabel !== expectedSource.label ||
        evidence.attribute_label !== expectedSource.attribute ||
        visibleAttributeValues.get(evidence.attribute_label) !== candidateValue ||
        (evidence.relation === "matches") !== (candidateValue === observedValue)
      ) return null;
      seenEvidenceSources.add(evidence.source_id);
      const definition = [sourceLabel, evidence.attribute_label, observedValue].join("\u0000");
      const retainedDefinition = evidenceDefinitions.get(evidence.source_id);
      if (retainedDefinition !== undefined && retainedDefinition !== definition) return null;
      evidenceDefinitions.set(evidence.source_id, definition);
      observedEvidence.push({
        sourceId: evidence.source_id,
        sourceLabel,
        attributeLabel: evidence.attribute_label,
        observedValue,
        candidateValue,
        relation: evidence.relation,
      });
    }
    const evidenceSources = [...seenEvidenceSources].sort().join("|");
    expectedEvidenceSources ??= evidenceSources;
    const shouldRecommend = observedEvidence.length === 2
      && observedEvidence.every((evidence) => evidence.relation === "matches");
    const expectedAssessment = observedEvidence.length === 0
      ? "unknown"
      : shouldRecommend ? "recommended" : "observed";
    if (evidenceSources !== expectedEvidenceSources || evidenceAssessment !== expectedAssessment) return null;
    seen.add(id);
    result.push({ candidateId: id, label, visibleAttributes, evidenceAssessment, observedEvidence });
  }
  if (
    expectedEvidenceSources === "conservation|records" &&
    result.filter((candidate) => candidate.evidenceAssessment === "recommended").length !== 1
  ) return null;
  return result;
}

const UTC_CANONICAL_DEADLINE = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?Z$/u;

function specialistName(role: CompanionRole): "Mira" | "Jonah" {
  return role === "mira" ? "Mira" : "Jonah";
}

function preparationSummaries(role: CompanionRole): ReadonlySet<string> {
  const name = specialistName(role);
  return new Set([
    "none",
    `${name} will move one open passage.`,
    `${name} will inspect the assigned source.`,
    `${name} will share one inspected source.`,
    `${name} will run the catalog verifier.`,
    `${name} will open the Plant service hatch.`,
    `${name} will not contribute this turn.`,
    ...(role === "mira" ? [
      "Mira will collect a Vault assay sample.",
      "Mira will complete and disclose the Vault assay.",
    ] : []),
  ]);
}

function contributionSummary(role: CompanionRole, kind: MiraContributionKind): string {
  const name = specialistName(role);
  switch (kind) {
    case "none": return `No ${name} contribution has completed.`;
    case "move": return `${name} moved one open passage.`;
    case "inspect_source": return `${name} inspected the assigned source privately.`;
    case "share_source": return `${name} shared one inspected source with the crew.`;
    case "use_verifier": return `${name} ran the catalog verifier and disclosed its result.`;
    case "follow_move": return `${name} followed the lead through one open passage.`;
    case "regroup_move": return `${name} moved one open passage toward the Atrium.`;
    case "open_service_hatch": return `${name} opened the Plant service hatch.`;
    case "collect_assay_sample": return "Mira collected a Vault assay sample; no result is available yet.";
    case "complete_field_assay": return "Mira completed the Vault assay and disclosed the verified result.";
  }
}

function readSpecialist(value: unknown, role: CompanionRole): SpecialistCrewState | null {
  const source = exactRecord(value, [
    "dialogue_allowed", "field_assay", "knowledge", "last_contribution", "location", "mode", "planning", "preparation", "presence", "task",
  ]);
  if (source === null) return null;
  const presence = source.presence;
  const location = source.location === "none" ? "none" : readLocation(source.location);
  const mode = source.mode;
  const task = exactRecord(source.task, ["kind", "power_allowance", "power_spent", "revision", "status"]);
  const planning = exactRecord(source.planning, [
    "deadline", "opportunity_revision", "plan_revision", "status", "steps_completed", "steps_total",
  ]);
  const preparation = exactRecord(source.preparation, ["for_turn", "status", "summary"]);
  const knowledge = exactRecord(source.knowledge, ["conservation", "records", "verifier_result"]);
  const fieldAssay = exactRecord(source.field_assay, ["result", "steps_completed"]);
  const lastContribution = exactRecord(source.last_contribution, ["kind", "summary", "turn"]);
  if (
    (presence !== "absent" && presence !== "active" && presence !== "suspended")
    || typeof source.dialogue_allowed !== "boolean"
    || location === null
    || (mode !== "unavailable" && mode !== "following" && mode !== "holding"
      && mode !== "tasked" && mode !== "regrouping")
    || task === null || planning === null || preparation === null || knowledge === null || fieldAssay === null
    || lastContribution === null
    || !isMiraTaskStatus(task.status) || !isSpecialistTaskKind(task.kind)
    || (role === "jonah" && task.kind === "field_assay")
    || integerInRange(task.revision, 0, 65_535) === null
    || integerInRange(task.power_allowance, 0, 2) === null
    || integerInRange(task.power_spent, 0, 2) === null
    || (role === "jonah" && (task.power_allowance === 2 || task.power_spent === 2))
    || ((task.power_allowance === 2 || task.power_spent === 2) && task.kind !== "open_service_hatch")
    || !isMiraPlanningStatus(planning.status)
    || integerInRange(planning.opportunity_revision, 0, 65_535) === null
    || integerInRange(planning.plan_revision, 0, 65_535) === null
    || integerInRange(planning.steps_total, 0, 3) === null
    || integerInRange(planning.steps_completed, 0, 3) === null
    || typeof planning.deadline !== "string" || byteLength(planning.deadline) > 64
    || !isMiraPreparationStatus(preparation.status)
    || integerInRange(preparation.for_turn, 0, 16) === null
    || typeof preparation.summary !== "string" || !preparationSummaries(role).has(preparation.summary)
    || !isMiraKnowledgeStatus(knowledge.records) || !isMiraKnowledgeStatus(knowledge.conservation)
    || !isMiraContributionKind(lastContribution.kind)
    || integerInRange(lastContribution.turn, 0, 16) === null
    || lastContribution.summary !== contributionSummary(role, lastContribution.kind)
    || integerInRange(fieldAssay.steps_completed, 0, 2) === null
  ) return null;
  const verifierResult = readVerifierResult(knowledge.verifier_result);
  const assayResult = readVerifierResult(fieldAssay.result);
  if (verifierResult === undefined || assayResult === undefined) return null;
  return {
    dialogueAllowed: source.dialogue_allowed,
    presence,
    location,
    mode,
    task: {
      status: task.status,
      revision: task.revision as number,
      kind: task.kind,
      powerAllowance: task.power_allowance as 0 | 1 | 2,
      powerSpent: task.power_spent as 0 | 1 | 2,
    },
    planning: {
      status: planning.status,
      opportunityRevision: planning.opportunity_revision as number,
      planRevision: planning.plan_revision as number,
      stepsTotal: planning.steps_total as number,
      stepsCompleted: planning.steps_completed as number,
      deadline: planning.deadline,
    },
    preparation: {
      status: preparation.status,
      forTurn: preparation.for_turn as number,
      summary: preparation.summary as MiraCrewState["preparation"]["summary"],
    },
    knowledge: {
      records: knowledge.records,
      conservation: knowledge.conservation,
      verifierResult,
    },
    fieldAssay: {
      stepsCompleted: fieldAssay.steps_completed as 0 | 1 | 2,
      result: assayResult,
    },
    lastContribution: {
      turn: lastContribution.turn as number,
      kind: lastContribution.kind,
      summary: lastContribution.summary,
    },
  };
}

function specialistStateFitsProjection(
  mira: SpecialistCrewState,
  role: CompanionRole,
  turnsUsed: number,
  power: number,
  candidates: readonly ArchiveCandidate[],
  verifierResult: MidnightArchiveProjection["verifierResult"],
): boolean {
  const { task, planning, preparation, knowledge, lastContribution } = mira;
  const taskHasAssignment = task.status === "assigned" || task.status === "complete";
  const deadlineIsCanonical = isCanonicalUtcDeadline(planning.deadline);
  const planningValid = planning.stepsCompleted <= planning.stepsTotal && (
    planning.status === "not_requested"
      ? planning.deadline === "none" && planning.planRevision === 0
        && planning.stepsTotal === 0 && planning.stepsCompleted === 0
      : planning.status === "waiting"
        ? deadlineIsCanonical && planning.opportunityRevision > 0 && planning.planRevision === 0
          && planning.stepsTotal === 0 && planning.stepsCompleted === 0
        : planning.status === "ready"
          ? planning.deadline === "none" && planning.opportunityRevision > 0
            && planning.planRevision === planning.opportunityRevision
            && planning.stepsTotal >= 1 && planning.stepsCompleted < planning.stepsTotal
          : planning.status === "expired"
            ? planning.deadline === "none" && planning.opportunityRevision > 0 && planning.planRevision === 0
              && planning.stepsTotal === 0 && planning.stepsCompleted === 0
            : planning.deadline === "none" && planning.planRevision > 0
              && planning.planRevision === planning.opportunityRevision
              && planning.stepsTotal >= 1 && planning.stepsCompleted === planning.stepsTotal
  );
  const preparationValid = preparation.status === "none"
    ? preparation.forTurn === 0 && preparation.summary === "none"
    : preparation.forTurn === turnsUsed + 1 && (
      preparation.status === "deferred"
        ? preparation.summary === `${specialistName(role)} will not contribute this turn.`
        : preparation.summary !== "none" && preparation.summary !== `${specialistName(role)} will not contribute this turn.`
          && planning.status === "ready"
    );
  const sourceIsShared = (source: "records" | "conservation") => candidates.every((candidate) => (
    candidate.observedEvidence.some((evidence) => evidence.sourceId === source)
  ));
  const absenceIsCanonical = mira.presence !== "absent" || (
    mira.location === "none" && mira.mode === "unavailable"
    && task.status === "none" && task.revision === 0 && task.kind === "none"
    && task.powerAllowance === 0 && task.powerSpent === 0
    && planning.status === "not_requested" && planning.opportunityRevision === 0
    && preparation.status === "none"
    && knowledge.records === "unknown" && knowledge.conservation === "unknown"
    && knowledge.verifierResult === null
    && mira.fieldAssay.stepsCompleted === 0 && mira.fieldAssay.result === null
    && lastContribution.turn === 0 && lastContribution.kind === "none"
  );
  return absenceIsCanonical
    && (mira.presence !== "active" || mira.location !== "none")
    && (mira.presence === "active" ? mira.mode !== "unavailable" : mira.mode === "unavailable")
    && (taskHasAssignment || (task.kind === "none" && task.powerAllowance === 0 && task.powerSpent === 0))
    && (!taskHasAssignment || task.kind !== "none")
    && (task.status !== "none" || task.revision === 0)
    && (task.status === "assigned") === (mira.mode === "tasked")
    && (planning.status === "waiting" || planning.status === "ready" || planning.status === "expired"
      ? task.status === "assigned"
      : true)
    && (task.status !== "complete" || planning.status === "complete")
    && (preparation.status === "none" || task.status === "assigned")
    && task.powerSpent <= task.powerAllowance
    && task.powerSpent <= 3 - power
    && planningValid
    && preparationValid
    && lastContribution.turn <= turnsUsed
    && (lastContribution.kind !== "none" || lastContribution.turn === 0)
    && (knowledge.records !== "shared" || sourceIsShared("records"))
    && (knowledge.conservation !== "shared" || sourceIsShared("conservation"))
    && (knowledge.verifierResult === null || (
      mira.presence !== "absent"
      && verifierResult?.candidateId === knowledge.verifierResult.candidateId
    ))
    && (role !== "jonah" || (mira.fieldAssay.stepsCompleted === 0 && mira.fieldAssay.result === null))
    && (mira.fieldAssay.stepsCompleted !== 1 || (
      lastContribution.kind === "collect_assay_sample" && lastContribution.turn === turnsUsed
    ))
    && (mira.fieldAssay.result === null
      ? mira.fieldAssay.stepsCompleted < 2
      : mira.fieldAssay.stepsCompleted === 2 && candidates.some((candidate) => (
        candidate.candidateId === mira.fieldAssay.result?.candidateId
        && candidate.evidenceAssessment === "recommended"
      )));
}

const ROLE_ORDER: readonly ArchiveRole[] = ["lead", "mira", "jonah"];
const COMPANION_ORDER: readonly CompanionRole[] = ["mira", "jonah"];

function readTurnResolution(value: unknown): ArchiveTurnResolution | null {
  const source = exactRecord(value, [
    "conflicts", "deferred_roles", "power_reserved", "prepared_roles", "reservations", "status", "unprepared_roles",
  ]);
  if (source === null || (source.status !== "clear" && source.status !== "conflict")
    || integerInRange(source.power_reserved, 0, 5) === null
    || !Array.isArray(source.reservations) || source.reservations.length > 3
    || !Array.isArray(source.conflicts) || source.conflicts.length > 5) return null;
  const preparedRoles = readOrderedRoles(source.prepared_roles, COMPANION_ORDER);
  const deferredRoles = readOrderedRoles(source.deferred_roles, COMPANION_ORDER);
  const unpreparedRoles = readOrderedRoles(source.unprepared_roles, COMPANION_ORDER);
  if (preparedRoles === null || deferredRoles === null || unpreparedRoles === null) return null;
  const reservations: ArchiveTurnResolution["reservations"][number][] = [];
  const reservationRoles = new Set<ArchiveRole>();
  for (const value of source.reservations) {
    const item = exactRecord(value, ["interaction", "power", "role"]);
    if (item === null || !isArchiveRole(item.role) || reservationRoles.has(item.role)
      || integerInRange(item.power, 0, 2) === null
      || (item.interaction !== "none" && item.interaction !== "service_hatch" && item.interaction !== "catalog_verifier")) return null;
    reservationRoles.add(item.role);
    reservations.push({ role: item.role, power: item.power as 0 | 1 | 2, interaction: item.interaction });
  }
  if (!isCanonicalRoleOrder(reservations.map((item) => item.role), ROLE_ORDER)) return null;
  const conflicts: ArchiveTurnResolution["conflicts"][number][] = [];
  const seenConflicts = new Set<string>();
  for (const value of source.conflicts) {
    const item = exactRecord(value, ["code", "roles"]);
    const roles = item === null ? null : readOrderedRoles(item.roles, ROLE_ORDER);
    const conflictKey = item === null || roles === null ? "invalid" : `${String(item.code)}:${roles.join(",")}`;
    if (item === null || roles === null || roles.length === 0 || seenConflicts.has(conflictKey)
      || (item.code !== "shared_power" && item.code !== "service_hatch"
        && item.code !== "catalog_verifier" && item.code !== "ineligible_contribution")) return null;
    seenConflicts.add(conflictKey);
    conflicts.push({ code: item.code, roles });
  }
  if ((source.status === "clear") !== (conflicts.length === 0)) return null;
  return {
    status: source.status,
    powerReserved: source.power_reserved as number,
    preparedRoles,
    deferredRoles,
    unpreparedRoles,
    reservations,
    conflicts,
  };
}

function readExtraction(value: unknown): ArchiveExtractionPreview | null {
  const source = exactRecord(value, ["extracted_roles", "for_turn", "left_behind_roles", "revision", "status"]);
  if (source === null || (source.status !== "none" && source.status !== "prepared" && source.status !== "acknowledged")
    || integerInRange(source.revision, 0, 65_535) === null || integerInRange(source.for_turn, 0, 16) === null) return null;
  const extractedRoles = readOrderedRoles(source.extracted_roles, ROLE_ORDER);
  const leftBehindRoles = readOrderedRoles(source.left_behind_roles, COMPANION_ORDER);
  return extractedRoles === null || leftBehindRoles === null ? null : {
    status: source.status,
    revision: source.revision as number,
    forTurn: source.for_turn as number,
    extractedRoles,
    leftBehindRoles,
  };
}

function readCrewDebrief(value: unknown): ArchiveCrewDebrief | null {
  const source = exactRecord(value, ["completed_work", "extracted_roles", "left_behind_roles", "starting_roles"]);
  if (source === null || !Array.isArray(source.completed_work) || source.completed_work.length > 32) return null;
  const startingRoles = readOrderedRoles(source.starting_roles, ROLE_ORDER);
  const extractedRoles = readOrderedRoles(source.extracted_roles, ROLE_ORDER);
  const leftBehindRoles = readOrderedRoles(source.left_behind_roles, ROLE_ORDER);
  if (startingRoles === null || extractedRoles === null || leftBehindRoles === null || startingRoles[0] !== "lead") return null;
  const completedWork: ArchiveCrewDebrief["completedWork"][number][] = [];
  const seen = new Set<string>();
  for (const value of source.completed_work) {
    const item = exactRecord(value, ["kind", "role", "turn"]);
    if (item === null || !isCompanionRole(item.role) || !isMiraContributionKind(item.kind)
      || item.kind === "none" || integerInRange(item.turn, 1, 16) === null
      || (item.role === "jonah" && (item.kind === "collect_assay_sample" || item.kind === "complete_field_assay"))) return null;
    const key = `${item.role}:${item.turn}`;
    if (seen.has(key)) return null;
    seen.add(key);
    completedWork.push({ role: item.role, kind: item.kind, turn: item.turn as number });
  }
  if (completedWork.some((item, index) => index > 0 && (
    completedWork[index - 1]!.turn > item.turn
    || (completedWork[index - 1]!.turn === item.turn
      && COMPANION_ORDER.indexOf(completedWork[index - 1]!.role) >= COMPANION_ORDER.indexOf(item.role))
  ))) return null;
  return { startingRoles, extractedRoles, leftBehindRoles, completedWork };
}

function crewStateIsConsistent(
  mira: SpecialistCrewState,
  jonah: SpecialistCrewState,
  turn: ArchiveTurnResolution,
  extraction: ArchiveExtractionPreview,
  debrief: ArchiveCrewDebrief,
  phase: ArchivePhase,
  turnsUsed: number,
  power: number,
  staged: ArchiveStagedAction | null,
  operationCosts: MidnightArchiveProjection["operationCosts"],
): boolean {
  const companions = { mira, jonah } as const;
  const starting = new Set(debrief.startingRoles);
  if (COMPANION_ORDER.some((role) => (companions[role].presence === "absent") === starting.has(role))) return false;
  if (debrief.completedWork.some((item) => !starting.has(item.role) || item.turn > turnsUsed)) return false;

  const expectedPrepared = phase === "complete" ? [] : COMPANION_ORDER.filter((role) => companions[role].presence === "active"
    && companions[role].mode === "tasked" && companions[role].preparation.status === "prepared");
  const expectedDeferred = phase === "complete" ? [] : COMPANION_ORDER.filter((role) => companions[role].presence === "active"
    && companions[role].mode === "tasked" && companions[role].preparation.status === "deferred");
  const expectedUnprepared = phase === "complete" ? [] : COMPANION_ORDER.filter((role) => companions[role].presence === "active"
    && companions[role].mode === "tasked" && companions[role].preparation.status === "none");
  if (!sameArray(turn.preparedRoles, expectedPrepared) || !sameArray(turn.deferredRoles, expectedDeferred)
    || !sameArray(turn.unpreparedRoles, expectedUnprepared)) return false;

  const ineligible = new Set(turn.conflicts.filter((item) => item.code === "ineligible_contribution").flatMap((item) => item.roles));
  if (turn.conflicts.some((item) => item.code === "ineligible_contribution"
    && (item.roles.length !== 1 || !isCompanionRole(item.roles[0]) || !expectedPrepared.includes(item.roles[0])))) return false;
  for (const code of ["shared_power", "service_hatch", "catalog_verifier"] as const) {
    if (turn.conflicts.filter((item) => item.code === code).length > 1) return false;
  }
  const expectedReservations: ArchiveTurnResolution["reservations"][number][] = [];
  if (staged !== null) expectedReservations.push({
    role: "lead",
    power: staged.powerCost,
    interaction: interactionForAction(staged.actionType),
  });
  for (const role of expectedPrepared) {
    if (ineligible.has(role)) continue;
    const reservation = turn.reservations.find((item) => item.role === role);
    if (reservation === undefined || !reservationFitsTask(
      reservation, companions[role], role, operationCosts,
    )) return false;
    expectedReservations.push(reservation);
  }
  if (JSON.stringify(turn.reservations) !== JSON.stringify(expectedReservations)
    || turn.powerReserved !== turn.reservations.reduce((sum, item) => sum + item.power, 0)) return false;
  const conflictsBy = (code: ArchiveTurnResolution["conflicts"][number]["code"]) => turn.conflicts.filter((item) => item.code === code);
  const poweredRoles = turn.reservations.filter((item) => item.power > 0).map((item) => item.role);
  if ((turn.powerReserved > power) !== (conflictsBy("shared_power").length === 1)
    || (turn.powerReserved > power && !sameArray(conflictsBy("shared_power")[0]!.roles, poweredRoles))) return false;
  for (const interaction of ["service_hatch", "catalog_verifier"] as const) {
    const roles = turn.reservations.filter((item) => item.interaction === interaction).map((item) => item.role);
    const conflict = conflictsBy(interaction);
    if ((roles.length > 1) !== (conflict.length === 1) || (roles.length > 1 && !sameArray(conflict[0]!.roles, roles))) return false;
  }

  const extracted = extraction.extractedRoles;
  const left = extraction.leftBehindRoles;
  if (extraction.status === "none") {
    if (extraction.forTurn !== 0 || extracted.length !== 0 || left.length !== 0) return false;
  } else {
    if (extraction.revision < 1 || turn.status !== "clear") return false;
    if (extraction.forTurn !== (phase === "complete" ? turnsUsed : turnsUsed + 1)) return false;
    if (!sameArray([...extracted, ...left].sort(roleComparator), [...debrief.startingRoles].sort(roleComparator))
      || !extracted.includes("lead") || (phase === "active" && (staged?.actionType !== "stage_extract"))) return false;
  }
  if (phase === "complete") {
    if (!sameArray(debrief.extractedRoles, extracted)
      || !sameArray([...debrief.extractedRoles, ...debrief.leftBehindRoles].sort(roleComparator), debrief.startingRoles)) return false;
  } else if (debrief.extractedRoles.length !== 0 || debrief.leftBehindRoles.length !== 0) return false;
  return true;
}

function terminalOutcomeIsConsistent(
  kind: ArchiveOutcomeKind,
  carried: ArchiveCandidateId | null,
  extraction: ArchiveExtractionPreview,
  debrief: ArchiveCrewDebrief,
): boolean {
  if (kind === "exhausted_inside") {
    return extraction.status === "none" && debrief.extractedRoles.length === 0
      && sameArray(debrief.leftBehindRoles, debrief.startingRoles);
  }
  if (extraction.status !== "acknowledged") return false;
  if (kind === "no_ledger") return carried === null;
  if (carried === null) return false;
  if (kind === "success") return debrief.leftBehindRoles.length === 0;
  if (kind === "partial_extraction") return debrief.leftBehindRoles.length > 0;
  return kind === "wrong_ledger";
}

function reservationFitsTask(
  reservation: ArchiveTurnResolution["reservations"][number],
  specialist: SpecialistCrewState,
  role: CompanionRole,
  operationCosts: MidnightArchiveProjection["operationCosts"],
): boolean {
  const remainingAllowance = specialist.task.powerAllowance - specialist.task.powerSpent;
  if (reservation.interaction === "service_hatch") {
    const cost = role === "mira"
      ? operationCosts.mira_open_service_hatch.power
      : operationCosts.jonah_open_service_hatch.power;
    return specialist.task.kind === "open_service_hatch" && reservation.power === cost && cost <= remainingAllowance;
  }
  if (reservation.interaction === "catalog_verifier") {
    const cost = operationCosts.companion_verifier.power;
    return specialist.task.kind === "investigate_records"
      && reservation.power === cost && remainingAllowance >= cost;
  }
  return reservation.power === 0;
}

function interactionForAction(action: ArchiveStagedAction["actionType"]): ArchiveTurnResolution["reservations"][number]["interaction"] {
  return action === "stage_open_service_hatch" ? "service_hatch"
    : action === "stage_use_verifier" ? "catalog_verifier" : "none";
}

function readOrderedRoles<T extends ArchiveRole>(value: unknown, order: readonly T[]): T[] | null {
  if (!Array.isArray(value) || value.some((item) => !order.includes(item as T))) return null;
  const result = value as T[];
  return new Set(result).size === result.length && isCanonicalRoleOrder(result, order) ? result : null;
}

function isCanonicalRoleOrder<T extends ArchiveRole>(roles: readonly T[], order: readonly T[]): boolean {
  return roles.every((role, index) => index === 0 || order.indexOf(roles[index - 1]!) < order.indexOf(role));
}

function roleComparator(left: ArchiveRole, right: ArchiveRole): number {
  return ROLE_ORDER.indexOf(left) - ROLE_ORDER.indexOf(right);
}

function sameArray(left: readonly unknown[], right: readonly unknown[]): boolean {
  return left.length === right.length && left.every((item, index) => item === right[index]);
}

function readPreservationAgreement(value: unknown): PreservationAgreement | null {
  const source = exactRecord(value, ["commitment", "conditions", "speaker", "statement"]);
  if (
    source === null
    || source.speaker !== "Archivist"
    || source.statement !== "Preserve the threatened collection and I will open the Conservation–Vault gate."
    || !isAgreementCommitment(source.commitment)
    || !Array.isArray(source.conditions)
    || source.conditions.length !== 3
  ) return null;
  const expected = [
    {
      conditionId: "lead_acceptance",
      label: "Human lead accepts this fixed agreement",
      turnCost: 1,
      powerCost: 0,
      statuses: ["pending", "complete"],
    },
    {
      conditionId: "collection_preparation",
      label: "Prepare the threatened collection",
      turnCost: 1,
      powerCost: 0,
      statuses: ["pending", "complete"],
    },
    {
      conditionId: "equipment_energized",
      label: "Energize the preservation equipment",
      turnCost: 1,
      powerCost: 1,
      statuses: ["blocked", "pending", "complete"],
    },
  ] as const;
  const conditions: PreservationAgreementCondition[] = [];
  for (let index = 0; index < expected.length; index += 1) {
    const definition = expected[index]!;
    const item = exactRecord(source.conditions[index], [
      "condition_id", "label", "power_cost", "status", "turn_cost",
    ]);
    if (
      item === null
      || item.condition_id !== definition.conditionId
      || item.label !== definition.label
      || item.turn_cost !== definition.turnCost
      || item.power_cost !== definition.powerCost
      || !definition.statuses.includes(item.status as never)
    ) return null;
    conditions.push({
      conditionId: definition.conditionId,
      label: definition.label,
      status: item.status as PreservationAgreementCondition["status"],
      turnCost: definition.turnCost,
      powerCost: definition.powerCost,
    });
  }
  return {
    speaker: "Archivist",
    statement: "Preserve the threatened collection and I will open the Conservation–Vault gate.",
    commitment: source.commitment,
    conditions,
  };
}

function readOptionalObjectives(value: unknown): ArchiveOptionalObjectives | null {
  const source = exactRecord(value, ["collection_preserved", "source_record_protected"]);
  if (source === null) return null;
  const collection = exactRecord(source.collection_preserved, ["label", "power_cost", "status", "turn_cost"]);
  const sourceRecord = exactRecord(source.source_record_protected, ["label", "power_cost", "status", "turn_cost"]);
  if (
    collection === null
    || collection.label !== "Preserve the threatened collection"
    || collection.turn_cost !== 2
    || collection.power_cost !== 1
    || (collection.status !== "not_started" && collection.status !== "prepared" && collection.status !== "complete")
    || sourceRecord === null
    || sourceRecord.label !== "Protect the source's identifying record"
    || sourceRecord.turn_cost !== 1
    || sourceRecord.power_cost !== 1
    || (sourceRecord.status !== "locked" && sourceRecord.status !== "available" && sourceRecord.status !== "complete")
  ) return null;
  return {
    collectionPreserved: {
      label: "Preserve the threatened collection",
      status: collection.status,
      turnCost: 2,
      powerCost: 1,
    },
    sourceRecordProtected: {
      label: "Protect the source's identifying record",
      status: sourceRecord.status,
      turnCost: 1,
      powerCost: 1,
    },
  };
}

function readStagedAction(
  value: unknown,
  operationCosts: MidnightArchiveProjection["operationCosts"],
): ArchiveStagedAction | null | undefined {
  if (value === null) return null;
  if (!isRecord(value) || !isMidnightArchiveActionType(value.action_type) || !value.action_type.startsWith("stage_")) {
    return undefined;
  }
  const cost = actionCostFromOperations(operationCosts, value.action_type);
  if (value.turn_cost !== cost.turns || value.power_cost !== cost.power) return undefined;
  if (value.action_type === "stage_move") {
    if (!hasExactKeys(value, ["action_type", "destination", "power_cost", "turn_cost"])) return undefined;
    const destination = readLocation(value.destination);
    return destination === null ? undefined : {
      actionType: value.action_type,
      destination,
      turnCost: cost.turns,
      powerCost: cost.power,
    } as ArchiveStagedAction;
  }
  if (value.action_type === "stage_recover_candidate") {
    if (!hasExactKeys(value, ["action_type", "candidate_id", "power_cost", "turn_cost"])) return undefined;
    const id = candidateId(value.candidate_id);
    return id === undefined ? undefined : {
      actionType: value.action_type,
      candidateId: id,
      turnCost: cost.turns,
      powerCost: cost.power,
    } as ArchiveStagedAction;
  }
  if (!hasExactKeys(value, ["action_type", "power_cost", "turn_cost"])) return undefined;
  return STAGED_ACTION_OPERATIONS[value.action_type] === undefined
    ? undefined
    : { actionType: value.action_type as ArchiveStagedAction["actionType"], turnCost: cost.turns, powerCost: cost.power } as ArchiveStagedAction;
}

function readVerifierResult(
  value: unknown,
): MidnightArchiveProjection["verifierResult"] | undefined {
  if (value === null) return null;
  const source = exactRecord(value, ["candidate_id", "confidence"]);
  if (source === null || source.confidence !== "verified") return undefined;
  const id = candidateId(source.candidate_id);
  return id === undefined ? undefined : { candidateId: id, confidence: "verified" };
}

function readOutcome(value: unknown): MidnightArchiveProjection["outcome"] | undefined {
  if (value === null) return null;
  const source = exactRecord(value, ["kind"]);
  if (source === null || !isOutcomeKind(source.kind)) return undefined;
  return { kind: source.kind };
}

function readDebrief(value: unknown): MidnightArchiveProjection["debrief"] | undefined {
  if (value === null) return null;
  const source = exactRecord(value, [
    "agreement_commitment", "evidence_status", "message", "optional_objectives",
  ]);
  if (
    source === null
    || (source.evidence_status !== "none" && source.evidence_status !== "partial" && source.evidence_status !== "complete")
    || !isAgreementCommitment(source.agreement_commitment)
  ) return undefined;
  const expectedMessage = source.evidence_status === "none"
    ? "No authored source was inspected."
    : source.evidence_status === "partial"
      ? "Only one authored source was inspected; it did not uniquely identify a candidate."
      : "Both authored sources were inspected and their intersection informed the recommendation.";
  const objectives = exactRecord(source.optional_objectives, ["collection_preserved", "source_record_protected"]);
  return source.message !== expectedMessage || objectives === null
    || typeof objectives.collection_preserved !== "boolean"
    || typeof objectives.source_record_protected !== "boolean"
    ? undefined
    : {
      evidenceStatus: source.evidence_status,
      message: expectedMessage,
      agreementCommitment: source.agreement_commitment,
      optionalObjectives: {
        collectionPreserved: objectives.collection_preserved,
        sourceRecordProtected: objectives.source_record_protected,
      },
    };
}

function stagedActionFitsProjection(
  staged: ArchiveStagedAction | null,
  location: ArchiveLocation,
  power: number,
  gates: Readonly<Record<ArchiveGate, GateState>>,
  map: MidnightArchiveProjection["map"],
  agreement: PreservationAgreement,
  optionalObjectives: ArchiveOptionalObjectives,
  verifierResult: MidnightArchiveProjection["verifierResult"],
  carriedCandidate: ArchiveCandidateId | null,
  operationCosts: MidnightArchiveProjection["operationCosts"],
): boolean {
  if (staged === null) return true;
  const expectedCost = actionCostFromOperations(operationCosts, staged.actionType);
  if (staged.turnCost !== expectedCost.turns || staged.powerCost !== expectedCost.power) return false;
  if (staged.powerCost > power) return false;
  switch (staged.actionType) {
    case "stage_move": {
      const connection = map.connections.find((item) => (
        (item.from === location && item.to === staged.destination)
        || (item.to === location && item.from === staged.destination)
      ));
      return connection !== undefined
        && (connection.gate === null || gates[connection.gate] === "open");
    }
    case "stage_inspect_records": return location === "records";
    case "stage_inspect_conservation": return location === "conservation";
    case "stage_use_verifier": return location === "records" && verifierResult === null;
    case "stage_accept_preservation_agreement":
      return location === "conservation" && agreement.commitment === "not_accepted";
    case "stage_prepare_collection":
      return location === "conservation" && optionalObjectives.collectionPreserved.status === "not_started";
    case "stage_energize_preservation_equipment":
      return location === "conservation" && optionalObjectives.collectionPreserved.status === "prepared";
    case "stage_open_service_hatch": return location === "plant" && gates.service_hatch === "closed";
    case "stage_recover_candidate": return location === "vault";
    case "stage_protect_source_record":
      return location === "plant"
        && carriedCandidate !== null
        && optionalObjectives.sourceRecordProtected.status === "available";
    case "stage_extract": return location === "atrium";
    case "stage_wait": return true;
  }
}

function agreementStateIsConsistent(
  agreement: PreservationAgreement,
  objectives: ArchiveOptionalObjectives,
  gates: Readonly<Record<ArchiveGate, GateState>>,
): boolean {
  const [acceptance, preparation, equipment] = agreement.conditions;
  if (acceptance === undefined || preparation === undefined || equipment === undefined) return false;
  const acceptanceComplete = acceptance.status === "complete";
  const collectionStatus = objectives.collectionPreserved.status;
  const expectedPreparation = collectionStatus === "not_started" ? "pending" : "complete";
  const expectedEquipment = collectionStatus === "not_started"
    ? "blocked"
    : collectionStatus === "prepared" ? "pending" : "complete";
  const expectedCommitment: AgreementCommitment = !acceptanceComplete
    ? "not_accepted"
    : collectionStatus === "complete" ? "honored" : "accepted";
  return preparation.status === expectedPreparation
    && equipment.status === expectedEquipment
    && agreement.commitment === expectedCommitment
    && (gates.archive_gate === "open") === (expectedCommitment === "honored");
}

function readPhase(value: unknown): ArchivePhase | null {
  return value === "briefing" || value === "active" || value === "complete" ? value : null;
}

function readLocation(value: unknown): ArchiveLocation | null {
  return typeof value === "string" && (ARCHIVE_LOCATIONS as readonly string[]).includes(value)
    ? value as ArchiveLocation
    : null;
}

function readGate(value: unknown): ArchiveGate | null | undefined {
  return value === "archive_gate" || value === "service_hatch" ? value : undefined;
}

function readGateState(value: unknown): GateState | null {
  return value === "closed" || value === "open" ? value : null;
}

function isOutcomeKind(value: unknown): value is ArchiveOutcomeKind {
  return value === "success" || value === "partial_extraction" || value === "wrong_ledger"
    || value === "no_ledger" || value === "exhausted_inside";
}

function isAgreementCommitment(value: unknown): value is AgreementCommitment {
  return value === "not_accepted" || value === "accepted" || value === "honored";
}

function isMiraTaskStatus(value: unknown): value is MiraTaskStatus {
  return value === "none" || value === "assigned" || value === "complete" || value === "cancelled";
}

function isArchiveRole(value: unknown): value is ArchiveRole {
  return value === "lead" || value === "mira" || value === "jonah";
}

function isCompanionRole(value: unknown): value is CompanionRole {
  return value === "mira" || value === "jonah";
}

function isSpecialistTaskKind(value: unknown): value is SpecialistTaskKind {
  return value === "none" || value === "investigate_records" || value === "investigate_conservation"
    || value === "open_service_hatch" || value === "field_assay";
}

function isMiraPlanningStatus(value: unknown): value is MiraPlanningStatus {
  return value === "not_requested" || value === "waiting" || value === "ready"
    || value === "expired" || value === "complete";
}

function isMiraPreparationStatus(value: unknown): value is MiraPreparationStatus {
  return value === "none" || value === "prepared" || value === "deferred";
}

function isMiraKnowledgeStatus(value: unknown): value is MiraKnowledgeStatus {
  return value === "unknown" || value === "private" || value === "shared";
}

function isMiraContributionKind(value: unknown): value is MiraContributionKind {
  return value === "none" || value === "move" || value === "inspect_source"
    || value === "share_source" || value === "use_verifier" || value === "follow_move"
    || value === "regroup_move" || value === "open_service_hatch"
    || value === "collect_assay_sample" || value === "complete_field_assay";
}

function candidateId(value: unknown): ArchiveCandidateId | undefined {
  return value === "ledger-amber" || value === "ledger-cobalt" || value === "ledger-violet"
    ? value
    : undefined;
}

function boundedText(value: unknown, maximumBytes: number): string | null {
  return typeof value === "string" && value.length > 0
    && new TextEncoder().encode(value).length <= maximumBytes
    ? value
    : null;
}

function byteLength(value: string): number {
  return new TextEncoder().encode(value).length;
}

function isCanonicalUtcDeadline(value: string): boolean {
  const match = UTC_CANONICAL_DEADLINE.exec(value);
  if (match === null) return false;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const hour = Number(match[4]);
  const minute = Number(match[5]);
  const second = Number(match[6]);
  const fraction = match[7];
  return year >= 1 && month >= 1 && month <= 12 && day >= 1 && day <= daysInMonth(year, month)
    && hour <= 23 && minute <= 59 && second <= 59
    && (fraction === undefined || !fraction.endsWith("0"));
}

function daysInMonth(year: number, month: number): number {
  if (month === 2) return year % 400 === 0 || (year % 4 === 0 && year % 100 !== 0) ? 29 : 28;
  return month === 4 || month === 6 || month === 9 || month === 11 ? 30 : 31;
}

function integerInRange(value: unknown, minimum: number, maximum: number): number | null {
  return Number.isSafeInteger(value) && Number(value) >= minimum && Number(value) <= maximum
    ? Number(value)
    : null;
}

function exactRecord(
  value: unknown,
  keys: readonly string[],
): Record<string, unknown> | null {
  return isRecord(value) && hasExactKeys(value, keys) ? value : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

function connectionKey(a: ArchiveLocation, b: ArchiveLocation): string {
  return [a, b].sort().join("|");
}

function readCompanionDialogue(value: unknown): MidnightArchiveProjection["companionDialogue"] | null {
  if (!Array.isArray(value) || value.length > 4) return null;
  const result: { speaker: CompanionRole; turn: number; text: string }[] = [];
  for (const raw of value) {
    const item = exactRecord(raw, ["speaker", "turn", "text"]);
    if (item === null || !isCompanionRole(item.speaker) || typeof item.text !== "string" || item.text === ""
      || /[\u0000-\u001f\uD800-\uDFFF]/u.test(item.text) || byteLength(item.text) > 160
      || byteLength(JSON.stringify(JSON.stringify(item.text))) > 192 || integerInRange(item.turn, 0, 16) === null) return null;
    result.push({ speaker: item.speaker, turn: item.turn as number, text: item.text });
  }
  return result;
}
