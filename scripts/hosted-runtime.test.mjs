import { strict as assert } from "node:assert";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

import {
  hostedGatewayConfiguration,
  hostedRuntimeLayout,
  hostedStatusReady,
  renderHostedRuntimeConfig,
  renderHouseRunnerTemplate,
  renderHouseAgentProfiles,
  retainRunnerExecutable,
  verifyManagedAgentHostDigest,
  validateHostedRuntimeEnvironment,
} from "./hosted-runtime.mjs";

const digest = `blake3:${"1".repeat(64)}`;

test("the deployed retained catalog fits the allowlist contract, including 64 revisions", async () => {
  const fly = await readFile(new URL("../packaging/hosted/fly.toml", import.meta.url), "utf8");
  const line = fly.split("\n").find((value) => value.trim().startsWith("WORLDSTREAM_LISTING_ALLOWLIST = "));
  const allowlist = JSON.parse(line.slice(line.indexOf("=") + 1));
  assert.ok(allowlist.length > 512, "exercise the former scalar-string limit");
  assert.ok(
    allowlist.split(",").includes("blake3:71805434c2530094d3a575336cb0a44d71b411ccb089e37f142d9764af860397"),
    "the Fly Gateway must admit the current discovery Listing",
  );
  assert.doesNotThrow(() => validateHostedRuntimeEnvironment(environment({ WORLDSTREAM_LISTING_ALLOWLIST: allowlist })));
  assert.doesNotThrow(() => validateHostedRuntimeEnvironment(environment({ WORLDSTREAM_LISTING_ALLOWLIST: Array(64).fill(digest).join(",") })));
  assert.throws(() => validateHostedRuntimeEnvironment(environment({ WORLDSTREAM_LISTING_ALLOWLIST: Array(65).fill(digest).join(",") })));
  assert.throws(() => validateHostedRuntimeEnvironment(environment({ WORLDSTREAM_HOSTED_INSTALLATION_ID: "a".repeat(513) })));
});

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

test("the local appliance can move its Gateway without changing Fly's port contract", () => {
  assert.deepEqual(hostedGatewayConfiguration({}), {
    port: 8080,
    bind: "0.0.0.0:8080",
    loopbackOrigin: "http://127.0.0.1:8080",
    publicAuthority: "127.0.0.1:8080",
  });
  assert.deepEqual(hostedGatewayConfiguration({ WORLDSTREAM_HOSTED_GATEWAY_PORT: "18080" }), {
    port: 18080,
    bind: "0.0.0.0:18080",
    loopbackOrigin: "http://127.0.0.1:18080",
    publicAuthority: "127.0.0.1:18080",
  });
  for (const value of ["0", "65536", "8080x", "9410", "9420"]) {
    assert.throws(() => hostedGatewayConfiguration({ WORLDSTREAM_HOSTED_GATEWAY_PORT: value }));
  }
  assert.throws(() => hostedGatewayConfiguration({
    FLY_APP_NAME: "worldstream-preview",
    WORLDSTREAM_HOSTED_GATEWAY_PORT: "18080",
  }), /fly_gateway_port_override_forbidden/u);
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

test("the hosted image packages the same current client as hosted bindings", async () => {
  const dockerfile = await readFile(new URL("../packaging/hosted/Dockerfile", import.meta.url), "utf8");
  const name = dockerfile.match(/COPY config\/activity-clients\/releases\/(agent-heist-web-v\d+\.json) \/opt\/worldstream\/hosted\/agent-heist-web\.json/u)?.[1];
  assert.ok(name, "image must copy an exact reviewed client release");
  const release = JSON.parse(await readFile(new URL(`../config/activity-clients/releases/${name}`, import.meta.url), "utf8"));
  const bindings = JSON.parse(await readFile(new URL("../config/activity-clients/hosted-local-bindings.json", import.meta.url), "utf8"));
  const deployment = bindings.deployments.find(value => value.client_id === release.client_id);
  assert.equal(deployment.release_digest, release.release_digest);
});

test("the packaged managed Host digest is raw hex and mismatches fail closed", async () => {
  const dockerfile = await readFile(new URL("../packaging/hosted/Dockerfile", import.meta.url), "utf8");
  assert.match(
    dockerfile,
    /worldstream-hosted-artifact-digest\s+\\\n?\s*target\/release\/worldstream-managed-agent-host\s+\\\n?\s*> \/out\/managed-agent-host\.blake3/u,
  );
  const raw = "a".repeat(64);
  assert.equal(verifyManagedAgentHostDigest(raw, raw), raw);
  assert.throws(() => verifyManagedAgentHostDigest(`blake3:${raw}`, raw), /managed_agent_host_digest_invalid/u);
  assert.throws(() => verifyManagedAgentHostDigest(raw, "b".repeat(64)), /managed_agent_host_digest_mismatch/u);
});

test("House Runner import is exact and has no secret environment", () => {
  const manifest = renderHouseRunnerTemplate(
    "/var/lib/worldstream/retained-runner-executables/blake3-a/worldstream-managed-agent-host",
    "a".repeat(64),
  );
  assert.equal(manifest.template_id, "openrouter-house");
  assert.equal(manifest.revision, "16");
  assert.deepEqual(manifest.instances, [{
    instance_id: "hosted-house-r16-01", health_address: "127.0.0.1:9606",
  }]);
  assert.deepEqual(manifest.compatibility, [{
    activity_pack_id: "worldstream.agent-heist",
    exact_revisions: ["0.5.0"],
  }]);
  assert.deepEqual(manifest.secret_environment, []);
  assert.equal(manifest.capacity.maximum_concurrent_invocations, 4);
});

test("successor House Runner instances do not collide with retained installations", () => {
  for (const retained of [
    { instance_id: "hosted-house-01", health_address: "127.0.0.1:9591" },
    { instance_id: "hosted-house-r2-01", health_address: "127.0.0.1:9592" },
    { instance_id: "hosted-house-r6-01", health_address: "127.0.0.1:9596" },
    { instance_id: "hosted-house-r10-01", health_address: "127.0.0.1:9600" },
    { instance_id: "hosted-house-r11-01", health_address: "127.0.0.1:9601" },
    { instance_id: "hosted-house-r12-01", health_address: "127.0.0.1:9602" },
    { instance_id: "hosted-house-r13-01", health_address: "127.0.0.1:9603" },
    { instance_id: "hosted-house-r14-01", health_address: "127.0.0.1:9604" },
    { instance_id: "hosted-house-r15-01", health_address: "127.0.0.1:9605" },
  ]) {
  const successor = renderHouseRunnerTemplate("/var/lib/worldstream/retained-runner-executables/blake3-a/worldstream-managed-agent-host", "a".repeat(64));
  for (const instance of successor.instances) {
    assert.notEqual(instance.instance_id, retained.instance_id,
      "the registry rejects duplicate instance IDs across immutable revisions");
    assert.notEqual(instance.health_address, retained.health_address,
      "retained and successor runners must not compete for one listener");
  }
  }
});

test("fresh local and Fly imports bind the two Granite strategies to distinct exact profile revisions", () => {
  const profiles = renderHouseAgentProfiles();
  assert.deepEqual(Object.values(profiles).map(({ profile_id, revision }) => ({ profile_id, revision })), [
    { profile_id: "house-cooperative-planner", revision: "17" },
    { profile_id: "house-skeptical-auditor", revision: "16" },
  ]);
  for (const profile of Object.values(profiles)) {
    assert.equal(profile.schema, "worldstream/studio-agent-profile-publish/v2");
    assert.equal(profile.managed_provider_credential_id, "hosted-openrouter");
    assert.deepEqual(profile.non_secret_configuration, {});
    assert.deepEqual(profile.host_contract, {
      kind: "managed_house_openrouter", host_contract_revision: "1",
      runner_template: { template_id: "openrouter-house", revision: "16" },
    });
  }
});

test("r12 through r15 keep their retained executable bytes after r16 installs", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "worldstream-retained-runner-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const source = join(root, "mutable-build-output");
  const retainedRoot = join(root, "retained");
  await writeFile(source, "r12 executable bytes", { mode: 0o700 });
  const r12 = await retainRunnerExecutable({
    source, retainedRoot, digest: "a".repeat(64),
  });
  await writeFile(source, "r13 successor executable bytes", { mode: 0o700 });
  const r13 = await retainRunnerExecutable({
    source, retainedRoot, digest: "b".repeat(64),
  });
  await writeFile(source, "r14 successor executable bytes", { mode: 0o700 });
  const r14 = await retainRunnerExecutable({
    source, retainedRoot, digest: "c".repeat(64),
  });
  await writeFile(source, "r15 successor executable bytes", { mode: 0o700 });
  const r15 = await retainRunnerExecutable({
    source, retainedRoot, digest: "d".repeat(64),
  });
  await writeFile(source, "r16 successor executable bytes", { mode: 0o700 });
  const r16 = await retainRunnerExecutable({
    source, retainedRoot, digest: "e".repeat(64),
  });
  const r12Template = {
    ...renderHouseRunnerTemplate(r12, "a".repeat(64)), revision: "12",
    instances: [{ instance_id: "hosted-house-r12-01", health_address: "127.0.0.1:9602" }],
  };
  const r13Template = {
    ...renderHouseRunnerTemplate(r13, "b".repeat(64)), revision: "13",
    instances: [{ instance_id: "hosted-house-r13-01", health_address: "127.0.0.1:9603" }],
  };
  const r14Template = {
    ...renderHouseRunnerTemplate(r14, "c".repeat(64)), revision: "14",
    instances: [{ instance_id: "hosted-house-r14-01", health_address: "127.0.0.1:9604" }],
  };
  const r15Template = {
    ...renderHouseRunnerTemplate(r15, "d".repeat(64)), revision: "15",
    instances: [{ instance_id: "hosted-house-r15-01", health_address: "127.0.0.1:9605" }],
  };
  const r16Template = renderHouseRunnerTemplate(r16, "e".repeat(64));
  assert.notEqual(r12, r13);
  assert.notEqual(r13, r14);
  assert.notEqual(r14, r15);
  assert.equal(r12Template.revision, "12");
  assert.equal(r12Template.executable.path, r12);
  assert.equal(r13Template.revision, "13");
  assert.equal(r13Template.executable.path, r13);
  assert.equal(r14Template.revision, "14");
  assert.equal(r14Template.executable.path, r14);
  assert.equal(r15Template.revision, "15");
  assert.equal(r15Template.executable.path, r15);
  assert.equal(r16Template.revision, "16");
  assert.equal(r16Template.executable.path, r16);
  assert.equal(await readFile(r12, "utf8"), "r12 executable bytes",
    "an active r12 Assignment still resolves its content-addressed retained bytes");
  assert.equal(await readFile(r13, "utf8"), "r13 successor executable bytes");
  assert.equal(await readFile(r14, "utf8"), "r14 successor executable bytes");
  assert.equal(await readFile(r15, "utf8"), "r15 successor executable bytes");
  assert.equal(await readFile(r16, "utf8"), "r16 successor executable bytes");
  await assert.rejects(
    retainRunnerExecutable({ source, retainedRoot, digest: "a".repeat(64) }),
    /retained_runner_digest_collision/u,
    "a retained r12 digest address is never overwritten with successor bytes",
  );
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
