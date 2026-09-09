import { describe, expect, it } from "vitest";

import type { VerifiedRoomReplay } from "@worldstream/client";

import { replaySummaryFrom } from "./replay";
import { digest, projection, readyState } from "./testFixtures";

function terminalState() {
  return readyState(projection({
    phase: "complete",
    turns_used: 10,
    turns_remaining: 6,
    carried_candidate: "ledger-amber",
    outcome: { kind: "success" },
  }), []);
}

function replay(): VerifiedRoomReplay {
  const state = terminalState();
  return {
    requested_room_seq: state.roomSequence,
    room_head: {
      room_seq: state.roomSequence,
      genesis_or_transition_hash: state.roomHead.genesisOrTransitionHash,
      core_schema_version: "worldstream.core-room-state.v1",
      pack_digest: state.pack.digest,
      core_state_hash: digest("7"),
      activity_state_hash: digest("6"),
      authoritative_state_hash: state.roomHead.authoritativeStateHash,
    },
    projection: {},
    projection_hash: digest("5"),
    verification: "verified",
    room_health: "healthy",
    integrity_generation: 0,
  };
}

describe("terminal Replay binding", () => {
  it("accepts verified Replay only at the installed exact Room Head", () => {
    expect(replaySummaryFrom(replay(), terminalState())).toMatchObject({
      roomSequence: 7,
      projectionHash: digest("5"),
      verification: "verified",
    });
  });

  it("rejects every requested identity and head mismatch", () => {
    const state = terminalState();
    const valid = replay();
    const mismatches: VerifiedRoomReplay[] = [
      { ...valid, requested_room_seq: state.roomSequence + 1 },
      { ...valid, room_head: { ...valid.room_head, room_seq: state.roomSequence + 1 } },
      { ...valid, room_head: { ...valid.room_head, pack_digest: digest("4") } },
      { ...valid, room_head: { ...valid.room_head, genesis_or_transition_hash: digest("3") } },
      { ...valid, room_head: { ...valid.room_head, authoritative_state_hash: digest("2") } },
      { ...valid, projection_hash: "malformed" },
    ];
    for (const value of mismatches) expect(replaySummaryFrom(value, state)).toBeNull();
    expect(replaySummaryFrom(valid, readyState())).toBeNull();
  });
});
