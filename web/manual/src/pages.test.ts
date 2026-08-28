import { describe, expect, it } from "vitest";

import { manualPages, navigationGroups } from "./pages";

describe("manual page catalog", () => {
  it("has unique routes and substantial searchable sources", () => {
    expect(new Set(manualPages.map((page) => page.route)).size).toBe(manualPages.length);
    expect(manualPages.length).toBeGreaterThanOrEqual(28);
    expect(manualPages.every((page) => page.source.trim().length > 40)).toBe(true);
  });

  it("places every page in exactly one navigation group", () => {
    const routes = navigationGroups.flatMap((group) => group.items.map((page) => page.route));
    expect(routes).toHaveLength(manualPages.length);
    expect(new Set(routes)).toEqual(new Set(manualPages.map((page) => page.route)));
  });
});
