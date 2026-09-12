import assert from "node:assert/strict";
import test from "node:test";

import {
  HOSTED_RENDERED_BROWSER_JOURNEY_SCHEMA,
  activityClientBootstrapDiagnostic,
  attachRenderedNavigatorBeforeStart,
  browserConsoleFailure,
  browserRequestFailure,
  ensureRenderedNavigatorRouteClaim,
  hasRenderedHouseEndorsement,
  hasExplicitRenderedStaleRoom,
  hasRenderedOwnCommitment,
  isReviewedRenderedActivityClientUrl,
  launchIdFromUrl,
  localProductOrigin,
  navigatorPlanForRouteClaim,
  parseRenderedCommitmentCount,
  parseRenderedMissionBasis,
  publicRunPath,
  reconnectRenderedCommitment,
  remainingRenderedActionDeadlineMs,
  remainingRenderedDeadlineMs,
  resumeRetainedRenderedNavigator,
  renderedActionForm,
  renderedCommitmentDeadlines,
  renderedCrewCommitmentCompleted,
  renderedFreshCommitmentBasis,
  renderedHouseCommitmentMissed,
  renderedNavigatorPlan,
  renderedParticipantActionForms,
  renderedSameCommitmentWindow,
  retryRenderedStaleAction,
  runHostedRenderedBrowserJourney,
  sameOriginBrowserResponseFailure,
  submitRenderedForm,
  submitRenderedRecoverableAction,
  validateLaunchId,
  validateTimeouts,
  verifiedMyGamesLaunch,
  waitForHouseEndorsement,
  waitForIndependentActivityClient,
  waitForRenderedActionOutcome,
  waitForRenderedCrewCommitments,
  waitForRenderedTerminalComplete,
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
    url: `http://127.0.0.1:5180/agent-heist-v8/hosted/?ignored=value#handoff=wsh1:${"a".repeat(64)}`,
    heading: "Waiting for authorized Projection",
    cause: `network rejected Bearer ${"b".repeat(64)}`,
    failures: [
      `participant console error: wst1:${"c".repeat(64)}`,
      ...Array.from({ length: 10 }, (_, index) => `request-${index}`),
    ],
  });
  assert.match(diagnostic, /url=http:\/\/127\.0\.0\.1:5180\/agent-heist-v8\/hosted\//u);
  assert.match(diagnostic, /heading=Waiting for authorized Projection/u);
  assert.match(diagnostic, /wst1:\[redacted\]/u);
  assert.match(diagnostic, /Bearer \[redacted\]/u);
  assert.doesNotMatch(diagnostic, /handoff=|\?ignored=|wsh1:[0-9a-f]{64}|wst1:[0-9a-f]{64}/u);
  assert.doesNotMatch(diagnostic, /request-7/u);
});

test("rendered-client bootstrap diagnostics retain only same-origin failed response status and path", () => {
  const page = `http://127.0.0.1:5180/agent-heist-v8/hosted/#handoff=wsh1:${"a".repeat(64)}`;
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

test("rendered-client bootstrap proves the current Mission Focus authorization cards", async () => {
  for (const expected of [
    {
      diagnosticRole: "Navigator participant",
      selector: "section.role-card.role-navigator",
      label: "Your role",
      heading: "Navigator",
      clientUrl: "http://127.0.0.1:5180/agent-heist-v12/hosted/?platform_return=%2F",
    },
    {
      diagnosticRole: "Spectator view",
      selector: "section.role-card.spectator-card",
      label: "Public spectator view",
      heading: "Follow the crew",
      clientUrl: `http://127.0.0.1:5180/agent-heist-v12/hosted/?public_run=${"a".repeat(32)}&platform_return=%2F&platform_result=%2Fruns%2F${"a".repeat(32)}`,
    },
  ]) {
    const observed = [];
    const ready = (name) => ({
      async waitFor(options) {
        observed.push([name, options]);
      },
    });
    const card = {
      getByText(value, options) {
        assert.equal(value, expected.label);
        assert.deepEqual(options, { exact: true });
        return ready("authorization-label");
      },
      getByRole(role, options) {
        assert.equal(role, "heading");
        assert.deepEqual(options, { name: expected.heading, exact: true });
        return ready("authorization-heading");
      },
    };
    const page = {
      async waitForURL(predicate, options) {
        assert.equal(predicate(new URL(expected.clientUrl)), true);
        observed.push(["url", options]);
      },
      getByRole(role, options) {
        assert.equal(role, "heading");
        assert.deepEqual(options, { name: "Agent Heist" });
        return ready("client-heading");
      },
      getByText() {
        throw new Error("bootstrap must scope authorization checks to the current role card");
      },
      locator(selector) {
        assert.equal(selector, expected.selector);
        return card;
      },
      url() {
        return expected.clientUrl;
      },
    };

    await waitForIndependentActivityClient(
      page,
      1_000,
      expected.diagnosticRole,
      [],
      "http://127.0.0.1:5180",
    );
    assert.deepEqual(observed, [
      ["url", { timeout: 1_000 }],
      ["client-heading", { timeout: 1_000 }],
      ["authorization-label", { timeout: 1_000 }],
      ["authorization-heading", { timeout: 1_000 }],
    ]);
  }
});

test("rendered-client bootstrap rejects a lookalike origin or retained client route", async () => {
  const ready = () => ({ async waitFor() {} });
  const roleCard = {
    getByText() { return ready(); },
    getByRole() { return ready(); },
  };
  for (const unexpectedUrl of [
    "https://lookalike.invalid/agent-heist-v12/hosted/?platform_return=%2F",
    "http://127.0.0.1:5180/agent-heist-v11/hosted/?platform_return=%2F",
    "http://127.0.0.1:5180/agent-heist-v12/hosted/?platform_return=%2F&unexpected=true",
  ]) {
    const page = {
      async waitForURL(matcher) {
        const matched = typeof matcher === "function"
          ? matcher(new URL(unexpectedUrl))
          : matcher.test(unexpectedUrl);
        if (!matched) throw new Error("URL wait did not match");
      },
      getByRole() { return ready(); },
      locator(selector) {
        if (selector === "main h1") return { first() { return { async textContent() { return "Agent Heist"; } }; } };
        return roleCard;
      },
      url() { return unexpectedUrl; },
    };
    await assert.rejects(
      () => waitForIndependentActivityClient(
        page,
        1_000,
        "Navigator participant",
        [],
        "http://127.0.0.1:5180",
      ),
      /independent Activity Client bootstrap did not become ready/u,
    );
  }
});

test("rendered-client bootstrap accepts only platform-issued participant and spectator queries", () => {
  const origin = "http://127.0.0.1:5180";
  const publicId = "a".repeat(32);
  assert.equal(isReviewedRenderedActivityClientUrl(
    `${origin}/agent-heist-v12/hosted/?platform_return=%2F`,
    origin,
    "Navigator participant",
  ), true);
  assert.equal(isReviewedRenderedActivityClientUrl(
    `${origin}/agent-heist-v12/hosted/?public_run=${publicId}&platform_return=%2F&platform_result=%2Fruns%2F${publicId}`,
    origin,
    "Spectator view",
  ), true);
  for (const invalid of [
    `${origin}/agent-heist-v12/hosted/`,
    `${origin}/agent-heist-v12/hosted/?platform_return=%2F&unexpected=true`,
    `${origin}/agent-heist-v12/hosted/?platform_return=%2F#handoff=wsh1:${"b".repeat(64)}`,
    `${origin}/agent-heist-v12/hosted/?public_run=${publicId}&platform_return=%2F&platform_result=%2Fruns%2F${"c".repeat(32)}`,
  ]) {
    assert.equal(isReviewedRenderedActivityClientUrl(invalid, origin, "Navigator participant"), false);
    assert.equal(isReviewedRenderedActivityClientUrl(invalid, origin, "Spectator view"), false);
  }
});

test("rendered launch reuses the readiness popup for gameplay before waiting-room Start commits", async () => {
  const observed = [];
  const ready = (name) => ({
    async waitFor(options) {
      observed.push([name, options]);
    },
  });
  const roleCard = {
    getByText(value, options) {
      assert.equal(value, "Your role");
      assert.deepEqual(options, { exact: true });
      return ready("navigator-label");
    },
    getByRole(role, options) {
      assert.equal(role, "heading");
      assert.deepEqual(options, { name: "Navigator", exact: true });
      return ready("navigator-heading");
    },
  };
  const popup = {
    on(event) { observed.push(["popup-listener", event]); },
    async waitForURL(predicate, options) {
      assert.equal(predicate(new URL("http://127.0.0.1:5180/agent-heist-v12/hosted/?platform_return=%2F")), true);
      observed.push(["popup-url", options]);
    },
    getByRole(role, options) {
      assert.equal(role, "heading");
      assert.deepEqual(options, { name: "Agent Heist" });
      return ready("popup-heading");
    },
    locator(selector) {
      if (selector === "div.mission-connection") {
        return {
          getByText(value, options) {
            assert.equal(value, "Mission link live");
            assert.deepEqual(options, { exact: true });
            return ready("participant-live");
          },
        };
      }
      assert.equal(selector, "section.role-card.role-navigator");
      return roleCard;
    },
    url() { return "http://127.0.0.1:5180/agent-heist-v12/hosted/?platform_return=%2F"; },
  };
  const openNavigator = {
    first() { return this; },
    async waitFor(options) { observed.push(["open-visible", options]); },
    async click() { observed.push(["open-click"]); },
  };
  const waitingRoom = {
    async waitForEvent(event, options) {
      assert.equal(event, "popup");
      observed.push(["wait-popup", options]);
      return popup;
    },
    getByRole(role, options) {
      if (role === "button") {
        assert.deepEqual(options, { name: "Open Navigator to sync" });
        return openNavigator;
      }
      assert.equal(role, "heading");
      assert.deepEqual(options, { name: "Your activity is ready" });
      return ready("waiting-room-ready");
    },
  };

  const participant = await attachRenderedNavigatorBeforeStart({
    waitingRoom,
    productOrigin: "http://127.0.0.1:5180",
    actionTimeoutMs: 1_000,
    formationTimeoutMs: 2_000,
    failures: [],
  });

  assert.equal(participant, popup);
  assert.deepEqual(observed, [
    ["open-visible", { timeout: 2_000 }],
    ["wait-popup", { timeout: 1_000 }],
    ["open-click"],
    ["popup-listener", "pageerror"],
    ["popup-listener", "console"],
    ["popup-listener", "requestfailed"],
    ["popup-listener", "response"],
    ["popup-url", { timeout: 1_000 }],
    ["popup-heading", { timeout: 1_000 }],
    ["navigator-label", { timeout: 1_000 }],
    ["navigator-heading", { timeout: 1_000 }],
    ["participant-live", { timeout: 1_000 }],
    ["waiting-room-ready", { timeout: 2_000 }],
  ]);
});

test("retained rendered recovery synchronizes a pre-start Navigator", async () => {
  const observed = [];
  const ready = (name) => ({
    async waitFor(options) { observed.push([name, options]); },
  });
  const popup = {
    on(event) { observed.push(["popup-listener", event]); },
    async waitForURL(_pattern, options) { observed.push(["popup-url", options]); },
    getByRole(role, options) {
      assert.equal(role, "heading");
      assert.deepEqual(options, { name: "Agent Heist" });
      return ready("popup-heading");
    },
    locator(selector) {
      if (selector === "div.mission-connection") {
        return { getByText: () => ready("participant-live") };
      }
      assert.equal(selector, "section.role-card.role-navigator");
      return {
        getByText: () => ready("navigator-label"),
        getByRole: () => ready("navigator-heading"),
      };
    },
    url() { return "http://127.0.0.1:5180/agent-heist-v12/hosted/?platform_return=%2F"; },
  };
  const openNavigator = {
    first() { return this; },
    async waitFor(options) { observed.push(["open-visible", options]); },
    async innerText() { return "Open Navigator to sync"; },
    async click() { observed.push(["open-click"]); },
  };
  let lookup = 0;
  const waitingRoom = {
    async waitForEvent(_event, options) {
      observed.push(["wait-popup", options]);
      return popup;
    },
    getByRole(role, options) {
      if (role === "button") {
        lookup += 1;
        assert.match(String(options.name), /Open Navigator to sync/u);
        return openNavigator;
      }
      return ready("waiting-room-ready");
    },
  };

  const participant = await resumeRetainedRenderedNavigator({
    waitingRoom,
    productOrigin: "http://127.0.0.1:5180",
    actionTimeoutMs: 1_000,
    formationTimeoutMs: 2_000,
    failures: [],
  });

  assert.equal(participant, popup);
  assert.equal(lookup, 2);
  assert.deepEqual(observed.filter(([name]) => name === "open-visible"), [
    ["open-visible", { timeout: 2_000 }],
    ["open-visible", { timeout: 2_000 }],
  ]);
  assert.deepEqual(observed.filter(([name]) => name === "waiting-room-ready"), [
    ["waiting-room-ready", { timeout: 2_000 }],
  ]);
});

test("retained rendered recovery preserves ordinary post-start entry", async () => {
  const observed = [];
  const entryAction = {
    first() { return this; },
    async waitFor(options) { observed.push(["entry-visible", options]); },
    async innerText() { return "Enter Navigator"; },
  };
  const waitingRoom = {
    getByRole(role) {
      return role === "button"
        ? entryAction
        : { async waitFor(options) { observed.push(["ready-heading", options]); } };
    },
  };

  const participant = await resumeRetainedRenderedNavigator({
    waitingRoom,
    productOrigin: "http://127.0.0.1:5180",
    actionTimeoutMs: 1_000,
    formationTimeoutMs: 2_000,
    failures: [],
  });

  assert.equal(participant, waitingRoom);
  assert.deepEqual(observed, [
    ["entry-visible", { timeout: 2_000 }],
    ["ready-heading", { timeout: 1_000 }],
  ]);
});

test("retained rendered recovery retries Start before synchronizing its Navigator", async () => {
  const observed = [];
  const popup = {
    on(event) { observed.push(["popup-listener", event]); },
    async waitForURL(_pattern, options) { observed.push(["popup-url", options]); },
    getByRole() { return { async waitFor(options) { observed.push(["popup-heading", options]); } }; },
    locator(selector) {
      if (selector === "div.mission-connection") {
        return { getByText: () => ({ async waitFor(options) { observed.push(["participant-live", options]); } }) };
      }
      return {
        getByText: () => ({ async waitFor(options) { observed.push(["navigator-label", options]); } }),
        getByRole: () => ({ async waitFor(options) { observed.push(["navigator-heading", options]); } }),
      };
    },
    url() { return "http://127.0.0.1:5180/agent-heist-v12/hosted/?platform_return=%2F"; },
  };
  const startAction = {
    first() { return this; },
    async waitFor(options) { observed.push(["start-visible", options]); },
    async innerText() { return "Start activity"; },
    async click() { observed.push(["start-click"]); },
  };
  const openAction = {
    async waitFor(options) { observed.push(["open-visible", options]); },
    async click() { observed.push(["open-click"]); },
  };
  const waitingRoom = {
    async waitForEvent(_event, options) {
      observed.push(["wait-popup", options]);
      return popup;
    },
    getByRole(role, options) {
      if (role === "heading") {
        return { async waitFor(waitOptions) { observed.push(["ready-heading", waitOptions]); } };
      }
      return String(options.name).includes("Start activity") ? startAction : openAction;
    },
  };

  const participant = await resumeRetainedRenderedNavigator({
    waitingRoom,
    productOrigin: "http://127.0.0.1:5180",
    actionTimeoutMs: 1_000,
    formationTimeoutMs: 2_000,
    failures: [],
  });

  assert.equal(participant, popup);
  assert.deepEqual(observed.slice(0, 5), [
    ["start-visible", { timeout: 2_000 }],
    ["start-click"],
    ["open-visible", { timeout: 2_000 }],
    ["wait-popup", { timeout: 1_000 }],
    ["open-click"],
  ]);
});

test("rendered journey scopes actions and spectator checks to the Mission Focus surface", () => {
  const form = {};
  const filteredSection = {
    locator(selector) {
      assert.equal(selector, "form.mission-action-surface");
      return form;
    },
  };
  const section = {
    filter(options) {
      assert.equal(options.has.value, "Open a dossier");
      return filteredSection;
    },
  };
  const participantForms = {};
  const page = {
    locator(selector) {
      if (selector === "section#mission-action") return section;
      assert.equal(selector, "#mission-action form.mission-action-surface");
      return participantForms;
    },
    getByRole(role, options) {
      assert.equal(role, "heading");
      assert.deepEqual(options, { name: "Open a dossier", exact: true });
      return { value: options.name };
    },
  };
  assert.equal(renderedActionForm(page, "Open a dossier"), form);
  assert.equal(renderedParticipantActionForms(page), participantForms);
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
  await submitRenderedForm(form, "Open a dossier", 1_000, async () => {
    assert.equal(clicked, true);
    // The fake intentionally has no getByRole/status API: a committed frame
    // may already have unmounted its local form before React writes a notice.
  });
});

test("rendered Action recomputes every nested timeout from one absolute deadline", async () => {
  let now = 1_000;
  const observed = [];
  const form = {
    locator(selector) {
      assert.equal(selector, 'button[type="submit"]');
      return {
        async waitFor(options) {
          observed.push(["wait", options.timeout]);
          now = 1_090;
        },
        async isEnabled(options) {
          observed.push(["enabled", options.timeout]);
          now = 1_120;
          return true;
        },
        async click(options) {
          observed.push(["click", options.timeout]);
        },
      };
    },
  };
  await submitRenderedForm(
    form,
    "Seal your choice",
    300,
    async () => {},
    { deadlineMs: 1_300, now: () => now },
  );
  assert.deepEqual(observed, [
    ["wait", 300],
    ["enabled", 210],
    ["click", 180],
  ]);
});

test("rendered Action acceptance requires a durable Projection postcondition", async () => {
  await assert.rejects(
    () => submitRenderedForm({}, "Open a dossier", 1_000),
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
    () => submitRenderedForm(form, "Open a dossier", 1_000, async () => {
      throw new Error("transport rejected");
    }),
    /did not produce its durable authorized Projection\/result postcondition/u,
  );
});

test("rendered exact-head Action reconnects visibly and refills a fresh form after an explicit stale rejection", async () => {
  let roomSequence = 7;
  let stale = false;
  let live = true;
  let committed = false;
  let submits = 0;
  let reconnects = 0;
  let preparations = 0;
  const action = {
    async waitFor() {},
    async isEnabled() { return live; },
    async click() {
      submits += 1;
      if (submits === 1) {
        stale = true;
        live = false;
      } else {
        committed = true;
      }
    },
  };
  const form = {
    async count() { return 1; },
    async waitFor() {},
    locator(selector) {
      assert.equal(selector, 'button[type="submit"]');
      return action;
    },
  };
  const page = {
    getByText(value) { return { value }; },
    getByRole(role, options) {
      if (role === "heading") return { value: options.name };
      assert.equal(role, "button");
      assert.deepEqual(options, { name: "Reconnect", exact: true });
      return {
        async count() { return stale ? 1 : 0; },
        async waitFor() { assert.equal(stale, true); },
        async click() {
          reconnects += 1;
          stale = false;
          live = true;
          roomSequence = 8;
        },
      };
    },
    locator(selector) {
      if (selector === "main.mission-focus-shell") return {
        async evaluate() {
          return {
            roomSequence: String(roomSequence),
            phase: "briefing",
            phaseGeneration: "1",
            phaseDeadline: "2026-09-12T12:01:00Z",
          };
        },
      };
      if (selector === ".mission-connection") return {
        filter({ has }) {
          return {
            async count() {
              return has.value === "Mission link live"
                ? Number(live)
                : Number(stale);
            },
          };
        },
      };
      if (selector === "nav.mission-moves") return {
        getByRole() { return { async count() { return 0; } }; },
      };
      assert.equal(selector, "section#mission-action");
      return {
        filter() {
          return {
            locator(innerSelector) {
              assert.equal(innerSelector, "form.mission-action-surface");
              return form;
            },
          };
        },
      };
    },
  };
  await submitRenderedRecoverableAction(
    page,
    "Open a dossier",
    1_000,
    async (current) => {
      assert.equal(current, form);
      preparations += 1;
    },
    async () => committed,
  );
  assert.deepEqual({ submits, reconnects, preparations, roomSequence }, {
    submits: 2,
    reconnects: 1,
    preparations: 2,
    roomSequence: 8,
  });
});

test("rendered Action outcome accepts only a durable result or the explicit stale boundary", async () => {
  let now = 1_000;
  let committed = false;
  const stalePage = {
    locator(selector) {
      assert.equal(selector, ".mission-connection");
      return { filter() { return { async count() { return 1; } }; } };
    },
    getByText() { return {}; },
    getByRole() { return { async count() { return 1; } }; },
  };
  assert.equal(await waitForRenderedActionOutcome(
    stalePage,
    async () => committed,
    1_200,
    { now: () => now, wait: async () => { now += 10; } },
  ), "stale");
  committed = true;
  assert.equal(await waitForRenderedActionOutcome(
    stalePage,
    async () => committed,
    1_200,
    { now: () => now, wait: async () => { now += 10; } },
  ), "committed");
  assert.equal(remainingRenderedActionDeadlineMs(1_200, 1_050), 150);
  assert.throws(
    () => remainingRenderedActionDeadlineMs(1_200, 1_200),
    /Action deadline closed/u,
  );
});

test("resumed rendered journey reads its humanized Mission Focus Route claim without reopening a dossier", async () => {
  const existingClue = {
    filter() { return this; },
    async count() { return 1; },
    locator(selector) {
      assert.equal(selector, "strong");
      return { async textContent() { return "Route Canal"; } };
    },
  };
  const page = {
    locator(selector) {
      if (selector === ".private-intel .known-clue") return existingClue;
      throw new Error("a resumed Route claim must not reopen its dossier");
    },
    getByText(value, options) {
      assert.equal(value, "Route");
      assert.deepEqual(options, { exact: true });
      return {};
    },
  };
  assert.equal(await ensureRenderedNavigatorRouteClaim(page, 1_000), "route_canal");
});

test("rendered journey waits for terminal Complete without claiming an acknowledgement", async () => {
  let waited = false;
  const page = {
    locator(selector) {
      assert.equal(selector, ".mission-action .no-action");
      return {
        filter(options) {
          assert.deepEqual(options, { hasText: "This operation is complete." });
          return {
            async waitFor(waitOptions) {
              assert.deepEqual(waitOptions, { state: "visible", timeout: 1_000 });
              waited = true;
            },
          };
        },
      };
    },
  };
  await waitForRenderedTerminalComplete(page, 1_000);
  assert.equal(waited, true);
});

test("rendered Navigator plan postcondition requires the Navigator proposer", () => {
  const expectedTexts = [];
  const locator = {
    filter(options) {
      expectedTexts.push(options.has?.value ?? options.hasText);
      return this;
    },
  };
  const page = {
    locator(selector) {
      assert.equal(selector, ".crew-plan-list article");
      return locator;
    },
    getByText(value, options) {
      assert.equal(options, undefined);
      return { value };
    },
  };
  assert.equal(renderedNavigatorPlan(page, navigatorPlanForRouteClaim("route_canal")), locator);
  assert.equal(expectedTexts[0].test("Plan 1 · Navigator"), true);
  assert.deepEqual(expectedTexts.slice(1), ["Canal · Late", "Disguise → Van"]);
});

test("rendered House endorsement is scoped to the exact Navigator plan", async () => {
  const expectedTexts = [];
  const endorsement = {
    async allTextContents() { return ["1 backed · 0 flagged"]; },
  };
  const exactPlan = {
    filter(options) {
      expectedTexts.push(options.has?.value ?? options.hasText);
      return this;
    },
    locator(selector) {
      assert.equal(selector, "small");
      return endorsement;
    },
  };
  const page = {
    locator(selector) {
      assert.equal(selector, ".crew-plan-list article");
      return exactPlan;
    },
    getByText(value, options) {
      assert.equal(options, undefined);
      return { value };
    },
  };
  await waitForHouseEndorsement(page, navigatorPlanForRouteClaim("route_canal"), 1_000);
  assert.equal(expectedTexts[0].test("Plan 4 · Navigator"), true);
  assert.deepEqual(expectedTexts.slice(1), ["Canal · Late", "Disguise → Van"]);
  assert.equal(hasRenderedHouseEndorsement(["0 backed · 0 flagged"]), false);
  assert.equal(hasRenderedHouseEndorsement(["1 endorsements"]), false);
});

test("rendered commitment requires the private Navigator receipt and the full crew aggregate", async () => {
  const housePending = parseRenderedCommitmentCount("2 of 3");
  const completed = parseRenderedCommitmentCount("3 of 3");
  assert.deepEqual(housePending, { current: 2, total: 3 });
  assert.equal(renderedCrewCommitmentCompleted(0, completed), false);
  assert.equal(renderedCrewCommitmentCompleted(1, housePending), false);
  assert.equal(renderedCrewCommitmentCompleted(1, completed), true);
  assert.equal(renderedHouseCommitmentMissed("commitment", housePending), false);
  assert.equal(renderedHouseCommitmentMissed("resolution", housePending), true);
  assert.equal(renderedHouseCommitmentMissed("result", housePending), true);
  assert.equal(renderedHouseCommitmentMissed("complete", completed), false);
  assert.equal(await hasRenderedOwnCommitment({
    locator(selector) {
      assert.equal(selector, ".own-commitment");
      return { async count() { return 1; } };
    },
  }), true);
  for (const invalid of [null, "", "2/3", "4 of 3", "1 of 0"]) {
    assert.throws(() => parseRenderedCommitmentCount(invalid), /commitment count is invalid/u);
  }
});

test("rendered commitment keeps one absolute deadline across a stale reconnect", async () => {
  let now = 1_000;
  const deadlines = [];
  let attempts = 0;
  await retryRenderedStaleAction(
    async (deadlineMs) => {
      deadlines.push(["attempt", deadlineMs]);
      now += 90;
      attempts += 1;
      return attempts === 1 ? "stale" : "committed";
    },
    async (deadlineMs) => {
      deadlines.push(["reconnect", deadlineMs]);
      now += 90;
    },
    { deadlineMs: 1_300, now: () => now },
  );
  assert.deepEqual(deadlines, [
    ["attempt", 1_300],
    ["reconnect", 1_300],
    ["attempt", 1_300],
  ]);

  let expiredNow = 2_000;
  let reconnects = 0;
  await assert.rejects(
    () => retryRenderedStaleAction(
      async (deadlineMs) => {
        assert.equal(deadlineMs, 2_100);
        expiredNow = 2_100;
        return "stale";
      },
      async () => { reconnects += 1; },
      { deadlineMs: 2_100, now: () => expiredNow },
    ),
    /commitment window closed/u,
  );
  assert.equal(reconnects, 0);
});

test("rendered commitment derives a bounded window from the authoritative phase deadline", () => {
  const deadlines = renderedCommitmentDeadlines("1970-01-01T00:00:21.000Z", 1_000);
  assert.deepEqual(deadlines, { actionDeadlineMs: 21_000, observationDeadlineMs: 23_000 });
  const capped = renderedCommitmentDeadlines("1970-01-01T00:02:00.000Z", 1_000);
  assert.deepEqual(capped, { actionDeadlineMs: 31_000, observationDeadlineMs: 33_000 });
  assert.equal(remainingRenderedDeadlineMs(capped.actionDeadlineMs, 1_125), 29_875);
  assert.throws(
    () => remainingRenderedDeadlineMs(capped.actionDeadlineMs, capped.actionDeadlineMs),
    /commitment window closed/u,
  );
  assert.throws(
    () => renderedCommitmentDeadlines("not-a-deadline", 1_000),
    /commitment deadline is invalid/u,
  );
});

test("rendered House timeout retains the last exact aggregate without a final locator read", async () => {
  let now = 1_000;
  let snapshotReads = 0;
  const page = {
    locator(selector) {
      assert.equal(selector, "main.mission-focus-shell");
      return {
        async evaluate(_callback, _argument, options) {
          snapshotReads += 1;
          assert.deepEqual(options, { timeout: 100 });
          return {
            roomSequence: "8",
            phase: "commitment",
            phaseGeneration: "5",
            phaseDeadline: "1970-01-01T00:00:01.100Z",
            commitmentCount: "2 of 3",
            ownCommitmentCount: 1,
          };
        },
      };
    },
  };
  await assert.rejects(
    () => waitForRenderedCrewCommitments(page, 1_100, {
      now: () => now,
      wait: async () => { now = 1_100; },
    }),
    /House seats missed the 30-second commitment deadline \(2 of 3 sealed\)/u,
  );
  assert.equal(snapshotReads, 1);
});

test("rendered House completion cannot mix a stale aggregate with a newer resolution basis", async () => {
  let now = 1_000;
  const snapshots = [{
    roomSequence: "8",
    phase: "commitment",
    phaseGeneration: "5",
    phaseDeadline: "1970-01-01T00:00:01.100Z",
    commitmentCount: "2 of 3",
    ownCommitmentCount: 1,
  }, {
    roomSequence: "9",
    phase: "resolution",
    phaseGeneration: "6",
    phaseDeadline: "1970-01-01T00:00:01.101Z",
    commitmentCount: "3 of 3",
    ownCommitmentCount: 1,
  }];
  let snapshotReads = 0;
  const page = {
    locator(selector) {
      assert.equal(selector, "main.mission-focus-shell");
      return {
        async evaluate() {
          const snapshot = snapshots[snapshotReads];
          snapshotReads += 1;
          return snapshot;
        },
      };
    },
  };
  await waitForRenderedCrewCommitments(page, 1_100, {
    now: () => now,
    wait: async () => { now += 25; },
  });
  assert.equal(snapshotReads, 2);
});

test("rendered reconnect requires a newer Room basis in the same commitment window", () => {
  const expected = parseRenderedMissionBasis({
    roomSequence: "7",
    phase: "commitment",
    phaseGeneration: "5",
    phaseDeadline: "2026-08-15T12:03:00Z",
  });
  const fresh = { ...expected, roomSequence: 8 };
  assert.equal(renderedSameCommitmentWindow(expected, fresh), true);
  assert.equal(renderedFreshCommitmentBasis(expected, expected, 7), false);
  assert.equal(renderedFreshCommitmentBasis(expected, fresh, 7), true);
  assert.equal(renderedFreshCommitmentBasis(expected, { ...fresh, phase: "resolution" }, 7), false);
  assert.equal(renderedFreshCommitmentBasis(expected, { ...fresh, phaseGeneration: 6 }, 7), false);
  assert.equal(renderedFreshCommitmentBasis(expected, { ...fresh, phaseDeadline: "2026-08-15T12:03:01Z" }, 7), false);
  assert.throws(
    () => parseRenderedMissionBasis({ roomSequence: "07", phase: "commitment", phaseGeneration: "5", phaseDeadline: null }),
    /synchronization basis is invalid/u,
  );
});

test("rendered reconnect recomputes waits against the original absolute deadline", async () => {
  let now = 1_000;
  const observed = [];
  const expected = parseRenderedMissionBasis({
    roomSequence: "7",
    phase: "commitment",
    phaseGeneration: "5",
    phaseDeadline: "2026-08-15T12:03:00Z",
  });
  const action = {
    async isEnabled(options) {
      observed.push(["enabled", options.timeout]);
      return true;
    },
  };
  const form = {
    async count() { return 1; },
    locator(selector) {
      assert.equal(selector, 'button[type="submit"]');
      return action;
    },
  };
  const reconnect = {
    async waitFor(options) {
      observed.push(["wait", options.timeout]);
      now = 1_050;
    },
    async click(options) {
      observed.push(["click", options.timeout]);
      now = 1_100;
    },
  };
  const page = {
    getByRole(role) {
      return role === "button" ? reconnect : {};
    },
    getByText() { return {}; },
    locator(selector) {
      if (selector === "main.mission-focus-shell") return {
        async evaluate(_callback, _argument, options) {
          observed.push(["snapshot", options.timeout]);
          now = 1_150;
          return {
            roomSequence: "8",
            phase: "commitment",
            phaseGeneration: "5",
            phaseDeadline: "2026-08-15T12:03:00Z",
            commitmentCount: "2 of 3",
            ownCommitmentCount: 1,
          };
        },
      };
      if (selector === ".mission-connection") return {
        filter() { return { async count() { return 1; } }; },
      };
      assert.equal(selector, "section#mission-action");
      return {
        filter() {
          return {
            locator(innerSelector) {
              assert.equal(innerSelector, "form.mission-action-surface");
              return form;
            },
          };
        },
      };
    },
  };
  await reconnectRenderedCommitment(page, expected, 7, 1_300, {
    now: () => now,
  });
  assert.deepEqual(observed, [
    ["wait", 300],
    ["click", 250],
    ["snapshot", 200],
    ["enabled", 150],
  ]);
});

test("rendered stale Room boundary requires both the exact message and Reconnect control", async () => {
  const fixture = (messageCount, reconnectCount) => ({
    locator(selector) {
      assert.equal(selector, ".mission-connection");
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
