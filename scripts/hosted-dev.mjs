import { spawn, spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { once } from "node:events";
import { constants } from "node:fs";
import { lstat, mkdir, open, readFile, readdir, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { renderHouseAgentProfiles, renderHouseRunnerTemplate } from "./hosted-runtime.mjs";
import { runHostedAcceptancePrerequisites } from "./hosted-acceptance-prerequisites.mjs";
import {
  HOSTED_ACCEPTANCE_SCHEMA,
  LOCAL_ACCEPTANCE_CHECKS,
  writeHostedAcceptanceEvidence,
} from "./hosted-acceptance-evidence.mjs";

const REPOSITORY_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DEVELOPMENT_MODE = "visible-local-only";
const LISTING_DIGEST = "blake3:10135b2b12664dec3fc51a23c917d8468474af93b9c20ee5ed068b61e2dee61c";
const RETAINED_LISTING_DIGESTS = [
  "blake3:e202f7b24dbd99caeef6d8a1c904ae8131143d38af17a9512e562fe523654ed0",
  "blake3:8252e311f9ebbe20d1041877511932c9fac51155e0b26f6a247046fb383ddd99",
  "blake3:8106c3f34f52c8a2a216f2a88c842cea7b44db0241db4b0430e2c02a38f34f13",
  "blake3:f7beef31cc1160418a103963b7d3884e5f3a71ca1e4fc5c1c0a09d7cb98c909a",
  "blake3:48c397a32632896d66beb9ae7f8a6d090338800187c56eb0593997b80bd2b630",
  "blake3:48f76e8c1336e8f50fb6952cd0f2ff4c47cc8c372594db01422a67bca9363782",
  "blake3:9553f4fa320aa6901d0a03870f5c19ce4342d271efd2ef90d4fd287395f6cef1",
  "blake3:04edc964d5cbc1bc5efa422ac856305d55cec609a6ae5c5c1814c8389b776f80",
  "blake3:e3d401e783cec1ae4f911f682e8289054275dece60a0482b02f63e872f27dcc1",
  "blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956",
];

export function hostedDevelopmentListingAllowlist() {
  return [LISTING_DIGEST, ...RETAINED_LISTING_DIGESTS].join(",");
}

export function hostedDevelopmentLaunchHttpAccepted(status) {
  // The Gateway returns 200 after lobby launch, or 202 for retained setup work.
  // Neither status substitutes for the identity/evidence checks below.
  return status === 200 || status === 202;
}
const DEVELOPMENT_USER_ID = "00000000-0000-4000-8000-00000000d001";
const DEVELOPMENT_PROVIDER_SUBJECT = "worldstream-development";
const DEVELOPMENT_LOGIN = "worldstream-local-developer";
const DEVELOPMENT_AGENT_USER_ID = "00000000-0000-4000-8000-00000000d002";
const DEVELOPMENT_AGENT_PROVIDER_SUBJECT = "worldstream-development-agent";
const DEVELOPMENT_AGENT_LOGIN = "worldstream-local-browser-agent";
const SERVICE_AUTHORITY = "worldstream-local-service-authority-000000000000";
const CONTROLLER_AUTHORITY = "worldstream-local-controller-authority-000000000";
const HOST_INSTALLATION_ID = "hosted-dev";
const FAKE_OPENROUTER_KEY = "worldstream-development-key-000000000000";
const SUPABASE_EXCLUDES =
  "realtime,storage-api,imgproxy,mailpit,postgres-meta,studio,edge-runtime,logflare,vector,supavisor";
const READINESS_TIMEOUT_MS = 60_000;

export function hostedDevelopmentPorts(environment = process.env) {
  const ports = {
    runtime: port(environment.WORLDSTREAM_HOSTED_RUNTIME_PORT ?? "9410", "Runtime"),
    controller: port(environment.WORLDSTREAM_HOSTED_CONTROLLER_PORT ?? "9420", "Controller"),
    gateway: port(environment.WORLDSTREAM_HOSTED_GATEWAY_PORT ?? "8080", "Hosted Gateway"),
    product: port(environment.WORLDSTREAM_HOSTED_PRODUCT_PORT ?? "5180", "product"),
    heist: port(environment.WORLDSTREAM_HOSTED_HEIST_PORT ?? "5173", "Agent Heist"),
    platform: port(environment.WORLDSTREAM_HOSTED_PLATFORM_PORT ?? "3000", "Platform BFF"),
    fakeOpenRouter: port(
      environment.WORLDSTREAM_HOSTED_FAKE_OPENROUTER_PORT ?? "8787",
      "fake OpenRouter",
    ),
  };
  if (ports.controller !== 9420 || ports.product !== 5180 || ports.heist !== 5173) {
    throw new Error(
      "Controller 9420, product 5180, and Agent Heist 5173 are fixed by the reviewed local client declaration. " +
        "Regenerate and review that declaration before changing them.",
    );
  }
  if (new Set(Object.values(ports)).size !== Object.values(ports).length) {
    throw new Error("hosted development ports must be unique");
  }
  return ports;
}

export function assertHostedDevelopmentAllowed(environment = process.env) {
  if (environment.NODE_ENV === "production" || environment.VERCEL_ENV === "production") {
    throw new Error("hosted development substitutes are forbidden in production");
  }
}

export function hostedNativeBuildPlan(acceptance, profile = acceptance ? "release" : "debug") {
  if (profile !== "debug" && profile !== "release") {
    throw new Error("native profile must be debug or release");
  }
  const binaryRoot = join(REPOSITORY_ROOT, "target", profile);
  return {
    profile,
    cargoArgs: [
      "build", "--locked", ...(profile === "release" ? ["--release"] : []),
      "-p", "worldstream-server",
      "-p", "worldstream-studio-supervisor",
      "-p", "worldstream-hosted-gateway", "--bins",
    ],
    // worldstreamctl resolves Controller, Runtime, and assignment MCP beside
    // itself. The Gateway and approved House executable must use that build too.
    ctl: join(binaryRoot, "worldstreamctl"),
    gateway: join(binaryRoot, "worldstream-hosted-gateway"),
    managedAgentHost: join(binaryRoot, "worldstream-managed-agent-host"),
  };
}

export function renderHostedDevelopmentConfig({ dataDirectory, secretFile, runtimePort }) {
  return `# Generated by pnpm hosted:dev. Retained local data is not deleted on shutdown.
config_version = 1

[server]
bind = "127.0.0.1:${runtimePort}"

[storage]
profile = "sqlite-bundled"
data_dir = ${JSON.stringify(dataDirectory)}
deployment_lineage = "development/hosted-local"
storage_epoch = 1

[authority.bootstrap]
secret_file = ${JSON.stringify(secretFile)}
`;
}

async function main() {
  const options = hostedDevelopmentArguments(process.argv.slice(2));
  assertHostedDevelopmentAllowed();
  const ports = hostedDevelopmentPorts();
  const productOrigin = `http://127.0.0.1:${ports.product}`;
  const heistOrigin = `http://127.0.0.1:${ports.heist}`;
  const stateRoot = join(
    REPOSITORY_ROOT,
    ".worldstream",
    options.acceptance ? "hosted-acceptance" : "hosted-dev",
  );
  // The retained kernel contract still names this sibling directory `studio`;
  // it contains Controller authority/state and does not start a Studio UI.
  const stateDirectory = join(stateRoot, "studio");
  const dataDirectory = join(stateRoot, "data");
  const secretFile = join(stateRoot, "authority.secret");
  const configFile = join(stateRoot, "worldstream.toml");
  const providerSecretFile = join(stateRoot, "development-openrouter.secret");
  const native = hostedNativeBuildPlan(options.acceptance, options.nativeProfile);
  const worldstreamctl = native.ctl;
  const children = [];
  let ownsSupabase = false;
  let managedStarted = false;
  let worldstreamctlBuilt = false;
  let shuttingDown = false;
  let completed = false;

  const commonEnvironment = {
    ...process.env,
    NODE_ENV: "development",
    WORLDSTREAM_DEPLOYMENT_ENVIRONMENT: "development",
    WORLDSTREAM_HOSTED_INSTALLATION_ID: HOST_INSTALLATION_ID,
    WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY: CONTROLLER_AUTHORITY,
    WORLDSTREAM_HOSTED_CLIENT_ORIGIN: productOrigin,
    WORLDSTREAM_DEVELOPMENT_FAKE_OPENROUTER: DEVELOPMENT_MODE,
    WORLDSTREAM_DEVELOPMENT_HOUSE_OPENROUTER_ADDRESS:
      `127.0.0.1:${ports.fakeOpenRouter}`,
  };
  const ctl = (command, options = {}) =>
    run(
      worldstreamctl,
      [
        "--config",
        configFile,
        command[0],
        ...command.slice(1),
        "--state-dir",
        stateDirectory,
        "--controller",
        `127.0.0.1:${ports.controller}`,
        "--json",
      ],
      { ...options, environment: commonEnvironment },
    );

  try {
    preflight();
    const candidateBefore = options.acceptance
      ? await localCandidateIdentity(commonEnvironment)
      : null;
    await Promise.all([
      run("pnpm", ["install", "--frozen-lockfile"], { environment: commonEnvironment }),
      run(
        "cargo",
        native.cargoArgs,
        { environment: commonEnvironment },
      ),
    ]);
    worldstreamctlBuilt = true;
    if (candidateBefore !== null) {
      // Separate bounded build receipt; the existing acceptance evidence schema
      // is exact-shaped. This records source cleanliness and the selected local
      // Cargo profile, not binary digests, a running process tree, or a Fly image.
      process.stdout.write(`${JSON.stringify({
        schema: "worldstream/hosted-local-native-build/v1",
        source_revision: candidateBefore.commit,
        source_clean: candidateBefore.clean,
        profile: native.profile,
      })}\n`);
    }
    await Promise.all([
      run("pnpm", ["--filter", "@worldstream/platform", "build"], {
        environment: commonEnvironment,
      }),
      run("pnpm", ["--filter", "@worldstream/pack-sdk", "build"], {
        environment: commonEnvironment,
      }),
    ]);

    await mkdir(stateRoot, { recursive: true, mode: 0o700 });
    await ensureSecret(secretFile);
    await ensureDevelopmentProviderSecret(providerSecretFile);
    await writeFile(
      configFile,
      renderHostedDevelopmentConfig({ dataDirectory, secretFile, runtimePort: ports.runtime }),
      { mode: 0o600 },
    );

    ownsSupabase = !(await succeeds("supabase", ["status", "-o", "env"], commonEnvironment));
    if (ownsSupabase) {
      await run("supabase", ["start", "-x", SUPABASE_EXCLUDES], {
        environment: supabaseEnvironment(commonEnvironment),
      });
    }
    await run("supabase", ["migration", "up", "--local"], {
      environment: supabaseEnvironment(commonEnvironment),
    });
    const supabase = parseEnvironmentOutput(
      (
        await run("supabase", ["status", "-o", "env"], {
          capture: true,
          sensitive: true,
          environment: supabaseEnvironment(commonEnvironment),
        })
      ).stdout,
    );
    await seedSupabase(supabase, commonEnvironment);

    await ctl(["init"]);
    await ctl(["server", "stop"], { allowFailure: true, quiet: true });
    await ctl(["server", "controller-stop"], { allowFailure: true, quiet: true });
    const reuseHeist = await compatibleAgentHeistAlreadyRunning(ports.heist);
    if (reuseHeist) {
      process.stdout.write(
        `[Agent Heist] Reusing the compatible approved client already on port ${ports.heist}.\n`,
      );
    }
    await assertPortsAvailable(ports, reuseHeist ? new Set(["heist"]) : new Set());
    if (options.acceptance) {
      process.stdout.write("[Acceptance] Running negative/security prerequisites before pinning the Runner.\n");
      await runHostedAcceptancePrerequisites((command, args) => run(command, args, {
        // The production-appliance gate must not inherit local substitute flags.
        environment: {
          ...supabaseEnvironment(process.env),
          // Fixture traffic must not consume the actual story's trace budget.
          WORLDSTREAM_REENTRY_NATIVE_TRACE_FILE: "",
        },
      }));
      const paths = [];
      for (const [kind, key] of [["anonymous", requiredSupabase(supabase, "ANON_KEY")],
        ["service", requiredSupabase(supabase, "SERVICE_ROLE_KEY")]]) {
        const response = await fetch(`${requiredSupabase(supabase, "REST_URL")}/`, {
          headers: { apikey: key, ...(kind === "service" ? { authorization: `Bearer ${key}` } : {}) },
          redirect: "error", signal: AbortSignal.timeout(10_000),
        });
        if (!response.ok) throw new Error("Supabase exposure probe failed");
        const path = join(stateRoot, `acceptance-${kind}-openapi.json`);
        await writeFile(path, await response.text(), { mode: 0o600 });
        paths.push(path);
      }
      await run("node", ["scripts/verify-supabase-schema-exposure.mjs", ...paths], {
        environment: commonEnvironment,
      });
    }
    await importHostedDeclarations(
      ctl,
      stateDirectory,
      stateRoot,
      providerSecretFile,
      native.managedAgentHost,
    );

    await ctl(["server", "start", "--participant-console-origin", heistOrigin]);
    managedStarted = true;

    const platformOrigin = `http://127.0.0.1:${ports.platform}`;
    const childEnvironment = {
      ...commonEnvironment,
      CANONICAL_ORIGIN: productOrigin,
      SUPABASE_URL: requiredSupabase(supabase, "API_URL"),
      SUPABASE_PUBLISHABLE_KEY:
        supabase.PUBLISHABLE_KEY ?? requiredSupabase(supabase, "ANON_KEY"),
      SUPABASE_DATA_SECRET_KEY:
        supabase.SECRET_KEY ?? requiredSupabase(supabase, "SERVICE_ROLE_KEY"),
      WORLDSTREAM_SESSION_KEY_BASE64: Buffer.alloc(32, 7).toString("base64"),
      WORLDSTREAM_OAUTH_KEY_BASE64: Buffer.alloc(32, 9).toString("base64"),
      WORLDSTREAM_PLATFORM_BIND: "127.0.0.1",
      WORLDSTREAM_PLATFORM_PORT: String(ports.platform),
      WORLDSTREAM_DEVELOPMENT_IDENTITY_BYPASS: DEVELOPMENT_MODE,
      WORLDSTREAM_DEVELOPMENT_AUTH_USER_ID: DEVELOPMENT_USER_ID,
      WORLDSTREAM_DEVELOPMENT_GITHUB_SUBJECT: DEVELOPMENT_PROVIDER_SUBJECT,
      WORLDSTREAM_DEVELOPMENT_GITHUB_LOGIN: DEVELOPMENT_LOGIN,
      WORLDSTREAM_DEVELOPMENT_FAKE_OPENROUTER: DEVELOPMENT_MODE,
      WORLDSTREAM_FAKE_OPENROUTER_BIND: "127.0.0.1",
      WORLDSTREAM_FAKE_OPENROUTER_PORT: String(ports.fakeOpenRouter),
      WORLDSTREAM_DEVELOPMENT_OPENROUTER_KEY: FAKE_OPENROUTER_KEY,
      WORLDSTREAM_LOCAL_PLATFORM_BFF_TARGET: platformOrigin,
      WORLDSTREAM_LOCAL_ACTIVITY_CLIENT_TARGET: heistOrigin,
      WORLDSTREAM_HOSTED_GATEWAY_URL: `http://127.0.0.1:${ports.gateway}`,
      WORLDSTREAM_VERCEL_SERVICE_AUTHORITY: SERVICE_AUTHORITY,
      VITE_WORLDSTREAM_SUPERVISOR_URL: `http://127.0.0.1:${ports.controller}`,
    };

    children.push(
      startChild(
        "Platform BFF",
        "node",
        [join(REPOSITORY_ROOT, "web", "platform", "dist", "dev-server.js")],
        childEnvironment,
      ),
      startChild(
        "fake OpenRouter",
        "node",
        [join(REPOSITORY_ROOT, "scripts", "hosted-fake-openrouter.mjs")],
        childEnvironment,
      ),
      startChild(
        "product",
        "pnpm",
        [
          "--dir",
          "web/demos",
          "exec",
          "vite",
          "--host",
          "127.0.0.1",
          "--port",
          String(ports.product),
          "--strictPort",
        ],
        childEnvironment,
      ),
      startChild(
        "Hosted Gateway",
        native.gateway,
        [],
        {
          ...childEnvironment,
          HOSTED_GATEWAY_BIND: `127.0.0.1:${ports.gateway}`,
          WORLDSTREAM_HOST_ADAPTER_UPSTREAM: `127.0.0.1:${ports.controller}`,
          WORLDSTREAM_RUNTIME_UPSTREAM: `127.0.0.1:${ports.runtime}`,
          WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY: CONTROLLER_AUTHORITY,
          WORLDSTREAM_HOSTED_CLIENT_ORIGIN: productOrigin,
          WORLDSTREAM_PUBLIC_AUTHORITY: `127.0.0.1:${ports.gateway}`,
          WORLDSTREAM_VERCEL_SERVICE_AUTHORITY: SERVICE_AUTHORITY,
          WORLDSTREAM_LISTING_ALLOWLIST: hostedDevelopmentListingAllowlist(),
          WORLDSTREAM_DEPLOYMENT_VERSION: "hosted-local-development",
          RUST_LOG: "worldstream_hosted_gateway=info",
        },
      ),
    );
    if (!reuseHeist) {
      children.push(
        startChild(
          "Agent Heist",
          "pnpm",
          [
            "--dir",
            "clients/agent-heist-web",
            "exec",
            "vite",
            "--host",
            "127.0.0.1",
            "--port",
            String(ports.heist),
            "--strictPort",
          ],
          childEnvironment,
        ),
      );
    }

    await readiness(ports, ctl, children);
    await verifyDevelopmentFlow(ports);
    if (options.acceptance) {
      const evidencePath = await verifyCanonicalLocalCandidate({
        candidateBefore,
        childEnvironment,
        configFile,
        ctl: worldstreamctl,
        databaseUrl: requiredSupabase(supabase, "DB_URL"),
        outputPath: options.outputPath,
        ports,
        stateDirectory,
        stateRoot,
      });
      process.stdout.write(`Hosted local acceptance evidence: ${evidencePath}\n`);
      completed = true;
      return;
    }
    printReady(ports, supabase);
    if (options.checkOnly) {
      process.stdout.write("Hosted local-development acceptance check passed.\n");
      completed = true;
      return;
    }

    await waitUntilStopped(children, () => {
      shuttingDown = true;
    });
  } finally {
    shuttingDown = true;
    if (options.acceptance && !completed && managedStarted && worldstreamctlBuilt) {
      await ctl(["server", "logs"], { allowFailure: true, quiet: false });
    }
    if (managedStarted && worldstreamctlBuilt) {
      await ctl(["server", "stop"], { allowFailure: true, quiet: true });
    }
    if (worldstreamctlBuilt) {
      await ctl(["server", "controller-stop"], { allowFailure: true, quiet: true });
    }
    await stopChildren(children);
    if (ownsSupabase) {
      await run("supabase", ["stop"], {
        allowFailure: true,
        quiet: true,
        environment: supabaseEnvironment(commonEnvironment),
      });
    }
  }

  function startChild(name, command, args, environment) {
    return ownedChild(name, command, args, environment, () => shuttingDown);
  }
}

export function hostedDevelopmentArguments(args) {
  const usage = () => {
    throw new Error("usage: pnpm hosted:dev [--check | --acceptance [EVIDENCE_PATH]] [--native-profile=debug|release]");
  };
  const selectors = args.filter((arg) => arg.startsWith("--native-profile="));
  if (selectors.length > 1) usage();
  const selected = selectors[0]?.slice("--native-profile=".length);
  const remaining = args.filter((arg) => !arg.startsWith("--native-profile="));
  let options;
  if (remaining.length === 0) {
    options = { acceptance: false, checkOnly: false, outputPath: null };
  } else if (remaining.length === 1 && remaining[0] === "--check") {
    options = { acceptance: false, checkOnly: true, outputPath: null };
  } else if (remaining[0] === "--acceptance" && remaining.length <= 2 &&
    !remaining[1]?.startsWith("--")) {
    options = {
      acceptance: true,
      checkOnly: false,
      outputPath: remaining[1] === undefined ? null : resolve(remaining[1]),
    };
  } else {
    usage();
  }
  return { ...options, nativeProfile: hostedNativeBuildPlan(options.acceptance, selected).profile };
}

function preflight() {
  for (const command of ["cargo", "pnpm", "psql", "supabase"]) {
    const result = spawnSync(command, ["--version"], { cwd: REPOSITORY_ROOT, stdio: "ignore" });
    if (result.status !== 0) {
      throw new Error(`${command} is required; install it and retry pnpm hosted:dev`);
    }
  }
}

async function ensureSecret(path) {
  try {
    const stats = await lstat(path);
    if (!stats.isFile() || stats.isSymbolicLink() || stats.size !== 32 || (stats.mode & 0o077) !== 0) {
      throw new Error("hosted development authority secret must be a regular 32-byte mode-0600 file");
    }
  } catch (error) {
    if (!(error instanceof Error) || !("code" in error) || error.code !== "ENOENT") throw error;
    const handle = await open(path, constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY, 0o600);
    try {
      await handle.writeFile(randomBytes(32));
      await handle.sync();
    } finally {
      await handle.close();
    }
  }
}

async function ensureDevelopmentProviderSecret(path) {
  try {
    const stats = await lstat(path);
    if (!stats.isFile() || stats.isSymbolicLink() || (stats.mode & 0o077) !== 0) {
      throw new Error("hosted development provider secret must be a mode-0600 regular file");
    }
    if ((await readFile(path, "utf8")) !== FAKE_OPENROUTER_KEY) {
      throw new Error("retained hosted development provider secret has changed");
    }
  } catch (error) {
    if (!(error instanceof Error) || !("code" in error) || error.code !== "ENOENT") throw error;
    const handle = await open(path, constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY, 0o600);
    try {
      await handle.writeFile(FAKE_OPENROUTER_KEY);
      await handle.sync();
    } finally {
      await handle.close();
    }
  }
}

async function seedSupabase(supabase, environment) {
  const databaseUrl = requiredSupabase(supabase, "DB_URL");
  await run("psql", [databaseUrl, "-q", "-v", "ON_ERROR_STOP=1", "-f", "supabase/seed.sql"], {
    capture: true,
    sensitive: true,
    environment,
  });
  await run(
    "psql",
    [databaseUrl, "-q", "-v", "ON_ERROR_STOP=1", "-f", "supabase/development-seed.sql"],
    {
      capture: true,
      sensitive: true,
      environment: {
        ...environment,
        PGOPTIONS: "-c worldstream.development_seed=visible-local-only",
      },
    },
  );
  const verified = await run(
    "psql",
    [
      databaseUrl,
      "-At",
      "-v",
      "ON_ERROR_STOP=1",
      "-c",
      `select (select count(*) from platform_store.activity_listing_revisions where listing_revision_digest = '${LISTING_DIGEST}'), (select count(*) from platform_store.github_identities where (auth_user_id = '${DEVELOPMENT_USER_ID}' and provider_subject = '${DEVELOPMENT_PROVIDER_SUBJECT}') or (auth_user_id = '${DEVELOPMENT_AGENT_USER_ID}' and provider_subject = '${DEVELOPMENT_AGENT_PROVIDER_SUBJECT}')), (select count(distinct approvals.house_agent_revision_digest) from platform_store.house_agent_host_approvals approvals join platform_store.activity_listing_revisions listings on listings.listing_revision_digest = '${LISTING_DIGEST}' cross join lateral jsonb_array_elements(listings.seat_templates) seats(value) where approvals.host_installation_id = '${HOST_INSTALLATION_ID}' and approvals.available_for_new_assignments and approvals.revoked_at is null and (seats.value -> 'allowed_house_agent_revisions') ? approvals.house_agent_revision_digest);`,
    ],
    { capture: true, sensitive: true, environment },
  );
  if (verified.stdout.trim() !== "1|2|2") throw new Error("local Supabase seed verification failed: current Listing, development identities and both House strategies are required");
}

async function importHostedDeclarations(
  ctl,
  stateDirectory,
  stateRoot,
  providerSecretFile,
  managedHost,
) {
  const declaration = await hostedClientDeclaration(stateDirectory, stateRoot);
  const { taggedBlake3 } = await import(
    pathToFileURL(
      join(REPOSITORY_ROOT, "sdk", "typescript-pack", "packages", "pack-sdk", "dist", "index.js"),
    ).href
  );
  const executableDigest = taggedBlake3(await readFile(managedHost)).slice("blake3:".length);
  const generated = join(stateRoot, "generated-hosted-import");
  await mkdir(generated, { recursive: true, mode: 0o700 });
  const runner = join(generated, "openrouter-house-runner.json");
  const provider = join(generated, "openrouter-provider.json");
  const cooperative = join(generated, "cooperative-planner.json");
  const skeptical = join(generated, "skeptical-auditor.json");
  const profiles = renderHouseAgentProfiles();
  await Promise.all([
    writeFile(
      runner,
      `${JSON.stringify(renderHouseRunnerTemplate(managedHost, executableDigest))}\n`,
      { mode: 0o600 },
    ),
    writeFile(
      provider,
      `${JSON.stringify({
        schema: "worldstream/model-provider-credential-import/v1",
        credential_id: "hosted-openrouter",
        display_name: "Hosted development OpenRouter substitute",
        provider: "openrouter",
        secret_file: providerSecretFile,
      })}\n`,
      { mode: 0o600 },
    ),
    ...[
      [profiles.cooperative, cooperative],
      [profiles.skeptical, skeptical],
    ].map(([profile, path]) =>
      writeFile(
        path,
        `${JSON.stringify(profile)}\n`,
        { mode: 0o600 },
      )
    ),
  ]);
  const selected = [
    "init",
    "--runner-template", runner,
    "--provider-declaration", provider,
    "--agent-profile", cooperative,
    "--agent-profile", skeptical,
    "--client-declaration", declaration,
  ];
  const preview = await ctl([...selected, "--preview"], { capture: true });
  let digest;
  try {
    digest = JSON.parse(preview.stdout).import_review.digest;
  } catch {
    throw new Error("worldstreamctl returned an invalid client import review");
  }
  if (typeof digest !== "string" || !/^blake3:[0-9a-f]{64}$/u.test(digest)) {
    throw new Error("worldstreamctl client import review omitted its exact digest");
  }
  await ctl([...selected, "--approve-imports", digest]);
}

async function hostedClientDeclaration(stateDirectory, stateRoot) {
  const configuration = join(REPOSITORY_ROOT, "config", "activity-clients");
  const checkedIn = join(configuration, "hosted-local-import.json");
  const fallbackDirectory = join(stateDirectory, "client-bindings", "inspector-fallback");
  let fallbackFiles;
  try {
    fallbackFiles = (await readdir(fallbackDirectory)).filter((name) => name.endsWith(".json"));
  } catch (error) {
    if (error instanceof Error && "code" in error && error.code === "ENOENT") return checkedIn;
    throw error;
  }
  if (fallbackFiles.length === 0) return checkedIn;
  if (fallbackFiles.length !== 1) throw new Error("retained Inspector fallback inventory is ambiguous");

  const fallback = await readRegularJson(join(fallbackDirectory, fallbackFiles[0]));
  const deploymentId = requiredJsonString(fallback.deployment_id, "retained Inspector deployment");
  const deployment = await readRegularJson(
    join(stateDirectory, "client-bindings", "deployments", `${deploymentId}.json`),
  );
  const releaseDigest = requiredJsonString(deployment.release_digest, "retained Inspector release");
  const clientId = requiredJsonString(deployment.client_id, "retained Inspector client");
  const releasesDirectory = join(stateDirectory, "client-bindings", "releases");
  const releaseFiles = (await readdir(releasesDirectory)).filter((name) => name.endsWith(".json"));
  let retainedRelease = null;
  for (const name of releaseFiles) {
    const candidate = await readRegularJson(join(releasesDirectory, name));
    if (candidate.client_id === clientId && candidate.release_digest === releaseDigest) {
      if (retainedRelease !== null) throw new Error("retained Inspector release is ambiguous");
      retainedRelease = candidate;
    }
  }
  if (retainedRelease === null) throw new Error("retained Inspector release is unavailable");

  const template = await readRegularJson(join(configuration, "hosted-local-bindings.json"));
  const currentHeist = await readRegularJson(
    join(configuration, "releases", "agent-heist-web-v5.json"),
  );
  template.deployments = [
    deployment,
    ...template.deployments.filter((candidate) => candidate.client_id !== "worldstream.inspector.web"),
  ];
  template.inspector_fallback = fallback;
  const generated = join(stateRoot, "generated-client-import");
  await mkdir(generated, { recursive: true, mode: 0o700 });
  await writeFile(
    join(generated, "agent-heist-web.json"),
    `${JSON.stringify(currentHeist)}\n`,
    { mode: 0o600 },
  );
  await writeFile(
    join(generated, "retained-inspector-web.json"),
    `${JSON.stringify(retainedRelease)}\n`,
    { mode: 0o600 },
  );
  await writeFile(
    join(generated, "hosted-local-bindings.json"),
    `${JSON.stringify(template)}\n`,
    { mode: 0o600 },
  );
  const generatedDeclaration = join(generated, "hosted-local-import.json");
  await writeFile(
    generatedDeclaration,
    `${JSON.stringify({
      schema: "worldstream/client-declaration-import/v1",
      release_files: ["./agent-heist-web.json", "./retained-inspector-web.json"],
      bindings_file: "./hosted-local-bindings.json",
    })}\n`,
    { mode: 0o600 },
  );
  return generatedDeclaration;
}

async function readRegularJson(path) {
  const metadata = await lstat(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size > 1_048_576) {
    throw new Error("retained client record is not a bounded regular file");
  }
  const value = JSON.parse(await readFile(path, "utf8"));
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("retained client record is invalid");
  }
  return value;
}

function requiredJsonString(value, label) {
  if (typeof value !== "string" || value.length === 0 || value.length > 256) {
    throw new Error(`${label} is invalid`);
  }
  return value;
}

async function assertPortsAvailable(ports, reusableNames) {
  for (const [name, selected] of Object.entries(ports)) {
    if (reusableNames.has(name)) continue;
    await new Promise((resolvePromise, rejectPromise) => {
      const probe = createServer();
      probe.unref();
      probe.once("error", () => {
        rejectPromise(
          new Error(
            `${name} port ${selected} is already in use. Stop that process or configure an identity-safe alternate port.`,
          ),
        );
      });
      probe.listen(selected, "127.0.0.1", () => probe.close(resolvePromise));
    });
  }
}

async function compatibleAgentHeistAlreadyRunning(portNumber) {
  try {
    const response = await fetch(`http://127.0.0.1:${portNumber}/agent-heist-v5/hosted/`, {
      signal: AbortSignal.timeout(1_000),
    });
    if (response.status !== 200) return false;
    const contentSecurityPolicy = response.headers.get("content-security-policy") ?? "";
    const body = await response.text();
    return (
      contentSecurityPolicy.includes("connect-src 'self' http://127.0.0.1:9420") &&
      body.includes("<title>Agent Heist · WorldStream Activity Client</title>")
    );
  } catch {
    return false;
  }
}

function ownedChild(name, command, args, environment, shuttingDown) {
  const child = spawn(command, args, {
    cwd: REPOSITORY_ROOT,
    env: environment,
    detached: process.platform !== "win32",
    stdio: ["ignore", "pipe", "pipe"],
  });
  const recent = [];
  for (const [stream, label] of [
    [child.stdout, "out"],
    [child.stderr, "err"],
  ]) {
    stream.setEncoding("utf8");
    stream.on("data", (value) => {
      for (const line of value.split(/\r?\n/u).filter(Boolean)) {
        recent.push(`${label}: ${line}`);
        if (recent.length > 20) recent.shift();
        process.stdout.write(`[${name}] ${line}\n`);
      }
    });
  }
  const failure = new Promise((_, rejectPromise) => {
    child.once("error", rejectPromise);
    child.once("exit", (code, signal) => {
      if (!shuttingDown()) {
        rejectPromise(
          new Error(
            `${name} exited before shutdown (${signal ?? code ?? "unknown"}).\n${recent.join("\n")}`,
          ),
        );
      }
    });
  });
  return { child, failure, name };
}

async function readiness(ports, ctl, children) {
  const checks = [
    waitForHttp("Platform BFF", `http://127.0.0.1:${ports.platform}/api/dev/status`, 200, children),
    waitForHttp("fake OpenRouter", `http://127.0.0.1:${ports.fakeOpenRouter}/healthz`, 200, children),
    waitForHttp("product", `http://127.0.0.1:${ports.product}/`, 200, children),
    waitForHttp(
      "same-origin Agent Heist",
      `http://127.0.0.1:${ports.product}/agent-heist-v5/hosted/`,
      200,
      children,
    ),
    waitForHttp("Hosted Gateway", `http://127.0.0.1:${ports.gateway}/readyz`, 200, children),
    waitForCommand(
      "Runtime and Controller",
      () => ctl(["server", "status"], { capture: true, quiet: true }),
      children,
    ),
  ];
  await Promise.race([Promise.all(checks), ...children.map(({ failure }) => failure)]);
}

async function waitForHttp(name, url, expectedStatus, children) {
  const deadline = Date.now() + READINESS_TIMEOUT_MS;
  while (Date.now() < deadline) {
    assertChildrenLive(children);
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(2_000) });
      if (response.status === expectedStatus) return;
    } catch {
      // The bounded retry loop owns the diagnostic.
    }
    await delay(250);
  }
  throw new Error(`${name} did not become ready within ${READINESS_TIMEOUT_MS / 1000} seconds`);
}

async function waitForCommand(name, check, children) {
  const deadline = Date.now() + READINESS_TIMEOUT_MS;
  while (Date.now() < deadline) {
    assertChildrenLive(children);
    try {
      await check();
      return;
    } catch {
      await delay(250);
    }
  }
  throw new Error(`${name} did not become ready within ${READINESS_TIMEOUT_MS / 1000} seconds`);
}

function assertChildrenLive(children) {
  for (const { child, name } of children) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(`${name} exited during readiness checks`);
    }
  }
}

async function verifyDevelopmentFlow(ports) {
  const productOrigin = `http://127.0.0.1:${ports.product}`;
  const signedIn = await fetch(`${productOrigin}/api/dev/sign-in`, {
    method: "POST",
    headers: { "content-type": "application/json", origin: productOrigin },
    body: JSON.stringify({ mode: DEVELOPMENT_MODE }),
  });
  if (signedIn.status !== 200) throw new Error("development identity sign-in check failed");
  const cookie = signedIn.headers.getSetCookie()[0]?.split(";", 1)[0];
  if (cookie === undefined) throw new Error("development identity cookie check failed");
  const session = await fetch(`${productOrigin}/api/auth/session`, { headers: { cookie } });
  if (session.status !== 200) throw new Error("development session round-trip check failed");
  const sessionBody = await session.json();
  if (typeof sessionBody.csrf !== "string") {
    throw new Error("development session omitted CSRF authority");
  }
  const catalogResponse = await fetch(`${productOrigin}/api/catalog`);
  const catalog = await catalogResponse.json();
  const catalogBytes = JSON.stringify(catalog);
  if (
    catalogResponse.status !== 200 ||
    catalog.activities?.[0]?.slug !== "agent-heist" ||
    catalog.activities?.[0]?.availability !== "available" ||
    catalogBytes.includes("worldstream.agent-heist") ||
    catalogBytes.includes(LISTING_DIGEST)
  ) {
    throw new Error("hosted catalog boundary check failed");
  }
  const launchMutationHeaders = {
    "content-type": "application/json",
    cookie,
    origin: productOrigin,
    "sec-fetch-site": "same-origin",
    "x-worldstream-csrf": sessionBody.csrf,
  };
  const launchBody = JSON.stringify({
    listing_slug: "agent-heist",
    creator_access: "seat",
    creator_seat: "seat-1",
    fill_mode: "house_agents",
    idempotency_key: "hosted_local_acceptance_idempotency_key_0001",
  });
  const create = () => fetch(`${productOrigin}/api/launches`, {
    method: "POST",
    headers: launchMutationHeaders,
    body: launchBody,
  });
  const firstLaunchResponse = await create();
  const firstLaunch = await firstLaunchResponse.json();
  const repeatedLaunchResponse = await create();
  const repeatedLaunch = await repeatedLaunchResponse.json();
  if (
    ![200, 201].includes(firstLaunchResponse.status) ||
    repeatedLaunchResponse.status !== 200 ||
    typeof firstLaunch.launch_id !== "string" ||
    firstLaunch.launch_id !== repeatedLaunch.launch_id
  ) {
    throw new Error("hosted launch idempotency check failed");
  }
  if (repeatedLaunch.state === "collecting") {
    const invitation = await fetch(
      `${productOrigin}/api/launches/${repeatedLaunch.launch_id}/seats/seat-2/invitation`,
      { method: "POST", headers: launchMutationHeaders, body: "{}" },
    );
    const invitationBody = await invitation.json();
    if (
      invitation.status !== 201 ||
      typeof invitationBody.invitation_token !== "string" ||
      !/^[0-9a-f]{64}$/u.test(invitationBody.invitation_token)
    ) {
      throw new Error("hosted seat invitation check failed");
    }
    const cancellation = await fetch(
      `${productOrigin}/api/launches/${repeatedLaunch.launch_id}/cancel`,
      { method: "POST", headers: launchMutationHeaders, body: "{}" },
    );
    if (cancellation.status !== 200) throw new Error("hosted launch cancellation check failed");
  }

  const provider = await fetch(
    `http://127.0.0.1:${ports.fakeOpenRouter}/api/v1/chat/completions`,
    {
      method: "POST",
      headers: {
        authorization: `Bearer ${FAKE_OPENROUTER_KEY}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({
        model: "worldstream/development-deterministic",
        messages: [{ role: "user", content: "readiness" }],
        max_tokens: 1,
      }),
    },
  );
  if (provider.status !== 200) throw new Error("fake OpenRouter contract check failed");

  const { encodeCanonical, taggedBlake3 } = await import(
    pathToFileURL(
      join(REPOSITORY_ROOT, "sdk", "typescript-pack", "packages", "pack-sdk", "dist", "index.js"),
    ).href
  );
  const [sourceLaunchRequest, sourceRoster, listingValue] = await Promise.all(
    [
      "fixtures/hosted-contract/valid/agent-heist-launch-request.json",
      "fixtures/hosted-contract/valid/agent-heist-frozen-roster.json",
      "config/hosted/listings/agent-heist-0.12.0.json",
    ].map(async (path) => JSON.parse(await readFile(join(REPOSITORY_ROOT, path), "utf8"))),
  );
  const frozenLaunchRequest = {
    ...sourceLaunchRequest,
    listing_revision_digest: LISTING_DIGEST,
  };
  const frozenRoster = {
    ...sourceRoster,
    listing_revision_digest: LISTING_DIGEST,
  };
  const launchBytes = encodeCanonical(frozenLaunchRequest);
  const rosterBytes = encodeCanonical(frozenRoster);
  const { deriveRoomSetup, readListingRevision } = await import(
    pathToFileURL(
      join(REPOSITORY_ROOT, "sdk", "typescript-hosted-contract", "dist", "index.js"),
    ).href
  );
  const setupBytes = deriveRoomSetup(
    readListingRevision(encodeCanonical(listingValue)),
    launchBytes,
    rosterBytes,
  );
  const frozenRoomSetupSpecification = JSON.parse(new TextDecoder().decode(setupBytes));
  const launchRequest = {
    schema: "worldstream/hosted-launch-request/v1",
    listing_revision_digest: LISTING_DIGEST,
    launch_request_digest: taggedBlake3(launchBytes),
    launch_input_digest: taggedSha256(encodeCanonical(frozenLaunchRequest.inputs)),
    frozen_roster_digest: taggedSha256(rosterBytes),
    room_setup_specification_digest: taggedBlake3(setupBytes),
    room_setup_operation_id: "hosted-local-readiness-04edc964",
    capacity_authorization: {
      schema: "worldstream/platform-capacity-authorization/v1",
      host_installation_id: HOST_INSTALLATION_ID,
      reservation_reference: "04edc964-d5cb-41bc-aefa-422ac856305d",
    },
    frozen_launch_request: frozenLaunchRequest,
    frozen_roster: frozenRoster,
    frozen_room_setup_specification: frozenRoomSetupSpecification,
  };
  const gateway = await fetch(`http://127.0.0.1:${ports.gateway}/v1/hosted/launch`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${SERVICE_AUTHORITY}`,
      "content-type": "application/json",
    },
    body: encodeCanonical(launchRequest),
  });
  if (!hostedDevelopmentLaunchHttpAccepted(gateway.status)) {
    const diagnostic = (await gateway.text()).slice(0, 512).replaceAll(/[\r\n]/gu, " ");
    throw new Error(
      `development Hosted Gateway contract check failed (${gateway.status}: ${diagnostic})`,
    );
  }
  const launchStatus = await gateway.json();
  if (
    launchStatus.schema !== "worldstream/hosted-launch-status/v1" ||
    launchStatus.room_setup_operation_id !== launchRequest.room_setup_operation_id
  ) {
    throw new Error("development Hosted Gateway returned invalid launch status");
  }
  const evidence = await fetch(`http://127.0.0.1:${ports.gateway}/v1/hosted/evidence`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${SERVICE_AUTHORITY}`,
      "content-type": "application/json",
    },
    body: encodeCanonical({
      schema: "worldstream/hosted-launch-evidence-request/v1",
      listing_revision_digest: launchRequest.listing_revision_digest,
      launch_request_digest: launchRequest.launch_request_digest,
      room_setup_operation_id: launchRequest.room_setup_operation_id,
    }),
  });
  if (evidence.status !== 200) throw new Error("development Hosted Gateway evidence check failed");
  const evidenceStatus = await evidence.json();
  if (
    evidenceStatus.launch_request_digest !== launchStatus.launch_request_digest ||
    evidenceStatus.room_setup_operation_id !== launchStatus.room_setup_operation_id
  ) {
    throw new Error("development Hosted Gateway evidence identity changed");
  }
}

function taggedSha256(bytes) {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

async function verifyCanonicalLocalCandidate({
  candidateBefore,
  childEnvironment,
  configFile,
  ctl,
  databaseUrl,
  outputPath,
  ports,
  stateDirectory,
  stateRoot,
}) {
  if (candidateBefore === null) throw new Error("candidate source identity was not captured");
  const nonce = randomBytes(8).toString("hex");
  const resultPath = join(stateRoot, `local-acceptance-result-${nonce}.json`);
  await run(
    "psql",
    [
      databaseUrl,
      "-q",
      "-v",
      "ON_ERROR_STOP=1",
      "-c",
      `delete from platform_store.account_mutation_rate_limits where auth_user_id in ('${DEVELOPMENT_USER_ID}', '${DEVELOPMENT_AGENT_USER_ID}');`,
    ],
    { capture: true, sensitive: true, environment: childEnvironment },
  );
  await run(
    "pnpm",
    [
      "--filter",
      "@worldstream/platform",
      "exec",
      "vitest",
      "run",
      "src/hosted-local-acceptance.live.test.ts",
      "--environment",
      "node",
      "--testTimeout",
      "360000",
    ],
    {
      environment: {
        ...childEnvironment,
        WORLDSTREAM_LOCAL_ACCEPTANCE: DEVELOPMENT_MODE,
        WORLDSTREAM_ACCEPTANCE_NONCE: nonce,
        WORLDSTREAM_ACCEPTANCE_PRODUCT_ORIGIN: `http://127.0.0.1:${ports.product}`,
        WORLDSTREAM_ACCEPTANCE_FAKE_PROVIDER_ORIGIN:
          `http://127.0.0.1:${ports.fakeOpenRouter}`,
        WORLDSTREAM_ACCEPTANCE_AGENT_USER_ID: DEVELOPMENT_AGENT_USER_ID,
        WORLDSTREAM_ACCEPTANCE_AGENT_PROVIDER_SUBJECT:
          DEVELOPMENT_AGENT_PROVIDER_SUBJECT,
        WORLDSTREAM_ACCEPTANCE_AGENT_LOGIN: DEVELOPMENT_AGENT_LOGIN,
        WORLDSTREAM_ACCEPTANCE_CTL: ctl,
        WORLDSTREAM_ACCEPTANCE_CONFIG: configFile,
        WORLDSTREAM_ACCEPTANCE_STATE_DIR: stateDirectory,
        WORLDSTREAM_ACCEPTANCE_CONTROLLER: `127.0.0.1:${ports.controller}`,
        WORLDSTREAM_ACCEPTANCE_REPOSITORY_ROOT: REPOSITORY_ROOT,
        WORLDSTREAM_ACCEPTANCE_RESULT_PATH: resultPath,
      },
    },
  );
  const result = JSON.parse(await readFile(resultPath, "utf8"));
  if (
    result?.version !== "worldstream_hosted_local_acceptance_result.v1" ||
    !Number.isSafeInteger(result.provider_calls) ||
    result.provider_calls < 1 ||
    result.provider_calls > 10 ||
    !Number.isSafeInteger(result.maximum_direct_push_seconds) ||
    result.maximum_direct_push_seconds < 1 ||
    result.maximum_direct_push_seconds > 600
  ) {
    throw new Error("hosted local acceptance returned invalid evidence");
  }
  const candidateAfter = await localCandidateIdentity(childEnvironment);
  const commit = candidateBefore.commit;
  const cleanCandidate = candidateBefore.clean && candidateAfter.clean &&
    candidateBefore.commit === candidateAfter.commit;
  if (!/^[0-9a-f]{40}$/u.test(commit)) {
    throw new Error("hosted local acceptance commit identity is invalid");
  }
  const migrations = (await readdir(join(REPOSITORY_ROOT, "supabase", "migrations")))
    .map((name) => name.match(/^(\d{14})_/u)?.[1])
    .filter((value) => value !== undefined)
    .sort();
  const schemaHead = migrations.at(-1);
  if (schemaHead === undefined) throw new Error("Supabase schema head is unavailable");
  const recordedAt = new Date().toISOString();
  const selectedOutput = outputPath ?? join(
    REPOSITORY_ROOT,
    ".worldstream",
    "evidence",
    `hosted-local-${commit.slice(0, 12)}-${recordedAt.replaceAll(/[:.]/gu, "-")}.json`,
  );
  const evidencePath = await writeHostedAcceptanceEvidence(selectedOutput, {
    schema: HOSTED_ACCEPTANCE_SCHEMA,
    candidate_kind: "local",
    outcome: cleanCandidate ? "passed" : "blocked",
    commit,
    recorded_at: recordedAt,
    deployment: {
      platform_revision: commit,
      gateway_revision: commit,
      schema_head: schemaHead,
      listing_revision_digest: LISTING_DIGEST,
      pack_digest: "blake3:4455e4302bda695a5fc4aca150b5a8dac944775474930dafaef1539acb86c96e",
      client_release_digest:
        "sha256:12714052c8e1cac59a95b0439e8e86bf17766c8f689f5782d1dd5d4c0efd4cd6",
      projector_digest:
        "blake3:f676cab8007a374db5510d66f534697472c53ea4b2719900bfecce53786f7563",
    },
    checks: Object.fromEntries(
      LOCAL_ACCEPTANCE_CHECKS.map((name) => [name,
        name === "clean_candidate_revision" && !cleanCandidate
          ? { status: "blocked", note: "Working tree is dirty or the commit changed during verification. Repeat from a clean committed candidate." }
          : { status: "passed" },
      ]),
    ),
    metrics: {
      provider_calls: result.provider_calls,
      maximum_direct_push_seconds: result.maximum_direct_push_seconds,
    },
    redaction: {
      private_projections_retained: false,
      credentials_retained: false,
    },
  });
  if (!cleanCandidate) {
    throw new Error(`Gameplay checks passed, but release evidence is blocked by the candidate source state. Diagnostic evidence: ${evidencePath}`);
  }
  return evidencePath;
}

async function localCandidateIdentity(environment) {
  const commit = (await run("git", ["rev-parse", "HEAD"], {
    capture: true, environment,
  })).stdout.trim();
  const status = await run("git", ["status", "--porcelain", "--untracked-files=normal"], {
    capture: true, environment,
  });
  return { commit, clean: status.stdout.trim() === "" };
}

function printReady(ports, supabase) {
  process.stdout.write(
    [
      "",
      "WorldStream hosted development stack is ready.",
      `Product:        http://127.0.0.1:${ports.product}/`,
      `Agent Heist:    http://127.0.0.1:${ports.product}/agent-heist-v5/hosted/`,
      `Hosted Gateway: http://127.0.0.1:${ports.gateway}/`,
      `Supabase API:   ${requiredSupabase(supabase, "API_URL")}`,
      `Runtime:        127.0.0.1:${ports.runtime} (loopback only)`,
      `Controller:     127.0.0.1:${ports.controller} (loopback only)`,
      "",
      "Development substitutes are ACTIVE: identity bypass and fake OpenRouter.",
      "Press Ctrl-C to stop owned processes. Retained local data is preserved.",
      "",
    ].join("\n"),
  );
}

async function waitUntilStopped(children, beginShutdown) {
  const signal = new Promise((resolvePromise) => {
    const stop = (name) => {
      beginShutdown();
      resolvePromise(name);
    };
    process.once("SIGINT", () => stop("SIGINT"));
    process.once("SIGTERM", () => stop("SIGTERM"));
  });
  await Promise.race([signal, ...children.map(({ failure }) => failure)]);
}

async function stopChildren(children) {
  for (const { child } of [...children].reverse()) {
    if (child.exitCode !== null || child.signalCode !== null) continue;
    try {
      if (process.platform === "win32") child.kill("SIGTERM");
      else process.kill(-child.pid, "SIGTERM");
    } catch {
      // The child may have exited between observation and the signal.
    }
  }
  await Promise.all(
    children.map(async ({ child }) => {
      if (child.exitCode !== null || child.signalCode !== null) return;
      await Promise.race([once(child, "exit"), delay(5_000)]);
      if (child.exitCode === null && child.signalCode === null) {
        try {
          if (process.platform === "win32") child.kill("SIGKILL");
          else process.kill(-child.pid, "SIGKILL");
        } catch {
          // The bounded force-stop targets only a process group created above.
        }
      }
    }),
  );
}

async function succeeds(command, args, environment) {
  const result = await run(command, args, {
    allowFailure: true,
    capture: true,
    quiet: true,
    sensitive: true,
    environment: supabaseEnvironment(environment),
  });
  return result.code === 0;
}

async function run(command, args, options = {}) {
  const capture = options.capture === true || options.quiet === true;
  const child = spawn(command, args, {
    cwd: REPOSITORY_ROOT,
    env: options.environment ?? process.env,
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
  let stdout = "";
  let stderr = "";
  if (capture) {
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (value) => {
      stdout = boundedAppend(stdout, value);
    });
    child.stderr.on("data", (value) => {
      stderr = boundedAppend(stderr, value);
    });
  }
  const [code, signal] = await once(child, "exit");
  const result = { code: code ?? 1, signal, stdout, stderr };
  if (result.code !== 0 && options.allowFailure !== true) {
    const detail = options.sensitive === true ? "output redacted" : tail(stderr || stdout, 20);
    throw new Error(`${command} failed with status ${signal ?? result.code}: ${detail}`);
  }
  return result;
}

function boundedAppend(current, value) {
  const next = current + value;
  return next.length <= 1024 * 1024 ? next : next.slice(-1024 * 1024);
}

function tail(value, lines) {
  return value.trim().split(/\r?\n/u).slice(-lines).join("\n");
}

function parseEnvironmentOutput(value) {
  const result = {};
  for (const line of value.split(/\r?\n/u)) {
    const match = /^([A-Z][A-Z0-9_]*)=(.*)$/u.exec(line);
    if (match === null) continue;
    const [, name, raw] = match;
    if (name === undefined || raw === undefined) continue;
    result[name] = raw.startsWith('"') ? JSON.parse(raw) : raw;
  }
  return result;
}

function requiredSupabase(values, name) {
  const value = values[name];
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`Supabase status omitted ${name}`);
  }
  return value;
}

function supabaseEnvironment(environment) {
  return {
    ...environment,
    SUPABASE_AUTH_EXTERNAL_GITHUB_CLIENT_ID: "worldstream-hosted-local",
    SUPABASE_AUTH_EXTERNAL_GITHUB_SECRET: "worldstream-hosted-local-development-secret",
  };
}

function port(value, name) {
  if (!/^\d{1,5}$/u.test(value)) throw new Error(`${name} port is invalid`);
  const result = Number(value);
  if (!Number.isSafeInteger(result) || result < 1 || result > 65_535) {
    throw new Error(`${name} port is invalid`);
  }
  return result;
}

function delay(milliseconds) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));
}

if (process.argv[1] !== undefined && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  void main().catch((caught) => {
    const message = caught instanceof Error ? caught.message : "hosted development startup failed";
    process.stderr.write(`hosted:dev failed: ${message}\n`);
    process.exitCode = 1;
  });
}
