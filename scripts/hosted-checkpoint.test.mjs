import { strict as assert } from "node:assert";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import {
  createCheckpointManifest,
  createDeploymentRevision,
  deploymentRevisionBytes,
  parseDeploymentRevision,
  validateTarEntries,
} from "./hosted-checkpoint.mjs";
import { captureHostedVolume, validateCaptureReceipt } from "./hosted-volume-capture.mjs";

const sha = (digit) => `sha256:${digit.repeat(64)}`;
const blake = (digit) => `blake3:${digit.repeat(64)}`;

test("deployment revisions are exact, canonical, and stable", () => {
  const revision = createDeploymentRevision({
    sourceRevision: "a".repeat(40),
    imageDigest: sha("b"),
    migrationHead: "20260906034201",
    listingRevisionDigests: [blake("d"), blake("c"), blake("d")],
  });
  assert.deepEqual(revision.listing_revision_digests, [blake("c"), blake("d")]);
  assert.deepEqual(parseDeploymentRevision(deploymentRevisionBytes(revision)), revision);
  assert.throws(
    () => parseDeploymentRevision(Buffer.from(` ${JSON.stringify(revision)}\n`)),
    /deployment_revision_not_canonical/u,
  );
  assert.throws(
    () => createDeploymentRevision({
      sourceRevision: "main",
      imageDigest: sha("b"),
      migrationHead: "latest",
      listingRevisionDigests: [blake("c")],
    }),
    /invalid_hosted_deployment_revision/u,
  );
});

test("checkpoint manifests bind all three retained stores", () => {
  const manifest = createCheckpointManifest({
    checkpointId: "82000000-0000-4000-8000-000000000001",
    deploymentRevisionDigest: sha("1"),
    runtimeArchiveDigest: sha("2"),
    controllerArchiveDigest: sha("3"),
    supabaseDumpDigest: sha("4"),
    capturedAt: "2026-09-06T00:00:00.000Z",
  });
  assert.equal(manifest.runtime_archive, "worldstream-runtime.tar");
  assert.equal(manifest.controller_archive, "worldstream-controller.tar");
  assert.equal(manifest.supabase_dump, "supabase-platform.dump");
  assert.throws(
    () => createCheckpointManifest({
      checkpointId: "bad",
      deploymentRevisionDigest: sha("1"),
      runtimeArchiveDigest: sha("2"),
      controllerArchiveDigest: sha("3"),
      supabaseDumpDigest: sha("4"),
      capturedAt: "2026-09-06T00:00:00.000Z",
    }),
    /invalid_hosted_checkpoint_manifest/u,
  );
});

test("archive inventory cannot escape its retained volume children", () => {
  assert.deepEqual(
    validateTarEntries("runtime/\nruntime/worldstream.sqlite3\n", ["runtime"]),
    ["runtime", "runtime/worldstream.sqlite3"],
  );
  assert.throws(
    () => validateTarEntries("runtime/../private\n", ["runtime"]),
    /checkpoint_archive_path_invalid/u,
  );
  assert.throws(
    () => validateTarEntries("other/secret\n", ["runtime"]),
    /checkpoint_archive_scope_invalid/u,
  );
});

test("volume capture is no-clobber and requires an offline fence", async () => {
  const root = await mkdtemp(join(tmpdir(), "worldstream-volume-capture-test-"));
  try {
    await Promise.all([
      mkdir(join(root, "runtime"), { mode: 0o700 }),
      mkdir(join(root, "studio"), { mode: 0o700 }),
      mkdir(join(root, "maintenance"), { mode: 0o700 }),
      mkdir(join(root, "checkpoints"), { mode: 0o700 }),
    ]);
    await Promise.all([
      writeFile(join(root, "runtime", "worldstream.sqlite3"), "runtime", { mode: 0o600 }),
      writeFile(join(root, "studio", "retained.json"), "{}\n", { mode: 0o600 }),
    ]);
    const input = {
      volumeRoot: root,
      checkpointId: "82000000-0000-4000-8000-000000000001",
      deploymentVersion: "a".repeat(40),
      waitForClosed: async () => {},
    };
    await assert.rejects(() => captureHostedVolume(input), /maintenance_required_before_capture/u);
    await writeFile(join(root, "maintenance", "closed"), "maintenance\n", { mode: 0o600 });
    const captured = await captureHostedVolume(input);
    validateCaptureReceipt(captured.receipt);
    assert.equal(
      JSON.parse(await readFile(join(captured.destination, "capture.json"), "utf8")).checkpoint_id,
      input.checkpointId,
    );
    await assert.rejects(() => captureHostedVolume(input), /checkpoint_already_exists/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("operator source never accepts a password or DSN as a command option", async () => {
  const source = await readFile("scripts/hosted-checkpoint.mjs", "utf8");
  assert.match(source, /PGSERVICEFILE/u);
  assert.match(source, /worldstream-disposable-restore-target/u);
  assert.doesNotMatch(source, /--password|--dsn|--database-url/u);
  assert.match(source, /delete environment\.PGPASSWORD/u);
});
