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

  it("keeps the recorded gallery explicitly separate from live Activity Clients", () => {
    const markup = renderToStaticMarkup(<App initialView="participant" />);

    expect(markup).toContain("Recorded fixture mode");
    expect(markup).toContain("standalone Activity Client routes");
    expect(markup).not.toMatch(/wsb1:|wst1:|Authorization/);
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
