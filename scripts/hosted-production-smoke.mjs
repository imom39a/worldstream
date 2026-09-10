import { spawn } from "node:child_process";
import { chmod, copyFile, lstat, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { hostedDevelopmentListingAllowlist } from "./hosted-dev.mjs";
import { hostedGatewayConfiguration } from "./hosted-runtime.mjs";

const REPOSITORY_ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));
const CONTROLLER_AUTHORITY = "hosted-smoke-controller-authority-000000000000";
const SERVICE_AUTHORITY = "hosted-smoke-service-authority-000000000000000";

async function main() {
  const gatewayConfiguration = hostedGatewayConfiguration();
  const binaryRoot = hostedSmokeBinaryRoot();
  for (const binary of [
    "worldstreamctl",
    "worldstreamd",
    "worldstream-studio-supervisor",
    "worldstream-assignment-mcp",
    "worldstream-managed-agent-host",
    "worldstream-hosted-gateway",
    "worldstream-hosted-artifact-digest",
  ]) await requireExecutable(join(binaryRoot, binary));

  const temporary = await mkdtemp(join(tmpdir(), "worldstream-hosted-production-smoke-"));
  await chmod(temporary, 0o700);
  const volumeRoot = join(temporary, "volume");
  const assetRoot = join(temporary, "assets");
  const ephemeralRoot = join(temporary, "run");
  const secretRoot = join(ephemeralRoot, "secrets");
  await Promise.all([
    mkdir(volumeRoot, { mode: 0o700 }),
    mkdir(assetRoot, { mode: 0o700 }),
    mkdir(secretRoot, { recursive: true, mode: 0o700 }),
  ]);
  await Promise.all([
    ...hostedSmokeClientAssets(REPOSITORY_ROOT, assetRoot).map(({ source, destination }) =>
      copyFile(source, destination)),
    copyFile(
      join(REPOSITORY_ROOT, "packs", "midnight-archive", "releases", "0.1.0", "worldstream-midnight-archive-de3cd1d9fa45087b69cb107a663596305864c350f620fe4d7260e0341214d47a.wspack"),
      join(assetRoot, "midnight-archive.wspack"),
    ),
    copyFile(
      join(REPOSITORY_ROOT, "config", "activity-clients", "releases", "inspector-web-v2.json"),
      join(assetRoot, "inspector-web.json"),
    ),
    copyFile(
      join(REPOSITORY_ROOT, "config", "activity-clients", "hosted-local-bindings.json"),
      join(assetRoot, "activity-client-bindings.json"),
    ),
  ]);
  const digest = (await runHostedSmokeCommand(
    join(binaryRoot, "worldstream-hosted-artifact-digest"),
    [join(binaryRoot, "worldstream-managed-agent-host")],
    process.env,
    true,
  )).stdout.trim();
  await writeProtected(join(assetRoot, "managed-agent-host.blake3"), `${digest}\n`);
  const secrets = {
    authority: join(secretRoot, "authority-bootstrap"),
    controller: join(secretRoot, "controller-authority"),
    service: join(secretRoot, "vercel-service-authority"),
    openrouter: join(secretRoot, "openrouter-api-key"),
  };
  await Promise.all([
    writeProtected(secrets.authority, "0123456789abcdef0123456789abcdef"),
    writeProtected(secrets.controller, CONTROLLER_AUTHORITY),
    writeProtected(secrets.service, SERVICE_AUTHORITY),
    writeProtected(secrets.openrouter, "hosted-smoke-openrouter-key-0000000000000000"),
  ]);
  const environment = {
    ...process.env,
    NODE_ENV: "production",
    WORLDSTREAM_DEPLOYMENT_ENVIRONMENT: "production",
    WORLDSTREAM_HOSTED_INSTALLATION_ID: "hosted-smoke",
    WORLDSTREAM_DEPLOYMENT_VERSION: "a".repeat(40),
    WORLDSTREAM_LISTING_ALLOWLIST: hostedDevelopmentListingAllowlist(),
    WORLDSTREAM_PUBLIC_AUTHORITY: gatewayConfiguration.publicAuthority,
    WORLDSTREAM_HOSTED_CLIENT_ORIGIN: "https://worldstream.example",
    WORLDSTREAM_HOSTED_VOLUME_ROOT: volumeRoot,
    WORLDSTREAM_HOSTED_ASSET_ROOT: assetRoot,
    WORLDSTREAM_HOSTED_BINARY_ROOT: binaryRoot,
    WORLDSTREAM_HOSTED_EPHEMERAL_ROOT: ephemeralRoot,
    WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET_FILE: secrets.authority,
    WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY_FILE: secrets.controller,
    WORLDSTREAM_VERCEL_SERVICE_AUTHORITY_FILE: secrets.service,
    WORLDSTREAM_OPENROUTER_API_KEY_FILE: secrets.openrouter,
    WORLDSTREAM_HOSTED_FATAL_READINESS_MS: "30000",
  };
  let appliance = null;
  try {
    await assertPortsClosed([gatewayConfiguration.port, 9410, 9420]);
    appliance = startAppliance(environment);
    await waitForReady(appliance, gatewayConfiguration);
    const config = join(ephemeralRoot, "worldstream.toml");
    const controllerState = join(volumeRoot, "studio");
    const setup = join(temporary, "retained-room.json");
    const ctl = (args, capture = true) => runHostedSmokeCommand(
      join(binaryRoot, "worldstreamctl"),
      [
        "--config", config,
        ...args,
        "--state-dir", controllerState,
        "--controller", "127.0.0.1:9420",
        "--json",
      ],
      { ...environment, WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY: CONTROLLER_AUTHORITY },
      capture,
      `${args[0]} ${args[1]}`,
    );
    await ctl([
      "room", "example",
      "--pack", "worldstream.agent-heist@0.5.0",
      "--output", setup,
    ]);
    const created = JSON.parse((await runRoomCreateWithRetainedSetupRetry(
      ctl,
      ["room", "create", "--file", setup],
    )).stdout);
    const roomId = created.room_id;
    const operationId = created.operation_id;
    if (typeof roomId !== "string" || typeof operationId !== "string") {
      throw new Error("smoke_room_creation_incomplete");
    }
    await assertRoomRetained(ctl, roomId);

    await stopAppliance(appliance);
    appliance = null;
    await assertPortsClosed([gatewayConfiguration.port, 9410, 9420]);
    appliance = startAppliance(environment);
    await waitForReady(appliance, gatewayConfiguration);
    await assertRoomRetained(ctl, roomId);
    const credential = join(temporary, "reentry-membership.json");
    await ctl([
      "client", "export-credentials",
      "--operation", operationId,
      "--seat", "navigator",
      "--output", credential,
    ]);
    const credentialStat = await lstat(credential);
    if (!credentialStat.isFile() || credentialStat.size < 2 || (credentialStat.mode & 0o077) !== 0) {
      throw new Error("smoke_reentry_credential_unavailable");
    }
    process.stdout.write(`${JSON.stringify({
      status: "passed",
      room_id: roomId,
      same_volume_restart: "retained",
      explicit_reentry: "available",
    })}\n`);
  } finally {
    if (appliance !== null) await stopAppliance(appliance);
    await rm(temporary, { recursive: true, force: true });
  }
}

export function hostedSmokeClientAssets(repositoryRoot, assetRoot) {
  return [
    ["agent-heist-web-v9.json", "agent-heist-web.json"],
    ["agent-heist-web-v8.json", "agent-heist-web-v8.json"],
    ["agent-heist-web-v7.json", "agent-heist-web-v7.json"],
    ["midnight-archive-web-v13.json", "midnight-archive-web.json"],
    ["midnight-archive-web-v12.json", "midnight-archive-web-v12.json"],
    ["midnight-archive-web-v10.json", "midnight-archive-web-v10.json"],
  ].map(([release, installed]) => ({
    source: join(repositoryRoot, "config", "activity-clients", "releases", release),
    destination: join(assetRoot, installed),
  }));
}

// `hosted:package:smoke` builds debug binaries. Honor Cargo's standard target
// override so the smoke can isolate build artifacts without accidentally
// exercising a stale repository `target/debug` executable.
export function hostedSmokeBinaryRoot(environment = process.env) {
  const targetRoot = environment.CARGO_TARGET_DIR === undefined
    ? join(REPOSITORY_ROOT, "target")
    : resolve(REPOSITORY_ROOT, environment.CARGO_TARGET_DIR);
  return join(targetRoot, "debug");
}

function startAppliance(environment) {
  const child = spawn("node", [join(REPOSITORY_ROOT, "scripts", "hosted-runtime.mjs")], {
    cwd: REPOSITORY_ROOT,
    env: environment,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  for (const stream of [child.stdout, child.stderr]) {
    stream.setEncoding("utf8");
    stream.on("data", (value) => { output = (output + value).slice(-65_536); });
  }
  return { child, output: () => output };
}

async function waitForReady(appliance, gatewayConfiguration) {
  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    if (appliance.child.exitCode !== null || appliance.child.signalCode !== null) {
      throw new Error(`hosted_appliance_exited:${appliance.output().slice(-512)}`);
    }
    try {
      const response = await fetch(`${gatewayConfiguration.loopbackOrigin}/readyz`, {
        signal: AbortSignal.timeout(2_000),
      });
      if (response.status === 200) return;
    } catch {
      // Bounded startup retry.
    }
    await delay(250);
  }
  throw new Error(`hosted_appliance_not_ready:${appliance.output().slice(-512)}`);
}

async function stopAppliance(appliance) {
  if (appliance.child.exitCode !== null || appliance.child.signalCode !== null) return;
  appliance.child.kill("SIGTERM");
  await new Promise((resolvePromise, rejectPromise) => {
    const timer = setTimeout(() => {
      appliance.child.kill("SIGKILL");
      rejectPromise(new Error("hosted_appliance_shutdown_timeout"));
    }, 125_000);
    appliance.child.once("exit", () => {
      clearTimeout(timer);
      resolvePromise();
    });
  });
}

async function assertRoomRetained(ctl, roomId) {
  const listed = JSON.parse((await ctl(["room", "list"])).stdout);
  if (
    !Array.isArray(listed.rooms?.rooms) ||
    !listed.rooms.rooms.some((room) => room.room_id === roomId)
  ) {
    throw new Error("acknowledged_room_history_missing");
  }
}

async function assertPortsClosed(ports) {
  const results = await Promise.all(ports.map(async (port) => {
    try {
      await fetch(`http://127.0.0.1:${port}/healthz`, { signal: AbortSignal.timeout(500) });
      return false;
    } catch {
      return true;
    }
  }));
  if (results.some((closed) => !closed)) throw new Error("hosted_smoke_port_in_use");
}

async function requireExecutable(path) {
  const stat = await lstat(path);
  if (!stat.isFile() || (stat.mode & 0o111) === 0) throw new Error("hosted_smoke_binary_missing");
}

async function writeProtected(path, value) {
  await writeFile(path, value, { mode: 0o600, flag: "wx" });
  await chmod(path, 0o600);
}

const OPERATOR_COMMANDS = new Set([
  "room example", "room create", "room list", "client export-credentials",
]);
const SETUP_STAGES = new Set(["room_creation", "member_capability", "runner_capability"]);
// One initial create plus one exact re-observation for each legitimate setup
// boundary. A retained operation may advance from room creation to member and
// then runner capability across these attempts, but never indefinitely.
const MAX_ROOM_CREATE_ATTEMPTS = 1 + SETUP_STAGES.size;
const FAILURE_STATUS = new Map([
  ["setup_incomplete", "partial"],
  ["operation_failed", "failed"],
  ["operation_rejected", "rejected"],
  ["invalid_arguments", "invalid_arguments"],
  ["controller_unavailable", "unavailable"],
  ["stale_evidence", "unavailable"],
  ["not_implemented", "unavailable"],
]);
const FAILURE_FIELDS = new Set([
  "schema", "command", "status", "code", "message", "next_action",
  "operation_id", "room_id", "stage", "room_operation",
]);

function safeOperatorFailure(stdout, expectedCommand) {
  if (!OPERATOR_COMMANDS.has(expectedCommand) || Buffer.byteLength(stdout) > 16_384) return null;
  let value;
  try { value = JSON.parse(stdout); } catch { return null; }
  if (value === null || typeof value !== "object" || Array.isArray(value) ||
    Object.keys(value).some((key) => !FAILURE_FIELDS.has(key)) ||
    value.schema !== "worldstream/operator-command/v1" ||
    value.command !== expectedCommand || !FAILURE_STATUS.has(value.code) ||
    FAILURE_STATUS.get(value.code) !== value.status ||
    typeof value.message !== "string" || typeof value.next_action !== "string") return null;
  if (value.code === "setup_incomplete") {
    if (value.command !== "room create" || !SETUP_STAGES.has(value.stage) ||
      typeof value.operation_id !== "string" ||
      (value.room_id !== undefined && value.room_id !== null && typeof value.room_id !== "string")) return null;
    // The real CLI attaches operational detail. Admit its object shape but never
    // include any of it in diagnostics; the whole envelope remains size-bounded.
    if (value.room_operation !== undefined && (value.room_operation === null ||
      typeof value.room_operation !== "object" || Array.isArray(value.room_operation))) return null;
  } else if (value.stage !== undefined || value.operation_id !== undefined || value.room_id !== undefined || value.room_operation !== undefined) {
    return null;
  }
  // Never propagate descriptive text, identifiers, next_action, or extra data.
  return {
    command: value.command, status: value.status, code: value.code,
    ...(value.stage === undefined ? {} : { stage: value.stage }),
  };
}

export function runHostedSmokeCommand(command, args, environment, capture, expectedCommand = null) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, {
      cwd: REPOSITORY_ROOT,
      env: environment,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stdoutTruncated = false;
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (value) => {
      const combined = stdout + value;
      stdoutTruncated ||= combined.length > 1_048_576;
      stdout = combined.slice(-1_048_576);
    });
    child.stderr.resume();
    child.once("error", () => rejectPromise(new Error("hosted_smoke_command_unavailable")));
    // Wait until the captured streams close, not just until the process exits.
    child.once("close", (code) => {
      if (code === 0) resolvePromise({ stdout: capture ? stdout : "", stderr: "" });
      else {
        const diagnostic = stdoutTruncated ? null : safeOperatorFailure(stdout, expectedCommand);
        const error = new Error(`hosted_smoke_command_failed${diagnostic === null ? "" : `:${JSON.stringify(diagnostic)}`}`);
        if (diagnostic !== null) {
          Object.defineProperty(error, "operatorFailure", {
            configurable: false,
            enumerable: false,
            value: Object.freeze(diagnostic),
            writable: false,
          });
        }
        rejectPromise(error);
      }
    });
  });
}

export async function runRoomCreateWithRetainedSetupRetry(execute, commandArgs) {
  if (typeof execute !== "function" || !Array.isArray(commandArgs) ||
    commandArgs[0] !== "room" || commandArgs[1] !== "create") {
    throw new Error("hosted_smoke_invalid_room_create_command");
  }
  // Freeze one exact argv vector. A retry is a re-observation of the retained
  // operation, never a newly constructed Room create request.
  const exactCommand = Object.freeze([...commandArgs]);
  let attempts = 0;
  while (true) {
    attempts += 1;
    try {
      return await execute(exactCommand);
    } catch (error) {
      if (!isRetainedSetupIncomplete(error) || attempts >= MAX_ROOM_CREATE_ATTEMPTS) {
        throw error;
      }
    }
  }
}

function isRetainedSetupIncomplete(error) {
  const failure = error?.operatorFailure;
  return failure !== null && typeof failure === "object" &&
    failure.command === "room create" &&
    failure.status === "partial" &&
    failure.code === "setup_incomplete" &&
    SETUP_STAGES.has(failure.stage);
}

function delay(milliseconds) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : "hosted_production_smoke_failed"}\n`);
    process.exitCode = 1;
  });
}
