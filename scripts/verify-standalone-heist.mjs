#!/usr/bin/env node
// Real local Runtime/Controller acceptance. The OS browser opener is covered by
// the CLI unit test; this check consumes the same protected handoff in an
// isolated browser instead of opening the operator's personal browser.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { promisify } from "node:util";
import { setTimeout } from "node:timers/promises";
import { chromium } from "playwright";
import { startActivityClientHost } from "./serve-activity-clients.mjs";
import { activityClientBuildDigest } from "./activity-client-identities.mjs";

const execute = promisify(execFile);
const executableNames = ["worldstreamctl", "worldstreamd", "worldstream-studio-supervisor", "worldstream-assignment-mcp"];
const executablePreflight = {
  schema: "worldstream/local-executable-preflight/v1",
  purpose: "help_only_not_service_readiness",
  per_binary_timeout_ms: 180_000,
  aggregate_timeout_ms: 480_000,
  elapsed_ms: 0,
  executions: [],
};
const workspace = resolve(import.meta.dirname, "..");
const root = await realpath(await mkdtemp(join(tmpdir(), "worldstream-local-heist-proof-")));
const state = join(root, "studio");
const config = join(root, "runtime.toml");
const host = await startActivityClientHost({ port: 0 });
const browser = await chromium.launch({ headless: true, ...(process.env.WORLDSTREAM_BROWSER_BINARY
  ? { executablePath: process.env.WORLDSTREAM_BROWSER_BINARY }
  : process.platform === "darwin" ? { executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" } : {}) });
let startAttempted = false;
let proofFailure;
let receipt;
try {
  await mkdir(join(root, "bin"), { mode: 0o700 });
  for (const binary of executableNames) {
    await copyFile(join(workspace, "target/debug", binary), join(root, "bin", binary));
  }
  // Run the exact retained copies once before the managed startup deadline.
  // Normal OS execution policy still applies; never recopy after this check.
  await preflightExecutables();
  const release = JSON.parse(await readFile(join(workspace, "config/activity-clients/releases/agent-heist-web-v4.json"), "utf8"));
  assert.equal(await activityClientBuildDigest(join(workspace, "clients/agent-heist-web/dist")), release.artifacts[0].digest);
  await writeFile(config, `config_version = 1\n[server]\nbind = "127.0.0.1:9410"\n[storage]\nprofile = "sqlite-bundled"\ndata_dir = "${join(root, "runtime")}"\ndeployment_lineage = "development/local-heist-proof"\nstorage_epoch = 1\n[authority.bootstrap]\nsecret_file = "${join(root, "authority.secret")}"\n`, { mode: 0o600 });
  const bindings = JSON.parse(await readFile(join(workspace, "config/activity-clients/local-bindings.json"), "utf8"));
  bindings.deployments = bindings.deployments.filter((value) => value.release_digest === release.release_digest || value.client_id.includes("inspector"));
  bindings.bindings = bindings.bindings.filter((value) => value.pack.id === "worldstream.agent-heist" && value.pack.version === "0.3.0");
  for (const deployment of bindings.deployments) for (const surface of deployment.surfaces) {
    surface.launch_url = `${host.origin}${new URL(surface.launch_url).pathname}`;
  }
  await writeFile(join(root, "bindings.json"), JSON.stringify(bindings), { mode: 0o600 });
  const declaration = join(root, "clients.json");
  await writeFile(declaration, JSON.stringify({ schema: "worldstream/client-declaration-import/v1",
    release_files: ["agent-heist-web-v4.json", "inspector-web-v2.json"].map((name) => join(workspace, "config/activity-clients/releases", name)),
    bindings_file: join(root, "bindings.json") }), { mode: 0o600 });
  await cli("init");
  const preview = await cli("init", "--client-declaration", declaration, "--preview");
  await cli("init", "--client-declaration", declaration, "--approve-imports", preview.import_review.digest);
  startAttempted = true;
  await cli("server", "start", "--participant-console-origin", host.origin);
  const setup = join(root, "heist.json");
  await cli("room", "example", "--pack", "worldstream.agent-heist@0.3.0", "--output", setup);
  await cli("room", "validate", "--file", setup);
  const created = await cli("room", "create", "--file", setup);
  const operation = created.room_operation.operation;
  const room = created.room_operation.room_id;
  assert.ok(operation && room);
  // Only this freshly generated test installation's control credential is read.
  // It is sent to its already-started loopback Controller and is never printed.
  const credential = await readFile(join(state, "control-access.v1"));
  const handoffResponse = await fetch(`http://127.0.0.1:9420/api/v1/room-setup-operations/${operation}/seats/navigator/client-handoff`, {
    method: "POST", headers: { authorization: `Bearer ${credential.subarray(16).toString("hex")}`, "content-type": "application/json" }, body: "{}",
  });
  assert.ok(handoffResponse.ok);
  const handoff = await handoffResponse.json();
  assert.equal(new URL(handoff.client_url).pathname, "/agent-heist-v4/");
  const page = await browser.newPage();
  let platformRequests = 0;
  page.on("request", (request) => { if (new URL(request.url()).pathname === "/api/auth/session") platformRequests += 1; });
  await page.goto(handoff.client_url, { waitUntil: "domcontentloaded" });
  await page.getByRole("heading", { name: "Agent Heist", exact: true }).waitFor({ timeout: 15_000 });
  await page.getByText("Authorized participant surface", { exact: true }).waitFor();
  await page.getByText("Role Navigator", { exact: true }).waitFor();
  await page.getByText("Lobby", { exact: true }).first().waitFor();
  assert.equal(new URL(page.url()).hash, "");
  assert.equal(platformRequests, 0);
  let navigatorReady = false;
  for (let attempt = 0; attempt < 5; attempt += 1) {
    const inspection = await cli("room", "inspect", room);
    navigatorReady = inspection.launch_assessment?.readiness.seats.some((seat) => seat.seat_id === "navigator" && seat.ready && seat.reason === "ready") ?? false;
    if (navigatorReady) break;
    await setTimeout(1_000);
  }
  assert.ok(navigatorReady, "Room inspection must confirm live Navigator readiness");
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.getByRole("heading", { name: "Agent Heist", exact: true }).waitFor();
  await page.getByText("Role Navigator", { exact: true }).waitFor();
  assert.equal(platformRequests, 0);
  receipt = { status: "passed", surface: "heist-web", release_digest: release.release_digest,
    checks: ["exact_release_bytes", "cli_init_import_start_example_validate_create", "protected_controller_handoff", "authorized_navigator_lobby", "live_navigator_readiness", "fragment_removed", "cookie_reload", "no_platform_auth"],
    os_browser_opener: "not_exercised", provider_calls: 0, executable_preflight: executablePreflight };
} catch (error) {
  proofFailure = error;
} finally {
  const cleanupFailures = [];
  for (const cleanup of [
    () => browser.close(),
    async () => { if (startAttempted) await cli("server", "stop"); },
    async () => { if (startAttempted) await cli("server", "controller-stop"); },
    () => host.close(),
  ]) {
    try { await cleanup(); } catch (error) { cleanupFailures.push(error); }
  }
  if (cleanupFailures.length > 0) throw new AggregateError([...(proofFailure ? [proofFailure] : []), ...cleanupFailures], "Local Heist proof cleanup incomplete; retained test installation was preserved");
  // Delete only this test's exact generated directory after confirmed stop.
  await rm(root, { recursive: true, force: false });
}
if (proofFailure) throw proofFailure;
console.log(JSON.stringify(receipt));

async function preflightExecutables() {
  const startedAt = performance.now();
  for (const binary of executableNames) {
    const remaining = executablePreflight.aggregate_timeout_ms - (performance.now() - startedAt);
    if (remaining <= 0) throw new Error(`Executable help preflight aggregate deadline exceeded: ${JSON.stringify(executablePreflight)}`);
    const started = performance.now();
    let result;
    try {
      await execute(join(root, "bin", binary), ["--help"], {
        cwd: root,
        timeout: Math.max(1, Math.floor(Math.min(executablePreflight.per_binary_timeout_ms, remaining))),
        maxBuffer: 64 * 1024,
        // Only this owned, help-only subprocess is killed on its bounded timeout.
        killSignal: "SIGKILL",
      });
      result = { binary, elapsed_ms: Math.round(performance.now() - started), exit_code: 0, signal: null, timed_out: false };
    } catch (error) {
      result = { binary, elapsed_ms: Math.round(performance.now() - started), exit_code: Number.isInteger(error.code) ? error.code : null,
        signal: ["SIGKILL", "SIGTERM"].includes(error.signal) ? error.signal : null, timed_out: error.killed === true };
    }
    executablePreflight.executions.push(result);
    executablePreflight.elapsed_ms = Math.round(performance.now() - startedAt);
    if (result.exit_code !== 0) throw new Error(`Executable help preflight failed (subprocess output suppressed): ${JSON.stringify(executablePreflight)}`);
  }
}

async function cli(...args) {
  let output;
  try {
    output = await execute(join(root, "bin/worldstreamctl"), ["--config", config, ...args,
      "--state-dir", state, "--controller", "127.0.0.1:9420", "--json"], { cwd: workspace, timeout: 45_000, maxBuffer: 1024 * 1024 });
  } catch (error) {
    let code = "unavailable";
    try { const report = JSON.parse(error.stdout); if (/^[a-z_]+$/.test(report.code)) code = report.code; } catch {}
    throw new Error(`local Heist proof CLI ${args.slice(0, 2).join(" ")} failed: ${code} (private output suppressed)`);
  }
  return JSON.parse(output.stdout);
}
