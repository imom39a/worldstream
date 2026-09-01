import assert from "node:assert/strict";
import test from "node:test";

import { createCodexBrowserAdapter } from "./codex_browser_adapter.mjs";

test("Codex adapter claims exactly the popup opened from the selected Studio tab", async () => {
  const studio = { id: 4, playwright: { async domSnapshot() { return "Studio"; } } };
  const popup = { id: 9, title: "WorldStream Client Host" };
  let tabs = [studio];
  const browser = {
    user: {
      async openTabs() { return tabs; },
      async claimTab(id) {
        assert.equal(id, 9);
        return { id, playwright: { async domSnapshot() { return "Console"; } } };
      },
    },
  };
  const adapter = createCodexBrowserAdapter({
    browser, studioTab: studio, fixture: { profile: {} },
    async beginRequestCapture() { return { started: true }; },
    async captureTab(tab, capture) {
      assert.equal(tab.id, 9);
      assert.equal(capture.started, true);
      return { started: true, console: true };
    },
    async requestUrlsFor(tab, capture) {
      assert.equal(capture.started, true);
      if (tab.id === 9) assert.equal(capture.console, true);
      return [`http://127.0.0.1:517${tab.id}/`];
    },
    async currentUrlFor(tab) { return tab.id === 9 ? "http://127.0.0.1:5173/" : "http://127.0.0.1:5174/"; },
  });
  await adapter.beforeOpenProtectedConsole();
  tabs = [studio, popup];
  const claimed = await adapter.openProtectedConsole();
  assert.equal(claimed.id, 9);
  assert.deepEqual(await adapter.observedRequestUrls(studio), ["http://127.0.0.1:5174/"]);
  assert.equal(await adapter.currentUrl(studio), "http://127.0.0.1:5174/");
});

test("Codex adapter polls until one loaded Console popup can be claimed and captured", async () => {
  const studio = { id: 4, playwright: { async domSnapshot() { return "Studio"; } } };
  const blank = { id: 9, title: "" };
  const ready = { id: 9, title: "WorldStream Client Host" };
  const listings = [[studio], [studio, blank], [studio, ready]];
  let calls = 0;
  let claims = 0;
  const adapter = createCodexBrowserAdapter({
    browser: {
      user: {
        async openTabs() { return listings[Math.min(calls++, listings.length - 1)]; },
        async claimTab(id) {
          claims += 1;
          assert.equal(id, 9);
          return { id, title: "WorldStream Client Host", playwright: { async domSnapshot() { return "Console"; } } };
        },
      },
    },
    studioTab: studio,
    fixture: {},
    async beginRequestCapture() { return { target: "studio" }; },
    async captureTab(tab) { return { target: `tab-${tab.id}` }; },
    async requestUrlsFor() { return ["http://127.0.0.1:5173/"]; },
    async currentUrlFor(tab) { return tab.title ? "http://127.0.0.1:5173/" : "about:blank"; },
    popupTimeoutMs: 1_000,
    popupPollMs: 1,
  });
  await adapter.beforeOpenProtectedConsole();
  const claimed = await adapter.openProtectedConsole();
  assert.equal(claimed.id, 9);
  assert.equal(claims, 1);
  assert.ok(calls >= 3);
});

test("Codex adapter rejects a missing URL evidence source", () => {
  assert.throws(
    () => createCodexBrowserAdapter({ browser: {}, studioTab: {}, fixture: {} }),
    /beginRequestCapture/,
  );
});
