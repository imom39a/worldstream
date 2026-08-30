export type ProjectionLens = "public" | "navigator" | "operator";
export type FixtureRecordKind = "Projection Reset" | "Observation Frame" | "Attention Signal";
export type FixtureRecordTone = "blue" | "green" | "amber" | "violet";

export interface AgentHeistFixtureRecord {
  readonly id: string;
  readonly fixtureIndex: number;
  readonly roomSequence: string;
  readonly semanticTime: string;
  readonly kind: FixtureRecordKind;
  readonly title: string;
  readonly detail: string;
  readonly audiences: readonly ProjectionLens[];
  readonly tone: FixtureRecordTone;
}

export const agentHeistRecords: readonly AgentHeistFixtureRecord[] = [
  {
    id: "baseline",
    fixtureIndex: 0,
    roomSequence: "0000",
    semanticTime: "09:00:00.000",
    kind: "Projection Reset",
    title: "Authorized baseline established",
    detail: "Briefing · three enabled seats · Activity Pack Revision pinned",
    audiences: ["public", "navigator", "operator"],
    tone: "blue",
  },
  {
    id: "briefing-open",
    fixtureIndex: 1,
    roomSequence: "0001",
    semanticTime: "09:00:30.000",
    kind: "Observation Frame",
    title: "Negotiation phase opened",
    detail: "The phase deadline Transition committed at its Scheduled Time.",
    audiences: ["public", "navigator", "operator"],
    tone: "green",
  },
  {
    id: "seat-summary",
    fixtureIndex: 2,
    roomSequence: "0002",
    semanticTime: "09:00:31.400",
    kind: "Observation Frame",
    title: "Seat state updated",
    detail: "Three seats are enabled. No plan exists.",
    audiences: ["public", "navigator", "operator"],
    tone: "blue",
  },
  {
    id: "navigator-clue",
    fixtureIndex: 3,
    roomSequence: "0003",
    semanticTime: "09:00:32.150",
    kind: "Observation Frame",
    title: "Private clue inspected",
    detail: "Route clue value: service. This value is authorized for the Navigator Membership.",
    audiences: ["navigator"],
    tone: "amber",
  },
  {
    id: "plan-proposed",
    fixtureIndex: 4,
    roomSequence: "0004",
    semanticTime: "09:00:35.020",
    kind: "Observation Frame",
    title: "Plan proposed",
    detail: "Plan A · service · early · thermal_key · boat",
    audiences: ["public", "navigator", "operator"],
    tone: "green",
  },
  {
    id: "endorsement-attention",
    fixtureIndex: 5,
    roomSequence: "0004",
    semanticTime: "09:00:35.020",
    kind: "Attention Signal",
    title: "Endorsement response requested",
    detail: "Reason: endorsement_requested · target payload omitted",
    audiences: ["navigator", "operator"],
    tone: "violet",
  },
  {
    id: "commitment-open",
    fixtureIndex: 6,
    roomSequence: "0005",
    semanticTime: "09:02:00.000",
    kind: "Observation Frame",
    title: "Commitment phase opened",
    detail: "The commitment deadline is 09:02:30.000. Sealed selections are accepted before this time.",
    audiences: ["public", "navigator", "operator"],
    tone: "amber",
  },
  {
    id: "commitment-attention",
    fixtureIndex: 7,
    roomSequence: "0005",
    semanticTime: "09:02:00.000",
    kind: "Attention Signal",
    title: "Commitment response requested",
    detail: "Reason: commitment_opened · an Agent Participant may need to act",
    audiences: ["navigator", "operator"],
    tone: "violet",
  },
  {
    id: "commitment-count",
    fixtureIndex: 8,
    roomSequence: "0006",
    semanticTime: "09:02:04.320",
    kind: "Observation Frame",
    title: "Sealed commitment count changed",
    detail: "One of three commitments is recorded. The selection and contributor identity remain sealed.",
    audiences: ["public", "navigator", "operator"],
    tone: "blue",
  },
  {
    id: "navigator-catch-up",
    fixtureIndex: 9,
    roomSequence: "0007",
    semanticTime: "09:02:08.090",
    kind: "Projection Reset",
    title: "Navigator view re-established",
    detail: "A complete authorized Projection established a new Observation Stream baseline.",
    audiences: ["navigator"],
    tone: "amber",
  },
  {
    id: "resolution-open",
    fixtureIndex: 10,
    roomSequence: "0008",
    semanticTime: "09:02:13.440",
    kind: "Observation Frame",
    title: "Resolution phase opened",
    detail: "Three commitments are recorded. Old Commitment timers are fenced.",
    audiences: ["public", "navigator", "operator"],
    tone: "green",
  },
  {
    id: "result",
    fixtureIndex: 11,
    roomSequence: "0009",
    semanticTime: "09:02:13.441",
    kind: "Observation Frame",
    title: "Result available",
    detail: "Outcome: success · score 5 of 5 · strict majority selected Plan A",
    audiences: ["public", "navigator", "operator"],
    tone: "green",
  },
] as const;

export function visibleFixtureRecords(
  records: readonly AgentHeistFixtureRecord[],
  lens: ProjectionLens,
  revealedThroughIndex: number,
): readonly AgentHeistFixtureRecord[] {
  return records.filter(
    (record) => record.fixtureIndex <= revealedThroughIndex && record.audiences.includes(lens),
  );
}
