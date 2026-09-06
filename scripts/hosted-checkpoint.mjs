import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { constants, createReadStream } from "node:fs";
import {
  access,
  chmod,
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  open,
  readFile,
  rm,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { validateCaptureReceipt } from "./hosted-volume-capture.mjs";

const SHA256 = /^sha256:[0-9a-f]{64}$/u;
const BLAKE3 = /^blake3:[0-9a-f]{64}$/u;
const SOURCE_REVISION = /^[0-9a-f]{40}([0-9a-f]{24})?$/u;
const MIGRATION_HEAD = /^[0-9]{14}$/u;
const SERVICE_NAME = /^[A-Za-z][A-Za-z0-9_-]{0,62}$/u;
const CHECKPOINT_CONFIRMATION = "worldstream-disposable-restore-target";

export function createDeploymentRevision({
  sourceRevision,
  imageDigest,
  migrationHead,
  listingRevisionDigests,
}) {
  const listings = [...new Set(listingRevisionDigests)].sort();
  if (
    !SOURCE_REVISION.test(sourceRevision) ||
    !SHA256.test(imageDigest) ||
    !MIGRATION_HEAD.test(migrationHead) ||
    listings.length < 1 ||
    listings.length > 64 ||
    listings.some((value) => !BLAKE3.test(value))
  ) {
    throw new Error("invalid_hosted_deployment_revision");
  }
  return {
    schema: "worldstream/hosted-deployment-revision/v1",
    source_revision: sourceRevision,
    image_digest: imageDigest,
    supabase_migration_head: migrationHead,
    listing_revision_digests: listings,
  };
}

export function parseDeploymentRevision(bytes) {
  if (!Buffer.isBuffer(bytes) || bytes.length < 2 || bytes.length > 65_536) {
    throw new Error("invalid_hosted_deployment_revision");
  }
  let value;
  try {
    value = JSON.parse(bytes.toString("utf8"));
  } catch {
    throw new Error("invalid_hosted_deployment_revision");
  }
  if (
    value === null ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    Object.keys(value).length !== 5 ||
    value.schema !== "worldstream/hosted-deployment-revision/v1" ||
    !Array.isArray(value.listing_revision_digests)
  ) {
    throw new Error("invalid_hosted_deployment_revision");
  }
  const normalized = createDeploymentRevision({
    sourceRevision: value.source_revision,
    imageDigest: value.image_digest,
    migrationHead: value.supabase_migration_head,
    listingRevisionDigests: value.listing_revision_digests,
  });
  const canonical = deploymentRevisionBytes(normalized);
  if (!bytes.equals(canonical)) throw new Error("deployment_revision_not_canonical");
  return normalized;
}

export function deploymentRevisionBytes(value) {
  return Buffer.from(`${JSON.stringify(value)}\n`, "utf8");
}

export function validateTarEntries(entries, allowedRoots) {
  const roots = new Set(allowedRoots);
  const normalized = entries
    .split("\n")
    .map((entry) => entry.replace(/^\.\//u, "").replace(/\/$/u, ""))
    .filter(Boolean);
  if (normalized.length < 1) throw new Error("checkpoint_archive_empty");
  for (const entry of normalized) {
    if (entry.startsWith("/") || entry.split("/").includes("..")) {
      throw new Error("checkpoint_archive_path_invalid");
    }
    const root = entry.split("/")[0];
    if (!roots.has(root)) throw new Error("checkpoint_archive_scope_invalid");
  }
  for (const root of roots) {
    if (!normalized.some((entry) => entry === root || entry.startsWith(`${root}/`))) {
      throw new Error("checkpoint_archive_root_missing");
    }
  }
  return normalized;
}

export function createCheckpointManifest({
  checkpointId,
  deploymentRevisionDigest,
  runtimeArchiveDigest,
  controllerArchiveDigest,
  supabaseDumpDigest,
  capturedAt,
}) {
  if (
    !/^[0-9a-f-]{36}$/u.test(checkpointId) ||
    !SHA256.test(deploymentRevisionDigest) ||
    !SHA256.test(runtimeArchiveDigest) ||
    !SHA256.test(controllerArchiveDigest) ||
    !SHA256.test(supabaseDumpDigest) ||
    typeof capturedAt !== "string" ||
    Number.isNaN(Date.parse(capturedAt))
  ) {
    throw new Error("invalid_hosted_checkpoint_manifest");
  }
  return {
    schema: "worldstream/hosted-recovery-checkpoint/v1",
    checkpoint_id: checkpointId,
    deployment_revision_digest: deploymentRevisionDigest,
    runtime_archive: "worldstream-runtime.tar",
    runtime_archive_digest: runtimeArchiveDigest,
    controller_archive: "worldstream-controller.tar",
    controller_archive_digest: controllerArchiveDigest,
    supabase_dump: "supabase-platform.dump",
    supabase_dump_digest: supabaseDumpDigest,
    captured_at: capturedAt,
    paired_at: new Date().toISOString(),
  };
}

async function main() {
  const [group, action, ...rest] = process.argv.slice(2);
  const options = flags(rest);
  if (group === "deployment" && action === "create") {
    await createDeploymentCommand(options);
    return;
  }
  if (group === "deployment" && action === "record") {
    await recordDeploymentCommand(options);
    return;
  }
  if (group === "maintenance" && action !== undefined) {
    await maintenanceCommand(action);
    return;
  }
  if (group === "checkpoint" && action === "pair") {
    await pairCheckpointCommand(options);
    return;
  }
  if (group === "checkpoint" && action === "verify") {
    await verifyCheckpointDirectory(requiredOption(options, "directory"));
    process.stdout.write('{"status":"verified"}\n');
    return;
  }
  throw new Error(
    "usage: hosted-checkpoint deployment create|record, maintenance enter|open|recovery-fence|recovery-release, checkpoint pair|verify",
  );
}

async function createDeploymentCommand(options) {
  const output = resolve(requiredOption(options, "output"));
  const listingRevisionDigests = optionList(options, "listing-digest");
  const value = createDeploymentRevision({
    sourceRevision: requiredOption(options, "source-revision"),
    imageDigest: requiredOption(options, "image-digest"),
    migrationHead: requiredOption(options, "migration-head"),
    listingRevisionDigests,
  });
  const handle = await open(output, "wx", 0o600);
  await handle.writeFile(deploymentRevisionBytes(value));
  await handle.sync();
  await handle.close();
  process.stdout.write(`${JSON.stringify({
    status: "created",
    digest: await sha256File(output),
    file: output,
  })}\n`);
}

async function recordDeploymentCommand(options) {
  const path = resolve(requiredOption(options, "document"));
  const bytes = await readProtectedFile(path, 65_536);
  const revision = parseDeploymentRevision(bytes);
  const receipt = await recordDeployment(revision, bytes);
  process.stdout.write(`${JSON.stringify(receipt)}\n`);
}

async function maintenanceCommand(action) {
  const states = {
    enter: [false, false, true, false],
    open: [true, true, false, false],
    "recovery-fence": [false, false, false, true],
    "recovery-release": [false, false, true, false],
  };
  const selected = states[action];
  if (selected === undefined) throw new Error("invalid_maintenance_action");
  const [launchesOpen, houseFillOpen, maintenanceMode, recoveryFenced] = selected;
  const receipt = await supabaseRpc("set_hosted_operating_state_v1", {
    p_installation_id: hostedInstallationId(),
    p_launches_open: launchesOpen,
    p_house_fill_open: houseFillOpen,
    p_maintenance_mode: maintenanceMode,
    p_recovery_fenced: recoveryFenced,
  });
  process.stdout.write(`${JSON.stringify(receipt)}\n`);
}

async function pairCheckpointCommand(options) {
  if (requiredOption(options, "confirm-disposable-restore") !== CHECKPOINT_CONFIRMATION) {
    throw new Error("disposable_restore_confirmation_required");
  }
  const sourceService = databaseService(requiredOption(options, "source-service"));
  const restoreService = databaseService(requiredOption(options, "restore-service"));
  if (sourceService === restoreService) throw new Error("restore_service_must_be_isolated");
  const captureDirectory = resolve(requiredOption(options, "capture-directory"));
  const outputDirectory = resolve(requiredOption(options, "output-directory"));
  const deploymentPath = resolve(requiredOption(options, "deployment-document"));
  const worldstreamctl = resolve(requiredOption(options, "worldstreamctl"));
  await requireDatabaseTooling();
  await requireProtectedServiceFile();

  const capture = validateCaptureReceipt(
    JSON.parse((await readProtectedFile(join(captureDirectory, "capture.json"), 65_536)).toString("utf8")),
  );
  const runtimeSource = join(captureDirectory, capture.runtime_archive);
  const controllerSource = join(captureDirectory, capture.controller_archive);
  const [runtimeDigest, controllerDigest] = await Promise.all([
    sha256File(runtimeSource),
    sha256File(controllerSource),
  ]);
  if (
    runtimeDigest !== capture.runtime_archive_digest ||
    controllerDigest !== capture.controller_archive_digest
  ) {
    throw new Error("volume_capture_digest_mismatch");
  }
  await verifyTar(runtimeSource, ["runtime"]);
  await verifyTar(controllerSource, ["studio", "maintenance"]);

  const deploymentBytes = await readProtectedFile(deploymentPath, 65_536);
  const deployment = parseDeploymentRevision(deploymentBytes);
  const deploymentDigest = digestBytes(deploymentBytes);
  await recordDeployment(deployment, deploymentBytes);

  await mkdir(outputDirectory, { mode: 0o700 });
  await chmod(outputDirectory, 0o700);
  const runtimeArchive = join(outputDirectory, "worldstream-runtime.tar");
  const controllerArchive = join(outputDirectory, "worldstream-controller.tar");
  const deploymentCopy = join(outputDirectory, "hosted-deployment.json");
  await Promise.all([
    copyExclusive(runtimeSource, runtimeArchive),
    copyExclusive(controllerSource, controllerArchive),
    copyExclusive(deploymentPath, deploymentCopy),
  ]);
  const dump = join(outputDirectory, "supabase-platform.dump");
  await runDatabaseTool(pgTool("WORLDSTREAM_PG_DUMP", "pg_dump"), [
    `--dbname=service=${sourceService}`,
    "--format=custom",
    "--data-only",
    "--schema=platform_store",
    "--no-owner",
    "--no-privileges",
    `--file=${dump}`,
  ]);
  await chmod(dump, 0o600);
  const dumpDigest = await sha256File(dump);
  const manifest = createCheckpointManifest({
    checkpointId: capture.checkpoint_id,
    deploymentRevisionDigest: deploymentDigest,
    runtimeArchiveDigest: runtimeDigest,
    controllerArchiveDigest: controllerDigest,
    supabaseDumpDigest: dumpDigest,
    capturedAt: capture.captured_at,
  });
  const manifestPath = join(outputDirectory, "manifest.json");
  await writeExclusiveJson(manifestPath, manifest);
  const manifestDigest = await sha256File(manifestPath);
  await recordCheckpointEvent(manifest, manifestDigest, "created");

  const evidence = await isolatedRestoreDrill({
    outputDirectory,
    manifest,
    restoreService,
    worldstreamctl,
  });
  await recordCheckpointEvent(manifest, manifestDigest, "isolated_restore_verified");
  await writeExclusiveJson(join(outputDirectory, "verification.json"), evidence);
  process.stdout.write(`${JSON.stringify({
    status: "verified",
    checkpoint_id: manifest.checkpoint_id,
    manifest_digest: manifestDigest,
    directory: outputDirectory,
  })}\n`);
}

async function isolatedRestoreDrill({
  outputDirectory,
  manifest,
  restoreService,
  worldstreamctl,
}) {
  const isolated = await mkdtemp(join(tmpdir(), "worldstream-hosted-restore-"));
  await chmod(isolated, 0o700);
  try {
    await run("tar", [
      "--extract",
      "--file",
      join(outputDirectory, manifest.runtime_archive),
      "--directory",
      isolated,
    ]);
    await run("tar", [
      "--extract",
      "--file",
      join(outputDirectory, manifest.controller_archive),
      "--directory",
      isolated,
    ]);
    const database = join(isolated, "runtime", "worldstream.sqlite3");
    await validateProtectedFile(database, 16 * 1024 * 1024 * 1024);
    await run(worldstreamctl, ["sqlite", "verify", "--database", database]);

    const psql = pgTool("WORLDSTREAM_PSQL", "psql");
    await runDatabaseTool(psql, [
      `service=${restoreService}`,
      "-X",
      "-v",
      "ON_ERROR_STOP=1",
      "-c",
      "do $worldstream$ declare item record; begin for item in select tablename from pg_tables where schemaname = 'platform_store' loop execute format('truncate table platform_store.%I cascade', item.tablename); end loop; end $worldstream$;",
    ]);
    await runDatabaseTool(pgTool("WORLDSTREAM_PG_RESTORE", "pg_restore"), [
      `--dbname=service=${restoreService}`,
      "--data-only",
      "--disable-triggers",
      "--exit-on-error",
      "--single-transaction",
      join(outputDirectory, manifest.supabase_dump),
    ]);
    const restored = await runDatabaseTool(psql, [
      `service=${restoreService}`,
      "-X",
      "-A",
      "-t",
      "-v",
      "ON_ERROR_STOP=1",
      "-c",
      `select count(*) from platform_store.hosted_deployment_revisions where encode(deployment_revision_digest, 'hex') = '${manifest.deployment_revision_digest.slice(7)}';`,
    ], true);
    if (restored.stdout.trim() !== "1") throw new Error("isolated_supabase_restore_incomplete");
    return {
      schema: "worldstream/hosted-recovery-verification/v1",
      checkpoint_id: manifest.checkpoint_id,
      worldstream_sqlite: "verified",
      controller_state: "restored_in_isolation",
      supabase_platform_store: "restored_in_isolation",
      provider_calls: "fenced_until_explicit_recovery_release",
      verified_at: new Date().toISOString(),
    };
  } finally {
    await rm(isolated, { recursive: true, force: true });
  }
}

async function verifyCheckpointDirectory(directoryValue) {
  const directory = resolve(directoryValue);
  const manifest = JSON.parse(
    (await readProtectedFile(join(directory, "manifest.json"), 65_536)).toString("utf8"),
  );
  if (manifest?.schema !== "worldstream/hosted-recovery-checkpoint/v1") {
    throw new Error("invalid_hosted_checkpoint_manifest");
  }
  const checks = [
    [manifest.runtime_archive, manifest.runtime_archive_digest, ["runtime"]],
    [manifest.controller_archive, manifest.controller_archive_digest, ["studio", "maintenance"]],
    [manifest.supabase_dump, manifest.supabase_dump_digest, null],
  ];
  for (const [name, digest, roots] of checks) {
    if (basename(name) !== name || !SHA256.test(digest)) {
      throw new Error("invalid_hosted_checkpoint_manifest");
    }
    const path = join(directory, name);
    if (await sha256File(path) !== digest) throw new Error("checkpoint_digest_mismatch");
    if (roots !== null) await verifyTar(path, roots);
  }
  await runDatabaseTool(pgTool("WORLDSTREAM_PG_RESTORE", "pg_restore"), [
    "--list",
    join(directory, manifest.supabase_dump),
  ]);
}

async function recordDeployment(revision, bytes) {
  return supabaseRpc("record_hosted_deployment_revision_v1", {
    p_deployment_revision_digest: hexBytea(digestBytes(bytes)),
    p_canonical_document: `\\x${bytes.toString("hex")}`,
    p_source_revision: revision.source_revision,
    p_image_digest: revision.image_digest,
    p_supabase_migration_head: revision.supabase_migration_head,
    p_listing_revision_digests: revision.listing_revision_digests,
  });
}

async function recordCheckpointEvent(manifest, manifestDigest, eventKind) {
  return supabaseRpc("record_hosted_recovery_checkpoint_v1", {
    p_checkpoint_id: manifest.checkpoint_id,
    p_event_kind: eventKind,
    p_deployment_revision_digest: hexBytea(manifest.deployment_revision_digest),
    p_manifest_digest: hexBytea(manifestDigest),
    p_worldstream_backup_digest: hexBytea(manifest.runtime_archive_digest),
    p_controller_archive_digest: hexBytea(manifest.controller_archive_digest),
    p_supabase_dump_digest: hexBytea(manifest.supabase_dump_digest),
  });
}

async function supabaseRpc(name, args) {
  const endpoint = new URL(requiredEnvironment("SUPABASE_URL"));
  const localAllowed = process.env.WORLDSTREAM_ALLOW_LOCAL_OPERATIONS === "visible-local-only";
  if (endpoint.protocol !== "https:" && !(localAllowed && endpoint.hostname === "127.0.0.1")) {
    throw new Error("secure_supabase_url_required");
  }
  endpoint.pathname = `/rest/v1/rpc/${name}`;
  endpoint.search = "";
  endpoint.hash = "";
  const key = (await readProtectedFile(
    resolve(requiredEnvironment("SUPABASE_DATA_SECRET_KEY_FILE")),
    512,
  )).toString("utf8");
  if (key.length < 20 || !/^[!-~]+$/u.test(key)) throw new Error("invalid_supabase_secret_key");
  let response;
  try {
    response = await fetch(endpoint, {
      method: "POST",
      headers: {
        apikey: key,
        authorization: `Bearer ${key}`,
        "content-type": "application/json",
      },
      body: JSON.stringify(args),
      redirect: "error",
      signal: AbortSignal.timeout(15_000),
    });
  } catch {
    throw new Error("supabase_operation_unavailable");
  }
  if (!response.ok) throw new Error(`supabase_operation_rejected:${response.status}`);
  const value = await response.json();
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("supabase_operation_invalid_response");
  }
  return value;
}

async function requireDatabaseTooling() {
  for (const tool of [
    pgTool("WORLDSTREAM_PG_DUMP", "pg_dump"),
    pgTool("WORLDSTREAM_PG_RESTORE", "pg_restore"),
    pgTool("WORLDSTREAM_PSQL", "psql"),
  ]) {
    const result = await run(tool, ["--version"], true);
    if (!/\b17\.11\b/u.test(result.stdout)) throw new Error("postgresql_17_11_tools_required");
  }
}

async function requireProtectedServiceFile() {
  if (process.env.PGPASSWORD !== undefined || process.env.DATABASE_URL !== undefined) {
    throw new Error("ambiguous_database_secret_forbidden");
  }
  await readProtectedFile(resolve(requiredEnvironment("PGSERVICEFILE")), 65_536);
}

function runDatabaseTool(command, args, capture = false) {
  const environment = { ...process.env };
  delete environment.PGPASSWORD;
  delete environment.DATABASE_URL;
  delete environment.SUPABASE_DATA_SECRET_KEY;
  return run(command, args, capture, environment);
}

function run(command, args, capture = false, environment = process.env) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, {
      env: environment,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (value) => { stdout = (stdout + value).slice(-1_048_576); });
    child.stderr.on("data", (value) => { stderr = (stderr + value).slice(-65_536); });
    child.once("error", () => rejectPromise(new Error("required_checkpoint_command_unavailable")));
    child.once("exit", (code) => {
      if (code === 0) resolvePromise({ stdout: capture ? stdout : "", stderr: "" });
      else rejectPromise(new Error(`checkpoint_command_failed:${stderr.trim().slice(0, 256)}`));
    });
  });
}

async function verifyTar(path, roots) {
  const result = await run("tar", ["--list", "--file", path], true);
  validateTarEntries(result.stdout, roots);
}

async function copyExclusive(source, destination) {
  await copyFile(source, destination, constants.COPYFILE_EXCL);
  await chmod(destination, 0o600);
}

async function writeExclusiveJson(path, value) {
  const handle = await open(path, "wx", 0o600);
  await handle.writeFile(`${JSON.stringify(value)}\n`, "utf8");
  await handle.sync();
  await handle.close();
}

async function readProtectedFile(path, maximumBytes) {
  await validateProtectedFile(path, maximumBytes);
  return readFile(path);
}

async function validateProtectedFile(path, maximumBytes) {
  const stat = await lstat(path);
  if (
    !stat.isFile() ||
    stat.isSymbolicLink() ||
    stat.size < 1 ||
    stat.size > maximumBytes ||
    (stat.mode & 0o077) !== 0
  ) {
    throw new Error("protected_checkpoint_file_invalid");
  }
}

async function sha256File(path) {
  const stat = await lstat(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size < 1) {
    throw new Error("checkpoint_artifact_invalid");
  }
  const hash = createHash("sha256");
  await new Promise((resolvePromise, rejectPromise) => {
    const stream = createReadStream(path);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.once("error", rejectPromise);
    stream.once("end", resolvePromise);
  });
  return `sha256:${hash.digest("hex")}`;
}

function digestBytes(bytes) {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

function hexBytea(digest) {
  if (!SHA256.test(digest)) throw new Error("invalid_sha256_digest");
  return `\\x${digest.slice(7)}`;
}

function pgTool(name, fallback) {
  return process.env[name] ?? fallback;
}

function databaseService(value) {
  if (!SERVICE_NAME.test(value)) throw new Error("invalid_database_service_name");
  return value;
}

function hostedInstallationId() {
  const value = process.env.WORLDSTREAM_HOSTED_INSTALLATION_ID ?? "fly-primary";
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u.test(value)) {
    throw new Error("invalid_hosted_installation_id");
  }
  return value;
}

function requiredEnvironment(name) {
  const value = process.env[name];
  if (typeof value !== "string" || value.length < 1 || value.length > 2048) {
    throw new Error(`${name.toLowerCase()}_required`);
  }
  return value;
}

function flags(args) {
  const values = new Map();
  for (let index = 0; index < args.length; index += 2) {
    const name = args[index];
    const value = args[index + 1];
    if (name === undefined || value === undefined || !/^--[a-z][a-z-]*$/u.test(name)) {
      throw new Error("invalid_checkpoint_arguments");
    }
    const key = name.slice(2);
    const retained = values.get(key) ?? [];
    retained.push(value);
    values.set(key, retained);
  }
  return values;
}

function requiredOption(options, name) {
  const values = options.get(name);
  if (values?.length !== 1 || values[0].length < 1) throw new Error(`missing_${name}`);
  return values[0];
}

function optionList(options, name) {
  const values = options.get(name) ?? [];
  if (values.length < 1) throw new Error(`missing_${name}`);
  return values;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : "hosted_checkpoint_failed"}\n`);
    process.exitCode = 1;
  });
}
