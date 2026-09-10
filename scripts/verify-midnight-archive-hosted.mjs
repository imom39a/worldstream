#!/usr/bin/env node
// Local hosted journey. Requires `pnpm hosted:dev`; uses its visible development
// identity substitute, real Supabase/BFF/Gateway/Runtime and retained client bytes.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { chromium } from "playwright";

const origin = "http://127.0.0.1:5180";
const evidenceDirectory = resolve(".worldstream/evidence/imo-209");
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const page = await context.newPage();
page.setDefaultTimeout(45_000);
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
const evidence = { schema: "worldstream/hosted-candidate-witness/v1", local_development_identity: true, checks: [] };
await mkdir(evidenceDirectory, { recursive: true });
try {
  const guest = await (await fetch(`${origin}/api/catalog`)).json();
  assert.equal(guest.activities.some((entry) => entry.slug === "midnight-archive"), false);
  assert.equal((await fetch(`${origin}/api/catalog/internal`)).status, 401);
  await page.goto(origin);
  await page.locator(".activity-agent-heist").getByRole("button", { name: "Enter activity ↗" }).click();
  await page.getByRole("button", { name: "Use visible local-development sign-in" }).click();
  await page.getByRole("button", { name: "Create waiting room", exact: true }).waitFor();
  await page.getByRole("button", { name: "Close", exact: true }).click();
  const card = page.locator(".activity-midnight-archive");
  await card.getByText("Optional House Agents", { exact: true }).waitFor();
  await card.getByRole("button", { name: "Enter activity ↗" }).click();
  const dialog = page.getByRole("dialog");
  assert.match(await dialog.innerText(), /16 turns.*3 power/s);
  assert.match(await dialog.innerText(), /Play solo as Expedition lead/);
  assert.equal(await dialog.getByText("Fill open seats", { exact: true }).count(), 0);
  assert.equal(await dialog.getByText("Choose your role", { exact: true }).count(), 0);
  let launch;
  if (process.env.WORLDSTREAM_ARCHIVE_RESUME_LAUNCH_ID) {
    const launchId = process.env.WORLDSTREAM_ARCHIVE_RESUME_LAUNCH_ID;
    assert.match(launchId, /^[0-9a-f-]{36}$/u);
    launch = { launch_id: launchId };
    await page.goto(`${origin}/launches/${launchId}`);
    evidence.checks.push("resume_retained_launch");
  } else {
    const creationResponse = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/launches" && response.request().method() === "POST");
    await page.getByRole("button", { name: "Prepare solo activity", exact: true }).click();
    const createdResponse = await creationResponse;
    launch = await createdResponse.json();
    assert.ok([200, 201].includes(createdResponse.status()), JSON.stringify(launch));
    const createBody = createdResponse.request().postDataJSON();
    assert.equal(createBody.fill_mode, "people_only");
    const repeated = await mutation("/api/launches", createBody);
    assert.equal(repeated.launch_id, launch.launch_id);
    evidence.checks.push("idempotent_create");
    await page.getByRole("heading", { name: "Ready to begin", exact: true }).waitFor();
    await page.getByRole("button", { name: "Start activity", exact: true }).click();
  }
  evidence.launch_id = launch.launch_id;
  evidence.checks.push("guest_hidden", "signed_in_exact_candidate", "solo_controls");
  const entry = page.getByRole("button", { name: "Enter Expedition lead", exact: true });
  await entry.waitFor();
  const start = await mutation(`/api/launches/${launch.launch_id}/start`, {});
  assert.equal(start.state, "run_created", JSON.stringify(start));
  evidence.run_id = start.run.run_id;
  const initialDatabase = databaseSnapshot(evidence.run_id);
  assert.equal(initialDatabase.runs_for_launch, 1);
  await entry.click();
  await page.waitForURL("**/midnight-archive-v13/hosted/**");
  await page.getByText("Projection current", { exact: true }).waitFor();
  assert.equal(new URL(page.url()).hash, "");
  evidence.checks.push("idempotent_start", "exact_client_handoff", "authoritative_projection");
  let turns = 16;
  const route = [
    [/Move to Records/i], [/Stage Run the catalog verifier; costs 1 turn and 1 power/i],
    [/Move to Plant/i], [/Stage Open the service hatch; costs 1 turn and 2 power/i],
    [/Move to Vault/i], [/Stage recovery of Violet Ledger; costs 1 turn and 0 power/i],
    [/Move to Plant/i], [/Move to Records/i], [/Move to Atrium/i],
    [/Stage Extract from the archive; costs 1 turn and 0 power/i],
  ];
  for (const [name] of route) {
    await page.getByRole("button", { name }).first().click();
    const commit = page.getByRole("button", { name: /commit turn/i }).first();
    if (/Extract/.test(name.source)) {
      const prepare = page.locator('[data-action-type="prepare_extraction"]');
      await prepare.waitFor();
      await prepare.click();
      const preview = page.locator(".extraction-preview");
      await preview.getByText("No one", { exact: true }).waitFor();
      await preview.locator('[data-action-type="acknowledge_extraction"]').click();
      await preview.getByText("Exact crew result acknowledged.", { exact: true }).waitFor();
    }
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((button) => /commit turn/i.test(button.textContent) && !button.disabled));
    assert.equal(metric(await page.locator("body").innerText(), "Turns remaining"), turns);
    await commit.click();
    turns -= 1;
    await page.waitForFunction((remaining) => new RegExp(`Turns remaining\\s*${remaining}(?!\\d)`, "i").test(document.body.innerText), turns);
  }
  await page.getByRole("heading", { name: "The authentic ledger is out", exact: true }).waitFor();
  assert.equal(turns, 6);
  assert.equal(metric(await page.locator("body").innerText(), "Power reserve"), 0);
  await page.screenshot({ path: resolve(evidenceDirectory, "terminal.png"), fullPage: true });
  evidence.checks.push("ten_turn_solo_technical_route", "read_stage_do_not_spend_turn", "authentic_ledger_extracted");
  await page.goto(`${origin}/my-games`);
  const history = page.locator(`[data-launch-id="${launch.launch_id}"]`);
  await history.getByText("terminal private", { exact: true }).waitFor();
  assert.equal(await history.getByRole("button", { name: "View result", exact: true }).count(), 0);
  assert.equal(await history.getAttribute("data-result-public-id"), null);
  await history.getByRole("button", { name: "Return to game", exact: true }).click();
  await page.getByRole("button", { name: "Enter Expedition lead", exact: true }).click();
  await page.getByRole("heading", { name: "The authentic ledger is out", exact: true }).waitFor();
  assert.equal(metric(await page.locator("body").innerText(), "Turns remaining"), 6);
  evidence.checks.push("terminal_private_history", "no_public_result", "same_room_terminal_reentry");
  const terminalDatabase = databaseSnapshot(evidence.run_id);
  assert.equal(terminalDatabase.runs_for_launch, 1);
  assert.equal(terminalDatabase.terminal_evidence, 1);
  assert.equal(terminalDatabase.indexed_results, 0);
  assert.equal(terminalDatabase.active_capacity, 0);
  assert.equal(terminalDatabase.public_id, null);
  assert.deepEqual(terminalDatabase.memberships, initialDatabase.memberships);
  evidence.database = terminalDatabase;
  evidence.checks.push("one_genesis_run", "capacity_retired", "original_membership_correspondence");
  await page.goto(origin);
  await page.locator(".activity-agent-heist").getByRole("button", { name: "Enter activity ↗" }).click();
  await page.getByText("Fill open seats", { exact: true }).waitFor();
  await page.getByText("People and their agents only", { exact: true }).click();
  const heistCreation = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/launches" && response.request().method() === "POST");
  await page.getByRole("button", { name: "Create waiting room", exact: true }).click();
  const heistResponse = await heistCreation;
  const heist = await heistResponse.json();
  assert.ok([200, 201].includes(heistResponse.status()), JSON.stringify(heist));
  evidence.heist_launch_id = heist.launch_id;
  assert.equal(heist.state, "collecting");
  await mutation(`/api/launches/${heist.launch_id}/close`, {});
  evidence.checks.push("retained_heist_discovery_setup_and_close");
  assert.deepEqual(errors, []);
  evidence.checks.push("no_browser_errors");
  evidence.completed_at = new Date().toISOString();
  await writeFile(resolve(evidenceDirectory, "journey.json"), `${JSON.stringify(evidence, null, 2)}\n`);
  console.log(JSON.stringify(evidence, null, 2));
} catch (error) {
  await writeFile(resolve(evidenceDirectory, "partial-journey.json"), `${JSON.stringify(evidence, null, 2)}\n`);
  await page.screenshot({ path: resolve(evidenceDirectory, "failure.png"), fullPage: true });
  console.error((await page.locator("body").innerText()).slice(0, 5000));
  throw error;
} finally {
  await browser.close();
}

async function mutation(path, body) {
  return page.evaluate(async ({ path, body }) => {
    const session = await (await fetch("/api/auth/session", { cache: "no-store" })).json();
    const response = await fetch(path, { method: "POST", headers: { "content-type": "application/json", "x-worldstream-csrf": session.csrf }, body: JSON.stringify(body) });
    const value = await response.json();
    if (!response.ok) throw new Error(`mutation_failed:${response.status}:${JSON.stringify(value)}`);
    return value;
  }, { path, body });
}

function metric(body, label) {
  const match = body.match(new RegExp(`${label}\\s*(\\d+)`, "i"));
  assert.ok(match, `${label} missing`);
  return Number(match[1]);
}

function databaseSnapshot(runId) {
  assert.match(runId, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u);
  const sql = `select jsonb_build_object(
    'room_id',runs.room_id,'pack_revision_digest',runs.pack_revision_digest,'public_id',runs.public_id,
    'runs_for_launch',(select count(*) from platform_store.activity_runs other where other.launch_request_id=runs.launch_request_id),
    'terminal_evidence',(select count(*) from platform_store.activity_run_terminal_evidence terminals where terminals.activity_run_id=runs.activity_run_id),
    'indexed_results',(select count(*) from platform_store.indexed_activity_results results where results.activity_run_id=runs.activity_run_id),
    'active_capacity',(select count(*) from platform_store.capacity_reservations reservations where reservations.activity_run_id=runs.activity_run_id and reservations.released_at is null),
    'memberships',(select jsonb_agg(jsonb_build_object('membership_id',memberships.membership_id,'principal_id',memberships.principal_id,'purpose',memberships.purpose,'role',memberships.role) order by memberships.membership_id) from platform_store.activity_run_memberships memberships where memberships.activity_run_id=runs.activity_run_id)
  ) as evidence from platform_store.activity_runs runs where runs.activity_run_id='${runId}'::uuid`;
  const result = JSON.parse(execFileSync("supabase", ["db", "query", "--local", sql], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }));
  assert.equal(result.rows.length, 1);
  return result.rows[0].evidence;
}
