#!/usr/bin/env node
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const workspace = resolve(import.meta.dirname, "..");
const proofPath = join(
  workspace,
  "packs/midnight-archive/evidence/production-proof-0.1.0-authored-scenarios.json",
);
const proof = JSON.parse(await readFile(proofPath, "utf8"));
if (proof.status !== "passed" || !/^blake3:[0-9a-f]{64}$/u.test(proof.bundleDigest)) {
  throw new Error("current Midnight Archive production proof is not a passed exact Bundle identity");
}

const bundle = process.env.WORLDSTREAM_MIDNIGHT_ARCHIVE_BUNDLE ?? join(
  workspace,
  "packs/midnight-archive/releases/0.1.0",
  `worldstream-midnight-archive-${proof.bundleDigest.slice("blake3:".length)}.wspack`,
);
const release = process.env.WORLDSTREAM_MIDNIGHT_ARCHIVE_CLIENT_RELEASE
  ?? join(workspace, "config/activity-clients/releases/midnight-archive-web-v10.json");
const modes = ["agreement", "both-objectives", "low-reserve", "mira-live", "crew-live", "unavailable-live"];

for (const [index, mode] of modes.entries()) {
  await run(mode, index > 0 || process.env.WORLDSTREAM_ACCEPTANCE_REUSE_BINARIES === "1");
}

async function run(mode, reuseBinaries) {
  await new Promise((resolveRun, rejectRun) => {
    const child = spawn(process.execPath, [join(workspace, "scripts/verify-midnight-archive-browser.mjs")], {
      cwd: workspace,
      env: {
        ...process.env,
        WORLDSTREAM_ACCEPTANCE_REUSE_BINARIES: reuseBinaries ? "1" : "0",
        WORLDSTREAM_MIDNIGHT_ARCHIVE_BUNDLE: bundle,
        WORLDSTREAM_MIDNIGHT_ARCHIVE_CLIENT_RELEASE: release,
        WORLDSTREAM_MIDNIGHT_ARCHIVE_PROOF_MODE: mode,
      },
      stdio: "inherit",
    });
    child.once("error", rejectRun);
    child.once("exit", (code, signal) => {
      if (code === 0) resolveRun();
      else rejectRun(new Error(`Midnight Archive ${mode} acceptance failed (${signal ?? `exit ${code}`})`));
    });
  });
}
