import { describe, expect, it } from "vitest";

import {
  ActivityClientRequestQueue,
  type ActivityClientActionReceipt,
  type VerifiedRoomReplay,
} from "@worldstream/client";

import {
  preparationMatchesCurrentState,
  preparedActionMatches,
  replaySummaryFrom,
  receiptFrom,
  submitPreparedAgainstLatest,
  type PendingPreparation,
} from "./NegotiateClient";
import {
  NEGOTIATE_REVISION_0_1,
  NEGOTIATE_REVISION_0_2,
  type NegotiateReadyState,
} from "./liveAdapter";

const digest = (character: string) => `blake3:${character.repeat(64)}`;

const pending: PendingPreparation = {
  requestId: "request-1",
  offer: { action_type: "submit_proposal", payload_schema_digest: digest("a") },
  binding: {
    offerId: "12:submit_proposal:0",
    actionType: "submit_proposal",
    schemaDigest: digest("a"),
  },
  basedOnRoomSequence: 12,
  pack: {
    id: "worldstream.negotiate",
    version: "0.1.0",
    digest: NEGOTIATE_REVISION_0_1,
  },
};

function ready(
  roomSequence = 12,
  pack: NegotiateReadyState["pack"] = pending.pack,
): NegotiateReadyState {
  return {
    kind: "ready",
    pack,
    roomSequence,
    frameHead: 14,
    roomHead: {
      room_seq: roomSequence,
      genesis_or_transition_hash: digest("1"),
      core_schema_version: "worldstream.core-room-state.v1",
      pack_digest: pack.digest,
      core_state_hash: digest("2"),
      activity_state_hash: digest("3"),
      authoritative_state_hash: digest("4"),
    },
    authorization: { accessMode: "participant", role: "buyer_agent" },
    session: {
      room_sequence: roomSequence,
      persona: "buyer_agent",
      projection: {},
      action_offers: [pending.offer],
      connection: "live",
      replay: "available",
      evidence: "unavailable",
    },
    offerBindings: [pending.binding],
  };
}

describe("Negotiate prepared Action boundary", () => {
  it("requires both the retained request and the current Room offer/head", () => {
    const prepared = {
      request_id: "request-1",
      action_type: "submit_proposal",
      payload_schema_digest: digest("a"),
      based_on_room_seq: 12,
      pack: pending.pack,
      payload: { proposal_id: "proposal-1" },
    };

    expect(preparedActionMatches(pending, prepared)).toBe(true);
    expect(preparationMatchesCurrentState(pending, ready())).toBe(true);
    expect(preparationMatchesCurrentState(pending, ready(13))).toBe(false);
    expect(preparationMatchesCurrentState({
      ...pending,
      binding: { ...pending.binding, schemaDigest: digest("b") },
    }, ready())).toBe(false);
  });

  it("binds signer preparation to the exact 0.1 or 0.2 Pack Revision", () => {
    const pack0_2 = {
      id: "worldstream.negotiate",
      version: "0.2.0",
      digest: NEGOTIATE_REVISION_0_2,
    } as const;
    const pending0_2 = { ...pending, pack: pack0_2 };
    const prepared0_1 = {
      request_id: pending.requestId,
      action_type: pending.binding.actionType,
      payload_schema_digest: pending.binding.schemaDigest,
      based_on_room_seq: pending.basedOnRoomSequence,
      pack: pending.pack,
      payload: { proposal_id: "proposal-1" },
    };
    const prepared0_2 = { ...prepared0_1, pack: pack0_2 };

    expect(preparedActionMatches(pending, prepared0_1)).toBe(true);
    expect(preparedActionMatches(pending0_2, prepared0_2)).toBe(true);
    expect(preparedActionMatches(pending, prepared0_2)).toBe(false);
    expect(preparedActionMatches(pending0_2, prepared0_1)).toBe(false);
    expect(preparationMatchesCurrentState(pending0_2, ready(12, pack0_2))).toBe(true);
    expect(preparationMatchesCurrentState(pending0_2, ready())).toBe(false);
  });

  it("reads authoritative Action details from the broker receipt envelope", () => {
    const rejected = receiptFrom({
      state: "rejected",
      receipt: {
        action_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        code: "stale_action",
        message: "The Room Head advanced.",
      },
    } as ActivityClientActionReceipt);

    expect(rejected).toEqual({
      state: "rejected",
      action_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
      code: "stale_action",
      message: "The Room Head advanced.",
    });
  });

  it("rechecks freshness after an earlier queued refresh advances the Room Head", async () => {
    const queue = new ActivityClientRequestQueue();
    let current: NegotiateReadyState = ready();
    let releaseRefresh: () => void = () => {};
    const refreshGate = new Promise<void>((resolve) => { releaseRefresh = resolve; });
    const refresh = queue.run(async () => {
      await refreshGate;
      current = ready(13);
    });
    let submissions = 0;
    const submission = queue.run(() => submitPreparedAgainstLatest(
      pending,
      { proposal_id: "proposal-1" },
      () => current,
      async () => {
        submissions += 1;
        return { state: "accepted", receipt: {} } as ActivityClientActionReceipt;
      },
    ));

    releaseRefresh();
    await refresh;

    await expect(submission).resolves.toEqual({ state: "stale" });
    expect(submissions).toBe(0);
  });

  it("accepts Replay facts only at the exact retained Room and Pack heads", () => {
    const replay = {
      requested_room_seq: 12,
      room_head: ready().roomHead,
      projection: {},
      projection_hash: digest("5"),
      verification: "verified",
      room_health: "healthy",
      integrity_generation: 1,
    } satisfies VerifiedRoomReplay;

    expect(replaySummaryFrom(replay, ready())).toMatchObject({
      requested_room_seq: 12,
      pack_revision_digest: NEGOTIATE_REVISION_0_1,
      verification: "verified",
    });
    expect(replaySummaryFrom({ ...replay, requested_room_seq: 13 }, ready())).toBeNull();
    expect(replaySummaryFrom({
      ...replay,
      room_head: { ...replay.room_head, pack_digest: digest("9") },
    }, ready())).toBeNull();
  });
});
