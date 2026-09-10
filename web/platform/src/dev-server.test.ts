import { strict as assert } from "node:assert";
import type { AddressInfo } from "node:net";
import { test } from "vitest";
import { createDevelopmentPlatformServer } from "./dev-server.js";

function environment(internalDigests: string | undefined): NodeJS.ProcessEnv {
  return {
    WORLDSTREAM_PLATFORM_BIND: "127.0.0.1",
    CANONICAL_ORIGIN: "http://127.0.0.1:5180",
    SUPABASE_URL: "https://database.example.invalid",
    SUPABASE_PUBLISHABLE_KEY: "sb_publishable_synthetic-development-test",
    SUPABASE_DATA_SECRET_KEY: "sb_secret_synthetic-development-test",
    WORLDSTREAM_SESSION_KEY_BASE64: Buffer.alloc(32, 7).toString("base64"),
    WORLDSTREAM_OAUTH_KEY_BASE64: Buffer.alloc(32, 9).toString("base64"),
    WORLDSTREAM_DEVELOPMENT_IDENTITY_BYPASS: "visible-local-only",
    WORLDSTREAM_DEPLOYMENT_ENVIRONMENT: "development",
    WORLDSTREAM_DEVELOPMENT_AUTH_USER_ID: "00000000-0000-4000-8000-000000000001",
    WORLDSTREAM_DEVELOPMENT_GITHUB_SUBJECT: "development-fixture",
    WORLDSTREAM_DEVELOPMENT_GITHUB_LOGIN: "development-fixture",
    WORLDSTREAM_HOSTED_GATEWAY_URL: "http://localhost:8080",
    WORLDSTREAM_VERCEL_SERVICE_AUTHORITY: "synthetic-local-service-authority-000000000000",
    WORLDSTREAM_HOSTED_INSTALLATION_ID: "hosted-development-fixture",
    WORLDSTREAM_LOCAL_RECONCILIATION_SECRET: "synthetic-local-reconciliation-000000000000",
    WORLDSTREAM_LOCAL_INTERNAL_LISTING_DIGESTS: internalDigests,
  };
}

test.each([undefined, [].join(",")])("hosted development starts a public-only library with candidate allowlist %j", async (digests) => {
  // No candidate checker configuration is needed when the reviewed manifest
  // has no candidates. Hosted Heist dependencies remain configured normally.
  const { bind, server } = createDevelopmentPlatformServer(environment(digests));
  await new Promise<void>((resolve) => server.listen(0, bind, resolve));
  try {
    const port = (server.address() as AddressInfo).port;
    const response = await fetch(`http://${bind}:${port}/api/catalog`);
    assert.equal(response.status, 200);
    const catalog = await response.json();
    assert.deepEqual(catalog.activities.map((activity: { slug: string }) => activity.slug), ["agent-heist", "negotiate"]);
    assert.equal(catalog.activities[0].availability, "available");
    assert.equal((await fetch(`http://${bind}:${port}/api/catalog/internal`)).status, 401);
  } finally {
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
});

test("nonempty malformed candidate configuration still fails closed", () => {
  assert.throws(() => createDevelopmentPlatformServer(environment(" ")), /invalid_internal_candidate_allowlist/u);
});
