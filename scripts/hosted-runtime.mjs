import { spawn } from "node:child_process";
import { constants } from "node:fs";
import {
  access,
  chmod,
  link,
  lstat,
  mkdir,
  readFile,
  readdir,
  rm,
  writeFile,
} from "node:fs/promises";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const DEFAULT_VOLUME_ROOT = "/var/lib/worldstream";
const DEFAULT_ASSET_ROOT = "/opt/worldstream/hosted";
const DEFAULT_BINARY_ROOT = "/usr/local/bin";
const MAX_CAPTURE_BYTES = 1_048_576;
const DEFAULT_FATAL_READINESS_MS = 300_000;
const MONITOR_INTERVAL_MS = 2_000;

export function hostedGatewayConfiguration(environment = process.env) {
  const raw = environment.WORLDSTREAM_HOSTED_GATEWAY_PORT ?? "8080";
  if (!/^[1-9][0-9]{0,4}$/u.test(raw)) throw new Error("invalid_hosted_gateway_port");
  const port = Number(raw);
  if (!Number.isSafeInteger(port) || port > 65_535 || port === 9410 || port === 9420) {
    throw new Error("invalid_hosted_gateway_port");
  }
  if (port !== 8080 && (environment.FLY_APP_NAME !== undefined || environment.FLY_MACHINE_ID !== undefined)) {
    throw new Error("fly_gateway_port_override_forbidden");
  }
  return Object.freeze({
    port,
    bind: `0.0.0.0:${port}`,
    loopbackOrigin: `http://127.0.0.1:${port}`,
    publicAuthority: `127.0.0.1:${port}`,
  });
}

export function hostedRuntimeLayout(environment = process.env) {
  const volumeRoot = resolve(environment.WORLDSTREAM_HOSTED_VOLUME_ROOT ?? DEFAULT_VOLUME_ROOT);
  const assetRoot = resolve(environment.WORLDSTREAM_HOSTED_ASSET_ROOT ?? DEFAULT_ASSET_ROOT);
  const binaryRoot = resolve(environment.WORLDSTREAM_HOSTED_BINARY_ROOT ?? DEFAULT_BINARY_ROOT);
  const ephemeralRoot = resolve(environment.WORLDSTREAM_HOSTED_EPHEMERAL_ROOT ?? "/run/worldstream");
  return {
    volumeRoot,
    assetRoot,
    binaryRoot,
    ephemeralRoot,
    runtimeData: join(volumeRoot, "runtime"),
    // The kernel's retained backup contract derives this exact sibling name
    // from the Runtime data directory. It contains Controller state only; no
    // Studio process or UI is shipped in the hosted appliance.
    controllerState: join(volumeRoot, "studio"),
    checkpointStaging: join(volumeRoot, "checkpoints"),
    maintenanceRoot: join(volumeRoot, "maintenance"),
    maintenanceMarker: join(volumeRoot, "maintenance", "closed"),
    recoveryFence: join(volumeRoot, "maintenance", "recovery-house-calls-fenced"),
    retainedRunnerRoot: join(volumeRoot, "retained-runner-executables"),
    generatedRoot: join(ephemeralRoot, "generated"),
    runtimeConfig: join(ephemeralRoot, "worldstream.toml"),
    authoritySecret: requiredPath(environment, "WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET_FILE"),
    controllerAuthority: requiredPath(
      environment,
      "WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY_FILE",
    ),
    serviceAuthority: requiredPath(environment, "WORLDSTREAM_VERCEL_SERVICE_AUTHORITY_FILE"),
    openRouterSecret: requiredPath(environment, "WORLDSTREAM_OPENROUTER_API_KEY_FILE"),
    ctl: join(binaryRoot, "worldstreamctl"),
    gateway: join(binaryRoot, "worldstream-hosted-gateway"),
    managedAgentHost: join(binaryRoot, "worldstream-managed-agent-host"),
    managedAgentHostDigest: join(assetRoot, "managed-agent-host.blake3"),
    artifactDigest: join(binaryRoot, "worldstream-hosted-artifact-digest"),
    clientRelease: join(assetRoot, "agent-heist-web.json"),
    retainedClientRelease: join(assetRoot, "agent-heist-web-v7.json"),
    inspectorRelease: join(assetRoot, "inspector-web.json"),
    clientBindings: join(assetRoot, "activity-client-bindings.json"),
  };
}

export function validateHostedRuntimeEnvironment(environment = process.env) {
  if (
    environment.NODE_ENV !== "production" ||
    environment.WORLDSTREAM_DEPLOYMENT_ENVIRONMENT !== "production"
  ) {
    throw new Error("hosted_runtime_requires_production_mode");
  }
  const forbidden = Object.keys(environment).filter(
    (name) => name.startsWith("WORLDSTREAM_DEVELOPMENT_") && environment[name] !== undefined,
  );
  if (forbidden.length > 0) throw new Error("development_substitute_forbidden");
  required(environment, "WORLDSTREAM_HOSTED_INSTALLATION_ID", /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u);
  required(environment, "WORLDSTREAM_DEPLOYMENT_VERSION", /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u);
  required(environment, "WORLDSTREAM_LISTING_ALLOWLIST", /^blake3:[0-9a-f]{64}(,blake3:[0-9a-f]{64}){0,63}$/u, 64 * 72 - 1);
  const authority = required(environment, "WORLDSTREAM_PUBLIC_AUTHORITY", /^[a-z0-9][a-z0-9.:-]{0,254}$/u);
  if (authority.includes("/") || authority.includes("@")) {
    throw new Error("invalid_public_authority");
  }
  const origin = new URL(required(environment, "WORLDSTREAM_HOSTED_CLIENT_ORIGIN"));
  if (
    origin.protocol !== "https:" ||
    origin.username !== "" ||
    origin.password !== "" ||
    origin.pathname !== "/" ||
    origin.search !== "" ||
    origin.hash !== "" ||
    !origin.hostname.includes(".") ||
    origin.hostname !== origin.hostname.toLowerCase() ||
    environment.WORLDSTREAM_HOSTED_CLIENT_ORIGIN !== `${origin.protocol}//${origin.host}`
  ) {
    throw new Error("invalid_client_origin");
  }
}

export function renderHostedRuntimeConfig({ dataDirectory, authoritySecret }) {
  return `# Generated at boot. Mutable data is kept on the Fly volume.
config_version = 1

[server]
bind = "127.0.0.1:9410"

[storage]
profile = "sqlite-bundled"
data_dir = ${JSON.stringify(dataDirectory)}
deployment_lineage = "hosted/fly-single-authority"
storage_epoch = 1

[authority.bootstrap]
secret_file = ${JSON.stringify(authoritySecret)}
`;
}

export function renderHouseRunnerTemplate(executable, digest) {
  if (!/^[0-9a-f]{64}$/u.test(digest)) throw new Error("invalid_runner_digest");
  return {
    schema: "worldstream/runner-template/v1",
    template_id: "openrouter-house",
    revision: "16",
    display_name: "Hosted OpenRouter House Agent",
    executable: { path: executable, blake3: digest },
    compatibility: [{
      activity_pack_id: "worldstream.agent-heist",
      exact_revisions: ["0.5.0"],
    }],
    capacity: { maximum_concurrent_invocations: 4 },
    health: { path: "/healthz", timeout_ms: 1_000, stale_after_ms: 60_000 },
    non_secret_environment: { WORLDSTREAM_RUNNER_MODE: "hosted-house" },
    secret_environment: [],
    // Instance IDs are unique across retained immutable template revisions.
    instances: [{ instance_id: "hosted-house-r16-01", health_address: "127.0.0.1:9606" }],
  };
}

/**
 * The hosted digest executable and its image asset use a raw 64-character
 * BLAKE3 hex value. Runner manifests use that same raw value in `blake3`.
 * Keep the comparison exact: a tagged contract digest is a distinct wire
 * representation and must not be silently accepted here.
 */
export function verifyManagedAgentHostDigest(assetDigest, observedDigest) {
  if (typeof assetDigest !== "string" || !/^[0-9a-f]{64}$/u.test(assetDigest)) {
    throw new Error("managed_agent_host_digest_invalid");
  }
  if (typeof observedDigest !== "string" || observedDigest !== assetDigest) {
    throw new Error("managed_agent_host_digest_mismatch");
  }
  return assetDigest;
}

/**
 * Persist one immutable Runner executable under its verified content digest.
 * Existing content-addressed targets are reusable only when their bytes are
 * equal to the source; this function never overwrites a retained executable.
 */
export async function retainRunnerExecutable({ source, retainedRoot, digest, sourceBytes = undefined }) {
  if (!/^[0-9a-f]{64}$/u.test(digest)) throw new Error("invalid_runner_digest");
  const sourceMetadata = await lstat(source);
  if (!sourceMetadata.isFile() || sourceMetadata.isSymbolicLink() || sourceMetadata.size < 1) {
    throw new Error("runner_source_invalid");
  }
  await mkdir(retainedRoot, { recursive: true, mode: 0o700 });
  await chmod(retainedRoot, 0o700);
  const rootMetadata = await lstat(retainedRoot);
  if (!rootMetadata.isDirectory() || rootMetadata.isSymbolicLink() || (rootMetadata.mode & 0o077) !== 0) {
    throw new Error("retained_runner_root_invalid");
  }
  const digestDirectory = join(retainedRoot, `blake3-${digest}`);
  await mkdir(digestDirectory, { recursive: true, mode: 0o700 });
  await chmod(digestDirectory, 0o700);
  const directoryMetadata = await lstat(digestDirectory);
  if (!directoryMetadata.isDirectory() || directoryMetadata.isSymbolicLink() || (directoryMetadata.mode & 0o077) !== 0) {
    throw new Error("retained_runner_directory_invalid");
  }
  const target = join(digestDirectory, "worldstream-managed-agent-host");
  const bytes = sourceBytes ?? await readFile(source);
  if (!(bytes instanceof Uint8Array) || bytes.byteLength < 1) throw new Error("runner_source_invalid");
  try {
    const existing = await lstat(target);
    if (!existing.isFile() || existing.isSymbolicLink() || (existing.mode & 0o077) !== 0) {
      throw new Error("retained_runner_invalid");
    }
    if (!Buffer.from(await readFile(target)).equals(Buffer.from(bytes))) throw new Error("retained_runner_digest_collision");
    await chmod(target, 0o700);
    return target;
  } catch (error) {
    if (!(error instanceof Error) || !("code" in error) || error.code !== "ENOENT") throw error;
  }
  const temporary = join(digestDirectory, `.runner-${process.pid}-${Math.random().toString(16).slice(2)}`);
  try {
    await writeFile(temporary, bytes, { flag: "wx", mode: 0o700 });
    // `link` is no-clobber. A concurrent installer may only win with the
    // same bytes; a distinct byte sequence at this digest fails closed.
    try {
      await link(temporary, target);
    } catch (error) {
      if (!(error instanceof Error) || !("code" in error) || error.code !== "EEXIST") throw error;
      const existing = await lstat(target);
      if (!existing.isFile() || existing.isSymbolicLink() || (existing.mode & 0o077) !== 0
          || !Buffer.from(await readFile(target)).equals(Buffer.from(bytes))) {
        throw new Error("retained_runner_digest_collision");
      }
    }
  } finally {
    await rm(temporary, { force: true });
  }
  await chmod(target, 0o700);
  return target;
}

export function hostedStatusReady(value) {
  return value !== null && typeof value === "object" &&
    value.status === "complete" && value.code === "complete" &&
    value.server !== null && typeof value.server === "object" &&
    value.server.runtime === "ready";
}

async function main() {
  validateHostedRuntimeEnvironment();
  const gatewayConfiguration = hostedGatewayConfiguration();
  const layout = hostedRuntimeLayout();
  await prepareLayout(layout);
  const controllerAuthority = await readProtectedSecret(layout.controllerAuthority);
  await writeFile(
    layout.runtimeConfig,
    renderHostedRuntimeConfig({
      dataDirectory: layout.runtimeData,
      authoritySecret: layout.authoritySecret,
    }),
    { mode: 0o600 },
  );
  await initializeInstallation(layout, controllerAuthority);

  let managedStarted = false;
  let shuttingDown = false;
  const maintenanceAtBoot = await maintenanceActive(layout);
  if (!maintenanceAtBoot) {
    await startManaged(layout, controllerAuthority);
    managedStarted = true;
  }
  const gateway = startGateway(layout, gatewayConfiguration);
  const signal = shutdownSignal();

  try {
    await waitForGatewayLiveness(gateway);
    let maintenance = maintenanceAtBoot;
    let readinessFailedAt = null;
    while (!shuttingDown) {
      const winner = await Promise.race([
        signal.then(() => "signal"),
        gateway.exited.then(() => "gateway"),
        delay(MONITOR_INTERVAL_MS).then(() => "tick"),
      ]);
      if (winner === "signal") {
        shuttingDown = true;
        break;
      }
      if (winner === "gateway") throw new Error("required_gateway_exited");

      const nowMaintenance = await maintenanceActive(layout);
      if (nowMaintenance) {
        maintenance = true;
        readinessFailedAt = null;
        if (managedStarted) {
          await stopManaged(layout, controllerAuthority);
          managedStarted = false;
        }
        continue;
      }
      if (maintenance) {
        await startManaged(layout, controllerAuthority);
        managedStarted = true;
        maintenance = false;
      }
      const ready = await requiredServicesReady(layout, controllerAuthority, gatewayConfiguration);
      if (ready) {
        readinessFailedAt = null;
      } else {
        readinessFailedAt ??= Date.now();
        if (Date.now() - readinessFailedAt >= fatalReadinessMs(process.env)) {
          throw new Error("sustained_required_service_failure");
        }
      }
    }
  } finally {
    shuttingDown = true;
    await stopGateway(gateway.child);
    if (managedStarted) await stopManaged(layout, controllerAuthority);
  }
}

async function prepareLayout(layout) {
  for (const path of [
    layout.runtimeData,
    layout.controllerState,
    layout.checkpointStaging,
    layout.maintenanceRoot,
    layout.retainedRunnerRoot,
    layout.ephemeralRoot,
    layout.generatedRoot,
  ]) {
    await mkdir(path, { recursive: true, mode: 0o700 });
    await chmod(path, 0o700);
    const stat = await lstat(path);
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error("unsafe_hosted_directory");
  }
  for (const path of [
    layout.ctl,
    layout.gateway,
    layout.managedAgentHost,
    layout.managedAgentHostDigest,
    layout.artifactDigest,
    layout.clientRelease,
    layout.retainedClientRelease,
    layout.inspectorRelease,
    layout.clientBindings,
    layout.authoritySecret,
    layout.controllerAuthority,
    layout.serviceAuthority,
    layout.openRouterSecret,
  ]) await access(path, constants.R_OK);
  await Promise.all([
    readProtectedSecret(layout.authoritySecret),
    readProtectedSecret(layout.controllerAuthority),
    readProtectedSecret(layout.serviceAuthority),
    readProtectedSecret(layout.openRouterSecret),
  ]);
}

async function initializeInstallation(layout, controllerAuthority) {
  await ctl(layout, ["init"], controllerAuthority);
  const imports = await writeInitializationImports(layout);
  const selected = [
    "init",
    "--runner-template", imports.runner,
    "--provider-declaration", imports.provider,
    "--agent-profile", imports.cooperative,
    "--agent-profile", imports.skeptical,
    "--client-declaration", imports.client,
  ];
  const preview = await ctl(layout, [...selected, "--preview"], controllerAuthority);
  const digest = preview?.import_review?.digest;
  if (typeof digest !== "string" || !/^blake3:[0-9a-f]{64}$/u.test(digest)) {
    throw new Error("initialization_review_invalid");
  }
  await ctl(layout, [...selected, "--approve-imports", digest], controllerAuthority);
}

async function writeInitializationImports(layout) {
  const assetDigest = (await readFile(layout.managedAgentHostDigest, "utf8")).trim();
  const observedDigest = (await run(layout.artifactDigest, [layout.managedAgentHost], process.env)).stdout.trim();
  const runnerDigest = verifyManagedAgentHostDigest(assetDigest, observedDigest);
  const runnerBytes = await readFile(layout.managedAgentHost);
  const retainedRunner = await retainRunnerExecutable({
    source: layout.managedAgentHost,
    retainedRoot: layout.retainedRunnerRoot,
    digest: runnerDigest,
    sourceBytes: runnerBytes,
  });
  if ((await run(layout.artifactDigest, [retainedRunner], process.env)).stdout.trim() !== runnerDigest) {
    throw new Error("retained_managed_agent_host_digest_mismatch");
  }
  const runner = await writeJson(
    join(layout.generatedRoot, "openrouter-house-runner.json"),
    renderHouseRunnerTemplate(retainedRunner, runnerDigest),
  );
  const provider = await writeJson(join(layout.generatedRoot, "openrouter-provider.json"), {
    schema: "worldstream/model-provider-credential-import/v1",
    credential_id: "hosted-openrouter",
    display_name: "Hosted OpenRouter",
    provider: "openrouter",
    secret_file: layout.openRouterSecret,
  });
  const profiles = renderHouseAgentProfiles();
  const cooperative = await writeJson(join(layout.generatedRoot, "cooperative-planner.json"), profiles.cooperative);
  const skeptical = await writeJson(join(layout.generatedRoot, "skeptical-auditor.json"), profiles.skeptical);
  const client = await writeClientImport(layout);
  return { runner, provider, cooperative, skeptical, client };
}

// Shared by the local and Fly entrypoints so current House revisions cannot
// silently select different Host profile identities.
export function renderHouseAgentProfiles() {
  const hostContract = {
    kind: "managed_house_openrouter",
    host_contract_revision: "1",
    runner_template: { template_id: "openrouter-house", revision: "16" },
  };
  const cooperative = {
    schema: "worldstream/studio-agent-profile-publish/v2",
    profile_id: "house-cooperative-planner",
    revision: "17",
    display_name: "Cooperative Planner",
    non_secret_configuration: {},
    host_contract: hostContract,
    managed_provider_credential_id: "hosted-openrouter",
  };
  const skeptical = {
    schema: "worldstream/studio-agent-profile-publish/v2",
    profile_id: "house-skeptical-auditor",
    revision: "16",
    display_name: "Skeptical Auditor",
    non_secret_configuration: {},
    host_contract: hostContract,
    managed_provider_credential_id: "hosted-openrouter",
  };
  return { cooperative, skeptical };
}

async function writeClientImport(layout) {
  const fallbackDirectory = join(layout.controllerState, "client-bindings", "inspector-fallback");
  const bindings = await readJson(layout.clientBindings);
  const clientOrigin = process.env.WORLDSTREAM_HOSTED_CLIENT_ORIGIN;
  let fallback = bindings.inspector_fallback;
  let inspectorDeployment = bindings.deployments?.find(
    (candidate) => candidate.client_id === "worldstream.inspector.web",
  );
  let inspectorRelease = await readJson(layout.inspectorRelease);
  if (await exists(fallbackDirectory)) {
    const fallbackFiles = (await readdir(fallbackDirectory)).filter((name) => name.endsWith(".json"));
    if (fallbackFiles.length !== 1) throw new Error("inspector_fallback_inventory_invalid");
    fallback = await readJson(join(fallbackDirectory, fallbackFiles[0]));
    const deploymentId = jsonString(fallback.deployment_id, "inspector_deployment_invalid");
    inspectorDeployment = await readJson(
      join(layout.controllerState, "client-bindings", "deployments", `${deploymentId}.json`),
    );
    const releaseDigest = jsonString(
      inspectorDeployment.release_digest,
      "inspector_release_invalid",
    );
    const clientId = jsonString(inspectorDeployment.client_id, "inspector_client_invalid");
    const releasesDirectory = join(layout.controllerState, "client-bindings", "releases");
    const matches = [];
    for (const name of (await readdir(releasesDirectory)).filter((item) => item.endsWith(".json"))) {
      const candidate = await readJson(join(releasesDirectory, name));
      if (candidate.client_id === clientId && candidate.release_digest === releaseDigest) {
        matches.push(candidate);
      }
    }
    if (matches.length !== 1) throw new Error("inspector_release_inventory_invalid");
    inspectorRelease = matches[0];
  }
  const heistDeployments = bindings.deployments?.filter(
    (candidate) => candidate.client_id === "worldstream.agent-heist.web",
  );
  if (
    fallback === undefined ||
    inspectorDeployment === undefined ||
    !Array.isArray(inspectorDeployment.surfaces) ||
    !Array.isArray(heistDeployments) ||
    heistDeployments.length !== 2 ||
    heistDeployments.some((deployment) => !Array.isArray(deployment.surfaces))
  ) {
    throw new Error("hosted_client_deployment_invalid");
  }
  inspectorDeployment.surfaces = inspectorDeployment.surfaces.map((surface) => ({
    ...surface,
    launch_url: new URL(
      inspectorRelease.surfaces.find((candidate) => candidate.surface_id === surface.surface_id).entrypoint,
      clientOrigin,
    ).toString(),
  }));
  const heistReleases = [
    await readJson(layout.clientRelease),
    await readJson(layout.retainedClientRelease),
  ];
  for (const deployment of heistDeployments) {
    const release = heistReleases.find((candidate) => candidate.release_digest === deployment.release_digest);
    if (release === undefined) throw new Error("hosted_client_release_invalid");
    deployment.surfaces = deployment.surfaces.map((surface) => ({
      ...surface,
      launch_url: new URL(
        release.surfaces.find((candidate) => candidate.surface_id === surface.surface_id).entrypoint,
        clientOrigin,
      ).toString(),
    }));
  }
  bindings.deployments = [inspectorDeployment, ...heistDeployments];
  bindings.inspector_fallback = fallback;
  const retainedInspector = await writeJson(
    join(layout.generatedRoot, "retained-inspector-web.json"),
    inspectorRelease,
  );
  const heistRelease = await writeJson(
    join(layout.generatedRoot, "agent-heist-web.json"),
    heistReleases[0],
  );
  const retainedHeistRelease = await writeJson(
    join(layout.generatedRoot, "agent-heist-web-v7.json"),
    heistReleases[1],
  );
  const bindingFile = await writeJson(
    join(layout.generatedRoot, "hosted-client-bindings.json"),
    bindings,
  );
  return writeJson(join(layout.generatedRoot, "hosted-client-import.json"), {
    schema: "worldstream/client-declaration-import/v1",
    release_files: [heistRelease, retainedHeistRelease, retainedInspector],
    bindings_file: bindingFile,
  });
}

async function startManaged(layout, controllerAuthority) {
  // Hosted Browser Sessions read the public HTTPS client origin directly.
  // The legacy local Participant Console handoff retains its loopback default
  // and is not routed by the public Gateway.
  const result = await ctl(layout, ["server", "start"], controllerAuthority);
  if (!hostedStatusReady(result)) throw new Error("managed_services_not_ready");
}

async function stopManaged(layout, controllerAuthority) {
  await ctl(layout, ["server", "stop"], controllerAuthority, true);
  await ctl(layout, ["server", "controller-stop"], controllerAuthority, true);
}

function startGateway(layout, configuration) {
  const environment = { ...process.env };
  delete environment.WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY;
  delete environment.WORLDSTREAM_VERCEL_SERVICE_AUTHORITY;
  Object.assign(environment, {
    HOSTED_GATEWAY_BIND: configuration.bind,
    WORLDSTREAM_HOST_ADAPTER_UPSTREAM: "127.0.0.1:9420",
    WORLDSTREAM_RUNTIME_UPSTREAM: "127.0.0.1:9410",
    WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY_FILE: layout.controllerAuthority,
    WORLDSTREAM_VERCEL_SERVICE_AUTHORITY_FILE: layout.serviceAuthority,
  });
  const child = spawn(layout.gateway, [], { env: environment, stdio: "inherit" });
  const exited = new Promise((resolvePromise) => {
    child.once("error", resolvePromise);
    child.once("exit", resolvePromise);
  });
  return { child, exited, configuration };
}

async function waitForGatewayLiveness(gateway) {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    if (gateway.child.exitCode !== null || gateway.child.signalCode !== null) {
      throw new Error("required_gateway_exited");
    }
    try {
      const response = await fetch(`${gateway.configuration.loopbackOrigin}/healthz`, {
        signal: AbortSignal.timeout(2_000),
      });
      if (response.status === 200) return;
    } catch {
      // This bounded retry loop owns the generic failure.
    }
    await delay(250);
  }
  throw new Error("gateway_liveness_timeout");
}

async function requiredServicesReady(layout, controllerAuthority, gatewayConfiguration) {
  try {
    const [status, ready] = await Promise.all([
      ctl(layout, ["server", "status"], controllerAuthority),
      fetch(`${gatewayConfiguration.loopbackOrigin}/readyz`, { signal: AbortSignal.timeout(2_000) }),
    ]);
    return hostedStatusReady(status) && ready.status === 200;
  } catch {
    return false;
  }
}

async function ctl(layout, command, controllerAuthority, allowFailure = false) {
  const environment = {
    ...process.env,
    WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY: controllerAuthority,
    WORLDSTREAM_HOSTED_INSTALLATION_ID: process.env.WORLDSTREAM_HOSTED_INSTALLATION_ID,
    WORLDSTREAM_HOSTED_CLIENT_ORIGIN: process.env.WORLDSTREAM_HOSTED_CLIENT_ORIGIN,
  };
  const args = [
    "--config", layout.runtimeConfig,
    ...command,
    "--state-dir", layout.controllerState,
    "--controller", "127.0.0.1:9420",
    "--json",
  ];
  try {
    const result = await run(layout.ctl, args, environment);
    return JSON.parse(result.stdout);
  } catch (error) {
    if (allowFailure) return null;
    const operation = command.slice(0, 2).join("_").replace(/[^a-z-]/gu, "_");
    throw new Error(`hosted_ctl_${operation}_failed`, { cause: error });
  }
}

async function run(command, args, environment) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, { env: environment, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    const append = (current, value) => (current + value).slice(-MAX_CAPTURE_BYTES);
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (value) => { stdout = append(stdout, value); });
    child.stderr.on("data", (value) => { stderr = append(stderr, value); });
    child.once("error", () => rejectPromise(new Error("required_command_unavailable")));
    child.once("exit", (code, signal) => {
      if (code === 0) resolvePromise({ stdout, stderr });
      else rejectPromise(new Error(`required_command_failed:${signal ?? code ?? "unknown"}`));
    });
  });
}

async function stopGateway(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  await new Promise((resolvePromise) => {
    const timer = setTimeout(() => child.kill("SIGKILL"), 120_000);
    child.once("exit", () => {
      clearTimeout(timer);
      resolvePromise();
    });
  });
}

async function maintenanceActive(layout) {
  return (await exists(layout.maintenanceMarker)) || (await exists(layout.recoveryFence));
}

async function exists(path) {
  try {
    await access(path, constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

async function readProtectedSecret(path) {
  const stat = await lstat(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size < 32 || stat.size > 512) {
    throw new Error("protected_secret_invalid");
  }
  if ((stat.mode & 0o077) !== 0) throw new Error("protected_secret_permissions_invalid");
  const value = await readFile(path, "utf8");
  if (value.length < 32 || value.length > 512 || !/^[!-~]+$/u.test(value)) {
    throw new Error("protected_secret_invalid");
  }
  return value;
}

async function readJson(path) {
  const stat = await lstat(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size < 2 || stat.size > 1_048_576) {
    throw new Error("hosted_artifact_invalid");
  }
  const value = JSON.parse(await readFile(path, "utf8"));
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("hosted_artifact_invalid");
  }
  return value;
}

async function writeJson(path, value) {
  await writeFile(path, `${JSON.stringify(value)}\n`, { mode: 0o600 });
  await chmod(path, 0o600);
  return path;
}

function jsonString(value, code) {
  if (typeof value !== "string" || value.length < 1 || value.length > 256) throw new Error(code);
  return value;
}

function requiredPath(environment, name) {
  const value = required(environment, name);
  if (!value.startsWith("/")) throw new Error(`${name.toLowerCase()}_must_be_absolute`);
  return resolve(value);
}

function required(environment, name, pattern, maximumLength = 512) {
  const value = environment[name];
  if (typeof value !== "string" || value.length < 1 || value.length > maximumLength) {
    throw new Error(`${name.toLowerCase()}_required`);
  }
  if (pattern !== undefined && !pattern.test(value)) throw new Error(`${name.toLowerCase()}_invalid`);
  return value;
}

function fatalReadinessMs(environment) {
  const value = Number(environment.WORLDSTREAM_HOSTED_FATAL_READINESS_MS ?? DEFAULT_FATAL_READINESS_MS);
  if (!Number.isSafeInteger(value) || value < 30_000 || value > 600_000) {
    throw new Error("invalid_fatal_readiness_window");
  }
  return value;
}

function shutdownSignal() {
  return new Promise((resolvePromise) => {
    process.once("SIGINT", () => resolvePromise("SIGINT"));
    process.once("SIGTERM", () => resolvePromise("SIGTERM"));
  });
}

function delay(milliseconds) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    const code = error instanceof Error ? error.message : "hosted_runtime_failed";
    process.stderr.write(`WorldStream hosted runtime stopped: ${code}\n`);
    process.exitCode = 1;
  });
}
