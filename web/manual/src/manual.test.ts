import { describe, expect, it } from "vitest";
import { normalizeRoute, searchPages, type ManualPage } from "./manual";

const pages: ManualPage[] = [
  {
    route: "/quickstart",
    title: "Local quickstart",
    summary: "Run the local authority from the CLI.",
    group: "Start here",
    source: "The Controller probes worldstreamd and preserves authority boundaries.",
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
    expect(normalizeRoute("#/quickstart", pages)).toBe("/quickstart");
    expect(normalizeRoute("#/missing", pages)).toBe("/");
    expect(normalizeRoute("", pages)).toBe("/");
  });

  it("searches titles, summaries, and documentation text case-insensitively", () => {
    expect(searchPages("CONTROLLER", pages).map((page) => page.route)).toEqual(["/quickstart"]);
    expect(searchPages("Action Offers", pages).map((page) => page.route)).toEqual(["/agents/mcp"]);
    expect(searchPages("local authority", pages).map((page) => page.route)).toEqual(["/quickstart"]);
  });

  it("requires every query token and returns no results for an empty query", () => {
    expect(searchPages("external observations", pages).map((page) => page.route)).toEqual(["/agents/mcp"]);
    expect(searchPages("external postgres", pages)).toEqual([]);
    expect(searchPages("   ", pages)).toEqual([]);
  });
});
