import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { resolveRosterOption } from "../sdk/typescript-hosted-contract/dist/index.js";
import { readRosterFixture, renderRosterFixtureRunner, controlledArchivePlan, assertRosterLineage } from "./hosted-roster-fixture.mjs";
import { createRosterFixtureProvider } from "./hosted-roster-fixture-provider.mjs";
import { renderRosterFixtureDirectory } from "./render-hosted-roster-fixture.mjs";
import { rosterHostSnapshot } from "./verify-hosted-roster-fixture.mjs";

test("qualification artifact retains exact Pack/client and selects only solo or one supplied Role", async () => {
  const before = await readFile("config/hosted/listings/midnight-archive-0.1.0.json");
  const { listing, house, profile } = await readRosterFixture();
  const base = JSON.parse(before);
  assert.deepEqual(listing.value.pack, base.pack);
  assert.deepEqual(listing.value.client, base.client);
  assert.deepEqual(listing.value.result, base.result);
  const solo = resolveRosterOption(listing, { roster_option: "solo" });
  const supplied = resolveRosterOption(listing, { roster_option: "supplied" });
  assert.deepEqual(solo.seat_ids, ["lead"]);
  assert.deepEqual(solo.house_agent_assignments, []);
  assert.deepEqual(supplied.seat_ids, ["lead", "mira"]);
  assert.deepEqual(supplied.house_agent_assignments, [{ seat_id: "mira", house_agent_revision_digest: house.digest }]);
  assert.deepEqual(solo.configuration, supplied.configuration);
  assert.throws(() => resolveRosterOption(listing, { roster_option: "crew" }));
  assert.throws(() => resolveRosterOption(listing, { roster_option: "solo", configuration: {} }));
  assert.equal(house.value.agent_profile.profile_id, profile.profile_id);
  assert.equal(house.value.agent_profile.revision, profile.revision);
  assert.deepEqual(house.value.runner_template, profile.host_contract.runner_template);
  assert.equal(house.value.allowance.model_call_attempts, 10);
  assert.equal(house.value.allowance.input_tokens_per_call, 12000);
  assert.deepEqual(await readFile("config/hosted/listings/midnight-archive-0.1.0.json"), before);
});

test("fixture Runner pins retained executable and Archive compatibility without altering Heist", async () => {
  const template = await renderRosterFixtureRunner({ executable: "/tmp/retained/managed-host", digest: "a".repeat(64) });
  assert.deepEqual(template.compatibility, [{ activity_pack_id: "worldstream.midnight-archive", exact_revisions: ["0.1.0"] }]);
  assert.equal(template.executable.blake3, "a".repeat(64));
  assert.equal(template.template_id, "qualification-archive-house");
  await assert.rejects(renderRosterFixtureRunner({ executable: "relative", digest: "a".repeat(64) }));
  await assert.rejects(renderRosterFixtureRunner({ executable: "/tmp/host", digest: "blake3:" + "a".repeat(64) }));
});

function invocation() {
  return { schema: "worldstream/house-model-invocation/v1", action_offers: { offers: [{ action_type: "submit_companion_plan", offer_id: "exact-offer" }] },
    projection: { schema: "worldstream/assignment-observation/v1", role: "mira", pack: { id: "worldstream.midnight-archive" },
      stream: { frame_head: 7 }, observations: [], projection_reset: { baseline_frame_head: 7,
        projection_schema: "worldstream.midnight-archive/participant-projection/v3", projection: { activity: {
          phase: "active", mira: { presence: "active", location: "records", task: { kind: "investigate_records", status: "assigned", revision: 2 },
            planning: { status: "waiting", opportunity_revision: 4 }, knowledge: { records: "unknown" } } } } } } };
}

test("controlled response uses exact current offer/task and performs only bounded source work", () => {
  const request = invocation();
  const response = controlledArchivePlan(request);
  assert.equal(response.offer_id, "exact-offer");
  assert.equal(response.payload.task_revision, 2);
  assert.equal(response.payload.opportunity_revision, 4);
  assert.deepEqual(response.payload.steps.map((step) => step.step_type), ["inspect_source", "share_source"]);
  assert.ok(Buffer.byteLength(JSON.stringify(response)) < 1000);
  request.projection.projection_reset.projection.activity.mira.knowledge.records = "private";
  assert.deepEqual(controlledArchivePlan(request).payload.steps.map((step) => step.step_type), ["share_source"]);
  request.projection.projection_reset.projection.activity.mira.knowledge.records = "shared";
  assert.equal(controlledArchivePlan(request), null);
});

test("controlled response fails closed on stale/newer invalid frames, wrong Role, absent offer and unsupported task", () => {
  for (const mutate of [
    (value) => { value.projection.role = "lead"; },
    (value) => { value.projection.projection_reset.baseline_frame_head = 6; },
    (value) => { value.projection.observations = [{ frame_seq: 8, observation: {} }]; },
    (value) => { value.action_offers.offers = []; },
    (value) => { value.projection.projection_reset.projection.activity.mira.task.kind = "field_assay"; },
    (value) => { value.projection.projection_reset.projection.activity.phase = "complete"; },
  ]) {
    const request = invocation(); mutate(request);
    assert.equal(controlledArchivePlan(request), null);
  }
});

test("lineage verification rejects changed Memberships, option, or extra optional assignments", () => {
  const snapshot = { room_id: "room", run_id: "run", launch_id: "launch", roster_option: "solo", runs_for_launch: 1,
    memberships: [{ role: "lead", membership_id: "member", principal_id: "principal", participation_source: "account_human", assignment_id: null }], assignments: [] };
  assertRosterLineage(snapshot, { option: "solo", prior: structuredClone(snapshot) });
  assert.throws(() => assertRosterLineage({ ...snapshot, assignments: [{}] }, { option: "solo" }));
  assert.throws(() => assertRosterLineage({ ...snapshot, roster_option: "supplied" }, { option: "solo" }));
  assert.throws(() => assertRosterLineage(snapshot, { option: "solo", prior: { ...snapshot, room_id: "another" } }));
  const supplied = { ...snapshot, roster_option: "supplied", memberships: [...snapshot.memberships,
    { role: "mira", membership_id: "companion", principal_id: "agent", participation_source: "platform_house_agent", assignment_id: "assignment" }],
    assignments: [{ assignment_id: "assignment", seat_id: "mira" }] };
  assertRosterLineage(supplied, { option: "supplied" });
  assert.throws(() => assertRosterLineage({ ...supplied, assignments: [{ assignment_id: "other", seat_id: "mira" }] }, { option: "supplied" }));
});

test("fixture rendering is idempotent and refuses changed executable bytes under the same revision", async () => {
  const directory = await mkdtemp(join(tmpdir(), "roster-fixture-render-"));
  try {
    const runner = { executable: "/tmp/retained/managed-host", digest: "a".repeat(64) };
    const first = await renderRosterFixtureDirectory(directory, runner);
    assert.deepEqual(await renderRosterFixtureDirectory(directory, runner), first);
    assert.equal(JSON.parse(await readFile(join(directory, "host/listing.json"))).listing_id, "worldstream.qualification.archive-roster");
    await assert.rejects(renderRosterFixtureDirectory(directory, { ...runner, digest: "b".repeat(64) }), /immutable fixture collision/u);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test("controlled plan crosses authenticated loopback OpenRouter HTTP boundary with honest provider metrics", async () => {
  const apiKey = "roster-fixture-local-key-" + "x".repeat(32);
  const { server } = createRosterFixtureProvider({ WORLDSTREAM_FAKE_OPENROUTER_BIND: "127.0.0.1", WORLDSTREAM_FAKE_OPENROUTER_PORT: "19079",
    WORLDSTREAM_DEVELOPMENT_OPENROUTER_KEY: apiKey, WORLDSTREAM_DEVELOPMENT_FAKE_OPENROUTER: "visible-local-only",
    WORLDSTREAM_DEPLOYMENT_ENVIRONMENT: "development", NODE_ENV: "test" });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const origin = `http://127.0.0.1:${server.address().port}`;
  try {
    const request = { model: "controlled-fixture", provider: { only: ["fixture-endpoint"] },
      messages: [{ role: "user", content: JSON.stringify(invocation()) }], max_tokens: 1000 };
    const unauthorized = await fetch(`${origin}/api/v1/chat/completions`, { method: "POST", body: JSON.stringify(request) });
    assert.equal(unauthorized.status, 401);
    const response = await fetch(`${origin}/api/v1/chat/completions`, { method: "POST", headers: { authorization: `Bearer ${apiKey}` }, body: JSON.stringify(request) });
    assert.equal(response.status, 200);
    assert.equal(response.headers.get("x-worldstream-development-substitute"), "fake-openrouter");
    const completion = await response.json();
    assert.deepEqual(JSON.parse(completion.choices[0].message.content), controlledArchivePlan(invocation()));
    assert.equal(completion.openrouter_metadata.attempts[0].provider, "fixture-endpoint");
    const metrics = await (await fetch(`${origin}/development/metrics`, { headers: { authorization: `Bearer ${apiKey}` } })).json();
    assert.equal(metrics.house_completion_count, 1);
  } finally { await new Promise((done) => server.close(done)); }
});

test("Host evidence preserves consumed allowance and exact bindings while omitting authentication material", async () => {
  const directory = await mkdtemp(join(tmpdir(), "roster-evidence-check-"));
  const assignment = { runner_unit_id: `house-${"a".repeat(32)}`, assignment_id: "assignment-1", reservation_operation_id: "reservation-1",
    house_agent_revision_digest: `blake3:${"b".repeat(64)}`, profile_id: "fixture", profile_revision: "1", template_id: "fixture", template_revision: "1" };
  const put = async (path, value) => { const target = join(directory, path); await mkdir(join(target, ".."), { recursive: true }); await writeFile(target, JSON.stringify(value)); };
  try {
    await put("hosted-house-runners/units/allowance/allowances.json", { assignments: { [assignment.runner_unit_id]: {
      revision_digest: assignment.house_agent_revision_digest, consumed_input_units: 100, consumed_output_units: 20,
      attempts: { completed: { state: "completed", input_units: 100, output_units: 20, response_digest: "response" } } } } });
    await put(`hosted-house-runners/units/${assignment.runner_unit_id}/retired.json`, { allowance_reset: false, authentication_tag: "private-tag",
      receipt: { schema: "worldstream/house-runner-retirement-receipt/v1", disposition: "run_terminal",
        house_agent_assignment_id: assignment.assignment_id, reservation_operation_id: assignment.reservation_operation_id, authentication_tag: "private-tag" } });
    await put("runner-templates/installed/fixture--1.json", { executable: { blake3: "c".repeat(64) } });
    await put("agent-profiles/revisions/66697874757265/31.json", { profile_id: "fixture", secret_settings: [{ reference: "private-reference" }] });
    const evidence = await rosterHostSnapshot(assignment, directory);
    assert.equal(evidence.allowance.consumed_input_units, 100);
    assert.equal(evidence.retirement.allowance_reset, false);
    assert.match(evidence.bindings.profile_revision_digest, /^blake3:[0-9a-f]{64}$/u);
    assert.doesNotMatch(JSON.stringify(evidence), /private-tag|private-reference|authentication_tag|secret_settings/u);
    await assert.rejects(rosterHostSnapshot({ ...assignment, assignment_id: "wrong" }, directory));
  } finally { await rm(directory, { recursive: true, force: true }); }
});
