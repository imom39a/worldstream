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
  } finally {
    await new Promise((resolvePromise) => server.close(resolvePromise));
  }
});
