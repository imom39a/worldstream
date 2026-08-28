import assert from "node:assert/strict";
import test from "node:test";

import { renderedBodyText } from "./playwright_browser_adapter.mjs";

test("Playwright evidence uses rendered text so visible replay labels survive markup", async () => {
  const page = {
    locator(selector) {
      assert.equal(selector, "body");
      return {
        async innerText() {
          return "Verified Canonical History at sequence 2\nLineage hash: blake3:" + "a".repeat(64);
        },
      };
    },
  };
  assert.match(await renderedBodyText(page), /Lineage hash:\s*blake3:a{64}/);
});
