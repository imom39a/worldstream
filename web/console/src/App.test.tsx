import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { App } from "./App";

describe("operator shell", () => {
  it("does not imply that Room or storage behavior exists", () => {
    const markup = renderToStaticMarkup(<App />);

    expect(markup).toContain("Operator shell");
    expect(markup).toContain("not implemented");
  });
});
