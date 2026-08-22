import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { completeFixture, resultFixture } from "./fixture";
import { initialLiveSessionView, projectLiveSessionMessage } from "./liveSession";
import { MAX_MESSAGE_BYTES, WORLDSTREAM_PROTOCOL } from "./transport";

const id = (last: string) => `01J0000000000000000000000${last}`.slice(0, 26);
const welcome = {
  protocol: WORLDSTREAM_PROTOCOL,
  type: "server.welcome" as const,
  message_id: id("01"),
  body: {
    session_id: id("02"),
    selected_protocol: WORLDSTREAM_PROTOCOL,
    server_version: "privacy-matrix",
    heartbeat_interval_ms: 60_000,
    maximum_message_bytes: MAX_MESSAGE_BYTES,
    authenticated_principal: { principal_id: "principal", kind: "agent" as const },
  },
};

function resetWithHiddenFields() {
  return {
    ...welcome,
    type: "projection.reset" as const,
    body: {
      room_head: { room_seq: 21, genesis_or_transition_hash: "blake3:head" },
      projection: {
        activity: {
          phase: "Commitment",
          phase_generation: 8,
          phase_deadline: "2026-08-21T12:00:00Z",
          public_claims: [{ clue_id: "route", claim_code: "route_public" }],
          commitment_count: 2,
          private_clues: ["navigator-private-clue"],
          own_commitment: "sealed-value",
        },
        action_offers: [{ action_type: "commit_move", payload_schema_digest: "blake3:offer" }],
      },
    },
  };
}

describe("Agent Heist privacy and golden matrix", () => {
  it("keeps reset, catch-up, error, and UI payloads independent of ignored private fields", () => {
    const reset = projectLiveSessionMessage(initialLiveSessionView(resultFixture), resetWithHiddenFields(), resultFixture);
    const cleanReset = projectLiveSessionMessage(initialLiveSessionView(resultFixture), {
      ...resetWithHiddenFields(),
      body: { ...resetWithHiddenFields().body, projection: { activity: { phase: "Commitment", phase_generation: 8, phase_deadline: "2026-08-21T12:00:00Z", public_claims: [{ clue_id: "route", claim_code: "route_public" }], commitment_count: 2 }, action_offers: [{ action_type: "commit_move", payload_schema_digest: "blake3:offer" }] } },
    }, resultFixture);
    expect(reset.fixture).toEqual(cleanReset.fixture);
    expect(JSON.stringify(reset.fixture)).not.toMatch(/navigator-private-clue|sealed-value/);

    const delivered = projectLiveSessionMessage(reset, {
      ...welcome,
      type: "observation.deliver" as const,
      body: { frame_seq: 22, observation: { reason: "timer_fired", private_clues: ["hidden"] } },
    }, resultFixture);
    expect(delivered.fixture).toEqual(projectLiveSessionMessage(cleanReset, {
      ...welcome,
      type: "observation.deliver" as const,
      body: { frame_seq: 22, observation: { reason: "timer_fired" } },
    }, resultFixture).fixture);

    const errored = projectLiveSessionMessage(delivered, { ...welcome, type: "error", body: { code: "slow_consumer", message: "private-value", retryable: true } }, resultFixture);
    expect(errored.error).toBe("The live WorldStream session reported an error.");
    expect(JSON.stringify(errored.fixture)).not.toContain("private-value");
  });

  it("keeps public/operator/replay DOM independent while participant ownership remains explicit", () => {
    const paired = {
      ...resultFixture,
      participant: { ...resultFixture.participant, offers: resultFixture.participant.offers.map((offer) => ({ ...offer, eligibility: "owner-only-private-change" })) },
    };
    expect(renderToStaticMarkup(<App fixture={paired} />)).toBe(renderToStaticMarkup(<App fixture={resultFixture} />));
    expect(renderToStaticMarkup(<App fixture={paired} initialView="operator" />)).toBe(renderToStaticMarkup(<App fixture={resultFixture} initialView="operator" />));
    expect(renderToStaticMarkup(<App fixture={paired} initialView="replay" />)).toBe(renderToStaticMarkup(<App fixture={resultFixture} initialView="replay" />));
    expect(renderToStaticMarkup(<App fixture={paired} initialView="participant" />)).toContain("owner-only-private-change");
  });

  it("makes final reveal terminal-only and keeps terminal UI free of private payloads", () => {
    const before = renderToStaticMarkup(<App initialView="replay" fixture={resultFixture} />);
    const after = renderToStaticMarkup(<App initialView="replay" fixture={completeFixture} />);
    expect(before).toContain("Locked until the Activity Phase is Complete");
    expect(before).not.toContain(">Available</span>");
    expect(after).toContain("The activity is terminal");
    expect(after).toContain(">Available</span>");
    expect(after).not.toMatch(/navigator-private-clue|sealed-value|Bearer /);
  });
});
