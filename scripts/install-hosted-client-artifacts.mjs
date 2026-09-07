import { cp, readFile, rm } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { activityClientBuildDigest } from "./activity-client-identities.mjs";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const source = resolve(repository, "clients/agent-heist-web/dist");
const destination = resolve(repository, "web/demos/dist/agent-heist-v2");
const release = JSON.parse(
  await readFile(
    resolve(repository, "config/activity-clients/releases/agent-heist-web-v2.json"),
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

process.stdout.write(`Installed reviewed Agent Heist Activity Client at /agent-heist-v2/ (${expected}).\n`);
