import { strict as assert } from "node:assert";
import { test } from "vitest";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { encodeCanonical } from "@worldstream/pack-sdk";
import { readListingRevision } from "@worldstream/hosted-contract";

import type { PlatformBff } from "./bff.js";
import { createHostedResultReconciler, withHostedResultReconciliation } from "./reconciliation-service.js";
import type { ResultReconcilerDependencies, ResultReconciliationCandidate } from "./result-reconciliation.js";

function platform(): PlatformBff {
  return { fetch: async () => new Response("ok", { status: 200 }) };
}

test("current and retained Listings resolve their exact result projector without network calls", async () => {
  const reconciler = createHostedResultReconciler({
    supabaseUrl: "https://database.example.invalid",
    dataSecretKey: "sb_secret_synthetic-test-secret-not-a-credential",
    hostedGatewayUrl: "https://gateway.example.invalid",
    serviceAuthority: "synthetic-test-authority-".repeat(3),
  });
  for (const version of ["0.2.0", "0.3.0", "0.4.0", "0.5.0", "0.6.0", "0.7.0", "0.8.0", "0.9.0", "0.10.0", "0.11.0", "0.12.0", "0.13.0", "0.14.0", "0.15.0", "0.16.0", "0.17.0", "0.18.0", "0.19.0", "0.20.0", "0.21.0", "0.22.0", "0.23.0", "0.24.0", "0.25.0"]) {
    const source = JSON.parse(await readFile(resolve("../..", `config/hosted/listings/agent-heist-${version}.json`), "utf8"));
    const listing = readListingRevision(encodeCanonical(source));
    const pinned = reconciler.projectors.resolve(listing.digest);
    assert.equal(pinned.listing.digest, listing.digest);
    assert.equal(pinned.listing.value.result.projector.version, ["0.24.0", "0.25.0"].includes(version) ? "0.5.0" : version === "0.12.0" ? "0.4.0" : ["0.7.0", "0.8.0", "0.9.0", "0.10.0", "0.11.0", "0.13.0", "0.14.0", "0.15.0", "0.16.0", "0.17.0", "0.18.0", "0.19.0", "0.20.0", "0.21.0", "0.22.0", "0.23.0"].includes(version) ? "0.3.0" : "0.2.0");
  }
});

function dependencies(list: () => Promise<readonly ResultReconciliationCandidate[]>): ResultReconcilerDependencies {
  return {
    data: {
      listCandidates: list,
      reconcileTerminalActivityCapacity: async () => 0,
      listTerminalHouseRunnerRetirementRuns: async () => [],
      listPrestartHouseRunnerRetirementRuns: async () => [],
      listPrestartAbandonmentLaunches: async () => [],
      markAttempt: async () => {},
      readTerminal: async () => null,
      readResult: async () => null,
      readTerminalHouseRunnerRetirements: async () => [],
      recordTerminalHouseRunnerRetirement: async () => false,
      readPrestartHouseRunnerRetirements: async () => [],
      recordPrestartHouseRunnerRetirement: async () => false,
      recordTerminal: async () => ({ disposition: "blocked", safeCode: "unused" }),
      recordTerminalConflict: async () => ({ disposition: "blocked", safeCode: "unused" }),
      recordResult: async () => ({ disposition: "blocked", safeCode: "unused" }),
      recordIntegrity: async () => ({ disposition: "blocked", safeCode: "unused" }),
    },
    source: { readResultSource: async () => ({}) },
    houseRetirement: { retireHouseRunner: async () => ({}) },
    projectors: {} as ResultReconcilerDependencies["projectors"],
  };
}

test("result reads and the first authenticated My Games read share one bounded reconciliation pass", async () => {
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
  assert.equal(calls, 1);
  assert.equal((await bff.fetch(new Request(
    "https://arena.example/api/my-games",
  ))).status, 200);
  assert.equal(calls, 1);
});

test("rapid authenticated My Games polling reuses a successful bounded maintenance pass", async () => {
  let reconciliationCalls = 0;
  let platformCalls = 0;
  const bff = withHostedResultReconciliation(
    {
      fetch: async (request) => {
        platformCalls += 1;
        return Response.json(
          { account: request.headers.get("x-account") },
          { headers: { "cache-control": "private, no-store, max-age=0" } },
        );
      },
    },
    dependencies(async () => {
      reconciliationCalls += 1;
      return [];
    }),
  );

  const first = await bff.fetch(new Request("https://arena.example/api/my-games", {
    headers: { "x-account": "first-account" },
  }));
  const second = await bff.fetch(new Request("https://arena.example/api/my-games", {
    headers: { "x-account": "second-account" },
  }));

  assert.equal(reconciliationCalls, 1);
  assert.equal(platformCalls, 4);
  assert.equal(first.headers.get("cache-control"), "private, no-store, max-age=0");
  assert.equal(second.headers.get("cache-control"), "private, no-store, max-age=0");
  assert.deepEqual(await first.json(), { account: "first-account" });
  assert.deepEqual(await second.json(), { account: "second-account" });
});

test("concurrent authenticated My Games reads share one maintenance pass", async () => {
  let reconciliationCalls = 0;
  let beginReconciliation: (() => void) | undefined;
  let finishReconciliation: (() => void) | undefined;
  const began = new Promise<void>((resolve) => { beginReconciliation = resolve; });
  const finish = new Promise<void>((resolve) => { finishReconciliation = resolve; });
  const bff = withHostedResultReconciliation(
    {
      fetch: async (request) => Response.json(
        { account: request.headers.get("x-account") },
        { headers: { "cache-control": "private, no-store, max-age=0" } },
      ),
    },
    dependencies(async () => {
      reconciliationCalls += 1;
      beginReconciliation?.();
      await finish;
      return [];
    }),
  );

  const first = bff.fetch(new Request("https://arena.example/api/my-games", {
    headers: { "x-account": "first-account" },
  }));
  await began;
  const second = bff.fetch(new Request("https://arena.example/api/my-games", {
    headers: { "x-account": "second-account" },
  }));
  await Promise.resolve();
  assert.equal(reconciliationCalls, 1);

  finishReconciliation?.();
  const [firstResponse, secondResponse] = await Promise.all([first, second]);
  assert.equal(firstResponse.headers.get("cache-control"), "private, no-store, max-age=0");
  assert.equal(secondResponse.headers.get("cache-control"), "private, no-store, max-age=0");
  assert.deepEqual(await firstResponse.json(), { account: "first-account" });
  assert.deepEqual(await secondResponse.json(), { account: "second-account" });
});

test("anonymous My Games reads do not trigger reconciliation", async () => {
  let calls = 0;
  const bff = withHostedResultReconciliation(
    { fetch: async () => new Response("unauthorized", { status: 401 }) },
    dependencies(async () => { calls += 1; return []; }),
  );
  const response = await bff.fetch(new Request("https://arena.example/api/my-games"));
  assert.equal(response.status, 401);
  assert.equal(calls, 0);
});

test("an authenticated start retries once after terminal-evidence capacity repair", async () => {
  let starts = 0;
  let repairs = 0;
  let resultSourceCandidateReads = 0;
  const deps = dependencies(async () => {
    resultSourceCandidateReads += 1;
    return [];
  });
  deps.data.reconcileTerminalActivityCapacity = async (limit) => {
    repairs += 1;
    assert.equal(limit, 10);
    return 1;
  };
  const bff = withHostedResultReconciliation({
    fetch: async () => {
      starts += 1;
      return starts === 1
        ? Response.json({ error: { code: "activity_capacity_unavailable" } }, { status: 409 })
        : Response.json({ state: "run_created" });
    },
  }, deps);
  const response = await bff.fetch(new Request(
    `https://arena.example/api/launches/${"a".repeat(8)}-${"a".repeat(4)}-4aaa-8aaa-${"a".repeat(12)}/start`,
    { method: "POST", body: "{}" },
  ));
  assert.equal(response.status, 200);
  assert.equal(starts, 2);
  assert.equal(repairs, 1);
  assert.equal(resultSourceCandidateReads, 0);
});

test("only an exact lowercase UUID start capacity response triggers terminal capacity repair", async () => {
  let calls = 0;
  const deps = dependencies(async () => []);
  deps.data.reconcileTerminalActivityCapacity = async () => {
    calls += 1;
    return 0;
  };
  const bff = withHostedResultReconciliation(
    { fetch: async () => Response.json({ error: { code: "activity_capacity_unavailable" } }, { status: 409 }) },
    deps,
  );
  const response = await bff.fetch(new Request("https://arena.example/api/launches", {
    method: "POST", body: "{}",
  }));
  assert.equal(response.status, 409);
  const uppercase = await bff.fetch(new Request(
    "https://arena.example/api/launches/AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA/start",
    { method: "POST", body: "{}" },
  ));
  assert.equal(uppercase.status, 409);
  assert.equal(calls, 0);
});

test("daily recovery requires exact authority and resumes Genesis candidates independently", async () => {
  let lists = 0;
  let repairs = 0;
  const recovered: string[] = [];
  const deps = dependencies(async () => {
    lists += 1;
    return ["first", "second"].map((id) => ({
      candidateKind: "genesis" as const, launchRequestId: id, runId: null,
      listingRevisionDigest: "unused", launchRequestDigest: null,
      hostInstallationId: "fixture", roomSetupOperationId: id,
    }));
  });
  deps.data.reconcileTerminalActivityCapacity = async (limit) => {
    repairs += 1;
    assert.equal(limit, 10);
    return 0;
  };
  const bff = withHostedResultReconciliation(platform(), deps, {
    canonicalOrigin: "https://arena.example", cronSecret: "c".repeat(40),
    recover: async (id) => { recovered.push(id); if (id === "first") throw new Error("private failure details"); },
    abandonPrestart: async () => {},
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
  assert.equal(repairs, 1);
});

test("stale provisioning candidates with no live Host authority remain recoverable", async () => {
  const recovered: string[] = [];
  const deps = dependencies(async () => [{
    // The SQL candidate is selected only for a host-mutated provisioning
    // request with no retained Activity Run. The coordinator may legitimately
    // return null when the Host has no live authority for that operation.
    candidateKind: "genesis" as const,
    launchRequestId: "stale-provisioning-launch",
    runId: null,
    listingRevisionDigest: "unused",
    launchRequestDigest: null,
    hostInstallationId: "hosted-dev",
    roomSetupOperationId: "stale-room-operation",
  }]);
  const bff = withHostedResultReconciliation(platform(), deps, {
    canonicalOrigin: "https://arena.example",
    cronSecret: "s".repeat(40),
    recover: async (launchRequestId) => {
      recovered.push(launchRequestId);
      return null;
    },
    abandonPrestart: async () => {},
  });
  const response = await bff.fetch(new Request("https://arena.example/api/internal/reconcile", {
    headers: { authorization: `Bearer ${"s".repeat(40)}` },
  }));
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { attempted: 1, failed: 0 });
  assert.deepEqual(recovered, ["stale-provisioning-launch"]);
});

test("the cron invokes exact Host abandonment only for the DB-selected pre-start deadline lane", async () => {
  const abandoned: string[] = [];
  const deps = dependencies(async () => []);
  deps.data.listPrestartAbandonmentLaunches = async () => ["deadline-selected-launch"];
  const bff = withHostedResultReconciliation(platform(), deps, {
    canonicalOrigin: "https://arena.example",
    cronSecret: "d".repeat(40),
    recover: async () => { throw new Error("not a Genesis repair"); },
    abandonPrestart: async (launchRequestId) => { abandoned.push(launchRequestId); },
  });
  const response = await bff.fetch(new Request("https://arena.example/api/internal/reconcile", {
    headers: { authorization: `Bearer ${"d".repeat(40)}` },
  }));
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { attempted: 1, failed: 0 });
  assert.deepEqual(abandoned, ["deadline-selected-launch"]);
});

test("public result reads serve durable state when opportunistic reconciliation is unavailable", async () => {
  const delegated: string[] = [];
  const bff = withHostedResultReconciliation(
    {
      fetch: async (request) => {
        delegated.push(new URL(request.url).pathname);
        return Response.json({ version: "durable_fixture.v1" });
      },
    },
    dependencies(async () => {
      throw new Error("fixture outage");
    }),
  );
  const paths = [
    `/api/runs/${"a".repeat(32)}`,
    "/api/results/agent-heist/recent",
  ];
  for (const path of paths) {
    const response = await bff.fetch(new Request(`https://arena.example${path}`));
    assert.equal(response.status, 200);
    assert.deepEqual(await response.json(), { version: "durable_fixture.v1" });
  }
  assert.deepEqual(delegated, paths);
});

test("a My Games reconciliation outage fails closed with private cache policy", async () => {
  const bff = withHostedResultReconciliation(
    platform(),
    dependencies(async () => { throw new Error("fixture outage"); }),
  );
  const response = await bff.fetch(new Request("https://arena.example/api/my-games"));
  assert.equal(response.status, 503);
  assert.equal(response.headers.get("cache-control"), "private, no-store, max-age=0");
  assert.deepEqual(await response.json(), { error: { code: "temporarily_unavailable" } });
});
