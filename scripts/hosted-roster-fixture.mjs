import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { encodeCanonical, taggedBlake3 } from "../sdk/typescript-pack/packages/pack-sdk/dist/index.js";
import { readHouseAgentRevision, readListingRevision } from "../sdk/typescript-hosted-contract/dist/index.js";

const repository = resolve(fileURLToPath(new URL("..", import.meta.url)));
const source = "config/hosted/fixtures/roster-options/fixture.json";
const json = async (path) => JSON.parse(await readFile(resolve(repository, path), "utf8"));

/** Explicit qualification-only artifacts; never included in the production registry. */
export async function readRosterFixture() {
  const fixture = await json(source);
  assert.equal(fixture.execution, "visible-local-controlled-provider-only");
  const base = readListingRevision(encodeCanonical(await json(fixture.base_listing_file)));
  assert.equal(base.digest, fixture.base_listing_digest, "retained fixture base changed");
  const routeSource = await json(fixture.route_source_file);
  const house = readHouseAgentRevision(encodeCanonical({
    schema: "worldstream/house-agent-revision/v1",
    house_agent_id: fixture.house_agent_id, version: fixture.version,
    display_name: "Qualification assistant (controlled provider)",
    behavior_policy: { policy_id: fixture.policy_id, revision: fixture.version, instructions: fixture.policy_instructions },
    route: routeSource.route,
    agent_profile: { profile_id: fixture.profile_id, revision: fixture.version },
    runner_template: { template_id: fixture.template_id, revision: fixture.version },
    tools: [], accounting_tokenizer: routeSource.accounting_tokenizer, allowance: routeSource.allowance,
  }));
  const listing = readListingRevision(encodeCanonical({
    ...base.value, listing_id: fixture.listing_id, version: fixture.version,
    title: "Archive roster qualification",
    description: "Local controlled-provider fixture. Solo or one supplied assistant; 16 turns and 3 power. No paid execution, invitations, or cross-run memory.",
    catalog: { visibility: "unlisted", review_status: "reviewed" },
    launch_input_schema: { schema: "worldstream/launch-input-schema/v2", accepts: "roster_option",
      defaults: { roster_option: "solo" }, roster_options: [
        { option_id: "solo", label: "Solo fixture", seat_ids: ["lead"], configuration: { scenario_id: "standard-v1" }, house_agent_assignments: [] },
        { option_id: "supplied", label: "Supplied assistant fixture", seat_ids: ["lead", "mira"], configuration: { scenario_id: "standard-v1" },
          house_agent_assignments: [{ seat_id: "mira", house_agent_revision_digest: house.digest }] },
      ] },
    seats: [base.value.seats[0], { seat_id: "mira", role: "mira", display_name: "Qualification assistant",
      required: false, allowed_participation: ["house_agent_fill"], allowed_house_agent_revisions: [house.digest] }],
  }));
  const profile = { schema: "worldstream/studio-agent-profile-publish/v2",
    profile_id: fixture.profile_id, revision: fixture.version, display_name: "Controlled Archive qualification assistant",
    non_secret_configuration: {}, host_contract: { kind: "managed_house_openrouter", host_contract_revision: "1",
      runner_template: { template_id: fixture.template_id, revision: fixture.version } },
    managed_provider_credential_id: "hosted-openrouter" };
  return { fixture, listing, house, profile };
}

export async function renderRosterFixtureRunner({ executable, digest }) {
  assert.match(digest, /^[0-9a-f]{64}$/u);
  assert.equal(typeof executable, "string");
  assert.ok(executable.startsWith("/"), "retained executable must use an absolute path");
  const { fixture, listing } = await readRosterFixture();
  return { schema: "worldstream/runner-template/v1", template_id: fixture.template_id, revision: fixture.version,
    display_name: "Local controlled Archive qualification Runner", executable: { path: executable, blake3: digest },
    compatibility: [{ activity_pack_id: listing.value.pack.id, exact_revisions: [listing.value.pack.version] }],
    capacity: { maximum_concurrent_invocations: 1 },
    health: { path: "/healthz", timeout_ms: 1000, stale_after_ms: 60000 },
    non_secret_environment: { WORLDSTREAM_RUNNER_MODE: "hosted-house" }, secret_environment: [],
    instances: [{ instance_id: "qualification-archive-house-1", health_address: "127.0.0.1:9607" }] };
}

/** Uses only one current authorized frame/reset, never hidden Pack state. */
export function controlledArchivePlan(input) {
  if (input?.schema !== "worldstream/house-model-invocation/v1") return null;
  const observation = input.projection;
  if (observation?.schema !== "worldstream/assignment-observation/v1"
      || observation.role !== "mira" || observation.pack?.id !== "worldstream.midnight-archive") return null;
  const frames = observation.observations;
  if (!Array.isArray(frames)) return null;
  const last = frames.at(-1);
  const activity = last === undefined ? observation.projection_reset?.projection?.activity : last.observation;
  if (last !== undefined && (last.frame_seq !== observation.stream?.frame_head
      || last.observation_schema !== "worldstream.midnight-archive/participant-observation/v3")) return null;
  if (last === undefined && (observation.projection_reset?.baseline_frame_head !== observation.stream?.frame_head
      || observation.projection_reset?.projection_schema !== "worldstream.midnight-archive/participant-projection/v3")) return null;
  const companion = activity?.mira;
  if (activity?.phase !== "active" || companion?.location !== "records" || companion.presence !== "active"
      || companion.task?.kind !== "investigate_records" || companion.task.status !== "assigned"
      || companion.planning?.status !== "waiting") return null;
  const offers = input.action_offers?.offers;
  if (!Array.isArray(offers)) return null;
  const offer = offers.find((item) => item.action_type === "submit_companion_plan");
  if (typeof offer?.offer_id !== "string") return null;
  const revision = companion.task.revision;
  const opportunity = companion.planning.opportunity_revision;
  if (!Number.isSafeInteger(revision) || revision < 1 || !Number.isSafeInteger(opportunity) || opportunity < 1) return null;
  const knowledge = companion.knowledge?.records;
  if (knowledge !== "unknown" && knowledge !== "private") return null;
  const step = (type) => ({ step_type: type, destination: "none", source_id: "records", power_cost: 0 });
  const payload = { task_revision: revision, opportunity_revision: opportunity,
    steps: [...(knowledge === "unknown" ? [step("inspect_source")] : []), step("share_source")] };
  return { offer_id: offer.offer_id, payload };
}

/** Checks observable correspondence; it never repairs retained state. */
export function assertRosterLineage(snapshot, { option, prior } = {}) {
  assert.ok(option === "solo" || option === "supplied");
  assert.equal(snapshot.roster_option, option);
  assert.equal(snapshot.runs_for_launch, 1);
  const roles = option === "solo" ? ["lead"] : ["lead", "mira"];
  assert.deepEqual(snapshot.memberships.map((entry) => entry.role).sort(), roles);
  assert.equal(snapshot.assignments.length, roles.length - 1);
  assert.ok(snapshot.memberships.every((entry) => typeof entry.membership_id === "string" && typeof entry.principal_id === "string"));
  assert.equal(new Set(snapshot.memberships.map((entry) => entry.membership_id)).size, roles.length);
  assert.equal(new Set(snapshot.memberships.map((entry) => entry.principal_id)).size, roles.length);
  const lead = snapshot.memberships.find((entry) => entry.role === "lead");
  assert.equal(lead.participation_source, "account_human");
  assert.equal(lead.assignment_id, null);
  if (option === "supplied") {
    const companion = snapshot.memberships.find((entry) => entry.role === "mira");
    assert.equal(companion.participation_source, "platform_house_agent");
    assert.equal(companion.assignment_id, snapshot.assignments[0].assignment_id);
    assert.equal(snapshot.assignments[0].seat_id, "mira");
  }
  if (prior !== undefined) {
    for (const field of ["room_id", "run_id", "launch_id", "roster_option", "memberships", "assignments", "launch_input_digest", "frozen_roster_digest", "room_setup_specification_digest"])
      assert.deepEqual(snapshot[field], prior[field], `retry changed ${field}`);
  }
}

export function rosterFixtureDigest(value) { return taggedBlake3(encodeCanonical(value)); }
