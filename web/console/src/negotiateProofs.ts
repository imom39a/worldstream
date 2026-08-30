import type { JsonValue } from "./transport";

const ROOM_ID = /^[0-9A-HJKMNP-TV-Z]{26}$/;
const DIGEST = /^(?:blake3|sha256):[0-9a-f]{64}$/;
const MAX_PROOF_BYTES = 16 * 1024 * 1024;
const MAX_PROOF_ROWS = 100_000;

export interface NegotiateReplaySummary {
  readonly room_id: string | null;
  readonly requested_room_seq: number;
  readonly pack_id: "worldstream.negotiate";
  readonly pack_revision_digest: string;
  readonly lineage_hash: string;
  readonly authoritative_state_hash: string;
  readonly projection_hash: string;
  readonly verification: "verified";
}

export interface NegotiateEvidenceDownload {
  readonly room_id: string;
  readonly room_sequence: number;
  readonly filename: string;
  readonly proof_json: string;
  readonly pack_revision_digest: string;
  readonly physical_bundle_digest: string;
  readonly party_object_count: number;
  readonly venue_record_count: number;
  readonly cross_index_count: number;
  readonly replay_declared_result: string;
}

export interface NegotiateEvidenceReadyDetail {
  readonly room_id: string;
  readonly room_sequence: number;
  readonly proof_json: string;
}

/** Reduce an authorized Replay response to safe verification facts for the DOM. */
export function readNegotiateReplaySummary(
  value: JsonValue | unknown,
  roomId: string,
  roomSequence: number,
): NegotiateReplaySummary | null {
  if (!isObject(value) || !hasExactKeys(value, [
    "room_id", "pack", "requested_room_seq", "room_head", "projection",
    "projection_hash", "verification", "room_health", "integrity_generation",
  ])) return null;
  const pack = object(value.pack);
  const head = object(value.room_head);
  if (value.room_id !== roomId || value.requested_room_seq !== roomSequence || value.verification !== "verified") return null;
  if (pack.id !== "worldstream.negotiate" || !isDigest(pack.digest)) return null;
  if (head.room_id !== roomId || head.room_seq !== roomSequence || !isDigest(head.genesis_or_transition_hash)
    || !isDigest(head.authoritative_state_hash) || !isDigest(value.projection_hash)) return null;
  return {
    room_id: roomId,
    requested_room_seq: roomSequence,
    pack_id: "worldstream.negotiate",
    pack_revision_digest: pack.digest,
    lineage_hash: head.genesis_or_transition_hash,
    authoritative_state_hash: head.authoritative_state_hash,
    projection_hash: value.projection_hash,
    verification: "verified",
  };
}

/**
 * Validate the application-provided proof package binding while preserving its
 * exact JSON text for offline download. The browser does not claim to replace
 * `worldstream-negotiate-verify`.
 */
export function readNegotiateEvidenceDownload(
  detail: unknown,
  expectedRoomId: string,
  expectedRoomSequence: number,
): NegotiateEvidenceDownload | null {
  if (!isObject(detail) || !hasExactKeys(detail, ["room_id", "room_sequence", "proof_json"])) return null;
  if (detail.room_id !== expectedRoomId || detail.room_sequence !== expectedRoomSequence || typeof detail.proof_json !== "string") return null;
  const byteCount = new TextEncoder().encode(detail.proof_json).length;
  if (byteCount === 0 || byteCount > MAX_PROOF_BYTES) return null;
  let proof: unknown;
  try { proof = JSON.parse(detail.proof_json) as unknown; } catch { return null; }
  if (!isObject(proof) || !hasExactKeys(proof, ["format", "profile", "party_protocol_proof", "venue_runtime_proof"])) return null;
  if (proof.format !== "worldstream/negotiate-proof-package/v1") return null;
  const party = object(proof.party_protocol_proof);
  const venue = object(proof.venue_runtime_proof);
  const pack = object(venue.pack);
  const finalHead = object(venue.final_room_head);
  const replay = object(venue.replay);
  if (venue.room_id !== expectedRoomId || finalHead.sequence !== expectedRoomSequence || !isDigest(finalHead.digest)) return null;
  if (pack.pack_id !== "worldstream.negotiate" || !isDigest(pack.revision_digest) || !isDigest(pack.physical_bundle_digest)) return null;
  if (!boundedArray(party.exact_objects) || !boundedArray(venue.records) || !boundedArray(venue.cross_index)) return null;
  if (typeof replay.declared_result !== "string" || replay.declared_result.length === 0 || replay.declared_result.length > 64) return null;
  return {
    room_id: expectedRoomId,
    room_sequence: expectedRoomSequence,
    filename: `worldstream-negotiate-${expectedRoomId}-${expectedRoomSequence}.proof.json`,
    proof_json: detail.proof_json,
    pack_revision_digest: pack.revision_digest,
    physical_bundle_digest: pack.physical_bundle_digest,
    party_object_count: party.exact_objects.length,
    venue_record_count: venue.records.length,
    cross_index_count: venue.cross_index.length,
    replay_declared_result: replay.declared_result,
  };
}

export function evidenceRequestDetail(roomId: string, roomSequence: number) {
  if (!ROOM_ID.test(roomId) || !Number.isSafeInteger(roomSequence) || roomSequence < 0) {
    throw new TypeError("evidence request requires an exact Room Head");
  }
  return {
    format: "worldstream/negotiate-evidence-request/v1" as const,
    room_id: roomId,
    room_sequence: roomSequence,
  };
}

export function downloadNegotiateEvidence(
  evidence: NegotiateEvidenceDownload,
  target: Pick<Document, "createElement" | "body"> = document,
  urls: Pick<typeof URL, "createObjectURL" | "revokeObjectURL"> = URL,
): void {
  const objectUrl = urls.createObjectURL(new Blob([evidence.proof_json], { type: "application/json" }));
  try {
    const link = target.createElement("a");
    link.href = objectUrl;
    link.download = evidence.filename;
    link.rel = "noopener";
    target.body.append(link);
    link.click();
    link.remove();
  } finally {
    urls.revokeObjectURL(objectUrl);
  }
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function object(value: unknown): Record<string, unknown> {
  return isObject(value) ? value : {};
}
function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return Object.keys(value).length === keys.length && keys.every((key) => key in value);
}
function isDigest(value: unknown): value is string {
  return typeof value === "string" && DIGEST.test(value);
}
function boundedArray(value: unknown): value is unknown[] {
  return Array.isArray(value) && value.length <= MAX_PROOF_ROWS;
}
