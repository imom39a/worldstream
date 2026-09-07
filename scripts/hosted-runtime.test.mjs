import { strict as assert } from "node:assert";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

import {
  hostedRuntimeLayout,
  hostedStatusReady,
  renderHostedRuntimeConfig,
  renderHouseRunnerTemplate,
  renderHouseAgentProfiles,
  validateHostedRuntimeEnvironment,
} from "./hosted-runtime.mjs";

const digest = `blake3:${"1".repeat(64)}`;

function environment(overrides = {}) {
  return {
    NODE_ENV: "production",
    WORLDSTREAM_DEPLOYMENT_ENVIRONMENT: "production",
    WORLDSTREAM_HOSTED_INSTALLATION_ID: "fly-primary",
    WORLDSTREAM_DEPLOYMENT_VERSION: "0123456789abcdef",
    WORLDSTREAM_LISTING_ALLOWLIST: digest,
    WORLDSTREAM_PUBLIC_AUTHORITY: "worldstream-preview.fly.dev",
    WORLDSTREAM_HOSTED_CLIENT_ORIGIN: "https://worldstream.example",
    WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET_FILE: "/run/worldstream/secrets/bootstrap",
    WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY_FILE: "/run/worldstream/secrets/controller",
    WORLDSTREAM_VERCEL_SERVICE_AUTHORITY_FILE: "/run/worldstream/secrets/vercel",
    WORLDSTREAM_OPENROUTER_API_KEY_FILE: "/run/worldstream/secrets/openrouter",
    ...overrides,
  };
}

test("production runtime accepts only fixed public and secret-file bindings", () => {
  assert.doesNotThrow(() => validateHostedRuntimeEnvironment(environment()));
  assert.throws(
    () => validateHostedRuntimeEnvironment(environment({ WORLDSTREAM_DEVELOPMENT_FAKE_OPENROUTER: "visible-local-only" })),
    /development_substitute_forbidden/u,
  );
  assert.throws(
    () => validateHostedRuntimeEnvironment(environment({ WORLDSTREAM_HOSTED_CLIENT_ORIGIN: "http://worldstream.example/" })),
    /invalid_client_origin/u,
  );
  assert.throws(
    () => validateHostedRuntimeEnvironment(environment({ WORLDSTREAM_HOSTED_CLIENT_ORIGIN: "https://worldstream.example/" })),
    /invalid_client_origin/u,
  );
  assert.throws(
    () => hostedRuntimeLayout(environment({ WORLDSTREAM_OPENROUTER_API_KEY_FILE: "relative" })),
    /must_be_absolute/u,
  );
});

test("runtime configuration fixes internal listeners and persistent children", () => {
  const layout = hostedRuntimeLayout(environment({
    WORLDSTREAM_HOSTED_VOLUME_ROOT: "/var/lib/worldstream",
    WORLDSTREAM_HOSTED_ASSET_ROOT: "/opt/worldstream/hosted",
    WORLDSTREAM_HOSTED_BINARY_ROOT: "/usr/local/bin",
    WORLDSTREAM_HOSTED_EPHEMERAL_ROOT: "/run/worldstream",
  }));
  assert.equal(layout.runtimeData, "/var/lib/worldstream/runtime");
  assert.equal(layout.controllerState, "/var/lib/worldstream/studio");
  assert.equal(layout.maintenanceMarker, "/var/lib/worldstream/maintenance/closed");
  const config = renderHostedRuntimeConfig({
    dataDirectory: layout.runtimeData,
    authoritySecret: layout.authoritySecret,
  });
  assert.match(config, /bind = "127\.0\.0\.1:9410"/u);
  assert.match(config, /data_dir = "\/var\/lib\/worldstream\/runtime"/u);
  assert.match(config, /secret_file = "\/run\/worldstream\/secrets\/bootstrap"/u);
  assert.doesNotMatch(config, /0\.0\.0\.0|OPENROUTER|VERCEL_SERVICE/u);
});

test("House Runner import is exact and has no secret environment", () => {
  const manifest = renderHouseRunnerTemplate(
    "/usr/local/bin/worldstream-managed-agent-host",
    "a".repeat(64),
  );
  assert.equal(manifest.template_id, "openrouter-house");
  assert.deepEqual(manifest.compatibility, [{
    activity_pack_id: "worldstream.agent-heist",
    exact_revisions: ["0.2.0"],
  }]);
  assert.deepEqual(manifest.secret_environment, []);
  assert.equal(manifest.capacity.maximum_concurrent_invocations, 4);
});

test("fresh local and Fly imports bind the two Granite strategies to distinct exact profile revisions", () => {
  const profiles = renderHouseAgentProfiles();
  assert.deepEqual(Object.values(profiles).map(({ profile_id, revision }) => ({ profile_id, revision })), [
    { profile_id: "house-cooperative-planner", revision: "2" },
    { profile_id: "house-skeptical-auditor", revision: "1" },
  ]);
  for (const profile of Object.values(profiles)) {
    assert.equal(profile.schema, "worldstream/studio-agent-profile-publish/v2");
    assert.equal(profile.managed_provider_credential_id, "hosted-openrouter");
    assert.deepEqual(profile.non_secret_configuration, {});
    assert.deepEqual(profile.host_contract, {
      kind: "managed_house_openrouter", host_contract_revision: "1",
      runner_template: { template_id: "openrouter-house", revision: "1" },
    });
  }
});

test("managed status requires the complete ready contract", () => {
  assert.equal(hostedStatusReady({
    status: "complete",
    code: "complete",
    server: { runtime: "ready" },
  }), true);
  assert.equal(hostedStatusReady({ status: "complete", server: { runtime: "ready" } }), false);
  assert.equal(hostedStatusReady({ status: "complete", code: "complete", server: { runtime: "stopped" } }), false);
});

test("Fly package exposes only the Gateway and forbids automatic stop", async () => {
  const [dockerfile, fly, entrypoint, ignored] = await Promise.all([
    readFile("packaging/hosted/Dockerfile", "utf8"),
    readFile("packaging/hosted/fly.toml", "utf8"),
    readFile("packaging/hosted/entrypoint.sh", "utf8"),
    readFile(".dockerignore", "utf8"),
  ]);
  assert.match(dockerfile, /EXPOSE 8080/u);
  assert.doesNotMatch(dockerfile, /EXPOSE (9410|9420)/u);
  assert.match(fly, /internal_port = 8080/u);
  assert.match(fly, /auto_stop_machines = "off"/u);
  assert.match(fly, /policy = "on-failure"/u);
  assert.match(fly, /retries = 10/u);
  assert.match(fly, /kill_timeout = 120/u);
  assert.match(entrypoint, /--reuid=65532/u);
  assert.match(entrypoint, /chmod 0600/u);
  assert.match(entrypoint, /chmod 0700 "\$volume_root"/u);
  assert.match(entrypoint, /printf '%s' "\$WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET" \| wc -c\)" -ne 32/u);
  assert.match(ignored, /\.worldstream\*\//u);
  assert.doesNotMatch(
    `${dockerfile}\n${fly}`,
    /WORLDSTREAM_DEVELOPMENT_(IDENTITY_BYPASS|FAKE_OPENROUTER)|worldstream-development-key/u,
  );
});

test("Fly builder passes the declared source revision to the gitless Rust build", async () => {
  const dockerfile = await readFile("packaging/hosted/Dockerfile", "utf8");
  const builder = dockerfile.split("FROM ${WORLDSTREAM_NODE_RUNTIME_IMAGE}")[0];
  assert.match(builder, /FROM \$\{WORLDSTREAM_RUST_BUILDER_IMAGE\} AS builder\s+ARG SOURCE_REVISION/u);
  assert.match(builder, /RUN WORLDSTREAM_BUILD_REVISION="\$\{SOURCE_REVISION\}" cargo build --locked --release/u);
  assert.match(dockerfile, /org\.opencontainers\.image\.revision="\$\{SOURCE_REVISION\}"/u);
});

// Use a locally built hosted image to exercise Linux mount and privilege behavior.
// The source entrypoint replaces the image's copy; no rebuild, network, provider,
// persistent volume, or real credential is used by this regression.
const entrypointImage = process.env.WORLDSTREAM_HOSTED_ENTRYPOINT_TEST_IMAGE;
const entrypointSkip = entrypointImage === undefined
  ? "set WORLDSTREAM_HOSTED_ENTRYPOINT_TEST_IMAGE to a locally built hosted image"
  : false;

async function runEntrypoint(bootstrapSecret) {
  const fixtureRoot = await mkdtemp(join(tmpdir(), "worldstream-entrypoint-test-"));
  const runtime = join(fixtureRoot, "runtime.mjs");
  try {
    await writeFile(runtime, `
import { strict as assert } from "node:assert";
import { readFile, stat } from "node:fs/promises";
assert.equal(process.getuid(), 65532);
assert.equal(process.getgid(), 65532);
const volume = await stat("/var/lib/worldstream");
assert.equal(volume.mode & 0o777, 0o700, "mounted volume root must be owner-only");
assert.equal(volume.uid, 65532);
assert.equal(volume.gid, 65532);
for (const [variable, name] of [
  ["WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET", "authority-bootstrap"],
  ["WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY", "controller-authority"],
  ["WORLDSTREAM_VERCEL_SERVICE_AUTHORITY", "vercel-service-authority"],
  ["OPENROUTER_API_KEY", "openrouter-api-key"],
]) {
  assert.equal(process.env[variable], undefined, "credential must not reach the child environment");
  const path = "/run/worldstream/secrets/" + name;
  const info = await stat(path);
  assert.equal(info.mode & 0o777, 0o600);
  assert.equal(info.uid, 65532);
  assert.equal(info.gid, 65532);
  if (name === "authority-bootstrap") {
    assert.equal((await readFile(path)).length, 32, "kernel bootstrap file must contain exactly 32 bytes");
  }
}
console.log("hosted_entrypoint_contract_verified");
`, { mode: 0o644 });
    return spawnSync("docker", [
      "run", "--rm", "--pull=never", "--platform=linux/amd64", "--network=none", "--read-only", "--user=0:0",
      "--tmpfs", "/var/lib/worldstream:rw,mode=0755,uid=0,gid=0",
      "--tmpfs", "/run/worldstream:rw,mode=0755,uid=0,gid=0",
      "--mount", `type=bind,src=${resolve("packaging/hosted/entrypoint.sh")},dst=/test-entrypoint.sh,readonly`,
      "--mount", `type=bind,src=${runtime},dst=/opt/worldstream/hosted/hosted-runtime.mjs,readonly`,
      "--env", "WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET",
      "--env", "WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY",
      "--env", "WORLDSTREAM_VERCEL_SERVICE_AUTHORITY",
      "--env", "OPENROUTER_API_KEY",
      "--entrypoint", "/bin/sh", entrypointImage, "/test-entrypoint.sh",
    ], {
      encoding: "utf8",
      timeout: 30_000,
      env: {
        ...process.env,
        WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET: bootstrapSecret,
        WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY: "controller-fixture-".padEnd(32, "x"),
        WORLDSTREAM_VERCEL_SERVICE_AUTHORITY: "vercel-fixture-".padEnd(32, "x"),
        OPENROUTER_API_KEY: "openrouter-fixture-".padEnd(32, "x"),
      },
    });
  } finally {
    await rm(fixtureRoot, { recursive: true, force: true });
  }
}

test("hosted entrypoint repairs mounted root mode and materializes exact private bytes", {
  skip: entrypointSkip,
}, async () => {
  // The second value has 16 characters but exactly 32 UTF-8 bytes.
  for (const secret of ["bootstrap-fixture-".padEnd(32, "x"), "é".repeat(16)]) {
    const result = await runEntrypoint(secret);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), "hosted_entrypoint_contract_verified");
  }
});

test("hosted entrypoint rejects wrong bootstrap byte counts before Runtime startup", {
  skip: entrypointSkip,
}, async () => {
  for (const secret of ["x".repeat(64), "x".repeat(31), "x".repeat(33), "é".repeat(32)]) {
    const result = await runEntrypoint(secret);
    assert.equal(result.status, 78, result.stderr);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /Runtime bootstrap secret must contain exactly 32 bytes/u);
    assert.equal(result.stderr.includes(secret), false, "failure must not disclose credential bytes");
  }
});
