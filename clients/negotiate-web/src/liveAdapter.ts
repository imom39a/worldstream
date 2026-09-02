import type { AuthorizedRoomDeliveryBatch } from "@worldstream/client";

import {
  readNegotiateAuthorizedView,
  type JsonObject,
  type NegotiateActionOffer,
  type NegotiateConsoleBootstrap,
  type NegotiatePersona,
} from "./model";

export const NEGOTIATE_PACK_ID = "worldstream.negotiate" as const;
export const NEGOTIATE_REVISION_0_1 =
  "blake3:a62585c88ffebe0b2222f5f93e17de1e9cbb003593eca4891225f75dca985589" as const;

const DIGEST = /^(?:blake3|sha256):[0-9a-f]{64}$/;
const MAX_ROLE_BYTES = 128;

export interface NegotiateOfferBinding {
  readonly offerId: string;
  readonly actionType: string;
  readonly schemaDigest: string;
}

export interface NegotiateActionReceipt {
  readonly state: "idle" | "submitting" | "accepted" | "rejected" | "stale";
  readonly action_id: string | null;
  readonly code: string | null;
  readonly message: string | null;
}

export interface NegotiateReadyState {
  readonly kind: "ready";
  readonly pack: {
    readonly id: typeof NEGOTIATE_PACK_ID;
    readonly version: "0.1.0";
    readonly digest: typeof NEGOTIATE_REVISION_0_1;
  };
  readonly roomSequence: number;
  readonly frameHead: number;
  readonly roomHead: AuthorizedRoomDeliveryBatch["room_head"];
  readonly authorization: {
    readonly accessMode: "participant" | "spectator";
    readonly role: string | null;
  };
  readonly session: Pick<
    NegotiateConsoleBootstrap,
    "room_sequence" | "persona" | "projection" | "action_offers" | "connection" | "replay" | "evidence"
  >;
  readonly offerBindings: readonly NegotiateOfferBinding[];
}

export type NegotiateLiveState =
  | { readonly kind: "awaiting" }
  | NegotiateReadyState
  | { readonly kind: "incompatible"; readonly reason: string };

export function initialNegotiateLiveState(): NegotiateLiveState {
  return { kind: "awaiting" };
}

/**
 * Installs only complete, server-authorized Negotiate views. A Reset starts
 * from empty state, so a field omitted by a later Projection can never retain
 * data from an earlier view or a recorded fixture.
 */
export function reduceNegotiateObservation(
  current: NegotiateLiveState,
  batch: AuthorizedRoomDeliveryBatch,
): NegotiateLiveState {
  const identityError = validateIdentity(batch);
  if (identityError !== null) return incompatible(identityError);
  if (current.kind === "ready" && current.pack.digest !== batch.pack.digest) {
    return incompatible("The attached Room changed its pinned Activity Pack Revision.");
  }
  if (batch.delivery.length === 0) {
    return current.kind === "ready" ? withHead(current, batch) : current;
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

function validateIdentity(batch: AuthorizedRoomDeliveryBatch): string | null {
  if (batch.pack.id !== NEGOTIATE_PACK_ID) {
    return "This Activity Client only supports WorldStream Negotiate Rooms.";
  }
  if (
    batch.pack.digest !== NEGOTIATE_REVISION_0_1
    || batch.room_head.pack_digest !== NEGOTIATE_REVISION_0_1
    || !DIGEST.test(batch.pack.digest)
  ) {
    return "This client does not support the pinned Activity Pack Revision.";
  }
  return batch.pack.version === "0.1.0"
    ? null
    : "The Pack version does not match its exact revision digest.";
}

function installReset(
  batch: AuthorizedRoomDeliveryBatch,
  body: Record<string, unknown>,
): NegotiateLiveState {
  const envelope = record(body.projection);
  const core = record(envelope.core);
  const accessMode = readAccessMode(core.access_mode);
  const role = core.role === null ? null : boundedText(core.role);
  if (
    accessMode === null
    || core.standing !== "enabled"
    || (accessMode === "participant" && (core.viewer_class !== "participant" || role === null))
    || (accessMode === "spectator" && (core.viewer_class !== "public" || role !== null))
  ) {
    return incompatible("The retained authority is not an enabled Negotiate participant or spectator Membership.");
  }
  return installCompleteView(batch, accessMode, role, envelope.activity, envelope.action_offers);
}

function installObservation(
  current: NegotiateLiveState,
  batch: AuthorizedRoomDeliveryBatch,
  body: Record<string, unknown>,
): NegotiateLiveState {
  if (current.kind !== "ready") {
    return incompatible("An Observation arrived before an authorized Projection Reset.");
  }
  const wire = record(body.observation);
  const activity = Object.hasOwn(wire, "projection") ? wire.projection : wire;
  const actionOffers = Object.hasOwn(wire, "action_offers")
    ? wire.action_offers
    : current.session.action_offers;
  return installCompleteView(
    batch,
    current.authorization.accessMode,
    current.authorization.role,
    activity,
    actionOffers,
  );
}

function installCompleteView(
  batch: AuthorizedRoomDeliveryBatch,
  accessMode: "participant" | "spectator",
  role: string | null,
  projection: unknown,
  actionOffers: unknown,
): NegotiateLiveState {
  const source = record(projection);
  const authorized = readNegotiateAuthorizedView(source, source.persona, actionOffers);
  if (authorized === null || !authorizationMatches(accessMode, role, authorized.persona)) {
    return incompatible("The authorized Negotiate Projection does not match this Membership.");
  }
  const exactOffers = authorized.action_offers.filter(hasExactSchemaDigest);
  return ready(batch, accessMode, role, authorized.persona, authorized.projection, exactOffers);
}

function authorizationMatches(
  accessMode: "participant" | "spectator",
  role: string | null,
  persona: NegotiatePersona,
): boolean {
  return accessMode === "spectator"
    ? role === null && persona === "spectator"
    : role === persona && persona !== "spectator";
}

function ready(
  batch: AuthorizedRoomDeliveryBatch,
  accessMode: "participant" | "spectator",
  role: string | null,
  persona: NegotiatePersona,
  projection: JsonObject,
  offers: readonly NegotiateActionOffer[],
): NegotiateReadyState {
  const offerBindings = offers.map((offer, index) => ({
    offerId: `${batch.room_head.room_seq}:${offer.action_type}:${index}`,
    actionType: offer.action_type,
    schemaDigest: offer.payload_schema_digest!,
  }));
  return {
    kind: "ready",
    pack: { id: NEGOTIATE_PACK_ID, version: "0.1.0", digest: NEGOTIATE_REVISION_0_1 },
    roomSequence: batch.room_head.room_seq,
    frameHead: batch.frame_head,
    roomHead: batch.room_head,
    authorization: { accessMode, role },
    session: {
      room_sequence: batch.room_head.room_seq,
      persona,
      projection,
      action_offers: offers,
      connection: "live",
      replay: "available",
      evidence: "unavailable",
    },
    offerBindings,
  };
}

function withHead(
  current: NegotiateReadyState,
  batch: AuthorizedRoomDeliveryBatch,
): NegotiateReadyState {
  return ready(
    batch,
    current.authorization.accessMode,
    current.authorization.role,
    current.session.persona,
    current.session.projection,
    current.session.action_offers,
  );
}

function hasExactSchemaDigest(
  offer: NegotiateActionOffer,
): offer is NegotiateActionOffer & { readonly payload_schema_digest: string } {
  return offer.payload_schema_digest !== null && DIGEST.test(offer.payload_schema_digest);
}

function readAccessMode(value: unknown): "participant" | "spectator" | null {
  return value === "participant" || value === "spectator" ? value : null;
}

function boundedText(value: unknown): string | null {
  return typeof value === "string"
    && value.length > 0
    && new TextEncoder().encode(value).length <= MAX_ROLE_BYTES
    ? value
    : null;
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function incompatible(reason: string): NegotiateLiveState {
  return { kind: "incompatible", reason };
}
