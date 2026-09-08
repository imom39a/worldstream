import assert from "node:assert/strict";
import { once } from "node:events";
import { test } from "node:test";

import {
  assertDevelopmentFakeOpenRouterAllowed,
  createDevelopmentFakeOpenRouter,
} from "./hosted-fake-openrouter.mjs";

const KEY = "worldstream-development-key-000000000000";

function environment(port = "18787") {
  return {
    WORLDSTREAM_DEVELOPMENT_FAKE_OPENROUTER: "visible-local-only",
    WORLDSTREAM_DEPLOYMENT_ENVIRONMENT: "development",
    WORLDSTREAM_FAKE_OPENROUTER_BIND: "127.0.0.1",
    WORLDSTREAM_FAKE_OPENROUTER_PORT: port,
    WORLDSTREAM_DEVELOPMENT_OPENROUTER_KEY: KEY,
  };
}

test("fake provider refuses production and non-loopback configuration", () => {
  assert.throws(() =>
    assertDevelopmentFakeOpenRouterAllowed(
      { ...environment(), NODE_ENV: "production" },
      "127.0.0.1",
      KEY,
    ),
  );
  assert.throws(() =>
    assertDevelopmentFakeOpenRouterAllowed(environment(), "0.0.0.0", KEY),
  );
});

test("fake House decisions respect the current role and its authorized clues", async () => {
  const { server } = createDevelopmentFakeOpenRouter(environment());
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  try {
    const origin = `http://127.0.0.1:${server.address().port}`;
    for (const [role, clue] of [["navigator", "route"], ["insider", "entry_window"], ["broker", "required_tool"]]) {
      const response = await fetch(`${origin}/api/v1/chat/completions`, {
        method: "POST", headers: { authorization: `Bearer ${KEY}`, "content-type": "application/json" },
        body: JSON.stringify({ model: "worldstream/development-house", provider: { only: ["fixture-provider"] },
          messages: [{ role: "user", content: JSON.stringify({ schema: "worldstream/house-model-invocation/v1",
            projection: { role, projection_reset: { projection: { activity: { private_clues: [] } } }, observations: [] },
            action_offers: { schema: "worldstream/assignment-action-offer-list/v1", offers: [{ offer_id: "inspect", action_type: "inspect_clue" }] },
          }) }] }),
      });
      assert.equal(response.status, 200);
      assert.deepEqual(JSON.parse((await response.json()).choices[0].message.content), { offer_id: "inspect", payload: { clue_id: clue } });
    }
  } finally { await new Promise(resolve => server.close(resolve)); }
});

test("fake provider exposes an authenticated deterministic OpenRouter-shaped response", async () => {
  const { server } = createDevelopmentFakeOpenRouter(environment());
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.ok(address && typeof address === "object");
  const origin = `http://127.0.0.1:${address.port}`;
  try {
    const health = await fetch(`${origin}/healthz`);
    assert.equal(health.status, 200);
    assert.equal(
      health.headers.get("x-worldstream-development-substitute"),
      "fake-openrouter",
    );

    const unauthorized = await fetch(`${origin}/api/v1/chat/completions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ model: "test", messages: [{ role: "user", content: "hello" }] }),
    });
    assert.equal(unauthorized.status, 401);

    const completion = await fetch(`${origin}/api/v1/chat/completions`, {
      method: "POST",
      headers: {
        authorization: `Bearer ${KEY}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({ model: "test", messages: [{ role: "user", content: "hello" }] }),
    });
    assert.equal(completion.status, 200);
    assert.equal(
      (await completion.json()).choices[0].message.content,
      "DEVELOPMENT_FAKE_RESPONSE",
    );

    const house = await fetch(`${origin}/api/v1/chat/completions`, {
      method: "POST",
      headers: {
        authorization: `Bearer ${KEY}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({
        model: "worldstream/development-house",
        messages: [{
          role: "user",
          content: JSON.stringify({
            schema: "worldstream/house-model-invocation/v1",
            instruction: "fixture",
            projection: {
              schema: "worldstream/assignment-observation/v1",
              role: "insider",
              projection_reset: { projection: { activity: { private_clues: [] } } },
              observations: [],
            },
            action_offers: {
              schema: "worldstream/assignment-action-offer-list/v1",
              offers: [{
                offer_id: "4:0:fixture",
                action_type: "inspect_clue",
                payload_schema: { schema: { type: "object" } },
              }],
            },
          }),
        }],
        provider: { only: ["fixture-provider"] },
        max_tokens: 1_000,
      }),
    });
    assert.equal(house.status, 200);
    const houseBody = await house.json();
    assert.match(houseBody.id, /^gen-/u);
    assert.equal(houseBody.openrouter_metadata.strategy, "direct");
    assert.equal(houseBody.openrouter_metadata.attempts[0].provider, "fixture-provider");
    assert.deepEqual(JSON.parse(houseBody.choices[0].message.content), {
      offer_id: "4:0:fixture",
      payload: { clue_id: "entry_window" },
    });
    for (const observation of [
      { projection_reset: { projection: { activity: { plans: [{ plan_id: "reviewed-plan" }] } } }, observations: [] },
      { projection_reset: null, observations: [{ observation: { plans: [{ plan_id: "reviewed-plan" }] } }] },
      { projection_reset: { projection: { activity: { plans: [{ plan_id: "old-plan" }] } } }, observations: [{ observation: { plans: [{ plan_id: "reviewed-plan" }] } }] },
    ]) {
      const endorsement = await fetch(`${origin}/api/v1/chat/completions`, {
        method: "POST",
        headers: { authorization: `Bearer ${KEY}`, "content-type": "application/json" },
        body: JSON.stringify({
          model: "worldstream/development-house",
          messages: [{ role: "user", content: JSON.stringify({
            schema: "worldstream/house-model-invocation/v1",
            projection: {
              schema: "worldstream/assignment-observation/v1",
              role: "insider",
              ...observation,
            },
            action_offers: {
              schema: "worldstream/assignment-action-offer-list/v1",
              offers: [
                { offer_id: "inspect", action_type: "inspect_clue" },
                { offer_id: "endorse", action_type: "endorse_plan" },
              ],
            },
          }) }],
          provider: { only: ["fixture-provider"] },
        }),
      });
      assert.equal(endorsement.status, 200);
      assert.deepEqual(JSON.parse((await endorsement.json()).choices[0].message.content), {
        offer_id: "endorse", payload: { plan_id: "reviewed-plan" },
      });
    }
    const metrics = await fetch(`${origin}/development/metrics`, {
      headers: { authorization: `Bearer ${KEY}` },
    });
    assert.equal(metrics.status, 200);
    assert.deepEqual(await metrics.json(), {
      version: "worldstream_development_fake_openrouter_metrics.v1",
      completion_count: 5,
      house_completion_count: 4,
    });
  } finally {
    await new Promise((resolvePromise) => server.close(resolvePromise));
  }
});

test("fake House proposes a plan only from a complete set of authorized claims", async () => {
  const { server } = createDevelopmentFakeOpenRouter(environment());
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  try {
    const claims = [
      { clue_id: "route", claim_code: "route_canal" },
      { clue_id: "entry_window", claim_code: "entry_window_late" },
      { clue_id: "required_tool", claim_code: "required_tool_disguise" },
      { clue_id: "extraction", claim_code: "extraction_van" },
    ];
    for (const complete of [true, false]) {
      const response = await fetch(`http://127.0.0.1:${server.address().port}/api/v1/chat/completions`, {
        method: "POST", headers: { authorization: `Bearer ${KEY}`, "content-type": "application/json" },
        body: JSON.stringify({ model: "worldstream/development-house", provider: { only: ["fixture-provider"] },
          messages: [{ role: "user", content: JSON.stringify({ schema: "worldstream/house-model-invocation/v1",
            projection: { role: "navigator", projection_reset: { projection: { activity: { public_claims: complete ? claims : claims.slice(0, 3), plans: [] } } }, observations: [] },
            action_offers: { schema: "worldstream/assignment-action-offer-list/v1", offers: [{ offer_id: "propose", action_type: "propose_plan" }] },
          }) }] }),
      });
      const content = (await response.json()).choices[0].message.content;
      if (complete) assert.deepEqual(JSON.parse(content), { offer_id: "propose", payload: { route: "canal", entry_window: "late", required_tool: "disguise", extraction: "van" } });
      else assert.equal(content, "DEVELOPMENT_FAKE_RESPONSE", "must not invent a missing clue from fixtures");
    }
  } finally { await new Promise(resolve => server.close(resolve)); }
});
