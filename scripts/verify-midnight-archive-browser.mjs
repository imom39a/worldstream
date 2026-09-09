#!/usr/bin/env node
// Local, provider-free Midnight Archive acceptance.  This intentionally keeps
// the browser on the real protected Activity Client route while the Runtime
// and Component Host own every state transition.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { promisify } from "node:util";
import { chromium } from "playwright";
import { startActivityClientHost } from "./serve-activity-clients.mjs";
import { activityClientBuildDigest } from "./activity-client-identities.mjs";

const execute = promisify(execFile);
const workspace = resolve(import.meta.dirname, "..");
const binaries = ["worldstreamctl", "worldstreamd", "worldstream-studio-supervisor", "worldstream-assignment-mcp"];
const forbidden = ["authentic_candidate_id", "is_authentic", "truth_marker"];
const recoveryCandidateId = "ledger-violet";
const technicalRoute = [
  ["stage_move", { destination: "records" }], ["commit_turn", {}],
  ["stage_use_verifier", {}], ["commit_turn", {}],
  ["stage_move", { destination: "plant" }], ["commit_turn", {}],
  ["stage_open_service_hatch", {}], ["commit_turn", {}],
  ["stage_move", { destination: "vault" }], ["commit_turn", {}],
  ["stage_recover_candidate", { candidate_id: recoveryCandidateId }], ["commit_turn", {}],
  ["stage_move", { destination: "plant" }], ["commit_turn", {}],
  ["stage_move", { destination: "records" }], ["commit_turn", {}],
  ["stage_move", { destination: "atrium" }], ["commit_turn", {}],
  ["stage_extract", {}], ["commit_turn", {}],
];
const evidenceServiceRoute = [
  ["stage_move", { destination: "conservation" }], ["commit_turn", {}],
  ["stage_inspect_conservation", {}], ["commit_turn", {}],
  ["stage_move", { destination: "records" }], ["commit_turn", {}],
  ["stage_inspect_records", {}], ["commit_turn", {}],
  ["stage_move", { destination: "plant" }], ["commit_turn", {}],
  ["stage_open_service_hatch", {}], ["commit_turn", {}],
  ["stage_move", { destination: "vault" }], ["commit_turn", {}],
  ["stage_recover_candidate", { candidate_id: recoveryCandidateId }], ["commit_turn", {}],
  ["stage_move", { destination: "plant" }], ["commit_turn", {}],
  ["stage_move", { destination: "records" }], ["commit_turn", {}],
  ["stage_move", { destination: "atrium" }], ["commit_turn", {}],
  ["stage_extract", {}], ["commit_turn", {}],
];
const proofMode = process.env.WORLDSTREAM_MIDNIGHT_ARCHIVE_PROOF_MODE ?? "technical";
const witness = proofMode === "technical"
  ? { route: technicalRoute, turns: 10, turnsRemaining: 6, powerRemaining: 0, candidate: "Violet Ledger", checks: ["ten_turn_phone_witness", "verifier_selected_candidate"] }
  : proofMode === "evidence-service"
    ? { route: evidenceServiceRoute, turns: 12, turnsRemaining: 4, powerRemaining: 1, candidate: "Violet Ledger", checks: ["twelve_turn_evidence_service_witness", "sourced_evidence_disclosure"] }
    : (() => { throw new Error(`unknown Midnight Archive proof mode: ${proofMode}`); })();
const acceptanceStartedAt = Date.now();

const root = resolve(await realpath(await mkdtemp(join(tmpdir(), "worldstream-midnight-archive-proof-"))));
const state = join(root, "studio");
const config = join(root, "runtime.toml");
const [runtimePort, controllerPort] = await availableLoopbackPorts(2);
const controllerAddress = `127.0.0.1:${controllerPort}`;
let host;
let browser;
let page;
let bridgeFailure;
let started = false;
let failure;
let receipt;
let submittedRecoveryPayload;

try {
  const bundle = await archiveBundle();
  if (process.env.WORLDSTREAM_ACCEPTANCE_REUSE_BINARIES !== "1") await buildExecutables();
  debug("executables ready");
  await mkdir(join(root, "bin"), { mode: 0o700 });
  for (const binary of binaries) await copyFile(join(workspace, "target/release", binary), join(root, "bin", binary));
  await preflightExecutables();
  const bundleIdentity = await inspectBundle(bundle);
  debug("binary preflight and bundle inspection complete");

  const releasePath = join(workspace, "config/activity-clients/releases/midnight-archive-web-v2.json");
  const release = JSON.parse(await readFile(releasePath, "utf8"));
  const standaloneSurface = release.surfaces.find((surface) => surface.surface_id === "midnight-archive-web");
  assert.ok(standaloneSurface, "current Archive standalone Client Surface missing");
  assert.equal(await activityClientBuildDigest(join(workspace, "clients/midnight-archive-web/dist")), release.artifacts[0].digest);
  host = await startActivityClientHost({ port: 0 });
  browser = await chromium.launch({
    headless: true,
    ...(process.env.WORLDSTREAM_BROWSER_BINARY ? { executablePath: process.env.WORLDSTREAM_BROWSER_BINARY } : {}),
  });
  await writeFile(config, `config_version = 1\n[server]\nbind = "127.0.0.1:${runtimePort}"\n[storage]\nprofile = "sqlite-bundled"\ndata_dir = "${join(root, "runtime")}"\ndeployment_lineage = "development/midnight-archive-proof"\nstorage_epoch = 1\n[authority.bootstrap]\nsecret_file = "${join(root, "authority.secret")}"\n`, { mode: 0o600 });

  const bootstrap = JSON.parse(await readFile(join(workspace, "config/activity-clients/local-bindings.json"), "utf8"));
  bootstrap.deployments = bootstrap.deployments.filter((deployment) => deployment.client_id.includes("inspector"));
  const declaration = join(root, "clients.json");
  for (const deployment of bootstrap.deployments) for (const surface of deployment.surfaces) surface.launch_url = `${host.origin}${new URL(surface.launch_url).pathname}`;
  const archiveDeployment = {
    schema: "worldstream/client-deployment/v1",
    deployment_id: "first-party-midnight-archive-web-v2",
    client_id: release.client_id,
    release_digest: release.release_digest,
    trust_level: "externally_trusted",
    surfaces: [{ surface_id: standaloneSurface.surface_id, launch_url: `${host.origin}${standaloneSurface.entrypoint}` }],
  };
  bootstrap.deployments.push(archiveDeployment);
  bootstrap.bindings = bootstrap.bindings.filter((binding) => binding.deployment_id === "first-party-inspector-web-v2");
  bootstrap.bindings.push({
    schema: "worldstream/client-binding/v1",
    binding_id: "midnight-archive-0-1-lead-web-v2",
    pack: { id: "worldstream.midnight-archive", version: "0.1.0", digest: bundleIdentity.revision_digest },
    client_contract: "worldstream/activity-client-protocol/v1",
    access_mode: "participant",
    roles: ["lead"],
    deployment_id: archiveDeployment.deployment_id,
    surface_id: "midnight-archive-web",
    preference: "default",
  });
  const bindings = join(root, "bindings.json");
  await writeFile(bindings, JSON.stringify(bootstrap), { mode: 0o600 });
  await writeFile(declaration, JSON.stringify({
    schema: "worldstream/client-declaration-import/v1",
    release_files: [join(workspace, "config/activity-clients/releases/inspector-web-v2.json"), releasePath],
    bindings_file: bindings,
  }), { mode: 0o600 });

  await cli("init");
  const preview = await cli("init", "--client-declaration", declaration, "--preview");
  await cli("init", "--client-declaration", declaration, "--approve-imports", preview.import_review.digest);
  await cli("pack", "inspect", "--bundle", bundle);
  await cli("pack", "prove", bundle);
  await cli("pack", "approve", "--bundle", bundle, "--operator-id", "local-midnight-archive-proof", "--decided-at", "2026-09-09T12:00:00Z");
  await cli("pack", "install", "--bundle", bundle, "--installed-at", "2026-09-09T12:00:01Z");
  await cli("pack", "inventory");
  const digest = bundleIdentity.revision_digest;
  await cli("pack", "set-selectable", "--bundle-digest", bundleIdentity.bundle_digest, "--selectable", "true");
  await cli("pack", "restart-readiness");
  debug("Pack approved, installed, selectable, and restart-ready");
  started = true;
  await lifecycleCli("server", "start", "--participant-console-origin", host.origin, "--timeout-seconds", "90");
  debug("managed Runtime ready");
  const setup = join(root, "archive.json");
  await writeFile(setup, JSON.stringify({
    schema: "worldstream/room-setup/v1",
    pack: {
      id: "worldstream.midnight-archive",
      version: "0.1.0",
      digest,
    },
    configuration: { scenario_id: "standard-v1" },
    seats: [{
      label: "lead",
      role: "lead",
      required: true,
      display_name: "Lead",
      principal: { reference: "lead", kind: "human" },
    }],
    operator_view: false,
  }), { mode: 0o600 });
  await cli("room", "validate", "--file", setup);
  const created = await createRoom(setup);
  const operation = created.room_operation.operation;
  const room = created.room_operation.room_id;
  const credential = await readFile(join(state, "control-access.v1"));
  const handoff = await issueClientHandoff(operation, credential);
  assert.equal(new URL(handoff.client_url).pathname, standaloneSurface.entrypoint);
  debug("solo Room and client handoff ready");

  page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  page.on("console", (message) => {
    if (message.type() === "error") debug(`browser console error: ${message.text().slice(0, 500)}`);
  });
  page.on("pageerror", (error) => debug(`browser page error: ${error.message.slice(0, 500)}`));
  page.on("requestfailed", (request) => debug(`browser request failed: ${new URL(request.url()).pathname} (${request.failure()?.errorText ?? "unknown"})`));
  await page.route("http://127.0.0.1:9420/**", async (requestRoute) => {
    const browserRequest = requestRoute.request();
    const target = new URL(browserRequest.url());
    target.port = String(controllerPort);
    try {
      if (target.pathname.endsWith("/session:act")) {
        const action = JSON.parse(browserRequest.postData() ?? "null");
        if (action?.action_type === "stage_recover_candidate") submittedRecoveryPayload = action.payload;
      }
      const response = await requestRoute.fetch({ url: target.href });
      const body = await response.body();
      assertPrivatePayload(body.toString("utf8"), "controller response");
      debug(`browser controller request: ${target.pathname} -> ${response.status()}`);
      await requestRoute.fulfill({ response, body });
    } catch (error) {
      bridgeFailure ??= error instanceof Error && error.code === "ERR_ASSERTION"
        ? error
        : new Error(`browser controller bridge failed for ${target.pathname}`);
      debug(`browser controller request failed: ${target.pathname}`);
      if (!page.isClosed()) await requestRoute.abort("failed").catch(() => {});
    }
  });
  page.on("request", (request) => { if (request.url().includes("fixture")) throw new Error("fixture request observed"); });
  page.on("websocket", (socket) => socket.on("framereceived", (payload) => {
    try { assertPrivatePayload(String(payload), "WebSocket frame"); } catch (error) {
      bridgeFailure ??= error;
    }
  }));
  await page.goto(handoff.client_url, { waitUntil: "domcontentloaded" });
  await waitForText(page, /Preparing the archive/i, "briefing");
  await page.getByText(/The Room is synchronized for readiness/i).first().waitFor({ timeout: 15_000 });
  debug("briefing Projection synchronized");
  await assertActionControlsDisabled(page);
  await assertViewportFits(page, "desktop briefing");
  await startActivity(room);
  await page.getByText("Projection current", { exact: true }).waitFor({ timeout: 15_000 });
  debug("Activity Start committed");
  await page.setViewportSize({ width: 390, height: 844 });
  await assertViewportFits(page, "phone mission");
  await assertPhoneControls(page);
  await runRoute(page);
  await page.getByRole("heading", { name: "The authentic ledger is out", exact: true }).waitFor({ timeout: 15_000 });
  assert.deepEqual(submittedRecoveryPayload, { candidate_id: recoveryCandidateId }, "Recovery must submit the canonical candidate payload");
  const body = await page.locator("body").innerText();
  assert.match(body, new RegExp(`Turns remaining\\s*${witness.turnsRemaining}`, "i"));
  assert.match(body, new RegExp(`Power reserve\\s*${witness.powerRemaining}`, "i"));
  const replayButton = page.getByRole("button", { name: /replay/i });
  assert.equal(await replayButton.count(), 1, "Replay acceptance blocked: the Archive client exposes no Replay control or authorized replay endpoint seam");
  await replayButton.click();
  await page.getByText(/verified.*replay|replay.*verified|canonical history/i).waitFor({ timeout: 10_000 });
  if (bridgeFailure) throw bridgeFailure;
  receipt = { status: "passed", pack: "worldstream.midnight-archive@0.1.0", bundle_digest: bundleIdentity.bundle_digest, revision_digest: bundleIdentity.revision_digest, room, turns: witness.turns, turns_remaining: witness.turnsRemaining, power_remaining: witness.powerRemaining, provider_calls: 0, checks: ["briefing_projection_before_activity_start", "actions_disabled_until_sync", "desktop_and_phone_layouts", ...witness.checks, "canonical_recovery_payload", "service_gate_boundary", "terminal_success", "authorized_replay", "private_authenticity_non_leakage"] };
} catch (error) {
  failure = error;
} finally {
  const cleanupFailures = [];
  for (const cleanup of [async () => { if (page) await page.unrouteAll({ behavior: "ignoreErrors" }); if (browser) await browser.close(); }, async () => { if (started) await stopIfAvailable("stop"); }, async () => { if (started) await stopIfAvailable("controller-stop"); }, async () => { if (host) await host.close(); }]) {
    try { await cleanup(); } catch (error) { cleanupFailures.push(error); }
  }
  if (cleanupFailures.length) throw new AggregateError([...(failure ? [failure] : []), ...cleanupFailures], "Midnight Archive proof cleanup incomplete");
  if (failure && process.env.WORLDSTREAM_ACCEPTANCE_KEEP_ROOT === "1") {
    process.stderr.write(`Midnight Archive diagnostic root retained at ${root}\n`);
  } else {
    await rm(root, { recursive: true, force: false });
  }
}
if (failure) throw failure;
console.log(JSON.stringify(receipt));

async function archiveBundle() {
  if (process.env.WORLDSTREAM_MIDNIGHT_ARCHIVE_BUNDLE) return process.env.WORLDSTREAM_MIDNIGHT_ARCHIVE_BUNDLE;
  const proof = JSON.parse(await readFile(join(
    workspace,
    "packs/midnight-archive/evidence/production-proof-0.1.0-evidence-route.json",
  ), "utf8"));
  assert.equal(proof.status, "passed");
  assert.match(proof.bundleDigest, /^blake3:[0-9a-f]{64}$/u);
  return join(
    workspace,
    "packs/midnight-archive/releases/0.1.0",
    `worldstream-midnight-archive-${proof.bundleDigest.slice("blake3:".length)}.wspack`,
  );
}

async function inspectBundle(bundle) {
  const output = await execute(join(root, "bin/worldstreamctl"), ["pack", "inspect", "--bundle", bundle], {
    cwd: root,
    timeout: 90_000,
    maxBuffer: 2 * 1024 * 1024,
  });
  const report = JSON.parse(output.stdout);
  assert.match(report.bundle_digest, /^blake3:[0-9a-f]{64}$/u);
  assert.match(report.revision_digest, /^blake3:[0-9a-f]{64}$/u);
  return { bundle_digest: report.bundle_digest, revision_digest: report.revision_digest };
}

async function buildExecutables() {
  await execute("cargo", ["build", "--locked", "--release", "-j", "1", "-p", "worldstream-server", "--bin", "worldstreamctl", "--bin", "worldstreamd", "-p", "worldstream-studio-supervisor", "--bin", "worldstream-studio-supervisor", "--bin", "worldstream-assignment-mcp"], { cwd: workspace, timeout: 900_000, maxBuffer: 4 * 1024 * 1024 });
}

async function preflightExecutables() {
  for (const binary of binaries) await execute(join(root, "bin", binary), ["--help"], { cwd: root, timeout: 180_000, maxBuffer: 64 * 1024 });
}

async function cli(...args) {
  const command = args[0] === "pack"
    ? ["--config", config, ...args, ...(args[1] === "prove" ? ["--json"] : [])]
    : ["--config", config, ...args, "--state-dir", state, "--controller", controllerAddress, "--json"];
  const output = await execute(join(root, "bin/worldstreamctl"), command, { cwd: workspace, timeout: 90_000, maxBuffer: 2 * 1024 * 1024 });
  return JSON.parse(output.stdout);
}

async function lifecycleCli(...args) {
  let lastError;
  for (let attempt = 0; attempt < 20; attempt += 1) {
    try {
      return await cli(...args);
    } catch (error) {
      lastError = error;
      const report = parseCommandFailure(error);
      if (!report || !["controller_unavailable", "lifecycle_incomplete"].includes(report.code)) throw error;
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
    }
  }
  throw lastError;
}

async function createRoom(setup) {
  let operation;
  let lastError;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    try {
      return operation === undefined
        ? await cli("room", "create", "--file", setup)
        : await cli("room", "setup", "resume", operation);
    } catch (error) {
      lastError = error;
      const report = parseCommandFailure(error);
      if (
        report?.code !== "setup_incomplete"
        || typeof report.operation_id !== "string"
        || (operation !== undefined && report.operation_id !== operation)
      ) throw error;
      operation = report.operation_id;
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
    }
  }
  throw lastError;
}

async function stopIfAvailable(operation) {
  try {
    await lifecycleCli("server", operation, "--timeout-seconds", "90");
  } catch (error) {
    if (parseCommandFailure(error)?.code !== "controller_unavailable") throw error;
  }
}

async function issueClientHandoff(operation, credential) {
  let lastFailure;
  for (let attempt = 0; attempt < 60; attempt += 1) {
    const response = await fetch(`http://${controllerAddress}/api/v1/room-setup-operations/${operation}/seats/lead/client-handoff`, {
      method: "POST",
      headers: { authorization: `Bearer ${credential.subarray(16).toString("hex")}`, "content-type": "application/json" },
      body: "{}",
    });
    const body = await response.json();
    if (response.ok) return body;
    lastFailure = { status: response.status, code: safeFailureCode(body) };
    if (response.status !== 502 || lastFailure.code !== "participant_session_unavailable") break;
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
  }
  assert.fail(`client handoff failed with HTTP ${lastFailure?.status ?? "unknown"} (${lastFailure?.code ?? "unavailable"})`);
}

async function availableLoopbackPorts(count) {
  const servers = [];
  try {
    for (let index = 0; index < count; index += 1) {
      const server = createServer();
      servers.push(server);
      await new Promise((resolveListen, rejectListen) => {
        server.once("error", rejectListen);
        server.listen(0, "127.0.0.1", resolveListen);
      });
    }
    return servers.map((server) => {
      const address = server.address();
      assert.ok(address && typeof address === "object");
      return address.port;
    });
  } finally {
    await Promise.all(servers.map((server) => new Promise((resolveClose) => server.close(resolveClose))));
  }
}

function parseCommandFailure(error) {
  if (!(error instanceof Error) || !("stdout" in error) || typeof error.stdout !== "string") return undefined;
  try { return JSON.parse(error.stdout); } catch { return undefined; }
}

function safeFailureCode(value) {
  if (value !== null && typeof value === "object" && !Array.isArray(value) && typeof value.code === "string" && /^[a-z_]+$/u.test(value.code)) return value.code;
  return "unavailable";
}

async function assertActionControlsDisabled(page) {
  const controls = page.locator("#archive-actions button, .archive-map button, .candidate-card button, .context-actions button");
  assert.ok(await controls.count() > 0, "Authorized briefing Projection rendered no game controls to gate");
  for (let index = 0; index < await controls.count(); index += 1) assert.equal(await controls.nth(index).isDisabled(), true, "Action enabled before active authorized Projection");
}

async function assertViewportFits(page, description) {
  const layout = await page.evaluate(() => {
    const verticalPosition = window.scrollY;
    window.scrollTo(document.documentElement.scrollWidth, verticalPosition);
    const horizontalScroll = window.scrollX;
    window.scrollTo(0, verticalPosition);
    return {
      horizontalScroll,
      reportedOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      offenders: [...document.querySelectorAll("body *")]
      .map((element) => ({
        selector: `${element.tagName.toLowerCase()}${element.id ? `#${element.id}` : ""}${element.classList.length ? `.${[...element.classList].join(".")}` : ""}`,
        rect: element.getBoundingClientRect(),
      }))
      .filter(({ rect }) => rect.left < -1 || rect.right > window.innerWidth + 1)
      .slice(0, 5)
      .map(({ selector, rect }) => ({ selector, left: rect.left, right: rect.right })),
    };
  });
  assert.ok(layout.horizontalScroll <= 1 && layout.offenders.length === 0, `${description} overflows horizontally: ${JSON.stringify(layout)}`);
}

async function assertPhoneControls(page) {
  const controls = page.locator("#archive-actions button:visible, .archive-map button:visible, .candidate-card button:visible");
  assert.ok(await controls.count() > 0, "phone layout exposes no visible game controls");
  for (let index = 0; index < await controls.count(); index += 1) {
    const box = await controls.nth(index).boundingBox();
    assert.ok(box !== null && box.height >= 40, `phone control ${index} is smaller than the 40px touch target`);
  }
}

async function startActivity(room) {
  const result = await lifecycleCli("room", "launch", room, "--timeout-seconds", "90");
  assert.equal(result.status, "complete");
  assert.equal(result.launch_assessment?.launch?.state, "launched");
}

async function runRoute(page) {
  let turns = readMetric(await page.locator("body").innerText(), "Turns remaining");
  for (const [actionType, payload] of witness.route) {
    debug(`route action ${actionType}`);
    const action = page.locator(`[data-action-type="${actionType}"]`).first();
    if (actionType.startsWith("stage_")) {
      if (actionType === "stage_move") {
        const destination = payload.destination[0].toUpperCase() + payload.destination.slice(1);
        await page.getByRole("button", { name: new RegExp(`Move to ${destination}`, "i") }).click();
      } else {
        const labels = {
          stage_use_verifier: /^Stage Run the catalog verifier; costs 1 turn and 1 power$/i,
          stage_inspect_records: /^Stage Inspect the intake evidence; costs 1 turn and 0 power$/i,
          stage_inspect_conservation: /^Stage Inspect the restoration evidence; costs 1 turn and 0 power$/i,
          stage_open_service_hatch: /^Stage Open the service hatch; costs 1 turn and 2 power$/i,
          stage_recover_candidate: new RegExp(`^Stage recovery of ${witness.candidate}; costs 1 turn and 0 power$`, "i"),
          stage_extract: /^Stage Extract from the archive; costs 1 turn and 0 power$/i,
        };
        const button = actionType === "stage_recover_candidate"
          ? page.locator(".candidate-card").filter({ hasText: new RegExp(witness.candidate, "i") }).getByRole("button", { name: labels[actionType] }).first()
          : page.locator(".context-actions").getByRole("button", { name: labels[actionType] }).first();
        if (actionType === "stage_recover_candidate") {
          const card = page.locator(".candidate-card").filter({ hasText: new RegExp(witness.candidate, "i") });
          assert.equal(await card.count(), 1, "Recovery must target the evidence-supported candidate");
          if (proofMode === "technical") assert.equal(await card.filter({ hasText: /Verified by the catalog instrument/i }).count(), 1, "Recovery must target the verifier-selected candidate");
          if (proofMode === "evidence-service") assert.equal(await card.filter({ hasText: /Evidence recommendation/i }).count(), 1, "Recovery must target the source-evidence recommendation");
        }
        if (await button.count()) await button.click();
        else if (await action.count()) await action.click();
        else throw new Error(`Action control missing for ${actionType}`);
      }
      await waitForEnabled(page, page.getByRole("button", { name: /commit turn/i }).first(), `${actionType} staged Action`);
      const afterStage = readMetric(await page.locator("body").innerText(), "Turns remaining");
      assert.equal(afterStage, turns, `${actionType} consumed a turn before Commit`);
    } else {
      await page.getByRole("button", { name: /commit turn/i }).first().click();
      const afterCommit = await waitForMetric(page, "Turns remaining", (value) => value === turns - 1, "Commit Turn");
      assert.equal(afterCommit, turns - 1, "Commit did not consume exactly one turn");
      turns = afterCommit;
    }
    await page.waitForTimeout(100);
  }
}

function debug(message) {
  if (process.env.WORLDSTREAM_ACCEPTANCE_DEBUG === "1") {
    process.stderr.write(`[archive +${Date.now() - acceptanceStartedAt}ms] ${message}\n`);
  }
}

async function waitForEnabled(page, locator, description) {
  for (let attempt = 0; attempt < 150; attempt += 1) {
    if (bridgeFailure) throw bridgeFailure;
    if (await locator.isEnabled()) return;
    await page.waitForTimeout(100);
  }
  const body = await page.locator("body").innerText();
  throw new Error(`${description} did not become enabled after the authoritative Projection:\n${body.slice(0, 4_000)}`);
}

async function waitForMetric(page, label, predicate, description) {
  for (let attempt = 0; attempt < 150; attempt += 1) {
    if (bridgeFailure) throw bridgeFailure;
    const text = await page.locator("body").innerText();
    try {
      const value = readMetric(text, label);
      if (predicate(value)) return value;
    } catch {}
    await page.waitForTimeout(100);
  }
  throw new Error(`${description} did not publish the expected ${label} metric`);
}

function readMetric(text, label) {
  const match = text.match(new RegExp(`${label}\\s*[:\\n]?\\s*(\\d+)`, "i"));
  if (!match) throw new Error(`${label} metric missing from Activity Client`);
  return Number(match[1]);
}

async function waitForText(page, pattern, description) {
  try {
    await page.getByText(pattern).first().waitFor({ timeout: 15_000 });
  } catch (error) {
    if (bridgeFailure) throw bridgeFailure;
    const body = (await page.locator("body").innerText()).slice(0, 2_000);
    throw new Error(`${description} did not render at ${new URL(page.url()).pathname}: ${body}`, { cause: error });
  }
}

function assertPrivatePayload(text, transport) {
  for (const key of forbidden) assert.equal(text.includes(`\"${key}\"`), false, `${key} leaked in ${transport}`);
  assert.equal(/\"[^\"]*(?:authentic|truth)[^\"]*\"\s*:/i.test(text), false, `private authenticity state key leaked in ${transport}`);
}
