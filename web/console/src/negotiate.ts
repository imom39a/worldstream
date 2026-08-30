import type { JsonObject, JsonValue } from "./transport";

export const NEGOTIATE_CONSOLE_BOOTSTRAP_VERSION =
  "worldstream.negotiate.console.v1" as const;

const MAX_TEXT_BYTES = 16_384;
const MAX_ACTION_OFFERS = 64;
const ID_PATTERN = /^[0-9A-HJKMNP-TV-Z]{26}$/;
const DIGEST_PATTERN = /^(?:blake3|sha256):[0-9a-f]{64}$/;

export type NegotiatePersona =
  | "buyer_agent"
  | "seller_agent"
  | "buyer_approver"
  | "venue_signer"
  | "operator"
  | "spectator";

export type NegotiatePhase =
  | "formation_open"
  | "approval_pending"
  | "offer_accepted"
  | "agreement_pending"
  | "withdrawn_pending_expiry"
  | "deadline_resolution"
  | "complete"
  | "expired";

export interface NegotiateActionOffer {
  readonly action_type: string;
  readonly payload_schema_digest: string | null;
}

export interface NegotiateConsoleBootstrap {
  readonly version: typeof NEGOTIATE_CONSOLE_BOOTSTRAP_VERSION;
  readonly room_id: string;
  readonly member_id: string | null;
  readonly room_sequence: number;
  readonly persona: NegotiatePersona;
  readonly projection: JsonObject;
  readonly action_offers: readonly NegotiateActionOffer[];
  readonly connection: "connecting" | "catching_up" | "live" | "disconnected";
  readonly replay: "available" | "pending" | "unavailable";
  readonly evidence: "available" | "pending" | "unavailable";
}

export interface NegotiateAuthorizedView {
  readonly persona: NegotiatePersona;
  readonly projection: JsonObject;
  readonly action_offers: readonly NegotiateActionOffer[];
}

export function readNegotiateConsoleBootstrap(
  value: unknown,
): NegotiateConsoleBootstrap | null {
  if (!isRecord(value) || value.version !== NEGOTIATE_CONSOLE_BOOTSTRAP_VERSION) {
    return null;
  }
  if (!isIdentifier(value.room_id)) return null;
  if (value.member_id !== null && !isIdentifier(value.member_id)) return null;
  if (!isSafeSequence(value.room_sequence)) return null;
  if (!isPersona(value.persona) || !isRecord(value.projection)) return null;
  if (!isConnection(value.connection) || !isAvailability(value.replay)) return null;
  if (!isAvailability(value.evidence)) return null;
  const authorized = readNegotiateAuthorizedView(
    value.projection,
    value.persona,
    value.action_offers,
  );
  if (authorized === null) return null;
  return {
    version: NEGOTIATE_CONSOLE_BOOTSTRAP_VERSION,
    room_id: value.room_id,
    member_id: value.member_id,
    room_sequence: value.room_sequence,
    persona: value.persona,
    projection: authorized.projection,
    action_offers: authorized.action_offers,
    connection: value.connection,
    replay: value.replay,
    evidence: value.evidence,
  };
}

export function readNegotiateAuthorizedView(
  projection: unknown,
  persona: unknown,
  actionOffers: unknown,
): NegotiateAuthorizedView | null {
  if (!isPersona(persona) || !isRecord(projection) || !Array.isArray(actionOffers)) return null;
  if (actionOffers.length > MAX_ACTION_OFFERS || !projectionMatchesPersona(projection, persona)) return null;
  const offers: NegotiateActionOffer[] = [];
  for (const candidate of actionOffers) {
    if (typeof candidate === "string") {
      if (!isBoundedText(candidate)) return null;
      offers.push({ action_type: candidate, payload_schema_digest: null });
      continue;
    }
    if (
      !isRecord(candidate)
      || !isBoundedText(candidate.action_type)
      || (candidate.payload_schema_digest !== null && !isDigest(candidate.payload_schema_digest))
    ) return null;
    offers.push({
      action_type: candidate.action_type,
      payload_schema_digest: candidate.payload_schema_digest,
    });
  }
  return { persona, projection, action_offers: offers };
}

export function consumeNegotiateConsoleBootstrap(target: {
  __WORLDSTREAM_NEGOTIATE_CONSOLE__?: unknown;
}): NegotiateConsoleBootstrap | null {
  const bootstrap = readNegotiateConsoleBootstrap(
    target.__WORLDSTREAM_NEGOTIATE_CONSOLE__,
  );
  delete target.__WORLDSTREAM_NEGOTIATE_CONSOLE__;
  return bootstrap;
}

export function projectionPhase(projection: JsonObject): NegotiatePhase {
  const phase = projection.phase;
  if (!isPhase(phase)) throw new TypeError("Negotiate projection has an invalid phase");
  return phase;
}

export function projectionText(
  projection: JsonObject,
  field: string,
): string | null {
  const value = projection[field];
  return typeof value === "string" && isBoundedText(value) ? value : null;
}

export function projectionRecord(
  projection: JsonObject,
  field: string,
): JsonObject | null {
  const value = projection[field];
  return isRecord(value) ? value : null;
}

export function safeReference(record: JsonObject | null): {
  readonly id: string | null;
  readonly hash: string | null;
  readonly digest: string | null;
} {
  if (record === null) return { id: null, hash: null, digest: null };
  const nestedOffer = isRecord(record.offer) ? record.offer : null;
  return {
    id: firstBoundedText(record, ["object_id", "proposal_id", "agreement_id"])
      ?? (nestedOffer === null ? null : firstBoundedText(nestedOffer, ["object_id"])),
    hash: firstDigest(record, ["content_hash", "declared_content_hash", "proposal_content_hash", "agreement_content_hash"])
      ?? (nestedOffer === null ? null : firstDigest(nestedOffer, ["declared_content_hash", "content_hash"])),
    digest: firstDigest(record, ["wire_digest", "candidate_wire_digest"])
      ?? (nestedOffer === null ? null : firstDigest(nestedOffer, ["wire_digest"])),
  };
}

export function projectionInteger(
  projection: JsonObject,
  field: string,
): number | null {
  const value = projection[field];
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? value
    : null;
}

export function projectionTextList(
  projection: JsonObject,
  field: string,
): readonly string[] {
  const value = projection[field];
  if (!Array.isArray(value) || value.length > 64) return [];
  return value.every(isBoundedText) ? value : [];
}

export function negotiateBootstrapMatchesLiveSession(
  bootstrap: Pick<NegotiateConsoleBootstrap, "room_id" | "member_id">,
  live: { readonly roomId: string; readonly memberId: string },
): boolean {
  return bootstrap.room_id === live.roomId && bootstrap.member_id === live.memberId;
}

export function hasActionOffer(
  bootstrap: Pick<NegotiateConsoleBootstrap, "action_offers">,
  actionType: string,
): boolean {
  return bootstrap.action_offers.some((offer) => offer.action_type === actionType);
}

function projectionMatchesPersona(
  projection: JsonObject,
  persona: NegotiatePersona,
): boolean {
  return projection.persona === persona && isPhase(projection.phase);
}

function isRecord(value: unknown): value is JsonObject {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isBoundedText(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    new TextEncoder().encode(value).length <= MAX_TEXT_BYTES
  );
}

function isIdentifier(value: unknown): value is string {
  return typeof value === "string" && ID_PATTERN.test(value);
}

function isDigest(value: unknown): value is string {
  return typeof value === "string" && DIGEST_PATTERN.test(value);
}

function isSafeSequence(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isPersona(value: unknown): value is NegotiatePersona {
  return (
    value === "buyer_agent" ||
    value === "seller_agent" ||
    value === "buyer_approver" ||
    value === "venue_signer" ||
    value === "operator" ||
    value === "spectator"
  );
}

function isPhase(value: JsonValue | unknown): value is NegotiatePhase {
  return (
    value === "formation_open" ||
    value === "approval_pending" ||
    value === "offer_accepted" ||
    value === "agreement_pending" ||
    value === "withdrawn_pending_expiry" ||
    value === "deadline_resolution" ||
    value === "complete" ||
    value === "expired"
  );
}

function isConnection(
  value: unknown,
): value is NegotiateConsoleBootstrap["connection"] {
  return (
    value === "connecting" ||
    value === "catching_up" ||
    value === "live" ||
    value === "disconnected"
  );
}

function isAvailability(
  value: unknown,
): value is NegotiateConsoleBootstrap["replay"] {
  return value === "available" || value === "pending" || value === "unavailable";
}

function firstBoundedText(
  record: JsonObject,
  fields: readonly string[],
): string | null {
  for (const field of fields) {
    const value = record[field];
    if (isBoundedText(value)) return value;
  }
  return null;
}

function firstDigest(record: JsonObject, fields: readonly string[]): string | null {
  for (const field of fields) {
    const value = record[field];
    if (isDigest(value)) return value;
  }
  return null;
}
