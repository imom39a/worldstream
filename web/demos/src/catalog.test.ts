import { describe, expect, it } from "vitest";

import { defaultDemoCatalogFilter, demos, filterDemos, getDemoById } from "./catalog";

describe("demo catalog", () => {
  it("returns the complete catalog for an empty filter", () => {
    expect(filterDemos(demos, defaultDemoCatalogFilter).map((demo) => demo.id)).toEqual([
      "agent-heist",
      "negotiate",
    ]);
  });

  it("combines a category filter with a capability search", () => {
    const result = filterDemos(demos, {
      ...defaultDemoCatalogFilter,
      category: "technical-fixture",
      query: "  projection privacy ",
    });

    expect(result.map((demo) => demo.id)).toEqual(["agent-heist"]);
  });

  it("finds an available demo from its stable route identifier", () => {
    expect(getDemoById(demos, "agent-heist")?.route).toBe("/demos/agent-heist");
    expect(getDemoById(demos, "not-a-demo")).toBeUndefined();
  });

  it("combines Activity Pack, capability, experience, perspective, and availability facets", () => {
    const result = filterDemos(demos, {
      ...defaultDemoCatalogFilter,
      activityPack: "worldstream.negotiate",
      capability: "Evidence",
      experience: "live-shared-room",
      perspective: "integrator",
      availability: "planned",
    });

    expect(result.map((demo) => demo.id)).toEqual(["negotiate"]);
  });

  it("keeps the required technical metadata in each manifest entry", () => {
    const agentHeist = getDemoById(demos, "agent-heist");

    expect(agentHeist).toMatchObject({
      activityPack: {
        id: "worldstream.agent-heist",
        fixtureId: "service_window",
      },
      backendRequirement: { required: false },
      documentationRoute: "/demos/agent-heist/#what-this-shows",
      thumbnail: { kind: "css-diagram" },
    });
    expect(agentHeist?.buildIdentity?.revisionDigest).toMatch(/^blake3:[0-9a-f]{64}$/);
  });
});
