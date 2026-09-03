#!/usr/bin/env node
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { activityClientBuildDigest, activityClientReleaseDigest } from "./activity-client-identities.mjs";

const args = argumentsFrom(process.argv.slice(2));
const required = ["dist", "source-release", "pack-id", "pack-version", "pack-digest", "access-mode", "roles", "surface", "entrypoint", "launch-url",
  "fallback-dist", "fallback-source-release", "fallback-surface", "fallback-entrypoint", "fallback-launch-url", "output"];
for (const name of required) if (!args.has(name)) fail(`missing --${name}`);
const output = resolve(args.get("output"));
try {
  if ((await readdir(output)).length !== 0) fail("output directory must be empty");
} catch (error) {
  if (error?.code !== "ENOENT") throw error;
}

const launchUrl = args.get("launch-url");
const fallbackLaunchUrl = args.get("fallback-launch-url");
for (const value of [launchUrl, fallbackLaunchUrl]) validateLaunchUrl(value);
for (const value of [args.get("pack-digest")]) if (!/^(?:blake3|sha256):[0-9a-f]{64}$/.test(value)) fail("Pack digest is invalid");
if (!isIdentifier(args.get("pack-id")) || !isVersion(args.get("pack-version"))) fail("Pack identity is invalid");
const roles = args.get("roles") === "" ? [] : args.get("roles").split(",");
const accessMode = args.get("access-mode");
if (roles.length > 64 || new Set(roles).size !== roles.length || roles.some((role) => !isRole(role))
  || (accessMode === "participant") !== (roles.length > 0) || !["participant", "spectator"].includes(accessMode)) fail("Access Mode and Roles are invalid");

const { release, surface, artifactDigest } = await prepareRelease(
  args.get("source-release"), args.get("dist"), args.get("surface"), args.get("entrypoint"),
);
const { release: fallbackRelease, surface: fallbackSurface } = await prepareRelease(
  args.get("fallback-source-release"), args.get("fallback-dist"), args.get("fallback-surface"), args.get("fallback-entrypoint"),
);
if (fallbackRelease.client_id === release.client_id) fail("fallback Release must use a separate Client identity");
await mkdir(output, { recursive: true, mode: 0o700 });
const suffix = release.release_digest.slice("sha256:".length, "sha256:".length + 12);
const deploymentId = `local-${safeId(release.client_id)}-${suffix}`;
const fallbackSuffix = fallbackRelease.release_digest.slice("sha256:".length, "sha256:".length + 12);
const fallbackDeploymentId = `local-${safeId(fallbackRelease.client_id)}-${fallbackSuffix}`;
const bindingId = `${deploymentId}-${safeId(args.get("pack-id"))}-${accessMode}`;
const bindings = {
  schema: "worldstream/client-binding-bootstrap/v1",
  deployment_trust_policy: "allow_externally_trusted",
  deployments: [
    { schema: "worldstream/client-deployment/v1", deployment_id: deploymentId,
      client_id: release.client_id, release_digest: release.release_digest, trust_level: "externally_trusted",
      surfaces: [{ surface_id: surface.surface_id, launch_url: launchUrl }] },
    { schema: "worldstream/client-deployment/v1", deployment_id: fallbackDeploymentId,
      client_id: fallbackRelease.client_id, release_digest: fallbackRelease.release_digest, trust_level: "externally_trusted",
      surfaces: [{ surface_id: fallbackSurface.surface_id, launch_url: fallbackLaunchUrl }] },
  ],
  bindings: [{ schema: "worldstream/client-binding/v1", binding_id: bindingId,
    pack: { id: args.get("pack-id"), version: args.get("pack-version"), digest: args.get("pack-digest") },
    client_contract: release.client_contract, access_mode: accessMode, roles,
    deployment_id: deploymentId, surface_id: surface.surface_id, preference: "default" }],
  inspector_fallback: { schema: "worldstream/inspector-fallback/v1",
    fallback_id: `${fallbackDeploymentId}-fallback`, deployment_id: fallbackDeploymentId,
    surface_id: fallbackSurface.surface_id },
};
const releaseFile = "activity-client-release.json";
const fallbackReleaseFile = "inspector-client-release.json";
const bindingsFile = "client-bindings.json";
const declaration = { schema: "worldstream/client-declaration-import/v1", release_files: [releaseFile, fallbackReleaseFile], bindings_file: bindingsFile };
await writeJson(output, releaseFile, release);
await writeJson(output, fallbackReleaseFile, fallbackRelease);
await writeJson(output, bindingsFile, bindings);
await writeJson(output, "client-declaration.json", declaration);
process.stdout.write(`${JSON.stringify({ schema: "worldstream/local-activity-client-preparation/v1", status: "prepared_for_review", client_id: release.client_id, release_digest: release.release_digest, artifact_digest: artifactDigest, declaration: resolve(output, "client-declaration.json"), qualification: "not_claimed" })}\n`);

function argumentsFrom(values) {
  const parsed = new Map();
  for (let index = 0; index < values.length; index += 2) {
    const flag = values[index]; const value = values[index + 1];
    if (!flag?.startsWith("--") || value === undefined || value.startsWith("--") || parsed.has(flag.slice(2))) fail("arguments must be unique --name value pairs");
    parsed.set(flag.slice(2), value);
  }
  return parsed;
}
function safeId(value) { return value.replaceAll(/[^a-z0-9-]/g, "-").replaceAll(/-+/g, "-").slice(0, 64); }
async function prepareRelease(sourcePath, distPath, surfaceId, entrypoint) {
  const source = JSON.parse(await readFile(resolve(sourcePath), "utf8"));
  validateSourceRelease(source);
  const surface = source.surfaces?.find((candidate) => candidate.surface_id === surfaceId && candidate.entrypoint === entrypoint);
  if (surface === undefined || source.artifacts?.length !== 1) fail("source Release surface is unavailable");
  const artifactDigest = await activityClientBuildDigest(resolve(distPath));
  const release = structuredClone(source);
  release.artifacts[0].digest = artifactDigest;
  for (const candidate of release.surfaces) candidate.artifact_digest = artifactDigest;
  release.conformance = [];
  release.release_digest = activityClientReleaseDigest(release);
  return { release, surface, artifactDigest };
}
function validateSourceRelease(source) {
  if (!isExactObject(source, ["schema", "client_id", "release_digest", "client_contract", "artifacts", "surfaces", "conformance"])
    || source.schema !== "worldstream/activity-client-release/v1" || !isIdentifier(source.client_id)
    || !isDigest(source.release_digest) || !isContract(source.client_contract)
    || !Array.isArray(source.artifacts) || source.artifacts.length < 1 || source.artifacts.length > 32
    || !Array.isArray(source.surfaces) || source.surfaces.length < 1 || source.surfaces.length > 32
    || !Array.isArray(source.conformance) || source.conformance.length > 32) fail("source Release is invalid");
  const artifactIds = new Set(); const artifactDigests = new Set();
  for (const artifact of source.artifacts) {
    if (!isExactObject(artifact, ["artifact_id", "media_type", "digest"])
      || !isIdentifier(artifact.artifact_id) || artifactIds.has(artifact.artifact_id)
      || typeof artifact.media_type !== "string" || Buffer.byteLength(artifact.media_type) > 256
      || !artifact.media_type.includes("/") || !isGraphic(artifact.media_type) || !isDigest(artifact.digest)) fail("source Release is invalid");
    artifactIds.add(artifact.artifact_id); artifactDigests.add(artifact.digest);
  }
  const surfaceIds = new Set();
  for (const surface of source.surfaces) {
    if (!isExactObject(surface, ["surface_id", "kind", "artifact_digest", "entrypoint", "capabilities"])
      || !isIdentifier(surface.surface_id) || surfaceIds.has(surface.surface_id)
      || !["browser", "cli", "tui", "native", "sdk"].includes(surface.kind)
      || !artifactDigests.has(surface.artifact_digest) || !isEntrypoint(surface.entrypoint)
      || !Array.isArray(surface.capabilities) || surface.capabilities.length < 1 || surface.capabilities.length > 32
      || new Set(surface.capabilities).size !== surface.capabilities.length
      || surface.capabilities.some((value) => !isContract(value))) fail("source Release is invalid");
    surfaceIds.add(surface.surface_id);
  }
  for (const evidence of source.conformance) {
    if (!isExactObject(evidence, ["contract", "evidence_digest"])
      || !isContract(evidence.contract) || !isDigest(evidence.evidence_digest)) fail("source Release is invalid");
  }
}
function isExactObject(value, keys) {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).length === keys.length && keys.every((key) => Object.hasOwn(value, key));
}
function isIdentifier(value) {
  return typeof value === "string" && Buffer.byteLength(value) <= 128 && /^[a-z0-9][a-z0-9._-]*$/.test(value);
}
function isRole(value) {
  return typeof value === "string" && Buffer.byteLength(value) <= 128 && /^[A-Za-z0-9._-]+$/.test(value);
}
function isContract(value) {
  return typeof value === "string" && Buffer.byteLength(value) <= 128 && /^[a-z0-9._\/-]+$/.test(value);
}
function isDigest(value) { return typeof value === "string" && /^(?:blake3|sha256):[0-9a-f]{64}$/.test(value); }
function isVersion(value) {
  return typeof value === "string" && Buffer.byteLength(value) > 0 && Buffer.byteLength(value) <= 128
    && isGraphic(value) && !value.includes("/") && !value.includes("\\");
}
function isEntrypoint(value) {
  return typeof value === "string" && Buffer.byteLength(value) > 0 && Buffer.byteLength(value) <= 1_024
    && isGraphic(value) && !/[?#\\\r\n]/.test(value) && !value.split("/").some((part) => part === "." || part === "..");
}
function isGraphic(value) { return [...Buffer.from(value)].every((byte) => byte >= 0x21 && byte <= 0x7e); }
function validateLaunchUrl(value) {
  if (!/^http:\/\/(?:127\.0\.0\.1|localhost|\[::1\])(?::[1-9][0-9]{0,4})?\/[a-z0-9._/-]+\/$/.test(value)) {
    fail("launch URL must be an exact loopback browser URL");
  }
}
async function writeJson(root, name, value) { await writeFile(resolve(root, name), `${JSON.stringify(value, null, 2)}\n`, { encoding: "utf8", flag: "wx", mode: 0o600 }); }
function fail(message) { throw new Error(`local Activity Client preparation failed: ${message}`); }
