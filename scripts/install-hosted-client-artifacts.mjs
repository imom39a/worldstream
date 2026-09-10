import { cp, readFile, rm } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { activityClientBuildDigest } from "./activity-client-identities.mjs";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// Current and retained clients remain separate immutable builds. Old Rooms
// must never fetch changed bytes at their retained entrypoint.
export const hostedClientArtifacts = Object.freeze([
  Object.freeze(["Agent Heist", "agent-heist-v9", "clients/agent-heist-web/dist", "agent-heist-web-v9.json"]),
  Object.freeze(["Agent Heist", "agent-heist-v8", "config/activity-clients/artifacts/agent-heist-web-v8", "agent-heist-web-v8.json"]),
  Object.freeze(["Agent Heist", "agent-heist-v7", "config/activity-clients/artifacts/agent-heist-web-v7", "agent-heist-web-v7.json"]),
  Object.freeze(["Agent Heist", "agent-heist-v6", "config/activity-clients/artifacts/agent-heist-web-v6", "agent-heist-web-v6.json"]),
  Object.freeze(["Agent Heist", "agent-heist-v5", "config/activity-clients/artifacts/agent-heist-web-v5", "agent-heist-web-v5.json"]),
  Object.freeze(["Agent Heist", "agent-heist-v4", "config/activity-clients/artifacts/agent-heist-web-v4", "agent-heist-web-v4.json"]),
  Object.freeze(["Agent Heist", "agent-heist-v3", "config/activity-clients/artifacts/agent-heist-web-v3", "agent-heist-web-v3.json"]),
  Object.freeze(["Agent Heist", "agent-heist-v2", "config/activity-clients/artifacts/agent-heist-web-v2", "agent-heist-web-v2.json"]),
  Object.freeze(["Midnight Archive", "midnight-archive-v13", "config/activity-clients/artifacts/midnight-archive-web-v13", "midnight-archive-web-v13.json"]),
  Object.freeze(["Midnight Archive", "midnight-archive-v12", "config/activity-clients/artifacts/midnight-archive-web-v12", "midnight-archive-web-v12.json"]),
  Object.freeze(["Midnight Archive", "midnight-archive-v10", "config/activity-clients/artifacts/midnight-archive-web-v10", "midnight-archive-web-v10.json"]),
]);

export async function installHostedClientArtifacts({
  repositoryRoot = repository,
  destinationRoot = resolve(repositoryRoot, "web/demos/dist"),
  artifacts = hostedClientArtifacts,
} = {}) {
  for (const [label, path, sourcePath, releaseFile] of artifacts) {
    const source = resolve(repositoryRoot, sourcePath);
    const destination = resolve(destinationRoot, path);
    const release = JSON.parse(
      await readFile(
        resolve(repositoryRoot, "config/activity-clients/releases", releaseFile),
        "utf8",
      ),
    );
    const expected = release.artifacts?.[0]?.digest;
    const observed = await activityClientBuildDigest(source);
    if (typeof expected !== "string" || observed !== expected) {
      throw new Error(`reviewed ${label} artifact changed: expected ${expected}, observed ${observed}`);
    }

    // This path is inside the disposable Vite output tree and is never retained
    // user data. Copying after the shell build keeps the Activity Client a
    // separately built artifact while serving it from the product origin.
    await rm(destination, { recursive: true, force: true });
    await cp(source, destination, { recursive: true, errorOnExist: true });
    if (await activityClientBuildDigest(destination) !== expected) {
      throw new Error(`installed ${label} artifact differs from its reviewed release`);
    }

    process.stdout.write(`Installed reviewed ${label} Activity Client at /${path}/ (${expected}).\n`);
  }
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await installHostedClientArtifacts();
}
