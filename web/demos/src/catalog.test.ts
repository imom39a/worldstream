import { describe, expect, it } from "vitest";

import { demos, filterDemos, getDemoById } from "./catalog";

describe("demo catalog", () => {
  it("returns the complete catalog for an empty filter", () => {
    expect(filterDemos(demos, { category: "all", query: "" }).map((demo) => demo.id)).toEqual([
      "agent-heist",
      "negotiate",
    ]);
  });

  it("combines a category filter with a capability search", () => {
    const result = filterDemos(demos, {
      category: "conformance",
      query: "  projection privacy ",
    });

    expect(result.map((demo) => demo.id)).toEqual(["agent-heist"]);
  });

  it("finds an available demo from its stable route identifier", () => {
    expect(getDemoById(demos, "agent-heist")?.route).toBe("/demos/agent-heist");
    expect(getDemoById(demos, "not-a-demo")).toBeUndefined();
  });
});
