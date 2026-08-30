import { describe, expect, it } from "vitest";

import { evidenceRequestDetail, readNegotiateEvidenceDownload, readNegotiateReplaySummary } from "./negotiateProofs";

const room = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const digest = `blake3:${"a".repeat(64)}`;

function proofJson(overrides: Record<string, unknown> = {}) {
  return JSON.stringify({
    format: "worldstream/negotiate-proof-package/v1",
    profile: {},
    party_protocol_proof: { exact_objects: [{ object_id: "offer" }] },
    venue_runtime_proof: {
      room_id: room,
      pack: { pack_id: "worldstream.negotiate", revision_digest: digest, physical_bundle_digest: digest },
      final_room_head: { sequence: 9, digest },
      records: [{}],
      replay: { declared_result: "verified" },
      cross_index: [{}],
      ...overrides,
    },
  });
}

describe("Negotiate Replay and dual-evidence clients", () => {
  it("reduces a verified exact-head Replay to privacy-safe facts", () => {
    const summary = readNegotiateReplaySummary({
      room_id: room,
      pack: { id: "worldstream.negotiate", version: "0.1.0", digest },
      requested_room_seq: 9,
      room_head: { room_id: room, room_seq: 9, genesis_or_transition_hash: digest, authoritative_state_hash: digest },
      projection: { activity: { outcome: { kind: "agreement_committed" } } },
      projection_hash: digest,
      verification: "verified",
      room_health: "healthy",
      integrity_generation: 1,
    }, room, 9);
    expect(summary?.verification).toBe("verified");
    expect(summary).not.toHaveProperty("projection");
  });

  it("preserves exact proof JSON for offline download and exposes only counts", () => {
    const proof_json = proofJson();
    const evidence = readNegotiateEvidenceDownload({ room_id: room, room_sequence: 9, proof_json }, room, 9);
    expect(evidence?.proof_json).toBe(proof_json);
    expect(evidence?.party_object_count).toBe(1);
    expect(evidence?.cross_index_count).toBe(1);
  });

  it("rejects tampered Room heads, Pack identity, and wrapper fields", () => {
    expect(readNegotiateEvidenceDownload({ room_id: room, room_sequence: 8, proof_json: proofJson() }, room, 9)).toBeNull();
    expect(readNegotiateEvidenceDownload({ room_id: room, room_sequence: 9, proof_json: proofJson({ room_id: "01ARZ3NDEKTSV4RRFFQ69G5FA0" }) }, room, 9)).toBeNull();
    expect(readNegotiateEvidenceDownload({ room_id: room, room_sequence: 9, proof_json: proofJson({ pack: { pack_id: "other", revision_digest: digest, physical_bundle_digest: digest } }) }, room, 9)).toBeNull();
    expect(readNegotiateEvidenceDownload({ room_id: room, room_sequence: 9, proof_json: proofJson(), bearer: "wsb1:secret" }, room, 9)).toBeNull();
  });

  it("creates a metadata-only evidence request", () => {
    expect(evidenceRequestDetail(room, 9)).toEqual({
      format: "worldstream/negotiate-evidence-request/v1",
      room_id: room,
      room_sequence: 9,
    });
  });
});
