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
    { schema: "worldstream/hosted-deployment-preflight/v1", source_revision: revision, clean: true },
  );
  for (const input of [
    { statusPorcelain: " M file" },
    { dockerSourceRevision: "c".repeat(40) },
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
    sourceRevision: revision,
    rustBuilderImage: `rust@sha256:${digest}`,
    nodeRuntimeImage: `node@sha256:${digest}`,
  });
  assert.deepEqual(plan.build, [
    "docker", "build", "--build-arg", `WORLDSTREAM_RUST_BUILDER_IMAGE=rust@sha256:${digest}`,
    "--build-arg", `WORLDSTREAM_NODE_RUNTIME_IMAGE=node@sha256:${digest}`,
    "--build-arg", `SOURCE_REVISION=${revision}`,
    "-f", "/checkout/packaging/hosted/Dockerfile", "-t", `worldstream-hosted:${revision}`, "/checkout",
  ]);
  assert.deepEqual(plan.deploy.slice(-2), ["--env", `WORLDSTREAM_DEPLOYMENT_VERSION=${revision}`]);
});
