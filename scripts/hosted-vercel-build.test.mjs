import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { test } from "node:test";
import { activityClientBuildDigest } from "./activity-client-identities.mjs";
import {
  hostedClientArtifacts,
  installHostedClientArtifacts,
} from "./install-hosted-client-artifacts.mjs";
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

test("the Vercel product build installs the exact reviewed Midnight Archive v13 artifact", async () => {
  assert.deepEqual(
    hostedClientArtifacts.filter(([label]) => label === "Midnight Archive").map(([, path]) => path),
    ["midnight-archive-v13", "midnight-archive-v12", "midnight-archive-v10"],
  );
  const archive = hostedClientArtifacts.find(([, path]) => path === "midnight-archive-v13");
  assert.deepEqual(archive, [
    "Midnight Archive",
    "midnight-archive-v13",
    "config/activity-clients/artifacts/midnight-archive-web-v13",
    "midnight-archive-web-v13.json",
  ]);

  const release = await manifest(`config/activity-clients/releases/${archive[3]}`);
  assert.equal(release.client_id, "worldstream.midnight-archive.web");
  assert.equal(
    release.release_digest,
    "sha256:240ac1b94e9e89c0eb5a059ba85d116b807258cfd5917bc0dd879139de1b35bb",
  );
  assert.equal(
    release.artifacts[0]?.digest,
    "sha256:6d7fcea16b1305034af5a4f46214fb22b686d6aa831d53513ebe7d33ee342c4e",
  );
  assert.ok(release.surfaces.some(
    ({ entrypoint }) => entrypoint === "/midnight-archive-v13/hosted/",
  ));

  const output = await mkdtemp(resolve(tmpdir(), "worldstream-vercel-clients-"));
  try {
    await installHostedClientArtifacts({ destinationRoot: output, artifacts: [archive] });
    assert.equal(
      await activityClientBuildDigest(resolve(output, "midnight-archive-v13")),
      release.artifacts[0].digest,
    );
  } finally {
    await rm(output, { recursive: true, force: true });
  }
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
    .map(({ source }) => source).sort(), ["/join", "/launches/:launch_id", "/my-games", "/runs/:public_id"]);
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
