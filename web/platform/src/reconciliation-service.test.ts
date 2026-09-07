import { strict as assert } from "node:assert";
import { test } from "vitest";

import type { PlatformBff } from "./bff.js";
import { withHostedResultReconciliation } from "./reconciliation-service.js";
import type { ResultReconcilerDependencies, ResultReconciliationCandidate } from "./result-reconciliation.js";

function platform(): PlatformBff {
  return { fetch: async () => new Response("ok", { status: 200 }) };
}

function dependencies(list: () => Promise<readonly ResultReconciliationCandidate[]>): ResultReconcilerDependencies {
  return {
    data: {
      listCandidates: list,
      markAttempt: async () => {},
      readTerminal: async () => null,
      readResult: async () => null,
      recordTerminal: async () => ({ disposition: "blocked", safeCode: "unused" }),
      recordTerminalConflict: async () => ({ disposition: "blocked", safeCode: "unused" }),
      recordResult: async () => ({ disposition: "blocked", safeCode: "unused" }),
      recordIntegrity: async () => ({ disposition: "blocked", safeCode: "unused" }),
    },
    source: { readResultSource: async () => ({}) },
    projectors: {} as ResultReconcilerDependencies["projectors"],
  };
}

test("only public result reads trigger one bounded reconciliation pass", async () => {
  let calls = 0;
  const bff = withHostedResultReconciliation(
    platform(),
    dependencies(async () => {
      calls += 1;
      return [];
    }),
  );
  assert.equal((await bff.fetch(new Request("https://arena.example/api/catalog"))).status, 200);
  assert.equal(calls, 0);
  assert.equal((await bff.fetch(new Request(
    `https://arena.example/api/runs/${"a".repeat(32)}`,
  ))).status, 200);
  assert.equal(calls, 1);
  assert.equal((await bff.fetch(new Request(
    "https://arena.example/api/results/agent-heist/recent",
  ))).status, 200);
  assert.equal(calls, 2);
});

test("daily recovery requires exact authority and resumes Genesis candidates independently", async () => {
  let lists = 0;
  const recovered: string[] = [];
  const bff = withHostedResultReconciliation(platform(), dependencies(async () => {
    lists += 1;
    return ["first", "second"].map((id) => ({
      candidateKind: "genesis" as const, launchRequestId: id, runId: null,
      listingRevisionDigest: "unused", launchRequestDigest: null,
      hostInstallationId: "fixture", roomSetupOperationId: id,
    }));
  }), {
    canonicalOrigin: "https://arena.example", cronSecret: "c".repeat(40),
    recover: async (id) => { recovered.push(id); if (id === "first") throw new Error("private failure details"); },
  });
  for (const [url, authorization, method] of [
    ["https://arena.example/api/internal/reconcile", "", "GET"],
    ["https://other.example/api/internal/reconcile", `Bearer ${"c".repeat(40)}`, "GET"],
    ["https://arena.example/api/internal/reconcile?limit=1000", `Bearer ${"c".repeat(40)}`, "GET"],
    ["https://arena.example/api/internal/reconcile", `Bearer ${"c".repeat(40)}`, "POST"],
  ] as const) {
    assert.equal((await bff.fetch(new Request(url!, { method, headers: { authorization: authorization! } }))).status, 401);
  }
  assert.equal(lists, 0);
  const response = await bff.fetch(new Request("https://arena.example/api/internal/reconcile", {
    headers: { authorization: `Bearer ${"c".repeat(40)}` },
  }));
  assert.equal(response.status, 503);
  assert.deepEqual(await response.json(), { attempted: 2, failed: 1 });
  assert.deepEqual(recovered, ["first", "second"]);
});

test("a reconciliation outage fails the public result read closed", async () => {
  const bff = withHostedResultReconciliation(
    platform(),
    dependencies(async () => {
      throw new Error("fixture outage");
    }),
  );
  const response = await bff.fetch(new Request(
    `https://arena.example/api/runs/${"a".repeat(32)}`,
  ));
  assert.equal(response.status, 503);
  assert.deepEqual(await response.json(), { error: { code: "temporarily_unavailable" } });
});
