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
  | "wrong_ledger"
  | "no_ledger"
  | "exhausted_inside";

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

export interface MidnightArchiveProjection {
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
  readonly debrief: null | {
    readonly evidenceStatus: "none" | "partial" | "complete";
    readonly message: string;
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
  | { readonly action: "stage_open_service_hatch" }
  | { readonly action: "stage_recover_candidate"; readonly candidate_id: ArchiveCandidateId }
  | { readonly action: "stage_extract" }
  | { readonly action: "stage_wait" }
  | { readonly action: "commit_turn" };

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
  "candidates",
  "debrief",
  "carried_candidate",
  "gates",
  "location",
  "map",
  "objective",
  "outcome",
  "phase",
  "power",
  "staged_action",
  "turns_remaining",
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

export const TEN_TURN_TECHNICAL_ROUTE: readonly MidnightArchiveActionIntent[] = [
  { action: "stage_move", destination: "records" },
  { action: "stage_use_verifier" },
  { action: "stage_move", destination: "plant" },
  { action: "stage_open_service_hatch" },
  { action: "stage_move", destination: "vault" },
  { action: "stage_recover_candidate", candidate_id: "ledger-violet" },
  { action: "stage_move", destination: "plant" },
  { action: "stage_move", destination: "records" },
  { action: "stage_move", destination: "atrium" },
  { action: "stage_extract" },
];

/** Reads the complete authorized participant Projection and rejects any drift. */
export function readMidnightArchiveProjection(
  value: unknown,
): MidnightArchiveProjection | null {
  const source = exactRecord(value, ROOT_KEYS);
  if (source === null) return null;

  const phase = readPhase(source.phase);
  const objective = boundedText(source.objective, MAX_OBJECTIVE_BYTES);
  const location = readLocation(source.location);
  const turnsUsed = integerInRange(source.turns_used, 0, 16);
  const turnsRemaining = integerInRange(source.turns_remaining, 0, 16);
  const power = integerInRange(source.power, 0, 3);
  const gates = readGates(source.gates);
  const map = readMap(source.map);
  const candidates = readCandidates(source.candidates);
  const debrief = readDebrief(source.debrief);
  const stagedAction = readStagedAction(source.staged_action);
  const carriedCandidate = source.carried_candidate === null
    ? null
    : candidateId(source.carried_candidate);
  const verifierResult = readVerifierResult(source.verifier_result);
  const outcome = readOutcome(source.outcome);

  if (
    phase === null || objective === null || location === null
    || turnsUsed === null || turnsRemaining === null || power === null
    || gates === null || map === null || candidates === null || debrief === undefined
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
    || !stagedActionFitsProjection(stagedAction, location, power, gates, map)
    || (phase === "briefing" && (turnsUsed !== 0 || stagedAction !== null || outcome !== null))
    || (phase === "active" && (outcome !== null || debrief !== null))
    || (phase === "complete" && (outcome === null || debrief === null))
    || (phase === "complete" && debrief?.evidenceStatus !== expectedDebriefStatus)
    || (outcome?.kind === "exhausted_inside" && turnsRemaining !== 0)
  ) return null;

  return {
    phase,
    objective,
    location,
    turnsUsed,
    turnsRemaining,
    power,
    gates,
    map,
    candidates,
    debrief,
    stagedAction,
    carriedCandidate,
    verifierResult,
    outcome,
  };
}

export function actionCost(action: MidnightArchiveActionType): ArchiveActionCost {
  switch (action) {
    case "stage_use_verifier": return { turns: 1, power: 1 };
    case "stage_open_service_hatch": return { turns: 1, power: 2 };
    case "commit_turn": return { turns: 0, power: 0 };
    default: return { turns: 1, power: 0 };
  }
}

export function actionPayload(intent: MidnightArchiveActionIntent): JsonValue {
  switch (intent.action) {
    case "stage_move":
      return { destination: intent.destination };
    case "stage_recover_candidate":
      return { candidate_id: intent.candidate_id };
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

function readStagedAction(value: unknown): ArchiveStagedAction | null | undefined {
  if (value === null) return null;
  if (!isRecord(value) || !isMidnightArchiveActionType(value.action_type) || value.action_type === "commit_turn") {
    return undefined;
  }
  const cost = actionCost(value.action_type);
  if (value.turn_cost !== cost.turns || value.power_cost !== cost.power) return undefined;
  if (value.action_type === "stage_move") {
    if (!hasExactKeys(value, ["action_type", "destination", "power_cost", "turn_cost"])) return undefined;
    const destination = readLocation(value.destination);
    return destination === null ? undefined : {
      actionType: value.action_type,
      destination,
      turnCost: 1,
      powerCost: 0,
    };
  }
  if (value.action_type === "stage_recover_candidate") {
    if (!hasExactKeys(value, ["action_type", "candidate_id", "power_cost", "turn_cost"])) return undefined;
    const id = candidateId(value.candidate_id);
    return id === undefined ? undefined : {
      actionType: value.action_type,
      candidateId: id,
      turnCost: 1,
      powerCost: 0,
    };
  }
  if (!hasExactKeys(value, ["action_type", "power_cost", "turn_cost"])) return undefined;
  if (value.action_type === "stage_use_verifier") {
    return { actionType: value.action_type, turnCost: 1, powerCost: 1 };
  }
  if (value.action_type === "stage_open_service_hatch") {
    return { actionType: value.action_type, turnCost: 1, powerCost: 2 };
  }
  if (value.action_type === "stage_inspect_records" || value.action_type === "stage_inspect_conservation") {
    return { actionType: value.action_type, turnCost: 1, powerCost: 0 };
  }
  return { actionType: value.action_type, turnCost: 1, powerCost: 0 };
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
  const source = exactRecord(value, ["evidence_status", "message"]);
  if (source === null || (source.evidence_status !== "none" && source.evidence_status !== "partial" && source.evidence_status !== "complete")) return undefined;
  const message = boundedText(source.message, MAX_OBJECTIVE_BYTES);
  return message === null ? undefined : { evidenceStatus: source.evidence_status, message };
}

function stagedActionFitsProjection(
  staged: ArchiveStagedAction | null,
  location: ArchiveLocation,
  power: number,
  gates: Readonly<Record<ArchiveGate, GateState>>,
  map: MidnightArchiveProjection["map"],
): boolean {
  if (staged === null) return true;
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
    case "stage_use_verifier": return location === "records";
    case "stage_open_service_hatch": return location === "plant";
    case "stage_recover_candidate": return location === "vault";
    case "stage_extract": return location === "atrium";
    case "stage_wait": return true;
  }
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
  return value === "success" || value === "wrong_ledger"
    || value === "no_ledger" || value === "exhausted_inside";
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
