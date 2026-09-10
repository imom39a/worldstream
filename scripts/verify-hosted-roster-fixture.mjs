import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { chromium } from "playwright";
import { readRosterFixture, assertRosterLineage, rosterFixtureDigest } from "./hosted-roster-fixture.mjs";

/** Real local UI/API/Host journey. Requires explicit fixture registration/imports. */
export async function verifyRosterFixture({ origin, evidenceDirectory, snapshot = rosterDatabaseSnapshot, hostSnapshot = rosterHostSnapshot }) {
  const url = new URL(origin);
  assert.ok(url.protocol === "http:" && ["127.0.0.1", "localhost"].includes(url.hostname), "fixture journey is loopback-only");
  assert.equal(url.pathname, "/");
  const { fixture, listing, house } = await readRosterFixture();
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  page.setDefaultTimeout(45000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const evidence = { schema: "worldstream/roster-fixture-journey/v1", status: "running",
    execution: fixture.execution, authentication: "visible-local-development-identity", listing_digest: listing.digest,
    house_agent_digest: house.digest, pack: listing.value.pack, client: listing.value.client, runs: [] };
  await mkdir(evidenceDirectory, { recursive: true });
  try {
    assert.equal((await fetch(new URL("/api/catalog/internal", origin))).status, 401);
    await page.goto(origin);
    await page.locator(".activity-agent-heist").getByRole("button", { name: "Enter activity ↗" }).click();
    await page.getByRole("button", { name: "Use visible local-development sign-in" }).click();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    for (const option of ["solo", "supplied"]) {
      await page.goto(origin);
      const card = page.locator(`.activity-${fixture.slug}`);
      await card.getByRole("button", { name: "Enter activity ↗" }).click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("radio", { name: option === "solo" ? "Solo fixture" : "Supplied assistant fixture", exact: true }).check();
      const created = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/launches" && response.request().method() === "POST");
      await dialog.locator("button.primary-launch").click();
      const response = await created;
      assert.ok([200, 201].includes(response.status()));
      const launch = await response.json();
      const body = response.request().postDataJSON();
      assert.equal(body.roster_option, option);
      assert.equal(body.fill_mode, option === "solo" ? "people_only" : "house_agents");
      const repeat = await mutation(page, "/api/launches", body);
      assert.equal(repeat.status, 200);
      assert.equal(repeat.value.launch_id, launch.launch_id);
      const changed = await mutation(page, "/api/launches", { ...body, roster_option: option === "solo" ? "supplied" : "solo",
        fill_mode: option === "solo" ? "house_agents" : "people_only" });
      assert.ok(changed.status >= 400, "an existing idempotency key must not change roster intent");
      await page.getByRole("button", { name: "Start activity", exact: true }).click();
      const entry = page.getByRole("button", { name: "Enter Expedition lead", exact: true });
      await entry.waitFor();
      const started = await mutation(page, `/api/launches/${launch.launch_id}/start`, {});
      assert.equal(started.status, 200);
      assert.equal(started.value.state, "run_created");
      const runId = started.value.run.run_id;
      const initial = await snapshot(runId);
      assertRosterLineage(initial, { option });
      assert.equal(initial.pack_revision_digest, listing.value.pack.digest);
      if (option === "supplied") {
        const assignment = initial.assignments[0];
        assert.equal(assignment.house_agent_revision_digest, house.digest);
        assert.equal(assignment.profile_id, house.value.agent_profile.profile_id);
        assert.equal(assignment.profile_revision, house.value.agent_profile.revision);
        assert.equal(assignment.template_id, house.value.runner_template.template_id);
        assert.equal(assignment.template_revision, house.value.runner_template.revision);
        assert.equal(assignment.principal_reference, `house:${launch.launch_id}:mira`);
        assert.equal(typeof assignment.reservation_operation_id, "string");
        assert.equal(typeof assignment.runner_unit_id, "string");
      }
      const runEvidence = { option, initial, checks: ["authenticated_option_selection", "idempotent_launch", "changed_option_refused",
        "idempotent_start", "one_genesis", "exact_memberships_assignments"] };
      evidence.runs.push(runEvidence);
      await entry.click();
      await page.waitForURL("**/midnight-archive-v10/hosted/**");
      await page.getByText("Projection current", { exact: true }).waitFor();
      let remaining = 16;
      if (option === "supplied") {
        const companion = page.getByTestId("mira-crew-card");
        await page.getByRole("button", { name: /Move to Records/i }).first().click();
        await commit(page, --remaining);
        await companion.getByRole("button", { name: "Assign Investigate Records with 0 power allowance", exact: true }).click();
        await companion.getByRole("button", { name: "Request a plan", exact: true }).click();
        await companion.getByText(/ready\s*·\s*revision/i).waitFor();
        for (let step = 1; step <= 2; step += 1) {
          await page.getByRole("button", { name: /Stage Wait for one turn/i }).click();
          await companion.getByRole("button", { name: "Prepare next eligible step", exact: true }).click();
          await commit(page, --remaining);
        }
        await companion.getByText(/Records evidence is disclosed on the candidate board/i).waitFor();
        runEvidence.checks.push("controlled_provider_plan", "two_committed_companion_steps");
        await companion.getByRole("button", { name: "Follow lead", exact: true }).click();
        await page.getByRole("button", { name: /Move to Atrium/i }).first().click();
        await commit(page, --remaining);
      }
      // Formation qualification ends with an honest no-ledger extraction; this is not a mission-success claim.
      await page.getByRole("button", { name: /Stage Extract from the archive/i }).click();
      await page.locator('[data-action-type="prepare_extraction"]').click();
      await page.locator('[data-action-type="acknowledge_extraction"]').click();
      await commit(page, --remaining);
      await page.getByText("You reached the Atrium and chose extraction without recovering a ledger.", { exact: true }).waitFor();
      runEvidence.checks.push("partial_extraction");
      await page.goto(new URL("/my-games", origin).href);
      const history = page.locator(`[data-launch-id="${launch.launch_id}"]`);
      await history.getByText("terminal private", { exact: true }).waitFor();
      runEvidence.terminal = await snapshot(runId);
      assertRosterLineage(runEvidence.terminal, { option, prior: initial });
      assert.equal(runEvidence.terminal.active_capacity, 0);
      assert.equal(runEvidence.terminal.indexed_results, 0);
      runEvidence.checks.push("active_capacity_released", "no_public_result");
      if (option === "supplied") {
        runEvidence.execution = await hostSnapshot(initial.assignments[0]);
        assert.equal(runEvidence.execution.allowance.attempts.length, 1);
        assert.equal(runEvidence.execution.allowance.attempts[0].state, "completed");
        assert.ok(runEvidence.execution.allowance.attempts[0].input_units <= house.value.allowance.input_tokens_per_call);
        assert.ok(runEvidence.execution.allowance.attempts[0].output_units <= house.value.allowance.output_tokens_per_call);
        assert.equal(runEvidence.execution.retirement.disposition, "run_terminal");
        assert.equal(runEvidence.execution.retirement.allowance_reset, false);
        runEvidence.checks.push("one_bounded_provider_attempt", "durable_runner_retirement");
      }
      await history.getByRole("button", { name: "Return to game", exact: true }).click();
      await page.getByRole("button", { name: "Enter Expedition lead", exact: true }).click();
      await page.getByText("You reached the Atrium and chose extraction without recovering a ledger.", { exact: true }).waitFor();
      const terminal = await snapshot(runId);
      assertRosterLineage(terminal, { option, prior: initial });
      assert.equal(terminal.active_capacity, 0);
      assert.equal(terminal.indexed_results, 0);
      if (option === "supplied") {
        assert.deepEqual(await hostSnapshot(initial.assignments[0]), runEvidence.execution, "terminal re-entry changed retained allowance or retirement");
        runEvidence.checks.push("consumed_allowance_preserved_on_reentry");
      }
      await page.screenshot({ path: resolve(evidenceDirectory, `${option}-terminal.png`), fullPage: true });
      runEvidence.terminal = terminal;
      runEvidence.turns_remaining = remaining;
      runEvidence.checks.push(...(option === "solo" ? ["no_optional_assignment"] : []), "same_terminal_membership_reentry");
    }
    assert.notEqual(evidence.runs[0].terminal.room_id, evidence.runs[1].terminal.room_id);
    assert.deepEqual(errors, []);
    evidence.status = "passed";
    return evidence;
  } catch (error) {
    evidence.status = "failed";
    evidence.failure = error instanceof Error ? error.message : String(error);
    await page.screenshot({ path: resolve(evidenceDirectory, "failure.png"), fullPage: true }).catch(() => {});
    await writeFile(resolve(evidenceDirectory, "failure-visible.txt"), await page.locator("body").innerText().catch(() => "Page unavailable"));
    throw error;
  } finally {
    await writeFile(resolve(evidenceDirectory, "journey.json"), JSON.stringify(evidence, null, 2) + "\n");
    await browser.close();
  }
}

async function mutation(page, path, body) {
  return page.evaluate(async ({ path, body }) => {
    const session = await (await fetch("/api/auth/session", { cache: "no-store" })).json();
    const response = await fetch(path, { method: "POST", headers: { "content-type": "application/json", "x-worldstream-csrf": session.csrf }, body: JSON.stringify(body) });
    return { status: response.status, value: await response.json() };
  }, { path, body });
}

async function commit(page, remaining) {
  const button = page.getByRole("button", { name: /commit turn/i }).first();
  await button.click();
  await page.waitForFunction((value) => new RegExp(`Turns remaining\\s*${value}(?!\\d)`, "i").test(document.body.innerText), remaining);
}

export function rosterDatabaseSnapshot(runId) {
  assert.match(runId, /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u);
  const query = `select jsonb_build_object('run_id',runs.activity_run_id,'launch_id',launches.launch_request_id,
    'room_id',runs.room_id,'pack_revision_digest',runs.pack_revision_digest,
    'roster_option',convert_from(launches.canonical_launch_input,'utf8')::jsonb->>'roster_option',
    'launch_input_digest',encode(launches.launch_input_digest,'hex'),'frozen_roster_digest',encode(launches.frozen_roster_digest,'hex'),
    'room_setup_specification_digest',launches.frozen_room_setup_specification_digest,
    'runs_for_launch',(select count(*) from platform_store.activity_runs other where other.launch_request_id=runs.launch_request_id),
    'memberships',(select jsonb_agg(jsonb_build_object('membership_id',m.membership_id,'principal_id',m.principal_id,'role',m.role,
      'participation_source',m.participation_source,'assignment_id',m.house_agent_assignment_id) order by m.role)
      from platform_store.activity_run_memberships m where m.activity_run_id=runs.activity_run_id and m.role is not null),
    'assignments',coalesce((select jsonb_agg(jsonb_build_object('assignment_id',a.house_agent_assignment_id,'seat_id',a.seat_id,
      'principal_reference',a.setup_principal_reference,'house_agent_revision_digest',a.house_agent_revision_digest,
      'reservation_operation_id',a.reservation_operation_id,'profile_id',a.agent_profile_id,'profile_revision',a.agent_profile_revision,
      'template_id',a.runner_template_id,'template_revision',a.runner_template_revision,
      'runner_unit_id',(select r.runner_unit_id from platform_store.house_runner_reservations r where r.reservation_operation_id=a.reservation_operation_id)) order by a.seat_id)
      from platform_store.house_agent_assignments a where a.launch_request_id=launches.launch_request_id),'[]'::jsonb),
    'active_capacity',(select count(*) from platform_store.capacity_reservations c where c.activity_run_id=runs.activity_run_id and c.released_at is null),
    'indexed_results',(select count(*) from platform_store.indexed_activity_results r where r.activity_run_id=runs.activity_run_id)) as evidence
    from platform_store.activity_runs runs join platform_store.launch_requests launches using(launch_request_id) where runs.activity_run_id='${runId}'::uuid`;
  const result = JSON.parse(execFileSync("supabase", ["db", "query", "--local", query], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }));
  assert.equal(result.rows.length, 1);
  return result.rows[0].evidence;
}

/** Only non-secret accounting and retirement fields enter shareable evidence. */
export async function rosterHostSnapshot(assignment, stateDirectory = resolve(".worldstream/hosted-dev/studio")) {
  assert.match(assignment.runner_unit_id, /^house-[0-9a-f]{32}$/u);
  for (const value of [assignment.template_id, assignment.template_revision, assignment.profile_id, assignment.profile_revision])
    assert.match(value, /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u);
  const root = resolve(stateDirectory, "hosted-house-runners/units");
  const ledger = JSON.parse(await readFile(resolve(root, "allowance/allowances.json"), "utf8"));
  const allowance = ledger.assignments[assignment.runner_unit_id];
  assert.equal(allowance.revision_digest, assignment.house_agent_revision_digest);
  const retired = JSON.parse(await readFile(resolve(root, assignment.runner_unit_id, "retired.json"), "utf8"));
  assert.equal(retired.receipt.house_agent_assignment_id, assignment.assignment_id);
  assert.equal(retired.receipt.reservation_operation_id, assignment.reservation_operation_id);
  const template = JSON.parse(await readFile(resolve(stateDirectory, "runner-templates/installed", `${assignment.template_id}--${assignment.template_revision}.json`), "utf8"));
  const hex = (value) => Buffer.from(value).toString("hex");
  const profile = JSON.parse(await readFile(resolve(stateDirectory, "agent-profiles/revisions", hex(assignment.profile_id), `${hex(assignment.profile_revision)}.json`), "utf8"));
  return { runner_unit_id: assignment.runner_unit_id,
    bindings: { profile_revision_digest: rosterFixtureDigest(profile), runner_template_revision_digest: rosterFixtureDigest(template),
      executable_digest: `blake3:${template.executable.blake3}` },
    allowance: { revision_digest: allowance.revision_digest, consumed_input_units: allowance.consumed_input_units,
      consumed_output_units: allowance.consumed_output_units,
      attempts: Object.entries(allowance.attempts).map(([attempt_digest, attempt]) => ({ attempt_digest,
        state: attempt.state, input_units: attempt.input_units, output_units: attempt.output_units, response_digest: attempt.response_digest })) },
    retirement: { schema: retired.receipt.schema, disposition: retired.receipt.disposition,
      assignment_id: retired.receipt.house_agent_assignment_id, reservation_operation_id: retired.receipt.reservation_operation_id,
      allowance_reset: retired.allowance_reset } };
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === resolve(import.meta.filename)) {
  assert.equal(process.env.WORLDSTREAM_ROSTER_FIXTURE_QUALIFICATION, "visible-local-only", "explicit local fixture qualification opt-in required");
  console.log(JSON.stringify(await verifyRosterFixture({ origin: process.env.WORLDSTREAM_ROSTER_FIXTURE_ORIGIN ?? "http://127.0.0.1:5180",
    evidenceDirectory: resolve(".worldstream/evidence/imo-208-roster-fixture") }), null, 2));
}
