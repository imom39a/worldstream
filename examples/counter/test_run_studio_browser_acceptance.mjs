import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { createCodexBrowserAdapter } from "./codex_browser_adapter.mjs";
import {
  assertCountsRemainStable,
  protectedConsoleCallbacks,
  SidecarVerificationError,
  waitForCompletedEvidence,
} from "./run_studio_browser_acceptance.mjs";

test("driver starts request capture before it claims the protected popup without URL recursion", async () => {
  const studio = { id: 4, playwright: { async domSnapshot() { return "Studio"; } } };
  const popup = { id: 9, title: "WorldStream Client Host" };
  let tabs = [studio];
  let captures = 0;
  const adapter = createCodexBrowserAdapter({
    browser: {
      user: {
        async openTabs() { return tabs; },
        async claimTab() {
          return { id: 9, title: "WorldStream Client Host", playwright: { async domSnapshot() { return "Console"; } } };
        },
      },
    },
    studioTab: studio,
    fixture: {},
    async beginRequestCapture() { captures += 1; return { generation: captures }; },
    async captureTab(_tab, capture) { return capture; },
    async requestUrlsFor(tab, capture) {
      assert.equal(capture.generation, 1);
      return [`http://127.0.0.1:517${tab.id}/`];
    },
    async currentUrlFor(tab) { return tab.id === 9 ? "http://127.0.0.1:5173/" : "http://127.0.0.1:5174/"; },
  });
  let consoleTab;
  const callbacks = protectedConsoleCallbacks(adapter, (tab) => { consoleTab = tab; });
  await callbacks.beforeOpenProtectedConsole();
  tabs = [studio, popup];
  await callbacks.openProtectedConsole();
  assert.equal(captures, 1);
  assert.deepEqual(await consoleTab.observedRequestUrls(), ["http://127.0.0.1:5179/"]);
  assert.deepEqual(await adapter.observedRequestUrls(consoleTab), ["http://127.0.0.1:5179/"]);
});

test("completed restart requires a stable provider observation window", async () => {
  let reads = 0;
  await assertCountsRemainStable(
    async () => {
      reads += 1;
      return [2, 2];
    },
    [2, 2],
    { observeMs: 20, pollMs: 5 },
  );
  assert.ok(reads >= 2);
  await assert.rejects(
    assertCountsRemainStable(async () => [3, 2], [2, 2], { observeMs: 20, pollMs: 5 }),
    /completed_host_restart_contacted_provider/,
  );
});

test("completion evidence retries only known durable-readiness states and reports a safe reason", async () => {
  let attempts = 0;
  const report = await waitForCompletedEvidence(
    async () => {
      attempts += 1;
      if (attempts < 3) throw new SidecarVerificationError("completion_receipt_count_invalid");
      return { status: "passed" };
    },
    { timeoutMs: 100, pollMs: 1 },
  );
  assert.deepEqual(report, { status: "passed" });
  await assert.rejects(
    waitForCompletedEvidence(
      async () => { throw new SidecarVerificationError("secret_absence_scan_failed"); },
      { timeoutMs: 100, pollMs: 1 },
    ),
    /studio_acceptance_sidecar_failed:secret_absence_scan_failed/,
  );
  await assert.rejects(
    waitForCompletedEvidence(
      async () => { throw new SidecarVerificationError("increment_action_receipt_missing"); },
      { timeoutMs: 2, pollMs: 1 },
    ),
    /managed_host_completion_receipt_not_stable:increment_action_receipt_missing/,
  );
});

test("driver imports in the Codex Node realm without a global process", () => {
  const moduleUrl = pathToFileURL(new URL("./run_studio_browser_acceptance.mjs", import.meta.url).pathname).href;
  execFileSync(
    process.execPath,
    ["--input-type=module", "--eval", `globalThis.process = undefined; await import(${JSON.stringify(moduleUrl)});`],
    { stdio: "pipe" },
  );
});
