import assert from "node:assert/strict";
import test from "node:test";
import { hostedFirstDeploymentPlan, validateHostedDeploymentBinding } from "./hosted-deploy-preflight.mjs";

const revision = "a".repeat(40);
const digest = "b".repeat(64);

test("deployment binding requires a clean exact identity", () => {
  assert.deepEqual(
    validateHostedDeploymentBinding({
      sourceRevision: revision,
      dockerSourceRevision: revision,
      runtimeDeploymentVersion: revision,
    }),
    { schema: "worldstream/hosted-deployment-preflight/v2", source_revision: revision, clean: true },
  );
  for (const input of [
    { statusPorcelain: " M file" },
    { dockerSourceRevision: undefined },
    { dockerSourceRevision: "c".repeat(40) },
    { runtimeDeploymentVersion: undefined },
    { runtimeDeploymentVersion: "bad" },
    { sourceRevision: "A".repeat(40) },
  ]) {
    assert.throws(() => validateHostedDeploymentBinding({
      sourceRevision: revision,
      dockerSourceRevision: revision,
      runtimeDeploymentVersion: revision,
      ...input,
    }));
  }
});

test("first deployment commands resolve the Dockerfile from repository root and bind runtime revision", () => {
  const plan = hostedFirstDeploymentPlan({
    repositoryRoot: "/checkout",
    app: "worldstream-preview",
    clientOrigin: "https://worldstream.example",
    sourceRevision: revision,
    rustBuilderImage: `rust@sha256:${digest}`,
    nodeRuntimeImage: `node@sha256:${digest}`,
  });
  assert.deepEqual(plan.deploy, [
    "fly", "deploy", "-a", "worldstream-preview", "-c", "/checkout/packaging/hosted/fly.toml",
    "--remote-only", "--ha=false",
    "--build-arg", `WORLDSTREAM_RUST_BUILDER_IMAGE=rust@sha256:${digest}`,
    "--build-arg", `WORLDSTREAM_NODE_RUNTIME_IMAGE=node@sha256:${digest}`,
    "--build-arg", `SOURCE_REVISION=${revision}`,
    "--env", `WORLDSTREAM_DEPLOYMENT_VERSION=${revision}`,
    "--env", "WORLDSTREAM_PUBLIC_AUTHORITY=worldstream-preview.fly.dev",
    "--env", "WORLDSTREAM_HOSTED_CLIENT_ORIGIN=https://worldstream.example",
  ]);
  assert.throws(() => hostedFirstDeploymentPlan({
    repositoryRoot: "/checkout",
    app: "worldstream-preview",
    clientOrigin: "http://worldstream.example/path",
    sourceRevision: revision,
    rustBuilderImage: `rust@sha256:${digest}`,
    nodeRuntimeImage: `node@sha256:${digest}`,
  }));
});
