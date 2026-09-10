import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { activityClientMounts, startActivityClientHost } from "./serve-activity-clients.mjs";
import { readInternalCandidates } from "./hosted-internal-candidates.mjs";
import { activityClientBuildDigest } from "./activity-client-identities.mjs";

const repository = resolve(import.meta.dirname, "..");
const bindings = JSON.parse(await readFile(resolve(repository, "config/activity-clients/hosted-local-bindings.json"), "utf8"));
const paths = new Set(bindings.deployments.flatMap((deployment) => deployment.surfaces.map((surface) =>
  `/${new URL(surface.launch_url).pathname.split("/")[1]}/`)));
const candidates = await readInternalCandidates();
for (const candidate of candidates) {
  if (await activityClientBuildDigest(resolve(repository, candidate.client_artifact_directory)) !== candidate.surface.artifact_digest) {
    throw new Error("reviewed_internal_client_bytes_unavailable");
  }
}
const mounts = activityClientMounts.filter((mount) => paths.has(mount.prefix)
  || mount.root.startsWith(resolve(repository, "config/activity-clients/artifacts")));
const values = process.argv.slice(2);
if (values.length !== 2 || values[0] !== "--port" || !/^[0-9]{1,5}$/u.test(values[1])) throw new Error("invalid_local_client_host_port");
const host = await startActivityClientHost({ port: Number(values[1]), mounts,
  browserStreamOrigin: process.env.WORLDSTREAM_LOCAL_BROWSER_STREAM_URL });
console.log(`Reviewed local Activity Clients listening on ${host.origin}`);
