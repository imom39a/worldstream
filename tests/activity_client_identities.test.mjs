import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { activityClientBuildDigest, activityClientReleaseClaimsDigest, activityClientReleaseDigest } from "../scripts/activity-client-identities.mjs";

const run = promisify(execFile);
const zeros = `sha256:${"0".repeat(64)}`;
const ones = `sha256:${"1".repeat(64)}`;

test("shared identity module preserves the frozen identity algorithms", () => {
  const release = { schema: "worldstream/activity-client-release/v1", client_id: "example.client", release_digest: zeros,
    client_contract: "worldstream/activity-client-protocol/v1", artifacts: [{ artifact_id: "browser-dist", media_type: "x", digest: ones }], surfaces: [], conformance: [] };
  assert.equal(activityClientReleaseClaimsDigest(release), "sha256:beb786cf038c199ccb462d5ac00433fb665e8f7a4dca2ba624586366cd845819");
  assert.equal(activityClientReleaseDigest(release), "sha256:c157ac3bc6d5299cca6bf109cf298024aee60667364ef4cf956bace2183b5df3");
});

test("local preparation binds the actual isolated bytes without claiming qualification", async () => {
  const root = await mkdtemp(resolve(tmpdir(), "worldstream-client-preparation-"));
  const dist = resolve(root, "dist"); const fallbackDist = resolve(root, "fallback-dist"); const output = resolve(root, "output");
  await mkdir(dist); await writeFile(resolve(dist, "index.html"), "isolated-client\n");
  await mkdir(fallbackDist); await writeFile(resolve(fallbackDist, "index.html"), "isolated-inspector\n");
  assert.equal(await activityClientBuildDigest(dist), "sha256:0b1eb9291d3ceb05292c4cdbbfa8daa6510d7a222d41b750bf5650aa949248e0");
  const source = resolve(root, "source.json");
  await writeFile(source, JSON.stringify({ schema: "worldstream/activity-client-release/v1", client_id: "example.heist.web", release_digest: zeros,
    client_contract: "worldstream/activity-client-protocol/v1", artifacts: [{ artifact_id: "browser-dist", media_type: "application/vnd.worldstream.activity-client.web.v1+directory", digest: zeros }],
    surfaces: [{ surface_id: "heist-web", kind: "browser", artifact_digest: zeros, entrypoint: "/agent-heist/", capabilities: ["observe", "act", "replay"] }], conformance: [{ contract: "worldstream/activity-client-protocol/v1", evidence_digest: ones }] }));
  const fallbackSource = resolve(root, "fallback-source.json");
  await writeFile(fallbackSource, JSON.stringify({ schema: "worldstream/activity-client-release/v1", client_id: "example.inspector.web", release_digest: zeros,
    client_contract: "worldstream/activity-client-protocol/v1", artifacts: [{ artifact_id: "browser-dist", media_type: "application/vnd.worldstream.activity-client.web.v1+directory", digest: zeros }],
    surfaces: [{ surface_id: "inspector-web", kind: "browser", artifact_digest: zeros, entrypoint: "/inspector/", capabilities: ["observe", "replay"] }], conformance: [] }));
  const preparationArguments = [resolve("scripts/prepare-local-activity-client.mjs"), "--dist", dist, "--source-release", source,
    "--pack-id", "worldstream.agent-heist", "--pack-version", "0.2.0", "--pack-digest", `blake3:${"a".repeat(64)}`,
    "--access-mode", "participant", "--roles", "navigator,insider", "--surface", "heist-web", "--entrypoint", "/agent-heist/",
    "--launch-url", "http://127.0.0.1:15173/agent-heist/", "--fallback-dist", fallbackDist,
    "--fallback-source-release", fallbackSource, "--fallback-surface", "inspector-web", "--fallback-entrypoint", "/inspector/",
    "--fallback-launch-url", "http://127.0.0.1:15173/inspector/", "--output", output];
  const { stdout } = await run(process.execPath, preparationArguments);
  const receipt = JSON.parse(stdout); assert.equal(receipt.qualification, "not_claimed");
  const release = JSON.parse(await readFile(resolve(output, "activity-client-release.json"), "utf8"));
  assert.equal(release.artifacts[0].digest, await activityClientBuildDigest(dist)); assert.deepEqual(release.conformance, []);
  const bindings = JSON.parse(await readFile(resolve(output, "client-bindings.json"), "utf8"));
  assert.equal(bindings.deployments[0].trust_level, "externally_trusted");
  assert.deepEqual(bindings.inspector_fallback, { schema: "worldstream/inspector-fallback/v1",
    fallback_id: bindings.inspector_fallback.fallback_id, deployment_id: bindings.deployments[1].deployment_id, surface_id: "inspector-web" });
  assert.equal(bindings.deployments[0].release_digest, release.release_digest);
  const declaration = JSON.parse(await readFile(resolve(output, "client-declaration.json"), "utf8"));
  assert.equal(declaration.release_files.length, 2);

  const invalidOutput = resolve(root, "invalid-output");
  const invalidArguments = [...preparationArguments];
  invalidArguments[invalidArguments.indexOf("navigator,insider")] = "navigator,navigator";
  invalidArguments[invalidArguments.lastIndexOf(output)] = invalidOutput;
  await assert.rejects(run(process.execPath, invalidArguments), /Access Mode and Roles are invalid/);
  await assert.rejects(readFile(resolve(invalidOutput, "client-declaration.json")), /ENOENT/);

  const invalidSource = resolve(root, "invalid-source.json");
  const invalidRelease = JSON.parse(await readFile(source, "utf8"));
  invalidRelease.client_id = "Invalid Client ID";
  await writeFile(invalidSource, JSON.stringify(invalidRelease));
  const invalidReleaseOutput = resolve(root, "invalid-release-output");
  const invalidReleaseArguments = [...preparationArguments];
  invalidReleaseArguments[invalidReleaseArguments.indexOf(source)] = invalidSource;
  invalidReleaseArguments[invalidReleaseArguments.lastIndexOf(output)] = invalidReleaseOutput;
  await assert.rejects(run(process.execPath, invalidReleaseArguments), /source Release is invalid/);
  await assert.rejects(readFile(resolve(invalidReleaseOutput, "client-declaration.json")), /ENOENT/);
});

test("artifact identity covers files under source-like nested directory names", async () => {
  const root = await mkdtemp(resolve(tmpdir(), "worldstream-client-tree-"));
  await mkdir(resolve(root, "dist"));
  await mkdir(resolve(root, "node_modules"));
  await writeFile(resolve(root, "index.html"), "client\n");
  await writeFile(resolve(root, "dist", "nested.js"), "one\n");
  await writeFile(resolve(root, "node_modules", "dependency.js"), "dependency\n");
  const before = await activityClientBuildDigest(root);
  await writeFile(resolve(root, "dist", "nested.js"), "two\n");
  assert.notEqual(await activityClientBuildDigest(root), before);
});

test("artifact identity rejects inside-root and outside-root symlinks", async () => {
  const parent = await mkdtemp(resolve(tmpdir(), "worldstream-client-symlinks-"));
  const root = resolve(parent, "artifact");
  await mkdir(root);
  await writeFile(resolve(root, "index.html"), "client\n");
  await writeFile(resolve(root, "inside.js"), "inside\n");
  await writeFile(resolve(parent, "outside.js"), "outside\n");
  await symlink(resolve(root, "inside.js"), resolve(root, "inside-link.js"));
  await assert.rejects(
    activityClientBuildDigest(root),
    /artifact tree may contain only regular files and directories/,
  );
  await import("node:fs/promises").then(({ unlink }) => unlink(resolve(root, "inside-link.js")));
  await symlink(resolve(parent, "outside.js"), resolve(root, "outside-link.js"));
  await assert.rejects(
    activityClientBuildDigest(root),
    /artifact tree may contain only regular files and directories/,
  );
});
