import { agentHeistEvidence } from "./buildIdentity";

export { agentHeistEvidence };

export type ProjectionLens = "public" | "navigator" | "operator";
export type FixtureRecordKind =
  | "Attention summary"
  | "Projection example"
  | "Replay evidence"
  | "Transition summary";
export type FixtureRecordTone = "blue" | "green" | "amber" | "violet";
export type FixturePhase = "briefing" | "negotiation" | "commitment" | "resolution" | "result" | "complete";

export interface FixtureStep {
  readonly value: number;
}

export interface RoomSequence {
  readonly value: string;
}

export interface SemanticTime {
  readonly value: string;
}

export interface AgentHeistFixtureRecord {
  readonly id: string;
  readonly fixtureStep: FixtureStep;
  readonly roomSequence: RoomSequence | null;
  readonly semanticTime: SemanticTime | null;
  readonly kind: FixtureRecordKind;
  readonly phase: FixturePhase;
  readonly title: string;
  readonly detail: string;
  readonly audiences: readonly ProjectionLens[];
  readonly tone: FixtureRecordTone;
}

function fixtureStep(value: number): FixtureStep {
  if (!Number.isSafeInteger(value) || value < 0) throw new Error("fixture step must be a non-negative integer");
  return Object.freeze({ value });
}

function roomSequence(value: number): RoomSequence {
  if (!Number.isSafeInteger(value) || value < 1) throw new Error("Room sequence must be a positive integer");
  return Object.freeze({ value: String(value).padStart(4, "0") });
}

function semanticTime(value: string): SemanticTime {
  if (!/^\d{2}:\d{2}:\d{2}\.\d{3,6}$/.test(value)) throw new Error("Semantic Time has an invalid format");
  return Object.freeze({ value });
}

export const agentHeistRecords: readonly AgentHeistFixtureRecord[] = [
  {
    id: "baseline",
    fixtureStep: fixtureStep(0),
    roomSequence: null,
    semanticTime: null,
    kind: "Projection example",
    phase: "briefing",
    title: "Illustrative baseline",
    detail: "Three enabled participant seats exist in the recorded Agent Heist story.",
    audiences: ["public", "navigator", "operator"],
    tone: "blue",
  },
  {
    id: "negotiation-open",
    fixtureStep: fixtureStep(1),
    roomSequence: roomSequence(1),
    semanticTime: semanticTime("12:00:30.000"),
    kind: "Transition summary",
    phase: "negotiation",
    title: "Negotiation phase opened",
    detail: "The briefing deadline committed Transition 1 in the retained story.",
    audiences: ["public", "navigator", "operator"],
    tone: "green",
  },
  {
    id: "navigator-clue",
    fixtureStep: fixtureStep(2),
    roomSequence: roomSequence(2),
    semanticTime: semanticTime("12:00:31.000"),
    kind: "Transition summary",
    phase: "negotiation",
    title: "Navigator inspected a private clue",
    detail: "Route clue value: service. This value is scoped to the Navigator Projection example.",
    audiences: ["navigator"],
    tone: "amber",
  },
  {
    id: "projection-boundary",
    fixtureStep: fixtureStep(3),
    roomSequence: null,
    semanticTime: null,
    kind: "Projection example",
    phase: "negotiation",
    title: "Other views omit the private clue",
    detail: "The Public and Operator examples do not render the Navigator clue value.",
    audiences: ["public", "operator"],
    tone: "blue",
  },
  {
    id: "public-claim",
    fixtureStep: fixtureStep(4),
    roomSequence: roomSequence(4),
    semanticTime: semanticTime("12:00:33.000"),
    kind: "Transition summary",
    phase: "negotiation",
    title: "A public clue claim was recorded",
    detail: "The recorded claim code is route_service.",
    audiences: ["public", "navigator", "operator"],
    tone: "blue",
  },
  {
    id: "plan-proposed",
    fixtureStep: fixtureStep(5),
    roomSequence: roomSequence(5),
    semanticTime: semanticTime("12:00:34.000"),
    kind: "Transition summary",
    phase: "negotiation",
    title: "A plan was proposed",
    detail: "Plan A uses service, early, thermal_key, and boat.",
    audiences: ["public", "navigator", "operator"],
    tone: "green",
  },
  {
    id: "endorsement-attention",
    fixtureStep: fixtureStep(6),
    roomSequence: null,
    semanticTime: semanticTime("12:00:34.000"),
    kind: "Attention summary",
    phase: "negotiation",
    title: "An endorsement response was requested",
    detail: "The recorded reason is endorsement_requested. The target payload is omitted.",
    audiences: ["operator"],
    tone: "violet",
  },
  {
    id: "commitment-open",
    fixtureStep: fixtureStep(7),
    roomSequence: roomSequence(7),
    semanticTime: semanticTime("12:02:00.000"),
    kind: "Transition summary",
    phase: "commitment",
    title: "Commitment phase opened",
    detail: "The negotiation deadline committed Transition 7 in the retained story.",
    audiences: ["public", "navigator", "operator"],
    tone: "amber",
  },
  {
    id: "commitment-attention",
    fixtureStep: fixtureStep(8),
    roomSequence: null,
    semanticTime: semanticTime("12:02:00.000"),
    kind: "Attention summary",
    phase: "commitment",
    title: "A commitment response was requested",
    detail: "The recorded reason is commitment_opened. Claiming an Activation Intent does not grant Action authority.",
    audiences: ["operator"],
    tone: "violet",
  },
  {
    id: "first-commitment",
    fixtureStep: fixtureStep(9),
    roomSequence: roomSequence(8),
    semanticTime: semanticTime("12:02:03.000"),
    kind: "Transition summary",
    phase: "commitment",
    title: "The commitment count changed",
    detail: "One of three commitments is recorded. The selected plan and contributor identity remain sealed.",
    audiences: ["public", "navigator", "operator"],
    tone: "blue",
  },
  {
    id: "second-commitment",
    fixtureStep: fixtureStep(10),
    roomSequence: roomSequence(9),
    semanticTime: semanticTime("12:02:04.000"),
    kind: "Transition summary",
    phase: "commitment",
    title: "A second commitment was recorded",
    detail: "Two of three commitments are recorded. The Broker remains missing.",
    audiences: ["public", "navigator", "operator"],
    tone: "blue",
  },
  {
    id: "result",
    fixtureStep: fixtureStep(11),
    roomSequence: roomSequence(12),
    semanticTime: semanticTime("12:02:30.000001"),
    kind: "Transition summary",
    phase: "result",
    title: "The recorded Result became available",
    detail: "Outcome: success. Score: 5 of 5. The selected plan received a strict 2-of-3 majority.",
    audiences: ["public", "navigator", "operator"],
    tone: "green",
  },
  {
    id: "replay-evidence",
    fixtureStep: fixtureStep(12),
    roomSequence: null,
    semanticTime: null,
    kind: "Replay evidence",
    phase: "complete",
    title: "The retained story records verified Replay",
    detail: `${agentHeistEvidence.verifiedTransitionCount} Transitions were verified by read-only Replay. The browser does not run Replay.`,
    audiences: ["public", "navigator", "operator"],
    tone: "green",
  },
] as const;

export function visibleFixtureRecords(
  records: readonly AgentHeistFixtureRecord[],
  lens: ProjectionLens,
  revealedThroughStep: number,
): readonly AgentHeistFixtureRecord[] {
  return records.filter(
    (record) => record.fixtureStep.value <= revealedThroughStep && record.audiences.includes(lens),
  );
}
