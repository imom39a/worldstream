import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { assertHeistFixtureParity, completeFixture, heistParity, resultFixture } from "./fixture";

describe("Agent Heist fixture console DOM", () => {
  it("shares the offline story transcript, six phases, and replay digest", () => {
    expect(() => assertHeistFixtureParity(resultFixture)).not.toThrow();
    expect(resultFixture.parity.phasePath).toEqual(heistParity.phase_path);
    expect(resultFixture.parity.replayTransitionCount).toBe(15);
    expect(resultFixture.parity.replayVerified).toBe(true);
    expect(resultFixture.parity.transcriptDigest).toBe(
      "sha256:4432882d59fc4883e36b405518092dc71242ed6ff4856fafc4a9f6f17286ac36",
    );
  });

  it("fails closed if any Replay component or Transition hash drifts", () => {
    expect(resultFixture.replay.hashes.coreStateHash).toBe(heistParity.retained_executor.replay_hashes.core_state_hash);
    expect(resultFixture.replay.hashes.activityStateHash).toBe(heistParity.retained_executor.replay_hashes.activity_state_hash);
    expect(resultFixture.replay.hashes.aggregateStateHash).toBe(heistParity.retained_executor.replay_hashes.authoritative_state_hash);
    expect(resultFixture.replay.hashes.lineageHash).toBe(heistParity.retained_executor.replay_hashes.lineage_hash);
    expect(resultFixture.replay.hashes.transitionHashes).toEqual(heistParity.retained_executor.replay_hashes.transition_hashes);
    const tampered = { ...resultFixture, replay: { ...resultFixture.replay, hashes: { ...resultFixture.replay.hashes, transitionHashes: ["sha256:tampered"] } } };
    expect(() => assertHeistFixtureParity(tampered)).toThrow("Replay component or Transition hashes");
  });

  it("keeps the public projection noninterfering and free of private payloads", () => {
    const dom = renderToStaticMarkup(<App />);

    expect(dom).toContain("Public projection");
    expect(dom).toContain("5/5");
    expect(dom).toContain("individual commitments remain withheld");
    expect(dom).not.toContain("Propose a structured plan");
    expect(dom).not.toContain("schema:plan/");
    expect(dom).not.toContain("navigator-private-clue");
    expect(dom).not.toContain("sealed-value");
    expect(dom).not.toContain("Bearer ");
    expect(dom).not.toContain("chain-of-thought");
  });

  it("renders bounded operator diagnostics without participant projection data", () => {
    const dom = renderToStaticMarkup(<App initialView="operator" />);

    expect(dom).toContain("Bounded diagnostics");
    expect(dom).toContain("Redacted membership facts");
    expect(dom).toContain("Runner &amp; Activation");
    expect(dom).toContain("head:sha256:···7d91");
    expect(dom).not.toContain("Canal lift / service window");
    expect(dom).not.toContain("Endorse a published plan");
    expect(dom).not.toContain("commitment values");
  });

  it("makes exact-head, recovery, fault, and quarantine states visible", () => {
    const dom = renderToStaticMarkup(<App initialView="participant" />);

    expect(dom).toContain("Exact head");
    expect(dom).toContain("Stale head");
    expect(dom).toContain("Resync required");
    expect(dom).toContain("Catching up");
    expect(dom).toContain("Faulted");
    expect(dom).toContain("Quarantined");
    expect(dom).toContain("Room integrity states");
    expect(dom).toContain("disabled");
    expect(dom).toContain("Fixture-gated controls");
  });

  it("keeps replay read-only and final reveal locked before Complete", () => {
    const dom = renderToStaticMarkup(<App initialView="replay" fixture={resultFixture} />);

    expect(dom).toContain("Replay · read-only");
    expect(dom).toContain("Mutation controls unavailable");
    expect(dom).toContain("Present authorization · Authorized");
    expect(dom).toContain("Historical authorization · Public projection authorized");
    expect(dom).toContain("Canonical story replay");
    expect(dom).toContain("Final reveal");
    expect(dom).toContain("Locked until the Activity Phase is Complete");
    expect(dom).not.toContain("Available to an authorized current membership");
  });

  it("only marks final reveal available for a Complete fixture", () => {
    const dom = renderToStaticMarkup(<App initialView="replay" fixture={completeFixture} />);

    expect(dom).toContain("The activity is terminal");
    expect(dom).toContain(">Available</span>");
    expect(dom).toContain("after Complete");
    expect(dom).not.toContain("Locked until the Activity Phase is Complete");
    expect(dom).not.toContain("navigator-private-clue");
  });

  it("renders discovery, membership status, and explicit unavailable runtime states", () => {
    const dom = renderToStaticMarkup(<App initialView="participant" />);

    expect(dom).toContain("Room and membership scope");
    expect(dom).toContain("Scoped seats");
    expect(dom).toContain("Navigator");
    expect(dom).toContain("Participant");
    expect(dom).toContain("No server session is configured");
    expect(dom).toContain("Loading");
    expect(dom).toContain("Catching up");
    expect(dom).toContain("Active");
    expect(dom).toContain("Passivating");
    expect(dom).toContain("Inactive");
    expect(dom).toContain("Projection reset");
    expect(dom).toContain("Unavailable");
    expect(dom).toContain("Phase generation 5");
  });

  it("renders faulted and quarantined integrity surfaces without enabling actions", () => {
    for (const roomHealth of ["Faulted", "Quarantined"] as const) {
      const dom = renderToStaticMarkup(
        <App
          fixture={{
            ...resultFixture,
            runtime: { ...resultFixture.runtime, roomHealth },
          }}
          initialView="participant"
        />,
      );
      expect(dom).toContain(roomHealth);
      if (roomHealth === "Faulted") {
        expect(dom).toContain("Canonical mutation remains disabled");
        expect(dom).toContain("Submit unavailable");
      } else {
        expect(dom).toContain("Participant Projection unavailable while Quarantined");
        expect(dom).not.toContain("Submit unavailable");
      }
    }
  });

  it("removes normal Projection and participant offers while Quarantined", () => {
    const quarantined = {
      ...resultFixture,
      runtime: { ...resultFixture.runtime, roomHealth: "Quarantined" as const },
    };
    const publicDom = renderToStaticMarkup(<App fixture={quarantined} />);
    const participantDom = renderToStaticMarkup(<App fixture={quarantined} initialView="participant" />);

    expect(publicDom).toContain("Public Projection unavailable while Quarantined");
    expect(publicDom).not.toContain("Canal lift / service window");
    expect(participantDom).toContain("Participant Projection unavailable while Quarantined");
    expect(participantDom).not.toContain("Propose a structured plan");
    expect(participantDom).toContain("Quarantined · fail closed");
  });

  it("does not render a stale participant offer surface during catch-up", () => {
    const dom = renderToStaticMarkup(
      <App
        fixture={{ ...resultFixture, runtime: { ...resultFixture.runtime, recovery: "CatchingUp" as const } }}
        initialView="participant"
      />,
    );

    expect(dom).toContain("Current Action Offers withheld");
    expect(dom).not.toContain("Propose a structured plan");
  });

  it("does not render stale public or Replay bytes while Loading or Catching up", () => {
    for (const recovery of ["Loading", "CatchingUp"] as const) {
      const label = recovery === "Loading" ? "Loading" : "Catching up";
      const publicDom = renderToStaticMarkup(<App fixture={{ ...resultFixture, runtime: { ...resultFixture.runtime, recovery } }} />);
      const replayDom = renderToStaticMarkup(<App fixture={{ ...resultFixture, runtime: { ...resultFixture.runtime, recovery } }} initialView="replay" />);
      expect(publicDom).toContain("Public Projection unavailable while " + label);
      expect(publicDom).not.toContain("Canal lift / service window");
      expect(replayDom).toContain("Historical Replay unavailable while " + label);
      expect(replayDom).not.toContain("Canonical story replay");
    }
  });

  it("renders typed Action Offer inputs without inventing a live submission", () => {
    const dom = renderToStaticMarkup(<App initialView="participant" />);

    expect(dom).toContain("Draft payload");
    expect(dom).toContain('name="route"');
    expect(dom).toContain('name="plan_id"');
    expect(dom).toContain('name="reason"');
    expect(dom).toContain("based_on_room_seq");
    expect(dom).toContain("Action ID generated only by a live client");
    expect(dom).toContain("Submit unavailable");
    expect(dom).toContain("Any stale response requires a new synchronized Action ID");
    expect(dom).toContain("round_result_available");
    expect(dom).toContain("deduplication");
    expect(dom).not.toContain("action.accepted");
  });

  it("keeps faulted runtime metadata distinct from healthy fixture data", () => {
    const fixture = {
      ...resultFixture,
      runtime: {
        ...resultFixture.runtime,
        transport: "Disconnected" as const,
        recovery: "Active" as const,
        roomHealth: "Faulted" as const,
        reason: "The last verified projection is retained; canonical mutation is disabled.",
        frame: {
          ...resultFixture.runtime.frame,
          delivery: "Reset required" as const,
          reset: "Required" as const,
        },
      },
    };
    const dom = renderToStaticMarkup(<App fixture={fixture} initialView="participant" />);

    expect(dom).toContain("The last verified projection is retained");
    expect(dom).toContain("Faulted");
    expect(dom).toContain("Reset required");
    expect(dom).toContain("Canonical mutation remains disabled");
  });
});
