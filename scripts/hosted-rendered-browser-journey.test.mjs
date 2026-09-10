import assert from "node:assert/strict";
import test from "node:test";

import {
  HOSTED_RENDERED_BROWSER_JOURNEY_SCHEMA,
  activityClientBootstrapDiagnostic,
  browserConsoleFailure,
  browserRequestFailure,
  ensureRenderedNavigatorRouteClaim,
  hasExplicitRenderedStaleRoom,
  launchIdFromUrl,
  localProductOrigin,
  navigatorPlanForRouteClaim,
  publicRunPath,
  renderedNavigatorPlan,
  retryRenderedStaleAction,
  runHostedRenderedBrowserJourney,
  sameOriginBrowserResponseFailure,
  submitRenderedForm,
  validateLaunchId,
  validateTimeouts,
  verifiedMyGamesLaunch,
  waitForRenderedResultAcknowledgementOpportunity,
} from "./hosted-rendered-browser-journey.mjs";

test("rendered-browser journey accepts only exact local product origins", () => {
  assert.equal(localProductOrigin("http://127.0.0.1:5180"), "http://127.0.0.1:5180");
  assert.equal(localProductOrigin("http://localhost:5180"), "http://localhost:5180");
  for (const value of [
    "https://worldstream-demos.vercel.app",
    "http://127.0.0.1:5180/path",
    "http://127.0.0.1:5180?unexpected=true",
    "http://example.test:5180",
  ]) {
    assert.throws(() => localProductOrigin(value), /exact loopback HTTP product origin/u);
  }
});

test("rendered-browser journey keeps formation and Action waits bounded", () => {
  assert.deepEqual(validateTimeouts({ actionTimeoutMs: 1_000, formationTimeoutMs: 300_000 }), {
    action: 1_000,
    formation: 300_000,
  });
  assert.throws(
    () => validateTimeouts({ actionTimeoutMs: 999, formationTimeoutMs: 1_000 }),
    /actionTimeoutMs/u,
  );
  assert.throws(
    () => validateTimeouts({ actionTimeoutMs: 1_000, formationTimeoutMs: 300_001 }),
    /formationTimeoutMs/u,
  );
});

test("rendered-browser journey recognizes only one waiting-room route", () => {
  const id = "a0000000-0000-4000-8000-000000000001";
  assert.equal(launchIdFromUrl(`http://127.0.0.1:5180/launches/${id}`), id);
  assert.equal(validateLaunchId(id), id);
  assert.throws(() => launchIdFromUrl("http://127.0.0.1:5180/launches/not-an-id"));
  for (const invalid of ["", "not-an-id", "10000000-0000-0000-8000-000000000001", id.toUpperCase()]) {
    assert.throws(() => validateLaunchId(invalid), /retained Launch identity/u);
  }
  assert.equal(HOSTED_RENDERED_BROWSER_JOURNEY_SCHEMA, "worldstream/hosted-rendered-browser-journey/v1");
});

test("rendered-browser journey keeps the public Run page on its exact URL", () => {
  const publicId = "a".repeat(32);
  assert.equal(
    publicRunPath("http://127.0.0.1:5180", publicId),
    `http://127.0.0.1:5180/runs/${publicId}`,
  );
  for (const invalid of ["", "not-a-public-id", "A".repeat(32), "a".repeat(31)]) {
    assert.throws(() => publicRunPath("http://127.0.0.1:5180", invalid), /public Run identity/u);
  }
});

test("retained recovery binds verified My Games evidence to the exact Launch", () => {
  const launchId = "a0000000-0000-4000-8000-000000000001";
  const publicId = "b".repeat(32);
  assert.equal(
    verifiedMyGamesLaunch({
      version: "platform_my_games.v1",
      items: [{ launch_id: launchId, state: "verified_result", action: "view_result", result_public_id: publicId }],
    }, launchId),
    publicId,
  );
  assert.throws(
    () => verifiedMyGamesLaunch({
      version: "platform_my_games.v0",
      items: [{ launch_id: launchId, state: "verified_result", action: "view_result", result_public_id: publicId }],
    }, launchId),
    /My Games response is invalid/u,
  );
  assert.throws(
    () => verifiedMyGamesLaunch({
      version: "platform_my_games.v1",
      items: [{ launch_id: "c0000000-0000-4000-8000-000000000003", state: "verified_result", action: "view_result", result_public_id: publicId }],
    }, launchId),
    /exactly one retained Launch/u,
  );
  assert.throws(
    () => verifiedMyGamesLaunch({
      version: "platform_my_games.v1",
      items: [{ launch_id: launchId, state: "publication_pending", action: "history", result_public_id: publicId }],
    }, launchId),
    /no verified result/u,
  );
});

test("rendered-client bootstrap failures retain only bounded, capability-safe browser diagnostics", () => {
  const diagnostic = activityClientBootstrapDiagnostic({
    expectedRole: "Navigator participant",
    url: `http://127.0.0.1:5180/agent-heist-v7/hosted/?ignored=value#handoff=wsh1:${"a".repeat(64)}`,
    heading: "Waiting for authorized Projection",
    cause: `network rejected Bearer ${"b".repeat(64)}`,
    failures: [
      `participant console error: wst1:${"c".repeat(64)}`,
      ...Array.from({ length: 10 }, (_, index) => `request-${index}`),
    ],
  });
  assert.match(diagnostic, /url=http:\/\/127\.0\.0\.1:5180\/agent-heist-v7\/hosted\//u);
  assert.match(diagnostic, /heading=Waiting for authorized Projection/u);
  assert.match(diagnostic, /wst1:\[redacted\]/u);
  assert.match(diagnostic, /Bearer \[redacted\]/u);
  assert.doesNotMatch(diagnostic, /handoff=|\?ignored=|wsh1:[0-9a-f]{64}|wst1:[0-9a-f]{64}/u);
  assert.doesNotMatch(diagnostic, /request-7/u);
});

test("rendered-client bootstrap diagnostics retain only same-origin failed response status and path", () => {
  const page = `http://127.0.0.1:5180/agent-heist-v7/hosted/#handoff=wsh1:${"a".repeat(64)}`;
  assert.equal(
    sameOriginBrowserResponseFailure(page, "http://127.0.0.1:5180/api/v1/participant-console/session:stream-ticket?ignored=true", 401),
    "response 401: http://127.0.0.1:5180/api/v1/participant-console/session:stream-ticket",
  );
  assert.equal(
    sameOriginBrowserResponseFailure(page, "http://127.0.0.1:5180/api/auth/session", 401),
    null,
  );
  assert.equal(sameOriginBrowserResponseFailure(page, "http://127.0.0.1:8080/v1/hosted/browser-stream", 403), null);
  assert.equal(sameOriginBrowserResponseFailure(page, "http://127.0.0.1:5180/api/auth/session", 200), null);
});

test("rendered-browser diagnostics ignore expected browser aborts and redundant resource console noise", () => {
  assert.equal(
    browserRequestFailure(
      `http://127.0.0.1:5180/api/runs/${"a".repeat(32)}`,
      "net::ERR_ABORTED",
    ),
    null,
  );
  assert.equal(
    browserRequestFailure(
      `http://127.0.0.1:5180/api/runs/${"a".repeat(32)}?ignored=true`,
      "net::ERR_CONNECTION_RESET",
    ),
    `request failed: http://127.0.0.1:5180/api/runs/${"a".repeat(32)}`,
  );
  assert.equal(
    browserConsoleFailure(
      "error",
      "Failed to load resource: the server responded with a status of 401 (Unauthorized)",
    ),
    null,
  );
  assert.equal(
    browserConsoleFailure("error", "Uncaught Error: participant session rejected"),
    "console error: Uncaught Error: participant session rejected",
  );
  assert.equal(browserConsoleFailure("warning", "not an error"), null);
});

test("rendered-browser qualification does not fall back when a browser is unavailable", async () => {
  await assert.rejects(
    () => runHostedRenderedBrowserJourney({
      productOrigin: "http://127.0.0.1:5180",
      launchBrowser: async () => { throw new Error("browser executable missing"); },
    }),
    /browser executable missing/u,
  );
});

test("rendered Navigator completion uses only the authorized Route claim", () => {
  assert.deepEqual(navigatorPlanForRouteClaim("route_canal"), {
    route: "canal", entry_window: "late", required_tool: "disguise", extraction: "van",
  });
  assert.deepEqual(navigatorPlanForRouteClaim("route_service"), {
    route: "service", entry_window: "early", required_tool: "thermal_key", extraction: "boat",
  });
  assert.deepEqual(navigatorPlanForRouteClaim("route_roof"), {
    route: "roof", entry_window: "middle", required_tool: "jammer", extraction: "motorbike",
  });
  assert.throws(() => navigatorPlanForRouteClaim("route_unknown"), /no reviewed completion plan/u);
  assert.throws(() => navigatorPlanForRouteClaim("not-a-claim"), /no reviewed completion plan/u);
});

test("rendered Action acceptance follows a durable Projection after its form is removed", async () => {
  let clicked = false;
  const form = {
    locator(selector) {
      assert.equal(selector, 'button[type="submit"]');
      return {
        async waitFor(options) {
          assert.deepEqual(options, { state: "visible", timeout: 1_000 });
        },
        async isEnabled() { return true; },
        async click() { clicked = true; },
      };
    },
  };
  await submitRenderedForm(form, "Inspect clue", 1_000, async () => {
    assert.equal(clicked, true);
    // The fake intentionally has no getByRole/status API: a committed frame
    // may already have unmounted its local form before React writes a notice.
  });
});

test("rendered Action acceptance requires a durable Projection postcondition", async () => {
  await assert.rejects(
    () => submitRenderedForm({}, "Inspect clue", 1_000),
    /lacks a durable postcondition/u,
  );
});

test("rendered Action acceptance fails closed when no durable Projection arrives", async () => {
  const form = {
    locator() {
      return {
        async waitFor() {},
        async isEnabled() { return true; },
        async click() {},
      };
    },
  };
  await assert.rejects(
    () => submitRenderedForm(form, "Inspect clue", 1_000, async () => {
      throw new Error("transport rejected");
    }),
    /did not produce its durable authorized Projection\/result postcondition/u,
  );
});

test("resumed rendered journey reads its existing authorized Route claim without replaying Inspect", async () => {
  const existingClue = {
    filter() { return this; },
    async count() { return 1; },
    locator(selector) {
      assert.equal(selector, "code");
      return { async innerText() { return "route_canal"; } };
    },
  };
  const page = {
    locator(selector) {
      if (selector === ".private-clue-list article") return existingClue;
      throw new Error("a resumed Route claim must not recreate an Inspect Action form");
    },
    getByText(value, options) {
      assert.equal(value, "route");
      assert.deepEqual(options, { exact: true });
      return {};
    },
  };
  assert.equal(await ensureRenderedNavigatorRouteClaim(page, 1_000), "route_canal");
});

test("rendered journey waits for a delayed result acknowledgement offer", async () => {
  let observedResult = false;
  const acknowledgement = {
    async count() { return observedResult ? 1 : 0; },
  };
  const complete = {
    async count() {
      observedResult = true;
      return 0;
    },
  };
  const page = {
    locator(selector) {
      if (selector === "form.live-action-form") {
        return { filter() { return acknowledgement; } };
      }
      if (selector === ".phase-window") {
        return { filter() { return { filter() { return complete; } }; } };
      }
      throw new Error(`unexpected locator ${selector}`);
    },
    getByText() { return {}; },
  };
  assert.equal(await waitForRenderedResultAcknowledgementOpportunity(page, 1_000), acknowledgement);
});

test("rendered Navigator plan postcondition requires the Navigator proposer", () => {
  const expectedTexts = [];
  const locator = {
    filter(options) {
      expectedTexts.push(options.has.value);
      return this;
    },
  };
  const page = {
    locator(selector) {
      assert.equal(selector, ".plan-grid article");
      return locator;
    },
    getByText(value, options) {
      assert.deepEqual(options, { exact: true });
      return { value };
    },
  };
  assert.equal(renderedNavigatorPlan(page, navigatorPlanForRouteClaim("route_canal")), locator);
  assert.deepEqual(expectedTexts, ["navigator", "canal · late", "disguise → van"]);
});

test("rendered commitment retries only after the explicit stale Room reconnect boundary", async () => {
  const attempts = [];
  let reconnects = 0;
  await retryRenderedStaleAction(
    async () => {
      attempts.push(attempts.length);
      return attempts.length === 1 ? "stale" : "committed";
    },
    async () => { reconnects += 1; },
  );
  assert.deepEqual(attempts, [0, 1]);
  assert.equal(reconnects, 1);
});

test("rendered stale Room boundary requires both the exact message and Reconnect control", async () => {
  const fixture = (messageCount, reconnectCount) => ({
    locator(selector) {
      assert.equal(selector, ".live-client-notice");
      return { filter() { return { async count() { return messageCount; } }; } };
    },
    getByText(value, options) {
      assert.equal(value, "The Room advanced. Reconnect to synchronize before acting.");
      assert.deepEqual(options, { exact: true });
      return {};
    },
    getByRole(role, options) {
      assert.equal(role, "button");
      assert.deepEqual(options, { name: "Reconnect", exact: true });
      return { async count() { return reconnectCount; } };
    },
  });
  assert.equal(await hasExplicitRenderedStaleRoom(fixture(1, 1)), true);
  assert.equal(await hasExplicitRenderedStaleRoom(fixture(1, 0)), false);
  assert.equal(await hasExplicitRenderedStaleRoom(fixture(0, 1)), false);
});

test("rendered commitment never retries a non-stale failure", async () => {
  let reconnects = 0;
  await assert.rejects(
    () => retryRenderedStaleAction(
      async () => { throw new Error("policy rejected"); },
      async () => { reconnects += 1; },
    ),
    /policy rejected/u,
  );
  assert.equal(reconnects, 0);
});
