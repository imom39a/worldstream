import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, readFile, realpath, rm, stat, symlink, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import { test } from "node:test";
import { canonicalBytes, prepareHouseApprovals, writeApprovalFiles } from "./hosted-house-approval.mjs";

const require = createRequire(new URL("../sdk/typescript-pack/packages/pack-sdk/package.json", import.meta.url));
const { blake3 } = await import(pathToFileURL(require.resolve("@noble/hashes/blake3.js")));
const hash = (bytes) => Buffer.from(blake3(bytes)).toString("hex");

async function fixture(t) {
  const root = await realpath(await mkdtemp(join(tmpdir(), "worldstream-house-approval-test-")));
  t.after(() => rm(root, { recursive: true, force: true }));
  const house = JSON.parse(await readFile(new URL("../config/hosted/house-agents/cooperative-planner-1.json", import.meta.url), "utf8"));
  const binary = Buffer.from("test-only approved executable bytes");
  const reference = "a".repeat(64);
  const profile = {
    schema: "worldstream/studio-agent-profile/v1", ...house.agent_profile,
    display_name: "Cooperative Planner", non_secret_configuration: {},
    secret_settings: [{ key: "MODEL_PROVIDER_TOKEN", kind: "model_provider", reference }],
    host_contract: { kind: "managed_house_openrouter", host_contract_revision: "1", runner_template: house.runner_template },
  };
  const runner = {
    schema: "worldstream/runner-template/v1", ...house.runner_template,
    display_name: "Hosted OpenRouter House Runner",
    executable: { path: "/usr/local/bin/worldstream-managed-agent-host", blake3: hash(binary) },
    compatibility: [{ activity_pack_id: "worldstream.agent-heist", exact_revisions: ["0.2.0"] }],
    capacity: { maximum_concurrent_invocations: 1 },
    health: { path: "/healthz", timeout_ms: 1000, stale_after_ms: 60000 },
    non_secret_environment: { WORLDSTREAM_RUNNER_MODE: "hosted-house" }, secret_environment: [],
    instances: [{ instance_id: "hosted-house-01", health_address: "127.0.0.1:9591" }],
  };
  const report = {
    status: "complete", code: "complete", import_apply: {
      schema: "worldstream/initialization-import-apply/v1", digest: `blake3:${"b".repeat(64)}`,
      services_started: false, created_agent_profiles: [], reused_agent_profiles: [house.agent_profile],
      created_runner_templates: [], reused_runner_templates: [runner],
      created_provider_credentials: [], reused_provider_credentials: ["hosted-openrouter"],
    },
  };
  const state = join(root, "studio");
  const profilePath = join(state, "agent-profiles/revisions", Buffer.from(profile.profile_id).toString("hex"), "31.json");
  const runnerPath = join(state, "runner-templates/installed/openrouter-house--1.json");
  const providerPath = join(state, "model-provider-credentials/installed/hosted-openrouter.json");
  const write = async (path, value) => {
    await mkdir(dirname(path), { recursive: true, mode: 0o700 });
    await writeFile(path, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
  };
  await write(profilePath, profile);
  await write(runnerPath, runner);
  await write(providerPath, {
    schema: "worldstream/model-provider-credential/v1", credential_id: "hosted-openrouter",
    display_name: "Hosted OpenRouter", provider: "openrouter", secret: { kind: "model_provider", reference },
  });
  const importReceipt = join(root, "import-apply.json"), housePath = join(root, "house.json"), runnerBinary = join(root, "runner");
  await write(importReceipt, report);
  await write(housePath, house);
  await writeFile(runnerBinary, binary, { mode: 0o700 });
  // A secret-looking sentinel must remain unread; it is never a tool input.
  await mkdir(join(state, "secrets"), { mode: 0o700 });
  await writeFile(join(state, "secrets", "do-not-read"), "sk-test-MUST-NOT-APPEAR", { mode: 0o000 });
  return {
    root, profile, profilePath, runner, runnerPath, report, reference, write,
    options: { controllerState: state, runnerBinary, importReceipt,
      houseRevisions: [housePath], credentialId: "hosted-openrouter", installationId: "approval-test",
      sourceRevision: "c".repeat(40), imageDigest: `sha256:${"d".repeat(64)}` },
  };
}

test("approval recipe pins canonical installed metadata and actual binary without leaking vault references", async (t) => {
  const f = await fixture(t);
  const prepared = await prepareHouseApprovals(f.options);
  const { receipt, approval_receipt_digest } = prepared.evidence[0];
  assert.equal(receipt.agent_profile_revision_digest, `blake3:${hash(canonicalBytes(f.profile))}`);
  assert.equal(receipt.runner_template_revision_digest, `blake3:${hash(canonicalBytes(f.runner))}`);
  assert.equal(receipt.runner_executable_digest, `blake3:${f.runner.executable.blake3}`);
  assert.equal(receipt.initialization_import_review_digest, f.report.import_apply.digest);
  assert.notEqual(receipt.agent_profile_revision_digest, receipt.initialization_import_review_digest);
  assert.equal(approval_receipt_digest, `sha256:${createHash("sha256").update(canonicalBytes(receipt)).digest("hex")}`);
  assert.equal(JSON.stringify(prepared).includes(f.reference), false);
  assert.equal(JSON.stringify(prepared).includes("sk-test-MUST-NOT-APPEAR"), false);
  assert.match(prepared.approvalSql, /false\)\n  on conflict/u);
  assert.doesNotMatch(prepared.approvalSql, /set available_for_new_assignments = true/u);
  assert.match(prepared.activationSql, /house_approval_identity_conflict/u);
  assert.match(prepared.activationSql, /revoked_at is null/u);
  await f.write(f.profilePath, Object.fromEntries(Object.entries(f.profile).reverse()));
  assert.deepEqual((await prepareHouseApprovals(f.options)).evidence, prepared.evidence);
});

test("approval preparation rejects changed executable bytes and cross-credential bindings", async (t) => {
  const f = await fixture(t);
  await writeFile(f.options.runnerBinary, "different executable", { mode: 0o700 });
  await assert.rejects(prepareHouseApprovals(f.options), /approval_runner_binding_invalid/u);
  await writeFile(f.options.runnerBinary, "test-only approved executable bytes", { mode: 0o700 });
  f.profile.secret_settings[0].reference = "f".repeat(64);
  await f.write(f.profilePath, f.profile);
  await assert.rejects(prepareHouseApprovals(f.options), /approval_profile_credential_mismatch/u);
});

test("approval preparation rejects unreviewed templates, duplicate JSON, symlinks and SQL-shaped identifiers", async (t) => {
  const f = await fixture(t);
  await assert.rejects(prepareHouseApprovals({ ...f.options, installationId: "x';delete" }), /approval_options_invalid/u);
  f.report.import_apply.reused_runner_templates = [];
  await f.write(f.options.importReceipt, f.report);
  await assert.rejects(prepareHouseApprovals(f.options), /approval_runner_binding_invalid/u);
  f.report.import_apply.reused_runner_templates = [f.runner];
  await f.write(f.options.importReceipt, f.report);
  await writeFile(f.profilePath, JSON.stringify(f.profile).replace('"schema":', '"schema":"duplicate","schema":'), { mode: 0o600 });
  await assert.rejects(prepareHouseApprovals(f.options), /approval_json_ambiguous/u);
  await f.write(f.profilePath, f.profile);
  const link = join(f.root, "runner-link");
  await symlink(f.options.runnerBinary, link);
  await assert.rejects(prepareHouseApprovals({ ...f.options, runnerBinary: link }), /approval_file_path_invalid/u);
});

test("approval files are private, split from activation, and never overwritten", async (t) => {
  const f = await fixture(t);
  const prepared = await prepareHouseApprovals(f.options);
  const output = join(f.root, "output");
  await writeApprovalFiles(output, prepared);
  for (const name of ["receipts.json", "approve.sql", "activate.sql"]) {
    assert.equal((await stat(join(output, name))).mode & 0o777, 0o600);
  }
  assert.equal((await stat(output)).mode & 0o777, 0o700);
  await assert.rejects(writeApprovalFiles(output, prepared), /EEXIST/u);
  assert.deepEqual(JSON.parse(await readFile(join(output, "receipts.json"), "utf8")), prepared.evidence);
});

test("generated SQL retries exactly, activates separately and rejects changed evidence or revocation", {
  skip: process.env.WORLDSTREAM_APPROVAL_TEST_DATABASE_URL === undefined,
}, async (t) => {
  const f = await fixture(t);
  f.options.installationId = `approval-test-${process.pid}`;
  const prepared = await prepareHouseApprovals(f.options);
  const insideTransaction = (sql) => sql.replace(/^begin;$/mu, "").replace(/^commit;$/mu, "");
  const run = (sql) => spawnSync("psql", [process.env.WORLDSTREAM_APPROVAL_TEST_DATABASE_URL, "-X", "-v", "ON_ERROR_STOP=1"], {
    input: sql, encoding: "utf8", timeout: 10_000,
  });
  const success = run(`begin;\n${insideTransaction(prepared.approvalSql)}\n${insideTransaction(prepared.approvalSql)}
do $$ begin
  if (select available_for_new_assignments from platform_store.house_agent_host_approvals where host_installation_id = '${f.options.installationId}') is distinct from false then
    raise exception 'must start unavailable';
  end if;
end $$;
${insideTransaction(prepared.activationSql)}
do $$ begin
  if (select available_for_new_assignments from platform_store.house_agent_host_approvals where host_installation_id = '${f.options.installationId}') is distinct from true then
    raise exception 'activation did not occur';
  end if;
end $$;
rollback;`);
  assert.equal(success.status, 0, success.stderr);
  const changed = await prepareHouseApprovals({ ...f.options, imageDigest: `sha256:${"e".repeat(64)}` });
  const conflict = run(`begin;\n${insideTransaction(prepared.approvalSql)}\n${insideTransaction(changed.approvalSql)}\nrollback;`);
  assert.notEqual(conflict.status, 0);
  assert.match(conflict.stderr, /house_approval_identity_conflict/u);
  // ON_ERROR_STOP closes the failed transaction; no approval survives that check.
  const revoked = run(`begin;\n${insideTransaction(prepared.approvalSql)}
update platform_store.house_agent_host_approvals
  set available_for_new_assignments = false, revoked_at = clock_timestamp()
  where host_installation_id = '${f.options.installationId}';
do $revocation_test$
begin
  begin
    execute $activation_sql$${insideTransaction(prepared.activationSql)}$activation_sql$;
    raise exception 'revoked approval activation unexpectedly succeeded';
  exception when sqlstate '23505' then
    if sqlerrm <> 'house_approval_identity_conflict' then raise; end if;
  end;
  if not exists (select 1 from platform_store.house_agent_host_approvals
    where host_installation_id = '${f.options.installationId}'
      and available_for_new_assignments = false and revoked_at is not null) then
    raise exception 'revoked approval was re-enabled or lost';
  end if;
end;
$revocation_test$;
rollback;`);
  assert.equal(revoked.status, 0, revoked.stderr);
});
