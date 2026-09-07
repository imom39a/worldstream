import { strict as assert } from "node:assert";
import { chmod, copyFile, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { execFile, execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { promisify } from "node:util";

import {
  createCheckpointManifest,
  createDeploymentRevision,
  deploymentRevisionBytes,
  parseDeploymentRevision,
  validateTarEntries,
  validateRestartReadinessReceipt,
  verifyIsolatedRuntime,
  verifyDatabaseMigrationHead,
  verifyDisposableRestorePrivileges,
} from "./hosted-checkpoint.mjs";
import { captureHostedVolume, validateCaptureReceipt } from "./hosted-volume-capture.mjs";
import { EMPTY_PLATFORM_TABLES } from "./hosted-prelaunch-recovery.mjs";
import { seedPrelaunchFixture } from "./fixtures/hosted-prelaunch.mjs";

const sha = (digit) => `sha256:${digit.repeat(64)}`;
const blake = (digit) => `blake3:${digit.repeat(64)}`;

function readinessReceipt() {
  return {
    schema: "worldstream/pack-operator-receipt/v1",
    status: "ready",
    operation: "restart_readiness",
    storage_profile: "sqlite-bundled",
    deployment_binding: blake("a"),
    inventory_digest: blake("b"),
    original_bytes_reverified: true,
    production_component_host_admission: true,
    room_replay_checked: true,
    embedded_revisions: 8,
    installed_bundles: 0,
    installed_selectable: 0,
    installed_retained_only: 0,
    total_revisions: 8,
    rooms_replayed: 1,
    isolated_rooms_skipped: 0,
  };
}

test("hosted restore accepts executable restart verification, never native-only evidence", () => {
  assert.equal(validateRestartReadinessReceipt(readinessReceipt()).rooms_replayed, 1);
  assert.throws(() => validateRestartReadinessReceipt({
    schema: "worldstream/operator-sqlite-result/v1",
    status: "ok",
    operation: "verify",
    native_verifier: "pass",
    semantic_verifier: "not_invoked",
  }), /hosted_runtime_restart_verification_incomplete/u);
  for (const field of ["original_bytes_reverified", "production_component_host_admission", "room_replay_checked"]) {
    assert.throws(() => validateRestartReadinessReceipt({ ...readinessReceipt(), [field]: false }),
      /hosted_runtime_restart_verification_incomplete/u);
  }
});

test("restore admission requires the exact source migration set, not only its last version", async () => {
  const root = await mkdtemp(join(tmpdir(), "worldstream-checkpoint-migrations-test-"));
  try {
    const psql = join(root, "psql");
    await writeFile(psql,
      `#!${process.execPath}\nprocess.stdout.write('["20260906034201","20260907133437"]\\n');\n`,
      { mode: 0o700 });
    const migrations = await verifyDatabaseMigrationHead({
      service: "disposable_restore", expectedHead: "20260907133437", psql,
    });
    assert.deepEqual(migrations, ["20260906034201", "20260907133437"]);
    await assert.rejects(() => verifyDatabaseMigrationHead({
      service: "disposable_restore", expectedHead: "20260906034201", psql,
    }), /checkpoint_migration_head_mismatch/u);
    await assert.rejects(() => verifyDatabaseMigrationHead({
      service: "disposable_restore", expectedHead: "20260907133437", psql,
      expectedMigrations: ["20260906034201", "20260907000101", "20260907133437"],
    }), /checkpoint_migration_set_mismatch/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("disposable restore refuses insufficient trigger privileges before mutation", async () => {
  const root = await mkdtemp(join(tmpdir(), "worldstream-restore-privilege-test-"));
  try {
    const psql = join(root, "psql");
    await writeFile(psql, `#!${process.execPath}\nprocess.stdout.write('f\\n');\n`, { mode: 0o700 });
    await assert.rejects(() => verifyDisposableRestorePrivileges({ service: "disposable", psql }),
      /disposable_restore_superuser_required/u);
    await writeFile(psql, `#!${process.execPath}\nprocess.stdout.write('t\\n');\n`, { mode: 0o700 });
    await verifyDisposableRestorePrivileges({ service: "disposable", psql });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a failed repeat drill preserves its captured identity and refuses verification overwrite", async () => {
  const root = await mkdtemp(join(tmpdir(), "worldstream-checkpoint-repeat-test-"));
  try {
    await seedPrelaunchFixture(root);
    const runtimeArchive = join(root, "worldstream-runtime.tar");
    const controllerArchive = join(root, "worldstream-controller.tar");
    execFileSync("tar", ["-cf", runtimeArchive, "-C", root, "runtime"]);
    execFileSync("tar", ["-cf", controllerArchive, "-C", root, "studio", "maintenance"]);
    const digest = (bytes) => `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
    const deployment = deploymentRevisionBytes(createDeploymentRevision({
      sourceRevision: "a".repeat(40), imageDigest: sha("b"),
      migrationHead: "20260907133437", listingRevisionDigests: [blake("c")],
    }));
    await writeFile(join(root, "hosted-deployment.json"), deployment, { mode: 0o600 });
    const dump = Buffer.from("external-pg-restore-boundary-fixture");
    await writeFile(join(root, "supabase-platform.dump"), dump, { mode: 0o600 });
    const manifest = createCheckpointManifest({
      checkpointId: "82000000-0000-4000-8000-000000000001",
      deploymentRevisionDigest: digest(deployment),
      runtimeArchiveDigest: digest(await readFile(runtimeArchive)),
      controllerArchiveDigest: digest(await readFile(controllerArchive)),
      supabaseDumpDigest: digest(dump), capturedAt: "2026-09-07T16:00:00Z",
    });
    await writeFile(join(root, "manifest.json"), JSON.stringify(manifest), { mode: 0o600 });
    const tool = join(root, "pg-tool");
    await writeFile(tool, `#!${process.execPath}\nconst args=process.argv.slice(2); if(args.includes('--version')) process.stdout.write('PostgreSQL 17.11\\n'); else if(args.includes('--list')) {} else if(args.some(x=>x.includes('select rolsuper'))) process.stdout.write('f\\n'); else process.exit(95);\n`, { mode: 0o700 });
    const serviceFile = join(root, "pg_service.conf");
    await writeFile(serviceFile, "[disposable]\nhost=127.0.0.1\n", { mode: 0o600 });
    const options = [resolve("scripts/hosted-checkpoint.mjs"), "checkpoint", "drill", "--directory", root,
      "--source-service", "source", "--restore-service", "disposable", "--worldstreamctl", tool,
      "--confirm-disposable-restore", "worldstream-disposable-restore-target"];
    const environment = { ...process.env, PGSERVICEFILE: serviceFile,
      WORLDSTREAM_PSQL: tool, WORLDSTREAM_PG_DUMP: tool, WORLDSTREAM_PG_RESTORE: tool };
    delete environment.PGPASSWORD;
    delete environment.DATABASE_URL;
    const files = ["manifest.json", "hosted-deployment.json", "worldstream-runtime.tar", "worldstream-controller.tar", "supabase-platform.dump"];
    const before = await Promise.all(files.map((name) => readFile(join(root, name))));
    assert.throws(() => execFileSync(process.execPath, options, { env: environment, stdio: "pipe" }),
      (error) => /disposable_restore_superuser_required/u.test(error.stderr.toString()));
    assert.deepEqual(await Promise.all(files.map((name) => readFile(join(root, name)))), before);
    assert.equal(existsSync(join(root, "verification-prelaunch-v1.json")), false);
    await writeFile(join(root, "verification-prelaunch-v1.json"), "retained-proof", { mode: 0o600 });
    assert.throws(() => execFileSync(process.execPath, options, { env: environment, stdio: "pipe" }),
      (error) => /checkpoint_verification_already_exists/u.test(error.stderr.toString()));
    assert.equal(await readFile(join(root, "verification-prelaunch-v1.json"), "utf8"), "retained-proof");

    // External CLI and RPC boundaries prove retry after a lost/failed event
    // response. The real bundled-SQLite behavior has its own WAL test below.
    const retry = join(root, "retry");
    await mkdir(retry, { mode: 0o700 });
    await Promise.all(files.map((name) => copyFile(join(root, name), join(retry, name))));
    await writeFile(join(retry, "verification.json"), "retained-insufficient-v2-proof", { mode: 0o600 });
    const platformFixture = { installation_id: "fly-primary", launches_open: false, house_fill_open: false,
      maintenance_mode: true, recovery_fenced: false, counts: Object.fromEntries(EMPTY_PLATFORM_TABLES.map((table) => [table, 0])) };
    await writeFile(tool, `#!${process.execPath}\nconst args=process.argv.slice(2); if(args.includes('--version')) process.stdout.write('PostgreSQL 17.11\\n'); else if(args[0]==='version') process.stdout.write(${JSON.stringify(JSON.stringify({ product_build: { binary: "worldstreamctl", source_revision: "a".repeat(40) } }))}); else if(args[0]==='pack') process.stdout.write(${JSON.stringify(JSON.stringify({ ...readinessReceipt(), rooms_replayed: 0 }))}); else if(args.some(x=>x.includes('select rolsuper'))) process.stdout.write('t\\n'); else if(args.some(x=>x.includes('json_agg'))) process.stdout.write('["20260907133437"]\\n'); else if(args.some(x=>x.includes('json_build_object'))) process.stdout.write(${JSON.stringify(JSON.stringify(platformFixture))}); else if(args.some(x=>x.includes('select count(*)'))) process.stdout.write('1\\n'); else if(args.includes('--list')||args.includes('--data-only')||args.some(x=>x.startsWith('do $worldstream$'))) {} else process.exit(95);\n`, { mode: 0o700 });
    const requests = [];
    const server = createServer(async (request, response) => {
      let body = "";
      for await (const chunk of request) body += chunk;
      requests.push(JSON.parse(body));
      response.writeHead(requests.length === 1 ? 500 : 200, { "content-type": "application/json" });
      response.end("{}");
    });
    await new Promise((done) => server.listen(0, "127.0.0.1", done));
    try {
      const key = join(root, "test-rpc-key");
      await writeFile(key, "worldstream-test-only-not-a-real-key", { mode: 0o600 });
      const retryEnvironment = { ...environment, SUPABASE_URL: `http://127.0.0.1:${server.address().port}`,
        SUPABASE_DATA_SECRET_KEY_FILE: key, WORLDSTREAM_ALLOW_LOCAL_OPERATIONS: "visible-local-only" };
      const retryOptions = options.map((value) => value === root ? retry : value);
      const execute = promisify(execFile);
      await assert.rejects(() => execute(process.execPath, retryOptions, { env: retryEnvironment }),
        (error) => /supabase_operation_rejected:500/u.test(error.stderr));
      assert.equal(existsSync(join(retry, "verification-prelaunch-v1.json")), false);
      const passed = await execute(process.execPath, retryOptions, { env: retryEnvironment });
      assert.equal(JSON.parse(passed.stdout).checkpoint_id, manifest.checkpoint_id);
      assert.deepEqual(requests[0], requests[1]);
      assert.equal(requests[1].p_event_kind, "isolated_restore_verified");
      const proof = JSON.parse(await readFile(join(retry, "verification-prelaunch-v1.json"), "utf8"));
      assert.equal(proof.recovery_profile, "worldstream/hosted-prelaunch-zero-history/v1");
      assert.equal(proof.prelaunch_correspondence.platform_activity_records, 0);
      assert.equal(proof.populated_recovery, "not_verified_requires_complete_correspondence_verifier");
      assert.equal(await readFile(join(retry, "verification.json"), "utf8"), "retained-insufficient-v2-proof");
      assert.deepEqual(await Promise.all(files.map((name) => readFile(join(retry, name)))), before);
    } finally {
      server.closeAllConnections();
      await new Promise((done) => server.close(done));
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

const control = resolve(process.env.WORLDSTREAM_CHECKPOINT_TEST_CTL ?? "target/debug/worldstreamctl");
test("an isolated real WAL Runtime copy passes restart verification without rewriting captured archives", {
  skip: !existsSync(control) && "build worldstreamctl to run the real WAL regression",
}, async () => {
  const root = await mkdtemp(join(tmpdir(), "worldstream-checkpoint-wal-test-"));
  try {
    const runtime = join(root, "runtime");
    await mkdir(runtime, { mode: 0o700 });
    const database = join(runtime, "worldstream.sqlite3");
    const connection = new DatabaseSync(database);
    connection.exec("PRAGMA journal_mode=WAL; PRAGMA user_version=0;");
    connection.close();
    await chmod(database, 0o600);
    assert.deepEqual((await readFile(database)).subarray(18, 20), Buffer.from([2, 2]));
    const archive = join(root, "captured.tar");
    execFileSync("tar", ["-cf", archive, "-C", root, "runtime"]);
    const original = await readFile(archive);
    const isolated = join(root, "isolated");
    await mkdir(isolated, { mode: 0o700 });
    execFileSync("tar", ["-xf", archive, "-C", isolated]);
    const version = JSON.parse(execFileSync(control, ["version"], { encoding: "utf8" }));
    const result = await verifyIsolatedRuntime({
      runtimeDirectory: join(isolated, "runtime"),
      worldstreamctl: control,
      sourceRevision: version.product_build.source_revision,
    });
    assert.equal(result.room_replay_checked, true);
    assert.equal(result.rooms_replayed, 0);
    assert.deepEqual(await readFile(archive), original);
    assert.deepEqual((await readFile(database)).subarray(18, 20), Buffer.from([2, 2]));
    await assert.rejects(() => verifyIsolatedRuntime({
      runtimeDirectory: join(isolated, "runtime"),
      worldstreamctl: control,
      sourceRevision: "0".repeat(40),
    }), /checkpoint_verifier_revision_mismatch/u);
    await assert.rejects(() => verifyIsolatedRuntime({
      runtimeDirectory: join(root, "missing-runtime"),
      worldstreamctl: control,
      sourceRevision: version.product_build.source_revision,
    }), /ENOENT|protected_checkpoint/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

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
