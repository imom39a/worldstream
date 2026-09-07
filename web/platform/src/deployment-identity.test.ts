import assert from "node:assert/strict";
import { test } from "vitest";
import { withDeploymentIdentity } from "./deployment-identity.js";

test("deployment identity reports observed schema and compiled artifacts, or fails closed", async () => {
  const input = { canonicalOrigin: "https://arena.example", commit: "a".repeat(40),
    platformRevision: "dpl_fixture", gatewayOrigin: "https://gateway.example",
    readSchemaHead: async () => "20260907132124" };
  const platform = { fetch: async () => new Response("next") };
  const bff = withDeploymentIdentity(platform, input);
  const response = await bff.fetch(new Request("https://arena.example/api/deployment"));
  assert.equal(response.status, 200);
  const body = await response.json() as Record<string, unknown>;
  assert.equal(body.schema_head, "20260907132124");
  assert.equal(body.commit, input.commit);
  assert.match(String(body.pack_digest), /^blake3:[0-9a-f]{64}$/u);
  assert.equal(Object.keys(body).length, 9);
  for (const change of [{ commit: undefined }, { readSchemaHead: async () => { throw new Error("private DB details"); } }]) {
    const failed = await withDeploymentIdentity(platform, { ...input, ...change }).fetch(new Request("https://arena.example/api/deployment"));
    assert.equal(failed.status, 503);
    assert.equal((await failed.text()).includes("private"), false);
  }
  assert.equal((await bff.fetch(new Request("https://other.example/api/deployment"))).status, 404);
});
