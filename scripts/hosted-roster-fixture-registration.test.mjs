import assert from "node:assert/strict";
import { test } from "node:test";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { readRosterFixture, renderRosterFixtureRunner } from "./hosted-roster-fixture.mjs";
import { rosterFixtureRegistrationSql } from "./hosted-roster-fixture-registration.mjs";

test("local registration requires explicit development mode before accessing Host state", async () => {
  await assert.rejects(rosterFixtureRegistrationSql({ environment: { NODE_ENV: "production" } }));
});

test("exact fixture registration is accepted twice by local database and rolls back all fixture writes", {
  skip: process.env.WORLDSTREAM_ROSTER_FIXTURE_DATABASE_TEST !== "visible-local-only",
}, async () => {
  const directory = await mkdtemp(join(tmpdir(), "roster-registration-check-"));
  try {
    const { fixture, profile } = await readRosterFixture();
    const { managed_provider_credential_id: ignored, ...published } = profile;
    const stored = { ...published, schema: "worldstream/studio-agent-profile/v1",
      secret_settings: [{ key: "MODEL_PROVIDER_TOKEN", kind: "model_provider", reference: "local-test-reference" }] };
    const hex = (value) => Buffer.from(value).toString("hex");
    const profileDirectory = join(directory, "agent-profiles/revisions", hex(fixture.profile_id));
    await mkdir(profileDirectory, { recursive: true });
    await writeFile(join(profileDirectory, `${hex(fixture.version)}.json`), JSON.stringify(stored));
    const template = await renderRosterFixtureRunner({ executable: "/tmp/qualification-test/managed-host", digest: "a".repeat(64) });
    await mkdir(join(directory, "runner-templates/installed"), { recursive: true });
    await writeFile(join(directory, "runner-templates/installed", `${fixture.template_id}--${fixture.version}.json`), JSON.stringify(template));
    const sql = await rosterFixtureRegistrationSql({ stateDirectory: directory, installationId: "local-transaction-fixture-check",
      importReceipt: { status: "complete", import_apply: { created_agent_profiles: [{ profile_id: profile.profile_id }], digest: `blake3:${"a".repeat(64)}` } },
      environment: { WORLDSTREAM_ROSTER_FIXTURE_QUALIFICATION: "visible-local-only", WORLDSTREAM_DEPLOYMENT_ENVIRONMENT: "development" } });
    const body = sql.replace(/^begin;\n/u, "").replace(/commit;$/u, "");
    const path = join(directory, "check.sql");
    await writeFile(path, `begin; set local worldstream.development_seed='visible-local-only';\n${body}\nupdate platform_store.house_agent_host_approvals set available_for_new_assignments=false where host_installation_id='local-transaction-fixture-check';\n${body}\nrollback;`, { mode: 0o600 });
    const local = JSON.parse(execFileSync("supabase", ["status", "-o", "json"], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }));
    assert.ok(new URL(local.DB_URL).hostname === "127.0.0.1", "registration check is local-only");
    execFileSync("psql", [local.DB_URL, "-q", "-v", "ON_ERROR_STOP=1", "-f", path], { stdio: ["ignore", "pipe", "pipe"] });
  } finally { await rm(directory, { recursive: true, force: true }); }
});
