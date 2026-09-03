import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import {
  ParticipantHandoffView,
  readInspectorActionFailure,
  readInspectorActionReceipt,
} from "./HandedOffParticipant";
import { ParticipantHandoffError } from "./participantHandoff";

describe("handed-off Participant Console", () => {
  it("renders the authorized projection, observation, and exact offered Action controls", () => {
    const markup = renderToStaticMarkup(<ParticipantHandoffView
      state={{
        state: "live",
        action: "continue",
        message: null,
        deliveryBatch: {
          pack: { id: "worldstream.agent-heist", version: "0.1.0", digest: `blake3:${"2".repeat(64)}` },
          room_head: {
            room_seq: 12,
            genesis_or_transition_hash: `blake3:${"1".repeat(64)}`,
            core_schema_version: "core.v1",
            pack_digest: `blake3:${"2".repeat(64)}`,
            core_state_hash: `blake3:${"3".repeat(64)}`,
            activity_state_hash: `blake3:${"4".repeat(64)}`,
            authoritative_state_hash: `blake3:${"5".repeat(64)}`,
          },
          frame_head: 13,
          delivery: [
            { kind: "projection_reset", body: { projection: { phase: "Lobby", action_offers: [{ domain: "activity", action_type: "ready", payload_schema_digest: `blake3:${"a".repeat(64)}` }] } } },
            { kind: "observation", body: { observation: { notice: "Crew assembled" } } },
          ],
        },
      }}
      replay={{
        requested_room_seq: 12,
        room_head: {
          room_seq: 12,
          genesis_or_transition_hash: `blake3:${"1".repeat(64)}`,
          core_schema_version: "core.v1",
          pack_digest: `blake3:${"2".repeat(64)}`,
          core_state_hash: `blake3:${"3".repeat(64)}`,
          activity_state_hash: `blake3:${"4".repeat(64)}`,
          authoritative_state_hash: `blake3:${"5".repeat(64)}`,
        },
        projection: { core: {}, activity: { value: 2 }, action_offers: [] },
        projection_hash: `blake3:${"6".repeat(64)}`,
        verification: "verified",
        room_health: "healthy",
        integrity_generation: 1,
      }}
      onReconnect={vi.fn()}
      onAct={vi.fn()}
      onReplay={vi.fn()}
    />);
    expect(markup).toContain("WorldStream Inspector");
    expect(markup).toContain("Authorized Room projection");
    expect(markup).toContain("Projection reset");
    expect(markup).toContain("Crew assembled");
    expect(markup).toContain("Submit ready");
    expect(markup).toContain(`blake3:${"a".repeat(64)}`);
    expect(markup).toContain("Authorized Replay");
    expect(markup).toContain("Verify Replay at current sequence");
    expect(markup).toContain("Verified Canonical History at sequence 12");
    expect(markup).toContain("&quot;value&quot;: 2");
    expect(markup).not.toMatch(/room_id|member_id|membership_id|wsb1:/);
  });

  it("clears prior Action Offers when a later delivery explicitly replaces them with an empty set", () => {
    const markup = renderToStaticMarkup(<ParticipantHandoffView
      state={{
        state: "live",
        action: "continue",
        message: null,
        deliveryBatch: {
          pack: { id: "example.pack", version: "0.1.0", digest: `blake3:${"2".repeat(64)}` },
          room_head: {
            room_seq: 13,
            genesis_or_transition_hash: `blake3:${"1".repeat(64)}`,
            core_schema_version: "core.v1",
            pack_digest: `blake3:${"2".repeat(64)}`,
            core_state_hash: `blake3:${"3".repeat(64)}`,
            activity_state_hash: `blake3:${"4".repeat(64)}`,
            authoritative_state_hash: `blake3:${"5".repeat(64)}`,
          },
          frame_head: 14,
          delivery: [
            { kind: "projection_reset", body: { projection: { action_offers: [{ action_type: "ready", payload_schema_digest: `blake3:${"a".repeat(64)}` }] } } },
            { kind: "observation", body: { observation: { notice: "Phase advanced", action_offers: [] } } },
          ],
        },
      }}
      onReconnect={vi.fn()}
      onAct={vi.fn()}
    />);
    expect(markup).toContain("No Action is offered at this synchronized head.");
    expect(markup).not.toContain("Submit ready");
  });

  it("renders the session's retained Action Offers when the current batch omits them", () => {
    const markup = renderToStaticMarkup(<ParticipantHandoffView
      state={{
        state: "live",
        action: "continue",
        message: null,
        deliveryBatch: {
          pack: { id: "example.pack", version: "0.1.0", digest: `blake3:${"2".repeat(64)}` },
          room_head: {
            room_seq: 14,
            genesis_or_transition_hash: `blake3:${"1".repeat(64)}`,
            core_schema_version: "core.v1",
            pack_digest: `blake3:${"2".repeat(64)}`,
            core_state_hash: `blake3:${"3".repeat(64)}`,
            activity_state_hash: `blake3:${"4".repeat(64)}`,
            authoritative_state_hash: `blake3:${"5".repeat(64)}`,
          },
          frame_head: 15,
          delivery: [{ kind: "observation", body: { observation: { notice: "Still available" } } }],
        },
      }}
      actionOfferCandidates={[{
        action_type: "ready",
        payload_schema_digest: `blake3:${"a".repeat(64)}`,
      }]}
      onReconnect={vi.fn()}
      onAct={vi.fn()}
    />);
    expect(markup).toContain("Submit ready");
    expect(markup).not.toContain("No Action is offered at this synchronized head.");
  });

  it("renders only the bounded accepted Action receipt fields", () => {
    const markup = renderToStaticMarkup(<ParticipantHandoffView
      state={{ state: "disconnected", action: "reconnect", message: "Reconnect required." }}
      actionReceipt={readInspectorActionReceipt({
        state: "accepted",
        receipt: {
          action_id: "01JACT00000000000000000000",
          transition_id: "01JTRANSITION0000000000000",
          room_head: { room_seq: 21 },
          duplicate: false,
          room_id: "must-never-render",
          bearer: "wsb1:must-never-render",
        },
      }, "fallback-action")}
      onReconnect={vi.fn()}
      onAct={vi.fn()}
    />);
    expect(markup).toContain("Action accepted");
    expect(markup).toContain("01JACT00000000000000000000");
    expect(markup).toContain("01JTRANSITION0000000000000");
    expect(markup).toContain("Room sequence");
    expect(markup).toContain("21");
    expect(markup).not.toMatch(/room_id|member_id|membership_id|wsb1:|wst1:/i);
  });

  it("renders a safe rejected Action receipt without transport or authority details", () => {
    const markup = renderToStaticMarkup(<ParticipantHandoffView
      state={{ state: "disconnected", action: "reconnect", message: "Reconnect required." }}
      actionReceipt={readInspectorActionReceipt({
        state: "rejected",
        receipt: {
          code: "action_stale",
          message: "The Action was based on an earlier Room Head.",
          retryable_with_same_action_id: true,
          room_id: "must-never-render",
        },
      }, "fallback-action")}
      onReconnect={vi.fn()}
      onAct={vi.fn()}
    />);
    expect(markup).toContain("Action rejected");
    expect(markup).toContain("action_stale");
    expect(markup).toContain("The Action was based on an earlier Room Head.");
    expect(markup).toContain("Retryable");
    expect(markup).toContain("Yes");
    expect(markup).not.toMatch(/room_id|member_id|membership_id|wsb1:|wst1:|https?:\/\//i);
  });

  it("preserves only bounded failure guidance from a Participant handoff error", () => {
    expect(readInspectorActionFailure(new ParticipantHandoffError(
      "action_stale",
      "The Action was based on an earlier Room Head.",
      "retry",
      true,
    ))).toEqual({
      state: "rejected",
      code: "action_stale",
      message: "The Action was based on an earlier Room Head.",
      retryable: true,
    });
  });

  it("bounds an unknown Action submission failure to fixed safe copy", () => {
    expect(readInspectorActionFailure(new Error("http://internal/rooms/secret bearer wsb1:secret"))).toEqual({
      state: "rejected",
      code: "action_submission_failed",
      message: "The Action could not be submitted. Reconnect before trying again.",
      retryable: null,
    });
  });

  it("redacts authority-shaped text even when wrapped in a handoff error", () => {
    expect(readInspectorActionFailure(new ParticipantHandoffError(
      "room_id_mismatch",
      "Retry with wsb1:must-never-render at http://internal/api/room.",
      "return_to_task_setup",
      false,
    ))).toEqual({
      state: "rejected",
      code: "action_rejected",
      message: "The Action was rejected.",
      retryable: false,
    });
  });

  it("sends expired handoffs back to the CLI without coupling the client to Studio", () => {
    const markup = renderToStaticMarkup(<ParticipantHandoffView
      state={{
        state: "setup_required",
        action: "return_to_task_setup",
        message: "This participant client session is missing or expired.",
      }}
      onReconnect={vi.fn()}
      onAct={vi.fn()}
    />);
    expect(markup).toContain(
      "Ask the Host Operator to run worldstreamctl client open again for this Room setup operation.",
    );
    expect(markup).not.toMatch(/Studio|control credential|wsb1:/i);
  });
});
