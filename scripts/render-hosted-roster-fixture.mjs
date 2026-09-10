import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";
import { readRosterFixture, renderRosterFixtureRunner, rosterFixtureDigest } from "./hosted-roster-fixture.mjs";

/** Writes exact qualification inputs; an existing different file is never replaced. */
export async function renderRosterFixtureDirectory(directory, runner) {
  const root = resolve(directory);
  const { fixture, listing, house, profile } = await readRosterFixture();
  const template = await renderRosterFixtureRunner(runner);
  const descriptor = { schema: "worldstream/hosted-roster-fixture-descriptor/v1", execution: fixture.execution,
    slug: fixture.slug, listing_digest: listing.digest, house_agent_digest: house.digest,
    pack: listing.value.pack, client: listing.value.client,
    profile_digest: rosterFixtureDigest(profile), runner_template_digest: rosterFixtureDigest(template),
    source: "config/hosted/fixtures/roster-options/fixture.json" };
  for (const [name, value] of [["host/listing.json", listing.value], ["host/house-agent-1.json", house.value],
    ["imports/profile.json", profile], ["imports/runner-template.json", template], ["descriptor.json", descriptor]]) {
    const path = join(root, name);
    const bytes = JSON.stringify(value, null, 2) + "\n";
    await mkdir(resolve(path, ".."), { recursive: true, mode: 0o700 });
    try { await writeFile(path, bytes, { flag: "wx", mode: 0o600 }); }
    catch (error) {
      if (error.code !== "EEXIST") throw error;
      assert.equal(await readFile(path, "utf8"), bytes, `immutable fixture collision: ${name}`);
    }
  }
  return descriptor;
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === resolve(import.meta.filename)) {
  const [directory, executable, digest] = process.argv.slice(2);
  assert.ok(directory && executable && digest, "usage: node scripts/render-hosted-roster-fixture.mjs OUTPUT RETAINED_EXECUTABLE RAW_BLAKE3");
  console.log(JSON.stringify(await renderRosterFixtureDirectory(directory, { executable, digest }), null, 2));
}
