import { strict as assert } from "node:assert";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

import {
  hostedRuntimeLayout,
  hostedStatusReady,
  renderHostedRuntimeConfig,
  renderHouseRunnerTemplate,
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
