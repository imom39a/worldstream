import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { allowsFreshLaunchRetry, CatalogPage, friendlyError } from "./CatalogPage";

describe("hosted activity catalog shell", () => {
  it("describes temporary service failure without blaming the participant choices", () => {
    expect(friendlyError(new Error("temporarily_unavailable")))
      .toBe("The live room service is temporarily unavailable.");
    expect(friendlyError(new Error("formation_unavailable")))
      .toBe("This setup could not continue. Retry it, or close it from My games and start again.");
    expect(friendlyError(new Error("activity_capacity_unavailable")))
      .toBe("Active activity capacity is in use. Open My games to continue or close the existing activity.");
  });

  it("rotates a retained setup key only after the old launch is terminal", () => {
    expect(allowsFreshLaunchRetry("closing")).toBe(false);
    expect(allowsFreshLaunchRetry("reconciling")).toBe(false);
    expect(allowsFreshLaunchRetry("run_created")).toBe(false);
    expect(allowsFreshLaunchRetry("closed_by_creator")).toBe(true);
    expect(allowsFreshLaunchRetry("failed_pre_genesis")).toBe(true);
  });

  it("keeps discovery independent of any one Activity Pack before catalog data arrives", () => {
    const markup = renderToStaticMarkup(createElement(CatalogPage, { onNavigate: () => undefined }));

    expect(markup).toContain("A playground for people + agents");
    expect(markup).toContain("Pick your world");
    expect(markup).toContain("Play yourself. Or bring your agent.");
    expect(markup).toContain("Finding available activities");
    expect(markup.match(/<h1[^>]*>(.*?)<\/h1>/u)?.[1]).not.toMatch(/heist|crew|mission/iu);
    expect(markup).not.toContain("operation-hero");
    expect(markup).not.toMatch(/Pack digest|Role identifier|setup operation|Inspector fallback/giu);
  });
});
