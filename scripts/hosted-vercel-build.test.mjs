import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { resolveBuildRevision } from "../web/demos/buildRevision.ts";
import { describeDeploymentRequestBoundary } from "../web/demos/requestBoundaryDiagnostic.mjs";

const repository = new URL("../", import.meta.url);

async function manifest(path) {
  return JSON.parse(await readFile(new URL(path, repository), "utf8"));
}

test("temporary deployment diagnostics expose only bounded URL components and header checks", () => {
  const diagnostic = describeDeploymentRequestBoundary(new Request(
    "http://arena.example/api/deployment?code=private-query-value", {
      headers: {
        host: "arena.example",
        "x-forwarded-host": "arena.example",
        "x-forwarded-proto": "https",
        "x-forwarded-for": "192.0.2.10",
        "x-real-ip": "192.0.2.10",
        "x-vercel-id": "iad1::fixture",
        authorization: "Bearer private-auth-value",
        cookie: "private-cookie-value",
        "x-original-host": "private-original-host-value",
      },
    },
  ), "https://arena.example/");
  assert.equal(diagnostic.url_scheme, "http:");
  assert.equal(diagnostic.url_hostname, "arena.example");
  assert.equal(diagnostic.url_path, "/api/deployment");
  assert.equal(diagnostic.url_query_present, true);
  assert.deepEqual(diagnostic.url_query_parameter_names, ["code"]);
  assert.equal(diagnostic.forwarded_proto_matches_url_scheme, false);
  assert.equal(diagnostic.real_ip_matches_forwarded_for, true);
  assert.deepEqual(diagnostic.unknown_forwarding_header_names, ["x-original-host"]);
  assert.equal(diagnostic.canonical_origin_matches_request_origin, false);
  assert.equal(diagnostic.normalized_canonical_origin_matches_request_origin, false);
  const external = describeDeploymentRequestBoundary(new Request("https://arena.example/api/deployment"), "https://arena.example/");
  assert.equal(external.canonical_origin_matches_request_origin, false);
  assert.equal(external.normalized_canonical_origin_matches_request_origin, true);
  for (const value of ["private-query-value", "private-auth-value", "private-cookie-value", "private-original-host-value", "192.0.2.10"]) {
    assert.ok(!JSON.stringify(diagnostic).includes(value));
  }
  for (const [key, value] of Object.entries(diagnostic)) {
    if (!["version", "url_scheme", "url_hostname", "url_port", "url_path", "url_query_parameter_names", "unknown_forwarding_header_names"].includes(key)) {
      assert.equal(typeof value, "boolean", key);
    }
  }
  assert.equal(describeDeploymentRequestBoundary(new Request("https://arena.example/api/catalog")), null);
  assert.equal(describeDeploymentRequestBoundary(new Request("https://arena.example/api/deployment", { method: "POST" })), null);
});

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
  assert.equal(configuration.rewrites.find(({ source }) => source === "/api/:path*")?.destination,
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
