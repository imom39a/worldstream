import { strict as assert } from "node:assert";
import { Buffer } from "node:buffer";
import { test } from "vitest";

import { MIDNIGHT_ARCHIVE_LISTING_DIGEST } from "./hosted-catalog.js";
import { createProductionPlatformBff } from "./production.js";

function productionEnvironment(): NodeJS.ProcessEnv {
  return {
    NODE_ENV: "production",
    VERCEL_ENV: "production",
    WORLDSTREAM_DEPLOYMENT_ENVIRONMENT: "production",
    CANONICAL_ORIGIN: "https://arena.example",
    SUPABASE_URL: "https://project.supabase.co",
    SUPABASE_PUBLISHABLE_KEY: `sb_publishable_${"p".repeat(32)}`,
    SUPABASE_DATA_SECRET_KEY: `sb_secret_${"s".repeat(32)}`,
    WORLDSTREAM_SESSION_KEY_BASE64: Buffer.alloc(32, 7).toString("base64"),
    WORLDSTREAM_OAUTH_KEY_BASE64: Buffer.alloc(32, 9).toString("base64"),
    WORLDSTREAM_HOSTED_GATEWAY_URL: "https://worldstream-fly.example",
    WORLDSTREAM_VERCEL_SERVICE_AUTHORITY: "v".repeat(40),
    WORLDSTREAM_HOSTED_INSTALLATION_ID: "fly-primary",
    CRON_SECRET: "fixture-cron-secret-with-32-characters",
  };
}

test("production Vercel function exposes the HTTP catalog without a WebSocket fallback", async () => {
  const bff = createProductionPlatformBff(productionEnvironment());
  const response = await bff.fetch(new Request("https://arena.example/api/catalog", {
    headers: {
      host: "arena.example",
      "x-forwarded-host": "arena.example",
      "x-forwarded-port": "443",
      "x-forwarded-proto": "https",
      "x-forwarded-for": "192.0.2.10",
      "x-vercel-id": "iad1::fixture",
    },
  }));
  assert.equal(response.status, 200);
  const body = await response.json() as { activities?: Array<{ availability?: string }> };
  assert.equal(body.activities?.[0]?.availability, "available");
  assert.equal(response.headers.get("x-worldstream-development-substitute"), null);

  const websocket = await bff.fetch(new Request("https://arena.example/api/ws", {
    headers: {
      host: "arena.example",
      upgrade: "websocket",
      "x-forwarded-host": "arena.example",
      "x-forwarded-port": "443",
      "x-forwarded-proto": "https",
      "x-forwarded-for": "192.0.2.10",
      "x-vercel-id": "iad1::fixture",
    },
  }));
  assert.equal(websocket.status, 404);
});

test("production construction rejects development substitutes and malformed keys", () => {
  assert.doesNotThrow(() => createProductionPlatformBff({
    ...productionEnvironment(),
    WORLDSTREAM_INTERNAL_CANDIDATE_LISTING_DIGEST: MIDNIGHT_ARCHIVE_LISTING_DIGEST,
  }));
  assert.throws(() => createProductionPlatformBff({
    ...productionEnvironment(),
    WORLDSTREAM_INTERNAL_CANDIDATE_LISTING_DIGEST: `blake3:${"a".repeat(64)}`,
  }), /invalid_internal_candidate_listing_digest/u);
  assert.throws(() => createProductionPlatformBff({
    ...productionEnvironment(),
    WORLDSTREAM_DEVELOPMENT_FAKE_OPENROUTER: "visible-local-only",
  }), /production_platform_configuration_required/u);
  assert.throws(() => createProductionPlatformBff({
    ...productionEnvironment(),
    WORLDSTREAM_LOCAL_BROWSER_STREAM_URL: "http://localhost:8080",
  }), /production_platform_configuration_required/u);
  assert.throws(() => createProductionPlatformBff({
    ...productionEnvironment(),
    WORLDSTREAM_SESSION_KEY_BASE64: "not-a-key",
  }), /invalid_production_encryption_key/u);
  assert.throws(() => createProductionPlatformBff({
    ...productionEnvironment(),
    VERCEL_ENV: "preview",
  }), /production_platform_configuration_required/u);
});

test("the production adapter discards unused Forwarded without changing route or origin authority", async () => {
  const bff = createProductionPlatformBff(productionEnvironment());
  const headers = {
    host: "arena.example",
    "x-forwarded-host": "arena.example",
    "x-forwarded-port": "443",
    "x-forwarded-proto": "https",
    "x-forwarded-for": "192.0.2.10",
    "x-vercel-id": "iad1::fixture",
    // This field may come from an untrusted client and is never routing input.
    forwarded: "for=unknown;host=attacker.example;proto=http",
  };
  for (const [path, status] of [["/api/catalog", 200], ["/api/auth/session", 401]] as const) {
    const response = await bff.fetch(new Request(`https://arena.example${path}`, { headers }));
    assert.equal(response.status, status, path);
  }
  for (const extra of [
    { origin: "https://attacker.example" },
    { "x-forwarded-host": "attacker.example" },
    { "x-forwarded-uri": "/api/catalog" },
    { "x-original-path": "/api/catalog" },
  ]) {
    const response = await bff.fetch(new Request("https://arena.example/api/auth/session", {
      headers: { ...headers, ...extra },
    }));
    assert.equal(response.status, 403);
  }
  const mutation = await bff.fetch(new Request("https://arena.example/api/account/public-profile", {
    method: "POST", headers: { ...headers, "content-type": "application/json" },
    body: JSON.stringify({ enabled: true }),
  }));
  assert.equal(mutation.status, 403); // Forwarded cannot replace the required Origin/CSRF.
  assert.equal((await bff.fetch(new Request("https://arena.example/api/deployment?path=deployment", { headers }))).status, 404);
});
