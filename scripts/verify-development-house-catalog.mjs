import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

// Run on the host: the Supabase pgTAP container copies individual test files,
// not the development seed included by this rollback-only integration check.
const repository = fileURLToPath(new URL("..", import.meta.url));
const status = execFileSync("supabase", ["status", "-o", "env"], {
  cwd: repository, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"],
});
const database = JSON.parse(status.split("\n").find((line) => line.startsWith("DB_URL="))?.slice(7) ?? "null");
assert.equal(typeof database, "string", "local database is unavailable");
const local = new URL(database);
assert.ok(["127.0.0.1", "localhost", "[::1]"].includes(local.hostname), "local database is required");
const result = spawnSync("psql", ["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "-f",
  "scripts/fixtures/development-house-catalog.test.sql"], {
  cwd: repository, encoding: "utf8", timeout: 30_000,
  env: { ...process.env, PGHOST: local.hostname, PGPORT: local.port,
    PGUSER: decodeURIComponent(local.username), PGPASSWORD: decodeURIComponent(local.password),
    PGDATABASE: decodeURIComponent(local.pathname.slice(1)) },
  stdio: ["ignore", "pipe", "pipe"],
});
// Do not echo database connection errors, seed data or environment values.
const checks = (result.stdout ?? "").split("\n").filter((line) => /^(?:not )?ok \d+ /u.test(line));
for (const check of checks) console.log(check);
assert.equal(result.status, 0, "local development seed check could not complete");
assert.equal(checks.length, 4, "local development seed check was incomplete");
assert.ok(checks.every((line) => line.startsWith("ok ")), "local development House catalog is incompatible");
