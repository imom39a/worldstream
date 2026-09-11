import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { test } from "node:test";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer } from "node:net";

import {
  assertHostedDevelopmentAllowed,
  assertHostedDevelopmentBrowserAddressUnused,
  assertHostedDevelopmentPortsAvailable,
  hostedDevelopmentBuildEnvironment,
  HOSTED_LOCAL_ACTIVITY_CLIENT_BUILDS,
  hostedDevelopmentGatewayConfiguration,
  hostedDevelopmentPorts,
  hostedDevelopmentListingAllowlist,
  hostedDevelopmentClientReleaseDigest,
  hostedDevelopmentLaunchHttpAccepted,
  hostedDevelopmentReadinessProbeIdentity,
  hasRetainedHostedDevelopmentSetup,
  reconcileHostedDevelopment,
  retryHostedServerStart,
  HOSTED_LOCAL_RECONCILIATION_SECRET,
  HOSTED_LOCAL_SMOKE_IDEMPOTENCY_KEY,
  hostedLocalSmokeIdempotencyKey,
  hostedDevelopmentArguments,
  hostedNativeBuildPlan,
  hostedNativeShutdownPlan,
  installHostedNativeBinaries,
  renderHostedDevelopmentConfig,
} from "./hosted-dev.mjs";
import { activityClientMounts } from "./serve-activity-clients.mjs";

const repository = new URL("../", import.meta.url);

test("hosted development builds immutable browser artifacts in production mode", () => {
  assert.deepEqual(hostedDevelopmentBuildEnvironment({
    NODE_ENV: "development",
    WORLDSTREAM_DEPLOYMENT_ENVIRONMENT: "development",
  }), {
    NODE_ENV: "production",
    WORLDSTREAM_DEPLOYMENT_ENVIRONMENT: "development",
  });
});

test("hosted acceptance derives its client identity from the current Listing", async () => {
  const listing = JSON.parse(await readFile(
    new URL("config/hosted/listings/agent-heist-0.29.0.json", repository),
    "utf8",
  ));
  const release = JSON.parse(await readFile(
    new URL("config/activity-clients/releases/agent-heist-web-v12.json", repository),
    "utf8",
  ));
  assert.equal(hostedDevelopmentClientReleaseDigest(listing), release.release_digest);
  assert.throws(
    () => hostedDevelopmentClientReleaseDigest({ client: { release_digest: "sha256:invalid" } }),
    /client release is invalid/u,
  );
});

test("hosted development builds every source Activity Client selected by its local bindings", async () => {
  const bindings = JSON.parse(await readFile(
    new URL("config/activity-clients/hosted-local-bindings.json", repository),
    "utf8",
  ));
  const selectedPrefixes = new Set(bindings.deployments.flatMap((deployment) =>
    deployment.surfaces.map((surface) => `/${new URL(surface.launch_url).pathname.split("/")[1]}/`)));
  const sourceRoots = activityClientMounts
    .filter(({ prefix, root }) => selectedPrefixes.has(prefix) && !root.includes("/config/activity-clients/artifacts/"))
    .map(({ root }) => root)
    .sort();

  assert.deepEqual(
    HOSTED_LOCAL_ACTIVITY_CLIENT_BUILDS.map(({ artifactRoot }) => artifactRoot).sort(),
    sourceRoots,
  );
  assert.deepEqual(
    HOSTED_LOCAL_ACTIVITY_CLIENT_BUILDS.map(({ packageName }) => packageName).sort(),
    [
      "@worldstream/agent-heist-client",
      "@worldstream/console",
      "@worldstream/midnight-archive-client",
    ],
  );
});

test("the canonical hosted candidate provisions its locked Playwright browser before acceptance", async () => {
  const workflow = await readFile(
    new URL(".github/workflows/hosted-foundations.yml", repository),
    "utf8",
  );
  const candidate = workflow.slice(workflow.indexOf("  local-candidate:"));

  assert.match(candidate, /pnpm install --frozen-lockfile/u);
  assert.match(candidate, /pnpm exec playwright install --with-deps --only-shell chromium/u);
  assert.ok(
    candidate.indexOf("pnpm exec playwright install --with-deps --only-shell chromium") <
      candidate.indexOf("pnpm hosted:acceptance:local"),
    "the locked browser must be installed before the rendered acceptance journey",
  );
});

test("the canonical hosted candidate qualifies the exact PR head without serializing outer gates", async () => {
  const workflow = await readFile(
    new URL(".github/workflows/hosted-foundations.yml", repository),
    "utf8",
  );
  const candidate = workflow.slice(workflow.indexOf("  local-candidate:"));

  assert.match(
    workflow,
    /concurrency:\n  group: hosted-foundations-\$\{\{ github\.event\.pull_request\.number \|\| github\.ref \}\}\n  cancel-in-progress: true/u,
  );
  assert.doesNotMatch(candidate, /^    needs:/mu);
  assert.match(
    candidate,
    /ref: \$\{\{ github\.event\.pull_request\.head\.sha \|\| github\.sha \}\}/u,
  );
  assert.match(
    candidate,
    /name: hosted-local-\$\{\{ github\.event\.pull_request\.head\.sha \|\| github\.sha \}\}/u,
  );
});

test("hosted server start retries the bounded Runtime-restart startup sequence", async () => {
  const attempts = [];
  const pauses = [];
  const result = await retryHostedServerStart(async () => {
    attempts.push("start");
    return [
      {
        code: 3,
        stdout: JSON.stringify({ command: "server start", code: "controller_unavailable" }),
        stderr: "The local controller is unavailable.",
      },
      {
        code: 4,
        stdout: JSON.stringify({ command: "server start", code: "lifecycle_incomplete", stage: "runtime_restart" }),
        stderr: "The Runtime is restarting.",
      },
      { code: 0, stdout: "", stderr: "" },
    ][attempts.length - 1];
  }, async (milliseconds) => {
    pauses.push(milliseconds);
  });
  assert.equal(result.code, 0);
  assert.deepEqual(attempts, ["start", "start", "start"]);
  assert.deepEqual(pauses, [1_000, 1_000]);
});

test("hosted server start stops after its fixed transient retry bound", async () => {
  const attempts = [];
  const pauses = [];
  const exhausted = {
    code: 4,
    stdout: JSON.stringify({ command: "server start", code: "lifecycle_incomplete", stage: "runtime_restart" }),
    stderr: "The Runtime is restarting.",
  };
  const result = await retryHostedServerStart(async () => {
    attempts.push("start");
    return exhausted;
  }, async (milliseconds) => {
    pauses.push(milliseconds);
  }, 4);
  assert.equal(result, exhausted);
  assert.deepEqual(attempts, ["start", "start", "start", "start"]);
  assert.deepEqual(pauses, [1_000, 1_000, 1_000]);
});

test("hosted server start retries only exact controller and Runtime-restart reports", async () => {
  const nonTransientReports = [
    { code: 3, command: "server start", reportCode: "lifecycle_incomplete", stage: "runtime_restart" },
    { code: 4, command: "server start", reportCode: "lifecycle_incomplete", stage: "controller_start" },
    { code: 3, command: "server status", reportCode: "controller_unavailable" },
  ];
  for (const { code, command, reportCode, stage } of nonTransientReports) {
    const attempts = [];
    const result = await retryHostedServerStart(async () => {
      attempts.push("start");
      return {
        code,
        stdout: JSON.stringify({ command, code: reportCode, ...(stage === undefined ? {} : { stage }) }),
        stderr: "non-transient failure",
      };
    }, async () => {
      throw new Error("non-transient failures must not pause or retry");
    });
    assert.equal(result.code, code);
    assert.deepEqual(attempts, ["start"]);
  }
});

test("the browser-visible Gateway port must also be unused on IPv6 localhost", async (context) => {
  const blocker = createServer();
  try {
    await new Promise((resolvePromise, rejectPromise) => {
      blocker.once("error", rejectPromise);
      blocker.listen({ host: "::1", port: 0, ipv6Only: true }, resolvePromise);
    });
  } catch (error) {
    if (error?.code === "EADDRNOTAVAIL" || error?.code === "EAFNOSUPPORT") {
      context.skip("IPv6 loopback is unavailable on this host");
      return;
    }
    throw error;
  }
  try {
    const address = blocker.address();
    assert.equal(typeof address, "object");
    await assert.rejects(
      () => assertHostedDevelopmentPortsAvailable(
        { gateway: address.port },
        new Set(),
      ),
      /Gateway browser loopback port .* is already serving another process/u,
    );
  } finally {
    await new Promise((resolvePromise) => blocker.close(resolvePromise));
  }
});

test("the browser-visible Gateway rejects an already serving IPv6 endpoint", async (context) => {
  const blocker = createServer();
  try {
    await new Promise((resolvePromise, rejectPromise) => {
      blocker.once("error", rejectPromise);
      blocker.listen({ host: "::1", port: 0, ipv6Only: true }, resolvePromise);
    });
  } catch (error) {
    if (error?.code === "EADDRNOTAVAIL" || error?.code === "EAFNOSUPPORT") {
      context.skip("IPv6 loopback is unavailable on this host");
      return;
    }
    throw error;
  }
  try {
    const address = blocker.address();
    assert.equal(typeof address, "object");
    await assert.rejects(
      () => assertHostedDevelopmentBrowserAddressUnused(address.port),
      /Gateway browser loopback port .* is already serving another process/u,
    );
  } finally {
    await new Promise((resolvePromise) => blocker.close(resolvePromise));
  }
});

test("hosted smoke uses a deterministic key derived from its exact request intent", () => {
  assert.match(HOSTED_LOCAL_SMOKE_IDEMPOTENCY_KEY, /^hosted_local_[0-9a-f]{32}$/u);
  assert.notEqual(HOSTED_LOCAL_SMOKE_IDEMPOTENCY_KEY, "hosted_local_acceptance_idempotency_key_0001");
  assert.equal(HOSTED_LOCAL_SMOKE_IDEMPOTENCY_KEY, hostedLocalSmokeIdempotencyKey());
  assert.notEqual(
    HOSTED_LOCAL_SMOKE_IDEMPOTENCY_KEY,
    hostedLocalSmokeIdempotencyKey("blake3:21d7d5439208df0b1dbb18f7f42f3a3687d248a523b03fb5b4184b2dd0dcb626"),
  );
  assert.throws(() => hostedLocalSmokeIdempotencyKey("not-a-listing"), /Listing digest/u);
});

test("hosted development reconciliation accepts only a successful bounded pass", async () => {
  const requests = [];
  const result = await reconcileHostedDevelopment(
    "http://127.0.0.1:5180",
    HOSTED_LOCAL_RECONCILIATION_SECRET,
    async (input, init) => {
      requests.push({ input, init });
      return new Response(JSON.stringify({ attempted: 1, failed: 0 }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    },
  );
  assert.deepEqual(result, { attempted: 1, failed: 0 });
  assert.equal(requests.length, 1);
  assert.equal(requests[0].input, "http://127.0.0.1:5180/api/internal/reconcile");
  assert.equal(requests[0].init.method, "GET");
  assert.equal(
    requests[0].init.headers.authorization,
    `Bearer ${HOSTED_LOCAL_RECONCILIATION_SECRET}`,
  );
  await assert.rejects(
    () => reconcileHostedDevelopment(
      "http://127.0.0.1:5180",
      HOSTED_LOCAL_RECONCILIATION_SECRET,
      async () => new Response(JSON.stringify({ attempted: 1, failed: 1 }), { status: 503 }),
    ),
    /reconciliation failed/u,
  );
});

test("fresh hosted client import supplies every exact deployment release", async () => {
  const declarationUrl = new URL("../config/activity-clients/hosted-local-import.json", import.meta.url);
  const declaration = JSON.parse(await readFile(declarationUrl, "utf8"));
  const bindings = JSON.parse(await readFile(new URL(declaration.bindings_file, declarationUrl), "utf8"));
  const releases = await Promise.all(declaration.release_files.map(async (path) =>
    JSON.parse(await readFile(new URL(path, declarationUrl), "utf8"))));
  for (const deployment of bindings.deployments) {
    assert.ok(releases.some((release) => release.client_id === deployment.client_id &&
      release.release_digest === deployment.release_digest),
    `fresh import is missing the exact release for ${deployment.deployment_id}`);
  }
});

test("the local launch probe accepts both committed and pending Gateway responses", () => {
  assert.equal(hostedDevelopmentLaunchHttpAccepted(200), true);
  assert.equal(hostedDevelopmentLaunchHttpAccepted(202), true);
  for (const status of [201, 204, 400, 401, 403, 409, 429, 500, 503, "200", null]) {
    assert.equal(hostedDevelopmentLaunchHttpAccepted(status), false);
  }
});

test("the readiness probe identity is stable and scoped to its Listing", () => {
  const retained = "blake3:8be1c66c9c69a4a67800dadf8e60d66bdf8a8b9118fb3baa96b5e8cdaf272b7d";
  const first = hostedDevelopmentReadinessProbeIdentity(retained);
  const second = hostedDevelopmentReadinessProbeIdentity(retained);
  const earlierListing = hostedDevelopmentReadinessProbeIdentity(
    "blake3:04edc964d5cbc1bc5efa422ac856305d55cec609a6ae5c5c1814c8389b776f80",
  );
  assert.deepEqual(first, second);
  assert.deepEqual(first, {
    roomSetupOperationId: "hosted-local-readiness-8be1c66c",
    reservationReference: "8be1c66c-9c69-44a6-8800-dadf8e60d66b",
  });
  assert.notDeepEqual(first, earlierListing);
});

test("retained setup detection accepts only an owned resumable setup", () => {
  assert.equal(hasRetainedHostedDevelopmentSetup({
    version: "platform_my_games.v1",
    items: [{
      launch_id: "71923a05-7fcd-4bea-a53b-0c15676e484d",
      state: "setup_pending",
      action: "continue_setup",
    }],
  }), true);
  assert.equal(hasRetainedHostedDevelopmentSetup({
    version: "platform_my_games.v1",
    items: [{
      launch_id: "71923a05-7fcd-4bea-a53b-0c15676e484d",
      state: "activity_closing",
      action: "finish_closing",
    }],
  }), true);
  for (const value of [
    null,
    { version: "platform_my_games.v1", items: [] },
    { version: "platform_my_games.v1", items: [{ launch_id: "a", state: "live", action: "return_to_game" }] },
    { version: "platform_my_games.v1", items: [{ launch_id: "a", state: "setup_pending", action: "none" }] },
    { version: "other", items: [{ launch_id: "a", state: "setup_pending", action: "continue_setup" }] },
  ]) {
    assert.equal(hasRetainedHostedDevelopmentSetup(value), false);
  }
});

test("canonical acceptance selects one release build while ordinary development stays debug", () => {
  // This contract is about the default repository target. Do not inherit a
  // caller's isolated Cargo build directory; a separate test covers the
  // production behavior that deliberately honors that configuration.
  const development = hostedNativeBuildPlan(false, undefined, {});
  const acceptance = hostedNativeBuildPlan(true, undefined, {});
  assert.equal(development.profile, "debug");
  assert.equal(acceptance.profile, "release");
  assert.deepEqual(acceptance.cargoArgs, [
    "build", "--locked", "--release",
    "-p", "worldstream-server",
    "-p", "worldstream-studio-supervisor",
    "-p", "worldstream-hosted-gateway", "--bins",
  ]);
  assert.deepEqual(development.cargoArgs, [
    "build", "--locked",
    "-p", "worldstream-server",
    "-p", "worldstream-studio-supervisor",
    "-p", "worldstream-hosted-gateway", "--bins",
  ]);
  for (const plan of [development, acceptance]) {
    assert.deepEqual(plan.copyArtifacts, []);
    for (const [field, binary] of [
      ["ctl", "worldstreamctl"],
      ["gateway", "worldstream-hosted-gateway"],
      ["managedAgentHost", "worldstream-managed-agent-host"],
    ]) {
      assert.equal(plan[field], new URL(`../target/${plan.profile}/${binary}`, import.meta.url).pathname);
    }
  }
});

test("an alternate Cargo target installs executable artifacts with their mode", async () => {
  const root = await mkdtemp(join(tmpdir(), "worldstream-hosted-native-"));
  try {
    const source = join(root, "cargo", "debug", "worldstreamctl");
    const execution = join(root, "execution", "debug");
    await mkdir(join(root, "cargo", "debug"), { recursive: true });
    await writeFile(source, "#!/bin/sh\n", { mode: 0o700 });
    await chmod(source, 0o755);
    await mkdir(execution, { recursive: true });
    await writeFile(join(execution, "worldstreamctl"), "old-bytes\n", { mode: 0o755 });
    await installHostedNativeBinaries({
      executionBinaryRoot: execution,
      copyArtifacts: [["worldstreamctl", source, join(execution, "worldstreamctl")]],
    });
    assert.equal((await stat(join(execution, "worldstreamctl"))).mode & 0o777, 0o755);
    assert.equal(await readFile(join(execution, "worldstreamctl"), "utf8"), "#!/bin/sh\n");
    assert.deepEqual(await readdir(execution), ["worldstreamctl"]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("alternate-target shutdown uses the built ctl before stable publication", () => {
  const plan = hostedNativeBuildPlan(false, "debug", { CARGO_TARGET_DIR: "/tmp/alternate-cargo-target" });
  const shutdown = hostedNativeShutdownPlan(plan, {
    configFile: "/repo/.worldstream/worldstream.toml",
    stateDirectory: "/repo/.worldstream/studio",
    controller: "127.0.0.1:9420",
  });
  assert.deepEqual(shutdown.map(({ executable, args }) => ({ executable, command: args.slice(2, 4) })), [
    { executable: "/tmp/alternate-cargo-target/debug/worldstreamctl", command: ["server", "stop"] },
    { executable: "/tmp/alternate-cargo-target/debug/worldstreamctl", command: ["server", "controller-stop"] },
  ]);
});

test("the native profile can be selected explicitly but never names arbitrary executables", () => {
  assert.equal(hostedNativeBuildPlan(true, "debug").profile, "debug");
  assert.equal(hostedNativeBuildPlan(false, "release").profile, "release");
  for (const invalid of ["", "production", "../release", "/tmp/bin", null, 1]) {
    assert.throws(() => hostedNativeBuildPlan(true, invalid), /native profile/u);
  }
});

test("the native build plan honors Cargo's configured target directory", () => {
  const targetRoot = new URL("../.worldstream/test-cargo-target", import.meta.url).pathname;
  const plan = hostedNativeBuildPlan(false, "debug", { CARGO_TARGET_DIR: targetRoot });
  const stableRoot = new URL("../target/debug", import.meta.url).pathname;
  assert.equal(plan.buildBinaryRoot, `${targetRoot}/debug`);
  assert.equal(plan.executionBinaryRoot, stableRoot);
  assert.equal(plan.ctl, `${stableRoot}/worldstreamctl`);
  assert.equal(plan.gateway, `${stableRoot}/worldstream-hosted-gateway`);
  assert.equal(plan.managedAgentHost, `${stableRoot}/worldstream-managed-agent-host`);
  assert.deepEqual(plan.copyArtifacts, [
    ["worldstreamctl", `${targetRoot}/debug/worldstreamctl`, `${stableRoot}/worldstreamctl`],
    ["worldstreamd", `${targetRoot}/debug/worldstreamd`, `${stableRoot}/worldstreamd`],
    ["worldstream-studio-supervisor", `${targetRoot}/debug/worldstream-studio-supervisor`, `${stableRoot}/worldstream-studio-supervisor`],
    ["worldstream-assignment-mcp", `${targetRoot}/debug/worldstream-assignment-mcp`, `${stableRoot}/worldstream-assignment-mcp`],
    ["worldstream-hosted-gateway", `${targetRoot}/debug/worldstream-hosted-gateway`, `${stableRoot}/worldstream-hosted-gateway`],
    ["worldstream-managed-agent-host", `${targetRoot}/debug/worldstream-managed-agent-host`, `${stableRoot}/worldstream-managed-agent-host`],
  ]);
});

test("the command keeps the release default and accepts only one closed profile override", () => {
  assert.equal(hostedDevelopmentArguments(["--acceptance"]).nativeProfile, "release");
  assert.equal(hostedDevelopmentArguments([]).nativeProfile, "debug");
  assert.deepEqual(hostedDevelopmentArguments(["--acceptance", "--native-profile=debug"]), {
    acceptance: true, checkOnly: false, outputPath: null, nativeProfile: "debug",
  });
  assert.equal(hostedDevelopmentArguments(["--check", "--native-profile=release"]).nativeProfile, "release");
  for (const args of [
    ["--acceptance", "--native-profile=custom"],
    ["--acceptance", "--native-profile=../release"],
    ["--acceptance", "--native-profile=debug", "--native-profile=release"],
    ["--acceptance", "--native-binary-root=/tmp/bin"],
  ]) assert.throws(() => hostedDevelopmentArguments(args), /native profile|usage/u);
});

test("local and Fly gateways retain public Listings and Fly admits the exact internal candidate", async () => {
  const fly = await readFile(new URL("../packaging/hosted/fly.toml", import.meta.url), "utf8");
  const value = fly.split("\n").find((line) => line.trim().startsWith("WORLDSTREAM_LISTING_ALLOWLIST = "));
  assert.ok(value);
  const deployed = JSON.parse(value.slice(value.indexOf("=") + 1).trim());
  const admitted = new Set(deployed.split(","));
  const localPublic = new Set(hostedDevelopmentListingAllowlist().split(","));
  assert.equal(admitted.size, localPublic.size + 1);
  for (const digest of localPublic) assert.ok(admitted.has(digest));
  assert.ok(admitted.has("blake3:c2e07bc3c8ff2b36a549127d1f9f6403c52dcaca45065cb5923debe705111de2"));
  assert.ok(admitted.has("blake3:d738402a5acb404dead979c21002fa02d95f4c1e89f6d4c47e617a6c6be27bc3"));
  assert.ok(admitted.has("blake3:71eb289ce6c730fa6e8eefc9d222f9730e8a69b6cc297d43b8794b7eda35f5f9"));
  assert.ok(admitted.has("blake3:4c9a98ec044e9389b9a4e3d8a8f33a371dc6ed3991556037ada61cfba4bf718a"));
  assert.ok(admitted.has("blake3:cc1c92ebc6ba7cccc9474186ff8107cf97f6bd0ce2676c6d1a2aa203c2a62d35"));
  assert.ok(admitted.has("blake3:8be1c66c9c69a4a67800dadf8e60d66bdf8a8b9118fb3baa96b5e8cdaf272b7d"));
  assert.ok(admitted.has("blake3:71805434c2530094d3a575336cb0a44d71b411ccb089e37f142d9764af860397"));
  assert.ok(admitted.has("blake3:945664f9fea18ace9991c44d43febc142a58244d69352d98514a69b4f7b22030"));
  assert.ok(admitted.has("blake3:0cd11b3aee7596f0f4c2ff5640247c29038f903914d0206a784f7adde8a84c46"));
  assert.ok(admitted.has("blake3:1cf75abcb30d77fdbe0abc5e39813a315bea6900c61e9b49c51b84d995335d74"));
  assert.ok(admitted.has("blake3:21d7d5439208df0b1dbb18f7f42f3a3687d248a523b03fb5b4184b2dd0dcb626"));
  assert.ok(admitted.has("blake3:5b0993de4c858771cce34b16cb25e03b2bf509cbe16cd1ce7249a789ea8c426f"));
  assert.ok(admitted.has("blake3:350beff2dbb28d495a5355ac19a7580f8494589bc0d5c6fec1c521be86a7cf38"));
  assert.ok(admitted.has("blake3:ab6d61d35786e51e3849c68467aad664bd35cb41f299b86d1bcce3f52e4249db"));
  assert.ok(admitted.has("blake3:48c397a32632896d66beb9ae7f8a6d090338800187c56eb0593997b80bd2b630"));
  assert.ok(admitted.has("blake3:48f76e8c1336e8f50fb6952cd0f2ff4c47cc8c372594db01422a67bca9363782"));
  assert.ok(admitted.has("blake3:9553f4fa320aa6901d0a03870f5c19ce4342d271efd2ef90d4fd287395f6cef1"));
  assert.ok(admitted.has("blake3:04edc964d5cbc1bc5efa422ac856305d55cec609a6ae5c5c1814c8389b776f80"));
  assert.ok(admitted.has("blake3:e3d401e783cec1ae4f911f682e8289054275dece60a0482b02f63e872f27dcc1"));
  assert.ok(admitted.has("blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956"));
});

test("hosted development refuses production mode", () => {
  assert.throws(() => assertHostedDevelopmentAllowed({ NODE_ENV: "production" }));
  assert.throws(() => assertHostedDevelopmentAllowed({ VERCEL_ENV: "production" }));
  assert.doesNotThrow(() => assertHostedDevelopmentAllowed({ NODE_ENV: "development" }));
});

test("review-bound ports cannot drift silently", () => {
  assert.deepEqual(hostedDevelopmentPorts({}), {
    runtime: 9410,
    controller: 9420,
    gateway: 8080,
    product: 5180,
    heist: 5173,
    platform: 3000,
    fakeOpenRouter: 8787,
  });
  assert.throws(() => hostedDevelopmentPorts({ WORLDSTREAM_HOSTED_CONTROLLER_PORT: "9999" }));
  assert.throws(() => hostedDevelopmentPorts({ WORLDSTREAM_HOSTED_HEIST_PORT: "9998" }));
  assert.throws(() => hostedDevelopmentPorts({ WORLDSTREAM_HOSTED_PRODUCT_PORT: "5190" }));
  assert.throws(() =>
    hostedDevelopmentPorts({ WORLDSTREAM_HOSTED_PRODUCT_PORT: "8080" }),
  );
});

test("the local browser stream uses a distinct loopback hostname from the product", () => {
  const product = new URL("http://127.0.0.1:5180");
  const gateway = hostedDevelopmentGatewayConfiguration({});
  const browserStream = new URL(gateway.browserStreamUrl);
  assert.deepEqual(gateway, {
    internalUrl: "http://127.0.0.1:8080",
    browserStreamUrl: "http://localhost:8080",
    publicAuthority: "localhost:8080",
  });
  assert.notEqual(browserStream.hostname, product.hostname);
  assert.equal(browserStream.port, "8080");
});

test("generated Runtime configuration retains dedicated absolute paths", () => {
  assert.equal(
    renderHostedDevelopmentConfig({
      dataDirectory: "/tmp/worldstream hosted/data",
      secretFile: "/tmp/worldstream hosted/authority.secret",
      runtimePort: 9410,
    }),
    `# Generated by pnpm hosted:dev. Retained local data is not deleted on shutdown.
config_version = 1

[server]
bind = "127.0.0.1:9410"

[storage]
profile = "sqlite-bundled"
data_dir = "/tmp/worldstream hosted/data"
deployment_lineage = "development/hosted-local"
storage_epoch = 1

[authority.bootstrap]
secret_file = "/tmp/worldstream hosted/authority.secret"
`,
  );
});
