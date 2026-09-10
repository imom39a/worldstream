import assert from "node:assert/strict";
import { test } from "node:test";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { internalCandidateAvailable, readInternalCandidates } from "./hosted-internal-candidates.mjs";

test("local candidate availability requires exact running Pack, approved Binding, ready Deployment and served bytes", async () => {
  const [candidate] = await readInternalCandidates();
  const root = await mkdtemp(join(tmpdir(), "hosted-candidate-"));
  const bootstrap = JSON.parse(await readFile("config/activity-clients/hosted-local-bindings.json", "utf8"));
  const deployment = bootstrap.deployments.find((entry) => entry.release_digest === candidate.release.release_digest);
  const binding = bootstrap.bindings.find((entry) => entry.deployment_id === deployment.deployment_id);
  let serveExact = true;
  const server = createServer((request, response) => {
    const path = new URL(request.url, "http://local.invalid").pathname.replace(/^\/[^/]+\//u, "");
    void readFile(resolve(candidate.client_artifact_directory, path)).then((bytes) => response.end(serveExact ? bytes : "unreviewed client"));
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const options = { ctl: join(root, "ctl"), config: join(root, "config"), stateDirectory: root, controller: "127.0.0.1:9420",
    clientOrigin: "http://127.0.0.1:5180", clientHostOrigin: `http://127.0.0.1:${server.address().port}`, listingDigest: candidate.listing.digest };
  const pack = { pack_id: candidate.listing.value.pack.id, explanatory_version: candidate.listing.value.pack.version,
    bundle_digest: candidate.bundle_digest, revision_digest: candidate.listing.value.pack.digest, install_state: "selectable" };
  const packs = { running: { availability: "available", facts: { installed: [pack] } },
    installed: { availability: "available", entries: [pack] }, pending_changes: false };
  const report = async () => writeFile(options.ctl, `#!/bin/sh\ncat <<'REPORT'\n${JSON.stringify({ packs })}\nREPORT\n`);
  const record = async (directory, id, value) => {
    const path = join(root, "client-bindings", directory);
    await mkdir(path, { recursive: true });
    await writeFile(join(path, `${id}.json`), JSON.stringify(value));
  };
  try {
    await report(); await chmod(options.ctl, 0o700);
    await record("releases", "release", candidate.release);
    await record("deployments", "deployment", deployment);
    await record("bindings", "binding", binding);
    await record("binding-status", binding.binding_id, { status: "approved" });
    await record("deployment-status", deployment.deployment_id, { status: "ready" });
    assert.equal(await internalCandidateAvailable(options), true);
    assert.equal(await internalCandidateAvailable({ ...options, listingDigest: `blake3:${"0".repeat(64)}` }), false);
    packs.pending_changes = true; await report();
    assert.equal(await internalCandidateAvailable(options), false);
    packs.pending_changes = false; pack.revision_digest = `blake3:${"0".repeat(64)}`; await report();
    assert.equal(await internalCandidateAvailable(options), false);
    pack.revision_digest = candidate.listing.value.pack.digest; await report();
    await record("binding-status", binding.binding_id, { status: "disabled" });
    assert.equal(await internalCandidateAvailable(options), false);
    await record("binding-status", binding.binding_id, { status: "approved" });
    await record("deployment-status", deployment.deployment_id, { status: "revoked" });
    assert.equal(await internalCandidateAvailable(options), false);
    await record("deployment-status", deployment.deployment_id, { status: "ready" });
    serveExact = false;
    assert.equal(await internalCandidateAvailable(options), false);
    serveExact = true;
    assert.equal(await internalCandidateAvailable(options), true);
  } finally {
    await new Promise((done) => server.close(done));
    await rm(root, { recursive: true, force: true });
  }
});
