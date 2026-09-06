import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { AgentHeistClientView } from "./AgentHeistClientView";
import type { AgentHeistReadyState } from "./liveAdapter";

const hash = (character: string) => `blake3:${character.repeat(64)}`;

function readyState(): AgentHeistReadyState {
  return {
    kind: "ready",
    pack: {
      id: "worldstream.agent-heist",
      version: "0.1.0",
      digest: hash("a"),
    },
    roomHead: {
      genesisOrTransitionHash: hash("1"),
      authoritativeStateHash: hash("2"),
    },
    roomSequence: 7,
    frameHead: 9,
    authorization: { accessMode: "participant", role: "navigator" },
    projection: {
      phase: "negotiation",
      phaseGeneration: 2,
      phaseStart: "2026-08-15T12:00:30Z",
      phaseDeadline: "2026-08-15T12:02:00Z",
      seats: [
        { role: "navigator", present: true },
        { role: "insider", present: true },
        { role: "broker", present: false },
      ],
      publicClaims: [{ clueId: "route", claimCode: "route_public" }],
      plans: [{
        planId: "plan-a",
        proposerRole: "navigator",
        route: "service",
        entryWindow: "early",
        requiredTool: "thermal_key",
        extraction: "boat",
        endorsements: 1,
        challenges: 0,
      }],
      challenges: [],
      commitmentCount: 0,
      outcome: null,
      privateClues: [{ clueId: "route", ownerRole: "navigator", claimCode: "navigator-only-value" }],
      ownCommitment: null,
      addressedOffers: [{
        offerId: "exchange-1",
        senderRole: "broker",
        offeredClueId: "route",
        considerationKind: "plan_endorsement",
        considerationId: "plan-a",
        status: "open",
      }],
    },
    offers: [{
      offerId: "7:propose_plan:0",
      actionType: "propose_plan",
      schemaDigest: hash("b"),
      eligibility: "Current synchronized phase",
    }],
  };
}

describe("standalone Agent Heist live participant surface", () => {
  it("renders the authorized participant workspace without demo lenses or raw protocol JSON", () => {
    const markup = renderToStaticMarkup(
      <AgentHeistClientView state={readyState()} connection="live" onAct={vi.fn()} />,
    );

    expect(markup).toContain("Agent Heist");
    expect(markup).toContain("Navigator participant");
    expect(markup).toContain("navigator-only-value");
    expect(markup).toContain("propose_plan");
    expect(markup).toContain("plan endorsement: plan-a");
    expect(markup).toContain("based_on_room_seq");
    expect(markup).toContain("7");
    expect(markup).not.toMatch(/Select Projection|Operator diagnostics|Public board|Recorded fixture|Fixture mode/);
    expect(markup).not.toMatch(/fixture/i);
    expect(markup).not.toContain("<pre");
    expect(markup).not.toMatch(/room_id|member_id|Bearer\s|wsh1:|wsb1:/);
  });

  it("renders no action or domain content before an authorized Reset", () => {
    const markup = renderToStaticMarkup(
      <AgentHeistClientView state={{ kind: "awaiting" }} connection="connecting" onAct={vi.fn()} />,
    );

    expect(markup).toContain("Waiting for authorized Projection");
    expect(markup).not.toContain("navigator-only-value");
    expect(markup).not.toContain("propose_plan");
    expect(markup).not.toMatch(/fixture/i);
  });

  it("fails closed on incompatible Pack data", () => {
    const markup = renderToStaticMarkup(
      <AgentHeistClientView
        state={{ kind: "incompatible", reason: "This client does not support the pinned Activity Pack Revision." }}
        connection="live"
        onAct={vi.fn()}
      />,
    );

    expect(markup).toContain("Client incompatible");
    expect(markup).not.toContain("Submit action");
    expect(markup).not.toContain("Authorized private clue");
  });

  it("disables exact Actions while the retained session is not live", () => {
    const markup = renderToStaticMarkup(
      <AgentHeistClientView state={readyState()} connection="disconnected" onAct={vi.fn()} />,
    );

    expect(markup).toContain("Reconnect before acting");
    expect(markup).toContain("disabled");
  });

  it("keeps Actions disabled while a live transport has no current action grant", () => {
    const markup = renderToStaticMarkup(
      <AgentHeistClientView
        state={readyState()}
        connection="live"
        actionsEnabled={false}
        agentAssist="available"
        onAct={vi.fn()}
      />,
    );

    expect(markup).toContain("Agent tools ready");
    expect(markup).toContain("Reconnect before acting");
    expect(markup).toContain("disabled");
  });

  it("renders the authorized spectator surface without participant-private controls", () => {
    const state = readyState();
    const markup = renderToStaticMarkup(
      <AgentHeistClientView
        state={{
          ...state,
          authorization: { accessMode: "spectator", role: null },
          projection: { ...state.projection, privateClues: [], addressedOffers: [] },
          offers: [],
        }}
        connection="live"
        onAct={vi.fn()}
      />,
    );

    expect(markup).toContain("Authorized spectator surface");
    expect(markup).toContain("Public spectator view");
    expect(markup).not.toContain("navigator-only-value");
    expect(markup).not.toContain("Current Actions");
  });

  it("renders the revision 0.2 Lobby without inventing a participant launch Action", () => {
    const state = readyState();
    const markup = renderToStaticMarkup(
      <AgentHeistClientView
        state={{
          ...state,
          pack: { ...state.pack, version: "0.2.0", digest: hash("b") },
          projection: { ...state.projection, phase: "lobby" },
          offers: [],
        }}
        connection="live"
        onAct={vi.fn()}
      />,
    );

    expect(markup).toContain("Lobby");
    expect(markup).toContain("No Action is offered at this synchronized Head.");
    expect(markup).not.toContain("Submit action");
    expect(markup).not.toContain("Action payload (JSON)");
  });
});
