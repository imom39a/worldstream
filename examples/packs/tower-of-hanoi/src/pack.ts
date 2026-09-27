import {
  canonicalStringify,
  defineActivityPack,
  type ActivityPackDefinition,
  type ActivityPackViewerClass,
  type CanonicalJson,
  type CanonicalObject,
  type PackReduceOutput,
} from "@worldstream/pack-sdk";

type Rod = "A" | "B" | "C";
type Move = { readonly from: Rod; readonly to: Rod; readonly disk: number };
type Board = Record<Rod, readonly number[]>;
type Assessment = "endorse" | "challenge" | "defer";
type LastMove = { readonly member_id: string; readonly move: Move; readonly round: number };
type CompletionClaim = {
  readonly claimant_member_id: string;
  readonly work_revision: number;
  readonly claim_round: number;
  /** Snapshot electorate prevents a later Membership edit from lowering the threshold. */
  readonly electorate_members: readonly string[];
  readonly quorum: number;
};
type HanoiState = {
  readonly phase: "solving" | "complete";
  readonly disks: number;
  readonly board: Board;
  /** Counts every accepted domain Action; it is not a puzzle-progress claim. */
  readonly round: number;
  /** Increments only when a legal disk move changes the shared work. */
  readonly work_revision: number;
  readonly contributions_by_member: Readonly<Record<string, number>>;
  readonly completion_claim: CompletionClaim | null;
  /** The latest assessment from each reviewer on the current claim. */
  readonly claim_assessments: Readonly<Record<string, Assessment>>;
  readonly last_move: LastMove | null;
  readonly move_limit: number;
  readonly outcome: { readonly moves: number; readonly status: "in_progress" | "participant_accepted_completion" };
  readonly member_notices: Readonly<Record<string, string>>;
};

const MOVE_ACTION = "move_disk";
const POST_CLAIM_ACTION = "post_completion_claim";
const ASSESS_CLAIM_ACTION = "assess_claim";
const MAX_DISKS = 10;
const MAX_MOVE_LIMIT = 10000;
const MAX_ROUND = 30000;
/** At most fifteen other Participants can be signalled in one bounded Apply. */
const MAX_PARTICIPANTS = 16;
const SOLVER_NOTICE = "Choose legal work or review a current completion claim from the latest Projection.";
const OBSERVER_NOTICE = "This Membership can observe the current board and claim review, but cannot submit Actions.";
/** A public objective for Participants; reducer completion deliberately does not enforce it. */
function participantObjective(): CanonicalObject {
  return { source_rod: "A", target_rod: "C", description: "Participants may aim to move the full tower from A to C." };
}

function rods(): readonly Rod[] { return ["A", "B", "C"]; }

class RuleRejection extends Error {
  constructor(readonly code: string, message: string) { super(message); }
}

function record(value: CanonicalJson | undefined, label: string): Record<string, CanonicalJson> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new TypeError(`${label} must be an object`);
  return value as Record<string, CanonicalJson>;
}
function stringValue(value: CanonicalJson | undefined, label: string): string {
  if (typeof value !== "string") throw new TypeError(`${label} must be a string`);
  return value;
}
function integerValue(value: CanonicalJson | undefined, label: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) throw new TypeError(`${label} must be a safe integer`);
  return value;
}
function rodValue(value: CanonicalJson | undefined, label: string): Rod {
  const rod = stringValue(value, label);
  if (rod !== "A" && rod !== "B" && rod !== "C") throw new RuleRejection("invalid_move", `${label} is not a declared rod`);
  return rod;
}
function moveValue(value: CanonicalJson | undefined, label: string): Move | null {
  if (value === null || value === undefined) return null;
  const move = record(value, label);
  return { from: rodValue(move.from, `${label}.from`), to: rodValue(move.to, `${label}.to`), disk: integerValue(move.disk, `${label}.disk`) };
}
function lastMoveValue(value: CanonicalJson | undefined, label: string): LastMove | null {
  if (value === null || value === undefined) return null;
  const lastMove = record(value, label);
  const move = moveValue(lastMove.move, `${label}.move`);
  if (move === null) throw new TypeError(`${label}.move must be present`);
  return { member_id: stringValue(lastMove.member_id, `${label}.member_id`), move, round: integerValue(lastMove.round, `${label}.round`) };
}
function claimValue(value: CanonicalJson | undefined, label: string): CompletionClaim | null {
  if (value === null || value === undefined) return null;
  const claim = record(value, label);
  const claimant_member_id = stringValue(claim.claimant_member_id, `${label}.claimant_member_id`);
  const electorate = claim.electorate_members;
  if (!Array.isArray(electorate) || electorate.length < 1 || electorate.length > MAX_PARTICIPANTS || electorate.some((member) => typeof member !== "string" || !member)) throw new TypeError(`${label}.electorate_members is invalid`);
  const electorate_members = [...electorate] as string[];
  if (new Set(electorate_members).size !== electorate_members.length || !electorate_members.includes(claimant_member_id)) throw new TypeError(`${label}.electorate_members must include the unique claimant`);
  const quorum = integerValue(claim.quorum, `${label}.quorum`);
  if (quorum !== Math.floor(electorate_members.length / 2) + 1) throw new TypeError(`${label}.quorum is not the electorate majority`);
  return {
    claimant_member_id,
    work_revision: integerValue(claim.work_revision, `${label}.work_revision`),
    claim_round: integerValue(claim.claim_round, `${label}.claim_round`),
    electorate_members,
    quorum,
  };
}
function assessmentsValue(value: CanonicalJson | undefined, label: string): Readonly<Record<string, Assessment>> {
  const assessments = record(value, label);
  return Object.fromEntries(Object.entries(assessments).map(([memberId, assessment]) => {
    if (assessment !== "endorse" && assessment !== "challenge" && assessment !== "defer") throw new TypeError(`${label}.${memberId} must be an assessment`);
    return [memberId, assessment];
  }));
}
function stateValue(value: CanonicalJson | undefined): HanoiState {
  const state = record(value, "Activity State");
  const boardValue = record(state.board, "Activity State board");
  const board = Object.fromEntries(rods().map((rod) => {
    const stack = boardValue[rod];
    if (!Array.isArray(stack) || stack.some((disk) => typeof disk !== "number" || !Number.isSafeInteger(disk))) throw new TypeError(`Activity State board.${rod} must be an integer array`);
    return [rod, [...stack] as number[]];
  })) as unknown as Board;
  const outcome = record(state.outcome, "Activity State outcome");
  const status = stringValue(outcome.status, "Activity State outcome.status");
  if (status !== "in_progress" && status !== "participant_accepted_completion") throw new TypeError("Activity State outcome.status is invalid");
  const phase = stringValue(state.phase, "Activity State phase");
  if (phase !== "solving" && phase !== "complete") throw new TypeError("Activity State phase is invalid");
  const contributions = record(state.contributions_by_member, "Activity State contributions_by_member");
  const notices = record(state.member_notices, "Activity State member_notices");
  return {
    phase,
    disks: integerValue(state.disks, "Activity State disks"),
    board,
    round: integerValue(state.round, "Activity State round"),
    work_revision: integerValue(state.work_revision, "Activity State work_revision"),
    contributions_by_member: Object.fromEntries(Object.entries(contributions).map(([memberId, count]) => [memberId, integerValue(count, `Activity State contributions_by_member.${memberId}`)])),
    completion_claim: claimValue(state.completion_claim, "Activity State completion_claim"),
    claim_assessments: assessmentsValue(state.claim_assessments, "Activity State claim_assessments"),
    last_move: lastMoveValue(state.last_move, "Activity State last_move"),
    move_limit: integerValue(state.move_limit, "Activity State move_limit"),
    outcome: { moves: integerValue(outcome.moves, "Activity State outcome.moves"), status },
    member_notices: Object.fromEntries(Object.entries(notices).map(([memberId, notice]) => [memberId, stringValue(notice, `Activity State member_notices.${memberId}`)])),
  };
}
function stateCanonical(state: HanoiState): CanonicalObject {
  return {
    board: { A: [...state.board.A], B: [...state.board.B], C: [...state.board.C] },
    disks: state.disks,
    round: state.round,
    work_revision: state.work_revision,
    contributions_by_member: { ...state.contributions_by_member },
    ...(state.completion_claim === null ? {} : { completion_claim: { ...state.completion_claim } }),
    claim_assessments: { ...state.claim_assessments },
    ...(state.last_move === null ? {} : { last_move: { member_id: state.last_move.member_id, move: { ...state.last_move.move }, round: state.last_move.round } }),
    move_limit: state.move_limit,
    outcome: { ...state.outcome },
    phase: state.phase,
    member_notices: { ...state.member_notices },
  };
}

function coreRecord(value: CanonicalJson | undefined): Record<string, CanonicalJson> { return record(value, "Core Room State"); }
function coreRole(core: Record<string, CanonicalJson>, memberId: string): string | null {
  const membershipValue = record(core.memberships, "Core memberships")[memberId];
  if (membershipValue === undefined) return null;
  const membership = record(membershipValue, "Core membership");
  if (membership.access_mode !== "participant" || membership.standing !== "enabled") return null;
  return typeof membership.role === "string" ? membership.role : null;
}
/** The electorate deliberately excludes observers, operators, and non-agent participants. */
function eligibleSolverMembers(core: Record<string, CanonicalJson>): string[] {
  const solverMembers: string[] = [];
  for (const [memberId, membershipValue] of Object.entries(record(core.memberships, "Core memberships"))) {
    const membership = record(membershipValue, "Core membership");
    if (membership.access_mode === "participant" && membership.standing === "enabled" && membership.principal_kind === "agent" && membership.role === "solver") solverMembers.push(memberId);
  }
  solverMembers.sort();
  if (solverMembers.length < 1 || solverMembers.length > MAX_PARTICIPANTS) throw new RuleRejection("core_role_invariant", "Tower of Hanoi requires one to sixteen enabled agent solver Memberships");
  return solverMembers;
}
function completionSummary(state: HanoiState, eligible: readonly string[]): CanonicalObject {
  const claim = state.completion_claim;
  const electorate = claim === null ? eligible : claim.electorate_members;
  const claimantEligible = claim !== null && electorate.includes(claim.claimant_member_id);
  const assessments = Object.fromEntries(electorate.flatMap((memberId) => {
    if (claim === null || memberId === claim.claimant_member_id) return [];
    const assessment = state.claim_assessments[memberId];
    return assessment === undefined ? [] : [[memberId, assessment] as const];
  }));
  const endorsementCount = Object.values(assessments).filter((assessment) => assessment === "endorse").length;
  const quorum = claim === null ? Math.floor(eligible.length / 2) + 1 : claim.quorum;
  return {
    claim_open: claim !== null,
    ...(claim === null ? {} : { claim: {
      claimant_member_id: claim.claimant_member_id,
      claim_round: claim.claim_round,
      work_revision: claim.work_revision,
      electorate_size: claim.electorate_members.length,
      quorum: claim.quorum,
    } }),
    assessments_by_member: assessments,
    endorsement_count: endorsementCount,
    approval_count: (claimantEligible ? 1 : 0) + endorsementCount,
    quorum,
  };
}
function isAccepted(state: HanoiState, eligible: readonly string[]): boolean {
  const summary = completionSummary(state, eligible);
  return integerValue(summary.approval_count, "completion approval_count") >= integerValue(summary.quorum, "completion quorum");
}
function initialState(disks: number, moveLimit: number, eligible: readonly string[], board: Board | null = null): HanoiState {
  return {
    phase: "solving", disks,
    board: board === null ? { A: Array.from({ length: disks }, (_, index) => disks - index), B: [], C: [] } : { A: [...board.A], B: [...board.B], C: [...board.C] },
    round: 1, work_revision: 0,
    contributions_by_member: Object.fromEntries(eligible.map((memberId) => [memberId, 0])),
    completion_claim: null, claim_assessments: {}, last_move: null,
    move_limit: moveLimit, outcome: { moves: 0, status: "in_progress" },
    member_notices: Object.fromEntries(eligible.map((memberId) => [memberId, SOLVER_NOTICE])),
  };
}
/** Accept an optional reviewed starting position so Participants can solve from mid-state. */
function initialBoardValue(value: CanonicalJson | undefined, disks: number): Board | null {
  if (value === null || value === undefined) return null;
  const boardValue = record(value, "configuration.initial_board");
  const seen: number[] = [];
  const board = Object.fromEntries(rods().map((rod) => {
    const stack = boardValue[rod];
    if (!Array.isArray(stack)) throw new TypeError(`configuration.initial_board.${rod} must be an array`);
    const stacked = stack.map((disk) => integerValue(disk, `configuration.initial_board.${rod}`));
    if (stacked.some((disk) => disk < 1 || disk > disks)) throw new TypeError("configuration.initial_board disks must be within configuration.disks");
    for (let index = 1; index < stacked.length; index += 1) {
      if (stacked[index - 1]! <= stacked[index]!) throw new TypeError("configuration.initial_board stacks must be strictly decreasing");
    }
    seen.push(...stacked);
    return [rod, stacked];
  })) as unknown as Board;
  const sorted = [...seen].sort((left, right) => left - right);
  if (sorted.length !== disks || sorted.some((disk, index) => disk !== index + 1)) throw new TypeError("configuration.initial_board must place each disk exactly once");
  return board;
}
function withKnownSolver(state: HanoiState, memberId: string): HanoiState {
  return Object.hasOwn(state.contributions_by_member, memberId) ? state : {
    ...state,
    contributions_by_member: { ...state.contributions_by_member, [memberId]: 0 },
    member_notices: { ...state.member_notices, [memberId]: SOLVER_NOTICE },
  };
}
function nextMoveState(state: HanoiState, move: Move, memberId: string): HanoiState {
  if (state.phase === "complete") throw new RuleRejection("activity_complete", "participant acceptance has already completed the activity");
  if (state.outcome.moves >= state.move_limit) throw new RuleRejection("move_limit", "the bounded move limit has been reached");
  if (move.from === move.to) throw new RuleRejection("invalid_move", "source and destination rods must differ");
  const source = [...state.board[move.from]], destination = [...state.board[move.to]];
  const top = source[source.length - 1];
  if (top === undefined) throw new RuleRejection("invalid_move", "the source rod is empty");
  if (top !== move.disk) throw new RuleRejection("wrong_disk", "the submitted disk is not on top of the source rod");
  const destinationTop = destination[destination.length - 1];
  if (destinationTop !== undefined && destinationTop < move.disk) throw new RuleRejection("blocked_disk", "a larger disk cannot be placed on a smaller disk");
  source.pop(); destination.push(move.disk);
  const nextRound = state.round + 1;
  return {
    ...state, phase: "solving", board: { ...state.board, [move.from]: source, [move.to]: destination }, round: nextRound,
    work_revision: state.work_revision + 1,
    contributions_by_member: { ...state.contributions_by_member, [memberId]: state.contributions_by_member[memberId]! + 1 },
    completion_claim: null, claim_assessments: {}, last_move: { member_id: memberId, move: { ...move }, round: nextRound },
    outcome: { moves: state.outcome.moves + 1, status: "in_progress" },
  };
}
function nextClaimState(state: HanoiState, memberId: string, workRevision: number, eligible: readonly string[]): HanoiState {
  if (state.phase === "complete") throw new RuleRejection("activity_complete", "participant acceptance has already completed the activity");
  if (workRevision !== state.work_revision) throw new RuleRejection("claim_superseded", "claim work_revision is no longer current");
  if (state.completion_claim !== null) throw new RuleRejection("claim_open", "an unchanged-work completion claim is already open");
  const nextRound = state.round + 1;
  const claimed: HanoiState = {
    ...state, round: nextRound,
    completion_claim: { claimant_member_id: memberId, work_revision: state.work_revision, claim_round: nextRound, electorate_members: [...eligible], quorum: Math.floor(eligible.length / 2) + 1 },
    claim_assessments: {},
  };
  const accepted = isAccepted(claimed, eligible);
  return { ...claimed, phase: accepted ? "complete" : "solving", outcome: { moves: state.outcome.moves, status: accepted ? "participant_accepted_completion" : "in_progress" } };
}
function nextAssessmentState(state: HanoiState, memberId: string, workRevision: number, claimRound: number, assessment: Assessment, eligible: readonly string[]): HanoiState {
  if (state.phase === "complete") throw new RuleRejection("activity_complete", "participant acceptance has already completed the activity");
  const claim = state.completion_claim;
  if (claim === null) throw new RuleRejection("claim_absent", "there is no current completion claim to assess");
  if (workRevision !== state.work_revision || claim.work_revision !== workRevision || claim.claim_round !== claimRound) throw new RuleRejection("claim_superseded", "claim identity is no longer current");
  if (!claim.electorate_members.includes(memberId)) throw new RuleRejection("not_claim_electorate", "the acting Membership was not eligible when this claim opened");
  if (claim.claimant_member_id === memberId) throw new RuleRejection("claimant_cannot_assess", "the claimant assertion is already counted and cannot be assessed by its author");
  const assessed: HanoiState = { ...state, round: state.round + 1, claim_assessments: { ...state.claim_assessments, [memberId]: assessment } };
  const accepted = isAccepted(assessed, eligible);
  return { ...assessed, phase: accepted ? "complete" : "solving", outcome: { moves: state.outcome.moves, status: accepted ? "participant_accepted_completion" : "in_progress" } };
}
function boardChangedAttention(state: HanoiState, core: Record<string, CanonicalJson>, actor: string): readonly CanonicalJson[] {
  if (state.phase === "complete") return [];
  return eligibleSolverMembers(core).map((memberId) => ({
    action_types: [MOVE_ACTION, POST_CLAIM_ACTION],
    deduplication_key: `board-changed:${state.work_revision}:${memberId}`,
    priority: 1,
    reason: "board_changed",
    target_member_id: memberId,
  }));
}
function claimReviewAttention(state: HanoiState, core: Record<string, CanonicalJson>): readonly CanonicalJson[] {
  const claim = state.completion_claim;
  if (claim === null || state.phase === "complete") return [];
  // Re-signal every non-endorsing electorate member, not only the never-assessed
  // ones, so a deferred or challenged review can still be revisited while the
  // claim is open. The deduplication key changes with the claim round and the
  // current round, so a fresh activation is issued after each decision.
  return eligibleSolverMembers(core).filter((memberId) => claim.electorate_members.includes(memberId) && memberId !== claim.claimant_member_id && state.claim_assessments[memberId] !== "endorse").map((memberId) => ({
    action_types: [ASSESS_CLAIM_ACTION],
    deduplication_key: `claim-review:${claim.work_revision}:${claim.claim_round}:${state.round}:${memberId}`,
    priority: 1,
    reason: "claim_review_requested",
    target_member_id: memberId,
  }));
}
function reject(error: RuleRejection): PackReduceOutput { return { activity_disposition_type: "reject", bounded_safe_details: { reason: error.message }, declared_code: error.code }; }

function publicFields(state: HanoiState, eligible: readonly string[]): CanonicalObject {
  return {
    board: { A: [...state.board.A], B: [...state.board.B], C: [...state.board.C] }, disks: state.disks,
    phase: state.phase, round: state.round, work_revision: state.work_revision,
    objective: participantObjective(),
    rules: { largest_disk_on_bottom: true, move_limit: state.move_limit, one_disk_per_action: true },
    outcome: { ...state.outcome }, contributions_by_member: { ...state.contributions_by_member },
    completion: completionSummary(state, eligible),
    ...(state.last_move === null ? {} : { last_move: { member_id: state.last_move.member_id, move: { ...state.last_move.move }, round: state.last_move.round } }),
  };
}
function participantProjection(state: HanoiState, memberId: string, eligible: readonly string[], role: "solver" | "observer", canMove: boolean): CanonicalObject {
  return { ...publicFields(state, eligible), can_move: canMove, private_member_id: memberId, private_role: role, member_notice: state.member_notices[memberId] ?? (role === "solver" ? SOLVER_NOTICE : OBSERVER_NOTICE) };
}
function viewerClass(core: Record<string, CanonicalJson>, viewer: Record<string, CanonicalJson>): ActivityPackViewerClass {
  const viewerType = stringValue(viewer.viewer_type, "viewer viewer_type");
  if (viewerType === "public" || viewerType === "operator" || viewerType === "final_reveal") return viewerType;
  const memberId = stringValue(viewer.member_id, "viewer member_id");
  if (viewerType === "participant") return "participant";
  if (viewerType !== "historical") throw new TypeError("viewer viewer_type is invalid");
  const membership = record(record(core.memberships, "Core memberships")[memberId], "Core membership");
  return membership.access_mode === "participant" ? "historical_participant" : membership.access_mode === "operator" ? "historical_operator" : "historical_public";
}
function authorizedView(state: HanoiState, core: Record<string, CanonicalJson>, viewer: Record<string, CanonicalJson>) {
  const schema = viewerClass(core, viewer);
  const memberId = schema === "participant" || schema === "historical_participant" ? stringValue(viewer.member_id, "viewer member_id") : null;
  const role = memberId === null ? null : coreRole(core, memberId);
  const solver = role === "solver";
  const participant = memberId !== null && role !== null;
  const eligible = eligibleSolverMembers(core);
  const activeSolver = schema === "participant" && solver && state.phase === "solving";
  const canMove = activeSolver && state.outcome.moves < state.move_limit;
  const claim = state.completion_claim;
  const offers = !activeSolver ? [] : [
    ...(canMove ? [MOVE_ACTION] : []),
    POST_CLAIM_ACTION,
    ...(claim !== null && claim.claimant_member_id !== memberId ? [ASSESS_CLAIM_ACTION] : []),
  ];
  const publicSchema = schema === "historical_participant" ? "historical_public" : schema === "historical_public" ? "historical_public" : schema === "historical_operator" ? "historical_operator" : schema === "operator" ? "operator" : schema === "final_reveal" ? "final_reveal" : "public";
  return {
    action_offers: offers,
    projection: participant && memberId !== null ? participantProjection(state, memberId, eligible, solver ? "solver" : "observer", canMove) : publicFields(state, eligible),
    projection_schema: participant ? schema : publicSchema,
  };
}

export function reduceTower(input: CanonicalObject): PackReduceOutput {
  const before = stateValue(input.prior_activity_state);
  const stimulus = record(input.recorded_stimulus, "recorded_stimulus");
  const coreBefore = coreRecord(input.core_before), coreAfter = coreRecord(input.proposed_core_after);
  eligibleSolverMembers(coreBefore); eligibleSolverMembers(coreAfter);
  try {
    const stimulusType = stringValue(stimulus.stimulus_type, "stimulus_type");
    if (stimulusType === "core_proposed") return { activity_disposition_type: "apply", next_activity_state: stateCanonical(before), ordered_domain_events: [], timer_requests: [], ordered_attention_signals: [] };
    if (stimulusType !== "participant_action") throw new RuleRejection("invalid_payload", "Tower of Hanoi accepts participant Actions only");
    const actionType = stringValue(stimulus.action_type, "action_type");
    if (actionType !== MOVE_ACTION && actionType !== POST_CLAIM_ACTION && actionType !== ASSESS_CLAIM_ACTION) throw new RuleRejection("invalid_payload", "unknown Action type");
    const memberId = stringValue(stimulus.member_id, "member_id");
    if (coreRole(coreBefore, memberId) !== "solver" || !eligibleSolverMembers(coreBefore).includes(memberId)) throw new RuleRejection("role_violation", "only enabled agent solver Memberships may act");
    const payload = record(stimulus.canonical_payload, "Action payload");
    const state = withKnownSolver(before, memberId);
    const eligible = eligibleSolverMembers(coreBefore);
    let next: HanoiState;
    let event: CanonicalObject;
    if (actionType === MOVE_ACTION) {
      if (Object.keys(payload).length !== 3) throw new RuleRejection("invalid_payload", "move payload has unexpected fields");
      const move = { from: rodValue(payload.from, "Action payload.from"), to: rodValue(payload.to, "Action payload.to"), disk: integerValue(payload.disk, "Action payload.disk") };
      next = nextMoveState(state, move, memberId);
      event = { event_type: "disk_moved", member_id: memberId, move, round: next.round, work_revision: next.work_revision, claim_superseded: state.completion_claim !== null };
    } else if (actionType === POST_CLAIM_ACTION) {
      if (Object.keys(payload).length !== 1) throw new RuleRejection("invalid_payload", "claim payload has unexpected fields");
      next = nextClaimState(state, memberId, integerValue(payload.work_revision, "Action payload.work_revision"), eligible);
      const completion = completionSummary(next, eligible);
      event = { event_type: "completion_claim_posted", member_id: memberId, work_revision: next.work_revision, claim_round: next.round, approval_count: completion.approval_count!, quorum: completion.quorum!, outcome: next.outcome.status };
    } else {
      if (Object.keys(payload).length !== 3 || (payload.assessment !== "endorse" && payload.assessment !== "challenge" && payload.assessment !== "defer")) throw new RuleRejection("invalid_payload", "claim assessment must be endorse, challenge, or defer");
      const assessment = payload.assessment;
      next = nextAssessmentState(state, memberId, integerValue(payload.work_revision, "Action payload.work_revision"), integerValue(payload.claim_round, "Action payload.claim_round"), assessment, eligible);
      const completion = completionSummary(next, eligible);
      event = { event_type: "completion_claim_assessed", member_id: memberId, assessment, work_revision: next.work_revision, approval_count: completion.approval_count!, quorum: completion.quorum!, outcome: next.outcome.status };
    }
    return { activity_disposition_type: "apply", next_activity_state: stateCanonical(next), ordered_domain_events: [event], timer_requests: [], ordered_attention_signals: actionType === MOVE_ACTION ? boardChangedAttention(next, coreAfter, memberId) : claimReviewAttention(next, coreAfter) };
  } catch (error) {
    if (error instanceof RuleRejection) return reject(error);
    throw error;
  }
}

function moveSchema(): CanonicalObject { return { additionalProperties: false, properties: { disk: { type: "integer", minimum: 1, maximum: MAX_DISKS }, from: { enum: ["A", "B", "C"] }, to: { enum: ["A", "B", "C"] } }, required: ["from", "to", "disk"], type: "object" }; }
function workRevisionSchema(): CanonicalObject { return { type: "integer", minimum: 0, maximum: MAX_MOVE_LIMIT }; }
function postClaimSchema(): CanonicalObject { return { additionalProperties: false, properties: { work_revision: workRevisionSchema() }, required: ["work_revision"], type: "object" }; }
function assessmentSchema(): CanonicalObject { return { additionalProperties: false, properties: { assessment: { enum: ["endorse", "challenge", "defer"] }, claim_round: { minimum: 1, maximum: MAX_ROUND, type: "integer" }, work_revision: workRevisionSchema() }, required: ["assessment", "claim_round", "work_revision"], type: "object" }; }
function boardSchema(): CanonicalObject { const rod = { items: { maximum: MAX_DISKS, minimum: 1, type: "integer" }, maxItems: MAX_DISKS, type: "array" }; return { additionalProperties: false, properties: { A: rod, B: rod, C: rod }, required: ["A", "B", "C"], type: "object" }; }
function lastMoveSchema(): CanonicalObject { return { additionalProperties: false, properties: { member_id: { maxLength: 256, minLength: 1, type: "string" }, move: moveSchema(), round: { maximum: MAX_ROUND, minimum: 1, type: "integer" } }, required: ["member_id", "move", "round"], type: "object" }; }
function claimSchema(): CanonicalObject { return { additionalProperties: false, properties: { claimant_member_id: { maxLength: 256, minLength: 1, type: "string" }, claim_round: { maximum: MAX_ROUND, minimum: 1, type: "integer" }, electorate_members: { items: { maxLength: 256, minLength: 1, type: "string" }, maxItems: MAX_PARTICIPANTS, minItems: 1, type: "array" }, quorum: { maximum: MAX_PARTICIPANTS, minimum: 1, type: "integer" }, work_revision: workRevisionSchema() }, required: ["claimant_member_id", "claim_round", "electorate_members", "quorum", "work_revision"], type: "object" }; }
function outcomeSchema(): CanonicalObject { return { additionalProperties: false, properties: { moves: { maximum: MAX_MOVE_LIMIT, minimum: 0, type: "integer" }, status: { enum: ["in_progress", "participant_accepted_completion"] } }, required: ["moves", "status"], type: "object" }; }
function summarySchema(): CanonicalObject { return { additionalProperties: false, properties: { claim: { additionalProperties: false, properties: { claimant_member_id: { maxLength: 256, minLength: 1, type: "string" }, claim_round: { maximum: MAX_ROUND, minimum: 1, type: "integer" }, electorate_size: { maximum: MAX_PARTICIPANTS, minimum: 1, type: "integer" }, quorum: { maximum: MAX_PARTICIPANTS, minimum: 1, type: "integer" }, work_revision: workRevisionSchema() }, required: ["claimant_member_id", "claim_round", "electorate_size", "quorum", "work_revision"], type: "object" }, claim_open: { type: "boolean" }, assessments_by_member: { additionalProperties: true, type: "object" }, endorsement_count: { maximum: MAX_PARTICIPANTS, minimum: 0, type: "integer" }, approval_count: { maximum: MAX_PARTICIPANTS, minimum: 0, type: "integer" }, quorum: { maximum: MAX_PARTICIPANTS, minimum: 1, type: "integer" } }, required: ["claim_open", "assessments_by_member", "endorsement_count", "approval_count", "quorum"], type: "object" }; }
function stateSchema(): CanonicalObject { return { additionalProperties: false, properties: { board: boardSchema(), claim_assessments: { additionalProperties: true, type: "object" }, completion_claim: claimSchema(), contributions_by_member: { additionalProperties: true, type: "object" }, disks: { maximum: MAX_DISKS, minimum: 1, type: "integer" }, last_move: lastMoveSchema(), member_notices: { additionalProperties: true, type: "object" }, move_limit: { maximum: MAX_MOVE_LIMIT, minimum: 1, type: "integer" }, outcome: outcomeSchema(), phase: { enum: ["solving", "complete"] }, round: { maximum: MAX_ROUND, minimum: 1, type: "integer" }, work_revision: workRevisionSchema() }, required: ["board", "claim_assessments", "contributions_by_member", "disks", "member_notices", "move_limit", "outcome", "phase", "round", "work_revision"], type: "object" }; }
function projectionSchema(participant: boolean): CanonicalObject {
  const properties: Record<string, CanonicalJson> = { board: boardSchema(), completion: summarySchema(), objective: { additionalProperties: false, properties: { description: { maxLength: 256, minLength: 1, type: "string" }, source_rod: { const: "A" }, target_rod: { const: "C" } }, required: ["description", "source_rod", "target_rod"], type: "object" }, contributions_by_member: { additionalProperties: true, type: "object" }, disks: { maximum: MAX_DISKS, minimum: 1, type: "integer" }, last_move: lastMoveSchema(), outcome: outcomeSchema(), phase: { enum: ["solving", "complete"] }, round: { maximum: MAX_ROUND, minimum: 1, type: "integer" }, work_revision: workRevisionSchema(), rules: { additionalProperties: false, properties: { largest_disk_on_bottom: { type: "boolean" }, move_limit: { maximum: MAX_MOVE_LIMIT, minimum: 1, type: "integer" }, one_disk_per_action: { type: "boolean" } }, required: ["largest_disk_on_bottom", "move_limit", "one_disk_per_action"], type: "object" } };
  const required = ["board", "completion", "contributions_by_member", "disks", "objective", "outcome", "phase", "round", "rules", "work_revision"];
  if (participant) { properties.can_move = { type: "boolean" }; properties.private_member_id = { maxLength: 26, minLength: 26, type: "string" }; properties.private_role = { enum: ["observer", "solver"], type: "string" }; properties.member_notice = { maxLength: 256, minLength: 1, type: "string" }; required.push("can_move", "private_member_id", "private_role", "member_notice"); }
  return { additionalProperties: false, properties, required, type: "object" };
}
function configurationSchema(): CanonicalObject { return { additionalProperties: false, properties: { disks: { type: "integer", minimum: 1, maximum: MAX_DISKS }, initial_board: boardSchema(), move_limit: { type: "integer", minimum: 1, maximum: MAX_MOVE_LIMIT } }, required: ["disks", "move_limit"], type: "object" }; }

export default defineActivityPack({
  descriptor: {
    packId: "worldstream.tower-of-hanoi", name: "Tower of Hanoi", version: "0.1.0",
    roles: [{ role: "solver", minimum: 1, maximum: MAX_PARTICIPANTS }, { role: "observer", minimum: 0, maximum: MAX_PARTICIPANTS }],
    actions: [{ actionType: MOVE_ACTION, payloadSchema: moveSchema() }, { actionType: POST_CLAIM_ACTION, payloadSchema: postClaimSchema() }, { actionType: ASSESS_CLAIM_ACTION, payloadSchema: assessmentSchema() }],
    rejectionCodes: ["activity_complete", "blocked_disk", "claim_absent", "claim_open", "claim_superseded", "claimant_cannot_assess", "not_claim_electorate", "core_role_invariant", "invalid_move", "invalid_payload", "move_limit", "role_violation", "wrong_disk"],
    events: [{ eventType: "completion_claim_assessed", payloadSchema: { type: "object" } }, { eventType: "completion_claim_posted", payloadSchema: { type: "object" } }, { eventType: "disk_moved", payloadSchema: { type: "object" } }],
    attentionReasons: ["board_changed", "claim_review_requested"], configurationSchema: configurationSchema(), stateSchema: stateSchema(),
    projectionSchemas: { final_reveal: projectionSchema(false), historical_operator: projectionSchema(false), historical_participant: projectionSchema(true), historical_public: projectionSchema(false), operator: projectionSchema(false), participant: projectionSchema(true), public: projectionSchema(false) },
    observationSchemas: { final_reveal: projectionSchema(false), historical_operator: projectionSchema(false), historical_participant: projectionSchema(true), historical_public: projectionSchema(false), operator: projectionSchema(false), participant: projectionSchema(true), public: projectionSchema(false) },
  },
  initialize(input) {
    const configuration = record(input.configuration, "configuration");
    const disks = integerValue(configuration.disks, "configuration.disks"), moveLimit = integerValue(configuration.move_limit, "configuration.move_limit");
    if (disks < 1 || disks > MAX_DISKS) throw new TypeError("configuration.disks is outside the bounded range");
    if (moveLimit < 1 || moveLimit > MAX_MOVE_LIMIT) throw new TypeError("configuration.move_limit is outside the bounded range");
    const initialBoard = initialBoardValue(configuration.initial_board, disks);
    return { initial_activity_state: stateCanonical(initialState(disks, moveLimit, eligibleSolverMembers(coreRecord(input.initial_core_state)), initialBoard)), timer_requests: [] };
  },
  reduce(input) { return reduceTower(input); },
  view(input) { return authorizedView(stateValue(input.activity_state), coreRecord(input.core), record(input.viewer, "viewer")); },
  observe(input) {
    const viewer = record(input.viewer, "viewer");
    const beforeCore = input.core_before === undefined ? coreRecord(input.core) : coreRecord(input.core_before);
    const afterCore = input.core_after === undefined ? coreRecord(input.core) : coreRecord(input.core_after);
    const before = authorizedView(stateValue(input.activity_before), beforeCore, viewer), after = authorizedView(stateValue(input.activity_after), afterCore, viewer);
    const offersChanged = canonicalStringify(before.action_offers) !== canonicalStringify(after.action_offers);
    if (!offersChanged && canonicalStringify(before.projection) === canonicalStringify(after.projection)) return null;
    return { observation_schema: after.projection_schema, observation: after.projection, action_offers: offersChanged ? "reuse_after_view" : "unchanged" };
  },
} satisfies ActivityPackDefinition);
