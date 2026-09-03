import { createHash } from "node:crypto";
import { lstat, readdir, readFile } from "node:fs/promises";
import { relative, resolve, sep } from "node:path";

export async function activityClientBuildDigest(root) {
  const rootMetadata = await lstat(root);
  if (!rootMetadata.isDirectory() || rootMetadata.isSymbolicLink()) {
    throw new Error("Activity Client artifact root must be a regular directory");
  }
  const hash = createHash("sha256");
  hash.update("worldstream-activity-client-build-tree-v1\0");
  for (const path of await files(root)) {
    hash.update(relative(root, path).split(sep).join("/"));
    hash.update("\0");
    hash.update(createHash("sha256").update(await readFile(path)).digest());
    hash.update("\0");
  }
  return `sha256:${hash.digest("hex")}`;
}

export function activityClientReleaseClaimsDigest(release) {
  return sha256(Buffer.from(`worldstream-activity-client-release-claims-v1\0${JSON.stringify({
    schema: release.schema, client_id: release.client_id,
    client_contract: release.client_contract, artifacts: release.artifacts, surfaces: release.surfaces,
  })}`));
}

export function activityClientReleaseDigest(release) {
  return sha256(Buffer.from(`worldstream-activity-client-release-identity-v1\0${JSON.stringify({
    schema: release.schema, client_id: release.client_id,
    client_contract: release.client_contract, artifacts: release.artifacts,
    surfaces: release.surfaces, conformance: release.conformance,
  })}`));
}

async function files(root) {
  const result = [];
  const entries = await readdir(root, { withFileTypes: true });
  entries.sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0);
  for (const entry of entries) {
    const path = resolve(root, entry.name);
    if (entry.isDirectory()) result.push(...await files(path));
    else if (entry.isFile()) result.push(path);
    else throw new Error("Activity Client artifact tree may contain only regular files and directories");
  }
  return result.sort();
}

function sha256(bytes) {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}
