import { strict as assert } from "node:assert";
import { test } from "vitest";
import { reportPlatformFailure, withPlatformDiagnostics } from "./diagnostics.js";

test("diagnostics correlate failures without recording user data or trusting supplied IDs", async () => {
  const logs: string[] = [];
  const platform = withPlatformDiagnostics({ fetch: async () => {
    reportPlatformFailure("run_entry", new Error("secret cookie token body", {
      cause: new DOMException("secret URL", "TimeoutError"),
    }));
    return new Response("unavailable", { status: 503, headers: {
      "cache-control": "private, no-store", "set-cookie": "session=secret; HttpOnly",
    } });
  } }, (line) => logs.push(line));
  const response = await platform.fetch(new Request("https://arena.example/api/runs/enter?secret=query", {
    method: "POST", headers: { cookie: "secret", "x-worldstream-request-id": "forged" }, body: "secret",
  }));
  const requestId = response.headers.get("x-worldstream-request-id");
  assert.match(requestId!, /^[0-9a-f-]{36}$/u);
  assert.equal(response.status, 503);
  assert.equal(response.headers.get("set-cookie"), "session=secret; HttpOnly");
  assert.equal(response.headers.get("cache-control"), "private, no-store");
  assert.ok(logs.length >= 2);
  for (const line of logs) {
    const value = JSON.parse(line);
    assert.equal(value.request_id, requestId);
    assert.equal(value.route, "run_entry");
    assert.doesNotMatch(line, /secret|forged|cookie|token|query/u);
  }
  assert.ok(logs.some((line) => line.includes('"error_kind":"timeout"')));
});

test("concurrent requests keep separate diagnostics and unexpected failures stay private", async () => {
  const logs: string[] = [];
  const platform = withPlatformDiagnostics({ fetch: async () => {
    await Promise.resolve();
    throw new Error("private stack and body");
  } }, (line) => logs.push(line));
  const responses = await Promise.all([1, 2].map(() => platform.fetch(new Request("https://arena.example/api/my-games"))));
  const ids = responses.map((response) => response.headers.get("x-worldstream-request-id"));
  assert.equal(new Set(ids).size, 2);
  for (const response of responses) {
    assert.equal(response.status, 503);
    assert.match(response.headers.get("cache-control")!, /private.*no-store/u);
    assert.deepEqual(await response.json(), { error: { code: "temporarily_unavailable" } });
  }
  assert.deepEqual(new Set(logs.map((line) => JSON.parse(line).request_id)), new Set(ids));
  assert.ok(logs.every((line) => !line.includes("private stack")));
});

test("a broken log sink never changes the response or retries a mutation", async () => {
  let calls = 0;
  const platform = withPlatformDiagnostics({ fetch: async () => {
    calls += 1;
    reportPlatformFailure("formation", new Error("failure"));
    return new Response(null, { status: 503 });
  } }, () => { throw new Error("logging unavailable"); });
  assert.equal((await platform.fetch(new Request("https://arena.example/api/launches", { method: "POST" }))).status, 503);
  assert.equal(calls, 1);
});
