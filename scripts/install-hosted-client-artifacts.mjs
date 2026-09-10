import { cp, readFile, rm } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { activityClientBuildDigest } from "./activity-client-identities.mjs";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// Current and retained clients remain separate immutable builds. Old Rooms
// must never fetch changed bytes at their retained entrypoint.
for (const [path, sourcePath, releaseFile] of [
  ["agent-heist-v9", "clients/agent-heist-web/dist", "agent-heist-web-v9.json"],
  ["agent-heist-v8", "config/activity-clients/artifacts/agent-heist-web-v8", "agent-heist-web-v8.json"],
  ["agent-heist-v7", "config/activity-clients/artifacts/agent-heist-web-v7", "agent-heist-web-v7.json"],
  ["agent-heist-v6", "config/activity-clients/artifacts/agent-heist-web-v6", "agent-heist-web-v6.json"],
  ["agent-heist-v5", "config/activity-clients/artifacts/agent-heist-web-v5", "agent-heist-web-v5.json"],
  ["agent-heist-v4", "config/activity-clients/artifacts/agent-heist-web-v4", "agent-heist-web-v4.json"],
  ["agent-heist-v3", "config/activity-clients/artifacts/agent-heist-web-v3", "agent-heist-web-v3.json"],
  ["agent-heist-v2", "config/activity-clients/artifacts/agent-heist-web-v2", "agent-heist-web-v2.json"],
]) {
  const source = resolve(repository, sourcePath);
  const destination = resolve(repository, "web/demos/dist", path);
  const release = JSON.parse(
    await readFile(
      resolve(repository, "config/activity-clients/releases", releaseFile),
      "utf8",
    ),
  );
  const expected = release.artifacts?.[0]?.digest;
  const observed = await activityClientBuildDigest(source);
  if (typeof expected !== "string" || observed !== expected) {
    throw new Error(`reviewed Agent Heist artifact changed: expected ${expected}, observed ${observed}`);
  }

  // This path is inside the disposable Vite output tree and is never retained
  // user data. Copying after the shell build keeps the Activity Client a
  // separately built artifact while serving it from the product origin.
  await rm(destination, { recursive: true, force: true });
  await cp(source, destination, { recursive: true, errorOnExist: true });
  if (await activityClientBuildDigest(destination) !== expected) {
    throw new Error("installed Agent Heist artifact differs from its reviewed release");
  }

  process.stdout.write(`Installed reviewed Agent Heist Activity Client at /${path}/ (${expected}).\n`);
}
