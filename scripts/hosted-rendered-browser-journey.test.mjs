import assert from "node:assert/strict";
import test from "node:test";

import {
  HOSTED_RENDERED_BROWSER_JOURNEY_SCHEMA,
  launchIdFromUrl,
  localProductOrigin,
  navigatorPlanForRouteClaim,
  publicRunPath,
  runHostedRenderedBrowserJourney,
  validateTimeouts,
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
  const id = "10000000-0000-4000-8000-000000000001";
  assert.equal(launchIdFromUrl(`http://127.0.0.1:5180/launches/${id}`), id);
  assert.throws(() => launchIdFromUrl("http://127.0.0.1:5180/launches/not-an-id"));
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
