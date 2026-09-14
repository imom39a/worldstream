#!/usr/bin/env node
/**
 * Pure, bounded preflight for the first hosted Fly deployment.
 *
 * This module deliberately does not call Fly, Docker, or mutate a Machine.
 * It verifies the exact clean source identity and constructs the commands an
 * operator may review and run afterwards.
 */
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const DEPLOYMENT_REVISION_PATTERN = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/u;
export const HOSTED_DEPLOYMENT_PREFLIGHT_SCHEMA =
  "worldstream/hosted-deployment-preflight/v2";

function exactRevision(value, label) {
  if (typeof value !== "string" || !DEPLOYMENT_REVISION_PATTERN.test(value)) {
    throw new Error(`${label} must be a 40- or 64-character lowercase hex revision`);
  }
  return value;
}

/**
 * Verify all three deployment identities and the clean checkout contract.
 * `statusPorcelain` is supplied by the caller so the function is deterministic
 * and easy to test without touching a repository.
 */
export function validateHostedDeploymentBinding({
  sourceRevision,
  dockerSourceRevision,
  runtimeDeploymentVersion,
  statusPorcelain = "",
}) {
  const source = exactRevision(sourceRevision, "source revision");
  const docker = exactRevision(dockerSourceRevision, "Docker SOURCE_REVISION");
  const runtime = exactRevision(runtimeDeploymentVersion, "WORLDSTREAM_DEPLOYMENT_VERSION");
  if (statusPorcelain.length !== 0) throw new Error("source checkout must be clean");
  if (source !== docker || source !== runtime) {
    throw new Error("source, Docker, and runtime deployment revisions must match exactly");
  }
  return Object.freeze({ source_revision: source, clean: true, schema: HOSTED_DEPLOYMENT_PREFLIGHT_SCHEMA });
}

/** Construct reviewable commands rooted at the repository, including the exact runtime binding. */
export function hostedFirstDeploymentPlan({
  repositoryRoot,
  app,
  clientOrigin,
  sourceRevision,
  rustBuilderImage,
  nodeRuntimeImage,
}) {
  const root = resolve(repositoryRoot);
  const revision = exactRevision(sourceRevision, "source revision");
  if (typeof app !== "string" || !/^[a-z][a-z0-9-]{0,63}$/u.test(app)) {
    throw new Error("Fly app must be a lowercase DNS label");
  }
  const origin = new URL(clientOrigin);
  if (
    origin.protocol !== "https:" ||
    origin.username !== "" ||
    origin.password !== "" ||
    origin.pathname !== "/" ||
    origin.search !== "" ||
    origin.hash !== ""
  ) {
    throw new Error("hosted client origin must be an exact HTTPS origin");
  }
  if (typeof rustBuilderImage !== "string" || !/@sha256:[0-9a-f]{64}$/u.test(rustBuilderImage)) {
    throw new Error("Rust builder image must be digest pinned");
  }
  if (typeof nodeRuntimeImage !== "string" || !/@sha256:[0-9a-f]{64}$/u.test(nodeRuntimeImage)) {
    throw new Error("Node runtime image must be digest pinned");
  }
  return Object.freeze({
    schema: HOSTED_DEPLOYMENT_PREFLIGHT_SCHEMA,
    source_revision: revision,
    dockerfile: `${root}/packaging/hosted/Dockerfile`,
    deploy: Object.freeze([
      "fly", "deploy", "-a", app, "-c", `${root}/packaging/hosted/fly.toml`,
      "--remote-only", "--ha=false",
      "--build-arg", `WORLDSTREAM_RUST_BUILDER_IMAGE=${rustBuilderImage}`,
      "--build-arg", `WORLDSTREAM_NODE_RUNTIME_IMAGE=${nodeRuntimeImage}`,
      "--build-arg", `SOURCE_REVISION=${revision}`,
      "--env", `WORLDSTREAM_DEPLOYMENT_VERSION=${revision}`,
      "--env", `WORLDSTREAM_PUBLIC_AUTHORITY=${app}.fly.dev`,
      "--env", `WORLDSTREAM_HOSTED_CLIENT_ORIGIN=${origin.origin}`,
    ]),
  });
}

export function readCleanHostedSource(repositoryRoot) {
  const cwd = resolve(repositoryRoot);
  const sourceRevision = execFileSync("git", ["rev-parse", "HEAD"], { cwd, encoding: "utf8" }).trim();
  const statusPorcelain = execFileSync("git", ["status", "--porcelain"], { cwd, encoding: "utf8" });
  return { sourceRevision, statusPorcelain };
}

function main() {
  const [command, app, clientOrigin, rustBuilderImage, nodeRuntimeImage] = process.argv.slice(2);
  if (command !== "plan" || app === undefined || clientOrigin === undefined || rustBuilderImage === undefined || nodeRuntimeImage === undefined) {
    throw new Error("usage: hosted-deploy-preflight.mjs plan <app> <https-client-origin> <rust-image@sha256:digest> <node-image@sha256:digest>");
  }
  const source = readCleanHostedSource(resolve(fileURLToPath(new URL("..", import.meta.url))));
  const binding = validateHostedDeploymentBinding({
    sourceRevision: source.sourceRevision,
    dockerSourceRevision: source.sourceRevision,
    runtimeDeploymentVersion: source.sourceRevision,
    statusPorcelain: source.statusPorcelain,
  });
  const plan = hostedFirstDeploymentPlan({
    repositoryRoot: resolve(fileURLToPath(new URL("..", import.meta.url))),
    app,
    clientOrigin,
    sourceRevision: binding.source_revision,
    rustBuilderImage,
    nodeRuntimeImage,
  });
  process.stdout.write(`${JSON.stringify(plan)}\n`);
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) main();
