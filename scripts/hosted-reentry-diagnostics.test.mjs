import assert from "node:assert/strict";
import { test } from "node:test";
import { execFileSync } from "node:child_process";
import { diagnosticFetch } from "./hosted-reentry-diagnostics.mjs";

const configuration = {
  actor: "creator_bff",
  gatewayOrigin: "http://127.0.0.1:8080",
  supabaseOrigin: "http://127.0.0.1:55321",
};
const target = `${configuration.gatewayOrigin}/v1/hosted/browser-handoffs/issue`;

test("re-entry probe preserves the exact request, unread Response and one-call semantics", async () => {
  const events = [];
  const init = { method: "POST", headers: { authorization: "secret-sentinel" }, body: "private-body", redirect: "error", signal: AbortSignal.timeout(1000) };
  const response = new Response("private-response", { status: 503 });
  let calls = 0;
  const wrapped = diagnosticFetch(async (input, options) => {
    calls += 1;
    assert.equal(input, target);
    assert.equal(options, init);
    return response;
  }, configuration, (event) => events.push(event));
  assert.equal(await wrapped(target, init), response);
  assert.equal(calls, 1);
  assert.equal(response.bodyUsed, false);
  assert.deepEqual(events, [{ actor: "creator_bff", stage: "gateway_issue_handoff", status: 503, category: "service_unavailable" }]);
  assert.equal(await response.text(), "private-response");
  assert.doesNotMatch(JSON.stringify(events), /secret-sentinel|private-body|private-response/u);
});

test("re-entry probe preserves thrown identity, skips unrelated URLs and bounds output", async () => {
  const events = [];
  const failure = new Error("private-error");
  const rejecting = diagnosticFetch(async () => { throw failure; }, configuration, (event) => events.push(event));
  await assert.rejects(rejecting(target, { method: "POST" }), (error) => error === failure);
  assert.equal(events[0].category, "fetch_rejected");
  assert.doesNotMatch(JSON.stringify(events), /private-error/u);
  let response = new Response("unread", { status: 201 });
  const wrapped = diagnosticFetch(async () => response, configuration, (event) => events.push(event));
  const before = events.length;
  await wrapped("http://127.0.0.1:8787/v1/chat/completions", { method: "POST" });
  await wrapped(`${target}?private-query`, { method: "POST" });
  assert.equal(events.length, before);
  for (let index = 0; index < 140; index += 1) await wrapped(target, { method: "POST" });
  assert.equal(events.length - before, 64);
  assert.equal(response.bodyUsed, false);
  response = new Response("unread error", { status: 429 });
  await wrapped(target, { method: "POST" });
  assert.equal(events.at(-1).category, "rate_limited");
  assert.equal(events.length - before, 65);
  assert.equal(response.bodyUsed, false);
  for (let index = 0; index < 100; index += 1) await wrapped(target, { method: "POST" });
  assert.equal(events.length - before, 128);
});

test("NODE_OPTIONS preload reaches creator and inherited acceptance processes only", () => {
  const stub = `data:text/javascript,${encodeURIComponent('globalThis.fetch = async () => new Response("private-unread-body", { status: 503 });')}`;
  const preload = new URL("./hosted-reentry-diagnostics.mjs", import.meta.url).href;
  const program = `const r = await fetch(${JSON.stringify(target)}, {method:"POST"}); if (r.status !== 503 || await r.text() !== "private-unread-body") throw new Error("response_changed");`;
  for (const [argument, acceptance, actor] of [
    ["/fixture/web/platform/dist/dev-server.js", "", "creator_bff"],
    ["/fixture/vitest-worker.js", "visible-local-only", "acceptance_process"],
    ["/fixture/hosted-fake-openrouter.mjs", "", null],
  ]) {
    const output = execFileSync(process.execPath, ["--input-type=module", "--eval", program, argument], {
      encoding: "utf8", timeout: 5000,
      env: {
        ...process.env, CI: "true", NODE_ENV: "test", VERCEL_ENV: "",
        NODE_OPTIONS: `--import=${stub} --import=${preload}`,
        WORLDSTREAM_REENTRY_DIAGNOSTICS: "visible-local-only",
        WORLDSTREAM_LOCAL_ACCEPTANCE: acceptance,
        WORLDSTREAM_HOSTED_GATEWAY_URL: configuration.gatewayOrigin,
        SUPABASE_URL: configuration.supabaseOrigin,
      },
    });
    const events = output.trim().split("\n").filter(Boolean);
    assert.equal(events.length, actor === null ? 0 : 1);
    if (actor !== null) assert.equal(JSON.parse(events[0].slice("[DEBUG-reentry-ad71] ".length)).actor, actor);
    assert.doesNotMatch(output, /private-unread-body|authorization/u);
  }
  assert.throws(() => execFileSync(process.execPath, ["--input-type=module", "--eval", program, "/fixture/web/platform/dist/dev-server.js"], {
    encoding: "utf8", timeout: 5000, stdio: "pipe",
    env: { ...process.env, CI: "true", NODE_ENV: "production", NODE_OPTIONS: `--import=${preload}`, WORLDSTREAM_REENTRY_DIAGNOSTICS: "visible-local-only" },
  }));
});

test("re-entry probe classifies only fixed database and Gateway status categories", async () => {
  const events = [];
  const wrapped = diagnosticFetch(async () => new Response(null, { status: 429 }), configuration, (event) => events.push(event));
  await wrapped(target, { method: "POST" });
  await wrapped(`${configuration.supabaseOrigin}/rest/v1/rpc/resolve_owned_run_membership_v1`, { method: "POST" });
  assert.deepEqual(events.map(({ stage, category }) => [stage, category]), [
    ["gateway_issue_handoff", "rate_limited"], ["database_run_membership", "rate_limited"],
  ]);
  assert.throws(() => diagnosticFetch(fetch, { ...configuration, gatewayOrigin: "https://remote.example" }, () => {}));
  const response = new Response(null, { status: 200 });
  assert.equal(await diagnosticFetch(async () => response, configuration, () => { throw new Error("sink failed"); })(target, { method: "POST" }), response);
});

test("re-entry probe covers hosted session admission, status and ticket requests", async () => {
  const events = [];
  const response = new Response("private-session-response", { status: 503 });
  const wrapped = diagnosticFetch(async () => response, configuration, (event) => events.push(event));
  for (const operation of ["admit", "status", "stream-ticket"]) {
    await wrapped(`${configuration.gatewayOrigin}/v1/hosted/browser-sessions/${operation}`, {
      method: "POST", body: "private-session-token",
    });
  }
  assert.deepEqual(events.map(({ stage }) => stage), [
    "gateway_admit_session", "gateway_session_status", "gateway_stream_ticket",
  ]);
  assert.equal(response.bodyUsed, false);
  assert.doesNotMatch(JSON.stringify(events), /private-session/u);
});
