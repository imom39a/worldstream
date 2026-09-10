#!/usr/bin/env node
// Local, provider-free Midnight Archive acceptance.  This intentionally keeps
// the browser on the real protected Activity Client route while the Runtime
// and Component Host own every state transition.
import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, stat, writeFile } from "node:fs/promises";
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
const proofMode = process.env.WORLDSTREAM_MIDNIGHT_ARCHIVE_PROOF_MODE ?? "agreement";
const lowReserveProof = proofMode === "low-reserve";
const scenarioId = lowReserveProof ? "low-reserve-v1" : "standard-v1";
const recoveryCandidateId = lowReserveProof ? "ledger-cobalt" : "ledger-violet";
const authorizedProjectionTimeoutMs = 45_000;
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
const agreementRoute = [
  ["stage_move", { destination: "records" }], ["commit_turn", {}],
  ["stage_use_verifier", {}], ["commit_turn", {}],
  ["stage_move", { destination: "conservation" }], ["commit_turn", {}],
  ["stage_accept_preservation_agreement", {}], ["commit_turn", {}],
  ["stage_prepare_collection", {}], ["commit_turn", {}],
  ["stage_energize_preservation_equipment", {}], ["commit_turn", {}],
  ["stage_move", { destination: "vault" }], ["commit_turn", {}],
  ["stage_recover_candidate", { candidate_id: recoveryCandidateId }], ["commit_turn", {}],
  ["stage_move", { destination: "conservation" }], ["commit_turn", {}],
  ["stage_move", { destination: "atrium" }], ["commit_turn", {}],
  ["stage_extract", {}], ["commit_turn", {}],
];
const bothObjectivesRoute = [
  ...agreementRoute.slice(0, 18),
  ["stage_move", { destination: "records" }], ["commit_turn", {}],
  ["stage_move", { destination: "plant" }], ["commit_turn", {}],
  ["stage_protect_source_record", {}], ["commit_turn", {}],
  ["stage_move", { destination: "records" }], ["commit_turn", {}],
  ["stage_move", { destination: "atrium" }], ["commit_turn", {}],
  ["stage_extract", {}], ["commit_turn", {}],
];
const lowReserveRoute = [
  ["stage_move", { destination: "records" }], ["commit_turn", {}],
  ["stage_inspect_records", {}], ["commit_turn", {}],
  ["stage_move", { destination: "conservation" }], ["commit_turn", {}],
  ["stage_inspect_conservation", {}], ["commit_turn", {}],
  ["stage_accept_preservation_agreement", {}], ["commit_turn", {}],
  ["stage_prepare_collection", {}], ["commit_turn", {}],
  ["stage_energize_preservation_equipment", {}], ["commit_turn", {}],
  ["stage_move", { destination: "vault" }], ["commit_turn", {}],
  ["stage_recover_candidate", { candidate_id: recoveryCandidateId }], ["commit_turn", {}],
  ["stage_move", { destination: "conservation" }], ["commit_turn", {}],
  ["stage_move", { destination: "records" }], ["commit_turn", {}],
  ["stage_move", { destination: "plant" }], ["commit_turn", {}],
  ["stage_protect_source_record", {}], ["commit_turn", {}],
  ["stage_move", { destination: "records" }], ["commit_turn", {}],
  ["stage_move", { destination: "atrium" }], ["commit_turn", {}],
  ["stage_extract", {}], ["commit_turn", {}],
];
const liveMiraProof = proofMode === "mira-live";
const crewLiveProof = proofMode === "crew-live";
const unavailableLiveProof = proofMode === "unavailable-live";
const liveSpecialistRoles = crewLiveProof || unavailableLiveProof ? ["mira", "jonah"] : liveMiraProof ? ["mira"] : [];
const liveCompanionProof = liveSpecialistRoles.length > 0;
const clientReleaseGeneration = "v11";
const archiveClientRelease = process.env.WORLDSTREAM_MIDNIGHT_ARCHIVE_CLIENT_RELEASE;
const witness = lowReserveProof
  ? { route: lowReserveRoute, turns: 16, turnsRemaining: 0, powerRemaining: 0, candidate: "Cobalt Ledger", checks: ["sixteen_turn_low_reserve_witness", "sourced_evidence_disclosure", "authored_agreement_honored", "collection_preserved", "source_record_protected"] }
  : proofMode === "technical"
  ? { route: technicalRoute, turns: 10, turnsRemaining: 6, powerRemaining: 0, candidate: "Violet Ledger", checks: ["ten_turn_phone_witness", "verifier_selected_candidate"] }
  : proofMode === "evidence-service"
    ? { route: evidenceServiceRoute, turns: 12, turnsRemaining: 4, powerRemaining: 1, candidate: "Violet Ledger", checks: ["twelve_turn_evidence_service_witness", "sourced_evidence_disclosure"] }
    : proofMode === "agreement"
      ? { route: agreementRoute, turns: 11, turnsRemaining: 5, powerRemaining: 1, candidate: "Violet Ledger", checks: ["eleven_turn_powered_agreement_witness", "authored_agreement_honored", "collection_preserved"] }
      : proofMode === "both-objectives"
        ? { route: bothObjectivesRoute, turns: 15, turnsRemaining: 1, powerRemaining: 0, candidate: "Violet Ledger", checks: ["fifteen_turn_both_objectives_witness", "authored_agreement_honored", "collection_preserved", "source_record_protected"] }
        : liveMiraProof
          ? { route: [], turns: 5, turnsRemaining: 11, powerRemaining: 3, candidate: "Violet Ledger", checks: ["separate_human_and_mira_memberships", "external_mira_runner_once", "one_recorded_mira_step_per_human_turn", "terminal_partial_outcome"] }
          : crewLiveProof
            ? { route: [], turns: 11, turnsRemaining: 5, powerRemaining: 2, candidate: "Violet Ledger", checks: ["separate_full_party_memberships", "external_specialist_runners_once_each", "one_recorded_specialist_step_per_human_turn", "field_assay_verified_candidate", "full_crew_extraction"] }
            : unavailableLiveProof
              ? { route: [], turns: 1, turnsRemaining: 15, powerRemaining: 3, candidate: "Violet Ledger", checks: ["separate_specialist_credentials", "bounded_provider_dispositions", "explicit_reopen_after_expiry", "defer_wait_preserves_standing_plan", "reload_preserves_authoritative_plan"] }
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
const specialistRuntime = new Map([
  ["mira", { name: "Mira", module: "examples.midnight_archive.run_mira", membershipFile: undefined, runnerFile: undefined, invocations: 0, process: undefined }],
  ["jonah", { name: "Jonah", module: "examples.midnight_archive.run_jonah", membershipFile: undefined, runnerFile: undefined, invocations: 0, process: undefined }],
]);
let replayRequests = 0;
let browserActionRequests = 0;
let browserActionResponses = 0;
let latestBrowserActionResponse;

try {
  if (liveCompanionProof) {
    assert.ok(process.env.WORLDSTREAM_MIDNIGHT_ARCHIVE_BUNDLE, `${proofMode} proof requires WORLDSTREAM_MIDNIGHT_ARCHIVE_BUNDLE for the exact approved Companion Pack`);
    assert.ok(archiveClientRelease, `${proofMode} proof requires WORLDSTREAM_MIDNIGHT_ARCHIVE_CLIENT_RELEASE for the exact approved Companion Client release`);
  }
  const bundle = await archiveBundle();
  if (process.env.WORLDSTREAM_ACCEPTANCE_REUSE_BINARIES !== "1") await buildExecutables();
  debug("executables ready");
  await mkdir(join(root, "bin"), { mode: 0o700 });
  for (const binary of binaries) await copyFile(join(workspace, "target/release", binary), join(root, "bin", binary));
  await preflightExecutables();
  const bundleIdentity = await inspectBundle(bundle);
  debug("binary preflight and bundle inspection complete");

  const releasePath = archiveClientRelease === undefined
    ? join(workspace, "config/activity-clients/releases/midnight-archive-web-v11.json")
    : resolve(workspace, archiveClientRelease);
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
    deployment_id: `first-party-midnight-archive-web-${clientReleaseGeneration}`,
    client_id: release.client_id,
    release_digest: release.release_digest,
    trust_level: "externally_trusted",
    surfaces: [{ surface_id: standaloneSurface.surface_id, launch_url: `${host.origin}${standaloneSurface.entrypoint}` }],
  };
  bootstrap.deployments.push(archiveDeployment);
  bootstrap.bindings = bootstrap.bindings.filter((binding) => binding.deployment_id === "first-party-inspector-web-v2");
  bootstrap.bindings.push({
    schema: "worldstream/client-binding/v1",
    binding_id: `midnight-archive-0-1-lead-web-${clientReleaseGeneration}`,
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
    configuration: { scenario_id: scenarioId },
    seats: [
      {
        label: "lead",
        role: "lead",
        required: true,
        display_name: "Lead",
        principal: { reference: "lead", kind: "human" },
      },
      ...liveSpecialistRoles.map((role) => ({
        label: role,
        role,
        required: true,
        display_name: specialist(role).name,
        principal: { reference: role, kind: "agent" },
        assignment: { mode: "external" },
      })),
    ],
    operator_view: false,
  }), { mode: 0o600 });
  await cli("room", "validate", "--file", setup);
  const created = await createRoom(setup);
  const operation = created.room_operation.operation;
  const room = created.room_operation.room_id;
  const credential = await readFile(join(state, "control-access.v1"));
  if (liveCompanionProof) {
    assertSeparateSpecialistIdentities(await readTaskSetupStatus(operation, credential), liveSpecialistRoles);
    const credentials = join(root, "credentials");
    await mkdir(credentials, { mode: 0o700 });
    const identities = [];
    for (const role of liveSpecialistRoles) {
      const runtime = specialist(role);
      runtime.membershipFile = join(credentials, `${role}-membership.json`);
      runtime.runnerFile = join(credentials, `${role}-runner.json`);
      await cli("client", "export-credentials", "--operation", operation, "--seat", role, "--output", runtime.membershipFile);
      await cli("runner", "export-credentials", "--operation", operation, "--seat", role, "--output", runtime.runnerFile);
      identities.push(await assertSpecialistCredentialPair(role, operation, room, bundleIdentity.revision_digest));
    }
    assertIndependentSpecialistCredentials(identities);
    debug(`separate ${liveSpecialistRoles.join(" and ")} Membership and Runner credentials exported`);
  }
  const handoff = await issueClientHandoff(operation, credential);
  assert.equal(new URL(handoff.client_url).pathname, standaloneSurface.entrypoint);
  debug(`${liveCompanionProof ? "companion" : "solo"} Room and client handoff ready`);

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
        browserActionRequests += 1;
        const action = JSON.parse(browserRequest.postData() ?? "null");
        if (action?.action_type === "stage_recover_candidate") submittedRecoveryPayload = action.payload;
      }
      if (target.pathname.endsWith("/session:replay")) replayRequests += 1;
      const response = await requestRoute.fetch({ url: target.href });
      const body = await response.body();
      assertPrivatePayload(body.toString("utf8"), "controller response");
      debug(`browser controller request: ${target.pathname} -> ${response.status()}`);
      await requestRoute.fulfill({ response, body });
      if (target.pathname.endsWith("/session:act")) {
        browserActionResponses += 1;
        latestBrowserActionResponse = {
          request: browserActionRequests,
          status: response.status(),
          state: responseActionState(body),
          code: responseErrorCode(body),
        };
      }
    } catch (error) {
      bridgeFailure ??= error instanceof Error && error.code === "ERR_ASSERTION"
        ? error
        : new Error(`browser controller bridge failed for ${target.pathname}`);
      const detail = (error instanceof Error ? error.message : String(error)).split("\n", 1)[0];
      debug(`browser controller request failed: ${target.pathname} (${detail.slice(0, 200)})`);
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
  await page.getByText(/The Room is synchronized for readiness/i).first().waitFor({ timeout: authorizedProjectionTimeoutMs });
  debug("briefing Projection synchronized");
  await assertActionControlsDisabled(page);
  await assertViewportFits(page, "desktop briefing");
  if (liveCompanionProof) {
    for (const role of liveSpecialistRoles) {
      startSpecialistRunner(role, bundleIdentity.revision_digest, unavailableLiveProof && role === "mira"
        ? { providerMode: "delayed", providerDelaySeconds: 5 }
        : unavailableLiveProof && role === "jonah"
          ? { providerMode: "rejected" }
          : undefined);
    }
    await waitForSpecialistRunnerReadiness(room, liveSpecialistRoles);
  }
  await startActivity(room);
  await page.getByText("Projection current", { exact: true }).waitFor({ timeout: authorizedProjectionTimeoutMs });
  debug("Activity Start committed");
  const activeBody = await page.locator("body").innerText();
  assert.match(activeBody, new RegExp(lowReserveProof ? "Low Reserve" : "Standard", "i"));
  assert.match(activeBody, new RegExp(`Power reserve\\s*${lowReserveProof ? 2 : 3}\\s*/\\s*${lowReserveProof ? 2 : 3}`, "i"));
  if (crewLiveProof) {
    await page.setViewportSize({ width: 390, height: 844 });
    await assertViewportFits(page, "phone full-party mission");
    await assertPhoneControls(page);
    await runLiveCrewWitness(page, { room, revision: bundleIdentity.revision_digest });
  } else if (liveMiraProof) {
    await page.setViewportSize({ width: 390, height: 844 });
    await assertViewportFits(page, "phone Mira mission");
    await assertPhoneControls(page);
    await runLiveMiraWitness(page, { room, revision: bundleIdentity.revision_digest });
  } else if (unavailableLiveProof) {
    await page.setViewportSize({ width: 390, height: 844 });
    await assertViewportFits(page, "phone unavailable-companion mission");
    await assertPhoneControls(page);
    await runUnavailableLiveWitness(page, { revision: bundleIdentity.revision_digest });
  } else {
  await page.setViewportSize({ width: 390, height: 844 });
  await assertViewportFits(page, "phone mission");
  await assertPhoneControls(page);
  await runRoute(page);
  await page.getByRole("heading", { name: "The authentic ledger is out", exact: true }).waitFor({ timeout: authorizedProjectionTimeoutMs });
  assert.deepEqual(submittedRecoveryPayload, { candidate_id: recoveryCandidateId }, "Recovery must submit the canonical candidate payload");
  const body = await page.locator("body").innerText();
  assert.match(body, new RegExp(`Turns remaining\\s*${witness.turnsRemaining}`, "i"));
  assert.match(body, new RegExp(`Power reserve\\s*${witness.powerRemaining}`, "i"));
  if (proofMode === "agreement" || proofMode === "both-objectives" || lowReserveProof) {
    assert.match(body, /Agreement\s*honored/i);
    assert.match(body, /Collection preserved\s*Yes/i);
  }
  if (proofMode === "both-objectives" || lowReserveProof) assert.match(body, /Source record protected\s*Yes/i);
  const replayButton = page.getByRole("button", { name: /replay/i });
  assert.equal(await replayButton.count(), 1, "Replay acceptance blocked: the Archive client exposes no Replay control or authorized replay endpoint seam");
  await replayButton.click();
  await page.getByRole("button", { name: "Replay verified", exact: true }).waitFor({ timeout: authorizedProjectionTimeoutMs });
  }
  if (bridgeFailure) throw bridgeFailure;
  receipt = { status: "passed", pack: "worldstream.midnight-archive@0.1.0", bundle_digest: bundleIdentity.bundle_digest, revision_digest: bundleIdentity.revision_digest, scenario_id: scenarioId, room, turns: witness.turns, turns_remaining: witness.turnsRemaining, power_remaining: witness.powerRemaining, provider_calls: 0, runner_invocations: runnerInvocationReceipt(), checks: ["briefing_projection_before_activity_start", "actions_disabled_until_sync", "authored_scenario_and_initial_power_rendered", "desktop_and_phone_layouts", ...witness.checks, ...(unavailableLiveProof ? ["authoritative_reconnect_without_runner_or_policy_rerun"] : liveCompanionProof ? ["authorized_replay_without_runner_or_policy_rerun"] : ["canonical_recovery_payload", "beginning_of_turn_gate_boundary", "terminal_success", "authorized_replay"]), "private_authenticity_non_leakage"] };
} catch (error) {
  failure = error;
} finally {
  const cleanupFailures = [];
  for (const cleanup of [async () => { if (page) await page.unrouteAll({ behavior: "ignoreErrors" }); if (browser) await browser.close(); }, stopSpecialistRunnersIfActive, async () => { if (started) await stopIfAvailable("stop"); }, async () => { if (started) await stopIfAvailable("controller-stop"); }, async () => { if (host) await host.close(); }]) {
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
    "packs/midnight-archive/evidence/production-proof-0.1.0-dialogue.json",
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

async function readTaskSetupStatus(operation, credential) {
  const response = await fetch(`http://${controllerAddress}/api/v1/task-setups/${operation}`, {
    headers: { authorization: `Bearer ${credential.subarray(16).toString("hex")}` },
  });
  assert.equal(response.ok, true, "companion proof could not read the persisted Task Setup status");
  const status = await response.json();
  assert.equal(status?.draft_id, operation, "Task Setup status did not match the created operation");
  return status;
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
  const controls = page.locator("#archive-actions button, .archive-map button, .candidate-card button, .context-actions button, [data-testid$='-crew-card'] button");
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
  const controls = page.locator("#archive-actions button:visible, .archive-map button:visible, .candidate-card button:visible, [data-testid$='-crew-card'] button:visible");
  assert.ok(await controls.count() > 0, "phone layout exposes no visible game controls");
  for (let index = 0; index < await controls.count(); index += 1) {
    const box = await controls.nth(index).boundingBox();
    assert.ok(box !== null && box.height >= 40, `phone control ${index} is smaller than the 40px touch target`);
  }
}

async function startActivity(room) {
  let lastError;
  for (let attempt = 0; attempt < 4; attempt += 1) {
    try {
      const result = await lifecycleCli("room", "launch", room, "--timeout-seconds", "90");
      assert.equal(result.status, "complete");
      assert.equal(result.launch_assessment?.launch?.state, "launched");
      return;
    } catch (error) {
      lastError = error;
      if (parseCommandFailure(error)?.code !== "room_launch_unconfirmed") throw error;
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
    }
  }
  throw lastError;
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
          stage_accept_preservation_agreement: /^Stage Accept the preservation agreement; costs 1 turn and 0 power$/i,
          stage_prepare_collection: /^Stage Prepare the threatened collection; costs 1 turn and 0 power$/i,
          stage_energize_preservation_equipment: /^Stage Energize preservation equipment; costs 1 turn and 1 power$/i,
          stage_open_service_hatch: /^Stage Open the service hatch; costs 1 turn and 2 power$/i,
          stage_recover_candidate: new RegExp(`^Stage recovery of ${witness.candidate}; costs 1 turn and 0 power$`, "i"),
          stage_protect_source_record: /^Stage Protect the source's identifying record; costs 1 turn and 1 power$/i,
          stage_extract: /^Stage Extract from the archive; costs 1 turn and 0 power$/i,
        };
        const button = actionType === "stage_recover_candidate"
          ? page.locator(".candidate-card").filter({ hasText: new RegExp(witness.candidate, "i") }).getByRole("button", { name: labels[actionType] }).first()
          : page.locator(".context-actions").getByRole("button", { name: labels[actionType] }).first();
        if (actionType === "stage_recover_candidate") {
          const card = page.locator(".candidate-card").filter({ hasText: new RegExp(witness.candidate, "i") });
          assert.equal(await card.count(), 1, "Recovery must target the evidence-supported candidate");
          if (proofMode !== "evidence-service" && !lowReserveProof) assert.equal(await card.filter({ hasText: /Verified by the catalog instrument/i }).count(), 1, "Recovery must target the verifier-selected candidate");
          if (proofMode === "evidence-service" || lowReserveProof) assert.equal(await card.filter({ hasText: /Evidence recommendation/i }).count(), 1, "Recovery must target the source-evidence recommendation");
        }
        if (await button.count()) await button.click();
        else if (await action.count()) await action.click();
        else throw new Error(`Action control missing for ${actionType}`);
      }
      if (actionType === "stage_extract") await prepareAndAcknowledgeExtraction(page, [], false);
      await waitForEnabled(page, page.getByRole("button", { name: /commit turn/i }).first(), `${actionType} staged Action`);
      const afterStage = readMetric(await page.locator("body").innerText(), "Turns remaining");
      assert.equal(afterStage, turns, `${actionType} consumed a turn before Commit`);
      if (actionType === "stage_energize_preservation_equipment") {
        const vault = page.getByRole("button", { name: /Move to Vault/i });
        assert.equal(await vault.isDisabled(), true, "staged preservation opened the Conservation gate before Commit Turn");
      }
    } else {
      await page.getByRole("button", { name: /commit turn/i }).first().click();
      const afterCommit = await waitForMetric(page, "Turns remaining", (value) => value === turns - 1, "Commit Turn");
      assert.equal(afterCommit, turns - 1, "Commit did not consume exactly one turn");
      turns = afterCommit;
    }
    await page.waitForTimeout(100);
  }
}

function assertSeparateSpecialistIdentities(operation, roles) {
  const seats = operation?.seats;
  assert.ok(Array.isArray(seats), "companion proof requires the persisted setup seats");
  const lead = seats.find((seat) => seat?.seat_id === "lead");
  const specialists = roles.map((role) => seats.find((seat) => seat?.seat_id === role));
  assert.ok(lead && specialists.every(Boolean), `companion proof requires Lead and ${roles.join(" and ")} setup seats`);
  assert.equal(lead.principal_kind, "human", "Lead must be a human Principal");
  assert.match(lead.principal_id ?? "", /^[0-9A-HJKMNP-TV-Z]{26}$/u, "Lead Principal was not provisioned");
  assert.match(lead.member_id ?? "", /^[0-9A-HJKMNP-TV-Z]{26}$/u, "Lead Membership was not provisioned");
  for (const [index, role] of roles.entries()) {
    const seat = specialists[index];
    const name = specialist(role).name;
    assert.equal(seat.seat_id, role, `${name} must occupy the exact ${role} seat`);
    assert.equal(seat.role, role, `${name} must have the exact ${role} Role`);
    assert.equal(seat.principal_kind, "agent", `${name} must be an agent Principal`);
    assert.match(seat.principal_id ?? "", /^[0-9A-HJKMNP-TV-Z]{26}$/u, `${name} Principal was not provisioned`);
    assert.match(seat.member_id ?? "", /^[0-9A-HJKMNP-TV-Z]{26}$/u, `${name} Membership was not provisioned`);
  }
  assert.equal(new Set([lead.principal_id, ...specialists.map((seat) => seat.principal_id)]).size, roles.length + 1, "every crew seat must use a separate Principal");
  assert.equal(new Set([lead.member_id, ...specialists.map((seat) => seat.member_id)]).size, roles.length + 1, "every crew seat must use a separate Membership");
}

async function assertSpecialistCredentialPair(role, operation, room, revision) {
  const runtime = specialist(role);
  const { name, membershipFile, runnerFile } = runtime;
  assert.ok(membershipFile && runnerFile, `${name} credential paths are unavailable`);
  assert.notEqual(membershipFile, runnerFile, `${name} Membership and Runner credentials need separate files`);
  const [membershipStat, runnerStat, membershipBytes, runnerBytes] = await Promise.all([
    stat(membershipFile), stat(runnerFile), readFile(membershipFile), readFile(runnerFile),
  ]);
  assert.equal(membershipStat.isFile(), true, `${name} Membership credential export is not a file`);
  assert.equal(runnerStat.isFile(), true, `${name} Runner credential export is not a file`);
  assert.equal(membershipStat.mode & 0o077, 0, `${name} Membership credential is not owner-only`);
  assert.equal(runnerStat.mode & 0o077, 0, `${name} Runner credential is not owner-only`);
  const membership = JSON.parse(membershipBytes.toString("utf8"));
  const runner = JSON.parse(runnerBytes.toString("utf8"));
  const pack = { id: "worldstream.midnight-archive", version: "0.1.0", digest: revision };
  assert.deepEqual(
    {
      schema: membership.schema, operation: membership.operation, seat: membership.seat,
      room_id: membership.room_id, role: membership.role, pack: membership.pack, scopes: membership.scopes,
    },
    {
      schema: "worldstream/membership-credentials/v1", operation, seat: role, room_id: room, role, pack,
      scopes: ["room:attach", "room:act", "room:observe_member"],
    },
    `${name} Membership credential has the wrong scope`,
  );
  assert.deepEqual(
    {
      schema: runner.schema, operation: runner.operation, seat: runner.seat,
      pack: runner.pack, permitted_memberships: runner.permitted_memberships, scopes: runner.scopes,
    },
    {
      schema: "worldstream/runner-credentials/v1", operation, seat: role, pack,
      permitted_memberships: [{ room_id: membership.room_id, member_id: membership.member_id }],
      scopes: ["activation:offer_receive", "activation:claim", "activation:complete"],
    },
    `${name} Runner credential has the wrong singleton scope`,
  );
  assert.equal(runner.owner_principal_id, membership.principal_id, `Runner must belong to ${name}'s Agent Principal`);
  assert.match(runner.runner_id ?? "", /^[0-9A-HJKMNP-TV-Z]{26}$/u, `${name} Runner identity was not provisioned`);
  assert.notEqual(membership.bearer, runner.bearer, `${name} Membership and Runner authorities must be distinct`);
  return { role, principal: membership.principal_id, member: membership.member_id, runner: runner.runner_id, membershipBearer: membership.bearer, runnerBearer: runner.bearer };
}

function assertIndependentSpecialistCredentials(identities) {
  for (const key of ["principal", "member", "runner", "membershipBearer", "runnerBearer"]) {
    assert.equal(new Set(identities.map((identity) => identity[key])).size, identities.length, `specialist ${key} authority must be unique per seat`);
  }
  assert.equal(new Set(identities.flatMap((identity) => [identity.membershipBearer, identity.runnerBearer])).size, identities.length * 2, "every specialist authority bearer must be distinct");
}

async function runLiveMiraWitness(page, { room, revision }) {
  const runtime = specialist("mira");
  assert.ok(runtime.membershipFile && runtime.runnerFile, "Mira credential exports are unavailable");
  const mira = page.getByTestId("mira-crew-card");
  await mira.waitFor({ timeout: authorizedProjectionTimeoutMs });
  await clickLiveAction(
    page,
    page.getByRole("button", { name: "Assign Investigate Records with 0 power allowance" }),
    "assign_mira_task",
  );
  await clickLiveAction(
    page,
    page.getByRole("button", { name: /Move to Records/i }),
    "stage_move",
  );
  await clickLiveAction(
    page,
    page.getByRole("button", { name: "Request a plan" }),
    "request_mira_plan",
  );
  // A fast Runner reply can share a delivery batch with the request. The client
  // correctly folds that batch to its final ready state without rendering the
  // transient waiting projection.
  await mira.getByText(/preparing a bounded plan|ready\s*·\s*revision/i).first()
    .waitFor({ timeout: authorizedProjectionTimeoutMs });

  const runnerResult = await waitForSpecialistRunnerResult("mira", revision);
  assert.deepEqual(runnerResult, { status: "handled", submitted_actions: 1 }, "Mira Runner did not handle exactly one planning Activation");
  await mira.getByText(/ready\s*·\s*revision/i).waitFor({ timeout: authorizedProjectionTimeoutMs });
  await assertMiraProgress(mira, 0, 3, "Mira plan acceptance must not advance an Activity Turn");

  let turns = readMetric(await page.locator("body").innerText(), "Turns remaining");
  for (const expectedStep of [1, 2, 3]) {
    await clickLiveAction(
      page,
      page.getByRole("button", { name: "Prepare next eligible step" }),
      "prepare_mira_contribution",
    );
    await mira.getByText(/Fenced to turn/i).waitFor({ timeout: authorizedProjectionTimeoutMs });
    await clickLiveAction(
      page,
      page.getByRole("button", { name: /commit turn/i }).first(),
      "commit_turn",
    );
    turns = await waitForMetric(page, "Turns remaining", (value) => value === turns - 1, `Mira companion turn ${expectedStep}`);
    await assertMiraProgress(mira, expectedStep, 3, `Mira may advance one recorded step on turn ${expectedStep}`);
    if (expectedStep === 1) await waitForMiraText(mira, /Location\s*Records/i, "Mira recorded location");
    if (expectedStep === 3) await mira.getByText(/Records evidence is disclosed on the candidate board/i).waitFor({ timeout: authorizedProjectionTimeoutMs });
    if (expectedStep < 3) {
      await clickLiveAction(
        page,
        page.getByRole("button", { name: /Stage Wait for one turn/i }),
        "stage_wait",
      );
    }
  }
  assert.equal(turns, 13, "three committed human turns must consume exactly three turns");
  assert.equal(runtime.invocations, 1, "Mira Runner must stop after its one Activation");

  await clickLiveAction(
    page,
    page.getByRole("button", { name: "Follow lead" }),
    "set_mira_follow",
  );
  await clickLiveAction(
    page,
    page.getByRole("button", { name: /Move to Atrium/i }),
    "stage_move",
  );
  await clickLiveAction(
    page,
    page.getByRole("button", { name: /commit turn/i }).first(),
    "commit_turn",
  );
  turns = await waitForMetric(page, "Turns remaining", (value) => value === 12, "lead and Mira return to Atrium");
  await waitForMiraText(mira, /Location\s*Atrium/i, "Mira followed one legal edge to Atrium");
  await clickLiveAction(
    page,
    page.getByRole("button", { name: /Stage Extract from the archive/i }),
    "stage_extract",
  );
  await prepareAndAcknowledgeExtraction(page, [], false);
  await clickLiveAction(
    page,
    page.getByRole("button", { name: /commit turn/i }).first(),
    "commit_turn",
  );
  turns = await waitForMetric(page, "Turns remaining", (value) => value === 11, "terminal extraction");
  assert.equal(turns, 11, "the Mira witness must finish after exactly five committed turns");
  await page.getByRole("heading", { name: "You escaped empty-handed", exact: true }).waitFor({ timeout: authorizedProjectionTimeoutMs });

  const actionsBeforeReplay = browserActionRequests;
  const replaysBefore = replayRequests;
  const runnersBeforeReplay = runtime.invocations;
  const replayButton = page.getByRole("button", { name: /replay/i });
  assert.equal(await replayButton.count(), 1, "Replay acceptance blocked: the Archive client exposes no Replay control or authorized replay endpoint seam");
  await replayButton.click();
  await page.getByRole("button", { name: "Replay verified", exact: true }).waitFor({ timeout: authorizedProjectionTimeoutMs });
  assert.equal(replayRequests, replaysBefore + 1, "the browser did not issue one authorized Replay request");
  assert.equal(browserActionRequests, actionsBeforeReplay, "Replay must not submit a new participant Action");
  assert.equal(runtime.invocations, runnersBeforeReplay, "Replay must not rerun Mira's external Runner or policy");
  assert.equal(room.length > 0, true, "Mira witness lost its Room identity");
}

async function runLiveCrewWitness(page, { room, revision }) {
  const mira = specialistCard(page, "mira");
  const jonah = specialistCard(page, "jonah");
  await Promise.all([
    mira.waitFor({ timeout: authorizedProjectionTimeoutMs }),
    jonah.waitFor({ timeout: authorizedProjectionTimeoutMs }),
  ]);

  await clickSpecialistAction(page, "mira", "set_mira_follow", "Follow lead");
  await clickSpecialistAction(page, "jonah", "assign_jonah_task", "Assign Open service hatch with 1 power allowance");
  await clickLiveAction(page, page.getByRole("button", { name: /Move to Records/i }), "stage_move");
  await clickSpecialistAction(page, "jonah", "request_jonah_plan", "Request a plan");
  await waitForSpecialistText(jonah, /preparing a bounded plan|ready\s*·\s*revision/i, "Jonah planning status");
  const jonahResult = await waitForSpecialistRunnerResult("jonah", revision);
  assert.deepEqual(jonahResult, { status: "handled", submitted_actions: 1 }, "Jonah Runner did not handle exactly one planning Activation");
  await waitForSpecialistText(jonah, /ready\s*·\s*revision/i, "Jonah accepted plan");
  await assertSpecialistProgress(jonah, "Jonah", 0, 3, "Jonah plan acceptance must not advance an Activity Turn");

  let turns = readMetric(await page.locator("body").innerText(), "Turns remaining");
  for (const expectedStep of [1, 2, 3]) {
    await clickSpecialistAction(page, "jonah", "prepare_jonah_contribution", "Prepare next eligible step");
    await waitForSpecialistText(jonah, /Fenced to turn/i, `Jonah step ${expectedStep} preparation`);
    turns = await commitLiveTurn(page, turns, `Jonah companion turn ${expectedStep}`);
    await assertSpecialistProgress(jonah, "Jonah", expectedStep, 3, `Jonah may advance one recorded step on turn ${expectedStep}`);
    if (expectedStep === 1) {
      await waitForSpecialistText(jonah, /Location\s*Records/i, "Jonah moved to Records");
      await waitForSpecialistText(mira, /Location\s*Records/i, "Mira followed the lead to Records");
      await clickLiveAction(page, page.getByRole("button", { name: /Move to Plant/i }), "stage_move");
    } else if (expectedStep === 2) {
      await waitForSpecialistText(jonah, /Location\s*Plant/i, "Jonah moved to Plant");
      await waitForSpecialistText(mira, /Location\s*Plant/i, "Mira followed the lead to Plant");
      await clickLiveAction(page, page.getByRole("button", { name: /Stage Wait for one turn/i }), "stage_wait");
    }
  }
  assert.equal(turns, 13, "Jonah's three-step plan must share exactly three human turns");
  await waitForSpecialistText(jonah, /opened the Plant service hatch/i, "Jonah hatch contribution");
  assert.equal(readMetric(await page.locator("body").innerText(), "Power reserve"), 2, "Jonah's hatch method must spend exactly one power");
  assert.equal(specialist("jonah").invocations, 1, "Jonah Runner must stop after its one Activation");

  await clickLiveAction(page, page.getByRole("button", { name: /Move to Vault/i }), "stage_move");
  turns = await commitLiveTurn(page, turns, "lead and Mira enter the Vault");
  await waitForSpecialistText(mira, /Location\s*Vault/i, "Mira followed the lead into the Vault");
  await waitForSpecialistText(jonah, /Location\s*Plant/i, "Jonah held at the Plant after hatch work");

  await clickSpecialistAction(page, "mira", "assign_mira_task", "Assign Field assay with 0 power allowance");
  await clickSpecialistAction(page, "mira", "request_mira_plan", "Request a plan");
  await waitForSpecialistText(mira, /preparing a bounded plan|ready\s*·\s*revision/i, "Mira planning status");
  const miraResult = await waitForSpecialistRunnerResult("mira", revision);
  assert.deepEqual(miraResult, { status: "handled", submitted_actions: 1 }, "Mira Runner did not handle exactly one planning Activation");
  await waitForSpecialistText(mira, /ready\s*·\s*revision/i, "Mira accepted plan");
  await assertSpecialistProgress(mira, "Mira", 0, 2, "Mira assay plan acceptance must not advance an Activity Turn");

  for (const expectedStep of [1, 2]) {
    await clickLiveAction(page, page.getByRole("button", { name: /Stage Wait for one turn/i }), "stage_wait");
    await clickSpecialistAction(page, "mira", "prepare_mira_contribution", "Prepare next eligible step");
    await waitForSpecialistText(mira, /Fenced to turn/i, `Mira assay step ${expectedStep} preparation`);
    turns = await commitLiveTurn(page, turns, `Mira assay turn ${expectedStep}`);
    await assertSpecialistProgress(mira, "Mira", expectedStep, 2, `Mira may advance one assay step on turn ${expectedStep}`);
  }
  assert.equal(turns, 10, "Vault entry and two assay commits must leave ten turns");
  await waitForSpecialistText(mira, /Field assay disclosed:\s*Violet Ledger\s*·\s*verified/i, "Mira verified the Violet Ledger");
  assert.equal(specialist("mira").invocations, 1, "Mira Runner must stop after its one Activation");

  const violet = page.locator(".candidate-card").filter({ hasText: /Violet Ledger/i });
  assert.equal(await violet.count(), 1, "the assay-supported Violet Ledger card is missing");
  await clickLiveAction(page, violet.getByRole("button", { name: /^Stage recovery of Violet Ledger; costs 1 turn and 0 power$/i }), "stage_recover_candidate");
  turns = await commitLiveTurn(page, turns, "recover the assay-verified Violet Ledger");
  assert.deepEqual(submittedRecoveryPayload, { candidate_id: recoveryCandidateId }, "Recovery must submit the canonical assay-supported candidate payload");

  await clickSpecialistAction(page, "mira", "set_mira_follow", "Follow lead");
  await clickSpecialistAction(page, "jonah", "set_jonah_regroup", "Regroup at Atrium");
  for (const [destination, expectedTurns] of [["Plant", 8], ["Records", 7], ["Atrium", 6]]) {
    await clickLiveAction(page, page.getByRole("button", { name: new RegExp(`Move to ${destination}`, "i") }), "stage_move");
    turns = await commitLiveTurn(page, turns, `full party return through ${destination}`);
    assert.equal(turns, expectedTurns, `return through ${destination} consumed an unexpected turn`);
  }
  await waitForSpecialistText(mira, /Location\s*Atrium/i, "Mira followed the lead back to Atrium");
  await waitForSpecialistText(jonah, /Location\s*Atrium/i, "Jonah regrouped at Atrium");

  await clickLiveAction(page, page.getByRole("button", { name: /Stage Extract from the archive/i }), "stage_extract");
  await prepareAndAcknowledgeExtraction(page, [], true);
  turns = await commitLiveTurn(page, turns, "full-party terminal extraction");
  assert.equal(turns, 5, "the full-party witness must finish after exactly eleven committed turns");
  assert.equal(readMetric(await page.locator("body").innerText(), "Power reserve"), 2, "the full-party witness must finish with two power");
  await page.getByRole("heading", { name: "The authentic ledger is out", exact: true }).waitFor({ timeout: authorizedProjectionTimeoutMs });
  const body = await page.locator("body").innerText();
  assert.match(body, /Turns used\s*11 of 16/i);
  assert.match(body, /Crew extracted\s*Lead, Mira, Jonah/i);
  assert.match(body, /Crew left behind\s*No one/i);
  assert.match(body, /Completed specialist work/i);
  assert.match(body, /Jonah\s*·\s*open service hatch\s*·\s*turn 3/i);
  assert.match(body, /Mira\s*·\s*collect assay sample\s*·\s*turn 5/i);
  assert.match(body, /Mira\s*·\s*complete field assay\s*·\s*turn 6/i);

  const actionsBeforeReplay = browserActionRequests;
  const replaysBefore = replayRequests;
  const invocationsBeforeReplay = liveSpecialistRoles.map((role) => specialist(role).invocations);
  const replayButton = page.getByRole("button", { name: /replay/i });
  assert.equal(await replayButton.count(), 1, "Replay acceptance blocked: the Archive client exposes no Replay control or authorized replay endpoint seam");
  await replayButton.click();
  await page.getByRole("button", { name: "Replay verified", exact: true }).waitFor({ timeout: authorizedProjectionTimeoutMs });
  assert.equal(replayRequests, replaysBefore + 1, "the browser did not issue one authorized Replay request");
  assert.equal(browserActionRequests, actionsBeforeReplay, "Replay must not submit a new participant Action");
  assert.deepEqual(liveSpecialistRoles.map((role) => specialist(role).invocations), invocationsBeforeReplay, "Replay must not rerun an external specialist Runner or policy");
  assert.equal(room.length > 0, true, "crew witness lost its Room identity");
}

async function runUnavailableLiveWitness(page, { revision }) {
  const mira = specialistCard(page, "mira");
  const jonah = specialistCard(page, "jonah");
  await Promise.all([
    mira.waitFor({ timeout: authorizedProjectionTimeoutMs }),
    jonah.waitFor({ timeout: authorizedProjectionTimeoutMs }),
  ]);

  await clickSpecialistAction(page, "mira", "assign_mira_task", "Assign Investigate Records with 0 power allowance");
  await clickSpecialistAction(page, "mira", "request_mira_plan", "Request a plan");
  await waitForSpecialistText(mira, /planning window is open until/i, "Mira delayed planning window");
  // The delayed provider owns the old claimed Activation. A lead cancellation
  // makes a newer Head before it replies, so no late plan can be admitted.
  await page.waitForTimeout(250);
  const browserActionsBeforeCancellation = browserActionRequests;
  await clickSpecialistAction(page, "mira", "cancel_mira_task", "Cancel current task");
  assert.equal(browserActionRequests, browserActionsBeforeCancellation + 1, "Mira cancellation must be one explicit lead Action");
  const delayedResult = await waitForSpecialistRunnerTerminalResult("mira", ["cancelled", "stale_head"]);
  assert.equal(delayedResult.submitted_actions, 0, "a delayed stale plan must not submit an Action");
  await waitForSpecialistText(mira, /standing task was cancelled/i, "Mira cancellation projection");
  assert.equal(specialist("mira").invocations, 1, "the delayed opportunity must invoke Mira exactly once");

  // Reopening is a new lead request and a new Runner invocation; the old
  // attempt is neither retried nor reused.
  startSpecialistRunner("mira", revision);
  await clickSpecialistAction(page, "mira", "assign_mira_task", "Assign Investigate Records with 0 power allowance");
  await clickSpecialistAction(page, "mira", "request_mira_plan", "Request a plan");
  const reopenedMira = await waitForSpecialistRunnerResult("mira", revision, 2);
  assert.deepEqual(reopenedMira, { status: "handled", submitted_actions: 1 }, "Mira reopened opportunity did not accept one plan");
  await waitForSpecialistText(mira, /ready\s*·\s*revision/i, "Mira reopened accepted plan");
  await assertSpecialistProgress(mira, "Mira", 0, 3, "Mira accepted plan must remain standing before any turn");

  // Jonah's first scripted provider declines. The Pack-owned opportunity stays
  // open until its own timer expires; the Runner must make no hidden retry.
  await clickSpecialistAction(page, "jonah", "assign_jonah_task", "Assign Investigate Records with 0 power allowance");
  await clickSpecialistAction(page, "jonah", "request_jonah_plan", "Request a plan");
  const rejectedResult = await waitForSpecialistRunnerTerminalResult("jonah", ["provider_rejected"]);
  assert.equal(rejectedResult.submitted_actions, 0, "a rejected provider must not submit Jonah's plan");
  await waitForSpecialistText(jonah, /planning window is open until/i, "Jonah rejected-provider waiting projection");
  await waitForSpecialistText(jonah, /expired\s*·\s*revision/i, "Jonah Pack-owned opportunity expiry");
  await waitForSpecialistText(jonah, /No Jonah plan was recorded before opportunity/i, "Jonah expiry continuation notice");

  // A second explicit request is bounded independently. Its missing scripted
  // provider times out once and leaves the authoritative waiting state intact.
  startSpecialistRunner("jonah", revision, { providerMode: "missing", providerTimeoutSeconds: 1 });
  await clickSpecialistAction(page, "jonah", "request_jonah_plan", "Request plan again");
  const missingResult = await waitForSpecialistRunnerTerminalResult("jonah", ["provider_timeout"]);
  assert.equal(missingResult.submitted_actions, 0, "a missing provider must not submit Jonah's plan");
  await waitForSpecialistText(jonah, /planning window is open until/i, "Jonah missing-provider waiting projection");
  assert.equal(specialist("jonah").invocations, 2, "Jonah must run exactly once for each explicit scripted opportunity");

  // The lead can continue while a recorded plan stands: deferring a companion
  // and waiting consumes one ordinary lead turn without invalidating that plan.
  let turns = readMetric(await page.locator("body").innerText(), "Turns remaining");
  await clickSpecialistAction(page, "mira", "defer_mira_contribution", "Defer Mira this turn");
  await clickLiveAction(page, page.getByRole("button", { name: /Stage Wait for one turn/i }), "stage_wait");
  turns = await commitLiveTurn(page, turns, "deferred companion wait turn");
  assert.equal(turns, 15, "one explicit wait turn must consume exactly one lead turn");
  await waitForSpecialistText(mira, /ready\s*·\s*revision/i, "Mira standing plan after defer and wait");
  await assertSpecialistProgress(mira, "Mira", 0, 3, "defer and wait must preserve Mira's eligible plan");

  await page.reload({ waitUntil: "domcontentloaded" });
  await page.getByText("Projection current", { exact: true }).waitFor({ timeout: authorizedProjectionTimeoutMs });
  await waitForSpecialistText(specialistCard(page, "mira"), /ready\s*·\s*revision/i, "Mira plan after browser reconnect");
  await assertSpecialistProgress(specialistCard(page, "mira"), "Mira", 0, 3, "reload must preserve the authoritative standing plan");
  assert.equal(specialist("mira").invocations, 2, "Mira must run exactly once for the cancelled and reopened opportunities");
  assert.deepEqual(runnerInvocationReceipt(), { mira: 2, jonah: 2 }, "unavailable-live must invoke each Runner exactly for its scripted opportunities");
}

function specialistCard(page, role) {
  return page.getByTestId(`${role}-crew-card`);
}

async function clickSpecialistAction(page, role, actionType, name) {
  const locator = specialistCard(page, role).locator(`[data-action-type="${actionType}"][aria-label="${name}"]`);
  const exact = specialistCard(page, role).getByRole("button", { name, exact: true });
  await clickLiveAction(page, await locator.count() === 1 ? locator : exact, actionType);
}

async function commitLiveTurn(page, turns, description) {
  await clickLiveAction(page, page.getByRole("button", { name: /commit turn/i }).first(), "commit_turn");
  return waitForMetric(page, "Turns remaining", (value) => value === turns - 1, description);
}

async function prepareAndAcknowledgeExtraction(page, leftBehindRoles, required) {
  const prepare = page.locator('[data-action-type="prepare_extraction"]');
  const commit = page.getByRole("button", { name: /commit turn/i }).first();
  for (let attempt = 0; attempt < authorizedProjectionTimeoutMs / 100; attempt += 1) {
    if (bridgeFailure) throw bridgeFailure;
    if (await prepare.count() === 1 && await prepare.isEnabled()) break;
    if (!required && await commit.isEnabled()) return false;
    await page.waitForTimeout(100);
  }
  assert.equal(await prepare.count(), 1, "staged extraction did not expose one prepare-extraction control");
  await clickLiveAction(page, prepare, "prepare_extraction");
  const preview = page.locator(".extraction-preview");
  await preview.getByText(leftBehindRoles.length === 0 ? "No one" : new RegExp(leftBehindRoles.join(".*"), "i"), { exact: leftBehindRoles.length === 0 }).waitFor({ timeout: authorizedProjectionTimeoutMs });
  const acknowledgement = preview.locator('[data-action-type="acknowledge_extraction"]');
  await clickLiveAction(page, acknowledgement, "acknowledge_extraction");
  await preview.getByText("Exact crew result acknowledged.", { exact: true }).waitFor({ timeout: authorizedProjectionTimeoutMs });
  return true;
}

async function clickLiveAction(page, locator, actionType) {
  let count = 0;
  for (let attempt = 0; attempt < authorizedProjectionTimeoutMs / 100; attempt += 1) {
    if (bridgeFailure) throw bridgeFailure;
    count = await locator.count();
    if (count !== 0) break;
    await page.waitForTimeout(100);
  }
  assert.equal(count, 1, `${actionType} control is missing or ambiguous`);
  await waitForEnabled(page, locator, `${actionType} control`);
  const before = browserActionRequests;
  const responsesBefore = browserActionResponses;
  await locator.click();
  for (let attempt = 0; attempt < authorizedProjectionTimeoutMs / 100; attempt += 1) {
    if (bridgeFailure) throw bridgeFailure;
    if (browserActionRequests === before + 1 && browserActionResponses === responsesBefore + 1) {
      if (latestBrowserActionResponse?.request === before + 1
        && (latestBrowserActionResponse.status < 200 || latestBrowserActionResponse.status >= 300)) {
        const code = latestBrowserActionResponse.code ? ` (${latestBrowserActionResponse.code})` : "";
        throw new Error(`${actionType} was rejected by the participant Action endpoint: HTTP ${latestBrowserActionResponse.status}${code}`);
      }
      if (latestBrowserActionResponse?.request === before + 1
        && latestBrowserActionResponse.state === "rejected") {
        const code = latestBrowserActionResponse.code ? ` (${latestBrowserActionResponse.code})` : "";
        throw new Error(`${actionType} was rejected by the authoritative Room${code}`);
      }
      if (latestBrowserActionResponse?.request !== before + 1
        || latestBrowserActionResponse.state !== "accepted") {
        throw new Error(`${actionType} returned an invalid participant Action receipt`);
      }
      await page.waitForTimeout(100);
      return;
    }
    await page.waitForTimeout(100);
  }
  throw new Error(`${actionType} did not reach the authorized participant Action endpoint`);
}

function responseErrorCode(body) {
  try {
    const decoded = JSON.parse(body.toString("utf8"));
    return typeof decoded?.code === "string"
      ? decoded.code
      : typeof decoded?.receipt?.code === "string"
        ? decoded.receipt.code
      : typeof decoded?.error?.code === "string"
        ? decoded.error.code
        : undefined;
  } catch {
    return undefined;
  }
}

function responseActionState(body) {
  try {
    const decoded = JSON.parse(body.toString("utf8"));
    return decoded?.state === "accepted" || decoded?.state === "rejected" ? decoded.state : undefined;
  } catch {
    return undefined;
  }
}

async function assertMiraProgress(mira, completed, total, description) {
  await assertSpecialistProgress(mira, "Mira", completed, total, description);
}

async function assertSpecialistProgress(card, name, completed, total, description) {
  await waitForSpecialistText(card, new RegExp(`Progress\\s*${completed} of ${total} complete`, "i"), description);
  const text = await card.innerText();
  const match = text.match(/Progress\s*(\d+) of (\d+) complete/i);
  assert.ok(match, `${description}: ${name} progress was not rendered`);
  assert.equal(Number(match[1]), completed, description);
  assert.equal(Number(match[2]), total, `${description}: ${name} plan length changed`);
}

async function waitForMiraText(mira, pattern, description) {
  return waitForSpecialistText(mira, pattern, description);
}

async function waitForSpecialistText(card, pattern, description) {
  for (let attempt = 0; attempt < authorizedProjectionTimeoutMs / 100; attempt += 1) {
    if (bridgeFailure) throw bridgeFailure;
    if (pattern.test(await card.innerText())) return;
    await card.page().waitForTimeout(100);
  }
  throw new Error(`${description} did not render`);
}

function specialist(role) {
  const runtime = specialistRuntime.get(role);
  assert.ok(runtime, `unknown specialist role: ${role}`);
  return runtime;
}

function startSpecialistRunner(role, revision, options = {}) {
  const runtime = specialist(role);
  assert.ok(runtime.membershipFile && runtime.runnerFile, `${runtime.name} credentials are unavailable`);
  assert.ok(runtime.process === undefined || runtime.process.child.exitCode !== null, `${runtime.name} Runner is still active`);
  const providerMode = options.providerMode ?? "deterministic";
  const providerDelaySeconds = options.providerDelaySeconds ?? 0;
  const providerTimeoutSeconds = options.providerTimeoutSeconds ?? 10;
  assert.ok(["deterministic", "delayed", "missing", "rejected"].includes(providerMode), `${runtime.name} scripted provider mode is invalid`);
  assert.ok(Number.isFinite(providerDelaySeconds) && providerDelaySeconds >= 0, `${runtime.name} provider delay is invalid`);
  assert.ok(Number.isFinite(providerTimeoutSeconds) && providerTimeoutSeconds > 0, `${runtime.name} provider timeout is invalid`);
  const python = join(workspace, "sdk/python/.venv/bin/python");
  runtime.invocations += 1;
  const child = spawn(python, [
    "-m", runtime.module,
    "--membership-file", runtime.membershipFile,
    "--runner-file", runtime.runnerFile,
    "--pack-revision", revision,
    "--wait-seconds", crewLiveProof ? "180" : "90",
    "--provider-mode", providerMode,
    "--provider-delay-seconds", String(providerDelaySeconds),
    "--provider-timeout-seconds", String(providerTimeoutSeconds),
  ], { cwd: workspace, stdio: ["ignore", "pipe", "ignore"] });
  let stdout = "";
  child.stdout.on("data", (chunk) => {
    stdout = `${stdout}${String(chunk)}`;
    if (stdout.length > 64 * 1024) child.kill("SIGTERM");
  });
  const completed = new Promise((resolveResult, rejectResult) => {
    child.once("error", () => rejectResult(new Error(`${runtime.name} Runner process could not start`)));
    child.once("exit", (code, signal) => {
      if (signal !== null || code === null) rejectResult(new Error(`${runtime.name} Runner ended without an exit code (${signal ?? "unknown"})`));
      else resolveResult({ stdout, code });
    });
  });
  void completed.catch(() => undefined);
  runtime.process = { child, completed };
}

async function waitForSpecialistRunnerReadiness(room, roles) {
  for (const role of roles) assert.ok(specialist(role).process, `${specialist(role).name} Runner process was not started`);
  for (let attempt = 0; attempt < authorizedProjectionTimeoutMs / 250; attempt += 1) {
    for (const role of roles) {
      const process = specialist(role).process;
      if (process.child.exitCode !== null) await process.completed;
    }
    const inspection = await cli("room", "inspect", room);
    const readiness = inspection.launch_assessment?.readiness;
    const seats = readiness?.seats;
    const ready = Array.isArray(seats) && ["lead", ...roles].every((role) => seats.find((seat) => seat?.seat_id === role)?.ready === true);
    if (ready && readiness?.ready_to_launch === true) return;
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
  }
  throw new Error(`Lead and ${roles.join(" and ")} did not all become ready before Activity Start`);
}

async function waitForSpecialistRunnerResult(role, revision, expectedInvocations = 1) {
  const runtime = specialist(role);
  assert.ok(runtime.process, `${runtime.name} Runner process was not started before Activity Start`);
  const completed = await runtime.process.completed;
  assert.equal(completed.code, 0, `${runtime.name} Runner ended before handling its bounded opportunity (${completed.code})`);
  assertPrivatePayload(completed.stdout, `${runtime.name} Runner output`);
  const lines = completed.stdout.trim().split("\n").filter(Boolean);
  assert.equal(lines.length, 1, `${runtime.name} Runner emitted more than one bounded result`);
  const result = JSON.parse(lines[0]);
  assert.equal(runtime.invocations, expectedInvocations, `${runtime.name} Runner invocation count changed unexpectedly`);
  assert.equal(typeof revision, "string", `${runtime.name} Runner lost the Pack revision fence`);
  return result;
}

async function waitForSpecialistRunnerTerminalResult(role, expectedStatuses) {
  const runtime = specialist(role);
  assert.ok(runtime.process, `${runtime.name} Runner process was not started before its terminal disposition`);
  const completed = await runtime.process.completed;
  assert.equal(completed.code, 2, `${runtime.name} Runner must report its terminal provider disposition with exit code 2`);
  assertPrivatePayload(completed.stdout, `${runtime.name} Runner terminal output`);
  const lines = completed.stdout.trim().split("\n").filter(Boolean);
  assert.equal(lines.length, 1, `${runtime.name} Runner emitted more than one terminal result`);
  const result = JSON.parse(lines[0]);
  assert.ok(expectedStatuses.includes(result.status), `${runtime.name} Runner terminal status was ${String(result.status)}`);
  assert.deepEqual(
    { status: result.status, submitted_actions: result.submitted_actions, provider_attempts_consumed: result.provider_attempts_consumed },
    { status: result.status, submitted_actions: 0, provider_attempts_consumed: 1 },
    `${runtime.name} terminal provider disposition must have one consumed attempt and no submitted Action`,
  );
  return result;
}

async function stopSpecialistRunnersIfActive() {
  await Promise.all([...specialistRuntime.values()].map(async (runtime) => {
    if (!runtime.process || runtime.process.child.exitCode !== null) return;
    runtime.process.child.kill("SIGTERM");
    await Promise.race([
      runtime.process.completed.catch(() => undefined),
      new Promise((resolveDelay) => setTimeout(resolveDelay, 5_000)),
    ]);
  }));
}

function runnerInvocationReceipt() {
  if (crewLiveProof || unavailableLiveProof) return Object.fromEntries(liveSpecialistRoles.map((role) => [role, specialist(role).invocations]));
  return specialist("mira").invocations;
}

function debug(message) {
  if (process.env.WORLDSTREAM_ACCEPTANCE_DEBUG === "1") {
    process.stderr.write(`[archive +${Date.now() - acceptanceStartedAt}ms] ${message}\n`);
  }
}

async function waitForEnabled(page, locator, description) {
  for (let attempt = 0; attempt < authorizedProjectionTimeoutMs / 100; attempt += 1) {
    if (bridgeFailure) throw bridgeFailure;
    if (await locator.isEnabled()) return;
    await page.waitForTimeout(100);
  }
  const body = await page.locator("body").innerText();
  throw new Error(`${description} did not become enabled after the authoritative Projection:\n${body.slice(0, 4_000)}`);
}

async function waitForMetric(page, label, predicate, description) {
  for (let attempt = 0; attempt < authorizedProjectionTimeoutMs / 100; attempt += 1) {
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
    await page.getByText(pattern).first().waitFor({ timeout: authorizedProjectionTimeoutMs });
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
