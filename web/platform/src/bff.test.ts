import { strict as assert } from "node:assert";
import { test } from "vitest";

import {
  createDevelopmentPlatformBff,
  createPlatformBff,
  createVercelPlatformBff,
  type AuthSession,
  type AuthUser,
  type BffDependencies,
  type OAuthAttempt,
  type PlatformAccount,
  type PlatformAuthClient,
  type PlatformDataClient,
  type RefreshedAuthSession,
  PlatformCredentialRejectedError,
  PlatformDependencyUnavailableError,
} from "./bff.js";
import {
  HostedBrowserSessionMissingError,
  HostedBrowserSessionRejectedError,
  type HostedBrowserHandoff,
  type HostedBrowserSessionClient,
  type HostedBrowserSessionStatus,
  type OwnedRunMembershipCorrespondence,
} from "./browser-sessions.js";

const ORIGIN = "https://arena.example";

class FakeData implements PlatformDataClient {
  readonly attempts = new Map<string, OAuthAttempt & { consumed: boolean }>();
  readonly refreshAdmissions: Array<{
    readonly authUserId: string;
    readonly providerSubject: string;
  }> = [];
  readonly syncs: string[] = [];
  profileEnabled = false;
  erased = false;
  oauthStarts = 0;
  accountMutations = new Map<string, number>();
  oauthStartLimit = 120;
  accountMutationLimit = 32;
  membership: OwnedRunMembershipCorrespondence | null = null;
  readonly membershipResolutions: Array<{
    readonly accountId: string;
    readonly runId: string;
    readonly entrySelector: string;
  }> = [];

  async beginGithubOAuth(attempt: OAuthAttempt): Promise<boolean> {
    if (this.oauthStarts >= this.oauthStartLimit) return false;
    this.oauthStarts += 1;
    this.attempts.set(attempt.stateDigest, { ...attempt, consumed: false });
    return true;
  }

  async consumeGithubOAuth(input: {
    stateDigest: string;
    browserNonceDigest: string;
    returnTarget: string;
  }): Promise<OAuthAttempt | null> {
    const attempt = this.attempts.get(input.stateDigest);
    if (
      attempt === undefined ||
      attempt.consumed ||
      attempt.browserNonceDigest !== input.browserNonceDigest ||
      attempt.returnTarget !== input.returnTarget
    ) {
      return null;
    }
    attempt.consumed = true;
    return attempt;
  }

  async resolveGithubAccount(input: {
    authUserId: string;
    providerSubject: string;
  }): Promise<PlatformAccount | null> {
    if (
      this.erased ||
      input.authUserId !== USER.id ||
      input.providerSubject !== USER.identities[0]?.subject
    ) {
      return null;
    }
    return {
      accountId: "00000000-0000-4000-8000-000000000001",
      publicProfileEnabled: this.profileEnabled,
    };
  }

  async beginSessionRefresh(input: {
    authUserId: string;
    providerSubject: string;
  }): Promise<(PlatformAccount & { admitted: boolean }) | null> {
    this.refreshAdmissions.push(input);
    const account = await this.resolveGithubAccount(input);
    if (account === null) return null;
    const admitted = this.admitAccountMutation(input.authUserId);
    return { ...account, admitted };
  }

  async syncGithubIdentity(input: {
    authUserId: string;
    providerSubject: string;
    githubLogin: string | null;
    avatarUrl: string | null;
  }): Promise<PlatformAccount> {
    if (this.erased) throw new Error("account_erasure_pending");
    this.syncs.push(input.authUserId);
    return {
      accountId: "00000000-0000-4000-8000-000000000001",
      publicProfileEnabled: this.profileEnabled,
    };
  }

  async syncGithubIdentityForMutation(input: {
    authUserId: string;
    providerSubject: string;
    githubLogin: string | null;
    avatarUrl: string | null;
  }): Promise<PlatformAccount | null> {
    if (!this.admitAccountMutation(input.authUserId)) return null;
    return this.syncGithubIdentity(input);
  }

  private admitAccountMutation(authUserId: string): boolean {
    const accepted = this.accountMutations.get(authUserId) ?? 0;
    if (accepted >= this.accountMutationLimit) return false;
    this.accountMutations.set(authUserId, accepted + 1);
    return true;
  }

  async setPublicProfile(_authUserId: string, enabled: boolean): Promise<boolean> {
    if (this.erased) return false;
    this.profileEnabled = enabled;
    return true;
  }

  async beginAccountErasure(_authUserId: string): Promise<boolean> {
    this.erased = true;
    return true;
  }

  async resolveOwnedRunMembership(input: {
    accountId: string;
    runId: string;
    entrySelector: string;
  }): Promise<OwnedRunMembershipCorrespondence | null> {
    this.membershipResolutions.push(input);
    return this.membership;
  }
}

class FakeHostedBrowserSessions implements HostedBrowserSessionClient {
  readonly issued: Array<{ accountId: string; binding: OwnedRunMembershipCorrespondence }> = [];
  readonly redeemed: Array<{
    accountId: string;
    handoff: string;
    priorSession: string | null;
  }> = [];
  readonly statusReads: string[] = [];
  readonly logouts: string[] = [];
  redeemError: Error | null = null;
  nextSession = `wss1:${"b".repeat(64)}`;

  async issueHandoff(
    accountId: string,
    binding: OwnedRunMembershipCorrespondence,
  ): Promise<HostedBrowserHandoff> {
    this.issued.push({ accountId, binding });
    return {
      clientUrl: `https://arena.example/clients/heist/#handoff=wsh1:${"a".repeat(64)}`,
    };
  }

  async redeemHandoff(
    accountId: string,
    handoff: string,
    priorSession: string | null,
  ): Promise<string> {
    this.redeemed.push({ accountId, handoff, priorSession });
    if (this.redeemError !== null) throw this.redeemError;
    return this.nextSession;
  }

  async sessionStatus(session: string): Promise<HostedBrowserSessionStatus> {
    this.statusReads.push(session);
    return { state: "usable" };
  }

  async logoutSession(session: string): Promise<void> {
    this.logouts.push(session);
  }
}

class FakeAuth implements PlatformAuthClient {
  exchangeCalls = 0;
  claimsCalls = 0;
  userCalls = 0;
  refreshCalls = 0;
  signOutCalls = 0;
  exchangeExpiresAt = 4_102_444_800;
  readonly usedRefreshTokens = new Set<string>();

  githubAuthorizeUrl(input: {
    callbackUrl: string;
    codeChallenge: string;
  }): string {
    const url = new URL("https://supabase.example/auth/v1/authorize");
    url.searchParams.set("redirect_to", input.callbackUrl);
    url.searchParams.set("code_challenge", input.codeChallenge);
    return url.toString();
  }

  async exchangeCodeForSession(code: string, verifier: string): Promise<AuthSession> {
    assert.equal(code, "provider-code");
    assert.ok(verifier.length >= 43);
    this.exchangeCalls += 1;
    return session(
      `access-${this.exchangeCalls}`,
      `refresh-${this.exchangeCalls}`,
      this.exchangeExpiresAt,
    );
  }

  async getClaims(accessToken: string): Promise<{ subject: string }> {
    assert.match(accessToken, /^access-/);
    this.claimsCalls += 1;
    return { subject: USER.id };
  }

  async getUser(accessToken: string): Promise<AuthUser> {
    assert.match(accessToken, /^access-/);
    this.userCalls += 1;
    return USER;
  }

  async refreshSession(refreshToken: string): Promise<RefreshedAuthSession> {
    assert.match(refreshToken, /^refresh-/);
    if (this.usedRefreshTokens.has(refreshToken)) throw new Error("refresh_replayed");
    this.usedRefreshTokens.add(refreshToken);
    this.refreshCalls += 1;
    return {
      ...session(`access-r${this.refreshCalls}`, `refresh-r${this.refreshCalls}`),
      user: USER,
    };
  }

  async signOut(accessToken: string): Promise<void> {
    assert.match(accessToken, /^access-/);
    this.signOutCalls += 1;
  }
}

const USER: AuthUser = {
  id: "00000000-0000-4000-8000-000000000010",
  identities: [
    {
      provider: "github",
      subject: "12345",
      login: "octocat",
      avatarUrl: "https://avatars.githubusercontent.com/u/12345",
    },
  ],
};

function session(
  accessToken: string,
  refreshToken: string,
  expiresAt = 4_102_444_800,
): AuthSession {
  return { accessToken, refreshToken, expiresAt };
}

function harness(hostedBrowserSessions?: HostedBrowserSessionClient) {
  const auth = new FakeAuth();
  const data = new FakeData();
  const dependencies: BffDependencies = {
    authClient: () => auth,
    dataClient: data,
    ...(hostedBrowserSessions === undefined ? {} : { hostedBrowserSessions }),
  };
  const bff = createPlatformBff(
    {
      canonicalOrigin: ORIGIN,
      allowedReturnTargets: ["/", "/activities/heist"],
      sessionKey: Buffer.alloc(32, 7),
      oauthKey: Buffer.alloc(32, 9),
    },
    dependencies,
  );
  return { auth, bff, data };
}

function membership(): OwnedRunMembershipCorrespondence {
  return {
    runId: "20000000-0000-4000-8000-000000000001",
    listingRevisionDigest: `blake3:${"1".repeat(64)}`,
    hostInstallationId: "hosted-preview-1",
    roomSetupOperationId: "hosted-launch-01",
    roomId: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
    pack: {
      id: "worldstream.agent-heist",
      version: "0.2.0",
      digest: `blake3:${"2".repeat(64)}`,
    },
    clientReleaseDigest: `blake3:${"3".repeat(64)}`,
    clientSurfaceId: "participant",
    accessMode: "participant",
    purpose: "participant",
    seatId: "navigator",
    role: "navigator",
    principalKind: "human",
    principalId: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
    membershipId: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
  };
}

function cookieValue(response: Response, name: string): string | undefined {
  const cookies = response.headers.getSetCookie();
  const prefix = `${name}=`;
  return cookies
    .map((cookie) => cookie.split(";", 1)[0] ?? "")
    .find((cookie) => cookie.startsWith(prefix))
    ?.slice(prefix.length);
}

async function begin(bff: { fetch(request: Request): Promise<Response> }) {
  const response = await bff.fetch(
    new Request(`${ORIGIN}/api/auth/github/start?return_to=%2Factivities%2Fheist`),
  );
  assert.equal(response.status, 302);
  const location = new URL(response.headers.get("location") ?? "");
  const callback = new URL(location.searchParams.get("redirect_to") ?? "");
  const state = callback.searchParams.get("state");
  const nonce = cookieValue(response, "__Host-worldstream-oauth");
  assert.ok(state);
  assert.ok(nonce);
  return { nonce, response, state };
}

async function signIn(bff: { fetch(request: Request): Promise<Response> }) {
  const started = await begin(bff);
  const response = await bff.fetch(
    new Request(
      `${ORIGIN}/api/auth/github/callback?code=provider-code&state=${encodeURIComponent(started.state)}&return_to=%2Factivities%2Fheist`,
      { headers: { cookie: `__Host-worldstream-oauth=${started.nonce}` } },
    ),
  );
  assert.equal(response.status, 302);
  const sessionCookie = cookieValue(response, "__Host-worldstream-session");
  assert.ok(sessionCookie);
  return { response, sessionCookie };
}

async function csrf(
  bff: { fetch(request: Request): Promise<Response> },
  sessionCookie: string,
): Promise<string> {
  const response = await bff.fetch(
    new Request(`${ORIGIN}/api/auth/session`, {
      headers: { cookie: `__Host-worldstream-session=${sessionCookie}` },
    }),
  );
  assert.equal(response.status, 200);
  const body = (await response.json()) as { csrf: string };
  return body.csrf;
}

function mutation(path: string, sessionCookie: string, csrfValue: string, body = "{}") {
  return new Request(`${ORIGIN}${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      cookie: `__Host-worldstream-session=${sessionCookie}`,
      origin: ORIGIN,
      "sec-fetch-site": "same-origin",
      "x-worldstream-csrf": csrfValue,
    },
    body,
  });
}

test("OAuth start retains protected PKCE state and emits the frozen cookie", async () => {
  const { bff, data } = harness();
  const { response } = await begin(bff);
  assert.equal(data.attempts.size, 1);
  const attempt = [...data.attempts.values()][0];
  assert.ok(attempt);
  assert.match(attempt.stateDigest, /^\\x[0-9a-f]{64}$/);
  assert.match(attempt.browserNonceDigest, /^\\x[0-9a-f]{64}$/);
  assert.notEqual(attempt.encryptedPkceVerifier, attempt.stateDigest);
  const cookie = response.headers.getSetCookie().join("\n");
  assert.match(cookie, /__Host-worldstream-oauth=/);
  assert.match(cookie, /Secure/);
  assert.match(cookie, /HttpOnly/);
  assert.match(cookie, /SameSite=Lax/);
  assert.match(cookie, /Path=\//);
  assert.match(cookie, /Max-Age=600/);
  assert.doesNotMatch(cookie, /Domain=/);
  const authorize = new URL(response.headers.get("location") ?? "");
  assert.equal(authorize.searchParams.has("state"), false);
  assert.match(authorize.searchParams.get("redirect_to") ?? "", /[?&]state=/u);
});

test("OAuth start accepts only immutable configured return targets", async () => {
  const { bff, data } = harness();
  const unlisted = await bff.fetch(
    new Request(`${ORIGIN}/api/auth/github/start?return_to=%2Fsafe-but-unlisted`),
  );
  assert.equal(unlisted.status, 400);
  assert.equal(data.attempts.size, 0);
  assert.throws(() =>
    createPlatformBff(
      {
        canonicalOrigin: ORIGIN,
        allowedReturnTargets: ["/", "/"],
        sessionKey: Buffer.alloc(32, 7),
        oauthKey: Buffer.alloc(32, 9),
      },
      { authClient: () => new FakeAuth(), dataClient: new FakeData() },
    ),
  );
});

test("OAuth callback fails closed for missing cookie, changed target, and replay", async () => {
  const { auth, bff } = harness();
  const started = await begin(bff);
  const missing = await bff.fetch(
    new Request(`${ORIGIN}/api/auth/github/callback?code=provider-code&state=${started.state}&return_to=%2Factivities%2Fheist`),
  );
  assert.equal(missing.status, 400);
  assert.equal(missing.headers.get("cache-control"), "private, no-store, max-age=0");
  assert.match(missing.headers.getSetCookie().join("\n"), /__Host-worldstream-oauth=;.*Max-Age=0/);

  const changed = await bff.fetch(
    new Request(`${ORIGIN}/api/auth/github/callback?code=provider-code&state=${started.state}&return_to=%2Fchanged`, {
      headers: { cookie: `__Host-worldstream-oauth=${started.nonce}` },
    }),
  );
  assert.equal(changed.status, 400);
  assert.equal(auth.exchangeCalls, 0);

  const successful = await signIn(bff);
  assert.equal(successful.response.headers.get("location"), `${ORIGIN}/activities/heist`);
  const oauthClear = successful.response.headers
    .getSetCookie()
    .find((value) => value.startsWith("__Host-worldstream-oauth="));
  assert.match(oauthClear ?? "", /Max-Age=0/);

  const replayStart = await begin(bff);
  const callbackUrl = `${ORIGIN}/api/auth/github/callback?code=provider-code&state=${replayStart.state}&return_to=%2Factivities%2Fheist`;
  const first = await bff.fetch(
    new Request(callbackUrl, {
      headers: { cookie: `__Host-worldstream-oauth=${replayStart.nonce}` },
    }),
  );
  const second = await bff.fetch(
    new Request(callbackUrl, {
      headers: { cookie: `__Host-worldstream-oauth=${replayStart.nonce}` },
    }),
  );
  assert.equal(first.status, 302);
  assert.equal(second.status, 400);
});

test("OAuth callback permits a real cross-site top-level provider redirect", async () => {
  const { bff } = harness();
  const started = await begin(bff);
  const response = await bff.fetch(
    new Request(
      `${ORIGIN}/api/auth/github/callback?code=provider-code&state=${started.state}&return_to=%2Factivities%2Fheist`,
      {
        headers: {
          cookie: `__Host-worldstream-oauth=${started.nonce}`,
          "sec-fetch-dest": "document",
          "sec-fetch-mode": "navigate",
          "sec-fetch-site": "cross-site",
          "sec-fetch-user": "?1",
        },
      },
    ),
  );
  assert.equal(response.status, 302);
});

test("the explicit Vercel adapter accepts only platform-consistent forwarding headers", async () => {
  const auth = new FakeAuth();
  const data = new FakeData();
  const bff = createVercelPlatformBff(
    {
      canonicalOrigin: ORIGIN,
      allowedReturnTargets: ["/", "/activities/heist"],
      sessionKey: Buffer.alloc(32, 7),
      oauthKey: Buffer.alloc(32, 9),
    },
    { authClient: () => auth, dataClient: data },
  );
  const headers = {
    host: "arena.example",
    "x-forwarded-host": "arena.example",
    "x-forwarded-proto": "https",
    "x-forwarded-for": "203.0.113.7",
    "x-vercel-forwarded-for": "203.0.113.7",
    "x-real-ip": "203.0.113.7",
    "x-vercel-id": "iad1::iad1::request-id",
  };
  const accepted = await bff.fetch(
    new Request(`${ORIGIN}/api/auth/github/start?return_to=%2Factivities%2Fheist`, { headers }),
  );
  assert.equal(accepted.status, 302);

  const rejected = await bff.fetch(
    new Request(`${ORIGIN}/api/auth/github/start?return_to=%2Factivities%2Fheist`, {
      headers: { ...headers, "x-forwarded-host": "attacker.example" },
    }),
  );
  assert.equal(rejected.status, 403);
  for (const untrustedHeader of ["x-forwarded-uri", "x-original-path"]) {
    const smuggled = await bff.fetch(
      new Request(`${ORIGIN}/api/auth/github/start?return_to=%2Factivities%2Fheist`, {
        headers: { ...headers, [untrustedHeader]: "/api/account/erasure" },
      }),
    );
    assert.equal(smuggled.status, 403, untrustedHeader);
  }
  assert.equal(data.attempts.size, 1);
});

test("OAuth callback rejects a cross-site subresource request", async () => {
  const { auth, bff } = harness();
  const started = await begin(bff);
  const response = await bff.fetch(
    new Request(
      `${ORIGIN}/api/auth/github/callback?code=provider-code&state=${started.state}&return_to=%2Factivities%2Fheist`,
      {
        headers: {
          cookie: `__Host-worldstream-oauth=${started.nonce}`,
          "sec-fetch-dest": "image",
          "sec-fetch-mode": "no-cors",
          "sec-fetch-site": "cross-site",
        },
      },
    ),
  );
  assert.equal(response.status, 400);
  assert.equal(auth.exchangeCalls, 0);
});

test("OAuth callback fails closed for tampered PKCE state and a callback race", async () => {
  const { auth, bff, data } = harness();
  const tampered = await begin(bff);
  const tamperedAttempt = [...data.attempts.values()][0];
  assert.ok(tamperedAttempt);
  Object.assign(tamperedAttempt, { encryptedPkceVerifier: "\\x" + "00".repeat(64) });
  const invalidPkce = await bff.fetch(
    new Request(
      `${ORIGIN}/api/auth/github/callback?code=provider-code&state=${tampered.state}&return_to=%2Factivities%2Fheist`,
      { headers: { cookie: `__Host-worldstream-oauth=${tampered.nonce}` } },
    ),
  );
  assert.equal(invalidPkce.status, 400);
  assert.equal(auth.exchangeCalls, 0);

  const raced = await begin(bff);
  const callbackUrl = `${ORIGIN}/api/auth/github/callback?code=provider-code&state=${raced.state}&return_to=%2Factivities%2Fheist`;
  const [left, right] = await Promise.all([
    bff.fetch(new Request(callbackUrl, {
      headers: { cookie: `__Host-worldstream-oauth=${raced.nonce}` },
    })),
    bff.fetch(new Request(callbackUrl, {
      headers: { cookie: `__Host-worldstream-oauth=${raced.nonce}` },
    })),
  ]);
  assert.deepEqual([left.status, right.status].sort(), [302, 400]);
  assert.equal(auth.exchangeCalls, 1);
});

test("protected reads use claims while mutations use a fresh verified user", async () => {
  const { auth, bff, data } = harness();
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  assert.equal(auth.claimsCalls, 1);
  assert.deepEqual(data.syncs, [USER.id]);

  const profile = await bff.fetch(
    mutation(
      "/api/account/public-profile",
      signedIn.sessionCookie,
      csrfValue,
      JSON.stringify({ enabled: true, account_id: "browser-forgery" }),
    ),
  );
  assert.equal(profile.status, 200);
  assert.equal(auth.userCalls, 2); // callback and mutation
  assert.equal(data.profileEnabled, true);
});

test("refresh rotates the session and sign-out and erasure clear it", async () => {
  const { auth, bff, data } = harness();
  const signedIn = await signIn(bff);
  const sessionHeader = signedIn.response.headers.getSetCookie().join("\n");
  assert.match(sessionHeader, /__Host-worldstream-session=/);
  assert.match(sessionHeader, /Secure/);
  assert.match(sessionHeader, /HttpOnly/);
  assert.match(sessionHeader, /SameSite=Lax/);
  assert.match(sessionHeader, /Path=\//);
  assert.match(sessionHeader, /Max-Age=604800/);
  assert.doesNotMatch(sessionHeader, /Domain=/);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  const refreshed = await bff.fetch(
    mutation("/api/auth/session/refresh", signedIn.sessionCookie, csrfValue),
  );
  assert.equal(refreshed.status, 200);
  const rotated = cookieValue(refreshed, "__Host-worldstream-session");
  assert.ok(rotated);
  assert.notEqual(rotated, signedIn.sessionCookie);
  assert.equal(refreshed.headers.get("cache-control"), "private, no-store, max-age=0");

  const rotatedCsrf = await csrf(bff, rotated);
  const signedOut = await bff.fetch(
    mutation("/api/auth/sign-out", rotated, rotatedCsrf),
  );
  assert.equal(signedOut.status, 200);
  assert.match(signedOut.headers.getSetCookie().join("\n"), /__Host-worldstream-session=;.*Max-Age=0/);
  assert.equal(auth.signOutCalls, 1);

  const another = await signIn(bff);
  const erasureCsrf = await csrf(bff, another.sessionCookie);
  const erased = await bff.fetch(
    mutation("/api/account/erasure", another.sessionCookie, erasureCsrf),
  );
  assert.equal(erased.status, 202);
  assert.equal(data.erased, true);
  assert.match(erased.headers.getSetCookie().join("\n"), /__Host-worldstream-session=;.*Max-Age=0/);
});

test("an expired access token preserves a reload path through explicit refresh", async () => {
  const originalNow = Date.now;
  const now = 2_000_000_000;
  Date.now = () => now * 1_000;
  try {
    const { auth, bff } = harness();
    auth.exchangeExpiresAt = now + 60;
    const signedIn = await signIn(bff);
    Date.now = () => (now + 61) * 1_000;
    const expired = await bff.fetch(
      new Request(`${ORIGIN}/api/auth/session`, {
        headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` },
      }),
    );
    assert.equal(expired.status, 401);
    assert.equal(expired.headers.getSetCookie().length, 0);
    const body = (await expired.json()) as { csrf: string; error: { code: string } };
    assert.equal(body.error.code, "session_refresh_required");
    assert.ok(body.csrf.length >= 43);

    const refreshed = await bff.fetch(
      mutation("/api/auth/session/refresh", signedIn.sessionCookie, body.csrf),
    );
    assert.equal(refreshed.status, 200);
    assert.ok(cookieValue(refreshed, "__Host-worldstream-session"));
    assert.equal(auth.refreshCalls, 1);
  } finally {
    Date.now = originalNow;
  }
});

test("refresh admission is durable and occurs before token rotation", async () => {
  const { auth, bff, data } = harness();
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  data.accountMutationLimit = 0;
  const limited = await bff.fetch(
    mutation(
      "/api/auth/session/refresh",
      signedIn.sessionCookie,
      csrfValue,
      '{"providerSubject":"browser-forgery","githubLogin":"attacker"}',
    ),
  );
  assert.equal(limited.status, 429);
  assert.equal(auth.refreshCalls, 0);
  assert.equal(limited.headers.getSetCookie().length, 0);
  assert.deepEqual(data.refreshAdmissions, [
    {
      authUserId: USER.id,
      providerSubject: "12345",
    },
  ]);
  assert.equal(await csrf(bff, signedIn.sessionCookie), csrfValue);
});

test("refresh installs rotated tokens without a post-exchange dependency lookup", async () => {
  const { auth, bff } = harness();
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  auth.getUser = async () => {
    throw new PlatformDependencyUnavailableError();
  };
  const response = await bff.fetch(
    mutation("/api/auth/session/refresh", signedIn.sessionCookie, csrfValue),
  );
  assert.equal(response.status, 200);
  assert.ok(cookieValue(response, "__Host-worldstream-session"));
  assert.equal(auth.refreshCalls, 1);
});

test("dependency outages return one temporary state without discarding a valid session", async () => {
  {
    const { auth, bff } = harness();
    const signedIn = await signIn(bff);
    auth.getClaims = async () => {
      throw new PlatformDependencyUnavailableError();
    };
    const response = await bff.fetch(
      new Request(`${ORIGIN}/api/auth/session`, {
        headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` },
      }),
    );
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { error: { code: "temporarily_unavailable" } });
    assert.equal(response.headers.getSetCookie().length, 0);
  }

  {
    const { bff, data } = harness();
    const signedIn = await signIn(bff);
    data.resolveGithubAccount = async () => {
      throw new PlatformDependencyUnavailableError();
    };
    const response = await bff.fetch(
      new Request(`${ORIGIN}/api/auth/session`, {
        headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` },
      }),
    );
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { error: { code: "temporarily_unavailable" } });
    assert.equal(response.headers.getSetCookie().length, 0);
  }

  {
    const { auth, bff } = harness();
    const signedIn = await signIn(bff);
    const csrfValue = await csrf(bff, signedIn.sessionCookie);
    auth.getUser = async () => {
      throw new PlatformDependencyUnavailableError();
    };
    const response = await bff.fetch(
      mutation(
        "/api/account/public-profile",
        signedIn.sessionCookie,
        csrfValue,
        '{"enabled":true}',
      ),
    );
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { error: { code: "temporarily_unavailable" } });
    assert.equal(response.headers.getSetCookie().length, 0);
  }

  {
    const { auth, bff } = harness();
    const signedIn = await signIn(bff);
    const csrfValue = await csrf(bff, signedIn.sessionCookie);
    auth.signOut = async () => {
      throw new PlatformDependencyUnavailableError();
    };
    const response = await bff.fetch(
      mutation("/api/auth/sign-out", signedIn.sessionCookie, csrfValue),
    );
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { error: { code: "temporarily_unavailable" } });
    assert.equal(response.headers.getSetCookie().length, 0);
  }

  {
    const { auth, bff } = harness();
    const signedIn = await signIn(bff);
    const csrfValue = await csrf(bff, signedIn.sessionCookie);
    auth.refreshSession = async () => {
      throw new PlatformDependencyUnavailableError();
    };
    const response = await bff.fetch(
      mutation("/api/auth/session/refresh", signedIn.sessionCookie, csrfValue),
    );
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { error: { code: "temporarily_unavailable" } });
    assert.equal(response.headers.getSetCookie().length, 0);
  }

  {
    const { auth, bff } = harness();
    const started = await begin(bff);
    auth.exchangeCodeForSession = async () => {
      throw new PlatformDependencyUnavailableError();
    };
    const response = await bff.fetch(
      new Request(
        `${ORIGIN}/api/auth/github/callback?code=provider-code&state=${started.state}&return_to=%2Factivities%2Fheist`,
        { headers: { cookie: `__Host-worldstream-oauth=${started.nonce}` } },
      ),
    );
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { error: { code: "temporarily_unavailable" } });
    assert.doesNotMatch(response.headers.getSetCookie().join("\n"), /worldstream-session/);
  }
});

test("credential rejection remains distinct from dependency unavailability", async () => {
  {
    const { auth, bff } = harness();
    const signedIn = await signIn(bff);
    auth.getClaims = async () => {
      throw new PlatformCredentialRejectedError();
    };
    const response = await bff.fetch(
      new Request(`${ORIGIN}/api/auth/session`, {
        headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` },
      }),
    );
    assert.equal(response.status, 401);
    assert.equal(
      ((await response.json()) as { error: { code: string } }).error.code,
      "session_refresh_required",
    );
    assert.equal(response.headers.getSetCookie().length, 0);
  }

  {
    const { auth, bff } = harness();
    const signedIn = await signIn(bff);
    const csrfValue = await csrf(bff, signedIn.sessionCookie);
    auth.getUser = async () => {
      throw new PlatformCredentialRejectedError();
    };
    const response = await bff.fetch(
      mutation(
        "/api/account/public-profile",
        signedIn.sessionCookie,
        csrfValue,
        '{"enabled":true}',
      ),
    );
    assert.equal(response.status, 401);
    assert.match(response.headers.getSetCookie().join("\n"), /worldstream-session=;.*Max-Age=0/);
  }

  {
    const { auth, bff } = harness();
    const signedIn = await signIn(bff);
    const csrfValue = await csrf(bff, signedIn.sessionCookie);
    auth.refreshSession = async () => {
      throw new PlatformCredentialRejectedError();
    };
    const response = await bff.fetch(
      mutation("/api/auth/session/refresh", signedIn.sessionCookie, csrfValue),
    );
    assert.equal(response.status, 401);
    assert.equal(
      ((await response.json()) as { error: { code: string } }).error.code,
      "session_refresh_failed",
    );
    assert.match(response.headers.getSetCookie().join("\n"), /worldstream-session=;.*Max-Age=0/);
  }
});

test("mutation checks reject cross-origin, form, and invalid CSRF before state change", async () => {
  const { auth, bff, data } = harness();
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  for (const request of [
    mutation("/api/account/public-profile", signedIn.sessionCookie, "wrong", '{"enabled":true}'),
    new Request(`${ORIGIN}/api/account/public-profile`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        cookie: `__Host-worldstream-session=${signedIn.sessionCookie}`,
        origin: "https://evil.example",
        "x-worldstream-csrf": csrfValue,
      },
      body: '{"enabled":true}',
    }),
    new Request(`${ORIGIN}/api/account/public-profile`, {
      method: "POST",
      headers: {
        "content-type": "application/x-www-form-urlencoded",
        cookie: `__Host-worldstream-session=${signedIn.sessionCookie}`,
        origin: ORIGIN,
        "x-worldstream-csrf": csrfValue,
      },
      body: "enabled=true",
    }),
  ]) {
    const response = await bff.fetch(request);
    assert.equal(response.status, 403);
  }
  assert.equal(data.profileEnabled, false);
  assert.equal(auth.userCalls, 1);
});

test("every mutation bounds its body and one verified account is durably rate bounded", async () => {
  const { auth, bff, data } = harness();
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  const oversized = JSON.stringify({ padding: "x".repeat(17 * 1024) });
  for (const path of [
    "/api/auth/session/refresh",
    "/api/auth/sign-out",
    "/api/account/public-profile",
    "/api/account/erasure",
  ]) {
    const response = await bff.fetch(
      mutation(path, signedIn.sessionCookie, csrfValue, oversized),
    );
    assert.equal(response.status, 400, path);
  }
  assert.equal(auth.userCalls, 1);
  assert.equal(auth.refreshCalls, 0);
  assert.equal(auth.signOutCalls, 0);
  assert.equal(data.erased, false);

  for (let count = 0; count < 32; count += 1) {
    const accepted = await bff.fetch(
      mutation(
        "/api/account/public-profile",
        signedIn.sessionCookie,
        csrfValue,
        JSON.stringify({ enabled: count % 2 === 0 }),
      ),
    );
    assert.equal(accepted.status, 200, `accepted mutation ${count}`);
  }
  const limited = await bff.fetch(
    mutation(
      "/api/account/public-profile",
      signedIn.sessionCookie,
      csrfValue,
      '{"enabled":true}',
    ),
  );
  assert.equal(limited.status, 429);
  assert.equal(auth.userCalls, 34); // callback, 32 accepted mutations, and the rejected verified user
});

test("OAuth starts and mutations share durable limits across BFF instances", async () => {
  const auth = new FakeAuth();
  const data = new FakeData();
  data.oauthStartLimit = 1;
  data.accountMutationLimit = 1;
  const dependencies: BffDependencies = { authClient: () => auth, dataClient: data };
  const config = {
    canonicalOrigin: ORIGIN,
    allowedReturnTargets: ["/", "/activities/heist"],
    sessionKey: Buffer.alloc(32, 7),
    oauthKey: Buffer.alloc(32, 9),
  } as const;
  const first = createPlatformBff(config, dependencies);
  const second = createPlatformBff(config, dependencies);
  const signedIn = await signIn(first);
  const csrfValue = await csrf(first, signedIn.sessionCookie);

  const startLimited = await second.fetch(
    new Request(`${ORIGIN}/api/auth/github/start?return_to=%2Factivities%2Fheist`),
  );
  assert.equal(startLimited.status, 429);
  assert.equal(data.attempts.size, 1);

  const accepted = await first.fetch(
    mutation("/api/account/public-profile", signedIn.sessionCookie, csrfValue, '{"enabled":true}'),
  );
  const limited = await second.fetch(
    mutation("/api/account/public-profile", signedIn.sessionCookie, csrfValue, '{"enabled":false}'),
  );
  assert.equal(accepted.status, 200);
  assert.equal(limited.status, 429);
  assert.equal(auth.userCalls, 3);
});

test("hosted Run entry resolves only the signed-in account and returns one client URL", async () => {
  const hosted = new FakeHostedBrowserSessions();
  const { bff, data } = harness(hosted);
  data.membership = membership();
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  const response = await bff.fetch(
    mutation(
      "/api/runs/enter",
      signedIn.sessionCookie,
      csrfValue,
      JSON.stringify({
        run_id: data.membership.runId,
        entry_selector: "e".repeat(32),
      }),
    ),
  );
  assert.equal(response.status, 201);
  const body = await response.json() as Record<string, unknown>;
  assert.deepEqual(body, {
    version: "platform_run_entry.v1",
    client_url: `https://arena.example/clients/heist/#handoff=wsh1:${"a".repeat(64)}`,
  });
  assert.deepEqual(data.membershipResolutions, [{
    accountId: "00000000-0000-4000-8000-000000000001",
    runId: data.membership.runId,
    entrySelector: "e".repeat(32),
  }]);
  assert.equal(hosted.issued.length, 1);
  assert.equal(hosted.issued[0]?.accountId, "00000000-0000-4000-8000-000000000001");
  const browserPayload = JSON.stringify(body);
  assert.doesNotMatch(browserPayload, /room_id|membership_id|principal_id|wss1:|wsb1:/u);

  const forged = await bff.fetch(
    mutation(
      "/api/runs/enter",
      signedIn.sessionCookie,
      csrfValue,
      JSON.stringify({
        run_id: data.membership.runId,
        entry_selector: "e".repeat(32),
        account_id: "attacker-selected",
      }),
    ),
  );
  assert.equal(forged.status, 400);
  const malformed = await bff.fetch(
    mutation(
      "/api/runs/enter",
      signedIn.sessionCookie,
      csrfValue,
      JSON.stringify({ run_id: "latest", entry_selector: "not-a-selector" }),
    ),
  );
  assert.equal(malformed.status, 400);
  assert.equal(hosted.issued.length, 1);
  assert.equal(data.membershipResolutions.length, 1);
});

test("hosted redemption installs and rotates only the strict path-scoped opaque cookie", async () => {
  const hosted = new FakeHostedBrowserSessions();
  const { bff } = harness(hosted);
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  const handoff = `wsh1:${"a".repeat(64)}`;

  const firstRequest = mutation(
    "/api/v1/participant-console/handoffs:redeem",
    signedIn.sessionCookie,
    csrfValue,
  );
  firstRequest.headers.set("x-worldstream-participant-handoff", handoff);
  const first = await bff.fetch(firstRequest);
  assert.equal(first.status, 200);
  assert.deepEqual(await first.json(), {
    version: "participant_console_session.v1",
    state: "usable",
    next_action: "continue",
  });
  const firstCookie = first.headers.getSetCookie().join("\n");
  assert.match(firstCookie, /^ws_participant_session=wss1:[0-9a-f]{64};/u);
  assert.match(firstCookie, /Path=\/api\/v1\/participant-console/u);
  assert.match(firstCookie, /Max-Age=43200/u);
  assert.match(firstCookie, /Secure/u);
  assert.match(firstCookie, /HttpOnly/u);
  assert.match(firstCookie, /SameSite=Strict/u);
  assert.doesNotMatch(firstCookie, /Domain=|room_id|membership_id|principal_id|client_id/u);

  const prior = cookieValue(first, "ws_participant_session");
  assert.ok(prior);
  hosted.nextSession = `wss1:${"c".repeat(64)}`;
  const secondRequest = mutation(
    "/api/v1/participant-console/handoffs:redeem",
    signedIn.sessionCookie,
    csrfValue,
  );
  secondRequest.headers.set(
    "cookie",
    `__Host-worldstream-session=${signedIn.sessionCookie}; ws_participant_session=${prior}`,
  );
  secondRequest.headers.set("x-worldstream-participant-handoff", handoff);
  const second = await bff.fetch(secondRequest);
  assert.equal(second.status, 200);
  assert.equal(cookieValue(second, "ws_participant_session"), hosted.nextSession);
  assert.equal(hosted.redeemed[1]?.priorSession, prior);
  assert.equal(hosted.redeemed[1]?.accountId, "00000000-0000-4000-8000-000000000001");
  assert.equal(hosted.redeemed.length, 2);
});

test("hosted status and logout use only the opaque session and fail closed", async () => {
  const hosted = new FakeHostedBrowserSessions();
  const { bff, data } = harness(hosted);
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  const activitySession = `wss1:${"b".repeat(64)}`;
  data.resolveOwnedRunMembership = async () => {
    throw new Error("Supabase should not be needed after admission");
  };

  const status = await bff.fetch(new Request(
    `${ORIGIN}/api/v1/participant-console/session`,
    {
      headers: {
        cookie: `ws_participant_session=${activitySession}`,
        origin: ORIGIN,
        "sec-fetch-site": "same-origin",
      },
    },
  ));
  assert.equal(status.status, 200);
  assert.deepEqual(hosted.statusReads, [activitySession]);

  const logout = mutation(
    "/api/v1/participant-console/session:logout",
    signedIn.sessionCookie,
    csrfValue,
  );
  logout.headers.set(
    "cookie",
    `__Host-worldstream-session=${signedIn.sessionCookie}; ws_participant_session=${activitySession}`,
  );
  const loggedOut = await bff.fetch(logout);
  assert.equal(loggedOut.status, 200);
  assert.deepEqual(hosted.logouts, [activitySession]);
  assert.match(
    loggedOut.headers.getSetCookie().join("\n"),
    /ws_participant_session=; Path=\/api\/v1\/participant-console; Max-Age=0; Secure; HttpOnly; SameSite=Strict/u,
  );

  hosted.redeemError = new HostedBrowserSessionRejectedError();
  const rejected = mutation(
    "/api/v1/participant-console/handoffs:redeem",
    signedIn.sessionCookie,
    csrfValue,
  );
  rejected.headers.set("x-worldstream-participant-handoff", `wsh1:${"d".repeat(64)}`);
  assert.equal((await bff.fetch(rejected)).status, 403);
  assert.equal(hosted.redeemed.length, 1);

  hosted.redeemError = new HostedBrowserSessionMissingError();
  const missing = mutation(
    "/api/v1/participant-console/handoffs:redeem",
    signedIn.sessionCookie,
    csrfValue,
  );
  missing.headers.set("x-worldstream-participant-handoff", `wsh1:${"e".repeat(64)}`);
  assert.equal((await bff.fetch(missing)).status, 401);
  assert.equal(hosted.redeemed.length, 2);
});

test("development identity substitute is explicit and preserves the production session contract", async () => {
  const data = new FakeData();
  const origin = "http://127.0.0.1:5180";
  const bff = createDevelopmentPlatformBff(
    {
      canonicalOrigin: origin,
      allowedReturnTargets: ["/"],
      sessionKey: Buffer.alloc(32, 7),
      oauthKey: Buffer.alloc(32, 9),
      developmentMode: "visible-local-only",
      deploymentEnvironment: "development",
      identity: {
        authUserId: USER.id,
        providerSubject: USER.identities[0]?.subject ?? "",
        githubLogin: "worldstream-local-developer",
        avatarUrl: null,
      },
    },
    data,
  );

  const status = await bff.fetch(new Request(`${origin}/api/dev/status`));
  assert.equal(status.status, 200);
  assert.equal(status.headers.get("x-worldstream-development-substitute"), "identity-bypass");

  const missingAcknowledgement = await bff.fetch(
    new Request(`${origin}/api/dev/sign-in`, {
      method: "POST",
      headers: { "content-type": "application/json", origin },
      body: "{}",
    }),
  );
  assert.equal(missingAcknowledgement.status, 400);

  const signedIn = await bff.fetch(
    new Request(`${origin}/api/dev/sign-in`, {
      method: "POST",
      headers: { "content-type": "application/json", origin },
      body: '{"mode":"visible-local-only"}',
    }),
  );
  assert.equal(signedIn.status, 200);
  const sessionCookie = cookieValue(signedIn, "__Host-worldstream-session");
  assert.ok(sessionCookie);
  assert.match(signedIn.headers.getSetCookie().join("\n"), /Secure; HttpOnly; SameSite=Lax/u);
  assert.deepEqual(data.syncs, [USER.id]);

  const session = await bff.fetch(
    new Request(`${origin}/api/auth/session`, {
      headers: { cookie: `__Host-worldstream-session=${sessionCookie}` },
    }),
  );
  assert.equal(session.status, 200);
  assert.equal(
    ((await session.json()) as { authenticated: boolean }).authenticated,
    true,
  );
});

test("development identity substitute fails closed outside loopback development", () => {
  const data = new FakeData();
  const common = {
    allowedReturnTargets: ["/"],
    sessionKey: Buffer.alloc(32, 7),
    oauthKey: Buffer.alloc(32, 9),
    developmentMode: "visible-local-only",
    deploymentEnvironment: "development",
    identity: {
      authUserId: USER.id,
      providerSubject: USER.identities[0]?.subject ?? "",
      githubLogin: "worldstream-local-developer",
      avatarUrl: null,
    },
  } as const;
  assert.throws(() =>
    createDevelopmentPlatformBff({ ...common, canonicalOrigin: "https://arena.example" }, data),
  );

  const priorNodeEnvironment = process.env.NODE_ENV;
  try {
    process.env.NODE_ENV = "production";
    assert.throws(() =>
      createDevelopmentPlatformBff(
        { ...common, canonicalOrigin: "http://127.0.0.1:5180" },
        data,
      ),
    );
  } finally {
    if (priorNodeEnvironment === undefined) delete process.env.NODE_ENV;
    else process.env.NODE_ENV = priorNodeEnvironment;
  }
});
