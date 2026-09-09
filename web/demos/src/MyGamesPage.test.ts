import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";

import { MyGamesPage } from "./MyGamesPage";

it("renders a private My games entry point without exposing a participant credential", () => {
  const markup = renderToStaticMarkup(createElement(MyGamesPage, { onNavigate: () => undefined }));
  expect(markup).toContain("My games");
  expect(markup).toContain("Checking sign-in");
  expect(markup).not.toMatch(/entry_selector|membership_id|principal_id/iu);
});
