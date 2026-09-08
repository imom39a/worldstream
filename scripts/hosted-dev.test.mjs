import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

import {
  assertHostedDevelopmentAllowed,
  hostedDevelopmentPorts,
  hostedDevelopmentListingAllowlist,
  hostedDevelopmentLaunchHttpAccepted,
  hostedDevelopmentArguments,
  hostedNativeBuildPlan,
  renderHostedDevelopmentConfig,
} from "./hosted-dev.mjs";

test("the local launch probe accepts both committed and pending Gateway responses", () => {
  assert.equal(hostedDevelopmentLaunchHttpAccepted(200), true);
  assert.equal(hostedDevelopmentLaunchHttpAccepted(202), true);
  for (const status of [201, 204, 400, 401, 403, 409, 429, 500, 503, "200", null]) {
    assert.equal(hostedDevelopmentLaunchHttpAccepted(status), false);
  }
});

test("canonical acceptance selects one release build while ordinary development stays debug", () => {
  const development = hostedNativeBuildPlan(false);
  const acceptance = hostedNativeBuildPlan(true);
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
    for (const [field, binary] of [
      ["ctl", "worldstreamctl"],
      ["gateway", "worldstream-hosted-gateway"],
      ["managedAgentHost", "worldstream-managed-agent-host"],
    ]) {
      assert.equal(plan[field], new URL(`../target/${plan.profile}/${binary}`, import.meta.url).pathname);
    }
  }
});

test("the native profile can be selected explicitly but never names arbitrary executables", () => {
  assert.equal(hostedNativeBuildPlan(true, "debug").profile, "debug");
  assert.equal(hostedNativeBuildPlan(false, "release").profile, "release");
  for (const invalid of ["", "production", "../release", "/tmp/bin", null, 1]) {
    assert.throws(() => hostedNativeBuildPlan(true, invalid), /native profile/u);
  }
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

test("local and Fly gateways retain all retained Listings as well as current discovery", async () => {
  const fly = await readFile(new URL("../packaging/hosted/fly.toml", import.meta.url), "utf8");
  const value = fly.split("\n").find((line) => line.trim().startsWith("WORLDSTREAM_LISTING_ALLOWLIST = "));
  assert.ok(value);
  const deployed = JSON.parse(value.slice(value.indexOf("=") + 1).trim());
  assert.equal(deployed, hostedDevelopmentListingAllowlist());
  const admitted = new Set(deployed.split(","));
  assert.equal(admitted.size, 11);
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
