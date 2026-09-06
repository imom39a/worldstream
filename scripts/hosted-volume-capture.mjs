import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { constants } from "node:fs";
import { access, chmod, lstat, mkdir, open } from "node:fs/promises";
import { createReadStream } from "node:fs";
import { connect } from "node:net";
import { basename, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const DIGEST = /^sha256:[0-9a-f]{64}$/u;

export async function captureHostedVolume({
  volumeRoot,
  checkpointId,
  deploymentVersion,
  waitForClosed = waitForManagedPortsClosed,
}) {
  const root = resolve(volumeRoot);
  if (!UUID.test(checkpointId)) throw new Error("invalid_checkpoint_id");
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u.test(deploymentVersion)) {
    throw new Error("invalid_deployment_version");
  }
  await requireSafeDirectory(root);
  const maintenance = join(root, "maintenance");
  const closed = join(maintenance, "closed");
  const recoveryFence = join(maintenance, "recovery-house-calls-fenced");
  if (!(await exists(closed)) && !(await exists(recoveryFence))) {
    throw new Error("maintenance_required_before_capture");
  }
  for (const child of ["runtime", "studio", "maintenance", "checkpoints"]) {
    await requireSafeDirectory(join(root, child));
  }
  await waitForClosed();

  const destination = join(root, "checkpoints", checkpointId);
  try {
    await mkdir(destination, { mode: 0o700 });
  } catch (error) {
    if (error?.code === "EEXIST") throw new Error("checkpoint_already_exists");
    throw new Error("hosted_volume_capture_failed", { cause: error });
  }
  await chmod(destination, 0o700);
  const runtimeArchive = join(destination, "worldstream-runtime.tar");
  const controllerArchive = join(destination, "worldstream-controller.tar");
  try {
    await run("tar", ["--create", "--file", runtimeArchive, "--directory", root, "runtime"]);
    await run("tar", [
      "--create",
      "--file",
      controllerArchive,
      "--directory",
      root,
      "studio",
      "maintenance",
    ]);
    await Promise.all([chmod(runtimeArchive, 0o600), chmod(controllerArchive, 0o600)]);
    const [runtimeDigest, controllerDigest] = await Promise.all([
      sha256File(runtimeArchive),
      sha256File(controllerArchive),
    ]);
    const receipt = {
      schema: "worldstream/hosted-volume-capture/v1",
      checkpoint_id: checkpointId,
      deployment_version: deploymentVersion,
      runtime_archive: basename(runtimeArchive),
      runtime_archive_digest: runtimeDigest,
      controller_archive: basename(controllerArchive),
      controller_archive_digest: controllerDigest,
      captured_at: new Date().toISOString(),
    };
    const receiptPath = join(destination, "capture.json");
    const handle = await open(receiptPath, "wx", 0o600);
    await handle.writeFile(`${JSON.stringify(receipt)}\n`, "utf8");
    await handle.sync();
    await handle.close();
    return { destination, receipt };
  } catch (error) {
    throw new Error("hosted_volume_capture_failed", { cause: error });
  }
}

export function validateCaptureReceipt(value) {
  if (
    value === null ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    value.schema !== "worldstream/hosted-volume-capture/v1" ||
    typeof value.checkpoint_id !== "string" ||
    !UUID.test(value.checkpoint_id) ||
    typeof value.deployment_version !== "string" ||
    !/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u.test(value.deployment_version) ||
    value.runtime_archive !== "worldstream-runtime.tar" ||
    value.controller_archive !== "worldstream-controller.tar" ||
    typeof value.runtime_archive_digest !== "string" ||
    !DIGEST.test(value.runtime_archive_digest) ||
    typeof value.controller_archive_digest !== "string" ||
    !DIGEST.test(value.controller_archive_digest) ||
    typeof value.captured_at !== "string" ||
    Number.isNaN(Date.parse(value.captured_at)) ||
    Object.keys(value).length !== 8
  ) {
    throw new Error("invalid_volume_capture_receipt");
  }
  return value;
}

async function main() {
  const args = process.argv.slice(2);
  const checkpointId = args.length === 1 ? args[0] : randomUUID();
  if (args.length > 1) throw new Error("usage: hosted-volume-capture [checkpoint-uuid]");
  const result = await captureHostedVolume({
    volumeRoot: process.env.WORLDSTREAM_HOSTED_VOLUME_ROOT ?? "/var/lib/worldstream",
    checkpointId,
    deploymentVersion: requiredEnvironment("WORLDSTREAM_DEPLOYMENT_VERSION"),
  });
  process.stdout.write(`${JSON.stringify(result.receipt)}\n`);
}

async function waitForManagedPortsClosed() {
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    const openPorts = await Promise.all([9410, 9420].map(loopbackPortOpen));
    if (openPorts.every((value) => !value)) return;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 500));
  }
  throw new Error("managed_services_did_not_stop");
}

function loopbackPortOpen(port) {
  return new Promise((resolvePromise) => {
    const socket = connect({ host: "127.0.0.1", port });
    socket.setTimeout(250);
    socket.once("connect", () => {
      socket.destroy();
      resolvePromise(true);
    });
    const closed = () => {
      socket.destroy();
      resolvePromise(false);
    };
    socket.once("error", closed);
    socket.once("timeout", closed);
  });
}

async function requireSafeDirectory(path) {
  const stat = await lstat(path);
  if (!stat.isDirectory() || stat.isSymbolicLink() || (stat.mode & 0o077) !== 0) {
    throw new Error("unsafe_hosted_volume_directory");
  }
}

async function sha256File(path) {
  const stat = await lstat(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size < 1) {
    throw new Error("invalid_checkpoint_artifact");
  }
  const hash = createHash("sha256");
  await new Promise((resolvePromise, rejectPromise) => {
    const input = createReadStream(path);
    input.on("data", (chunk) => hash.update(chunk));
    input.once("error", rejectPromise);
    input.once("end", resolvePromise);
  });
  return `sha256:${hash.digest("hex")}`;
}

function run(command, args) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, { stdio: ["ignore", "ignore", "pipe"] });
    let stderr = "";
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (value) => { stderr = (stderr + value).slice(-16_384); });
    child.once("error", () => rejectPromise(new Error("checkpoint_command_unavailable")));
    child.once("exit", (code) => {
      if (code === 0) resolvePromise();
      else rejectPromise(new Error(`checkpoint_command_failed:${stderr.trim().slice(0, 256)}`));
    });
  });
}

async function exists(path) {
  try {
    await access(path, constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

function requiredEnvironment(name) {
  const value = process.env[name];
  if (typeof value !== "string" || value.length < 1 || value.length > 512) {
    throw new Error(`${name.toLowerCase()}_required`);
  }
  return value;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : "hosted_volume_capture_failed"}\n`);
    process.exitCode = 1;
  });
}
