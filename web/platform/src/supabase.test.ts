import { strict as assert } from "node:assert";
import { test } from "vitest";

import {
  PlatformCredentialRejectedError,
  PlatformDependencyUnavailableError,
  PlatformRateLimitExceededError,
} from "./bff.js";
import {
  createSupabaseAuthAdminClient,
  createSupabaseBffDependencies,
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
