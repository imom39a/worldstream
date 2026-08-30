import { describe, expect, it } from "vitest";

import { agentHeistRecords, visibleFixtureRecords } from "./agentHeistFixture";

describe("Agent Heist fixture inspector", () => {
  it("does not include participant-private clue data in public or operator views", () => {
    const publicText = visibleFixtureRecords(agentHeistRecords, "public", agentHeistRecords.length - 1)
      .map((record) => record.detail)
      .join(" ");
    const operatorText = visibleFixtureRecords(agentHeistRecords, "operator", agentHeistRecords.length - 1)
      .map((record) => record.detail)
      .join(" ");

    expect(publicText).not.toContain("Route clue value: service");
    expect(operatorText).not.toContain("Route clue value: service");
  });

  it("includes the authorized clue in the Navigator view", () => {
    const navigatorText = visibleFixtureRecords(agentHeistRecords, "navigator", agentHeistRecords.length - 1)
      .map((record) => record.detail)
      .join(" ");

    expect(navigatorText).toContain("Route clue value: service");
  });

  it("reveals only records at or before the selected playback step", () => {
    const visible = visibleFixtureRecords(agentHeistRecords, "navigator", 3);

    expect(visible.every((record) => record.fixtureIndex <= 3)).toBe(true);
    expect(visible.at(-1)?.roomSequence).toBe("0003");
  });
});
