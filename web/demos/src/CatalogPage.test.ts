import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { CatalogPage } from "./CatalogPage";

describe("demo catalog card diagrams", () => {
  it("renders named process stages without legacy decorative shapes or repository status", () => {
    const markup = renderToStaticMarkup(createElement(CatalogPage, { onNavigate: () => undefined }));

    expect(markup.match(/class="visual-stage"/gu)).toHaveLength(6);
    expect(markup.match(/class="visual-arrow"/gu)).toHaveLength(4);
    expect(markup).toContain("Recorded Action-to-Observation flow for Room 017.");
    expect(markup).toContain("Planned proposal-to-signature flow that requires a persistent authority.");
    expect(markup).not.toMatch(/visual-room|visual-node|visual-line/gu);
    expect(markup).not.toMatch(/Source private|Private repository/giu);
  });
});
