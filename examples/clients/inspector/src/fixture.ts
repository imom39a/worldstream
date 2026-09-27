import parityFixture from "../../../heist/parity_fixture.json";

export type HeistPhase =
  | "Briefing"
  | "Negotiation"
  | "Commitment"
  | "Resolution"
  | "Result"
  | "Complete";

export type ViewId = "public" | "participant" | "operator" | "replay";

export type HeistActionType =
  | "inspect_clue"
  | "publish_clue"
  | "offer_exchange"
  | "accept_exchange"
  | "propose_plan"
  | "endorse_plan"
  | "challenge_plan"
  | "commit_move"
  | "acknowledge_result";

export type DiscoveryStatus = "Fixture only" | "Live" | "Unavailable";
export type ConsoleTransportState = "Unavailable" | "Connecting" | "Attached" | "Live" | "Disconnected";
export type RuntimeRecoveryState = "Loading" | "CatchingUp" | "Active" | "Passivating" | "Inactive";
export type RoomHealthState = "Healthy" | "Faulted" | "Quarantined";
export type FrameDeliveryState = "Unavailable" | "Awaiting attach" | "Catching up" | "Reset required" | "Live";

export type DeliveryState =
  | "exact-head"
  | "stale-head"
  | "resync-required"
  | "catching-up"
  | "faulted"
  | "quarantined";

export interface RolePresence {
  role: "Navigator" | "Insider" | "Broker";
  principalKind: "Human" | "Agent";
  presence: "Present" | "Awaiting";
  activation: "Ready" | "Idle" | "Attention pending";
}

export interface MembershipSummary {
  role: RolePresence["role"];
  principalKind: RolePresence["principalKind"];
  accessMode: "Participant" | "Spectator" | "Operator";
  standing: "Enabled" | "Suspended" | "Departed";
  session: "Attached" | "Disconnected" | "Unavailable";
  memberRef: string;
}

export interface PublicClue {
  id: string;
  label: string;
  claim: string;
  state: "Published" | "Challenged";
}

export interface PublicPlan {
  id: string;
  label: string;
  endorsements: number;
  challenges: number;
  status: "Leading" | "Under review";
}

export interface PublicChallenge {
  id: string;
  target: string;
  reason: string;
  state: "Open" | "Resolved";
}

export interface AggregateResult {
  availability: "Available" | "Pending";
  selectedPlan: string;
  voteSummary: string;
  checksPassed: number;
  checksTotal: number;
  scoreLabel: string;
  outcome: string;
}

export interface ActionOffer {
  id: string;
  label: string;
  actionType: HeistActionType;
  schemaDigest: string;
  eligibility: string;
}

export interface PrivateClue {
  clueId: string;
  claimCode: string;
}

export interface AttentionSignal {
  id: string;
  reason: string;
  target: string;
  priority: 1 | 2 | 3;
  deadline: string;
  actionTypes: HeistActionType[];
  deduplicationKey: string;
  status: "Pending" | "Acknowledged" | "Unavailable";
}

export interface RuntimeStatus {
  transport: ConsoleTransportState;
  recovery: RuntimeRecoveryState;
  roomHealth: RoomHealthState;
  frame: {
    delivery: FrameDeliveryState;
    cursor: number | null;
    head: number | null;
    reset: "Unavailable" | "Required" | "Installed";
    syncAck: "Unavailable" | "Pending" | "Sent";
    observationAck: "Unavailable" | "Pending" | "Sent";
  };
  reason: string;
}

export interface OperatorDiagnostics {
  membership: {
    access: "Operator";
    standing: "Enabled" | "Suspended" | "Departed";
    memberRef: string;
    role: "None";
  };
  session: {
    status: "Attached" | "Disconnected";
    sessionRef: string;
    cursor: string;
    lastHeartbeat: string;
  };
  runner: {
    availability: "Available" | "Unavailable";
    runnerRef: string;
    activation: "Idle" | "Claimed" | "Attention pending";
    invocation: string;
  };
  timer: {
    phaseDeadline: string;
    timerGeneration: string;
    nextTransition: string;
  };
  frame: {
    delivery: "Live" | "Catching up" | "Reset required";
    headSequence: number;
    cursorSequence: number;
    retainedRange: string;
  };
  integrity: {
    state: "Healthy" | "Faulted" | "Quarantined";
    generation: number;
    completeHead: string;
    coreHash: string;
    activityHash: string;
    aggregateHash: string;
    safeReason: string;
  };
}

export interface HeistFixture {
  fixtureLabel: string;
  roomLabel: string;
  phase: HeistPhase;
  phaseGeneration: number;
  phaseDeadline: string | null;
  phaseDescription: string;
  deadline: string;
  roomSequence: number;
  roles: RolePresence[];
  publicClues: PublicClue[];
  publicPlans: PublicPlan[];
  publicChallenges: PublicChallenge[];
  commitmentCount: { submitted: number; total: number };
  result: AggregateResult;
  discovery: {
    roomRef: string;
    status: DiscoveryStatus;
    protocol: "0.1";
    activityPack: string;
    availableViews: ViewId[];
  };
  memberships: MembershipSummary[];
  participant: {
    fixtureGate: string;
    exactHead: string;
    offers: ActionOffer[];
    privateClues: PrivateClue[];
  };
  operator: OperatorDiagnostics;
  attention: AttentionSignal[];
  runtime: RuntimeStatus;
  replay: {
    availableThrough: string;
    currentView: "Public projection";
    verification: "Verified" | "Pending" | "Unavailable while quarantined";
    presentAuthorization: "Authorized" | "Unavailable";
    historicalAuthorization: "Public projection authorized" | "Unavailable";
    checkpoints: ReplayCheckpoint[];
    hashes: ReplayHashes;
  };
  finalRevealAuthorized: boolean;
  parity: {
    transcriptDigest: string;
    phasePath: string[];
    replayTransitionCount: number;
    replayVerified: boolean;
  };
}

export interface ReplayHashes {
  coreStateHash: string;
  activityStateHash: string;
  aggregateStateHash: string;
  lineageHash: string;
  transitionHashes: string[];
}

export interface ReplayCheckpoint {
  sequence: number;
  phase: HeistPhase;
  summary: string;
}

export const heistParity = parityFixture;

export function assertHeistFixtureParity(fixture: HeistFixture): void {
  if (fixture.parity.transcriptDigest !== heistParity.transcript_digest) {
    throw new Error("Heist fixture transcript digest drifted from the offline story");
  }
  if (JSON.stringify(fixture.parity.phasePath) !== JSON.stringify(heistParity.phase_path)) {
    throw new Error("Heist fixture phase path drifted from the offline story");
  }
  if (fixture.parity.replayTransitionCount !== heistParity.replay.verified_transition_count) {
    throw new Error("Heist fixture replay count drifted from the offline story");
  }
  if (!fixture.parity.replayVerified || !heistParity.replay.verified) {
    throw new Error("Heist fixture replay must remain verified");
  }
  if (fixture.discovery.status === "Fixture only") {
    const expectedHashes = heistParity.retained_executor.replay_hashes;
    const actualHashes = fixture.replay.hashes;
    if (
      actualHashes.coreStateHash !== expectedHashes.core_state_hash ||
      actualHashes.activityStateHash !== expectedHashes.activity_state_hash ||
      actualHashes.aggregateStateHash !== expectedHashes.authoritative_state_hash ||
      actualHashes.lineageHash !== expectedHashes.lineage_hash ||
      JSON.stringify(actualHashes.transitionHashes) !== JSON.stringify(expectedHashes.transition_hashes)
    ) {
      throw new Error("Heist Replay component or Transition hashes drifted from the retained corpus");
    }
  }
  if (
    fixture.result.checksPassed !== 5 ||
    fixture.result.checksTotal !== 5 ||
    fixture.result.outcome !== "Success"
  ) {
    throw new Error("Heist fixture result drifted from the absent-Broker state machine");
  }
  const privacy = heistParity.ui_privacy_contract;
  if (
    !privacy.participant_is_fixture_gated ||
    !privacy.operator_is_redacted ||
    !privacy.replay_is_read_only ||
    !privacy.final_reveal_requires_complete
  ) {
    throw new Error("Heist fixture privacy contract is incomplete");
  }
  if (fixture.finalRevealAuthorized && fixture.phase !== "Complete") {
    throw new Error("Final reveal authorization cannot exist before Complete");
  }
}

const publicRoles: RolePresence[] = [
  { role: "Navigator", principalKind: "Human", presence: "Present", activation: "Ready" },
  { role: "Insider", principalKind: "Agent", presence: "Present", activation: "Idle" },
  { role: "Broker", principalKind: "Agent", presence: "Awaiting", activation: "Attention pending" },
];

const publicClues: PublicClue[] = [
  {
    id: "clue-01",
    label: "Route claim",
    claim: "The canal route remains viable under the current shift.",
    state: "Published",
  },
  {
    id: "clue-02",
    label: "Window claim",
    claim: "The service window is the currently published entry constraint.",
    state: "Published",
  },
  {
    id: "clue-03",
    label: "Tool claim",
    claim: "A required tool is still under public review.",
    state: "Challenged",
  },
];

const publicPlans: PublicPlan[] = [
  {
    id: "plan-a",
    label: "Canal lift / service window",
    endorsements: 2,
    challenges: 0,
    status: "Leading",
  },
  {
    id: "plan-b",
    label: "Roof signal / late window",
    endorsements: 1,
    challenges: 1,
    status: "Under review",
  },
];

const publicChallenges: PublicChallenge[] = [
  {
    id: "challenge-01",
    target: "Plan B",
    reason: "Entry window does not match the published claim.",
    state: "Open",
  },
  {
    id: "challenge-02",
    target: "Tool claim",
    reason: "Broker confirmation is still pending.",
    state: "Open",
  },
];

const participantOffers: ActionOffer[] = [
  {
    id: "offer-01",
    label: "Propose a structured plan",
    actionType: "propose_plan",
    schemaDigest: "schema:plan/···a81c",
    eligibility: "Negotiation phase",
  },
  {
    id: "offer-02",
    label: "Endorse a published plan",
    actionType: "endorse_plan",
    schemaDigest: "schema:endorse/···4c22",
    eligibility: "Negotiation phase",
  },
  {
    id: "offer-03",
    label: "Challenge a public claim",
    actionType: "challenge_plan",
    schemaDigest: "schema:challenge/···9b0e",
    eligibility: "Negotiation phase",
  },
];

const memberships: MembershipSummary[] = [
  {
    role: "Navigator",
    principalKind: "Human",
    accessMode: "Participant",
    standing: "Enabled",
    session: "Unavailable",
    memberRef: "member:navigator/···9a10",
  },
  {
    role: "Insider",
    principalKind: "Agent",
    accessMode: "Participant",
    standing: "Enabled",
    session: "Unavailable",
    memberRef: "member:insider/···14b2",
  },
  {
    role: "Broker",
    principalKind: "Agent",
    accessMode: "Participant",
    standing: "Enabled",
    session: "Unavailable",
    memberRef: "member:broker/···c03f",
  },
];

const attention: AttentionSignal[] = [
  {
    id: "attention-01",
    reason: "round_result_available",
    target: "Broker · member:···c03f",
    priority: 1,
    deadline: "00:12",
    actionTypes: ["acknowledge_result"],
    deduplicationKey: "round_result_available:broker:phase-06",
    status: "Unavailable",
  },
];

const runtime: RuntimeStatus = {
  transport: "Unavailable",
  recovery: "Inactive",
  roomHealth: "Healthy",
  frame: {
    delivery: "Unavailable",
    cursor: null,
    head: null,
    reset: "Unavailable",
    syncAck: "Unavailable",
    observationAck: "Unavailable",
  },
  reason: "No server session is configured. The fixture is readable, but live frames and participant submissions are unavailable.",
};

const operatorDiagnostics: OperatorDiagnostics = {
  membership: {
    access: "Operator",
    standing: "Enabled",
    memberRef: "member:operator/···2e7a",
    role: "None",
  },
  session: {
    status: "Attached",
    sessionRef: "session/···8bd1",
    cursor: "seq 24 · acknowledged",
    lastHeartbeat: "2026-08-20 14:04:12Z",
  },
  runner: {
    availability: "Available",
    runnerRef: "runner/···51af",
    activation: "Attention pending",
    invocation: "No active invocation",
  },
  timer: {
    phaseDeadline: "00:12 remaining",
    timerGeneration: "timer-gen-07",
    nextTransition: "Result deadline → Complete",
  },
  frame: {
    delivery: "Live",
    headSequence: 15,
    cursorSequence: 15,
    retainedRange: "seq 9–15",
  },
  integrity: {
    state: "Healthy",
    generation: 7,
    completeHead: "head:sha256:···7d91",
    coreHash: "core:sha256:···1a04",
    activityHash: "activity:sha256:···54be",
    aggregateHash: "aggregate:sha256:···c390",
    safeReason: "No incident recorded",
  },
};

const replayCheckpoints: ReplayCheckpoint[] = heistParity.phase_path.map((phase, sequence) => ({
  sequence,
  phase: phase as HeistPhase,
  summary:
    sequence === 0
      ? "Genesis establishes the public room projection."
      : phase === "Result"
        ? "Aggregate checks are public; individual commitments remain withheld."
        : phase === "Complete"
          ? "The Activity Phase is terminal; final reveal remains separately authorized."
          : `Public ${phase} projection at sequence ${sequence}.`,
}));

export const resultFixture: HeistFixture = {
  fixtureLabel: "Fixture-backed reference · no server connection",
  roomLabel: "Canal Shift / Room 07",
  phase: "Result",
  phaseGeneration: 5,
  phaseDeadline: "2026-08-15T12:02:50.000001Z",
  phaseDescription: "Aggregate checks are published; individual commitments remain withheld.",
  deadline: "00:12",
  roomSequence: 15,
  roles: publicRoles,
  publicClues,
  publicPlans,
  publicChallenges,
  commitmentCount: { submitted: 2, total: 3 },
  result: {
    availability: "Available",
    selectedPlan: "Canal lift / service window",
    voteSummary: "2 matching · 1 not counted",
    checksPassed: 5,
    checksTotal: 5,
    scoreLabel: "Success",
    outcome: "Success",
  },
  discovery: {
    roomRef: "room:canal-shift/···0707",
    status: "Fixture only",
    protocol: "0.1",
    activityPack: "Agent Heist / parity fixture",
    availableViews: ["public", "participant", "operator", "replay"],
  },
  memberships,
  participant: {
    fixtureGate: "Fixture-gated controls · no action is submitted",
    exactHead: "head:sha256:···0cae",
    offers: participantOffers,
    privateClues: [],
  },
  operator: operatorDiagnostics,
  attention,
  runtime,
  replay: {
    availableThrough: "Sequence 15",
    currentView: "Public projection",
    verification: "Verified",
    presentAuthorization: "Authorized",
    historicalAuthorization: "Public projection authorized",
    checkpoints: replayCheckpoints,
    hashes: {
      coreStateHash: heistParity.retained_executor.replay_hashes.core_state_hash,
      activityStateHash: heistParity.retained_executor.replay_hashes.activity_state_hash,
      aggregateStateHash: heistParity.retained_executor.replay_hashes.authoritative_state_hash,
      lineageHash: heistParity.retained_executor.replay_hashes.lineage_hash,
      transitionHashes: heistParity.retained_executor.replay_hashes.transition_hashes,
    },
  },
  finalRevealAuthorized: false,
  parity: {
    transcriptDigest: heistParity.transcript_digest,
    phasePath: heistParity.phase_path,
    replayTransitionCount: heistParity.replay.verified_transition_count,
    replayVerified: heistParity.replay.verified,
  },
};

export const completeFixture: HeistFixture = {
  ...resultFixture,
  fixtureLabel: "Fixture-backed terminal reference · no server connection",
  phase: "Complete",
  phaseGeneration: 6,
  phaseDeadline: null,
  phaseDescription: "The activity is terminal. Core Room status is separate from Activity Phase.",
  deadline: "—",
  roomSequence: 15,
  finalRevealAuthorized: true,
  replay: { ...resultFixture.replay, availableThrough: "Sequence 15" },
};
