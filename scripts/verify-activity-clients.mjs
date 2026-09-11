import { createHash } from "node:crypto";
import { readdir, readFile, stat } from "node:fs/promises";
import { relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { activityClientBuildDigest, activityClientReleaseClaimsDigest, activityClientReleaseDigest } from "./activity-client-identities.mjs";
import { activityClientMounts } from "./serve-activity-clients.mjs";

const workspace = resolve(fileURLToPath(new URL("..", import.meta.url)));
const configuration = resolve(workspace, "config/activity-clients");
const contract = "worldstream/activity-client-protocol/v1";
const printIdentities = process.argv.includes("--print-identities");
const buildRoots = new Map([
  ["worldstream.agent-heist.web", "clients/agent-heist-web/dist"],
  ["worldstream.negotiate.web", "clients/negotiate-web/dist"],
  ["worldstream.midnight-archive.web", "clients/midnight-archive-web/dist"],
  ["worldstream.inspector.web", "web/console/dist"],
]);
const currentReleaseFiles = new Map([
  ["worldstream.agent-heist.web", "agent-heist-web-v12.json"],
  ["worldstream.negotiate.web", "negotiate-web-v3.json"],
  ["worldstream.midnight-archive.web", "midnight-archive-web-v14.json"],
  ["worldstream.inspector.web", "inspector-web-v2.json"],
]);
const currentEvidenceFiles = new Map([
  ["worldstream.agent-heist.web", "agent-heist-web-v12.json"],
  ["worldstream.negotiate.web", "negotiate-web-v3.json"],
  ["worldstream.midnight-archive.web", "midnight-archive-web-v14.json"],
  ["worldstream.inspector.web", "inspector-web-v2.json"],
]);
const expectedChecks = new Map([
  ["worldstream.agent-heist.web", [
    "release-distribution-and-host-binding-exact-references",
    "live-adapter-starts-empty",
    "projection-reset-replaces-authorized-state",
    "participant-and-spectator-access-mode-gating",
    "webmcp-authorized-read-wait-and-semantic-action-boundaries",
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
  ["worldstream.midnight-archive.web", [
    "release-and-host-binding-exact-references",
    "live-adapter-starts-empty",
    "briefing-reset-and-activity-start-gating",
    "projection-reset-replaces-authorized-state",
    "participant-access-and-private-truth-gating",
    "staged-action-and-explicit-commit-boundaries",
    "responsive-map-and-contextual-action-controls",
  ]],
]);
const hostMounts = new Map(activityClientMounts.map((mount) => [mount.prefix, mount.root]));
const ignoredDirectories = new Set([".git", ".vite", "coverage", "dist", "node_modules"]);

const releaseDocuments = await readNamedJsonDirectory(resolve(configuration, "releases"));
const releases = releaseDocuments.map(({ value }) => value);
const distributions = await readJsonDirectory(resolve(configuration, "distributions"));
const bootstrap = await readJson(resolve(configuration, "local-bindings.json"));
const evidenceDocuments = await readNamedJsonDirectory(resolve(configuration, "conformance"));
const qualifiedNegotiate0_1 = await readJson(resolve(workspace, "packs/negotiate/evidence/conformance-v1.json"));
const provedNegotiate0_2 = await readJson(resolve(workspace, "packs/negotiate/evidence/production-proof-0.2.0.json"));
const negotiatePackProofs = new Map([
  [qualifiedNegotiate0_1.bundle.revision_digest, {
    version: qualifiedNegotiate0_1.version,
    proof: qualifiedNegotiate0_1.production_proof.complete,
  }],
  [provedNegotiate0_2.revision_digest, { version: "0.2.0", proof: provedNegotiate0_2 }],
]);
const evidenceByDigest = new Map();
const releaseIndex = new Map();
const declaredReleaseIndex = new Map();
const expectedIdentities = {};

for (const { proof } of negotiatePackProofs.values()) validateNegotiatePackProof(proof);

for (const { name, value: evidence } of evidenceDocuments) {
  exactKeys(evidence, ["schema", "subject", "checks", "reproduce", "authority", "notice"]);
  check(evidence.schema === "worldstream/client-conformance-evidence/v1", "unknown Client Conformance Evidence schema");
  exactKeys(evidence.subject, ["client_id", "artifact_digest", "release_claims_digest", "client_contract"]);
  check(buildRoots.has(evidence.subject.client_id), `unknown conformance subject ${evidence.subject.client_id}`);
  checkDigest(evidence.subject.artifact_digest, `${evidence.subject.client_id} conformance artifact`);
  checkDigest(evidence.subject.release_claims_digest, `${evidence.subject.client_id} conformance Release claims`);
  check(evidence.subject.client_contract === contract, `${evidence.subject.client_id} conformance contract is invalid`);
  check(
    JSON.stringify(evidence.checks) === JSON.stringify([
      ...expectedChecks.get(evidence.subject.client_id),
      ...(evidence.subject.client_id === "worldstream.midnight-archive.web" ? (
        ["midnight-archive-web-v5.json", "midnight-archive-web-v6.json", "midnight-archive-web-v7.json", "midnight-archive-web-v8.json", "midnight-archive-web-v9.json", "midnight-archive-web-v10.json", "midnight-archive-web-v11.json", "midnight-archive-web-v12.json", "midnight-archive-web-v13.json", "midnight-archive-web-v14.json"].includes(name) ? [
          "fixed-agreement-and-optional-objective-projection-boundaries",
          "all-four-starting-roster-component-host-witnesses",
          "structured-specialist-task-plan-and-private-knowledge-boundaries",
          "separate-human-mira-and-jonah-membership-live-browser-witness",
          "one-specialist-step-per-human-commit-and-provider-free-replay",
          "typed-turn-reservation-and-conflict-boundaries",
          "staged-extraction-preview-and-exact-crew-acknowledgement",
          "terminal-full-and-partial-crew-debrief-work-attribution",
          "responsive-specialist-controls",
          ...(["midnight-archive-web-v10.json", "midnight-archive-web-v11.json", "midnight-archive-web-v12.json", "midnight-archive-web-v13.json", "midnight-archive-web-v14.json"].includes(name) ? [
            "authored-standard-and-low-reserve-scenario-boundaries",
            "closed-genesis-operation-cost-schedule",
          ] : []),
        ] : name === "midnight-archive-web-v4.json" ? [
          "fixed-agreement-and-optional-objective-projection-boundaries",
          "solo-eleven-and-fifteen-turn-component-host-browser-and-replay-witnesses",
          "structured-mira-task-plan-and-private-knowledge-boundaries",
          "separate-human-and-agent-membership-live-browser-witness",
          "one-companion-step-per-human-commit-and-provider-free-replay",
          "responsive-mira-controls",
        ] : name === "midnight-archive-web-v3.json" ? [
          "fixed-agreement-and-optional-objective-projection-boundaries",
          "solo-eleven-and-fifteen-turn-component-host-browser-and-replay-witnesses",
        ] : ["solo-ten-turn-component-host-browser-and-replay-witness"]
      ) : []),
      ...(["agent-heist-web-v2.json", "agent-heist-web-v3.json", "agent-heist-web-v4.json", "agent-heist-web-v5.json", "agent-heist-web-v6.json", "agent-heist-web-v7.json", "agent-heist-web-v8.json", "agent-heist-web-v9.json", "agent-heist-web-v10.json", "agent-heist-web-v11.json", "agent-heist-web-v12.json"].includes(name) ? [
        "deployment-owned-stream-bootstrap-and-recovery",
        "separate-local-kernel-and-hosted-entrypoints-without-auth-fallback",
      ] : []),
      ...(["midnight-archive-web-v2.json", "midnight-archive-web-v3.json", "midnight-archive-web-v4.json", "midnight-archive-web-v5.json", "midnight-archive-web-v6.json", "midnight-archive-web-v7.json", "midnight-archive-web-v8.json", "midnight-archive-web-v9.json", "midnight-archive-web-v10.json", "midnight-archive-web-v11.json", "midnight-archive-web-v12.json", "midnight-archive-web-v13.json", "midnight-archive-web-v14.json"].includes(name)
        ? ["bounded-idempotent-upstream-retry"] : []),
      ...(["midnight-archive-web-v7.json", "midnight-archive-web-v8.json", "midnight-archive-web-v9.json", "midnight-archive-web-v10.json", "midnight-archive-web-v11.json", "midnight-archive-web-v12.json", "midnight-archive-web-v13.json", "midnight-archive-web-v14.json"].includes(name)
        ? ["unavailable-companion-plan-continuation-boundaries"] : []),
      ...(["midnight-archive-web-v11.json", "midnight-archive-web-v12.json", "midnight-archive-web-v13.json", "midnight-archive-web-v14.json"].includes(name) ? ["bounded-companion-dialogue-literal-rendering-and-schema-boundaries"] : []),
      ...(["midnight-archive-web-v12.json", "midnight-archive-web-v13.json", "midnight-archive-web-v14.json"].includes(name) ? ["recorded-session-expiry-and-terminal-replay-boundaries"] : []),
      ...(["midnight-archive-web-v13.json", "midnight-archive-web-v14.json"].includes(name) ? ["hosted-live-and-terminal-house-exhibition-disclosure"] : []),
      ...(["agent-heist-web-v11.json", "agent-heist-web-v12.json", "midnight-archive-web-v14.json"].includes(name) ? ["retryable-room-busy-synchronization-within-owning-attempt-deadline"] : []),
      ...(name === "agent-heist-web-v12.json" ? ["participant-private-commitment-receipt-and-deadline-bounded-stale-recovery"] : []),
    ]),
    `${evidence.subject.client_id} conformance checks do not match the exercised canonical lane`,
  );
  check(evidence.reproduce === "pnpm activity-clients:verify", `${evidence.subject.client_id} conformance is not reproducible through the canonical lane`);
  check(evidence.authority === "informative_only", `${evidence.subject.client_id} conformance overstates its authority`);
  const evidenceDigest = digest(Buffer.from(JSON.stringify(evidence, null, 2) + "\n"));
  check(!evidenceByDigest.has(evidenceDigest), `duplicate conformance evidence ${evidenceDigest}`);
  evidenceByDigest.set(evidenceDigest, { name, evidence });
}

for (const { name, value: release } of releaseDocuments) {
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
  const current = currentReleaseFiles.get(release.client_id) === name;
  const artifactDigest = current
    ? await activityClientBuildDigest(resolve(workspace, buildRoots.get(release.client_id)))
    : artifact.digest;
  if (current && !printIdentities) check(artifact.digest === artifactDigest, `${release.client_id} runnable artifact digest is stale: expected ${artifactDigest}`);
  const normalizedRelease = structuredClone(release);
  normalizedRelease.artifacts[0].digest = artifactDigest;
  for (const surface of release.surfaces) {
    exactKeys(surface, ["surface_id", "kind", "artifact_digest", "entrypoint", "capabilities"]);
    check(surface.kind === "browser", `${release.client_id}/${surface.surface_id} is not a browser surface`);
    if (!printIdentities || !current) check(surface.artifact_digest === artifactDigest, `${release.client_id}/${surface.surface_id} does not reference its exact artifact`);
    check(/^\/[a-z0-9._/-]+\/$/.test(surface.entrypoint), `${release.client_id}/${surface.surface_id} has an invalid artifact entrypoint`);
    check(Array.isArray(surface.capabilities) && surface.capabilities.includes("observe"), `${release.client_id}/${surface.surface_id} cannot observe`);
  }
  for (const surface of normalizedRelease.surfaces) surface.artifact_digest = artifactDigest;
  const evidence = release.conformance[0];
  exactKeys(evidence, ["contract", "evidence_digest"]);
  check(evidence.contract === contract, `${release.client_id} conformance contract is invalid`);
  const releaseClaimsDigest = activityClientReleaseClaimsDigest(normalizedRelease);
  const evidenceRecord = current
    ? evidenceDocuments.find(({ name: evidenceName }) => evidenceName === currentEvidenceFiles.get(release.client_id))
    : evidenceByDigest.get(evidence.evidence_digest);
  const evidenceDocument = evidenceRecord?.evidence ?? evidenceRecord?.value;
  check(evidenceDocument !== undefined, `${release.client_id}/${name} has no exact conformance evidence`);
  const normalizedEvidence = structuredClone(evidenceDocument);
  normalizedEvidence.subject = {
    client_id: release.client_id,
    artifact_digest: artifactDigest,
    release_claims_digest: releaseClaimsDigest,
    client_contract: release.client_contract,
  };
  const expectedEvidenceDigest = digest(Buffer.from(JSON.stringify(normalizedEvidence, null, 2) + "\n"));
  if (!printIdentities || !current) {
    check(JSON.stringify(evidenceDocument.subject) === JSON.stringify(normalizedEvidence.subject), `${release.client_id} conformance subject is stale`);
    check(evidence.evidence_digest === expectedEvidenceDigest, `${release.client_id} conformance evidence identity is stale`);
  }
  normalizedRelease.conformance[0].evidence_digest = expectedEvidenceDigest;
  const expectedReleaseDigest = activityClientReleaseDigest(normalizedRelease);
  normalizedRelease.release_digest = expectedReleaseDigest;
  if (current) {
    expectedIdentities[release.client_id] = {
      artifact_digest: artifactDigest,
      release_claims_digest: releaseClaimsDigest,
      release_digest: expectedReleaseDigest,
      evidence_digest: expectedEvidenceDigest,
    };
  }
  if (!printIdentities || !current) check(release.release_digest === expectedReleaseDigest, `${release.client_id}/${name} release digest is stale: expected ${expectedReleaseDigest}`);
  const key = `${release.client_id}\0${expectedReleaseDigest}`;
  const declaredKey = `${release.client_id}\0${release.release_digest}`;
  check(!releaseIndex.has(key), `duplicate Activity Client Release ${release.client_id}`);
  check(!declaredReleaseIndex.has(declaredKey), `duplicate declared Activity Client Release ${release.client_id}`);
  releaseIndex.set(key, normalizedRelease);
  declaredReleaseIndex.set(declaredKey, normalizedRelease);
}

check(Object.keys(expectedIdentities).length === buildRoots.size, "the current first-party Release set is incomplete");
// A fresh import sees only its declared Release files, not the whole repository
// directory. Check that closure independently of the artifact inventory above.
for (const declarationName of ["cli-import.json", "hosted-local-import.json"]) {
  const declaration = await readJson(resolve(configuration, declarationName));
  exactKeys(declaration, ["schema", "release_files", "bindings_file"]);
  check(declaration.schema === "worldstream/client-declaration-import/v1", `${declarationName} has an unknown import schema`);
  const imported = new Map();
  for (const releaseFile of declaration.release_files) {
    const release = await readJson(resolve(configuration, releaseFile));
    const key = `${release.client_id}\0${release.release_digest}`;
    check(declaredReleaseIndex.has(key), `${declarationName} imports an unavailable exact Release`);
    check(!imported.has(key), `${declarationName} imports a duplicate Release`);
    imported.set(key, release);
  }
  const declaredBindings = await readJson(resolve(configuration, declaration.bindings_file));
  for (const deployment of declaredBindings.deployments) {
    const release = imported.get(`${deployment.client_id}\0${deployment.release_digest}`);
    check(release !== undefined, `${declarationName}: ${deployment.deployment_id} requires a Release absent from release_files`);
    for (const surface of deployment.surfaces) {
      check(release.surfaces.some((candidate) => candidate.surface_id === surface.surface_id
        && candidate.entrypoint === new URL(surface.launch_url).pathname),
      `${declarationName}: ${deployment.deployment_id} exposes a surface outside its imported Release`);
    }
  }
}
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
      const proved = negotiatePackProofs.get(reference.pack.digest);
      check(proved !== undefined && reference.pack.version === proved.version, `${distribution.distribution_id} does not reference a proved exact Negotiate Pack Revision`);
      check(proved.proof.proof_type === "complete" && proved.proof.status === "passed", `${distribution.distribution_id} Negotiate Pack proof is incomplete`);
      check(reference.bundle_digest === proved.proof.bundle_digest, `${distribution.distribution_id} does not reference the proved Negotiate physical Bundle`);
      const bundlePath = resolve(
        workspace,
        `packs/negotiate/releases/${reference.pack.version}/worldstream-negotiate-${reference.bundle_digest.slice("blake3:".length)}.wspack`,
      );
      check((await stat(bundlePath)).isFile(), `${distribution.distribution_id} proved Negotiate physical Bundle is unavailable`);
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
  const releaseKey = `${deployment.client_id}\0${deployment.release_digest}`;
  const release = releaseIndex.get(releaseKey)
    ?? (printIdentities ? declaredReleaseIndex.get(releaseKey) : undefined);
  if (!printIdentities) check(release !== undefined, `${deployment.deployment_id} references an unavailable Release`);
  const declared = new Set((release?.surfaces ?? []).map((surface) => surface.surface_id));
  for (const surface of deployment.surfaces) {
    exactKeys(surface, ["surface_id", "launch_url"]);
    if (!printIdentities) check(declared.has(surface.surface_id), `${deployment.deployment_id} exposes an undeclared surface`);
    check(/^http:\/\/(?:127\.0\.0\.1|localhost|\[::1\])(?::[1-9][0-9]{0,4})?\/[a-z0-9._/-]+\/$/.test(surface.launch_url), `${deployment.deployment_id} has an unsafe local launch URL`);
    const entrypoint = new URL(surface.launch_url).pathname;
    const artifactRoot = activityClientMounts.find((mount) => entrypoint.startsWith(mount.prefix))?.root;
    check(artifactRoot !== undefined, `${deployment.deployment_id}/${surface.surface_id} has no exact Activity Client Host mount`);
    const releaseSurface = release?.surfaces.find((candidate) => candidate.surface_id === surface.surface_id);
    check(releaseSurface?.entrypoint === entrypoint, `${deployment.deployment_id}/${surface.surface_id} launch target differs from its Release entrypoint`);
    const servedArtifactDigest = await activityClientBuildDigest(artifactRoot);
    check(releaseSurface?.artifact_digest === servedArtifactDigest, `${deployment.deployment_id}/${surface.surface_id} serves bytes outside its declared Release artifact`);
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
  for (const relativePath of [
    "crates/worldstream-studio-supervisor/src/client_bindings.rs",
    "crates/worldstream-studio-supervisor/src/participant_handoff.rs",
  ]) {
    const source = await readFile(resolve(workspace, relativePath), "utf8");
    check(!/worldstream\.(?:agent-heist|negotiate|midnight-archive)|\/(?:agent-heist|negotiate|midnight-archive|inspector)\//i.test(source), `Supervisor contains a Pack/client route branch in ${relativePath}`);
  }
  const inspector = await readFile(resolve(workspace, "web/console/src/HandedOffParticipant.tsx"), "utf8");
  check(!/Negotiate|Midnight Archive|worldstream\.(?:negotiate|midnight-archive)|agent-heist/i.test(inspector), "generic Inspector imports a Pack-specific renderer");
  const gallery = await readFile(resolve(workspace, "web/console/src/App.tsx"), "utf8");
  check(!/useLiveSession|liveSession|liveTransport|liveReplayClient/.test(gallery), "recorded gallery can still overlay live authorized state");
  const clientHostMain = await readFile(resolve(workspace, "web/console/src/main.tsx"), "utf8");
  check(
    !/@worldstream\/(?:agent-heist|negotiate|midnight-archive)-client|clientSurface === "(?:agent-heist|negotiate|midnight-archive)"|NegotiateLiveApp|MidnightArchiveLiveApp|consumeNegotiateConsoleBootstrap|consumeLiveSessionBootstrap/.test(clientHostMain),
    "Inspector/recorded-gallery artifact still embeds a Pack-specific live client",
  );
  const heistWebMcp = await readFile(resolve(workspace, "clients/agent-heist-web/src/webmcp.ts"), "utf8");
  check(
    /heist_read_state/.test(heistWebMcp)
      && /heist_wait_for_update/.test(heistWebMcp)
      && /heist_commit_plan/.test(heistWebMcp)
      && /MAX_TOOL_RESULT_CHARACTERS\s*=\s*1_500/.test(heistWebMcp),
    "Agent Heist WebMCP surface is missing its bounded reviewed tools",
  );
  for (const [prefix, root] of [
    ["/agent-heist-v12/", "clients/agent-heist-web/dist"],
    ["/agent-heist-v11/", "config/activity-clients/artifacts/agent-heist-web-v11"],
    ["/agent-heist-v10/", "config/activity-clients/artifacts/agent-heist-web-v10"],
    ["/midnight-archive-v14/", "clients/midnight-archive-web/dist"],
    ["/midnight-archive-v13/", "config/activity-clients/artifacts/midnight-archive-web-v13"],
    ["/agent-heist-v9/", "config/activity-clients/artifacts/agent-heist-web-v9"],
    ["/agent-heist-v8/", "config/activity-clients/artifacts/agent-heist-web-v8"],
    ["/agent-heist-v7/", "config/activity-clients/artifacts/agent-heist-web-v7"],
    ["/agent-heist-v6/", "config/activity-clients/artifacts/agent-heist-web-v6"],
    ["/agent-heist-v5/", "config/activity-clients/artifacts/agent-heist-web-v5"],
    ["/agent-heist-v4/", "config/activity-clients/artifacts/agent-heist-web-v4"],
    ["/agent-heist-v3/", "config/activity-clients/artifacts/agent-heist-web-v3"],
    ["/agent-heist-v2/", "config/activity-clients/artifacts/agent-heist-web-v2"],
    ["/negotiate-v3/", "clients/negotiate-web/dist"],
    ["/negotiate-v2/", "config/activity-clients/artifacts/negotiate-web-v2"],
    ["/negotiate/", "config/activity-clients/artifacts/negotiate-web-v1"],
    ["/midnight-archive-v10/", "config/activity-clients/artifacts/midnight-archive-web-v10"],
    ["/midnight-archive-v9/", "config/activity-clients/artifacts/midnight-archive-web-v9"],
    ["/midnight-archive-v8/", "config/activity-clients/artifacts/midnight-archive-web-v8"],
    ["/midnight-archive-v7/", "config/activity-clients/artifacts/midnight-archive-web-v7"],
    ["/midnight-archive-v6/", "config/activity-clients/artifacts/midnight-archive-web-v6"],
    ["/midnight-archive-v5/", "config/activity-clients/artifacts/midnight-archive-web-v5"],
    ["/midnight-archive-v4/", "config/activity-clients/artifacts/midnight-archive-web-v4"],
    ["/midnight-archive-v3/", "config/activity-clients/artifacts/midnight-archive-web-v3"],
    ["/midnight-archive-v2/", "config/activity-clients/artifacts/midnight-archive-web-v2"],
    ["/midnight-archive-v1/", "config/activity-clients/artifacts/midnight-archive-web-v1"],
    ["/inspector-v2/", "web/console/dist"],
    ["/inspector/", "config/activity-clients/artifacts/inspector-web-v1"],
    ["/", "config/activity-clients/artifacts/inspector-web-v1"],
  ]) {
    const mounted = hostMounts.get(prefix);
    check(mounted !== undefined && relative(workspace, mounted) === root, `exact Activity Client Host is missing ${prefix} -> ${root}`);
  }
  for (const path of [
    "clients/agent-heist-web/src/liveAdapter.ts",
    "clients/negotiate-web/src/liveAdapter.ts",
    "clients/midnight-archive-web/src/liveAdapter.ts",
  ]) {
    const source = await readFile(resolve(workspace, path), "utf8");
    check(/return \{ kind: "awaiting" \};/.test(source), `${path} does not start from empty authorized state`);
  }
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
  return (await readNamedJsonDirectory(path)).map(({ value }) => value);
}

async function readNamedJsonDirectory(path) {
  const entries = (await readdir(path, { withFileTypes: true }))
    .filter((entry) => entry.isFile() && entry.name.endsWith(".json"))
    .sort((left, right) => left.name.localeCompare(right.name));
  return Promise.all(entries.map(async (entry) => ({
    name: entry.name,
    value: await readJson(resolve(path, entry.name)),
  })));
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

function validateNegotiatePackProof(proof) {
  exactKeys(proof, [
    "proof_type", "status", "pack_id", "bundle_digest", "revision_digest",
    "transcript_digest", "accepted_action", "declared_rejection", "private_views",
    "retained_old_revision", "room_id", "roles",
  ]);
  check(
    proof.proof_type === "complete"
      && proof.status === "passed"
      && proof.pack_id === "worldstream.negotiate"
      && proof.accepted_action === true
      && proof.declared_rejection === true
      && proof.private_views === 4
      && proof.retained_old_revision === true
      && /^[0-9A-HJKMNP-TV-Z]{26}$/.test(proof.room_id)
      && proof.roles === 4,
    "Negotiate production Pack proof does not satisfy the complete proof contract",
  );
  checkDigest(proof.bundle_digest, "Negotiate proved Pack Bundle");
  checkDigest(proof.revision_digest, "Negotiate proved Pack Revision");
  checkDigest(proof.transcript_digest, "Negotiate proved transcript");
}

function digest(bytes) {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

function check(condition, message) {
  if (!condition) throw new Error(message);
}
