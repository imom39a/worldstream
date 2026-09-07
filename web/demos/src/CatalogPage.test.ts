import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { CatalogPage } from "./CatalogPage";

describe("hosted activity catalog shell", () => {
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
