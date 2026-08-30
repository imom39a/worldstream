import { describe, expect, it } from "vitest";

import {
  agentHeistEvidence,
  agentHeistRecords,
  visibleFixtureRecords,
} from "./agentHeistFixture";

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

    expect(visible.every((record) => record.fixtureStep.value <= 3)).toBe(true);
    expect(visible.filter((record) => record.roomSequence !== null).at(-1)?.roomSequence?.value).toBe("0002");
  });

  it("uses exact retained parity identity for recorded evidence", () => {
    expect(agentHeistEvidence).toMatchObject({
      sourceSchema: "worldstream/imo-57-heist-parity/v1",
      packId: "worldstream.agent-heist",
      packVersion: "0.1.0",
      fixtureId: "service_window",
      finalRoomSequence: 15,
      replayVerified: true,
      verifiedTransitionCount: 15,
    });
    expect(agentHeistEvidence.revisionDigest).toMatch(/^blake3:[0-9a-f]{64}$/);
  });

  it("does not label reset or attention summaries with a Room sequence", () => {
    const nonTransitionRecords = agentHeistRecords.filter(
      (record) => record.kind === "Projection example" || record.kind === "Attention summary",
    );

    expect(nonTransitionRecords.length).toBeGreaterThan(0);
    expect(nonTransitionRecords.every((record) => record.roomSequence === null)).toBe(true);
  });
});
