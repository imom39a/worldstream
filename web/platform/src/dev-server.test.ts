import { strict as assert } from "node:assert";
import type { AddressInfo } from "node:net";
import { test } from "vitest";
import { createDevelopmentPlatformServer } from "./dev-server.js";

function environment(internalDigests: string | undefined): NodeJS.ProcessEnv {
  return {
    WORLDSTREAM_PLATFORM_BIND: "127.0.0.1",
    CANONICAL_ORIGIN: "http://127.0.0.1:5180",
    SUPABASE_URL: "https://database.example.invalid",
    SUPABASE_PUBLISHABLE_KEY: "sb_publishable_synthetic-development-test",
    SUPABASE_DATA_SECRET_KEY: "sb_secret_synthetic-development-test",
    WORLDSTREAM_SESSION_KEY_BASE64: Buffer.alloc(32, 7).toString("base64"),
    WORLDSTREAM_OAUTH_KEY_BASE64: Buffer.alloc(32, 9).toString("base64"),
    WORLDSTREAM_DEVELOPMENT_IDENTITY_BYPASS: "visible-local-only",
    WORLDSTREAM_DEPLOYMENT_ENVIRONMENT: "development",
    WORLDSTREAM_DEVELOPMENT_AUTH_USER_ID: "00000000-0000-4000-8000-000000000001",
    WORLDSTREAM_DEVELOPMENT_GITHUB_SUBJECT: "development-fixture",
    WORLDSTREAM_DEVELOPMENT_GITHUB_LOGIN: "development-fixture",
    WORLDSTREAM_HOSTED_GATEWAY_URL: "http://localhost:8080",
    WORLDSTREAM_VERCEL_SERVICE_AUTHORITY: "synthetic-local-service-authority-000000000000",
    WORLDSTREAM_HOSTED_INSTALLATION_ID: "hosted-development-fixture",
    WORLDSTREAM_LOCAL_RECONCILIATION_SECRET: "synthetic-local-reconciliation-000000000000",
    WORLDSTREAM_LOCAL_INTERNAL_LISTING_DIGESTS: internalDigests,
  };
}

test.each([undefined, [].join(",")])("hosted development starts a public-only library with candidate allowlist %j", async (digests) => {
  // No candidate checker configuration is needed when the reviewed manifest
  // has no candidates. Hosted Heist dependencies remain configured normally.
  const { bind, server } = createDevelopmentPlatformServer(environment(digests));
  await new Promise<void>((resolve) => server.listen(0, bind, resolve));
  try {
    const port = (server.address() as AddressInfo).port;
    const response = await fetch(`http://${bind}:${port}/api/catalog`);
    assert.equal(response.status, 200);
    const catalog = await response.json();
    assert.deepEqual(catalog.activities.map((activity: { slug: string }) => activity.slug), ["agent-heist", "negotiate"]);
    assert.equal(catalog.activities[0].availability, "available");
    assert.equal((await fetch(`http://${bind}:${port}/api/catalog/internal`)).status, 401);
  } finally {
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
});

test("nonempty malformed candidate configuration still fails closed", () => {
  assert.throws(() => createDevelopmentPlatformServer(environment(" ")), /invalid_internal_candidate_allowlist/u);
});

test("background reconciliation uses the injected reviewed catalog for Genesis recovery and deadline abandonment", async () => {
  const { readFileSync } = await import("node:fs");
  const { encodeCanonical } = await import("@worldstream/pack-sdk");
  const { readListingRevision } = await import("@worldstream/hosted-contract");
  const { MIDNIGHT_ARCHIVE_LISTING_DIGEST, reviewedActivityByDigest } = await import("./hosted-catalog.js");
  const listing = readListingRevision(encodeCanonical(JSON.parse(readFileSync(
    new URL("../../../fixtures/hosted-contract/valid/roster-options-listing.json", import.meta.url), "utf8"))));
  assert.equal(reviewedActivityByDigest(listing.digest), null, "fixture is absent from the production catalog");
  const base = reviewedActivityByDigest(MIDNIGHT_ARCHIVE_LISTING_DIGEST)!;
  const reviewed = { ...base, listing, slug: "background-roster-fixture" };
  const config = environment(undefined);
  const genesisLaunch = "20000000-0000-4000-8000-000000000001";
  const prestartLaunch = "20000000-0000-4000-8000-000000000002";
  const runId = "30000000-0000-4000-8000-000000000001";
  const operation = (id: string) => `launch-${id.replaceAll("-", "")}`;
  const rpcCalls: string[] = [];
  const hostCalls: string[] = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (input, init) => {
    const url = new URL(input instanceof Request ? input.url : String(input));
    if (url.hostname === "database.example.invalid") {
      const rpc = url.pathname.split("/").at(-1)!;
      const body = JSON.parse(String(init?.body));
      rpcCalls.push(rpc);
      switch (rpc) {
        case "list_reconciliation_candidates_v1": return Response.json([{
          candidate_kind: "genesis", launch_request_id: genesisLaunch, activity_run_id: null,
          listing_revision_digest: listing.digest, launch_request_digest: null,
          host_installation_id: config.WORLDSTREAM_HOSTED_INSTALLATION_ID,
          room_setup_operation_id: operation(genesisLaunch),
        }]);
        case "list_pending_launch_closures_v1": return Response.json([]);
        case "list_prestart_abandonment_candidates_v1":
          return Response.json([{ launch_request_id: prestartLaunch }]);
        case "mark_reconciliation_attempt_v1": return Response.json(null);
        case "read_hosted_recovery_material_v1": return Response.json({
          version: "platform_hosted_launch_material.v1", launch_request_id: body.p_launch_request_id,
          listing_revision_digest: listing.digest,
          state: body.p_launch_request_id === genesisLaunch ? "reconciling" : "run_created",
          expires_at: "2026-09-10T00:00:00Z", launch_inputs: { roster_option: "solo" },
          house_fill_choice: "disabled", creator_access_choice: "seat", creator_seat_id: "lead",
          host_installation_id: config.WORLDSTREAM_HOSTED_INSTALLATION_ID,
          room_setup_operation_id: operation(body.p_launch_request_id), roster_frozen: true,
          host_mutation_started: true, claims: [{ seat_id: "lead", display_name: "Expedition lead",
            participation_kind: "account_human", principal_reference: "seat:lead" }], house_assignments: [],
        });
        case "read_genesis_reconciliation_v1": return Response.json({
          version: "platform_genesis_reconciliation.v1", launch_state: "reconciling",
          activity_run_id: null, reconciliation_state: "pending", needs_genesis_pull: true,
        });
        case "record_genesis_v1":
          assert.equal(body.p_launch_request_id, genesisLaunch);
          return Response.json([{ activity_run_id: runId, reconciliation_state: "ready" }]);
        case "read_public_relay_binding_candidate_v1": return Response.json(null);
        case "record_prestart_abandonment_v1":
          assert.equal(body.p_launch_request_id, prestartLaunch);
          return Response.json(true);
        case "reconcile_terminal_activity_capacity_v1": return Response.json(0);
        case "list_terminal_house_runner_retirement_candidates_v1":
        case "list_prestart_house_runner_retirement_candidates_v1": return Response.json([]);
        default: throw new Error(`unexpected RPC ${rpc}`);
      }
    }
    if (url.hostname === "localhost" && url.port === "8080") {
      const body = JSON.parse(Buffer.from(init?.body as Uint8Array).toString());
      assert.equal(body.listing_revision_digest, listing.digest);
      hostCalls.push(url.pathname);
      if (url.pathname === "/v1/hosted/genesis-evidence") {
        assert.equal(body.room_setup_operation_id, operation(genesisLaunch));
        return Response.json({ schema: "worldstream/hosted-genesis-evidence/v1" });
      }
      if (url.pathname === "/v1/hosted/evidence") return Response.json({
        ...body, schema: "worldstream/hosted-launch-status/v1", stage: "launched", room_setup_complete: true,
      });
      if (url.pathname === "/v1/hosted/abandon-prestart") {
        assert.equal(body.room_setup_operation_id, operation(prestartLaunch));
        return Response.json({ ...body, schema: "worldstream/hosted-prestart-abandonment-evidence/v1",
          host_installation_id: config.WORLDSTREAM_HOSTED_INSTALLATION_ID, launch_request_id: prestartLaunch,
          room_id: "retained-room", lobby_launch_committed: false,
          abandonment_fence_digest: `blake3:${"a".repeat(64)}`, authentication_tag: "b".repeat(64),
        });
      }
      throw new Error(`unexpected Host mutation ${url.pathname}`);
    }
    return originalFetch(input, init);
  };
  const { bind, server } = createDevelopmentPlatformServer(config, {
    internalCandidates: { reviewedActivities: [reviewed], internalCandidateListingDigests: [listing.digest] },
  });
  await new Promise<void>((resolve) => server.listen(0, bind, resolve));
  try {
    const port = (server.address() as AddressInfo).port;
    const endpoint = `http://${bind}:${port}/api/internal/reconcile`;
    assert.equal((await originalFetch(endpoint)).status, 401);
    assert.equal(rpcCalls.length, 0, "unauthenticated maintenance cannot read retained launch material");
    const response = await originalFetch(endpoint, {
      headers: { authorization: `Bearer ${config.WORLDSTREAM_LOCAL_RECONCILIATION_SECRET}` },
    });
    const result = await response.json();
    assert.deepEqual(result, { attempted: 2, failed: 0 });
    assert.equal(response.status, 200);
    assert.ok(hostCalls.includes("/v1/hosted/genesis-evidence"));
    assert.ok(hostCalls.includes("/v1/hosted/abandon-prestart"));
    assert.ok(rpcCalls.includes("record_genesis_v1"));
    assert.ok(rpcCalls.includes("record_prestart_abandonment_v1"));
  } finally {
    globalThis.fetch = originalFetch;
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
});
