import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { resultFixture } from "./fixture";

describe("console browser/DOM privacy boundary", () => {
  it("keeps public and operator markup free of private offers, sealed values, raw state, and credentials", () => {
    const publicMarkup = renderToStaticMarkup(<App />);
    const operatorMarkup = renderToStaticMarkup(<App initialView="operator" />);
    const forbidden = /Propose a structured plan|schema:plan\/|navigator-private-clue|sealed-value|private_clue|own_commitment|chain-of-thought|Bearer\s|password|api[_-]?key|raw (?:core|activity) state/i;

    expect(publicMarkup).not.toMatch(forbidden);
    expect(operatorMarkup).not.toMatch(forbidden);
  });

  it("does not place a bearer or private projection value in an action payload", () => {
    const markup = renderToStaticMarkup(<App initialView="participant" />);

    expect(markup).toContain("Action ID generated only by a live client");
    expect(markup).not.toContain("Bearer ");
    expect(markup).not.toContain("sealed-value");
    expect(markup).not.toContain("navigator-private-clue");
  });

  it("keeps a runtime live-session bearer and browser ticket out of rendered DOM", () => {
    const bearer = "wsb1:dom-never-secret";
    const markup = renderToStaticMarkup(
      <App
        initialView="participant"
        liveSession={{
          endpoint: "wss://worldstream.test/v1/stream",
          clientName: "console",
          clientVersion: "test",
          roomId: "01J00000000000000000000001",
          memberId: "01J00000000000000000000002",
          bearer,
        }}
      />,
    );

    expect(markup).not.toContain(bearer);
    expect(markup).not.toContain("wst1:");
    expect(markup).not.toContain("Authorization");
  });

  it("fails closed in the browser-visible surface when Room integrity is Quarantined", () => {
    const markup = renderToStaticMarkup(
      <App fixture={{ ...resultFixture, runtime: { ...resultFixture.runtime, roomHealth: "Quarantined" as const } }} />,
    );

    expect(markup).toContain("Public Projection unavailable while Quarantined");
    expect(markup).toContain("Quarantined · fail closed");
    expect(markup).not.toContain("Canal lift / service window");
  });
});
