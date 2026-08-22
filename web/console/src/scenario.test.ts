import { describe, expect, it } from "vitest";

import { fixtureForScenario, scenarioFromSearch } from "./scenario";

describe("credential-free browser scenarios", () => {
  it("selects each authorization view and fails closed for unknown query values", () => {
    expect(scenarioFromSearch("?scenario=healthy&view=operator")).toMatchObject({ name: "healthy", initialView: "operator" });
    expect(scenarioFromSearch("?scenario=unknown&view=unknown")).toMatchObject({ name: "healthy", initialView: "public" });
  });

  it("exposes recovery and integrity states without putting credentials in fixture data", () => {
    for (const name of ["loading", "catching-up", "faulted", "quarantined"] as const) {
      const fixture = fixtureForScenario(name);
      expect(fixture.runtime.reason).not.toContain("Bearer");
      if (name === "quarantined") expect(fixture.runtime.roomHealth).toBe("Quarantined");
      if (name === "faulted") expect(fixture.runtime.roomHealth).toBe("Faulted");
    }
  });

  it("keeps terminal reveal explicitly separate from healthy result state", () => {
    expect(fixtureForScenario("healthy").finalRevealAuthorized).toBe(false);
    expect(fixtureForScenario("complete").finalRevealAuthorized).toBe(true);
    expect(fixtureForScenario("complete").phase).toBe("Complete");
  });
});
