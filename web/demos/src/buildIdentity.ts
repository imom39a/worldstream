import compatibilityManifest from "../../../compatibility.json";
import parityFixture from "../../../examples/heist/parity_fixture.json";

export const demoBuildIdentity = Object.freeze({
  catalogSchema: "worldstream/demo-catalog/v1",
  productVersion: compatibilityManifest.contracts.product,
  sourceRevision: __BUILD_REVISION__,
  websocketSubprotocol: "worldstream.json.v0.1",
  wireProtocol: compatibilityManifest.contracts.wire,
});

export const agentHeistEvidence = Object.freeze({
  finalOutcome: parityFixture.expected_fixture_outcome.outcome,
  finalRoomSequence: parityFixture.final_head.room_seq,
  fixtureId: parityFixture.fixture.fixture_id,
  packId: parityFixture.retained_executor.pack_id,
  packVersion: parityFixture.retained_executor.pack_version,
  replayReadOnly: parityFixture.replay.read_only,
  replayVerified: parityFixture.replay.verified,
  revisionDigest: parityFixture.retained_executor.pack_digest,
  sourceSchema: parityFixture.schema,
  transcriptDigest: parityFixture.transcript_digest,
  transcriptSchema: parityFixture.transcript_schema,
  verifiedTransitionCount: parityFixture.replay.verified_transition_count,
});
