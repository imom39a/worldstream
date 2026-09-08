import type { AuthorizedRoomDeliveryBatch } from "@worldstream/client";

import {
  isAgentHeistActionType,
  type AgentHeistActionType,
} from "./actionContract";

export type { AgentHeistActionType } from "./actionContract";

export const AGENT_HEIST_PACK_ID = "worldstream.agent-heist" as const;
export const AGENT_HEIST_REVISION_0_1 =
  "blake3:b05a682f0923001914a800072ee68348e68b979453c93e033cba7916b99a4407" as const;
export const AGENT_HEIST_REVISION_0_2 =
  "blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820" as const;
export const AGENT_HEIST_REVISION_0_3 =
  "blake3:4e4c970403f29a8448a1a3bcf7a96c030df713499730288f324c7e200d160b2d" as const;

const SUPPORTED_REVISIONS = new Map<string, string>([
  [AGENT_HEIST_REVISION_0_1, "0.1.0"],
  [AGENT_HEIST_REVISION_0_2, "0.2.0"],
  [AGENT_HEIST_REVISION_0_3, "0.3.0"],
]);
const DIGEST = /^blake3:[0-9a-f]{64}$/;
const MAX_COLLECTION = 128;
const MAX_TEXT = 2_048;

export type AgentHeistRole = "navigator" | "insider" | "broker";
export type AgentHeistPhase =
  | "lobby"
  | "briefing"
  | "negotiation"
  | "commitment"
  | "resolution"
  | "result"
  | "complete";
export interface AgentHeistActionOffer {
  readonly offerId: string;
  readonly actionType: AgentHeistActionType;
  readonly schemaDigest: string;
  readonly eligibility: string;
}

export interface AgentHeistProjection {
  readonly phase: AgentHeistPhase;
  readonly phaseGeneration: number;
  readonly phaseStart: string;
  readonly phaseDeadline: string | null;
  readonly seats: readonly { role: AgentHeistRole; present: boolean }[];
  readonly publicClaims: readonly { clueId: string; claimCode: string }[];
  readonly plans: readonly {
    planId: string;
    proposerRole: AgentHeistRole;
    route: string;
    entryWindow: string;
    requiredTool: string;
    extraction: string;
    endorsements: number;
    challenges: number;
  }[];
  readonly challenges: readonly { role: AgentHeistRole; planId: string; reason: string }[];
  readonly commitmentCount: number;
  readonly outcome: null | {
    outcome: string;
    selectedPlanId: string | null;
    score: number;
    reason: string;
  };
  readonly privateClues: readonly {
    clueId: string;
    ownerRole: AgentHeistRole;
    claimCode: string;
  }[];
  readonly ownCommitment: null | {
    selectedPlanId: string;
    contributeRequiredResource: boolean;
  };
  readonly addressedOffers: readonly {
    offerId: string;
    senderRole: AgentHeistRole;
    offeredClueId: string;
    considerationKind: "clue_disclosure" | "plan_endorsement";
    considerationId: string;
    status: string;
  }[];
}

export interface AgentHeistReadyState {
  readonly kind: "ready";
  readonly pack: {
    readonly id: typeof AGENT_HEIST_PACK_ID;
    readonly version: string;
    readonly digest: string;
  };
  readonly roomHead: {
    readonly genesisOrTransitionHash: string;
    readonly authoritativeStateHash: string;
  };
  readonly roomSequence: number;
  readonly frameHead: number;
  readonly authorization: {
    readonly accessMode: "participant" | "spectator";
    readonly role: AgentHeistRole | null;
  };
  readonly projection: AgentHeistProjection;
  readonly offers: readonly AgentHeistActionOffer[];
}

export type AgentHeistLiveState =
  | { readonly kind: "awaiting" }
  | AgentHeistReadyState
  | { readonly kind: "incompatible"; readonly reason: string };

export function initialAgentHeistLiveState(): AgentHeistLiveState {
  return { kind: "awaiting" };
}

/**
 * Reduces only already-authorized browser delivery. A Reset is constructed
 * from empty data and an Observation replaces the complete activity view.
 * Recorded fixtures are deliberately not accepted by this API.
 */
export function reduceAgentHeistObservation(
  current: AgentHeistLiveState,
  observation: AuthorizedRoomDeliveryBatch,
): AgentHeistLiveState {
  const identityError = validateIdentity(observation);
  if (identityError !== null) return incompatible(identityError);

  if (current.kind === "ready" && current.pack.digest !== observation.pack.digest) {
    return incompatible("The attached Room changed its pinned Activity Pack Revision.");
  }

  if (observation.delivery.length === 0) {
    if (current.kind !== "ready") return current;
    return withObservationHead(current, observation, current.offers);
  }

  let state = current;
  for (const delivery of observation.delivery) {
    if (delivery.kind === "projection_reset") {
      state = installReset(observation, delivery.body);
    } else {
      state = installObservation(state, observation, delivery.body);
    }
    if (state.kind === "incompatible") return state;
  }
  return state;
}

function validateIdentity(observation: AuthorizedRoomDeliveryBatch): string | null {
  if (observation.pack.id !== AGENT_HEIST_PACK_ID) {
    return "This Activity Client only supports Agent Heist Rooms.";
  }
  if (!DIGEST.test(observation.pack.digest) || observation.pack.digest !== observation.room_head.pack_digest) {
    return "The Pack identity does not match the attached Room Head.";
  }
  const expectedVersion = SUPPORTED_REVISIONS.get(observation.pack.digest);
  if (expectedVersion === undefined) {
    return "This client does not support the pinned Activity Pack Revision.";
  }
  if (observation.pack.version !== expectedVersion) {
    return "The Pack version does not match its supported exact revision digest.";
  }
  return null;
}

function installReset(
  observation: AuthorizedRoomDeliveryBatch,
  bodyValue: Record<string, unknown>,
): AgentHeistLiveState {
  const envelope = record(bodyValue.projection);
  const core = record(envelope.core);
  const accessMode = core.access_mode === "participant" || core.access_mode === "spectator"
    ? core.access_mode
    : null;
  const role = core.role === null ? null : parseRole(core.role);
  if (
    accessMode === null
    || core.standing !== "enabled"
    || (accessMode === "participant" && (core.viewer_class !== "participant" || role === null))
    || (accessMode === "spectator" && (core.viewer_class !== "public" || role !== null))
  ) {
    return incompatible("The retained authority is not an enabled Agent Heist participant or spectator Membership.");
  }
  const projection = parseProjection(envelope.activity);
  const offers = parseOffers(envelope.action_offers, observation.room_head.room_seq);
  if (projection === null || offers === null || !viewMatchesAuthorization(accessMode, projection, offers)) {
    return incompatible("The authorized Agent Heist Projection does not match this client contract.");
  }
  return ready(observation, accessMode, role, projection, offers);
}

function installObservation(
  current: AgentHeistLiveState,
  observation: AuthorizedRoomDeliveryBatch,
  bodyValue: Record<string, unknown>,
): AgentHeistLiveState {
  if (current.kind !== "ready") {
    return incompatible("An Observation arrived before an authorized Projection Reset.");
  }
  const wire = record(bodyValue.observation);
  const projection = parseProjection(wire);
  if (projection === null) {
    return incompatible("The Agent Heist Observation does not match this client contract.");
  }
  const offers = Object.prototype.hasOwnProperty.call(wire, "action_offers")
    ? parseOffers(wire.action_offers, observation.room_head.room_seq)
    : renumberOffers(current.offers, observation.room_head.room_seq);
  if (offers === null || !viewMatchesAuthorization(current.authorization.accessMode, projection, offers)) {
    return incompatible("The Agent Heist Observation carries invalid Action Offers.");
  }
  return ready(
    observation,
    current.authorization.accessMode,
    current.authorization.role,
    projection,
    offers,
  );
}

function ready(
  observation: AuthorizedRoomDeliveryBatch,
  accessMode: "participant" | "spectator",
  role: AgentHeistRole | null,
  projection: AgentHeistProjection,
  offers: readonly AgentHeistActionOffer[],
): AgentHeistReadyState {
  return {
    kind: "ready",
    pack: {
      id: AGENT_HEIST_PACK_ID,
      version: boundedText(observation.pack.version) ?? "unknown",
      digest: observation.pack.digest,
    },
    roomHead: {
      genesisOrTransitionHash: observation.room_head.genesis_or_transition_hash,
      authoritativeStateHash: observation.room_head.authoritative_state_hash,
    },
    roomSequence: observation.room_head.room_seq,
    frameHead: observation.frame_head,
    authorization: { accessMode, role },
    projection,
    offers,
  };
}

function withObservationHead(
  current: AgentHeistReadyState,
  observation: AuthorizedRoomDeliveryBatch,
  offers: readonly AgentHeistActionOffer[],
): AgentHeistReadyState {
  return ready(
    observation,
    current.authorization.accessMode,
    current.authorization.role,
    current.projection,
    renumberOffers(offers, observation.room_head.room_seq),
  );
}

function viewMatchesAuthorization(
  accessMode: "participant" | "spectator",
  projection: AgentHeistProjection,
  offers: readonly AgentHeistActionOffer[],
): boolean {
  return accessMode === "participant" || (
    offers.length === 0
    && projection.privateClues.length === 0
    && projection.ownCommitment === null
    && projection.addressedOffers.length === 0
  );
}

function parseProjection(value: unknown): AgentHeistProjection | null {
  const source = record(value);
  const phase = parsePhase(source.phase);
  const phaseGeneration = nonNegativeInteger(source.phase_generation);
  const phaseStart = boundedText(source.phase_start);
  const phaseDeadline = source.phase_deadline === null ? null : boundedText(source.phase_deadline);
  const seats = parseArray(source.seats, (candidate) => {
    const item = record(candidate);
    const role = parseRole(item.role);
    return role === null || typeof item.present !== "boolean" ? null : { role, present: item.present };
  });
  if (
    phase === null
    || phaseGeneration === null
    || phaseStart === null
    || (source.phase_deadline !== null && phaseDeadline === null)
    || seats === null
  ) return null;

  const publicClaims = parseArrayOrEmpty(source.public_claims, (candidate) => {
    const item = record(candidate);
    const clueId = boundedText(item.clue_id);
    const claimCode = boundedText(item.claim_code);
    return clueId === null || claimCode === null ? null : { clueId, claimCode };
  });
  const challenges = parseArrayOrEmpty(source.challenges, (candidate) => {
    const item = record(candidate);
    const role = parseRole(item.role);
    const planId = boundedText(item.plan_id);
    const reason = boundedText(item.reason);
    return role === null || planId === null || reason === null ? null : { role, planId, reason };
  });
  const endorsements = parseEndorsements(source.endorsements);
  const rawPlans = parseArrayOrEmpty(source.plans, (candidate) => {
    const item = record(candidate);
    const planId = boundedText(item.plan_id);
    const proposerRole = parseRole(item.proposer_role);
    const route = boundedText(item.route);
    const entryWindow = boundedText(item.entry_window);
    const requiredTool = boundedText(item.required_tool);
    const extraction = boundedText(item.extraction);
    if ([planId, route, entryWindow, requiredTool, extraction].some((item) => item === null) || proposerRole === null) return null;
    return {
      planId: planId as string,
      proposerRole,
      route: route as string,
      entryWindow: entryWindow as string,
      requiredTool: requiredTool as string,
      extraction: extraction as string,
    };
  });
  const commitmentCount = nonNegativeInteger(source.commitment_count);
  const outcome = parseOutcome(source.outcome);
  const privateClues = parseArrayOrEmpty(source.private_clues, (candidate) => {
    const item = record(candidate);
    const clueId = boundedText(item.clue_id);
    const ownerRole = parseRole(item.owner_role);
    const claimCode = boundedText(item.claim_code);
    return clueId === null || ownerRole === null || claimCode === null
      ? null
      : { clueId, ownerRole, claimCode };
  });
  const ownCommitment = parseCommitment(source.own_commitment);
  const addressedOffers = parseArrayOrEmpty(source.addressed_offers, (candidate) => {
    const item = record(candidate);
    const offerId = boundedText(item.offer_id);
    const senderRole = parseRole(item.sender_role);
    const offeredClueId = boundedText(item.offered_clue_id);
    const considerationKind: "clue_disclosure" | "plan_endorsement" | null = item.consideration_kind === "clue_disclosure"
      || item.consideration_kind === "plan_endorsement"
      ? item.consideration_kind
      : null;
    const considerationId = boundedText(item.consideration_id);
    const status = boundedText(item.status);
    return offerId === null
      || senderRole === null
      || offeredClueId === null
      || considerationKind === null
      || considerationId === null
      || status === null
      ? null
      : {
          offerId,
          senderRole,
          offeredClueId,
          considerationKind,
          considerationId,
          status,
        };
  });
  if (
    publicClaims === null
    || challenges === null
    || endorsements === null
    || rawPlans === null
    || commitmentCount === null
    || outcome === undefined
    || privateClues === null
    || ownCommitment === undefined
    || addressedOffers === null
  ) return null;

  const plans = rawPlans.map((plan) => ({
    ...plan,
    endorsements: [...endorsements.values()].filter((planId) => planId === plan.planId).length,
    challenges: challenges.filter((challenge) => challenge.planId === plan.planId).length,
  }));
  return {
    phase,
    phaseGeneration,
    phaseStart,
    phaseDeadline,
    seats,
    publicClaims,
    plans,
    challenges,
    commitmentCount,
    outcome,
    privateClues,
    ownCommitment,
    addressedOffers,
  };
}

function parseOffers(value: unknown, roomSequence: number): AgentHeistActionOffer[] | null {
  return parseArray(value, (candidate, index) => {
    const item = record(candidate);
    const actionType = parseActionType(item.action_type);
    const schemaDigest = typeof item.payload_schema_digest === "string" && DIGEST.test(item.payload_schema_digest)
      ? item.payload_schema_digest
      : null;
    if (actionType === null || schemaDigest === null) return null;
    const window = record(item.eligibility_window);
    const deadline = boundedText(window.deadline);
    return {
      offerId: `${roomSequence}:${actionType}:${index}`,
      actionType,
      schemaDigest,
      eligibility: deadline === null ? "Current synchronized phase" : `Until ${deadline}`,
    };
  });
}

function renumberOffers(
  offers: readonly AgentHeistActionOffer[],
  roomSequence: number,
): AgentHeistActionOffer[] {
  return offers.map((offer, index) => ({
    ...offer,
    offerId: `${roomSequence}:${offer.actionType}:${index}`,
  }));
}

function parseOutcome(value: unknown): AgentHeistProjection["outcome"] | undefined {
  if (value === null || value === undefined) return null;
  const item = record(value);
  const outcome = boundedText(item.outcome);
  const selectedPlanId = item.selected_plan_id === null ? null : boundedText(item.selected_plan_id);
  const score = nonNegativeInteger(item.score);
  const reason = boundedText(item.reason);
  if (outcome === null || score === null || reason === null || (item.selected_plan_id !== null && selectedPlanId === null)) return undefined;
  return { outcome, selectedPlanId, score, reason };
}

function parseCommitment(value: unknown): AgentHeistProjection["ownCommitment"] | undefined {
  if (value === null || value === undefined) return null;
  const item = record(value);
  const selectedPlanId = boundedText(item.selected_plan_id);
  return selectedPlanId === null || typeof item.contribute_required_resource !== "boolean"
    ? undefined
    : { selectedPlanId, contributeRequiredResource: item.contribute_required_resource };
}

function parseEndorsements(value: unknown): Map<AgentHeistRole, string> | null {
  if (value === undefined) return new Map();
  const source = record(value);
  if (Object.keys(source).length > MAX_COLLECTION) return null;
  const result = new Map<AgentHeistRole, string>();
  for (const [rawRole, rawPlan] of Object.entries(source)) {
    const role = parseRole(rawRole);
    const plan = boundedText(rawPlan);
    if (role === null || plan === null) return null;
    result.set(role, plan);
  }
  return result;
}

function parseArray<T>(
  value: unknown,
  read: (candidate: unknown, index: number) => T | null,
): T[] | null {
  if (!Array.isArray(value) || value.length > MAX_COLLECTION) return null;
  const result: T[] = [];
  for (let index = 0; index < value.length; index += 1) {
    const item = read(value[index], index);
    if (item === null) return null;
    result.push(item);
  }
  return result;
}

function parseArrayOrEmpty<T>(
  value: unknown,
  read: (candidate: unknown, index: number) => T | null,
): T[] | null {
  return value === undefined ? [] : parseArray(value, read);
}

function parseRole(value: unknown): AgentHeistRole | null {
  return value === "navigator" || value === "insider" || value === "broker" ? value : null;
}

function parsePhase(value: unknown): AgentHeistPhase | null {
  if (typeof value !== "string") return null;
  const normalized = value.toLowerCase();
  return ["lobby", "briefing", "negotiation", "commitment", "resolution", "result", "complete"].includes(normalized)
    ? normalized as AgentHeistPhase
    : null;
}

function parseActionType(value: unknown): AgentHeistActionType | null {
  return isAgentHeistActionType(value) ? value : null;
}

function boundedText(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 && new TextEncoder().encode(value).length <= MAX_TEXT
    ? value
    : null;
}

function nonNegativeInteger(value: unknown): number | null {
  return Number.isSafeInteger(value) && Number(value) >= 0 ? Number(value) : null;
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function incompatible(reason: string): AgentHeistLiveState {
  return { kind: "incompatible", reason };
}
