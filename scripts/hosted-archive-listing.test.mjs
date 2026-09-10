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

test("the internal candidate advances only to exact Archive Listing 0.3", async () => {
  const candidates = JSON.parse(await bytes("config/hosted/internal-candidates.json"));
  assert.deepEqual(candidates.candidates.map(({ listing_file, listing_digest }) => ({
    listing_file,
    listing_digest,
  })), [{
    listing_file: "config/hosted/listings/midnight-archive-0.3.0.json",
    listing_digest: LISTING_DIGEST,
  }]);
});
