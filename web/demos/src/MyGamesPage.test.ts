import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";

import { historyStatusDetail, MyGamesPage } from "./MyGamesPage";

it("renders a private My games entry point without exposing a participant credential", () => {
  const markup = renderToStaticMarkup(createElement(MyGamesPage, { onNavigate: () => undefined }));
  expect(markup).toContain("My games");
  expect(markup).toContain("Checking sign-in");
  expect(markup).not.toMatch(/entry_selector|membership_id|principal_id/iu);
});

it("describes creator closure without pretending that Genesis never happened", () => {
  const detail = historyStatusDetail("activity_closed", "human");
  expect(detail).toContain("cannot resume");
  expect(detail).not.toContain("before a Room was created");
});

it("describes an in-progress creator closure as safely retryable", () => {
  expect(historyStatusDetail("activity_closing", "human")).toContain("creator can safely retry");
});
