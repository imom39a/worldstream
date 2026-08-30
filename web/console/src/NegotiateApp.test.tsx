import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { NegotiateApp, preparedActionMatches } from "./NegotiateApp";
import {
  NEGOTIATE_CONSOLE_BOOTSTRAP_VERSION,
  consumeNegotiateConsoleBootstrap,
  negotiateBootstrapMatchesLiveSession,
  readNegotiateConsoleBootstrap,
  type NegotiateConsoleBootstrap,
  type NegotiatePersona,
} from "./negotiate";

const room = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const member = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const hash = `blake3:${"1".repeat(64)}`;
const wire = `sha256:${"2".repeat(64)}`;

function fixture(persona: NegotiatePersona): NegotiateConsoleBootstrap {
  const common = {
    aggregate_state: "formation",
    outcome: null,
    persona,
    phase: "approval_pending",
    session_state: "open",
  };
  const projection = persona === "spectator"
    ? common
    : persona === "operator"
      ? {
          ...common,
          current_proposal_reference: { object_id: "offer-2", content_hash: hash, wire_digest: wire },
          evidence_count: 4,
          signature_count: 0,
        }
      : persona === "buyer_approver"
        ? {
            ...common,
            current_proposal: {
              author: "seller_agent",
              offer: { object_id: "offer-2", declared_content_hash: hash, wire_digest: wire },
              valid_until: 20,
            },
            pending_approval: {
              proposal_id: "offer-2",
              proposal_content_hash: hash,
              candidate_wire_digest: wire,
              candidate_canonical_json: '{"type":"acceptance"}',
              expires_at: 18,
              room_head_at_request: { sequence: 8, digest: hash },
              transaction_head_at_request: { sequence: 2, event_hash: wire },
              session_head_at_request: { sequence: 3, event_hash: wire },
            },
          }
        : persona === "venue_signer"
          ? {
              ...common,
              current_proposal_reference: { object_id: "offer-2", content_hash: hash, wire_digest: wire },
              committed_agreement_reference: null,
              transaction_head: { sequence: 2, event_hash: wire },
              session_head: { sequence: 3, event_hash: wire },
              deadline_progress: "awaiting_session_expiry",
              event_candidates_required: ["session.closed"],
            }
          : {
              ...common,
              current_proposal: {
                author: "seller_agent",
                offer: { object_id: "offer-2", declared_content_hash: hash, wire_digest: wire },
                valid_until: 20,
              },
              agreement: null,
              recorded_approval: { decision: "approved" },
            };
  return {
    version: NEGOTIATE_CONSOLE_BOOTSTRAP_VERSION,
    room_id: room,
    member_id: persona === "spectator" || persona === "operator" ? null : member,
    room_sequence: 8,
    persona,
    projection,
    action_offers: persona === "buyer_approver"
      ? [{ action_type: "record_exact_approval", payload_schema_digest: hash }]
      : [],
    connection: "live",
    replay: "available",
    evidence: "available",
  };
}

describe("WorldStream Negotiate participant experience", () => {
  it("renders six distinct authorized personas without leaking raw signed bytes or keys", () => {
    const forbidden = /candidate_canonical_json|agreement_canonical_json|private-proof|private key material|Bearer\s|api[_-]?key/i;
    const personas: NegotiatePersona[] = [
      "buyer_agent",
      "seller_agent",
      "buyer_approver",
      "venue_signer",
      "operator",
      "spectator",
    ];
    for (const persona of personas) {
      const markup = renderToStaticMarkup(<NegotiateApp session={fixture(persona)} />);
      expect(markup).toContain("WorldStream Negotiate");
      expect(markup).toContain("Server-scoped");
      expect(markup).not.toMatch(forbidden);
    }
  });

  it("keeps spectator and operator DOM invariant when private party material changes", () => {
    for (const persona of ["spectator", "operator"] as const) {
      const original = fixture(persona);
      const mutated = {
        ...original,
        projection: {
          ...original.projection,
          candidate_canonical_json: "private-candidate-A",
          agreement_canonical_json: "private-agreement-B",
          signatures: { buyer_agent: { proof: "private-proof-C" } },
        },
      };
      expect(renderToStaticMarkup(<NegotiateApp session={mutated} />)).toBe(
        renderToStaticMarkup(<NegotiateApp session={original} />),
      );
    }
  });

  it("renders exact candidate bytes only in the authorized approver DOM", () => {
    const marker = "approval-candidate-visible-only-to-approver";
    const personas: NegotiatePersona[] = [
      "buyer_agent", "seller_agent", "buyer_approver", "venue_signer", "operator", "spectator",
    ];
    for (const persona of personas) {
      const original = fixture(persona);
      const pending = persona === "buyer_approver"
        ? { ...(original.projection.pending_approval as Record<string, unknown>), candidate_canonical_json: marker }
        : { candidate_canonical_json: marker };
      const dom = renderToStaticMarkup(
        <NegotiateApp session={{ ...original, projection: { ...original.projection, pending_approval: pending } }} />,
      );
      expect(dom.includes(marker)).toBe(persona === "buyer_approver");
    }
  });

  it("disables action submission until catch-up and exposes only server offers", () => {
    const onAction = vi.fn();
    const pending = { ...fixture("buyer_approver"), connection: "catching_up" as const };
    const markup = renderToStaticMarkup(
      <NegotiateApp session={pending} onSubmitPreparedAction={onAction} />,
    );
    expect(markup).toContain("Actions remain disabled until");
    expect(markup).toContain("Record Exact Approval");
    expect(markup).toContain("disabled=\"\"");
    expect(markup).not.toContain("commit_agreement");
  });

  it("validates and consumes the one-shot pack-specific bootstrap", () => {
    const target: { __WORLDSTREAM_NEGOTIATE_CONSOLE__?: unknown } = {
      __WORLDSTREAM_NEGOTIATE_CONSOLE__: fixture("buyer_agent"),
    };
    expect(consumeNegotiateConsoleBootstrap(target)?.persona).toBe("buyer_agent");
    expect(target.__WORLDSTREAM_NEGOTIATE_CONSOLE__).toBeUndefined();
    expect(readNegotiateConsoleBootstrap({ ...fixture("buyer_agent"), room_sequence: -1 })).toBeNull();
    expect(readNegotiateConsoleBootstrap({ ...fixture("buyer_agent"), projection: { phase: "complete", persona: "seller_agent" } })).toBeNull();
    expect(negotiateBootstrapMatchesLiveSession(fixture("buyer_agent"), { roomId: room, memberId: member })).toBe(true);
    expect(negotiateBootstrapMatchesLiveSession(fixture("buyer_agent"), { roomId: "01ARZ3NDEKTSV4RRFFQ69G5FA0", memberId: member })).toBe(false);
  });

  it("renders the real Pack nesting and exact approver and venue work context", () => {
    const approverDom = renderToStaticMarkup(<NegotiateApp session={fixture("buyer_approver")} />);
    expect(approverDom).toContain("offer-2");
    expect(approverDom).toContain("Exact acceptance candidate bytes");
    expect(approverDom).toContain("{&quot;type&quot;:&quot;acceptance&quot;}");
    expect(approverDom).toContain("Room Head at request");
    expect(approverDom).toContain("Approval expires at");

    const venueDom = renderToStaticMarkup(<NegotiateApp session={fixture("venue_signer")} />);
    expect(venueDom).toContain("A202 transaction Head");
    expect(venueDom).toContain("A202 session Head");
    expect(venueDom).toContain("session.closed");
  });

  it("accepts a prepared payload only for the exact request, offer, schema, and Room Head", () => {
    const offer = { action_type: "accept_current_proposal", payload_schema_digest: hash };
    const pending = { request_id: "prepare-1", offer, based_on_room_seq: 8 };
    const prepared = {
      request_id: "prepare-1",
      action_type: "accept_current_proposal",
      payload_schema_digest: hash,
      based_on_room_seq: 8,
      payload: { exact_bytes: "opaque" },
    };
    expect(preparedActionMatches(pending, prepared, 8)).toBe(true);
    expect(preparedActionMatches(pending, { ...prepared, based_on_room_seq: 7 }, 8)).toBe(false);
    expect(preparedActionMatches(pending, prepared, 9)).toBe(false);
    expect(preparedActionMatches(pending, { ...prepared, payload_schema_digest: wire }, 8)).toBe(false);
  });

  it("renders deterministic expiry and venue-unavailable recovery without inventing Actions", () => {
    const expired = {
      ...fixture("spectator"),
      projection: {
        aggregate_state: "expired",
        outcome: { kind: "formation_expired" },
        persona: "spectator",
        phase: "expired",
        session_state: "expired",
      },
      action_offers: [],
    } satisfies NegotiateConsoleBootstrap;
    const expiredDom = renderToStaticMarkup(<NegotiateApp session={expired} />);
    expect(expiredDom).toContain("Formation expired");
    expect(expiredDom).toContain("No Action is currently offered");

    const unavailableVenue = {
      ...fixture("venue_signer"),
      connection: "disconnected" as const,
      projection: {
        aggregate_state: "formation",
        outcome: null,
        persona: "venue_signer",
        phase: "deadline_resolution",
        session_state: "open",
      },
      action_offers: [{ action_type: "record_session_deadline_elapsed", payload_schema_digest: hash }],
    } satisfies NegotiateConsoleBootstrap;
    const venueDom = renderToStaticMarkup(<NegotiateApp session={unavailableVenue} onReconnect={() => undefined} />);
    expect(venueDom).toContain("Reconnect and catch up");
    expect(venueDom).toContain("Record Session Deadline Elapsed");
    expect(venueDom).toContain("disabled=\"\"");
  });
});
