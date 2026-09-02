import { createHash } from "node:crypto";
import { readdir, readFile } from "node:fs/promises";
import { relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const workspace = resolve(fileURLToPath(new URL("..", import.meta.url)));
const configuration = resolve(workspace, "config/activity-clients");
const contract = "worldstream/activity-client-protocol/v1";
const printIdentities = process.argv.includes("--print-identities");
const buildRoots = new Map([
  ["worldstream.agent-heist.web", "clients/agent-heist-web/dist"],
  ["worldstream.negotiate.web", "clients/negotiate-web/dist"],
  ["worldstream.inspector.web", "web/console/dist"],
]);
const expectedChecks = new Map([
  ["worldstream.agent-heist.web", [
    "release-distribution-and-host-binding-exact-references",
    "live-adapter-starts-empty",
    "projection-reset-replaces-authorized-state",
    "participant-and-spectator-access-mode-gating",
    "recorded-gallery-cannot-consume-live-state",
  ]],
  ["worldstream.inspector.web", [
    "release-and-host-fallback-exact-references",
    "studio-and-supervisor-pack-independence",
    "inspector-pack-independence",
    "opaque-one-use-handoff",
    "recorded-gallery-cannot-consume-live-state",
  ]],
  ["worldstream.negotiate.web", [
    "release-distribution-and-host-binding-exact-references",
    "live-adapter-starts-empty",
    "projection-reset-replaces-authorized-state",
    "participant-and-spectator-access-mode-gating",
    "prepared-action-queue-and-replay-validation-boundaries",
  ]],
]);
const ignoredDirectories = new Set([".git", ".vite", "coverage", "dist", "node_modules"]);

const releases = await readJsonDirectory(resolve(configuration, "releases"));
const distributions = await readJsonDirectory(resolve(configuration, "distributions"));
const bootstrap = await readJson(resolve(configuration, "local-bindings.json"));
const evidenceDocuments = await readJsonDirectory(resolve(configuration, "conformance"));
const qualifiedNegotiate = await readJson(resolve(workspace, "packs/negotiate/evidence/conformance-v1.json"));
const evidenceByClient = new Map();
const releaseIndex = new Map();
const expectedIdentities = {};

for (const evidence of evidenceDocuments) {
  exactKeys(evidence, ["schema", "subject", "checks", "reproduce", "authority", "notice"]);
  check(evidence.schema === "worldstream/client-conformance-evidence/v1", "unknown Client Conformance Evidence schema");
  exactKeys(evidence.subject, ["client_id", "artifact_digest", "release_claims_digest", "client_contract"]);
  check(buildRoots.has(evidence.subject.client_id), `unknown conformance subject ${evidence.subject.client_id}`);
  checkDigest(evidence.subject.artifact_digest, `${evidence.subject.client_id} conformance artifact`);
  checkDigest(evidence.subject.release_claims_digest, `${evidence.subject.client_id} conformance Release claims`);
  check(evidence.subject.client_contract === contract, `${evidence.subject.client_id} conformance contract is invalid`);
  check(
    JSON.stringify(evidence.checks) === JSON.stringify(expectedChecks.get(evidence.subject.client_id)),
    `${evidence.subject.client_id} conformance checks do not match the exercised canonical lane`,
  );
  check(evidence.reproduce === "pnpm activity-clients:verify", `${evidence.subject.client_id} conformance is not reproducible through the canonical lane`);
  check(evidence.authority === "informative_only", `${evidence.subject.client_id} conformance overstates its authority`);
  check(!evidenceByClient.has(evidence.subject.client_id), `duplicate conformance subject ${evidence.subject.client_id}`);
  evidenceByClient.set(evidence.subject.client_id, evidence);
}

for (const release of releases) {
  exactKeys(release, ["schema", "client_id", "release_digest", "client_contract", "artifacts", "surfaces", "conformance"]);
  check(release.schema === "worldstream/activity-client-release/v1", "unknown Activity Client Release schema");
  check(release.client_contract === contract, `unsupported client contract for ${release.client_id}`);
  check(buildRoots.has(release.client_id), `no conformance build root for ${release.client_id}`);
  check(Array.isArray(release.artifacts) && release.artifacts.length === 1, `${release.client_id} must have one local runnable artifact`);
  check(Array.isArray(release.surfaces) && release.surfaces.length > 0, `${release.client_id} declares no Client Surface`);
  check(Array.isArray(release.conformance) && release.conformance.length === 1, `${release.client_id} must link exact conformance evidence`);
  const artifact = release.artifacts[0];
  exactKeys(artifact, ["artifact_id", "media_type", "digest"]);
  check(artifact.media_type === "application/vnd.worldstream.activity-client.web.v1+directory", `${release.client_id} must identify a runnable browser build`);
  const artifactDigest = await buildTreeDigest(resolve(workspace, buildRoots.get(release.client_id)));
  if (!printIdentities) check(artifact.digest === artifactDigest, `${release.client_id} runnable artifact digest is stale: expected ${artifactDigest}`);
  const normalizedRelease = structuredClone(release);
  normalizedRelease.artifacts[0].digest = artifactDigest;
  for (const surface of release.surfaces) {
    exactKeys(surface, ["surface_id", "kind", "artifact_digest", "entrypoint", "capabilities"]);
    check(surface.kind === "browser", `${release.client_id}/${surface.surface_id} is not a browser surface`);
    if (!printIdentities) check(surface.artifact_digest === artifactDigest, `${release.client_id}/${surface.surface_id} does not reference its exact artifact`);
    check(/^\/[a-z0-9._/-]+\/$/.test(surface.entrypoint), `${release.client_id}/${surface.surface_id} has an invalid artifact entrypoint`);
    check(Array.isArray(surface.capabilities) && surface.capabilities.includes("observe"), `${release.client_id}/${surface.surface_id} cannot observe`);
  }
  for (const surface of normalizedRelease.surfaces) surface.artifact_digest = artifactDigest;
  const evidence = release.conformance[0];
  exactKeys(evidence, ["contract", "evidence_digest"]);
  check(evidence.contract === contract, `${release.client_id} conformance contract is invalid`);
  const releaseClaimsDigest = releaseClaimsIdentityDigest(normalizedRelease);
  const evidenceDocument = evidenceByClient.get(release.client_id);
  check(evidenceDocument !== undefined, `${release.client_id} has no conformance evidence`);
  const normalizedEvidence = structuredClone(evidenceDocument);
  normalizedEvidence.subject = {
    client_id: release.client_id,
    artifact_digest: artifactDigest,
    release_claims_digest: releaseClaimsDigest,
    client_contract: release.client_contract,
  };
  const expectedEvidenceDigest = digest(Buffer.from(JSON.stringify(normalizedEvidence, null, 2) + "\n"));
  if (!printIdentities) {
    check(JSON.stringify(evidenceDocument.subject) === JSON.stringify(normalizedEvidence.subject), `${release.client_id} conformance subject is stale`);
    check(evidence.evidence_digest === expectedEvidenceDigest, `${release.client_id} conformance evidence identity is stale`);
  }
  normalizedRelease.conformance[0].evidence_digest = expectedEvidenceDigest;
  const expectedReleaseDigest = releaseIdentityDigest(normalizedRelease);
  normalizedRelease.release_digest = expectedReleaseDigest;
  expectedIdentities[release.client_id] = {
    artifact_digest: artifactDigest,
    release_claims_digest: releaseClaimsDigest,
    release_digest: expectedReleaseDigest,
    evidence_digest: expectedEvidenceDigest,
  };
  if (!printIdentities) check(release.release_digest === expectedReleaseDigest, `${release.client_id} release digest is stale: expected ${expectedReleaseDigest}`);
  const key = `${release.client_id}\0${expectedReleaseDigest}`;
  check(!releaseIndex.has(key), `duplicate Activity Client Release ${release.client_id}`);
  releaseIndex.set(key, normalizedRelease);
}

check(releaseIndex.size === buildRoots.size, "the first-party Release set is incomplete");
for (const distribution of distributions) {
  exactKeys(distribution, ["schema", "distribution_id", "version", "pack_bundles", "clients", "client_compatibility", "integration_artifacts"]);
  check(distribution.schema === "worldstream/activity-distribution/v1", "unknown Activity Distribution schema");
  check(typeof distribution.version === "string" && distribution.version.length > 0, `${distribution.distribution_id} has no version`);
  check(Array.isArray(distribution.pack_bundles) && distribution.pack_bundles.length > 0, `${distribution.distribution_id} has no Pack Bundles`);
  const packRevisions = new Set();
  for (const reference of distribution.pack_bundles) {
    exactKeys(reference, ["pack", "bundle_digest"]);
    exactKeys(reference.pack, ["id", "version", "digest"]);
    checkDigest(reference.pack.digest, `${distribution.distribution_id} Pack Revision`);
    checkDigest(reference.bundle_digest, `${distribution.distribution_id} Pack Bundle`);
    packRevisions.add(reference.pack.digest);
    if (reference.pack.id === "worldstream.negotiate") {
      check(reference.pack.digest === qualifiedNegotiate.bundle.revision_digest, `${distribution.distribution_id} does not reference the qualified Negotiate Pack Revision`);
      check(reference.bundle_digest === qualifiedNegotiate.bundle.bundle_digest, `${distribution.distribution_id} does not reference the qualified Negotiate physical Bundle`);
    }
  }
  check(Array.isArray(distribution.clients) && distribution.clients.length > 0, `${distribution.distribution_id} has no client references`);
  for (const reference of distribution.clients) {
    exactKeys(reference, ["client_id", "release_digest"]);
    if (!printIdentities) check(releaseIndex.has(`${reference.client_id}\0${reference.release_digest}`), `${distribution.distribution_id} references an unavailable exact Release`);
  }
  check(Array.isArray(distribution.client_compatibility) && distribution.client_compatibility.length > 0, `${distribution.distribution_id} has no client compatibility claims`);
  for (const claim of distribution.client_compatibility) {
    exactKeys(claim, ["client_id", "release_digest", "surface_id", "pack_revision_digest", "access_mode", "roles"]);
    check(packRevisions.has(claim.pack_revision_digest), `${distribution.distribution_id} compatibility references an unavailable Pack Revision`);
    check(claim.access_mode === "participant" ? claim.roles.length > 0 : ["spectator", "operator"].includes(claim.access_mode) && claim.roles.length === 0, `${distribution.distribution_id} has an invalid compatibility Access Mode/Role set`);
    if (!printIdentities) check(releaseIndex.has(`${claim.client_id}\0${claim.release_digest}`), `${distribution.distribution_id} compatibility references an unavailable Release`);
  }
  check(Array.isArray(distribution.integration_artifacts), `${distribution.distribution_id} integration artifacts are invalid`);
  for (const artifact of distribution.integration_artifacts) {
    exactKeys(artifact, ["artifact_id", "kind", "media_type", "digest"]);
    check(["runner_integration", "documentation", "deployment_template"].includes(artifact.kind), `${distribution.distribution_id} has an unknown integration artifact kind`);
    checkDigest(artifact.digest, `${distribution.distribution_id}/${artifact.artifact_id}`);
  }
}

exactKeys(bootstrap, ["schema", "deployment_trust_policy", "deployments", "bindings", "inspector_fallback"]);
check(bootstrap.schema === "worldstream/client-binding-bootstrap/v1", "unknown Client Binding bootstrap schema");
check(["verified_only", "allow_externally_trusted"].includes(bootstrap.deployment_trust_policy), "unknown client Deployment trust policy");
const deployments = new Map();
for (const deployment of bootstrap.deployments) {
  exactKeys(deployment, ["schema", "deployment_id", "client_id", "release_digest", "trust_level", "surfaces"]);
  check(deployment.schema === "worldstream/client-deployment/v1", "unknown Client Deployment schema");
  check(deployment.trust_level === "externally_trusted", `${deployment.deployment_id} cannot claim verified bytes in the local Vite workflow`);
  const release = releaseIndex.get(`${deployment.client_id}\0${deployment.release_digest}`);
  if (!printIdentities) check(release !== undefined, `${deployment.deployment_id} references an unavailable Release`);
  const declared = new Set((release?.surfaces ?? []).map((surface) => surface.surface_id));
  for (const surface of deployment.surfaces) {
    exactKeys(surface, ["surface_id", "launch_url"]);
    if (!printIdentities) check(declared.has(surface.surface_id), `${deployment.deployment_id} exposes an undeclared surface`);
    check(/^http:\/\/(?:127\.0\.0\.1|localhost|\[::1\])(?::[1-9][0-9]{0,4})?\/[a-z0-9._/-]+\/$/.test(surface.launch_url), `${deployment.deployment_id} has an unsafe local launch URL`);
  }
  check(!deployments.has(deployment.deployment_id), `duplicate deployment ${deployment.deployment_id}`);
  deployments.set(deployment.deployment_id, deployment);
}

for (const binding of bootstrap.bindings) {
  exactKeys(binding, ["schema", "binding_id", "pack", "client_contract", "access_mode", "roles", "deployment_id", "surface_id", "preference"]);
  check(binding.schema === "worldstream/client-binding/v1" && binding.client_contract === contract, `invalid binding ${binding.binding_id}`);
  checkDigest(binding.pack.digest, `${binding.binding_id} Pack Revision`);
  const deployment = deployments.get(binding.deployment_id);
  check(deployment !== undefined && deployment.surfaces.some((surface) => surface.surface_id === binding.surface_id), `${binding.binding_id} targets an unavailable deployment surface`);
  check(binding.access_mode === "participant" ? binding.roles.length > 0 : binding.access_mode === "spectator" && binding.roles.length === 0, `${binding.binding_id} has an invalid Access Mode/Role set`);
}

exactKeys(bootstrap.inspector_fallback, ["schema", "fallback_id", "deployment_id", "surface_id"]);
check(bootstrap.inspector_fallback.schema === "worldstream/inspector-fallback/v1", "unknown Inspector fallback schema");
const fallbackDeployment = deployments.get(bootstrap.inspector_fallback.deployment_id);
check(fallbackDeployment?.client_id === "worldstream.inspector.web", "Inspector fallback is not an approved Inspector deployment");
check(fallbackDeployment.surfaces.some((surface) => surface.surface_id === bootstrap.inspector_fallback.surface_id), "Inspector fallback surface is unavailable");

await verifySourceBoundaries();

if (printIdentities) {
  console.log(JSON.stringify(expectedIdentities, null, 2));
} else {
  console.log(`Activity Client conformance passed for ${releases.length} Releases, ${distributions.length} Distributions, ${bootstrap.deployments.length} Deployments, and ${bootstrap.bindings.length} Bindings.`);
  console.log(`Evidence: ${evidenceDocuments.length} exact Release-claims-and-artifact-bound records (informative only; not Host approval or deployment proof)`);
}

async function verifySourceBoundaries() {
  const studioFiles = (await walk(resolve(workspace, "web/studio/src")))
    .filter((path) => /\.(?:ts|tsx)$/.test(path) && !/\.test\.(?:ts|tsx)$/.test(path));
  const forbiddenStudio = /worldstream\.(?:agent-heist|negotiate)|\/(?:agent-heist|negotiate|inspector)\/|Counter\b|counter\/public-projection/i;
  for (const path of studioFiles) {
    check(!forbiddenStudio.test(await readFile(path, "utf8")), `Studio contains Pack/client implementation knowledge in ${relative(workspace, path)}`);
  }
  for (const relativePath of [
    "crates/worldstream-studio-supervisor/src/client_bindings.rs",
    "crates/worldstream-studio-supervisor/src/participant_handoff.rs",
  ]) {
    const source = await readFile(resolve(workspace, relativePath), "utf8");
    check(!/worldstream\.(?:agent-heist|negotiate)|\/(?:agent-heist|negotiate|inspector)\//i.test(source), `Supervisor contains a Pack/client route branch in ${relativePath}`);
  }
  const inspector = await readFile(resolve(workspace, "web/console/src/HandedOffParticipant.tsx"), "utf8");
  check(!/Negotiate|worldstream\.negotiate|agent-heist/i.test(inspector), "generic Inspector imports a Pack-specific renderer");
  const gallery = await readFile(resolve(workspace, "web/console/src/App.tsx"), "utf8");
  check(!/useLiveSession|liveSession|liveTransport|liveReplayClient/.test(gallery), "recorded gallery can still overlay live authorized state");
  const clientHostMain = await readFile(resolve(workspace, "web/console/src/main.tsx"), "utf8");
  check(
    !/@worldstream\/(?:agent-heist|negotiate)-client|clientSurface === "(?:agent-heist|negotiate)"|NegotiateLiveApp|consumeNegotiateConsoleBootstrap|consumeLiveSessionBootstrap/.test(clientHostMain),
    "Inspector/recorded-gallery artifact still embeds a Pack-specific live client",
  );
  const exactHost = await readFile(resolve(workspace, "scripts/serve-activity-clients.mjs"), "utf8");
  for (const [prefix, root] of [
    ["/agent-heist/", "clients/agent-heist-web/dist"],
    ["/negotiate/", "clients/negotiate-web/dist"],
    ["/", "web/console/dist"],
  ]) {
    check(exactHost.includes(`prefix: "${prefix}"`) && exactHost.includes(`"${root}"`), `exact Activity Client Host is missing ${prefix} -> ${root}`);
  }
  for (const path of [
    "clients/agent-heist-web/src/liveAdapter.ts",
    "clients/negotiate-web/src/liveAdapter.ts",
  ]) {
    const source = await readFile(resolve(workspace, path), "utf8");
    check(/return \{ kind: "awaiting" \};/.test(source), `${path} does not start from empty authorized state`);
  }
}

function releaseIdentityDigest(release) {
  const identity = {
    schema: release.schema,
    client_id: release.client_id,
    client_contract: release.client_contract,
    artifacts: release.artifacts,
    surfaces: release.surfaces,
    conformance: release.conformance,
  };
  return digest(Buffer.from(`worldstream-activity-client-release-identity-v1\0${JSON.stringify(identity)}`));
}

function releaseClaimsIdentityDigest(release) {
  const claims = {
    schema: release.schema,
    client_id: release.client_id,
    client_contract: release.client_contract,
    artifacts: release.artifacts,
    surfaces: release.surfaces,
  };
  return digest(Buffer.from(`worldstream-activity-client-release-claims-v1\0${JSON.stringify(claims)}`));
}

async function buildTreeDigest(root) {
  const hash = createHash("sha256");
  hash.update("worldstream-activity-client-build-tree-v1\0");
  for (const path of await walk(root)) {
    const relativePath = relative(root, path).split(sep).join("/");
    hash.update(relativePath);
    hash.update("\0");
    hash.update(createHash("sha256").update(await readFile(path)).digest());
    hash.update("\0");
  }
  return `sha256:${hash.digest("hex")}`;
}

async function walk(root) {
  const entries = await readdir(root, { withFileTypes: true });
  const files = [];
  for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name))) {
    if (entry.isDirectory() && ignoredDirectories.has(entry.name)) continue;
    const path = resolve(root, entry.name);
    if (entry.isDirectory()) files.push(...await walk(path));
    else if (entry.isFile()) files.push(path);
  }
  return files.sort();
}

async function readJsonDirectory(path) {
  const entries = (await readdir(path, { withFileTypes: true }))
    .filter((entry) => entry.isFile() && entry.name.endsWith(".json"))
    .sort((left, right) => left.name.localeCompare(right.name));
  return Promise.all(entries.map((entry) => readJson(resolve(path, entry.name))));
}

async function readJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}

function exactKeys(value, keys) {
  check(value !== null && typeof value === "object" && !Array.isArray(value), "expected an object");
  check(Object.keys(value).sort().join("\0") === [...keys].sort().join("\0"), `unexpected fields: ${Object.keys(value).join(", ")}`);
}

function checkDigest(value, label) {
  check(typeof value === "string" && /^(?:blake3|sha256):[0-9a-f]{64}$/.test(value), `${label} is not an exact digest`);
}

function digest(bytes) {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

function check(condition, message) {
  if (!condition) throw new Error(message);
}
