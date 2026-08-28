import { describe, expect, it } from "vitest";
import { normalizeRoute, searchPages, type ManualPage } from "./manual";

const pages: ManualPage[] = [
  {
    route: "/studio/setup",
    title: "Studio setup",
    summary: "Run the local operator portal.",
    group: "Studio",
    source: "The Supervisor probes worldstreamd and preserves authority boundaries.",
  },
  {
    route: "/agents/mcp",
    title: "Generic MCP agent",
    summary: "Connect an external agent.",
    group: "Agents",
    source: "Consume observations, acknowledge frames, and submit exact Action Offers.",
  },
];

describe("manual routing and search", () => {
  it("accepts exact known routes and falls back to the manual home", () => {
    expect(normalizeRoute("#/studio/setup", pages)).toBe("/studio/setup");
    expect(normalizeRoute("#/missing", pages)).toBe("/");
    expect(normalizeRoute("", pages)).toBe("/");
  });

  it("searches titles, summaries, and documentation text case-insensitively", () => {
    expect(searchPages("STUDIO", pages).map((page) => page.route)).toEqual(["/studio/setup"]);
    expect(searchPages("Action Offers", pages).map((page) => page.route)).toEqual(["/agents/mcp"]);
    expect(searchPages("operator portal", pages).map((page) => page.route)).toEqual(["/studio/setup"]);
  });

  it("requires every query token and returns no results for an empty query", () => {
    expect(searchPages("external observations", pages).map((page) => page.route)).toEqual(["/agents/mcp"]);
    expect(searchPages("external postgres", pages)).toEqual([]);
    expect(searchPages("   ", pages)).toEqual([]);
  });
});
