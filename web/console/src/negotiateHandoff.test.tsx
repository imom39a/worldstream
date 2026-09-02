import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { ParticipantHandoffView } from "./HandedOffParticipant";
import {
  NegotiateRetainedParticipant,
  readNegotiateRetainedSessionView,
  readRetainedNegotiateReplaySummary,
} from "./negotiateHandoff";
import type {
  ParticipantBrowserObservation,
  ParticipantBrowserReplay,
} from "./participantHandoff";

const digest = `blake3:${"a".repeat(64)}`;
const hash = `blake3:${"b".repeat(64)}`;

function observation(packId = "worldstream.negotiate"): ParticipantBrowserObservation {
  return {
    pack: { id: packId, version: "0.1.0", digest },
    room_head: {
      room_seq: 9,
      genesis_or_transition_hash: hash,
      core_schema_version: "core.v1",
      pack_digest: digest,
      core_state_hash: hash,
      activity_state_hash: hash,
      authoritative_state_hash: hash,
    },
    frame_head: 12,
    delivery: [{
      kind: "projection_reset",
      body: {
        projection: {
          activity: {
            aggregate_state: "formation",
            outcome: null,
            persona: "buyer_approver",
            phase: "approval_pending",
            session_state: "open",
            current_proposal: {
              offer: { object_id: "offer-2", declared_content_hash: hash, wire_digest: hash },
            },
            pending_approval: {
              proposal_id: "offer-2",
              candidate_wire_digest: hash,
              candidate_canonical_json: '{"type":"acceptance"}',
              expires_at: 18,
              room_head_at_request: { sequence: 9, digest: hash },
              transaction_head_at_request: { sequence: 2, event_hash: hash },
              session_head_at_request: { sequence: 3, event_hash: hash },
            },
          },
          action_offers: [{
            action_type: "record_exact_approval",
            payload_schema_digest: hash,
          }],
        },
      },
    }],
  };
}

function replay(): ParticipantBrowserReplay {
  return {
    requested_room_seq: 9,
    room_head: observation().room_head,
    projection: { activity: { outcome: null } },
    projection_hash: hash,
    verification: "verified",
    room_health: "healthy",
    integrity_generation: 1,
  };
}

describe("Negotiate retained HttpOnly Participant session", () => {
  it("selects the pack renderer only from the pinned Pack identity", () => {
    expect(readNegotiateRetainedSessionView(observation("worldstream.counter"))).toBeNull();
    const view = readNegotiateRetainedSessionView(observation());
    expect(view?.session.persona).toBe("buyer_approver");
    expect(view?.session.action_offers).toEqual([{
      action_type: "record_exact_approval",
      payload_schema_digest: hash,
    }]);
  });

  it("does not expose a retained Action without the exact schema binding", () => {
    const withoutSchema = observation();
    const projection = withoutSchema.delivery[0]?.body.projection as {
      action_offers: Array<{ action_type: string; payload_schema_digest: string | null }>;
    };
    projection.action_offers[0]!.payload_schema_digest = null;
    expect(readNegotiateRetainedSessionView(withoutSchema)?.session.action_offers).toEqual([]);
  });

  it("renders exact approval and verified Replay without exposing routing authority", () => {
    const view = readNegotiateRetainedSessionView(observation());
    expect(view).not.toBeNull();
    if (view === null) throw new Error("fixture must select Negotiate");
    const dom = renderToStaticMarkup(
      <NegotiateRetainedParticipant
        view={view}
        replay={replay()}
        onAct={vi.fn()}
        onReplay={vi.fn()}
      />,
    );
    expect(dom).toContain("WorldStream Negotiate");
    expect(dom).toContain("Exact acceptance candidate bytes");
    expect(dom).toContain("Verified at sequence 9");
    expect(dom).not.toContain("room_id");
    expect(dom).not.toContain("member_id");
    expect(dom).not.toContain("Bearer");
  });

  it("keeps the generic Inspector independent from the Negotiate renderer", () => {
    const dom = renderToStaticMarkup(
      <ParticipantHandoffView
        state={{
          state: "live",
          action: "continue",
          message: null,
          deliveryBatch: observation(),
        }}
        onReconnect={vi.fn()}
        onAct={vi.fn()}
        onReplay={vi.fn()}
      />,
    );
    expect(dom).toContain("WorldStream Inspector");
    expect(dom).toContain("Authorized Room projection");
    expect(dom).toContain("Action payload (JSON)");
    expect(dom).not.toContain("Human buyer approver workspace");
  });

  it("rejects Replay from a different retained Head", () => {
    expect(readRetainedNegotiateReplaySummary({ ...replay(), requested_room_seq: 8 }, observation())).toBeNull();
    expect(readRetainedNegotiateReplaySummary(replay(), observation())?.verification).toBe("verified");
  });
});
