#!/usr/bin/env node
import { spawn } from "node:child_process";
import { access, readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const requirePack = createRequire(new URL(
  "../sdk/typescript-pack/packages/pack-sdk/package.json",
  import.meta.url,
));
const { blake3 } = await import(pathToFileURL(requirePack.resolve("@noble/hashes/blake3.js")));

const workspace = resolve(import.meta.dirname, "..");
const modes = new Set(process.argv.slice(2).filter((argument) => argument !== "--"));
for (const mode of modes) {
  if (!["--node-only", "--database-only", "--component-only"].includes(mode)) {
    throw new Error(`unknown Archive roster verification mode: ${mode}`);
  }
}
if (modes.size > 1) throw new Error("choose at most one Archive roster verification mode");

const listing = JSON.parse(await readFile(
  join(workspace, "config/hosted/listings/midnight-archive-0.4.0.json"),
  "utf8",
));
const proof = JSON.parse(await readFile(
  join(workspace, "packs/midnight-archive/evidence/production-proof-0.1.0-session-expiry.json"),
  "utf8",
));
if (
  proof.status !== "passed"
  || !/^blake3:[0-9a-f]{64}$/u.test(proof.bundleDigest)
  || listing.pack?.digest !== proof.revisionDigest
) {
  throw new Error("Archive roster Listing does not pin the exact passed Pack proof");
}
const bundle = resolve(process.env.WORLDSTREAM_MIDNIGHT_ARCHIVE_BUNDLE ?? join(
  workspace,
  "packs/midnight-archive/releases/0.1.0",
  `worldstream-midnight-archive-${proof.bundleDigest.slice("blake3:".length)}.wspack`,
));
await access(bundle);
const actualBundleDigest = `blake3:${Buffer.from(blake3(await readFile(bundle))).toString("hex")}`;
if (actualBundleDigest !== proof.bundleDigest) {
  throw new Error(`Archive roster Bundle digest mismatch: expected ${proof.bundleDigest}, received ${actualBundleDigest}`);
}

if (modes.size === 0 || modes.has("--node-only")) {
  await run("pnpm", ["--filter", "@worldstream/hosted-contract", "test"]);
  await run("pnpm", ["hosted:package:test"]);
  await run(process.execPath, ["--test",
    "scripts/hosted-roster-fixture.test.mjs",
    "scripts/hosted-rendered-browser-journey.test.mjs",
  ]);
  await run("pnpm", ["--dir", "web/platform", "exec", "vitest", "run", "--environment", "node",
    "src/midnight-archive-roster-qualification.test.ts",
    "src/hosted-catalog.test.ts",
    "src/hosted-formation.test.ts",
    "src/bff.test.ts",
  ]);
  await run("pnpm", ["--dir", "packs/midnight-archive", "pack:test"]);
  await run("pnpm", ["archive-client:lint"]);
  await run("pnpm", ["archive-client:test"]);
}

if (modes.size === 0 || modes.has("--database-only")) {
  await run("supabase", ["test", "db",
    "supabase/tests/database/midnight_archive_four_rosters.test.sql",
    "supabase/tests/database/platform_roster_options.test.sql",
    "supabase/tests/database/platform_house_fill.test.sql",
  ]);
  await run(process.execPath, ["scripts/verify-platform-formation-concurrency.mjs"]);
}

if (modes.size === 0 || modes.has("--component-only")) {
  await run("cargo", ["test", "--locked", "-p", "worldstream-server", "--test", "midnight_archive_component_room",
    "current_bundle_completes_all_four_rosters_and_replays_exactly", "--", "--ignored", "--exact", "--nocapture"], {
    WORLDSTREAM_MIDNIGHT_ARCHIVE_BUNDLE: bundle,
  });
}

console.log(`Midnight Archive four-roster verification passed${modes.size === 0 ? "" : ` (${[...modes][0]})`}.`);

async function run(command, args, environment = {}) {
  await new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, args, {
      cwd: workspace,
      env: { ...process.env, ...environment },
      stdio: "inherit",
    });
    child.once("error", rejectRun);
    child.once("exit", (code, signal) => {
      if (code === 0) resolveRun();
      else rejectRun(new Error(`${command} ${args.join(" ")} failed (${signal ?? `exit ${code}`})`));
    });
  });
}
