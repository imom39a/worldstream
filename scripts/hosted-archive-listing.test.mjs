import { strict as assert } from "node:assert";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

import {
  encodeCanonical,
  taggedBlake3,
} from "../sdk/typescript-pack/packages/pack-sdk/dist/index.js";
import {
  readHouseAgentRevision,
  readListingRevision,
} from "../sdk/typescript-hosted-contract/dist/index.js";

const LISTING_DIGEST = "blake3:cc1c92ebc6ba7cccc9474186ff8107cf97f6bd0ce2676c6d1a2aa203c2a62d35";
const HOUSE_DIGESTS = [
  "blake3:7e0b07b386009d509d605c9efdbe491a035f219d10ef7ebebc6f71e99461cdde",
  "blake3:b88da2260f391c91593996c5913961469619b53a9e457c5ea0783cd3cac59db0",
];

async function bytes(path) {
  return readFile(new URL(`../${path}`, import.meta.url));
}

async function canonical(path) {
  return Buffer.from(encodeCanonical(JSON.parse(await bytes(path))));
}

test("Archive 0.3 registers exact immutable House and Listing documents without granting Host approval", async () => {
  const migration = await bytes("supabase/migrations/20260910130000_midnight_archive_four_rosters.sql");
  const sql = migration.toString("utf8");
  const embeddedHouses = [...sql.matchAll(/\$house\$([\s\S]*?)\$house\$/gu)]
    .map((match) => Buffer.from(match[1]));
  const embeddedListing = Buffer.from(sql.match(/\$artifact\$([\s\S]*?)\$artifact\$/u)?.[1] ?? "");
  const sourceHouses = await Promise.all([
    canonical("config/hosted/house-agents/mira-1.json"),
    canonical("config/hosted/house-agents/jonah-1.json"),
  ]);
  const sourceListing = await canonical("config/hosted/listings/midnight-archive-0.3.0.json");

  assert.deepEqual(embeddedHouses, sourceHouses);
  assert.deepEqual(embeddedListing, sourceListing);
  assert.deepEqual(sourceHouses.map((document) => readHouseAgentRevision(document).digest), HOUSE_DIGESTS);
  assert.equal(readListingRevision(sourceListing).digest, LISTING_DIGEST);
  assert.equal(taggedBlake3(sourceListing), LISTING_DIGEST);
  for (const digest of [...HOUSE_DIGESTS, LISTING_DIGEST]) assert.ok(sql.includes(digest));
  assert.ok(sql.indexOf("insert into platform_store.house_agent_revisions") <
    sql.indexOf("insert into platform_store.activity_listing_revisions"));
  assert.doesNotMatch(
    sql,
    /house_agent_host_approvals|hosted_operating_state|available_for_new_assignments|update\s+platform_store|delete\s+from/iu,
  );
});

test("the internal candidate advances only to exact Archive Listing 0.5", async () => {
  const candidates = JSON.parse(await bytes("config/hosted/internal-candidates.json"));
  assert.deepEqual(candidates.candidates.map(({ listing_file, listing_digest }) => ({
    listing_file,
    listing_digest,
  })), [{
    listing_file: "config/hosted/listings/midnight-archive-0.5.0.json",
    listing_digest: "blake3:b2a69e00c4c617b97dce75a0a5f3d0b5278d116284774400cd4ba1720e77e573",
  }]);
});

for (const [client, version, predecessor] of [
  ["agent-heist", "0.29.0", "0.28.0"],
  ["midnight-archive", "0.4.0", "0.3.0"],
]) {
  test(`${client} ${version} seeds only the exact client successor and preserves its activity contract`, async () => {
    const source = await canonical(`config/hosted/listings/${client}-${version}.json`);
    const previous = JSON.parse(await bytes(`config/hosted/listings/${client}-${predecessor}.json`));
    const listing = readListingRevision(source);
    assert.deepEqual(listing.value, { ...previous, version, client: listing.value.client });
    assert.notEqual(listing.value.client.release_digest, previous.client.release_digest);
    const sql = (await bytes(`supabase/catalog/${client}-${version}.sql`)).toString("utf8");
    assert.deepEqual(Buffer.from(sql.match(/\$artifact\$([\s\S]*?)\$artifact\$/u)?.[1] ?? ""), source);
    assert.ok(sql.includes(listing.digest));
    assert.doesNotMatch(sql, /house_agent_host_approvals|hosted_operating_state|available_for_new_assignments|\b(?:alter|drop|truncate|delete|update|create)\s/iu);
    assert.match(sql, /on conflict \(listing_revision_digest\) do nothing/iu);
    assert.match(sql, /canonical_document = expected_document/iu);
  });
}

test("acquisition recovery successors append exact House and Listing bytes without changing gameplay or granting approval", async () => {
  const sql = (await bytes("supabase/migrations/20260912160000_managed_host_acquisition_recovery_successors.sql")).toString("utf8");
  const sourceHouses = await Promise.all([
    "cooperative-planner-18", "skeptical-auditor-17", "mira-2", "jonah-2",
  ].map((name) => canonical(`config/hosted/house-agents/${name}.json`)));
  assert.deepEqual([...sql.matchAll(/\$house\$([\s\S]*?)\$house\$/gu)].map((match) => Buffer.from(match[1])), sourceHouses);
  const sourceListings = await Promise.all([
    "agent-heist-0.30.0", "midnight-archive-0.5.0",
  ].map((name) => canonical(`config/hosted/listings/${name}.json`)));
  assert.deepEqual([...sql.matchAll(/\$artifact\$([\s\S]*?)\$artifact\$/gu)].map((match) => Buffer.from(match[1])), sourceListings);
  for (const document of [...sourceHouses, ...sourceListings]) assert.ok(sql.includes(taggedBlake3(document)));
  for (const [name, before, after] of [
    ["cooperative-planner", 17, 18], ["skeptical-auditor", 16, 17], ["mira", 1, 2], ["jonah", 1, 2],
  ]) {
    const old = JSON.parse(await bytes(`config/hosted/house-agents/${name}-${before}.json`));
    const current = JSON.parse(await bytes(`config/hosted/house-agents/${name}-${after}.json`));
    assert.deepEqual(current, { ...old, version: String(after),
      agent_profile: { ...old.agent_profile, revision: String(after) },
      runner_template: { ...old.runner_template, revision: String(name === "mira" || name === "jonah" ? 2 : 17) },
    });
  }
  for (const [name, before, after] of [["agent-heist", "0.29.0", "0.30.0"], ["midnight-archive", "0.4.0", "0.5.0"]]) {
    const old = JSON.parse(await bytes(`config/hosted/listings/${name}-${before}.json`));
    const current = JSON.parse(await bytes(`config/hosted/listings/${name}-${after}.json`));
    assert.deepEqual(current, { ...old, version: after, seats: current.seats, launch_input_schema: current.launch_input_schema });
  }
  assert.doesNotMatch(sql, /house_agent_host_approvals|hosted_operating_state|\b(?:update|delete|alter|drop|truncate)\s/iu);
});
