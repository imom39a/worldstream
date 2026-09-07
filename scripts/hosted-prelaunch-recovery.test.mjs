import { strict as assert } from "node:assert";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import {
  EMPTY_PLATFORM_TABLES,
  verifyPrelaunchController, validatePrelaunchPlatform,
} from "./hosted-prelaunch-recovery.mjs";
import { seedPrelaunchFixture } from "./fixtures/hosted-prelaunch.mjs";

test("zero-history recovery rejects empty/unrelated Controller state and retained activity", async () => {
  const root = await mkdtemp(join(tmpdir(), "worldstream-prelaunch-proof-test-"));
  try {
    const { database, studio } = await seedPrelaunchFixture(root);
    const receipt = { rooms_replayed: 0, isolated_rooms_skipped: 0 };
    const proof = await verifyPrelaunchController({ isolatedDirectory: root, runtimeReadiness: receipt });
    assert.equal(proof.controller_host_authority, "matched_enabled_runtime_host");
    const secretPath = join(studio, "secrets", `host-${"a".repeat(64)}.secret`);
    await writeFile(secretPath, Buffer.alloc(32, 8), { mode: 0o600 });
    await assert.rejects(() => verifyPrelaunchController({ isolatedDirectory: root, runtimeReadiness: receipt }),
      /controller_runtime_authority_mismatch/u);
    await writeFile(secretPath, Buffer.alloc(32, 7), { mode: 0o600 });
    await writeFile(join(studio, "hosted-launches", "retained.json"), "{}", { mode: 0o600 });
    await assert.rejects(() => verifyPrelaunchController({ isolatedDirectory: root, runtimeReadiness: receipt }),
      /populated_checkpoint_correspondence_required/u);
    await rm(join(studio, "hosted-launches", "retained.json"));
    const db = new DatabaseSync(database);
    db.exec("insert into rooms values ('a-room');");
    db.close();
    await assert.rejects(() => verifyPrelaunchController({ isolatedDirectory: root, runtimeReadiness: receipt }),
      /populated_checkpoint_correspondence_required/u);
    await assert.rejects(() => verifyPrelaunchController({ isolatedDirectory: join(root, "absent"), runtimeReadiness: receipt }));
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("zero-history recovery refuses platform history and open admission", () => {
  const counts = Object.fromEntries(EMPTY_PLATFORM_TABLES.map((table) => [table, 0]));
  const platform = { installation_id: "fly-primary", launches_open: false, house_fill_open: false,
    maintenance_mode: true, recovery_fenced: false, counts };
  assert.equal(validatePrelaunchPlatform(platform, "fly-primary").platform_activity_records, 0);
  assert.throws(() => validatePrelaunchPlatform({ ...platform, launches_open: true }, "fly-primary"), /prelaunch_platform_not_closed/u);
  assert.throws(() => validatePrelaunchPlatform({ ...platform, counts: { ...counts, activity_runs: 1 } }, "fly-primary"), /populated_checkpoint_correspondence_required/u);
  assert.throws(() => validatePrelaunchPlatform({ ...platform, counts: {} }, "fly-primary"), /populated_checkpoint_correspondence_required/u);
});
