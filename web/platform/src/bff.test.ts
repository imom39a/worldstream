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
  HostedBrowserSessionUnavailableError,
  type HostedBrowserHandoff,
  type HostedBrowserSessionClient,
  type HostedBrowserSessionStatus,
  type OwnedRunMembershipCorrespondence,
} from "./browser-sessions.js";
import type { PublicRunData } from "./public-runs.js";
import { createSupabaseBffDependencies } from "./supabase.js";
import { HttpHostedFormationGateway } from "./hosted-formation.js";
import type {
  FormationLaunchRecord,
  HostedFormationData,
  HostedFormationGateway,
  HostedLaunchMaterial,
} from "./hosted-formation.js";
import { AGENT_HEIST_LISTING_DIGEST, MIDNIGHT_ARCHIVE_LISTING_DIGEST } from "./hosted-catalog.js";
import { withPlatformDiagnostics } from "./diagnostics.js";

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
  membershipResolver: ((input: {
    accountId: string;
    runId: string;
    entrySelector: string;
  }) => OwnedRunMembershipCorrespondence | null) | null = null;
  readonly membershipResolutions: Array<{
    readonly accountId: string;
    readonly runId: string;
    readonly entrySelector: string;
  }> = [];
  myGames: import("./bff.js").MyGamesIndex | null = {
    version: "platform_my_games.v1",
    items: [{
      launchId: "10000000-0000-4000-8000-000000000001",
      title: "Agent Heist",
      state: "verified_result",
      updatedAt: "2026-09-08T00:00:00.000Z",
      participation: "external_agent",
      action: "view_result",
      resultPublicId: "a".repeat(32),
    }],
    next: null,
  };

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

  async listMyGames(): Promise<import("./bff.js").MyGamesIndex | null> {
    return this.erased ? null : this.myGames;
  }

  async resolveOwnedRunMembership(input: {
    accountId: string;
    runId: string;
    entrySelector: string;
  }): Promise<OwnedRunMembershipCorrespondence | null> {
    this.membershipResolutions.push(input);
    if (this.membershipResolver !== null) return this.membershipResolver(input);
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
  readonly streamTickets: Array<{ session: string; afterFrameSeq: number | null }> = [];
  readonly logouts: string[] = [];
  redeemError: Error | null = null;
  statusError: Error | null = null;
  streamTicketError: Error | null = null;
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
    if (this.statusError !== null) throw this.statusError;
    return { state: "usable" };
  }

  async issueStreamTicket(
    session: string,
    afterFrameSeq: number | null,
  ): Promise<{ ticket: string; expiresInMs: number }> {
    this.streamTickets.push({ session, afterFrameSeq });
    if (this.streamTicketError !== null) throw this.streamTicketError;
    return { ticket: `wst1:${"c".repeat(64)}`, expiresInMs: 15_000 };
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

function harness(
  hostedBrowserSessions?: HostedBrowserSessionClient,
  publicRunData?: PublicRunData,
  hostedPublicStreamBaseUrl?: string,
  overrides: Partial<BffDependencies> = {},
) {
  const auth = new FakeAuth();
  const data = new FakeData();
  const dependencies: BffDependencies = {
    authClient: () => auth,
    dataClient: data,
    ...(hostedBrowserSessions === undefined ? {} : { hostedBrowserSessions }),
    ...(publicRunData === undefined ? {} : { publicRunData }),
    ...(hostedPublicStreamBaseUrl === undefined ? {} : { hostedPublicStreamBaseUrl }),
    ...overrides,
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
    clientReleaseDigest: `sha256:${"3".repeat(64)}`,
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

test("internal catalog requires sign-in and exact deployment opt-in; missing artifacts block launch", async () => {
  const { bff } = harness(undefined, undefined, undefined, {
    internalCandidateListingDigests: [MIDNIGHT_ARCHIVE_LISTING_DIGEST],
    hostedFormationData: {} as HostedFormationData,
    hostedFormationGateway: {} as HostedFormationGateway,
    hostedFormationHostInstallationId: "internal-test",
    hostedBrowserSessions: {} as HostedBrowserSessionClient,
    hostedActivityAvailable: async () => false,
  });
  assert.equal((await bff.fetch(new Request(`${ORIGIN}/api/catalog/internal`))).status, 401);
  const publicResponse = await bff.fetch(new Request(`${ORIGIN}/api/catalog`));
  assert.equal((await publicResponse.text()).includes("midnight-archive"), false);
  const signedIn = await signIn(bff);
  const response = await bff.fetch(new Request(`${ORIGIN}/api/catalog/internal`, {
    headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` },
  }));
  assert.equal(response.status, 200);
  assert.match(response.headers.get("cache-control") ?? "", /no-store/);
  const body = await response.json() as { activities: Array<{ slug: string; availability: string }> };
  assert.equal(body.activities.find(({ slug }) => slug === "midnight-archive")?.availability, "dependency_unavailable");
  const denied = await bff.fetch(mutation("/api/launches", signedIn.sessionCookie, await csrf(bff, signedIn.sessionCookie), JSON.stringify({
    listing_slug: "midnight-archive", creator_access: "seat", creator_seat: "seat-1",
    fill_mode: "people_only", roster_option: "solo", idempotency_key: "a".repeat(32),
  })));
  assert.equal(denied.status, 409);
  assert.equal((await denied.text()).includes("activity_unavailable"), true);
});

test("signed-in catalog keeps internal candidates hidden without exact opt-in", async () => {
  const { bff } = harness();
  const signedIn = await signIn(bff);
  const response = await bff.fetch(new Request(`${ORIGIN}/api/catalog/internal`, {
    headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` },
  }));
  assert.equal(response.status, 200);
  assert.equal((await response.text()).includes("midnight-archive"), false);
});

test("My games is account-scoped, preserves the reviewed result route, and rejects erased accounts", async () => {
  const { bff, data } = harness();
  const signedIn = await signIn(bff);
  const response = await bff.fetch(new Request(`${ORIGIN}/api/my-games`, {
    headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` },
  }));
  assert.equal(response.status, 200);
  const body = await response.json() as { items: Array<Record<string, unknown>> };
  assert.equal(body.items[0]?.participation, "external_agent");
  assert.equal(body.items[0]?.result_public_id, "a".repeat(32));
  assert.equal("entry_selector" in (body.items[0] ?? {}), false);

  data.erased = true;
  const erased = await bff.fetch(new Request(`${ORIGIN}/api/my-games`, {
    headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` },
  }));
  assert.equal(erased.status, 401);
});

test("creator close uses the same evidence-backed lane before and after Host mutation", async () => {
  const launchId = "20000000-0000-4000-8000-000000000002";
  const launchTemplate = {
    launchRequestId: launchId,
    listingRevisionDigest: AGENT_HEIST_LISTING_DIGEST,
    state: "provisioning",
    expiresAt: "2026-09-09T00:00:00.000Z",
    creatorAccessChoice: "seat",
    creatorSeatId: "seat-1",
    houseFillChoice: "disabled",
    rosterFrozen: true,
    canManage: true,
    seats: [],
  } as FormationLaunchRecord;
  const makeBff = (hostMutationStarted: boolean, initialState = "provisioning") => {
    const counters = { requests: 0, records: 0, gateway: 0 };
    let launch = { ...launchTemplate, state: initialState };
    let material = {
      launchRequestId: launchId,
      listingRevisionDigest: AGENT_HEIST_LISTING_DIGEST,
      state: initialState,
      expiresAt: "2026-09-09T00:00:00.000Z",
      launchInputs: {},
      houseFillChoice: "disabled",
      creatorAccessChoice: "seat",
      creatorSeatId: "seat-1",
      hostInstallationId: hostMutationStarted ? "fly-primary" : null,
      roomSetupOperationId: hostMutationStarted
        ? `launch-${launchId.replaceAll("-", "")}`
        : null,
      rosterFrozen: hostMutationStarted,
      hostMutationStarted,
      claims: [{
        seatId: "seat-1",
        displayName: "Creator",
        participationKind: "account_human",
        principalReference: "seat:seat-1",
      }],
      houseAssignments: [],
    } as HostedLaunchMaterial;
    const hostedData = {
      readLaunchRequest: async () => launch,
      readHostedLaunchMaterial: async () => material,
      readHostedRecoveryMaterial: async () => material.state === "closing" ? material : null,
      readHouseFill: async () => null,
      readGenesisReconciliation: async () => ({
        launchState: material.state,
        runId: null,
        reconciliationState: "ready",
        needsGenesisPull: false,
      }),
      requestLaunchClosure: async () => {
        counters.requests += 1;
        material = { ...material, state: "closing" };
        launch = { ...launch, state: "closing" };
        return true;
      },
      recordLaunchClosure: async () => {
        counters.records += 1;
        material = { ...material, state: "closed_by_creator" };
        launch = { ...launch, state: "closed_by_creator" };
        return true;
      },
    } as unknown as HostedFormationData;
    const hostedGateway = {
      closeLaunch: async (request: Record<string, unknown>) => {
        counters.gateway += 1;
        return {
          schema: "worldstream/hosted-launch-closure-evidence/v1",
          host_installation_id: "fly-primary",
          launch_request_id: request.launch_request_id,
          listing_revision_digest: request.listing_revision_digest,
          launch_request_digest: request.launch_request_digest,
          room_setup_operation_id: request.room_setup_operation_id,
          disposition: "cancelled_before_genesis",
          room_id: null,
          room_head: null,
          closure_fence_digest: `blake3:${"a".repeat(64)}`,
          authentication_tag: "b".repeat(64),
        };
      },
    } as unknown as HostedFormationGateway;
    const bff = createPlatformBff({
      canonicalOrigin: ORIGIN,
      allowedReturnTargets: ["/", "/activities/heist"],
      sessionKey: Buffer.alloc(32, 7),
      oauthKey: Buffer.alloc(32, 9),
    }, {
      authClient: () => new FakeAuth(),
      dataClient: new FakeData(),
      hostedFormationData: hostedData,
      hostedFormationGateway: hostedGateway,
      hostedFormationHostInstallationId: "fly-primary",
    });
    return { bff, counters };
  };

  {
    const { bff, counters } = makeBff(false);
    const signedIn = await signIn(bff);
    const response = await bff.fetch(mutation(
      `/api/launches/${launchId}/close`,
      signedIn.sessionCookie,
      await csrf(bff, signedIn.sessionCookie),
    ));
    assert.equal(response.status, 200);
    assert.deepEqual(await response.json(), {
      version: "hosted_launch_closed.v1",
      closed: true,
    });
    assert.deepEqual(counters, { requests: 1, records: 1, gateway: 1 });
  }

  {
    const { bff, counters } = makeBff(true);
    const signedIn = await signIn(bff);
    const response = await bff.fetch(mutation(
      `/api/launches/${launchId}/close`,
      signedIn.sessionCookie,
      await csrf(bff, signedIn.sessionCookie),
    ));
    assert.equal(response.status, 200);
    assert.deepEqual(await response.json(), {
      version: "hosted_launch_closed.v1",
      closed: true,
    });
    assert.deepEqual(counters, { requests: 1, records: 1, gateway: 1 });
  }

  {
    const { bff, counters } = makeBff(true, "closing");
    const signedIn = await signIn(bff);
    const response = await bff.fetch(new Request(`${ORIGIN}/api/launches/${launchId}`, {
      headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` },
    }));
    assert.equal(response.status, 200);
    assert.equal((await response.json() as { state: string }).state, "closed_by_creator");
    assert.deepEqual(counters, { requests: 0, records: 1, gateway: 1 });
  }
});

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

test("authenticated session supplies only the configured fixed direct browser stream", async () => {
  for (const [gateway, expected] of [
    ["https://worldstream-preview.fly.dev", "wss://worldstream-preview.fly.dev/v1/hosted/browser-stream"],
    ["http://127.0.0.1:8080", "ws://127.0.0.1:8080/v1/hosted/browser-stream"],
  ]) {
    const { bff } = harness(undefined, undefined, gateway);
    const { sessionCookie } = await signIn(bff);
    const response = await bff.fetch(new Request(`${ORIGIN}/api/auth/session?browser_stream_url=wss://attacker.example/`, {
      headers: { cookie: `__Host-worldstream-session=${sessionCookie}` },
    }));
    assert.equal(response.status, 200);
    const body = await response.json();
    assert.equal(body.browser_stream_url, expected);
    assert.match(response.headers.get("cache-control") ?? "", /no-store/u);
    const spoofed = await bff.fetch(new Request(`${ORIGIN}/api/auth/session`, {
      headers: { cookie: `__Host-worldstream-session=${sessionCookie}`, "x-forwarded-host": "attacker.example" },
    }));
    assert.equal(spoofed.status, 403);
    const anonymous = await bff.fetch(new Request(`${ORIGIN}/api/auth/session`));
    assert.equal(anonymous.status, 401);
    assert.equal((await anonymous.json()).browser_stream_url, undefined);
  }
});

test("session does not invent a same-origin stream when deployment routing is absent", async () => {
  const { bff } = harness();
  const { sessionCookie } = await signIn(bff);
  const response = await bff.fetch(new Request(`${ORIGIN}/api/auth/session`, {
    headers: { cookie: `__Host-worldstream-session=${sessionCookie}` },
  }));
  assert.equal(response.status, 200);
  assert.equal((await response.json()).browser_stream_url, undefined);
  assert.throws(() => harness(undefined, undefined, "https://attacker.example/path"));
});

test("anonymous public Run and Recent Results routes expose only no-store DTOs", async () => {
  const calls: string[] = [];
  const publicRuns: PublicRunData = {
    async readPublicRun(publicId) {
      calls.push(`run:${publicId}`);
      return { version: "public_run.v1", state: "unavailable" };
    },
    async listRecentResults(limit) {
      calls.push(`recent:${limit}`);
      return {
        version: "recent_results.v1",
        activity: "agent-heist",
        order: "newest_first",
        maximum: 20,
        results: [],
      };
    },
  };
  const { bff } = harness(undefined, publicRuns);
  const run = await bff.fetch(new Request(`${ORIGIN}/api/runs/${"a".repeat(32)}`));
  assert.equal(run.status, 200);
  assert.equal(run.headers.get("cache-control"), "no-store, max-age=0");
  assert.deepEqual(await run.json(), { version: "public_run.v1", state: "unavailable" });

  const recent = await bff.fetch(new Request(`${ORIGIN}/api/results/agent-heist/recent`));
  assert.equal(recent.status, 200);
  assert.equal(recent.headers.get("cache-control"), "no-store, max-age=0");
  assert.equal(((await recent.json()) as { results: unknown[] }).results.length, 0);
  assert.deepEqual(calls, [`run:${"a".repeat(32)}`, "recent:20"]);

  const malformed = await bff.fetch(new Request(`${ORIGIN}/api/runs/not-a-public-id`));
  assert.equal(malformed.status, 404);
});

test("a live public Run points directly to Fly and never carries a credential", async () => {
  const source = {
    version: "public_run.v1",
    state: "live",
    public_id: "b".repeat(32),
    activity: {
      listing_key: "worldstream.agent-heist.public-preview",
      title: "Agent Heist",
      description: "A live social strategy activity.",
      listing_revision: `blake3:${"1".repeat(64)}`,
      pack: {
        id: "worldstream.agent-heist",
        version: "0.2.0",
        revision: `blake3:${"2".repeat(64)}`,
      },
    },
    started_at: "2026-09-05T10:00:00.000Z",
    evidence: { class: "unranked", label: "Unranked activity" },
    participants: [],
    live: { available: true },
  } as const;
  const publicRuns: PublicRunData = {
    async readPublicRun() { return source; },
    async listRecentResults() {
      return {
        version: "recent_results.v1",
        activity: "agent-heist",
        order: "newest_first",
        maximum: 20,
        results: [],
      };
    },
  };
  const { bff } = harness(undefined, publicRuns, "https://stream.arena.example");
  const response = await bff.fetch(
    new Request(`${ORIGIN}/api/runs/${"b".repeat(32)}`),
  );
  assert.equal(response.status, 200);
  const text = await response.text();
  assert.match(
    text,
    /"stream_url":"wss:\/\/stream\.arena\.example\/v1\/hosted\/public-runs\/b{32}\/stream"/u,
  );
  assert.doesNotMatch(text, /ticket|token|credential|membership|room_id/u);
});

test("an anonymous public Run receives only the server-selected exact viewer release", async () => {
  const source = {
    version: "public_run.v1",
    state: "live",
    public_id: "c".repeat(32),
    activity: {
      listing_key: "worldstream.agent-heist.public-preview",
      title: "Agent Heist",
      description: "A live social strategy activity.",
      listing_revision: AGENT_HEIST_LISTING_DIGEST,
      pack: {
        id: "worldstream.agent-heist",
        version: "0.3.0",
        revision: `blake3:${"2".repeat(64)}`,
      },
    },
    started_at: "2026-09-05T10:00:00.000Z",
    evidence: { class: "unranked", label: "Unranked activity" },
    participants: [],
    live: { available: true },
  } as const;
  const publicRuns: PublicRunData = {
    async readPublicRun() { return source; },
    async listRecentResults() {
      return { version: "recent_results.v1", activity: "agent-heist", order: "newest_first", maximum: 20, results: [] };
    },
  };
  const { bff } = harness(undefined, publicRuns, "https://stream.arena.example");
  const response = await bff.fetch(new Request(`${ORIGIN}/api/runs/${"c".repeat(32)}`));
  assert.equal(response.status, 200);
  const body = await response.json() as Record<string, unknown>;
  assert.deepEqual(body.client, {
    launch_url: `https://arena.example/agent-heist-v10/hosted/?public_run=${"c".repeat(32)}&platform_return=%2F&platform_result=%2Fruns%2F${"c".repeat(32)}`,
    back_to_games: "/",
    result_url: `/runs/${"c".repeat(32)}`,
  });
  assert.doesNotMatch(JSON.stringify(body), /handoff|membership|credential|room_id|entry_selector/u);
});

test("a retained public Run never upgrades itself to the current viewer release", async () => {
  const source = {
    version: "public_run.v1",
    state: "live",
    public_id: "d".repeat(32),
    activity: {
      listing_key: "worldstream.agent-heist.public-preview",
      title: "Retained Agent Heist",
      description: "A retained client contract.",
      listing_revision: "blake3:48c397a32632896d66beb9ae7f8a6d090338800187c56eb0593997b80bd2b630",
      pack: { id: "worldstream.agent-heist", version: "0.3.0", revision: `blake3:${"2".repeat(64)}` },
    },
    started_at: "2026-09-05T10:00:00.000Z",
    evidence: { class: "unranked", label: "Unranked activity" },
    participants: [],
    live: { available: true },
  } as const;
  const publicRuns: PublicRunData = {
    async readPublicRun() { return source; },
    async listRecentResults() {
      return { version: "recent_results.v1", activity: "agent-heist", order: "newest_first", maximum: 20, results: [] };
    },
  };
  const { bff } = harness(undefined, publicRuns, "https://stream.arena.example");
  const response = await bff.fetch(new Request(`${ORIGIN}/api/runs/${"d".repeat(32)}`));
  assert.equal(response.status, 200);
  const body = await response.json() as Record<string, unknown>;
  assert.equal(body.client, undefined);
  assert.match(JSON.stringify(body), /worldstream-preview\.fly|stream\.arena/u);
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

test("the deployment-neutral BFF still rejects caller forwarding metadata", async () => {
  const { bff } = harness();
  const response = await bff.fetch(new Request(`${ORIGIN}/api/auth/session`, {
    headers: { forwarded: "for=203.0.113.7;host=arena.example;proto=https" },
  }));
  assert.equal(response.status, 403);
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

test("closed people-only launch admission returns temporary unavailability without contacting Fly", async () => {
  const originalFetch = globalThis.fetch;
  const requests: string[] = [];
  try {
    globalThis.fetch = async (input, init) => {
      const request = input instanceof Request ? input : new Request(input, init);
      requests.push(request.url);
      assert.equal(request.url, "https://project.supabase.co/rest/v1/rpc/create_launch_request_v1");
      assert.equal((await request.json()).p_house_fill_choice, "disabled");
      return Response.json({ code: "55000", message: "hosted_launches_closed" }, { status: 400 });
    };
    const formationData = createSupabaseBffDependencies({
      url: "https://project.supabase.co",
      publishableKey: `sb_publishable_${"p".repeat(32)}`,
      dataSecretKey: `sb_secret_${"s".repeat(32)}`,
    }).hostedFormationData;
    assert.ok(formationData);
    const auth = new FakeAuth();
    const bff = createPlatformBff({
      canonicalOrigin: ORIGIN,
      allowedReturnTargets: ["/", "/activities/heist"],
      sessionKey: Buffer.alloc(32, 7),
      oauthKey: Buffer.alloc(32, 9),
    }, {
      authClient: () => auth,
      dataClient: new FakeData(),
      hostedFormationData: formationData,
      hostedFormationGateway: new HttpHostedFormationGateway({
        baseUrl: "https://gateway.example",
        serviceAuthority: "synthetic-gateway-authority-at-least-32-characters",
      }),
      hostedFormationHostInstallationId: "fly-primary",
    });
    const { sessionCookie } = await signIn(bff);
    const response = await bff.fetch(mutation("/api/launches", sessionCookie, await csrf(bff, sessionCookie), JSON.stringify({
      listing_slug: "agent-heist",
      creator_access: "seat",
      creator_seat: "seat-1",
      fill_mode: "people_only",
      idempotency_key: "a".repeat(32),
    })));
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { error: { code: "temporarily_unavailable" } });
    assert.match(response.headers.get("cache-control") ?? "", /no-store/u);
    assert.deepEqual(response.headers.getSetCookie(), []);
    assert.deepEqual(requests, ["https://project.supabase.co/rest/v1/rpc/create_launch_request_v1"]);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("a confirmed activity-capacity gate is not presented as a malformed formation", async () => {
  const originalFetch = globalThis.fetch;
  const requests: string[] = [];
  try {
    globalThis.fetch = async (input) => {
      const request = input instanceof Request ? input : new Request(input);
      requests.push(request.url);
      assert.equal(request.url, "https://project.supabase.co/rest/v1/rpc/create_launch_request_v1");
      return Response.json({
        code: "55000",
        message: "pre_genesis_capacity_unavailable",
      }, { status: 400 });
    };
    const formationData = createSupabaseBffDependencies({
      url: "https://project.supabase.co",
      publishableKey: `sb_publishable_${"p".repeat(32)}`,
      dataSecretKey: `sb_secret_${"s".repeat(32)}`,
    }).hostedFormationData;
    assert.ok(formationData);
    const auth = new FakeAuth();
    const bff = createPlatformBff({
      canonicalOrigin: ORIGIN,
      allowedReturnTargets: ["/", "/activities/heist"],
      sessionKey: Buffer.alloc(32, 7),
      oauthKey: Buffer.alloc(32, 9),
    }, {
      authClient: () => auth,
      dataClient: new FakeData(),
      hostedFormationData: formationData,
      hostedFormationGateway: new HttpHostedFormationGateway({
        baseUrl: "https://gateway.example",
        serviceAuthority: "synthetic-gateway-authority-at-least-32-characters",
      }),
      hostedFormationHostInstallationId: "fly-primary",
    });
    const { sessionCookie } = await signIn(bff);
    const response = await bff.fetch(mutation("/api/launches", sessionCookie, await csrf(bff, sessionCookie), JSON.stringify({
      listing_slug: "agent-heist",
      creator_access: "seat",
      creator_seat: "seat-1",
      fill_mode: "people_only",
      idempotency_key: "b".repeat(32),
    })));
    assert.equal(response.status, 409);
    assert.deepEqual(await response.json(), { error: { code: "activity_capacity_unavailable" } });
    assert.deepEqual(requests, ["https://project.supabase.co/rest/v1/rpc/create_launch_request_v1"]);
  } finally {
    globalThis.fetch = originalFetch;
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

test("retained Runs without their original client cannot silently enter a newer release", async () => {
  const hosted = new FakeHostedBrowserSessions();
  const { bff, data } = harness(hosted);
  data.membership = {
    ...membership(),
    listingRevisionDigest: "blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956",
  };
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  const response = await bff.fetch(mutation(
    "/api/runs/enter", signedIn.sessionCookie, csrfValue,
    JSON.stringify({ run_id: data.membership.runId, entry_selector: "e".repeat(32) }),
  ));
  assert.equal(response.status, 409);
  assert.deepEqual(await response.json(), { error: { code: "run_client_unavailable" } });
  assert.equal(hosted.issued.length, 0);
  for (const path of ["/agent-heist", "/agent-heist/", "/agent-heist/assets/old.js"]) {
    const retired = await bff.fetch(new Request(`${ORIGIN}${path}`));
    assert.equal(retired.status, 404);
  }
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
    client_url: `https://arena.example/clients/heist/?platform_return=%2F#handoff=wsh1:${"a".repeat(64)}`,
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

test("an entry timeout keeps sign-in and only an explicit retry issues a handoff for the same Room", async () => {
  const hosted = new FakeHostedBrowserSessions();
  const issue = hosted.issueHandoff.bind(hosted);
  let attempts = 0;
  hosted.issueHandoff = async (account, binding) => {
    attempts += 1;
    if (attempts === 1) throw new HostedBrowserSessionUnavailableError(undefined, {
      cause: new DOMException("private upstream URL", "TimeoutError"),
    });
    return issue(account, binding);
  };
  const harnessValue = harness(hosted);
  harnessValue.data.membership = membership();
  const logs: string[] = [];
  const bff = withPlatformDiagnostics(harnessValue.bff, (line) => logs.push(line));
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  const enter = () => bff.fetch(mutation("/api/runs/enter", signedIn.sessionCookie, csrfValue,
    JSON.stringify({ run_id: membership().runId, entry_selector: "e".repeat(32) })));
  const failed = await enter();
  assert.equal(failed.status, 503);
  assert.equal(failed.headers.get("set-cookie"), null);
  assert.equal(attempts, 1);
  assert.equal(hosted.issued.length, 0);
  const requestId = failed.headers.get("x-worldstream-request-id");
  assert.ok(logs.some((line) => {
    const value = JSON.parse(line);
    return value.request_id === requestId && value.operation === "run_entry" && value.error_kind === "timeout";
  }));
  const recovered = await enter();
  assert.equal(recovered.status, 201);
  assert.equal(attempts, 2);
  assert.equal(hosted.issued.length, 1);
  assert.deepEqual(hosted.issued[0]?.binding, membership());
  assert.equal(harnessValue.data.membershipResolutions.length, 2);
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

  const streamTicket = mutation(
    "/api/v1/participant-console/session:stream-ticket",
    signedIn.sessionCookie,
    csrfValue,
    JSON.stringify({ after_frame_seq: 29 }),
  );
  streamTicket.headers.set(
    "cookie",
    `__Host-worldstream-session=${signedIn.sessionCookie}; ws_participant_session=${activitySession}`,
  );
  const issued = await bff.fetch(streamTicket);
  assert.equal(issued.status, 201);
  assert.deepEqual(await issued.json(), {
    version: "participant_console_stream_ticket.v1",
    ticket: `wst1:${"c".repeat(64)}`,
    expires_in_ms: 15_000,
  });
  assert.deepEqual(hosted.streamTickets, [
    { session: activitySession, afterFrameSeq: 29 },
  ]);

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

test("hosted re-entry preserves the original participant Membership and supports spectator admission", async () => {
  const hosted = new FakeHostedBrowserSessions();
  const { bff, data } = harness(hosted);
  data.membership = membership();
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  const enter = async () => bff.fetch(mutation(
    "/api/runs/enter",
    signedIn.sessionCookie,
    csrfValue,
    JSON.stringify({ run_id: data.membership?.runId, entry_selector: "e".repeat(32) }),
  ));
  const redeem = async (response: Response, priorSession?: string) => {
    const body = await response.json() as { client_url: string };
    const handoff = new URL(body.client_url).hash.slice("#handoff=".length);
    const request = mutation(
      "/api/v1/participant-console/handoffs:redeem",
      signedIn.sessionCookie,
      csrfValue,
    );
    request.headers.set("x-worldstream-participant-handoff", handoff);
    if (priorSession !== undefined) {
      request.headers.set(
        "cookie",
        `__Host-worldstream-session=${signedIn.sessionCookie}; ws_participant_session=${priorSession}`,
      );
    }
    return bff.fetch(request);
  };

  const firstEntry = await enter();
  assert.equal(firstEntry.status, 201);
  const firstRedemption = await redeem(firstEntry);
  assert.equal(firstRedemption.status, 200);
  const firstSession = cookieValue(firstRedemption, "ws_participant_session");
  assert.ok(firstSession);

  hosted.nextSession = `wss1:${"d".repeat(64)}`;
  const reentry = await enter();
  assert.equal(reentry.status, 201);
  const secondRedemption = await redeem(reentry, firstSession);
  assert.equal(secondRedemption.status, 200);
  assert.equal(hosted.redeemed.at(-1)?.priorSession, firstSession);
  assert.equal(hosted.issued[0]?.binding.membershipId, data.membership?.membershipId);
  assert.equal(hosted.issued[1]?.binding.membershipId, data.membership?.membershipId);
  assert.equal(hosted.issued[1]?.binding.roomId, hosted.issued[0]?.binding.roomId);
  assert.equal(hosted.issued[1]?.binding.runId, hosted.issued[0]?.binding.runId);

  data.membership = {
    ...membership(),
    accessMode: "spectator",
    purpose: "creator_spectator",
    seatId: null,
    role: null,
    principalKind: "human",
  };
  const spectatorEntry = await enter();
  assert.equal(spectatorEntry.status, 201);
  assert.equal((await redeem(spectatorEntry)).status, 200);
  assert.equal(hosted.issued.at(-1)?.binding.accessMode, "spectator");
  assert.equal(hosted.issued.at(-1)?.binding.purpose, "creator_spectator");
  assert.equal(hosted.issued.at(-1)?.binding.seatId, null);
  assert.equal(hosted.issued.at(-1)?.binding.role, null);
});

test("expired hosted sessions clear only activity access, refuse cross-Run entry, and keep sign-in continuity", async () => {
  const hosted = new FakeHostedBrowserSessions();
  const { bff, data } = harness(hosted);
  data.membership = membership();
  data.membershipResolver = (input) =>
    input.runId === data.membership?.runId && input.entrySelector === "e".repeat(32)
      ? data.membership
      : null;
  const signedIn = await signIn(bff);
  const csrfValue = await csrf(bff, signedIn.sessionCookie);
  const expired = `wss1:${"e".repeat(64)}`;
  hosted.statusError = new HostedBrowserSessionMissingError();

  const status = await bff.fetch(new Request(
    `${ORIGIN}/api/v1/participant-console/session`,
    {
      headers: {
        cookie: `__Host-worldstream-session=${signedIn.sessionCookie}; ws_participant_session=${expired}`,
        origin: ORIGIN,
        "sec-fetch-site": "same-origin",
      },
    },
  ));
  assert.equal(status.status, 401);
  assert.deepEqual(await status.json(), {
    code: "participant_session_authority_invalid",
    message: "This Activity Client session is not available.",
    next_action: "return_to_task_setup",
    retryable: false,
  });
  assert.match(status.headers.getSetCookie().join("\n"), /ws_participant_session=;.*Max-Age=0/u);

  const signInStatus = await bff.fetch(new Request(`${ORIGIN}/api/auth/session`, {
    headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` },
  }));
  assert.equal(signInStatus.status, 200);
  assert.equal((await signInStatus.json() as { authenticated: boolean }).authenticated, true);

  const crossRun = await bff.fetch(mutation(
    "/api/runs/enter",
    signedIn.sessionCookie,
    csrfValue,
    JSON.stringify({
      run_id: "20000000-0000-4000-8000-000000000002",
      entry_selector: "e".repeat(32),
    }),
  ));
  assert.equal(crossRun.status, 404);
  assert.deepEqual(await crossRun.json(), { error: { code: "run_entry_unavailable" } });
  assert.equal(hosted.issued.length, 0);
});

test("transient hosted session outages retain the Browser Activity Session and platform sign-in", async () => {
  const hosted = new FakeHostedBrowserSessions();
  const { bff } = harness(hosted);
  const signedIn = await signIn(bff);
  const activitySession = `wss1:${"f".repeat(64)}`;
  hosted.statusError = new HostedBrowserSessionUnavailableError();

  const status = await bff.fetch(new Request(
    `${ORIGIN}/api/v1/participant-console/session`,
    {
      headers: {
        cookie: `__Host-worldstream-session=${signedIn.sessionCookie}; ws_participant_session=${activitySession}`,
        origin: ORIGIN,
        "sec-fetch-site": "same-origin",
      },
    },
  ));
  assert.equal(status.status, 503);
  assert.deepEqual(await status.json(), {
    code: "participant_session_unavailable",
    message: "The Activity Client cannot reach the Room service safely.",
    next_action: "reconnect",
    retryable: true,
  });
  assert.doesNotMatch(status.headers.getSetCookie().join("\n"), /ws_participant_session=;.*Max-Age=0/u);

  const signInStatus = await bff.fetch(new Request(`${ORIGIN}/api/auth/session`, {
    headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` },
  }));
  assert.equal(signInStatus.status, 200);
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

test("reviewed roster choices expose labels and freeze only exact option input through authenticated API", async () => {
  const { readFileSync } = await import("node:fs");
  const { encodeCanonical } = await import("@worldstream/pack-sdk");
  const { readListingRevision, resolveRosterOption } = await import("@worldstream/hosted-contract");
  const { reviewedActivityByDigest } = await import("./hosted-catalog.js");
  const listing = readListingRevision(encodeCanonical(JSON.parse(readFileSync(
    new URL("../../../fixtures/hosted-contract/valid/roster-options-listing.json", import.meta.url), "utf8"))));
  const base = reviewedActivityByDigest(MIDNIGHT_ARCHIVE_LISTING_DIGEST)!;
  const reviewed = { ...base, slug: "roster-test", listing, public: { ...base.public,
    slug: "roster-test", houseFillAvailable: true,
    seats: listing.value.seats.map((seat, index) => ({ key: `seat-${index + 1}`, label: seat.display_name, required: seat.required })),
  } };
  const accepted: Array<Parameters<HostedFormationData["createLaunchRequest"]>[0]> = [];
  const launchId = "d8000000-0000-4000-8000-000000000001";
  const data = {
    createLaunchRequest: async (input: Parameters<HostedFormationData["createLaunchRequest"]>[0]) => {
      accepted.push(input);
      return { launchRequestId: launchId, state: "collecting_roster", expiresAt: "2099-01-01T00:00:00Z", wasCreated: true };
    },
    readLaunchRequest: async () => {
      const input = accepted.at(-1)!;
      const option = resolveRosterOption(listing, JSON.parse(new TextDecoder().decode(input.canonicalLaunchInput)))!;
      return { launchRequestId: launchId, listingRevisionDigest: listing.digest, state: "collecting_roster", expiresAt: "2099-01-01T00:00:00Z",
        creatorAccessChoice: "seat", creatorSeatId: "lead", houseFillChoice: input.houseFillChoice, rosterFrozen: false, canManage: true,
        seats: option.seat_ids.map((id) => ({ seatId: id, displayName: id, required: true, participationKind: id === "lead" ? "account_human" : null,
          claimedByRequester: id === "lead", claimed: id === "lead" })) };
    },
    readHouseFill: async () => null,
  } as unknown as HostedFormationData;
  const { bff } = harness(undefined, undefined, undefined, {
    reviewedActivities: [reviewed], internalCandidateListingDigests: [listing.digest], hostedActivityAvailable: async () => true,
    hostedFormationData: data, hostedFormationGateway: {} as HostedFormationGateway,
    hostedFormationHostInstallationId: "roster-test", hostedBrowserSessions: {} as HostedBrowserSessionClient,
  });
  const signedIn = await signIn(bff);
  const token = await csrf(bff, signedIn.sessionCookie);
  const catalog = await bff.fetch(new Request(`${ORIGIN}/api/catalog/internal`, { headers: { cookie: `__Host-worldstream-session=${signedIn.sessionCookie}` } }));
  const summary = (await catalog.json()).activities.find((activity: { slug: string }) => activity.slug === "roster-test");
  assert.deepEqual(summary.rosterOptions.map((option: { label: string }) => option.label), ["Solo", "One supplied agent", "Other supplied agent", "Two supplied agents"]);
  assert.equal(JSON.stringify(summary).includes("house_agent_revision_digest"), false);
  assert.equal(JSON.stringify(summary).includes("configuration"), false);
  const body = { listing_slug: "roster-test", creator_access: "seat", creator_seat: "seat-1", fill_mode: "people_only", idempotency_key: "r".repeat(32), roster_option: "solo" };
  for (const invalid of [null, { ...body, roster_option: "unreviewed" }, { ...body, roster_option: "first" },
    { ...body, creator_seat: "seat-2" }, { ...body, provider: "arbitrary" }, { ...body, listing_slug: "agent-heist" }]) {
    assert.equal((await bff.fetch(mutation("/api/launches", signedIn.sessionCookie, token, JSON.stringify(invalid)))).status, 400);
  }
  assert.equal(accepted.length, 0);
  for (const [option, fill] of [["solo", "people_only"], ["first", "house_agents"]]) {
    const response = await bff.fetch(mutation("/api/launches", signedIn.sessionCookie, token, JSON.stringify({ ...body, roster_option: option, fill_mode: fill })));
    assert.equal(response.status, 201);
    assert.deepEqual(JSON.parse(new TextDecoder().decode(accepted.at(-1)!.canonicalLaunchInput)), { roster_option: option });
    assert.equal(accepted.at(-1)!.listingRevisionDigest, listing.digest);
  }
});
