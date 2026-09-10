import { execFile } from "node:child_process";
import { lstat, readFile, readdir } from "node:fs/promises";
import { resolve, join, relative } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";
import { activityClientBuildDigest, activityClientReleaseDigest } from "./activity-client-identities.mjs";

const execute = promisify(execFile);
const repository = resolve(import.meta.dirname, "..");
const manifestPath = resolve(repository, "config/hosted/internal-candidates.json");

export async function readInternalCandidates() {
  const manifest = await readJson(manifestPath);
  if (manifest.schema !== "worldstream/local-hosted-candidates/v1" || !Array.isArray(manifest.candidates)
      || manifest.candidates.length > 8) throw new Error("invalid_internal_candidates");
  const { encodeCanonical, taggedBlake3 } = await import(pathToFileURL(resolve(repository, "sdk/typescript-pack/packages/pack-sdk/dist/index.js")));
  const { readListingRevision } = await import(pathToFileURL(resolve(repository, "sdk/typescript-hosted-contract/dist/index.js")));
  return Promise.all(manifest.candidates.map(async (entry) => {
    const listing = readListingRevision(encodeCanonical(await readJson(repositoryPath(entry.listing_file))));
    const release = await readJson(repositoryPath(entry.client_release_file));
    if (listing.digest !== entry.listing_digest || listing.value.catalog.visibility === "public"
        || activityClientReleaseDigest(release) !== listing.value.client.release_digest
        || release.client_id !== listing.value.client.client_id
        || taggedBlake3(await readFile(repositoryPath(entry.bundle_file))) !== entry.bundle_digest) {
      throw new Error("internal_candidate_identity_mismatch");
    }
    const surface = release.surfaces.find(({ surface_id }) => surface_id === listing.value.client.surface_id);
    if (surface === undefined || !surface.entrypoint.startsWith("/") || surface.entrypoint.includes("..")) {
      throw new Error("internal_candidate_surface_unavailable");
    }
    return { ...entry, listing, release, surface };
  }));
}

/** Local-only read seam. It grants no Host approval and never mutates a Room. */
export async function internalCandidateAvailable(options) {
  try {
    const candidate = (await readInternalCandidates()).find((entry) => entry.listing.digest === options.listingDigest);
    if (candidate === undefined) return false;
    const { stdout } = await execute(options.ctl, ["--config", options.config, "pack", "list",
      "--state-dir", options.stateDirectory, "--controller", options.controller, "--json"], {
      timeout: 10_000, maxBuffer: 1_048_576,
    });
    const packs = JSON.parse(stdout).packs;
    const exactPack = (entry) => entry.pack_id === candidate.listing.value.pack.id
      && entry.explanatory_version === candidate.listing.value.pack.version
      && entry.bundle_digest === candidate.bundle_digest
      && entry.revision_digest === candidate.listing.value.pack.digest && entry.install_state === "selectable";
    if (packs?.running?.availability !== "available" || packs.installed?.availability !== "available"
        || packs.pending_changes !== false || !packs.running.facts.installed.some(exactPack)
        || !packs.installed.entries.some((entry) => entry.bundle_digest === candidate.bundle_digest
          && entry.revision_digest === candidate.listing.value.pack.digest && entry.install_state === "selectable")) return false;
    const root = join(options.stateDirectory, "client-bindings");
    const releases = await records(join(root, "releases"));
    if (!releases.some((release) => release.client_id === candidate.release.client_id
      && release.release_digest === candidate.release.release_digest
      && activityClientReleaseDigest(release) === candidate.release.release_digest)) return false;
    const deployments = await records(join(root, "deployments"));
    const bindings = await records(join(root, "bindings"));
    const expectedUrl = new URL(candidate.surface.entrypoint, options.clientOrigin).href;
    for (const seat of await candidateBrowserSeats(candidate.listing, options.launchInputs)) {
      let selectable = false;
      for (const binding of bindings) {
        if (binding.pack.digest !== candidate.listing.value.pack.digest
            || binding.pack.id !== candidate.listing.value.pack.id || binding.pack.version !== candidate.listing.value.pack.version
            || binding.client_contract !== candidate.listing.value.client.client_contract
            || binding.access_mode !== "participant" || !binding.roles.includes(seat.role)
            || binding.surface_id !== candidate.surface.surface_id || binding.preference !== "default") continue;
        const status = await readJson(join(root, "binding-status", `${binding.binding_id}.json`));
        const deployment = deployments.find((entry) => entry.deployment_id === binding.deployment_id);
        if (status.status !== "approved" || deployment?.client_id !== candidate.release.client_id
            || deployment.release_digest !== candidate.release.release_digest
            || !deployment.surfaces.some((surface) => surface.surface_id === candidate.surface.surface_id && surface.launch_url === expectedUrl)) continue;
        const deploymentStatus = await readJson(join(root, "deployment-status", `${deployment.deployment_id}.json`));
        if (deploymentStatus.status === "ready") selectable = true;
      }
      if (!selectable) return false;
    }
    const artifactRoot = repositoryPath(candidate.client_artifact_directory);
    if (await activityClientBuildDigest(artifactRoot) !== candidate.surface.artifact_digest) return false;
    const prefix = `/${candidate.surface.entrypoint.split("/")[1]}/`;
    const files = await readdir(artifactRoot, { recursive: true, withFileTypes: true });
    if (files.length > 128) return false;
    for (const file of files.filter((entry) => entry.isFile())) {
      const path = join(file.parentPath, file.name);
      const expectedBytes = await readFile(path);
      const response = await fetch(new URL(prefix + relative(artifactRoot, path), options.clientHostOrigin), {
        redirect: "error", signal: AbortSignal.timeout(3_000), cache: "no-store",
      });
      if (!response.ok || !Buffer.from(await response.arrayBuffer()).equals(expectedBytes)) return false;
    }
    return true;
  } catch {
    return false;
  }
}

/** Browser readiness belongs only to the selected account seats, never supplied agents. */
export async function candidateBrowserSeats(listing, inputs) {
  const { resolveRosterOption } = await import(pathToFileURL(resolve(repository, "sdk/typescript-hosted-contract/dist/index.js")));
  const option = resolveRosterOption(listing, inputs ?? listing.value.launch_input_schema.defaults);
  return listing.value.seats.filter((seat) =>
    seat.allowed_participation.some((kind) => kind === "account_human" || kind === "account_external_agent")
    && (option === null || (option.seat_ids.includes(seat.seat_id)
      && !option.house_agent_assignments.some((assignment) => assignment.seat_id === seat.seat_id))));
}

async function records(directory) {
  const names = (await readdir(directory)).filter((name) => name.endsWith(".json"));
  if (names.length > 128) throw new Error("unbounded_client_inventory");
  return Promise.all(names.map((name) => readJson(join(directory, name))));
}

async function readJson(path) {
  const metadata = await lstat(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size > 1_048_576) throw new Error("invalid_candidate_record");
  return JSON.parse(await readFile(path, "utf8"));
}

function repositoryPath(value) {
  if (typeof value !== "string") throw new Error("invalid_candidate_path");
  const path = resolve(repository, value);
  if (relative(repository, path).startsWith("..")) throw new Error("invalid_candidate_path");
  return path;
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === resolve(import.meta.filename)) {
  const [ctl, config, stateDirectory, controller, clientOrigin, clientHostOrigin, listingDigest, launchInputs] = process.argv.slice(2);
  process.stdout.write(JSON.stringify({ available: await internalCandidateAvailable({
    ctl, config, stateDirectory, controller, clientOrigin, clientHostOrigin, listingDigest,
    ...(launchInputs === undefined ? {} : { launchInputs: JSON.parse(launchInputs) }),
  }) }));
}
