import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

const command = new URL("./collect-native-reentry-diagnostics.mjs", import.meta.url);
const environment = { ...process.env, CI: "true", WORLDSTREAM_REENTRY_DIAGNOSTICS: "visible-local-only" };
function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), "native-reentry-collector-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  return join(directory, "trace.jsonl");
}
function run(operation, file, env = environment) {
  return spawnSync(process.execPath, [command.pathname, operation], {
    encoding: "utf8", timeout: 5000,
    env: { ...env, WORLDSTREAM_REENTRY_NATIVE_TRACE_FILE: file },
  });
}

test("native collector creates an exclusive private file and emits only validated records", (t) => {
  const file = fixture(t);
  const prepared = run("prepare", file);
  assert.equal(prepared.status, 0);
  assert.equal(statSync(file).mode & 0o777, 0o600);
  assert.equal(readFileSync(file, "utf8"), "");
  assert.equal(run("prepare", file).status, 1);
  const record = { stage: "membership_status_http", status: 429, category: "rate_limited" };
  writeFileSync(file, `${JSON.stringify(record)}\n`);
  const collected = run("collect", file);
  assert.equal(collected.status, 0);
  assert.equal(collected.stdout, `[DEBUG-reentry-native] ${JSON.stringify(record)}\n`);
  assert.equal(collected.stderr, "");
  assert.equal(readFileSync(file, "utf8"), `${JSON.stringify(record)}\n`);
});

test("native collector rejects arbitrary text, extra fields and unbounded input without reflecting it", (t) => {
  const file = fixture(t);
  const record = { stage: "membership_status_http", status: 503, category: "service_unavailable" };
  const valid = `${JSON.stringify(record)}\n`;
  for (const content of [
    "private-sentinel\n", `${valid}{malformed-private-sentinel}\n`,
    `${JSON.stringify({ ...record, token: "private-sentinel" })}\n`,
    `${JSON.stringify({ ...record, category: "private-sentinel" })}\n`,
    `${JSON.stringify({ ...record, status: 999 })}\n`,
    `${JSON.stringify({ ...record, stage: "private-sentinel" })}\n`,
    `${JSON.stringify({ ...record, status: "503" })}\n`,
    `${JSON.stringify({ ...record, category: "rate_limited" })}\n`,
    valid.trimEnd(), valid.repeat(129), "x".repeat(16385),
  ]) {
    writeFileSync(file, content, { mode: 0o600 });
    const result = run("collect", file);
    assert.equal(result.status, 1);
    assert.equal(result.stdout, "");
    assert.equal(result.stderr, "[DEBUG-reentry-native] diagnostic_file_rejected\n");
  }
  writeFileSync(file, valid.repeat(128));
  const bounded = run("collect", file);
  assert.equal(bounded.status, 0);
  assert.equal(bounded.stdout.trimEnd().split("\n").length, 128);
});

test("native collector refuses unsafe file permissions, symlinks and inactive CI gates", (t) => {
  const file = fixture(t);
  writeFileSync(file, "", { mode: 0o600 });
  chmodSync(file, 0o644);
  assert.equal(run("collect", file).status, 1);
  chmodSync(file, 0o600);
  const link = `${file}.link`;
  symlinkSync(file, link);
  assert.equal(run("collect", link).status, 1);
  for (const env of [
    { ...environment, CI: "" },
    { ...environment, WORLDSTREAM_REENTRY_DIAGNOSTICS: "" },
    { ...environment, WORLDSTREAM_REENTRY_DIAGNOSTICS: "wrong" },
  ]) assert.equal(run("collect", file, env).status, 1);
  const timeout = { stage: "membership_status_read", status: null, category: "timeout" };
  writeFileSync(file, `${JSON.stringify(timeout)}\n`);
  assert.equal(run("collect", file).stdout, `[DEBUG-reentry-native] ${JSON.stringify(timeout)}\n`);
});
