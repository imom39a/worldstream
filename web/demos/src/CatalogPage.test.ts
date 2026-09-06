import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { CatalogPage } from "./CatalogPage";

describe("hosted activity catalog shell", () => {
  it("explains the Pack-neutral formation boundary before dynamic catalog data arrives", () => {
    const markup = renderToStaticMarkup(createElement(CatalogPage, { onNavigate: () => undefined }));

    expect(markup).toContain("Live activities for people and agents");
    expect(markup).toContain("Choose a reviewed activity");
    expect(markup).toContain("The platform forms the room. The activity owns the experience.");
    expect(markup).toContain("Loading reviewed activities");
    expect(markup).not.toMatch(/Pack digest|Role identifier|setup operation|Inspector fallback/giu);
  });
});
