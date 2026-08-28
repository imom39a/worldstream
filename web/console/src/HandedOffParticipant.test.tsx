import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { ParticipantHandoffView } from "./HandedOffParticipant";

describe("handed-off Participant Console", () => {
  it("renders the authorized projection, observation, and exact offered Action controls", () => {
    const markup = renderToStaticMarkup(<ParticipantHandoffView
      state={{
        state: "live",
        action: "continue",
        message: null,
        observation: {
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
});
