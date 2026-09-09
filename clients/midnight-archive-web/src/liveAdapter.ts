import type {
  ActivityClientAction,
  AuthorizedRoomDeliveryBatch,
} from "@worldstream/client";

import {
  isMidnightArchiveActionType,
  type MidnightArchiveActionType,
} from "./actionContract";
import {
  MIDNIGHT_ARCHIVE_PACK_ID,
  MIDNIGHT_ARCHIVE_PACK_REVISION,
  MIDNIGHT_ARCHIVE_PACK_VERSION,
} from "./config";
import {
  actionPayload,
  readMidnightArchiveProjection,
  type MidnightArchiveActionIntent,
  type MidnightArchiveProjection,
} from "./model";

export interface MidnightArchiveActionOffer {
  readonly offerId: string;
  readonly actionType: MidnightArchiveActionType;
  readonly schemaDigest: string;
  readonly eligibility: string;
}

export interface MidnightArchiveReadyState {
  readonly kind: "ready";
  readonly pack: {
    readonly id: typeof MIDNIGHT_ARCHIVE_PACK_ID;
    readonly version: typeof MIDNIGHT_ARCHIVE_PACK_VERSION;
    readonly digest: typeof MIDNIGHT_ARCHIVE_PACK_REVISION;
  };
  readonly roomHead: {
    readonly genesisOrTransitionHash: string;
    readonly authoritativeStateHash: string;
  };
  readonly roomSequence: number;
  readonly frameHead: number;
  readonly authorization: {
    readonly accessMode: "participant";
    readonly role: "lead";
  };
  readonly projection: MidnightArchiveProjection;
  readonly offers: readonly MidnightArchiveActionOffer[];
}

export type MidnightArchiveLiveState =
  | { readonly kind: "awaiting" }
  | MidnightArchiveReadyState
  | { readonly kind: "incompatible"; readonly reason: string };

const DIGEST = /^(?:blake3|sha256):[0-9a-f]{64}$/u;
const MAX_OFFERS = 32;

export function initialMidnightArchiveLiveState(): MidnightArchiveLiveState {
  return { kind: "awaiting" };
}

/** Installs only server-authorized participant bytes; no scenario fixture exists here. */
export function reduceMidnightArchiveObservation(
  current: MidnightArchiveLiveState,
  batch: AuthorizedRoomDeliveryBatch,
): MidnightArchiveLiveState {
  const identityError = validateIdentity(batch);
  if (identityError !== null) return incompatible(identityError);
  if (current.kind === "ready" && current.pack.digest !== batch.pack.digest) {
    return incompatible("The attached Room changed its pinned Midnight Archive revision.");
  }

  if (batch.delivery.length === 0) {
    return current.kind === "ready" ? ready(
      batch,
      current.projection,
      renumberOffers(current.offers, batch.room_head.room_seq),
    ) : current;
  }

  let state = current;
  for (const delivery of batch.delivery) {
    state = delivery.kind === "projection_reset"
      ? installReset(batch, delivery.body)
      : installObservation(state, batch, delivery.body);
    if (state.kind === "incompatible") return state;
  }
  return state;
}

export function prepareMidnightArchiveAction(
  state: MidnightArchiveReadyState,
  intent: MidnightArchiveActionIntent,
  actionId: string = nextUlid(),
): ActivityClientAction {
  if (state.projection.phase !== "active") {
    throw new Error("Midnight Archive is not accepting gameplay Actions.");
  }
  if (intent.action === "commit_turn" && state.projection.stagedAction === null) {
    throw new Error("There is no staged Action to commit.");
  }
  const offer = state.offers.find((candidate) => candidate.actionType === intent.action);
  if (offer === undefined) {
    throw new Error("The requested Midnight Archive Action is not offered at this Head.");
  }
  return {
    actionId,
    basedOnRoomSeq: state.roomSequence,
    offerId: offer.offerId,
    schemaDigest: offer.schemaDigest,
    actionType: offer.actionType,
    payload: actionPayload(intent),
  };
}

function validateIdentity(batch: AuthorizedRoomDeliveryBatch): string | null {
  if (batch.pack.id !== MIDNIGHT_ARCHIVE_PACK_ID) {
    return "This Activity Client only supports Midnight Archive Rooms.";
  }
  if (batch.pack.digest !== batch.room_head.pack_digest || !DIGEST.test(batch.pack.digest)) {
    return "The Pack identity does not match the attached Room Head.";
  }
  if (
    batch.pack.version !== MIDNIGHT_ARCHIVE_PACK_VERSION
    || batch.pack.digest !== MIDNIGHT_ARCHIVE_PACK_REVISION
  ) {
    return "This client does not support the pinned Midnight Archive Pack revision.";
  }
  return null;
}

function installReset(
  batch: AuthorizedRoomDeliveryBatch,
  body: Record<string, unknown>,
): MidnightArchiveLiveState {
  const envelope = record(body.projection);
  const core = record(envelope.core);
  if (
    core.access_mode !== "participant"
    || core.viewer_class !== "participant"
    || core.standing !== "enabled"
    || core.role !== "lead"
  ) {
    return incompatible("The retained authority is not the enabled lead participant Membership.");
  }
  const projection = readMidnightArchiveProjection(envelope.activity);
  const offers = parseOffers(envelope.action_offers, batch.room_head.room_seq);
  if (projection === null || offers === null || !offersMatchPhase(projection, offers)) {
    return incompatible("The authorized Midnight Archive Projection does not match this client contract.");
  }
  return ready(batch, projection, offers);
}

function installObservation(
  current: MidnightArchiveLiveState,
  batch: AuthorizedRoomDeliveryBatch,
  body: Record<string, unknown>,
): MidnightArchiveLiveState {
  if (current.kind !== "ready") {
    return incompatible("An Observation arrived before an authorized Projection Reset.");
  }
  const wire = record(body.observation);
  const { projectionWire, actionOffers } = splitObservation(wire);
  const projection = readMidnightArchiveProjection(projectionWire);
  const offers = actionOffers.present
    ? parseOffers(actionOffers.value, batch.room_head.room_seq)
    : renumberOffers(current.offers, batch.room_head.room_seq);
  if (projection === null || offers === null || !offersMatchPhase(projection, offers)) {
    return incompatible("The Midnight Archive Observation does not match this client contract.");
  }
  return ready(batch, projection, offers);
}

function ready(
  batch: AuthorizedRoomDeliveryBatch,
  projection: MidnightArchiveProjection,
  offers: readonly MidnightArchiveActionOffer[],
): MidnightArchiveReadyState {
  return {
    kind: "ready",
    pack: {
      id: MIDNIGHT_ARCHIVE_PACK_ID,
      version: MIDNIGHT_ARCHIVE_PACK_VERSION,
      digest: MIDNIGHT_ARCHIVE_PACK_REVISION,
    },
    roomHead: {
      genesisOrTransitionHash: batch.room_head.genesis_or_transition_hash,
      authoritativeStateHash: batch.room_head.authoritative_state_hash,
    },
    roomSequence: batch.room_head.room_seq,
    frameHead: batch.frame_head,
    authorization: { accessMode: "participant", role: "lead" },
    projection,
    offers,
  };
}

function parseOffers(value: unknown, roomSequence: number): MidnightArchiveActionOffer[] | null {
  if (!Array.isArray(value) || value.length > MAX_OFFERS) return null;
  const result: MidnightArchiveActionOffer[] = [];
  const seen = new Set<MidnightArchiveActionType>();
  for (let index = 0; index < value.length; index += 1) {
    const item = record(value[index]);
    if (!isMidnightArchiveActionType(item.action_type) || seen.has(item.action_type)) return null;
    if (typeof item.payload_schema_digest !== "string" || !DIGEST.test(item.payload_schema_digest)) return null;
    const window = record(item.eligibility_window);
    const deadline = boundedText(window.deadline);
    seen.add(item.action_type);
    result.push({
      offerId: `${roomSequence}:${item.action_type}:${index}`,
      actionType: item.action_type,
      schemaDigest: item.payload_schema_digest,
      eligibility: deadline === null ? "Current synchronized turn" : `Until ${deadline}`,
    });
  }
  return result;
}

function offersMatchPhase(
  projection: MidnightArchiveProjection,
  offers: readonly MidnightArchiveActionOffer[],
): boolean {
  if (projection.phase !== "active") return offers.length === 0;
  const hasCommit = offers.some((offer) => offer.actionType === "commit_turn");
  return (projection.stagedAction === null && !hasCommit)
    || (projection.stagedAction !== null && hasCommit);
}

function renumberOffers(
  offers: readonly MidnightArchiveActionOffer[],
  roomSequence: number,
): MidnightArchiveActionOffer[] {
  return offers.map((offer, index) => ({
    ...offer,
    offerId: `${roomSequence}:${offer.actionType}:${index}`,
  }));
}

function splitObservation(wire: Record<string, unknown>): {
  readonly projectionWire: Record<string, unknown>;
  readonly actionOffers: { readonly present: boolean; readonly value: unknown };
} {
  if (!Object.hasOwn(wire, "action_offers")) {
    return { projectionWire: wire, actionOffers: { present: false, value: undefined } };
  }
  const { action_offers, ...projectionWire } = wire;
  return { projectionWire, actionOffers: { present: true, value: action_offers } };
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function boundedText(value: unknown): string | null {
  return typeof value === "string" && value.length > 0
    && new TextEncoder().encode(value).length <= 2_048
    ? value
    : null;
}

function incompatible(reason: string): MidnightArchiveLiveState {
  return { kind: "incompatible", reason };
}

function nextUlid(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[0] = (bytes[0] ?? 0) & 0x3f;
  let value = bytes.reduce((result, byte) => (result << 8n) | BigInt(byte), 0n);
  const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let encoded = "";
  for (let index = 0; index < 26; index += 1) {
    encoded = alphabet[Number(value & 31n)] + encoded;
    value >>= 5n;
  }
  return encoded;
}
