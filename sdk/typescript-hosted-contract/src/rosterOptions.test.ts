import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { encodeCanonical, type CanonicalJson } from "@worldstream/pack-sdk";
import { deriveRoomSetup, readHouseAgentRevision, readListingRevision, resolveRosterOption } from "./index.js";

const read = (path: string) => JSON.parse(readFileSync(new URL(`../../../${path}`, import.meta.url), "utf8"));
const source = () => read("fixtures/hosted-contract/valid/roster-options-listing.json");
const house = ["cooperative-planner-1", "skeptical-auditor-1"].map((name) =>
  readHouseAgentRevision(encodeCanonical(read(`config/hosted/house-agents/${name}.json`))));

test("reviewed options derive exactly the selected seats and server configuration", () => {
  const listing = readListingRevision(encodeCanonical(source()));
  for (const id of ["solo", "first", "second", "both"]) {
    const option = resolveRosterOption(listing, { roster_option: id })!;
    const launch = { schema: "worldstream/launch-request/v2", listing_revision_digest: listing.digest,
      inputs: { roster_option: id }, creator: { participation: "seat", principal_reference: "seat:lead" } };
    const members = [{ seat_id: "lead", participation: "account_human", principal_reference: "seat:lead", display_name: "Lead" },
      ...option.house_agent_assignments.map((assignment) => {
        const revision = house.find((revision) => revision.digest === assignment.house_agent_revision_digest)!;
        return { ...assignment, participation: "house_agent_fill", principal_reference: `seat:${assignment.seat_id}`,
          display_name: revision.value.display_name, agent_profile: revision.value.agent_profile, runner_template: revision.value.runner_template };
      })];
    const roster = { schema: "worldstream/frozen-roster/v1", listing_revision_digest: listing.digest, members };
    const derive = (nextLaunch = launch, nextRoster: unknown = roster) => JSON.parse(new TextDecoder().decode(deriveRoomSetup(
      listing, encodeCanonical(nextLaunch), encodeCanonical(nextRoster as CanonicalJson), house)));
    const setup = derive();
    assert.deepEqual(setup.seats.map((seat: { label: string }) => seat.label), option.seat_ids);
    assert.deepEqual(setup.configuration, option.configuration);
    if (id !== "solo") {
      assert.throws(() => derive(launch, { ...roster, members: members.slice(0, -1) }), /invalid_shape/);
      assert.throws(() => derive({ ...launch, inputs: { roster_option: "solo" } }), /invalid_shape/);
      assert.throws(() => derive(launch, { ...roster, members: members.map((member, index) => index === 0 ? member : {
        seat_id: member.seat_id, participation: "account_external_agent", principal_reference: member.principal_reference, display_name: "Replaced",
      }) }), /invalid_shape|unsupported/);
    }
  }
});

test("only exact option IDs are accepted, with no input authority beyond that identifier", () => {
  const listing = readListingRevision(encodeCanonical(source()));
  for (const input of [{}, { roster_option: "unreviewed" }, { roster_option: null },
    ...["role", "principal_id", "prompt", "provider", "model", "code", "configuration", "setup"].map((field) => ({ roster_option: "solo", [field]: "arbitrary" }))]) {
    assert.throws(() => resolveRosterOption(listing, input));
  }
  const legacy = readListingRevision(encodeCanonical(read("config/hosted/listings/agent-heist-0.2.0.json")));
  assert.equal(resolveRosterOption(legacy, {}), null);
  assert.throws(() => resolveRosterOption(legacy, { roster_option: "solo" }));
});

test("reviewed schemas reject missing required seats, duplicated or unknown seats and assignments, and unbounded choices", () => {
  const mutations = [
    (v: any) => { v.roster_options[0].seat_ids = ["mira"]; },
    (v: any) => { v.roster_options[0].seat_ids = ["lead", "unknown"]; },
    (v: any) => { v.roster_options[0].seat_ids = ["lead", "lead"]; },
    (v: any) => { v.defaults.roster_option = "unknown"; },
    (v: any) => { v.roster_options[1].house_agent_assignments[0].house_agent_revision_digest = house[1]!.digest; },
    (v: any) => { v.roster_options[3].house_agent_assignments[1] = v.roster_options[3].house_agent_assignments[0]; },
    (v: any) => { v.roster_options = Array(17).fill(v.roster_options[0]); },
    (v: any) => { v.roster_options[0].prompt = "injected"; },
    (v: any) => { v.roster_options[0].description = "Any addition is closed in v2."; },
  ];
  for (const mutate of mutations) {
    const value = source(); mutate(value.launch_input_schema);
    assert.throws(() => readListingRevision(encodeCanonical(value)));
  }
});

test("v3 requires bounded reviewed descriptions while retained v2 stays closed", () => {
  const value = source();
  value.launch_input_schema.schema = "worldstream/launch-input-schema/v3";
  for (const option of value.launch_input_schema.roster_options) {
    option.description = `Reviewed formation description for ${option.label}.`;
  }
  const listing = readListingRevision(encodeCanonical(value));
  const selected = resolveRosterOption(listing, { roster_option: "first" });
  assert.ok(selected && "description" in selected);
  assert.equal(
    selected.description,
    value.launch_input_schema.roster_options[1].description,
  );
  for (const description of [undefined, "", "x".repeat(257)]) {
    const invalid = structuredClone(value);
    if (description === undefined) delete invalid.launch_input_schema.roster_options[0].description;
    else invalid.launch_input_schema.roster_options[0].description = description;
    assert.throws(() => readListingRevision(encodeCanonical(invalid)));
  }
});
