import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { resolveBuildRevision } from "../web/demos/buildRevision.ts";

const repository = new URL("../", import.meta.url);

async function manifest(path) {
  return JSON.parse(await readFile(new URL(path, repository), "utf8"));
}

test("the Vercel product build prepares dist-only workspace dependencies", async () => {
  const [product, platform, contract] = await Promise.all([
    manifest("web/demos/package.json"),
    manifest("web/platform/package.json"),
    manifest("sdk/typescript-hosted-contract/package.json"),
  ]);

  assert.ok(product.scripts.build.startsWith("pnpm --dir ../platform build && "));
  assert.equal(platform.scripts.prebuild, "pnpm --filter @worldstream/hosted-contract build");
  assert.equal(contract.scripts.prebuild, "pnpm --filter @worldstream/pack-sdk build");
});

test("the retired client path reaches the BFF 404 instead of a new artifact or SPA", async () => {
  const configuration = await manifest("web/demos/vercel.json");
  const retired = configuration.rewrites.find(({ source }) => source === "/agent-heist/:path*");
  assert.equal(retired?.destination, "/api/platform");
  assert.ok(!configuration.rewrites.some(({ source, destination }) =>
    source.startsWith("/agent-heist/") && destination.includes("agent-heist-v2")));
});

test("the API rewrite uses an unnamed capture so Vercel does not inject a path query", async () => {
  // Vercel's real convertRewrites turns :path* into ?path=$1, but leaves an
  // unnamed capture out of the query. Caller query parameters remain intact.
  const configuration = await manifest("web/demos/vercel.json");
  const api = configuration.rewrites.find(({ source }) => source.startsWith("/api/"));
  assert.deepEqual(api, { source: "/api/(.*)", destination: "/api/platform" });
});

test("invitation links and GitHub return URLs load the join page", async () => {
  const configuration = await manifest("web/demos/vercel.json");
  const join = configuration.rewrites.find(({ source }) => source === "/join");
  assert.equal(join?.destination, "/index.html");
});

test("formation deep links load the launch page without a blanket SPA fallback", async () => {
  const configuration = await manifest("web/demos/vercel.json");
  const launch = configuration.rewrites.find(({ source }) => source === "/launches/:launch_id");
  assert.equal(launch?.destination, "/index.html");
  assert.deepEqual(configuration.rewrites
    .filter(({ destination }) => destination === "/index.html")
    .map(({ source }) => source).sort(), ["/join", "/launches/:launch_id", "/runs/:public_id"]);
  assert.equal(configuration.rewrites.find(({ source }) => source === "/api/(.*)")?.destination,
    "/api/platform");
});

test("a gitless Vercel build uses its provider-supplied source revision", () => {
  const revision = "a123456789".repeat(4);
  assert.equal(resolveBuildRevision({ VERCEL_GIT_COMMIT_SHA: revision }, () => {
    assert.fail("a Vercel source archive must not require local Git metadata");
  }), revision.slice(0, 12));
});

test("an invalid Vercel source revision does not silently fall back to local Git", () => {
  assert.throws(() => resolveBuildRevision({ VERCEL_GIT_COMMIT_SHA: "unknown" }, () => {
    assert.fail("invalid provider identity must fail closed");
  }), /invalid product build source revision/u);
});

test("a local build uses and validates the exact checkout revision", () => {
  const revision = "b987654321".repeat(4);
  assert.equal(resolveBuildRevision({}, () => revision), revision.slice(0, 12));
  assert.throws(() => resolveBuildRevision({}, () => "unknown"), /invalid product build source revision/u);
});
