import { strict as assert } from "node:assert";
import { test } from "vitest";

import {
  PlatformActivityCapacityUnavailableError,
  PlatformCredentialRejectedError,
  PlatformDependencyUnavailableError,
  PlatformRateLimitExceededError,
} from "./bff.js";
import {
  createSupabaseAuthAdminClient,
  createSupabaseBffDependencies,
  createSupabaseResultReconciliationData,
} from "./supabase.js";

const URL = "https://project.supabase.co";
const PUBLISHABLE = `sb_publishable_${"p".repeat(32)}`;
const SECRET = `sb_secret_${"s".repeat(32)}`;

function legacyKey(role: "anon" | "service_role"): string {
  const header = Buffer.from(JSON.stringify({ alg: "HS256", typ: "JWT" })).toString("base64url");
  const payload = Buffer.from(JSON.stringify({ role })).toString("base64url");
  return `${header}.${payload}.${"x".repeat(32)}`;
}

test("Supabase clients require separate publishable and secret key classes", () => {
  assert.doesNotThrow(() =>
    createSupabaseBffDependencies({
      url: URL,
      publishableKey: PUBLISHABLE,
      dataSecretKey: SECRET,
    }),
  );
  assert.throws(() =>
    createSupabaseBffDependencies({
      url: URL,
      publishableKey: SECRET,
      dataSecretKey: PUBLISHABLE,
    }),
  );
  assert.throws(() =>
    createSupabaseBffDependencies({
      url: URL,
      publishableKey: PUBLISHABLE,
      dataSecretKey: PUBLISHABLE,
    }),
  );
  assert.throws(() => createSupabaseAuthAdminClient(URL, PUBLISHABLE));
  assert.doesNotThrow(() => createSupabaseAuthAdminClient(URL, SECRET));
});

test("legacy Supabase role keys retain the same separation", () => {
  assert.doesNotThrow(() =>
    createSupabaseBffDependencies({
      url: "http://127.0.0.1:55321",
      publishableKey: legacyKey("anon"),
      dataSecretKey: legacyKey("service_role"),
    }),
  );
  assert.throws(() =>
    createSupabaseBffDependencies({
      url: "http://127.0.0.1:55321",
      publishableKey: legacyKey("service_role"),
      dataSecretKey: legacyKey("anon"),
    }),
  );
});

test("closed launch admission is temporary only for the exact launch RPC error", async () => {
  const originalFetch = globalThis.fetch;
  let failure = { code: "55000", message: "hosted_launches_closed" };
  try {
    globalThis.fetch = async () => Response.json(failure, { status: 400 });
    const data = createSupabaseBffDependencies({
      url: URL,
      publishableKey: PUBLISHABLE,
      dataSecretKey: SECRET,
    }).hostedFormationData;
    assert.ok(data);
    const create = () => data.createLaunchRequest({
      accountId: "10000000-0000-4000-8000-000000000001",
      listingRevisionDigest: `blake3:${"1".repeat(64)}`,
      idempotencyNamespace: "maintenance-test",
      idempotencyKeyDigest: new Uint8Array(32),
      canonicalLaunchInput: new TextEncoder().encode("{}"),
      launchInputDigest: new Uint8Array(32),
      houseFillChoice: "disabled",
      creatorAccessChoice: "seat",
      creatorSeatId: "navigator",
    });
    await assert.rejects(create(), PlatformDependencyUnavailableError);
    // An identically named error from another RPC must not gain a new meaning.
    await assert.rejects(
      data.readLaunchRequest("10000000-0000-4000-8000-000000000001", "20000000-0000-4000-8000-000000000001"),
      PlatformCredentialRejectedError,
    );
    for (const other of [
      { code: "55000", message: "hosted_house_fill_closed" },
      { code: "55000", message: "private-unrecognized-error" },
      { code: "23505", message: "hosted_launches_closed" },
      { code: "22023", message: "hosted_launches_closed" },
    ]) {
      failure = other;
      await assert.rejects(create(), PlatformCredentialRejectedError);
    }
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("only exact durable capacity gates become an activity-capacity response", async () => {
  const originalFetch = globalThis.fetch;
  let failure = { code: "55000", message: "pre_genesis_capacity_unavailable" };
  try {
    globalThis.fetch = async () => Response.json(failure, { status: 400 });
    const data = createSupabaseBffDependencies({
      url: URL,
      publishableKey: PUBLISHABLE,
      dataSecretKey: SECRET,
    }).hostedFormationData;
    assert.ok(data);
    const create = () => data.createLaunchRequest({
      accountId: "10000000-0000-4000-8000-000000000001",
      listingRevisionDigest: `blake3:${"1".repeat(64)}`,
      idempotencyNamespace: "capacity-test",
      idempotencyKeyDigest: new Uint8Array(32),
      canonicalLaunchInput: new TextEncoder().encode("{}"),
      launchInputDigest: new Uint8Array(32),
      houseFillChoice: "disabled",
      creatorAccessChoice: "seat",
      creatorSeatId: "navigator",
    });
    await assert.rejects(create(), (error: unknown) =>
      error instanceof PlatformActivityCapacityUnavailableError && error.scope === "account",
    );
    failure = { code: "55000", message: "global_active_run_capacity_unavailable" };
    await assert.rejects(() => data.authorizeHostMutation(
      "10000000-0000-4000-8000-000000000001",
      "20000000-0000-4000-8000-000000000001",
      "hosted-preview-1",
      "launch-20000000000040008000000000000001",
    ), (error: unknown) =>
      error instanceof PlatformActivityCapacityUnavailableError && error.scope === "platform",
    );
    failure = { code: "55000", message: "unrecognized_capacity_like_error" };
    await assert.rejects(create(), PlatformCredentialRejectedError);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("public Run reads use only the server-secret DTO RPCs", async () => {
  const originalFetch = globalThis.fetch;
  const calls: Array<{ readonly url: string; readonly body: unknown }> = [];
  try {
    globalThis.fetch = async (input, init) => {
      const request = input instanceof Request ? input : new Request(input, init);
      calls.push({ url: request.url, body: JSON.parse(await request.clone().text()) });
      if (request.url.endsWith("/read_public_run_v1")) {
        return Response.json({ version: "public_run.v1", state: "unavailable" });
      }
      return Response.json({
        version: "recent_results.v1",
        activity: "agent-heist",
        order: "newest_first",
        maximum: 20,
        results: [],
      });
    };
    const data = createSupabaseBffDependencies({
      url: URL,
      publishableKey: PUBLISHABLE,
      dataSecretKey: SECRET,
    }).publicRunData;
    assert.ok(data);
    assert.equal((await data.readPublicRun("a".repeat(32))).state, "unavailable");
    assert.equal((await data.listRecentResults(20)).results.length, 0);
    assert.match(calls[0]?.url ?? "", /\/rest\/v1\/rpc\/read_public_run_v1$/u);
    assert.deepEqual(calls[0]?.body, { p_public_id: "a".repeat(32) });
    assert.match(calls[1]?.url ?? "", /\/rest\/v1\/rpc\/list_recent_results_v1$/u);
    assert.deepEqual(calls[1]?.body, { p_limit: 20 });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("result reconciliation uses only typed server RPCs and exact bytea inputs", async () => {
  const originalFetch = globalThis.fetch;
  const calls: Array<{ url: string; body: Record<string, unknown> }> = [];
  try {
    globalThis.fetch = async (input, init) => {
      const request = input instanceof Request ? input : new Request(input, init);
      const body = JSON.parse(await request.clone().text()) as Record<string, unknown>;
      calls.push({ url: request.url, body });
      if (request.url.endsWith("/list_reconciliation_candidates_v1")) {
        return Response.json([{
          candidate_kind: "result_source",
          launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
          activity_run_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
          listing_revision_digest: `blake3:${"1".repeat(64)}`,
          launch_request_digest: `blake3:${"2".repeat(64)}`,
          host_installation_id: "hosted-test",
          room_setup_operation_id: "hosted-result-01",
        }]);
      }
      if (request.url.endsWith("/list_terminal_house_runner_retirement_candidates_v1")) {
        return Response.json([{ activity_run_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb" }]);
      }
      if (request.url.endsWith("/list_prestart_house_runner_retirement_candidates_v1")) {
        return Response.json([{ activity_run_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb" }]);
      }
      if (request.url.endsWith("/list_prestart_abandonment_candidates_v1")) {
        return Response.json([{
          activity_run_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
          launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        }]);
      }
      if (request.url.endsWith("/read_terminal_reconciliation_v1")) {
        return Response.json({
          terminal_recorded: true,
          projector_status: "summary",
          reconciliation_state: "terminal",
        });
      }
      if (request.url.endsWith("/read_result_reconciliation_v1")) {
        return Response.json({
          result_recorded: false,
          result_payload_digest: null,
          integrity_status: "healthy",
          integrity_generation: 3,
          publishable: false,
        });
      }
      if (request.url.endsWith("/read_terminal_house_runner_retirements_v1")) {
        return Response.json([{
          host_installation_id: "hosted-test",
          reservation_operation_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
          launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
          house_agent_assignment_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
          terminal_evidence_digest: `sha256:${"3".repeat(64)}`,
        }]);
      }
      if (request.url.endsWith("/record_terminal_house_runner_retirement_v1")) {
        return Response.json(true);
      }
      if (request.url.endsWith("/read_prestart_house_runner_retirements_v1")) {
        return Response.json([{
          host_installation_id: "hosted-test",
          reservation_operation_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
          launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
          house_agent_assignment_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
          abandonment_evidence_digest: `sha256:${"4".repeat(64)}`,
        }]);
      }
      if (request.url.endsWith("/record_prestart_house_runner_retirement_v1")) {
        return Response.json(true);
      }
      return Response.json({ disposition: "applied", safe_code: "result_recorded" });
    };
    const data = createSupabaseResultReconciliationData(URL, SECRET);
    const candidates = await data.listCandidates(10);
    assert.equal(candidates[0]?.candidateKind, "result_source");
    assert.deepEqual(await data.listTerminalHouseRunnerRetirementRuns(10), [
      "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
    ]);
    assert.deepEqual(await data.listPrestartHouseRunnerRetirementRuns(10), [
      "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
    ]);
    assert.deepEqual(await data.listPrestartAbandonmentLaunches(10), [
      "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    ]);
    assert.equal((await data.readTerminal("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"))?.projectorStatus, "summary");
    assert.equal((await data.readResult("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"))?.integrityGeneration, 3);
    const [retirement] = await data.readTerminalHouseRunnerRetirements(
      "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
    );
    assert.equal(retirement?.houseAgentAssignmentId, "dddddddd-dddd-4ddd-8ddd-dddddddddddd");
    assert.equal(await data.recordTerminalHouseRunnerRetirement({
      runId: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
      candidate: retirement!,
      canonicalReceipt: Uint8Array.of(4, 5, 6),
      receiptDigest: Uint8Array.of(7, 8, 9),
    }), true);
    const [prestartRetirement] = await data.readPrestartHouseRunnerRetirements(
      "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
    );
    assert.equal(prestartRetirement?.abandonmentEvidenceDigest, `sha256:${"4".repeat(64)}`);
    assert.equal(await data.recordPrestartHouseRunnerRetirement({
      runId: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
      candidate: prestartRetirement!,
      canonicalReceipt: Uint8Array.of(4, 5, 6),
      receiptDigest: Uint8Array.of(7, 8, 9),
    }), true);
    const bytes = Uint8Array.of(1, 2, 3);
    await data.recordResult(
      "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
      bytes,
      bytes,
      bytes,
      bytes,
    );
    assert.match(calls[0]?.url ?? "", /\/rpc\/list_reconciliation_candidates_v1$/u);
    assert.deepEqual(calls.at(-1)?.body, {
      p_activity_run_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
      p_canonical_result_evidence: "\\x010203",
      p_result_evidence_digest: "\\x010203",
      p_canonical_result_payload: "\\x010203",
      p_result_payload_digest: "\\x010203",
    });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("Supabase Auth distinguishes rejected credentials from dependency failure", async () => {
  const originalFetch = globalThis.fetch;
  const auth = createSupabaseBffDependencies({
    url: URL,
    publishableKey: PUBLISHABLE,
    dataSecretKey: SECRET,
  }).authClient();
  try {
    globalThis.fetch = async () => new Response(null, { status: 401 });
    await assert.rejects(
      auth.exchangeCodeForSession("rejected-code", "valid-verifier"),
      PlatformCredentialRejectedError,
    );

    globalThis.fetch = async () => {
      throw new TypeError("simulated network outage");
    };
    await assert.rejects(
      auth.exchangeCodeForSession("uncertain-code", "valid-verifier"),
      PlatformDependencyUnavailableError,
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("Supabase identity sync preserves the RPC zero-row rate-limit result", async () => {
  const originalFetch = globalThis.fetch;
  try {
    globalThis.fetch = async () =>
      Response.json([], { status: 200, headers: { "content-range": "*/*" } });
    const data = createSupabaseBffDependencies({
      url: URL,
      publishableKey: PUBLISHABLE,
      dataSecretKey: SECRET,
    }).dataClient;
    await assert.rejects(
      data.syncGithubIdentity({
        authUserId: "00000000-0000-4000-8000-000000000010",
        providerSubject: "12345",
        githubLogin: null,
        avatarUrl: null,
      }),
      PlatformRateLimitExceededError,
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("session refresh admission uses the identity-bound non-mutating sync overload", async () => {
  const originalFetch = globalThis.fetch;
  let rpcUrl = "";
  let rpcBody: unknown;
  try {
    globalThis.fetch = async (input, init) => {
      const request = input instanceof Request ? input : new Request(input, init);
      rpcUrl = request.url;
      rpcBody = JSON.parse(await request.clone().text()) as unknown;
      return Response.json([
        {
          account_id: "00000000-0000-4000-8000-000000000001",
          erased_at: null,
          public_profile_enabled: false,
          admitted: true,
        },
      ]);
    };
    const data = createSupabaseBffDependencies({
      url: URL,
      publishableKey: PUBLISHABLE,
      dataSecretKey: SECRET,
    }).dataClient;
    assert.deepEqual(
      await data.beginSessionRefresh({
        authUserId: "00000000-0000-4000-8000-000000000010",
        providerSubject: "12345",
      }),
      {
        accountId: "00000000-0000-4000-8000-000000000001",
        publicProfileEnabled: false,
        admitted: true,
      },
    );
    assert.match(rpcUrl, /\/rest\/v1\/rpc\/sync_github_identity_v1$/u);
    assert.deepEqual(rpcBody, {
      p_auth_user_id: "00000000-0000-4000-8000-000000000010",
      p_provider_subject: "12345",
      p_admit_session_refresh: true,
    });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("owned Run entry resolves through the service-only immutable membership RPC", async () => {
  const originalFetch = globalThis.fetch;
  let rpcUrl = "";
  let rpcBody: unknown;
  try {
    globalThis.fetch = async (input, init) => {
      const request = input instanceof Request ? input : new Request(input, init);
      rpcUrl = request.url;
      rpcBody = JSON.parse(await request.clone().text()) as unknown;
      assert.equal(request.headers.get("apikey"), SECRET);
      return Response.json({
        version: "platform_owned_run_membership.v1",
        run_id: "20000000-0000-4000-8000-000000000001",
        listing_revision_digest: `blake3:${"1".repeat(64)}`,
        host_installation_id: "hosted-preview-1",
        room_setup_operation_id: "hosted-launch-01",
        room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        pack: {
          id: "worldstream.agent-heist",
          version: "0.2.0",
          digest: `blake3:${"2".repeat(64)}`,
        },
        client_release_digest: `sha256:${"3".repeat(64)}`,
        client_surface_id: "participant",
        access_mode: "participant",
        purpose: "participant",
        seat_id: "navigator",
        role: "navigator",
        principal_kind: "human",
        principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
        membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
      });
    };
    const data = createSupabaseBffDependencies({
      url: URL,
      publishableKey: PUBLISHABLE,
      dataSecretKey: SECRET,
    }).dataClient;
    const resolved = await data.resolveOwnedRunMembership({
      accountId: "10000000-0000-4000-8000-000000000001",
      runId: "20000000-0000-4000-8000-000000000001",
      entrySelector: "e".repeat(32),
    });
    assert.equal(resolved?.membershipId, "01ARZ3NDEKTSV4RRFFQ69G5FAX");
    assert.match(rpcUrl, /\/rpc\/resolve_owned_run_membership_v1$/u);
    assert.deepEqual(rpcBody, {
      p_requesting_account_id: "10000000-0000-4000-8000-000000000001",
      p_activity_run_id: "20000000-0000-4000-8000-000000000001",
      p_entry_selector: "e".repeat(32),
    });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("owned Run entry retains an account-controlled agent participant but rejects an agent spectator", async () => {
  const originalFetch = globalThis.fetch;
  let purpose: "participant" | "creator_spectator" = "participant";
  try {
    globalThis.fetch = async () =>
      Response.json({
        version: "platform_owned_run_membership.v1",
        run_id: "20000000-0000-4000-8000-000000000001",
        listing_revision_digest: `blake3:${"1".repeat(64)}`,
        host_installation_id: "hosted-preview-1",
        room_setup_operation_id: "hosted-launch-01",
        room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        pack: {
          id: "worldstream.agent-heist",
          version: "0.2.0",
          digest: `blake3:${"2".repeat(64)}`,
        },
        client_release_digest: `sha256:${"3".repeat(64)}`,
        client_surface_id: "participant",
        access_mode: purpose === "participant" ? "participant" : "spectator",
        purpose,
        seat_id: purpose === "participant" ? "navigator" : null,
        role: purpose === "participant" ? "navigator" : null,
        principal_kind: "agent",
        principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
        membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
      });
    const data = createSupabaseBffDependencies({
      url: URL,
      publishableKey: PUBLISHABLE,
      dataSecretKey: SECRET,
    }).dataClient;
    const input = {
      accountId: "10000000-0000-4000-8000-000000000001",
      runId: "20000000-0000-4000-8000-000000000001",
      entrySelector: "e".repeat(32),
    };
    assert.equal((await data.resolveOwnedRunMembership(input))?.principalKind, "agent");
    purpose = "creator_spectator";
    assert.equal(await data.resolveOwnedRunMembership(input), null);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("Supabase user verification classifies an HTTP credential rejection", async () => {
  const originalFetch = globalThis.fetch;
  try {
    globalThis.fetch = async () =>
      Response.json(
        { code: "bad_jwt", message: "simulated rejected token" },
        { status: 401 },
      );
    const auth = createSupabaseBffDependencies({
      url: URL,
      publishableKey: PUBLISHABLE,
      dataSecretKey: SECRET,
    }).authClient();
    await assert.rejects(auth.getUser("rejected-access-token"), PlatformCredentialRejectedError);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("Supabase refresh preserves a canonical identity rejection from its returned user", async () => {
  const originalFetch = globalThis.fetch;
  try {
    globalThis.fetch = async () =>
      Response.json({
        access_token: "rotated-access-token",
        refresh_token: "rotated-refresh-token",
        expires_in: 3600,
        expires_at: 4_102_444_800,
        token_type: "bearer",
        user: {
          id: "00000000-0000-4000-8000-000000000010",
          aud: "authenticated",
          app_metadata: { provider: "github", providers: ["github"] },
          user_metadata: {},
          created_at: "2026-09-05T00:00:00Z",
          identities: [
            {
              id: "different-provider-subject",
              user_id: "00000000-0000-4000-8000-000000000010",
              identity_id: "00000000-0000-4000-8000-000000000020",
              provider: "github",
              identity_data: { sub: "12345" },
            },
          ],
        },
      });
    const auth = createSupabaseBffDependencies({
      url: URL,
      publishableKey: PUBLISHABLE,
      dataSecretKey: SECRET,
    }).authClient();
    await assert.rejects(
      auth.refreshSession("valid-looking-refresh-token"),
      PlatformCredentialRejectedError,
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});
