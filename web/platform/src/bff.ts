import {
  createCipheriv,
  createDecipheriv,
  createHash,
  randomBytes,
  timingSafeEqual,
} from "node:crypto";
import { isIP } from "node:net";
import { reportPlatformFailure } from "./diagnostics.js";

import {
  HostedBrowserSessionMissingError,
  HostedBrowserSessionRejectedError,
  type HostedBrowserSessionClient,
  type OwnedRunMembershipCorrespondence,
} from "./browser-sessions.js";
import {
  HostedFormationCoordinator,
  HostedFormationRejectedError,
  HostedFormationUnavailableError,
  sha256 as formationSha256,
  validLaunchIdentifier,
  type AccountParticipation,
  type FormationLaunchRecord,
  type HostedFormationData,
  type HostedFormationGateway,
  type HouseFillRecord,
  type OwnedRunRecord,
} from "./hosted-formation.js";
import {
  listPublicHostedActivities,
  reviewedActivityByDigest,
  reviewedPublicViewerClientPath,
  reviewedActivityBySlug,
  reviewedSeatId,
  reviewedSeatKey,
} from "./hosted-catalog.js";
import {
  presentPublicRun,
  presentPublicViewerClient,
  publicProjectionStreamBaseUrl,
  type PublicRunData,
} from "./public-runs.js";
import { encodeCanonical } from "@worldstream/pack-sdk";

const OAUTH_COOKIE = "__Host-worldstream-oauth";
const SESSION_COOKIE = "__Host-worldstream-session";
const ACTIVITY_SESSION_COOKIE = "ws_participant_session";
const ACTIVITY_SESSION_COOKIE_PATH = "/api/v1/participant-console";
const ACTIVITY_SESSION_MAX_AGE = 12 * 60 * 60;
const OAUTH_MAX_AGE = 600;
const SESSION_MAX_AGE = 7 * 24 * 60 * 60;
const MAX_BODY_BYTES = 16 * 1024;
const MAX_RETURN_TARGETS = 32;
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const ENTRY_SELECTOR_PATTERN = /^[0-9a-f]{32}$/u;
export const DEVELOPMENT_IDENTITY_MODE = "visible-local-only" as const;
const VERCEL_FORWARDING_HEADERS = new Set([
  "x-forwarded-host",
  "x-forwarded-port",
  "x-forwarded-proto",
  "x-forwarded-for",
  "x-vercel-forwarded-for",
  "x-real-ip",
]);

export interface OAuthAttempt {
  readonly stateDigest: string;
  readonly encryptedPkceVerifier: string;
  readonly browserNonceDigest: string;
  readonly returnTarget: string;
}

export interface PlatformAccount {
  readonly accountId: string;
  readonly publicProfileEnabled: boolean;
}

export interface MyGamesIndex {
  readonly version: "platform_my_games.v1";
  readonly items: readonly {
    readonly launchId: string;
    readonly title: string;
    /** A private, derived status from launch and verified result-index evidence. */
    readonly state:
      | "setup_pending"
      | "setup_cancelled"
      | "setup_abandoned"
      | "setup_failed"
      | "live"
      | "publication_pending"
      | "terminal_without_outcome"
      | "result_suppressed"
      | "dependency_failure"
      | "verified_result";
    readonly updatedAt: string;
    readonly participation: "human" | "external_agent";
    readonly action: "continue_setup" | "return_to_game" | "view_result" | "none";
    readonly resultPublicId: string | null;
  }[];
  readonly next: { readonly beforeAt: string; readonly beforeLaunchId: string } | null;
}

export interface SessionRefreshAdmission extends PlatformAccount {
  readonly admitted: boolean;
}

export interface AuthSession {
  readonly accessToken: string;
  readonly refreshToken: string;
  readonly expiresAt: number;
}

export interface RefreshedAuthSession extends AuthSession {
  readonly user: AuthUser;
}

export interface AuthIdentity {
  readonly provider: "github";
  readonly subject: string;
  readonly login: string | null;
  readonly avatarUrl: string | null;
}

export interface AuthUser {
  readonly id: string;
  readonly identities: readonly AuthIdentity[];
}

export interface PlatformAuthClient {
  githubAuthorizeUrl(input: {
    callbackUrl: string;
    codeChallenge: string;
  }): string;
  exchangeCodeForSession(code: string, verifier: string): Promise<AuthSession>;
  getClaims(accessToken: string): Promise<{ subject: string }>;
  getUser(accessToken: string): Promise<AuthUser>;
  /** Returns the user carried by the refresh exchange; callers do no later lookup. */
  refreshSession(refreshToken: string): Promise<RefreshedAuthSession>;
  signOut(accessToken: string): Promise<void>;
}

export interface PlatformDataClient {
  beginGithubOAuth(attempt: OAuthAttempt): Promise<boolean>;
  consumeGithubOAuth(input: {
    stateDigest: string;
    browserNonceDigest: string;
    returnTarget: string;
  }): Promise<OAuthAttempt | null>;
  resolveGithubAccount(input: {
    authUserId: string;
    providerSubject: string;
  }): Promise<PlatformAccount | null>;
  beginSessionRefresh(input: {
    authUserId: string;
    providerSubject: string;
  }): Promise<SessionRefreshAdmission | null>;
  syncGithubIdentity(input: {
    authUserId: string;
    providerSubject: string;
    githubLogin: string | null;
    avatarUrl: string | null;
  }): Promise<PlatformAccount>;
  syncGithubIdentityForMutation(input: {
    authUserId: string;
    providerSubject: string;
    githubLogin: string | null;
    avatarUrl: string | null;
  }): Promise<PlatformAccount | null>;
  setPublicProfile(authUserId: string, enabled: boolean): Promise<boolean>;
  beginAccountErasure(authUserId: string): Promise<boolean>;
  /** Private account-scoped discovery; it never contains Membership authority. */
  listMyGames?(input: {
    accountId: string;
    beforeAt: string | null;
    beforeLaunchId: string | null;
    limit: number;
  }): Promise<MyGamesIndex | null>;
  resolveOwnedRunMembership(input: {
    accountId: string;
    runId: string;
    entrySelector: string;
  }): Promise<OwnedRunMembershipCorrespondence | null>;
}

export interface BffDependencies {
  /** A fresh publishable-key Auth client for each request. */
  readonly authClient: () => PlatformAuthClient;
  /** A standalone server-secret data client with no user session installed. */
  readonly dataClient: PlatformDataClient;
  /** Server-secret reads that emit only the reviewed public Run DTO. */
  readonly publicRunData?: PublicRunData;
  /** Exact deployment-owned gateway origin for participant and public streams. */
  readonly hostedPublicStreamBaseUrl?: string;
  /** Fixed service-only client for the Fly hosted browser-session boundary. */
  readonly hostedBrowserSessions?: HostedBrowserSessionClient;
  /** Server-secret Supabase formation RPCs; never installed in browser code. */
  readonly hostedFormationData?: HostedFormationData;
  /** Fixed service-only Fly launch boundary. */
  readonly hostedFormationGateway?: HostedFormationGateway;
  /** Exact reviewed Host installation selected by deployment configuration. */
  readonly hostedFormationHostInstallationId?: string;
}

export interface PlatformBffConfig {
  readonly canonicalOrigin: string;
  readonly allowedReturnTargets: readonly string[];
  readonly sessionKey: Uint8Array;
  readonly oauthKey: Uint8Array;
}

export interface DevelopmentPlatformIdentity {
  readonly authUserId: string;
  readonly providerSubject: string;
  readonly githubLogin: string;
  readonly avatarUrl: string | null;
}

export interface DevelopmentPlatformBffConfig extends PlatformBffConfig {
  readonly developmentMode: typeof DEVELOPMENT_IDENTITY_MODE;
  readonly deploymentEnvironment: "development";
  readonly identity: DevelopmentPlatformIdentity;
}

interface SessionPayload {
  readonly schema: "worldstream/platform-session/v1";
  readonly authUserId: string;
  readonly accessToken: string;
  readonly refreshToken: string;
  readonly expiresAt: number;
  readonly csrf: string;
  readonly github: AuthIdentity;
}

interface AcceptedMutation {
  readonly payload: SessionPayload;
  readonly body: Record<string, unknown>;
}

interface VerifiedMutation extends AcceptedMutation {
  readonly account: PlatformAccount;
}

export interface PlatformBff {
  fetch(request: Request): Promise<Response>;
}

/** The upstream credential or its bound identity is no longer acceptable. */
export class PlatformCredentialRejectedError extends Error {
  constructor() {
    super("platform_credential_rejected");
    this.name = "PlatformCredentialRejectedError";
  }
}

/** A required hosted dependency could not answer reliably. */
export class PlatformDependencyUnavailableError extends Error {
  constructor() {
    super("platform_dependency_unavailable");
    this.name = "PlatformDependencyUnavailableError";
  }
}

/** A durable admission boundary rejected an otherwise valid operation. */
export class PlatformRateLimitExceededError extends Error {
  constructor() {
    super("platform_rate_limit_exceeded");
    this.name = "PlatformRateLimitExceededError";
  }
}

/** A reviewed platform capacity gate is currently holding a safe reservation. */
export class PlatformActivityCapacityUnavailableError extends Error {
  constructor(
    readonly scope: "account" | "platform",
  ) {
    super("platform_activity_capacity_unavailable");
    this.name = "PlatformActivityCapacityUnavailableError";
  }
}

/** Builds the fixed Vercel BFF route graph. */
export function createPlatformBff(
  config: PlatformBffConfig,
  dependencies: BffDependencies,
): PlatformBff {
  const origin = validateOrigin(config.canonicalOrigin);
  const allowedReturnTargets = validateReturnTargets(config.allowedReturnTargets);
  const sessionKey = validateKey(config.sessionKey);
  const oauthKey = validateKey(config.oauthKey);
  const formation = formationCoordinator(dependencies);
  const hostedPublicStreamBaseUrl = dependencies.hostedPublicStreamBaseUrl === undefined
    ? null
    : publicProjectionStreamBaseUrl(dependencies.hostedPublicStreamBaseUrl);
  const browserStreamUrl = hostedPublicStreamBaseUrl === null
    ? null
    : `${hostedPublicStreamBaseUrl.replace(/^http/u, "ws")}/v1/hosted/browser-stream`;
  return {
    async fetch(request: Request): Promise<Response> {
      const url = new URL(request.url);
      if (url.origin !== origin) return safeJson(404, "route_not_found");
      if (request.method === "GET" && url.pathname === "/api/catalog") {
        return publicCatalog(formation !== null && dependencies.hostedBrowserSessions !== undefined);
      }
      const publicRunId = publicRunRoute(url.pathname);
      if (request.method === "GET" && publicRunId !== null) {
        return readPublicRun(
          publicRunId,
          dependencies.publicRunData,
          hostedPublicStreamBaseUrl,
          origin,
        );
      }
      if (
        request.method === "GET" &&
        url.pathname === "/api/results/agent-heist/recent"
      ) {
        return readRecentAgentHeistResults(dependencies.publicRunData);
      }
      if (request.method === "GET" && url.pathname === "/api/auth/github/start") {
        return startGithub(
          request,
          url,
          origin,
          allowedReturnTargets,
          oauthKey,
          dependencies,
        );
      }
      if (request.method === "GET" && url.pathname === "/api/auth/github/callback") {
        return finishGithub(
          request,
          url,
          origin,
          allowedReturnTargets,
          sessionKey,
          oauthKey,
          dependencies,
        );
      }
      if (request.method === "GET" && url.pathname === "/api/auth/session") {
        return readSession(request, origin, sessionKey, dependencies, browserStreamUrl);
      }
      if (request.method === "GET" && url.pathname === "/api/my-games") {
        return readMyGames(request, url, origin, sessionKey, dependencies);
      }
      if (request.method === "POST" && url.pathname === "/api/auth/session/refresh") {
        return refreshSession(request, origin, sessionKey, dependencies);
      }
      if (request.method === "POST" && url.pathname === "/api/auth/sign-out") {
        return signOut(request, origin, sessionKey, dependencies);
      }
      if (request.method === "POST" && url.pathname === "/api/account/public-profile") {
        return setPublicProfile(request, origin, sessionKey, dependencies);
      }
      if (request.method === "POST" && url.pathname === "/api/account/erasure") {
        return beginErasure(request, origin, sessionKey, dependencies);
      }
      if (request.method === "POST" && url.pathname === "/api/runs/enter") {
        return enterRun(request, origin, sessionKey, dependencies);
      }
      if (request.method === "POST" && url.pathname === "/api/launches") {
        return createHostedLaunch(request, origin, sessionKey, dependencies, formation);
      }
      if (request.method === "POST" && url.pathname === "/api/invitations/claim") {
        return claimHostedInvitation(request, origin, sessionKey, dependencies, formation);
      }
      const launchRoute = hostedLaunchRoute(url.pathname);
      if (launchRoute !== null) {
        if (request.method === "GET" && launchRoute.action === "read") {
          return readHostedLaunch(request, origin, sessionKey, dependencies, formation, launchRoute.launchId);
        }
        if (request.method === "POST" && launchRoute.action !== "read") {
          return mutateHostedLaunch(
            request,
            origin,
            sessionKey,
            dependencies,
            formation,
            launchRoute,
          );
        }
      }
      if (
        request.method === "POST" &&
        url.pathname === "/api/v1/participant-console/handoffs:redeem"
      ) {
        return redeemHostedHandoff(request, origin, sessionKey, dependencies);
      }
      if (
        request.method === "GET" &&
        url.pathname === "/api/v1/participant-console/session"
      ) {
        return hostedSessionStatus(request, origin, dependencies);
      }
      if (
        request.method === "POST" &&
        url.pathname === "/api/v1/participant-console/session:stream-ticket"
      ) {
        return issueHostedStreamTicket(request, origin, sessionKey, dependencies);
      }
      if (
        request.method === "POST" &&
        url.pathname === "/api/v1/participant-console/session:logout"
      ) {
        return logoutHostedSession(request, origin, sessionKey, dependencies);
      }
      return safeJson(404, "route_not_found");
    },
  };
}

function publicRunRoute(pathname: string): string | null {
  return pathname.match(/^\/api\/runs\/([0-9a-f]{32})$/u)?.[1] ?? null;
}

async function readPublicRun(
  publicId: string,
  data: PublicRunData | undefined,
  hostedPublicStreamBaseUrl: string | null,
  origin: string,
): Promise<Response> {
  if (data === undefined) return temporarilyUnavailable();
  try {
    const record = await data.readPublicRun(publicId);
    const presented = presentPublicRun(record, hostedPublicStreamBaseUrl);
    const reviewed = record.state === "live"
      ? reviewedActivityByDigest(record.activity.listing_revision)
      : null;
    const viewerClientPath = reviewed === null
      ? null
      : reviewedPublicViewerClientPath(reviewed.public);
    return publicNoStoreJson(
      200,
      presentPublicViewerClient(presented, origin, viewerClientPath),
    );
  } catch {
    return temporarilyUnavailable();
  }
}

async function readRecentAgentHeistResults(
  data: PublicRunData | undefined,
): Promise<Response> {
  if (data === undefined) return temporarilyUnavailable();
  try {
    return publicNoStoreJson(200, await data.listRecentResults(20));
  } catch {
    return temporarilyUnavailable();
  }
}

type HostedLaunchRoute =
  | { readonly launchId: string; readonly action: "read" }
  | { readonly launchId: string; readonly action: "start" }
  | { readonly launchId: string; readonly action: "cancel" }
  | { readonly launchId: string; readonly action: "invite" | "release" | "reset"; readonly seatId: string };

function formationCoordinator(dependencies: BffDependencies): HostedFormationCoordinator | null {
  const configured = [
    dependencies.hostedFormationData,
    dependencies.hostedFormationGateway,
    dependencies.hostedFormationHostInstallationId,
  ].filter((value) => value !== undefined).length;
  if (configured === 0) return null;
  if (configured !== 3) throw new Error("hosted_formation_configuration_incomplete");
  return new HostedFormationCoordinator(
    dependencies.hostedFormationData!,
    dependencies.hostedFormationGateway!,
    dependencies.hostedFormationHostInstallationId!,
  );
}

function publicCatalog(dependenciesAvailable: boolean): Response {
  const response = Response.json({
    version: "hosted_activity_catalog.v1",
    activities: listPublicHostedActivities(dependenciesAvailable),
  });
  response.headers.set("cache-control", "public, max-age=30, stale-while-revalidate=60");
  return response;
}

function hostedLaunchRoute(pathname: string): HostedLaunchRoute | null {
  const read = pathname.match(/^\/api\/launches\/([0-9a-f-]+)$/u);
  if (read?.[1] !== undefined && validLaunchIdentifier(read[1])) {
    return { launchId: read[1], action: "read" };
  }
  const direct = pathname.match(/^\/api\/launches\/([0-9a-f-]+)\/(start|cancel)$/u);
  if (direct?.[1] !== undefined && direct[2] !== undefined && validLaunchIdentifier(direct[1])) {
    return { launchId: direct[1], action: direct[2] as "start" | "cancel" };
  }
  const seat = pathname.match(
    /^\/api\/launches\/([0-9a-f-]+)\/seats\/([A-Za-z0-9._-]+)\/(invitation|release|reset)$/u,
  );
  if (
    seat?.[1] !== undefined &&
    seat[2] !== undefined &&
    seat[3] !== undefined &&
    validLaunchIdentifier(seat[1]) &&
    seat[2].length <= 128
  ) {
    return {
      launchId: seat[1],
      seatId: seat[2],
      action: seat[3] === "invitation" ? "invite" : seat[3] as "release" | "reset",
    };
  }
  return null;
}

async function createHostedLaunch(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
  formation: HostedFormationCoordinator | null,
): Promise<Response> {
  const admitted = await verifiedMutation(request, origin, sessionKey, dependencies);
  if (admitted instanceof Response) return admitted;
  const data = dependencies.hostedFormationData;
  if (formation === null || data === undefined) return temporarilyUnavailable();
  if (
    !isExactObject(admitted.body, [
      "listing_slug",
      "creator_access",
      "creator_seat",
      "fill_mode",
      "idempotency_key",
    ]) ||
    typeof admitted.body.listing_slug !== "string" ||
    typeof admitted.body.creator_access !== "string" ||
    (admitted.body.creator_seat !== null && typeof admitted.body.creator_seat !== "string") ||
    typeof admitted.body.fill_mode !== "string" ||
    typeof admitted.body.idempotency_key !== "string" ||
    !/^[A-Za-z0-9_-]{32,128}$/u.test(admitted.body.idempotency_key)
  ) {
    return privateError(400, "invalid_request");
  }
  const reviewed = reviewedActivityBySlug(admitted.body.listing_slug);
  if (reviewed === null) return privateError(409, "activity_unavailable");
  const creatorAccess = admitted.body.creator_access;
  if (creatorAccess !== "seat" && creatorAccess !== "spectator") {
    return privateError(400, "invalid_request");
  }
  const seatKey = admitted.body.creator_seat;
  const seatId = typeof seatKey === "string" ? reviewedSeatId(reviewed, seatKey) : null;
  if (
    (creatorAccess === "seat" &&
      seatId === null) ||
    (creatorAccess === "spectator" &&
      (seatKey !== null || reviewed.listing.value.creator_access !== "may_spectate"))
  ) {
    return privateError(400, "invalid_request");
  }
  const houseFillChoice = admitted.body.fill_mode === "house_agents"
    ? "fill_unclaimed"
    : admitted.body.fill_mode === "people_only"
      ? "disabled"
      : null;
  if (houseFillChoice === null) return privateError(400, "invalid_request");
  const launchInput = encodeCanonical({});
  try {
    const created = await data.createLaunchRequest({
      accountId: admitted.account.accountId,
      listingRevisionDigest: reviewed.listing.digest,
      idempotencyNamespace: "hosted-shell-v1",
      idempotencyKeyDigest: formationSha256(admitted.body.idempotency_key),
      canonicalLaunchInput: launchInput,
      launchInputDigest: formationSha256(launchInput),
      houseFillChoice,
      creatorAccessChoice: creatorAccess,
      creatorSeatId: seatId,
    });
    const snapshot = await launchSnapshot(data, admitted.account.accountId, created.launchRequestId);
    if (snapshot === null) return temporarilyUnavailable();
    return privateJson(created.wasCreated ? 201 : 200, snapshot);
  } catch (error) {
    return formationError(error);
  }
}

async function readHostedLaunch(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
  formation: HostedFormationCoordinator | null,
  launchId: string,
): Promise<Response> {
  if (formation === null || dependencies.hostedFormationData === undefined) {
    return temporarilyUnavailable();
  }
  const admitted = await verifiedRead(request, origin, sessionKey, dependencies);
  if (admitted instanceof Response) return admitted;
  try {
    const launch = await dependencies.hostedFormationData.readLaunchRequest(admitted.account.accountId, launchId);
    if (launch === null) return privateError(404, "launch_unavailable");
    if (launch.canManage) {
      try {
        await formation.recover(launchId);
      } catch (error) {
        // A read retains the exact Launch Request when the Host is temporarily
        // unavailable. The snapshot below presents reconciliation and keeps
        // entry disabled; it never authorizes a replacement Room.
        if (!(error instanceof HostedFormationUnavailableError)) throw error;
      }
    }
    const snapshot = await launchSnapshot(
      dependencies.hostedFormationData,
      admitted.account.accountId,
      launchId,
      formation,
    );
    return snapshot === null
      ? privateError(404, "launch_unavailable")
      : privateJson(200, snapshot);
  } catch (error) {
    return formationError(error);
  }
}

async function readMyGames(
  request: Request,
  url: URL,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
): Promise<Response> {
  const admitted = await verifiedRead(request, origin, sessionKey, dependencies);
  if (admitted instanceof Response) return admitted;
  if (dependencies.dataClient.listMyGames === undefined) return temporarilyUnavailable();
  const limitValue = url.searchParams.get("limit");
  const beforeAt = url.searchParams.get("before_at");
  const beforeLaunchId = url.searchParams.get("before_launch_id");
  const limit = limitValue === null ? 20 : Number(limitValue);
  if (
    !Number.isInteger(limit) || limit < 1 || limit > 50 ||
    ((beforeAt === null) !== (beforeLaunchId === null)) ||
    (beforeAt !== null && Number.isNaN(Date.parse(beforeAt))) ||
    (beforeLaunchId !== null && !UUID_PATTERN.test(beforeLaunchId))
  ) return privateError(400, "invalid_request");
  try {
    const index = await dependencies.dataClient.listMyGames({
      accountId: admitted.account.accountId,
      beforeAt,
      beforeLaunchId,
      limit,
    });
    return index === null ? clearSessionError(401, "session_invalid") : privateJson(200, presentMyGames(index));
  } catch (error) {
    reportPlatformFailure("history_read", error);
    return credentialWasRejected(error) ? clearSessionError(401, "session_invalid") : temporarilyUnavailable();
  }
}

function presentMyGames(index: MyGamesIndex): Record<string, unknown> {
  return {
    version: index.version,
    items: index.items.map((item) => ({
      launch_id: item.launchId,
      title: item.title,
      state: item.state,
      updated_at: item.updatedAt,
      participation: item.participation,
      action: item.action,
      ...(item.resultPublicId === null ? {} : { result_public_id: item.resultPublicId }),
    })),
    next: index.next === null ? null : {
      before_at: index.next.beforeAt,
      before_launch_id: index.next.beforeLaunchId,
    },
  };
}

async function claimHostedInvitation(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
  formation: HostedFormationCoordinator | null,
): Promise<Response> {
  const admitted = await verifiedMutation(request, origin, sessionKey, dependencies);
  if (admitted instanceof Response) return admitted;
  const data = dependencies.hostedFormationData;
  if (formation === null || data === undefined) return temporarilyUnavailable();
  if (
    !isExactObject(admitted.body, ["invitation_token", "participation"]) ||
    typeof admitted.body.invitation_token !== "string" ||
    !/^[0-9a-f]{64}$/u.test(admitted.body.invitation_token) ||
    (admitted.body.participation !== "human" && admitted.body.participation !== "external_agent")
  ) {
    return privateError(400, "invalid_request");
  }
  const participation: AccountParticipation = admitted.body.participation === "human"
    ? "account_human"
    : "account_external_agent";
  try {
    const claimed = await data.claimInvitedSeat(
      admitted.account.accountId,
      formationSha256(admitted.body.invitation_token),
      participation,
    );
    if (claimed === null) return privateError(409, "invitation_unavailable");
    const snapshot = await launchSnapshot(data, admitted.account.accountId, claimed.launchRequestId);
    return snapshot === null
      ? temporarilyUnavailable()
      : privateJson(201, snapshot);
  } catch (error) {
    return formationError(error, "invitation_unavailable");
  }
}

async function mutateHostedLaunch(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
  formation: HostedFormationCoordinator | null,
  route: Exclude<HostedLaunchRoute, { readonly action: "read" }>,
): Promise<Response> {
  const admitted = await verifiedMutation(request, origin, sessionKey, dependencies);
  if (admitted instanceof Response) return admitted;
  const data = dependencies.hostedFormationData;
  if (data === undefined) return temporarilyUnavailable();
  if (!isExactObject(admitted.body, [])) return privateError(400, "invalid_request");
  try {
    if (route.action === "start") {
      if (formation === null) return temporarilyUnavailable();
      const launch = await data.readLaunchRequest(admitted.account.accountId, route.launchId);
      // The abandonment receipt is terminal for the generic start flow.  A
      // retained pre-start Run must remain inspectable, but cannot be resumed
      // by retrying the start endpoint or by issuing another Host operation.
      if (launch === null || launch.state === "abandoned_prestart") {
        return privateError(409, "launch_unavailable");
      }
      if (launch !== null && reviewedActivityByDigest(launch.listingRevisionDigest)?.public.clientPath === null) {
        return privateError(409, "launch_client_unavailable");
      }
      const advanced = await formation.advance(admitted.account.accountId, route.launchId);
      const snapshot = await launchSnapshot(data, admitted.account.accountId, route.launchId, formation);
      if (snapshot === null) return temporarilyUnavailable();
      return privateJson(advanced.state === "run_created" ? 200 : 202, {
        ...snapshot,
        retry_after_seconds: advanced.retryAfterSeconds,
      });
    }
    if (route.action === "cancel") {
      const launch = await data.readLaunchRequest(admitted.account.accountId, route.launchId);
      if (launch === null || !launch.canManage) return privateError(409, "launch_unavailable");
      // Before the Host receives a frozen operation, cancellation is a normal
      // creator-owned platform mutation. Once Genesis exists, the same UI
      // action becomes a distinct, evidence-bound Host abandonment operation.
      if (launch.state === "collecting_roster") {
        return (await data.cancelLaunchRequest(admitted.account.accountId, route.launchId))
          ? privateJson(200, { version: "hosted_launch_cancelled.v1", cancelled: true })
          : privateError(409, "launch_unavailable");
      }
      if (launch.state === "provisioning") {
        // The ordinary platform cancellation RPC is valid during provisioning
        // only while the Host has not started its exact retained mutation.
        // Once that boundary is crossed, keep the launch closed and require
        // the Host-authorized abandonment lane instead of racing the RPC.
        const material = await data.readHostedLaunchMaterial(
          admitted.account.accountId,
          route.launchId,
        );
        if (material === null || material.hostMutationStarted) {
          return privateError(409, "launch_unavailable");
        }
        return (await data.cancelLaunchRequest(admitted.account.accountId, route.launchId))
          ? privateJson(200, { version: "hosted_launch_cancelled.v1", cancelled: true })
          : privateError(409, "launch_unavailable");
      }
      if (launch.state !== "run_created" || formation === null) {
        return privateError(409, "launch_unavailable");
      }
      await formation.abandonPrestart(route.launchId);
      return privateJson(200, {
        version: "hosted_launch_abandoned_prestart.v1",
        abandoned: true,
      });
    }
    if (formation === null) return temporarilyUnavailable();
    const launch = await data.readLaunchRequest(
      admitted.account.accountId,
      route.launchId,
    );
    const reviewed = launch === null
      ? null
      : reviewedActivityByDigest(launch.listingRevisionDigest);
    const seatId = reviewed === null ? null : reviewedSeatId(reviewed, route.seatId);
    if (seatId === null) return privateError(409, "seat_unavailable");
    if (route.action === "invite") {
      const invitation = await data.rotateSeatInvitation(
        admitted.account.accountId,
        route.launchId,
        seatId,
      );
      return invitation === null
        ? privateError(409, "seat_unavailable")
        : privateJson(201, {
            version: "hosted_seat_invitation.v1",
            invitation_token: invitation.token,
            expires_at: invitation.expiresAt,
          });
    }
    const changed = route.action === "release"
      ? await data.releaseSeatClaim(admitted.account.accountId, route.launchId, seatId)
      : await data.resetSeatClaim(admitted.account.accountId, route.launchId, seatId);
    if (!changed) return privateError(409, "seat_unavailable");
    const snapshot = await launchSnapshot(data, admitted.account.accountId, route.launchId);
    return snapshot === null ? temporarilyUnavailable() : privateJson(200, snapshot);
  } catch (error) {
    return formationError(error);
  }
}

async function launchSnapshot(
  data: HostedFormationData,
  accountId: string,
  launchId: string,
  formation?: HostedFormationCoordinator,
): Promise<Record<string, unknown> | null> {
  const [launch, house] = await Promise.all([
    data.readLaunchRequest(accountId, launchId),
    data.readHouseFill(accountId, launchId),
  ]);
  if (launch === null) return null;
  const reconciliation = launch.state === "collecting_roster"
    ? null
    : await data.readGenesisReconciliation(launchId);
  let run = reconciliation?.runId === null || reconciliation?.runId === undefined
    ? null
    : await data.readOwnedRun(accountId, reconciliation.runId);
  const entryReady = run !== null && formation !== undefined && await formation.entryReady(launchId);
  if (run !== null && !entryReady) {
    run = { ...run, canEnter: false, memberships: [] };
  }
  return safeLaunchProjection(launch, house, reconciliation?.needsGenesisPull === true, run, entryReady);
}

function safeLaunchProjection(
  launch: FormationLaunchRecord,
  house: HouseFillRecord | null,
  reconciling: boolean,
  run: OwnedRunRecord | null,
  entryReady: boolean,
): Record<string, unknown> {
  const reviewed = reviewedActivityByDigest(launch.listingRevisionDigest);
  const state = launch.state === "collecting_roster"
    ? "collecting"
    : launch.state === "run_created"
      ? run?.canEnter ? "run_created" : "reconciling"
      : ["cancelled", "expired", "failed_pre_genesis", "abandoned_prestart"].includes(launch.state)
        ? launch.state
        : reconciling
          ? "reconciling"
          : "provisioning";
  return {
    version: "hosted_launch.v1",
    launch_id: launch.launchRequestId,
    activity_slug: reviewed?.slug ?? "unavailable",
    activity_title: reviewed?.public.title ?? "Unavailable activity",
    state,
    expires_at: launch.expiresAt,
    can_manage: launch.canManage,
    fill_mode: launch.houseFillChoice === "fill_unclaimed" ? "house_agents" : "people_only",
    house_fill: house === null
      ? null
      : {
          state: house.state,
          claim_window_closes_at: house.claimWindowClosesAt,
          failure_code: house.failureCode,
        },
    recovery_state: ["collecting_roster", "cancelled", "expired", "failed_pre_genesis", "abandoned_prestart"].includes(launch.state)
      ? "not_started"
      : run === null
        ? "genesis_not_proven"
        : entryReady
          ? "entry_ready"
          : "genesis_recorded_repairing",
    seats: launch.seats.map((seat) => {
      const houseAssignment = house?.assignments.find(
        ({ seatId }) => seatId === seat.seatId,
      );
      return {
        seat_key: reviewed === null
          ? "unavailable"
          : reviewedSeatKey(reviewed, seat.seatId) ?? "unavailable",
        label: seat.displayName,
        required: seat.required,
        status: houseAssignment !== undefined
          ? "house"
          : seat.claimedByRequester
            ? "yours"
            : seat.claimed
              ? "claimed"
              : "open",
        participation: houseAssignment !== undefined
          ? "house_agent"
          : seat.participationKind === "account_external_agent"
            ? "external_agent"
            : seat.participationKind === "account_human"
              ? "human"
              : null,
        ...(houseAssignment === undefined
          ? {}
          : { house_display_name: houseAssignment.displayName }),
      };
    }),
    run: run === null
      ? null
      : {
          run_id: run.runId,
          public_id: run.publicId,
          can_enter: run.canEnter,
          entries: run.memberships.map((membership) => ({
            label: membership.purpose === "creator_spectator"
              ? "Spectator view"
              : launch.seats.find(({ seatId }) => seatId === membership.seatId)?.displayName ?? "Participant view",
            entry_selector: membership.entrySelector,
          })),
        },
  };
}

async function verifiedRead(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
): Promise<{ readonly payload: SessionPayload; readonly account: PlatformAccount } | Response> {
  if (!readRequestIsSafe(request, origin)) return privateError(403, "request_rejected");
  const payload = readSessionCookie(request, sessionKey);
  if (payload === null || payload.expiresAt <= Math.floor(Date.now() / 1_000)) {
    return payload === null ? clearSessionError(401, "session_required") : refreshRequired(payload);
  }
  try {
    const claims = await dependencies.authClient().getClaims(payload.accessToken);
    if (claims.subject !== payload.authUserId) return clearSessionError(401, "session_invalid");
    const account = await dependencies.dataClient.resolveGithubAccount({
      authUserId: payload.authUserId,
      providerSubject: payload.github.subject,
    });
    return account === null
      ? clearSessionError(401, "session_invalid")
      : { payload, account };
  } catch (error) {
    reportPlatformFailure("account_verification", error);
    return credentialWasRejected(error) ? refreshRequired(payload) : temporarilyUnavailable();
  }
}

function formationError(error: unknown, rejectedCode = "formation_unavailable"): Response {
  if (
    error instanceof HostedFormationRejectedError ||
    error instanceof PlatformCredentialRejectedError
  ) {
    return privateError(409, rejectedCode);
  }
  if (error instanceof PlatformActivityCapacityUnavailableError) {
    return privateError(409, "activity_capacity_unavailable");
  }
  reportPlatformFailure("formation", error);
  if (
    error instanceof HostedFormationUnavailableError ||
    error instanceof PlatformDependencyUnavailableError
  ) {
    return temporarilyUnavailable();
  }
  return temporarilyUnavailable();
}

/**
 * Adds one conspicuous loopback-only identity substitute around the production
 * BFF. Supabase remains authoritative for the Platform Account; only the
 * external GitHub OAuth exchange is replaced.
 */
export function createDevelopmentPlatformBff(
  config: DevelopmentPlatformBffConfig,
  dataClient: PlatformDataClient,
  hostedBrowserSessions?: HostedBrowserSessionClient,
  hostedFormation?: {
    readonly data: HostedFormationData;
    readonly gateway: HostedFormationGateway;
    readonly hostInstallationId: string;
  },
  hostedPublicStreamBaseUrl?: string,
): PlatformBff {
  const origin = validateDevelopmentConfiguration(config);
  const user = developmentUser(config.identity);
  const auth = new DevelopmentPlatformAuthClient(user);
  const inner = createPlatformBff(config, {
    authClient: () => auth,
    dataClient,
    ...(supportsPublicRuns(dataClient) ? { publicRunData: dataClient } : {}),
    ...(hostedPublicStreamBaseUrl === undefined ? {} : { hostedPublicStreamBaseUrl }),
    ...(hostedBrowserSessions === undefined ? {} : { hostedBrowserSessions }),
    ...(hostedFormation === undefined
      ? {}
      : {
          hostedFormationData: hostedFormation.data,
          hostedFormationGateway: hostedFormation.gateway,
          hostedFormationHostInstallationId: hostedFormation.hostInstallationId,
        }),
  });

  return {
    async fetch(request: Request): Promise<Response> {
      const url = new URL(request.url);
      let response: Response;
      if (url.origin !== origin) {
        response = safeJson(404, "route_not_found");
      } else if (request.method === "GET" && url.pathname === "/api/dev/status") {
        response = readRequestIsSafe(request, origin)
          ? privateJson(200, {
              version: "platform_development_substitutes.v1",
              identity_bypass: DEVELOPMENT_IDENTITY_MODE,
              warning: "Development substitute. Never enable in production.",
            })
          : privateError(403, "request_rejected");
      } else if (request.method === "POST" && url.pathname === "/api/dev/sign-in") {
        response = await developmentSignIn(
          request,
          origin,
          validateKey(config.sessionKey),
          user,
          dataClient,
        );
      } else if (url.pathname.startsWith("/api/auth/github/")) {
        response = privateError(409, "development_identity_bypass_active");
      } else {
        response = await inner.fetch(request);
      }
      response.headers.set("x-worldstream-development-substitute", "identity-bypass");
      return response;
    },
  };
}

function supportsPublicRuns(
  dataClient: PlatformDataClient,
): dataClient is PlatformDataClient & PublicRunData {
  const candidate = dataClient as PlatformDataClient & Partial<PublicRunData>;
  return typeof candidate.readPublicRun === "function" &&
    typeof candidate.listRecentResults === "function";
}

async function developmentSignIn(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  user: AuthUser,
  dataClient: PlatformDataClient,
): Promise<Response> {
  if (!mutationHeadersAreSafe(request, origin)) return privateError(403, "request_rejected");
  const body = await boundedJson(request);
  if (
    body === null ||
    Object.keys(body).length !== 1 ||
    body.mode !== DEVELOPMENT_IDENTITY_MODE
  ) {
    return privateError(400, "development_acknowledgement_required");
  }
  const github = exactGithubIdentity(user);
  try {
    await dataClient.syncGithubIdentity({
      authUserId: user.id,
      providerSubject: github.subject,
      githubLogin: github.login,
      avatarUrl: github.avatarUrl,
    });
    const payload = newSessionPayload(developmentSession(), user.id, github);
    const response = privateJson(200, {
      version: "platform_development_session.v1",
      authenticated: true,
      csrf: payload.csrf,
      identity: { github_login: github.login },
      warning: "Development substitute. Never enable in production.",
    });
    response.headers.append(
      "set-cookie",
      sessionCookie(sealJson(sessionKey, "platform-session-v1", payload), SESSION_MAX_AGE),
    );
    return response;
  } catch {
    return temporarilyUnavailable();
  }
}

function validateDevelopmentConfiguration(config: DevelopmentPlatformBffConfig): string {
  const origin = validateOrigin(config.canonicalOrigin);
  const url = new URL(origin);
  if (
    config.developmentMode !== DEVELOPMENT_IDENTITY_MODE ||
    config.deploymentEnvironment !== "development" ||
    url.protocol !== "http:" ||
    !["127.0.0.1", "localhost", "[::1]"].includes(url.hostname) ||
    process.env.NODE_ENV === "production" ||
    process.env.VERCEL_ENV === "production"
  ) {
    throw new Error("development_identity_bypass_forbidden");
  }
  return origin;
}

function developmentUser(identity: DevelopmentPlatformIdentity): AuthUser {
  if (
    !safeSubject(identity.authUserId) ||
    !safeSubject(identity.providerSubject) ||
    identity.githubLogin.length < 1 ||
    identity.githubLogin.length > 128 ||
    /[\u0000-\u001f\u007f]/u.test(identity.githubLogin) ||
    (identity.avatarUrl !== null &&
      (identity.avatarUrl.length > 2048 || !identity.avatarUrl.startsWith("https://")))
  ) {
    throw new Error("invalid_development_identity");
  }
  return {
    id: identity.authUserId,
    identities: [
      {
        provider: "github",
        subject: identity.providerSubject,
        login: identity.githubLogin,
        avatarUrl: identity.avatarUrl,
      },
    ],
  };
}

function developmentSession(): AuthSession {
  return {
    accessToken: "worldstream-development-access-token",
    refreshToken: "worldstream-development-refresh-token",
    expiresAt: Math.floor(Date.now() / 1000) + SESSION_MAX_AGE,
  };
}

class DevelopmentPlatformAuthClient implements PlatformAuthClient {
  readonly #user: AuthUser;

  constructor(user: AuthUser) {
    this.#user = user;
  }

  githubAuthorizeUrl(): string {
    throw new PlatformCredentialRejectedError();
  }

  async exchangeCodeForSession(): Promise<AuthSession> {
    throw new PlatformCredentialRejectedError();
  }

  async getClaims(accessToken: string): Promise<{ subject: string }> {
    this.#requireAccess(accessToken);
    return { subject: this.#user.id };
  }

  async getUser(accessToken: string): Promise<AuthUser> {
    this.#requireAccess(accessToken);
    return this.#user;
  }

  async refreshSession(refreshToken: string): Promise<RefreshedAuthSession> {
    if (refreshToken !== "worldstream-development-refresh-token") {
      throw new PlatformCredentialRejectedError();
    }
    return { ...developmentSession(), user: this.#user };
  }

  async signOut(accessToken: string): Promise<void> {
    this.#requireAccess(accessToken);
  }

  #requireAccess(accessToken: string): void {
    if (accessToken !== "worldstream-development-access-token") {
      throw new PlatformCredentialRejectedError();
    }
  }
}

/**
 * Builds the Vercel entry boundary. Vercel supplies and overwrites its proxy
 * headers before a Function receives the request; this adapter validates that
 * fixed envelope and removes it before the deployment-neutral BFF runs.
 */
export function createVercelPlatformBff(
  config: PlatformBffConfig,
  dependencies: BffDependencies,
): PlatformBff {
  const origin = validateOrigin(config.canonicalOrigin);
  const inner = createPlatformBff(config, dependencies);
  return {
    async fetch(request: Request): Promise<Response> {
      const normalized = normalizeVercelRequest(request, origin);
      if (normalized === null) return safeJson(403, "request_rejected");
      return inner.fetch(normalized);
    },
  };
}

async function startGithub(
  request: Request,
  url: URL,
  origin: string,
  allowedReturnTargets: ReadonlySet<string>,
  oauthKey: Buffer,
  dependencies: BffDependencies,
): Promise<Response> {
  if (!readRequestIsSafe(request, origin)) return safeJson(403, "request_rejected");
  const returnTarget = url.searchParams.get("return_to") ?? "/";
  if (!allowedReturnTargets.has(returnTarget)) return safeJson(400, "invalid_return_target");

  try {
    const state = randomValue();
    const browserNonce = randomValue();
    const verifier = randomValue();
    const codeChallenge = base64Url(sha256(verifier));
    const callback = new URL("/api/auth/github/callback", origin);
    callback.searchParams.set("state", state);
    callback.searchParams.set("return_to", returnTarget);
    if (
      !(await dependencies.dataClient.beginGithubOAuth({
        stateDigest: bytea(sha256(state)),
        encryptedPkceVerifier: bytea(seal(oauthKey, "oauth-pkce-v1", Buffer.from(verifier))),
        browserNonceDigest: bytea(sha256(browserNonce)),
        returnTarget,
      }))
    ) {
      return safeJson(429, "rate_limited");
    }
    const location = dependencies.authClient().githubAuthorizeUrl({
      callbackUrl: callback.toString(),
      codeChallenge,
    });
    const response = redirect(location);
    response.headers.append("set-cookie", oauthCookie(browserNonce, OAUTH_MAX_AGE));
    response.headers.set("cache-control", "private, no-store, max-age=0");
    return response;
  } catch {
    return temporarilyUnavailable();
  }
}

async function finishGithub(
  request: Request,
  url: URL,
  origin: string,
  allowedReturnTargets: ReadonlySet<string>,
  sessionKey: Buffer,
  oauthKey: Buffer,
  dependencies: BffDependencies,
): Promise<Response> {
  const fail = (code: string, status = 400): Response => {
    const response = privateError(status, code);
    response.headers.append("set-cookie", clearCookie(OAUTH_COOKIE));
    return response;
  };
  if (!oauthCallbackRequestIsSafe(request, origin)) return fail("oauth_callback_rejected");
  const code = url.searchParams.get("code");
  const state = url.searchParams.get("state");
  const returnTarget = url.searchParams.get("return_to");
  const nonce = readCookie(request, OAUTH_COOKIE);
  if (
    code === null ||
    code.length === 0 ||
    code.length > 2048 ||
    state === null ||
    !safeRandomValue(state) ||
    nonce === null ||
    !safeRandomValue(nonce) ||
    returnTarget === null ||
    !allowedReturnTargets.has(returnTarget)
  ) {
    return fail("oauth_callback_invalid");
  }

  try {
    const attempt = await dependencies.dataClient.consumeGithubOAuth({
      stateDigest: bytea(sha256(state)),
      browserNonceDigest: bytea(sha256(nonce)),
      returnTarget,
    });
    if (attempt === null || attempt.returnTarget !== returnTarget) {
      return fail("oauth_attempt_unavailable");
    }
    let verifier: string;
    try {
      verifier = openBytea(
        oauthKey,
        "oauth-pkce-v1",
        attempt.encryptedPkceVerifier,
      ).toString();
    } catch {
      return fail("oauth_pkce_invalid");
    }
    if (!safeRandomValue(verifier)) return fail("oauth_pkce_invalid");
    const auth = dependencies.authClient();
    const authSession = await auth.exchangeCodeForSession(code, verifier);
    const user = await auth.getUser(authSession.accessToken);
    const github = exactGithubIdentity(user);
    await dependencies.dataClient.syncGithubIdentity({
      authUserId: user.id,
      providerSubject: github.subject,
      githubLogin: github.login,
      avatarUrl: github.avatarUrl,
    });
    const payload = newSessionPayload(authSession, user.id, github);
    const response = redirect(new URL(returnTarget, origin).toString());
    response.headers.append("set-cookie", clearCookie(OAUTH_COOKIE));
    response.headers.append(
      "set-cookie",
      sessionCookie(sealJson(sessionKey, "platform-session-v1", payload), SESSION_MAX_AGE),
    );
    response.headers.set("cache-control", "private, no-store, max-age=0");
    return response;
  } catch (error) {
    if (error instanceof PlatformRateLimitExceededError) return fail("rate_limited", 429);
    if (credentialWasRejected(error)) return fail("oauth_callback_invalid");
    const response = temporarilyUnavailable();
    response.headers.append("set-cookie", clearCookie(OAUTH_COOKIE));
    return response;
  }
}

async function readSession(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
  browserStreamUrl: string | null,
): Promise<Response> {
  if (!readRequestIsSafe(request, origin)) return safeJson(403, "request_rejected");
  const payload = readSessionCookie(request, sessionKey);
  if (payload === null) return privateJson(401, { error: { code: "session_required" } });
  if (payload.expiresAt <= Math.floor(Date.now() / 1000)) return refreshRequired(payload);
  try {
    const claims = await dependencies.authClient().getClaims(payload.accessToken);
    if (claims.subject !== payload.authUserId) return clearSessionError(401, "session_invalid");
    const account = await dependencies.dataClient.resolveGithubAccount({
      authUserId: payload.authUserId,
      providerSubject: payload.github.subject,
    });
    if (account === null) return clearSessionError(401, "session_invalid");
    return privateJson(200, {
      version: "platform_session.v1",
      authenticated: true,
      csrf: payload.csrf,
      public_profile_enabled: account.publicProfileEnabled,
      ...(browserStreamUrl === null ? {} : { browser_stream_url: browserStreamUrl }),
    });
  } catch (error) {
    return credentialWasRejected(error) ? refreshRequired(payload) : temporarilyUnavailable();
  }
}

async function refreshSession(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
): Promise<Response> {
  const admitted = await admitMutation(request, origin, sessionKey);
  if (admitted instanceof Response) return admitted;
  const { payload } = admitted;
  try {
    const refreshAdmission = await dependencies.dataClient.beginSessionRefresh({
      authUserId: payload.authUserId,
      providerSubject: payload.github.subject,
    });
    if (refreshAdmission === null) return clearSessionError(401, "session_invalid");
    if (!refreshAdmission.admitted) return privateError(429, "rate_limited");
    const auth = dependencies.authClient();
    const refreshed = await auth.refreshSession(payload.refreshToken);
    if (refreshed.user.id !== payload.authUserId) {
      return clearSessionError(401, "session_invalid");
    }
    const github = exactGithubIdentity(refreshed.user);
    if (github.subject !== payload.github.subject) {
      return clearSessionError(401, "session_invalid");
    }
    const next = newSessionPayload(refreshed, refreshed.user.id, github);
    const response = privateJson(200, { version: "platform_session_refreshed.v1", csrf: next.csrf });
    response.headers.append(
      "set-cookie",
      sessionCookie(sealJson(sessionKey, "platform-session-v1", next), SESSION_MAX_AGE),
    );
    return response;
  } catch (error) {
    return credentialWasRejected(error)
      ? clearSessionError(401, "session_refresh_failed")
      : temporarilyUnavailable();
  }
}

async function signOut(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
): Promise<Response> {
  const admitted = await verifiedMutation(
    request,
    origin,
    sessionKey,
    dependencies,
  );
  if (admitted instanceof Response) return admitted;
  try {
    await retireActivitySession(request, dependencies);
    await dependencies.authClient().signOut(admitted.payload.accessToken);
    const response = privateJson(200, { version: "platform_sign_out.v1" });
    response.headers.append("set-cookie", clearCookie(SESSION_COOKIE));
    response.headers.append("set-cookie", clearActivitySessionCookie());
    return response;
  } catch {
    return temporarilyUnavailable();
  }
}

async function setPublicProfile(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
): Promise<Response> {
  const admitted = await verifiedMutation(
    request,
    origin,
    sessionKey,
    dependencies,
  );
  if (admitted instanceof Response) return admitted;
  if (typeof admitted.body.enabled !== "boolean") return privateError(400, "invalid_request");
  try {
    if (
      !(await dependencies.dataClient.setPublicProfile(
        admitted.payload.authUserId,
        admitted.body.enabled,
      ))
    ) {
      return clearSessionError(401, "account_unavailable");
    }
    return privateJson(200, {
      version: "platform_public_profile.v1",
      public_profile_enabled: admitted.body.enabled,
    });
  } catch {
    return temporarilyUnavailable();
  }
}

async function beginErasure(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
): Promise<Response> {
  const admitted = await verifiedMutation(
    request,
    origin,
    sessionKey,
    dependencies,
  );
  if (admitted instanceof Response) return admitted;
  try {
    if (!(await dependencies.dataClient.beginAccountErasure(admitted.payload.authUserId))) {
      return clearSessionError(401, "account_unavailable");
    }
    const response = privateJson(202, { version: "platform_account_erasure.v1", accepted: true });
    response.headers.append("set-cookie", clearCookie(SESSION_COOKIE));
    response.headers.append("set-cookie", clearActivitySessionCookie());
    return response;
  } catch {
    return temporarilyUnavailable();
  }
}

async function enterRun(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
): Promise<Response> {
  const admitted = await verifiedMutation(request, origin, sessionKey, dependencies);
  if (admitted instanceof Response) return admitted;
  if (
    !isExactObject(admitted.body, ["run_id", "entry_selector"]) ||
    typeof admitted.body.run_id !== "string" ||
    !UUID_PATTERN.test(admitted.body.run_id) ||
    typeof admitted.body.entry_selector !== "string" ||
    !ENTRY_SELECTOR_PATTERN.test(admitted.body.entry_selector)
  ) {
    return privateError(400, "invalid_request");
  }
  const hosted = dependencies.hostedBrowserSessions;
  if (hosted === undefined) return temporarilyUnavailable();
  try {
    const binding = await dependencies.dataClient.resolveOwnedRunMembership({
      accountId: admitted.account.accountId,
      runId: admitted.body.run_id,
      entrySelector: admitted.body.entry_selector,
    });
    if (binding === null) return privateError(404, "run_entry_unavailable");
    if (reviewedActivityByDigest(binding.listingRevisionDigest)?.public.clientPath === null) {
      return privateError(409, "run_client_unavailable");
    }
    const handoff = await hosted.issueHandoff(admitted.account.accountId, binding);
    const client = new URL(handoff.clientUrl);
    if (client.origin !== origin) return temporarilyUnavailable();
    // The client receives only a fixed platform destination. The one-use
    // Membership handoff remains fragment-only and is not widened by this UI
    // convenience context.
    client.searchParams.set("platform_return", "/");
    return privateJson(201, {
      version: "platform_run_entry.v1",
      client_url: client.toString(),
    });
  } catch (error) {
    reportPlatformFailure("run_entry", error);
    return error instanceof HostedBrowserSessionRejectedError
      ? privateError(403, "run_entry_rejected")
      : temporarilyUnavailable();
  }
}

async function redeemHostedHandoff(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
): Promise<Response> {
  const admitted = await verifiedMutation(request, origin, sessionKey, dependencies);
  if (admitted instanceof Response) return admitted;
  if (!isExactObject(admitted.body, [])) return participantError(400, "participant_handoff_invalid");
  const hosted = dependencies.hostedBrowserSessions;
  if (hosted === undefined) return participantUnavailable();
  const handoff = request.headers.get("x-worldstream-participant-handoff");
  if (handoff === null || !/^wsh1:[0-9a-f]{64}$/u.test(handoff)) {
    return participantError(401, "participant_handoff_invalid");
  }
  const priorSession = readCookie(request, ACTIVITY_SESSION_COOKIE);
  try {
    const session = await hosted.redeemHandoff(
      admitted.account.accountId,
      handoff,
      priorSession,
    );
    const response = participantStatus("usable");
    response.headers.append("set-cookie", activitySessionCookie(session));
    return response;
  } catch (error) {
    if (error instanceof HostedBrowserSessionMissingError) {
      return participantError(401, "participant_handoff_invalid");
    }
    return error instanceof HostedBrowserSessionRejectedError
      ? participantError(403, "participant_handoff_invalid")
      : participantUnavailable();
  }
}

async function hostedSessionStatus(
  request: Request,
  origin: string,
  dependencies: BffDependencies,
): Promise<Response> {
  if (!readRequestIsSafe(request, origin)) return participantError(403, "participant_request_rejected");
  const hosted = dependencies.hostedBrowserSessions;
  if (hosted === undefined) return participantUnavailable();
  const session = readCookie(request, ACTIVITY_SESSION_COOKIE);
  if (session === null) return clearActivitySessionError(401, "participant_session_missing");
  try {
    const status = await hosted.sessionStatus(session);
    return participantStatus(status.state);
  } catch (error) {
    if (
      error instanceof HostedBrowserSessionMissingError ||
      error instanceof HostedBrowserSessionRejectedError
    ) {
      return clearActivitySessionError(401, "participant_session_authority_invalid");
    }
    return participantUnavailable();
  }
}

async function logoutHostedSession(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
): Promise<Response> {
  const admitted = await admitMutation(request, origin, sessionKey);
  if (admitted instanceof Response) return admitted;
  if (!isExactObject(admitted.body, [])) return participantError(400, "participant_request_invalid");
  const hosted = dependencies.hostedBrowserSessions;
  const session = readCookie(request, ACTIVITY_SESSION_COOKIE);
  if (hosted === undefined || session === null) {
    return clearActivitySessionError(401, "participant_session_missing");
  }
  try {
    await hosted.logoutSession(session);
    const response = privateJson(200, {
      version: "participant_console_logout.v1",
      logged_out: true,
    });
    response.headers.append("set-cookie", clearActivitySessionCookie());
    return response;
  } catch (error) {
    if (
      error instanceof HostedBrowserSessionMissingError ||
      error instanceof HostedBrowserSessionRejectedError
    ) {
      return clearActivitySessionError(401, "participant_session_missing");
    }
    return participantUnavailable();
  }
}

async function issueHostedStreamTicket(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
): Promise<Response> {
  const admitted = await admitMutation(request, origin, sessionKey);
  if (admitted instanceof Response) return admitted;
  if (
    !isExactObject(admitted.body, ["after_frame_seq"]) ||
    (admitted.body.after_frame_seq !== null &&
      (typeof admitted.body.after_frame_seq !== "number" ||
        !Number.isSafeInteger(admitted.body.after_frame_seq) ||
        admitted.body.after_frame_seq < 0))
  ) {
    return participantError(400, "participant_request_invalid");
  }
  const hosted = dependencies.hostedBrowserSessions;
  const session = readCookie(request, ACTIVITY_SESSION_COOKIE);
  if (hosted === undefined || session === null) {
    return clearActivitySessionError(401, "participant_session_missing");
  }
  try {
    const ticket = await hosted.issueStreamTicket(
      session,
      admitted.body.after_frame_seq as number | null,
    );
    return privateJson(201, {
      version: "participant_console_stream_ticket.v1",
      ticket: ticket.ticket,
      expires_in_ms: ticket.expiresInMs,
    });
  } catch (error) {
    if (
      error instanceof HostedBrowserSessionMissingError ||
      error instanceof HostedBrowserSessionRejectedError
    ) {
      return clearActivitySessionError(401, "participant_session_authority_invalid");
    }
    return participantUnavailable();
  }
}

async function retireActivitySession(
  request: Request,
  dependencies: BffDependencies,
): Promise<void> {
  const session = readCookie(request, ACTIVITY_SESSION_COOKIE);
  if (session === null || dependencies.hostedBrowserSessions === undefined) return;
  try {
    await dependencies.hostedBrowserSessions.logoutSession(session);
  } catch {
    // Clearing the host-only cookie still removes browser authority. Fly keeps
    // no durable Browser Activity Session and expires the orphan automatically.
  }
}

async function verifiedMutation(
  request: Request,
  origin: string,
  sessionKey: Buffer,
  dependencies: BffDependencies,
): Promise<VerifiedMutation | Response> {
  const admitted = await admitMutation(request, origin, sessionKey);
  if (admitted instanceof Response) return admitted;
  try {
    const user = await dependencies.authClient().getUser(admitted.payload.accessToken);
    if (user.id !== admitted.payload.authUserId) return clearSessionError(401, "session_invalid");
    const github = exactGithubIdentity(user);
    if (github.subject !== admitted.payload.github.subject) {
      return clearSessionError(401, "session_invalid");
    }
    const account = await dependencies.dataClient.syncGithubIdentityForMutation({
      authUserId: user.id,
      providerSubject: github.subject,
      githubLogin: github.login,
      avatarUrl: github.avatarUrl,
    });
    if (account === null) return privateError(429, "rate_limited");
    return { ...admitted, account };
  } catch (error) {
    reportPlatformFailure("account_verification", error);
    return credentialWasRejected(error)
      ? clearSessionError(401, "session_invalid")
      : temporarilyUnavailable();
  }
}

async function admitMutation(
  request: Request,
  origin: string,
  sessionKey: Buffer,
): Promise<AcceptedMutation | Response> {
  const payload = mutationSession(request, origin, sessionKey);
  if (payload instanceof Response) return payload;
  const body = await boundedJson(request);
  if (body === null) return privateError(400, "invalid_request");
  return { payload, body };
}

function mutationSession(
  request: Request,
  origin: string,
  sessionKey: Buffer,
): SessionPayload | Response {
  if (!mutationHeadersAreSafe(request, origin)) return privateError(403, "request_rejected");
  const payload = readSessionCookie(request, sessionKey);
  if (payload === null || !constantEqual(request.headers.get("x-worldstream-csrf"), payload.csrf)) {
    return privateError(403, "request_rejected");
  }
  return payload;
}

function newSessionPayload(
  session: AuthSession,
  authUserId: string,
  github: AuthIdentity,
): SessionPayload {
  if (
    session.accessToken.length === 0 ||
    session.refreshToken.length === 0 ||
    !Number.isSafeInteger(session.expiresAt) ||
    session.expiresAt <= Math.floor(Date.now() / 1000)
  ) {
    throw new Error("invalid_session");
  }
  return {
    schema: "worldstream/platform-session/v1",
    authUserId,
    accessToken: session.accessToken,
    refreshToken: session.refreshToken,
    expiresAt: session.expiresAt,
    csrf: randomValue(),
    github,
  };
}

function exactGithubIdentity(user: AuthUser): AuthIdentity {
  const github = user.identities.filter((identity) => identity.provider === "github");
  if (github.length !== 1 || github[0] === undefined || !safeSubject(github[0].subject)) {
    throw new PlatformCredentialRejectedError();
  }
  return github[0];
}

function readSessionCookie(request: Request, key: Buffer): SessionPayload | null {
  const value = readCookie(request, SESSION_COOKIE);
  if (value === null || value.length > 16_384) return null;
  try {
    const payload = openJson<SessionPayload>(key, "platform-session-v1", value);
    if (
      payload.schema !== "worldstream/platform-session/v1" ||
      !safeSubject(payload.authUserId) ||
      !safeRandomValue(payload.csrf) ||
      !Number.isSafeInteger(payload.expiresAt)
    ) {
      return null;
    }
    return payload;
  } catch {
    return null;
  }
}

async function boundedJson(request: Request): Promise<Record<string, unknown> | null> {
  const contentLengthHeader = request.headers.get("content-length");
  if (
    contentLengthHeader !== null &&
    (!/^\d+$/u.test(contentLengthHeader) || Number(contentLengthHeader) > MAX_BODY_BYTES)
  ) {
    return null;
  }
  if (request.body === null) return null;
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let byteLength = 0;
  try {
    while (true) {
      const chunk = await reader.read();
      if (chunk.done) break;
      byteLength += chunk.value.byteLength;
      if (byteLength > MAX_BODY_BYTES) {
        await reader.cancel().catch(() => undefined);
        return null;
      }
      chunks.push(chunk.value);
    }
  } catch {
    return null;
  }
  const bytes = Buffer.concat(chunks, byteLength);
  try {
    const value: unknown = JSON.parse(bytes.toString());
    return isRecord(value) ? value : null;
  } catch {
    return null;
  }
}

function mutationHeadersAreSafe(request: Request, origin: string): boolean {
  if (request.headers.get("origin") !== origin || !fetchSiteIsSafe(request)) return false;
  const contentType = request.headers.get("content-type")?.split(";", 1)[0]?.trim();
  if (contentType !== "application/json") return false;
  return !hasForwardingHeaders(request.headers);
}

function oauthCallbackRequestIsSafe(request: Request, origin: string): boolean {
  const suppliedOrigin = request.headers.get("origin");
  const mode = request.headers.get("sec-fetch-mode");
  const destination = request.headers.get("sec-fetch-dest");
  const site = request.headers.get("sec-fetch-site");
  return (
    (suppliedOrigin === null || suppliedOrigin === origin) &&
    (mode === null || mode === "navigate") &&
    (destination === null || destination === "document") &&
    (site === null || ["same-origin", "same-site", "cross-site", "none"].includes(site)) &&
    !hasForwardingHeaders(request.headers)
  );
}

function readRequestIsSafe(request: Request, origin: string): boolean {
  const suppliedOrigin = request.headers.get("origin");
  return (
    (suppliedOrigin === null || suppliedOrigin === origin) &&
    fetchSiteIsSafe(request) &&
    !hasForwardingHeaders(request.headers)
  );
}

function fetchSiteIsSafe(request: Request): boolean {
  const site = request.headers.get("sec-fetch-site");
  return site === null || site === "same-origin";
}

function hasForwardingHeaders(headers: Headers): boolean {
  for (const name of headers.keys()) {
    if (
      name === "forwarded" ||
      name.startsWith("x-forwarded-") ||
      name.startsWith("x-original-") ||
      name === "x-vercel-forwarded-for" ||
      name === "x-real-ip"
    ) {
      return true;
    }
  }
  return false;
}

function normalizeVercelRequest(request: Request, origin: string): Request | null {
  const url = new URL(request.url);
  const expected = new URL(origin);
  const host = request.headers.get("host");
  const forwardedHost = request.headers.get("x-forwarded-host");
  const forwardedPort = request.headers.get("x-forwarded-port");
  const forwardedProto = request.headers.get("x-forwarded-proto");
  const forwardedFor = request.headers.get("x-forwarded-for");
  const vercelForwardedFor = request.headers.get("x-vercel-forwarded-for");
  const realIp = request.headers.get("x-real-ip");
  const vercelId = request.headers.get("x-vercel-id");
  const hasUnrecognizedForwardingHeader = [...request.headers.keys()].some((name) =>
    name.startsWith("x-original-") ||
    (name.startsWith("x-forwarded-") && !VERCEL_FORWARDING_HEADERS.has(name))
  );
  if (
    url.origin !== origin ||
    host !== expected.host ||
    forwardedHost !== host ||
    (forwardedPort !== null && forwardedPort !== (expected.port || "443")) ||
    forwardedProto !== expected.protocol.slice(0, -1) ||
    forwardedFor === null ||
    isIP(forwardedFor) === 0 ||
    (vercelForwardedFor !== null && vercelForwardedFor !== forwardedFor) ||
    (realIp !== null && realIp !== forwardedFor) ||
    vercelId === null ||
    vercelId.length === 0 ||
    vercelId.length > 256 ||
    /[^\x21-\x7e]/u.test(vercelId) ||
    hasUnrecognizedForwardingHeader
  ) {
    return null;
  }
  const headers = new Headers(request.headers);
  // RFC 7239 Forwarded can be client-modified. It supplies no routing or
  // authority here: discard it only after validating Vercel's fixed envelope.
  // The deployment-neutral BFF still rejects all forwarding metadata.
  headers.delete("forwarded");
  for (const name of VERCEL_FORWARDING_HEADERS) {
    headers.delete(name);
  }
  return new Request(request, { headers });
}

function validateOrigin(value: string): string {
  const url = new URL(value);
  const local = url.hostname === "127.0.0.1" || url.hostname === "localhost";
  if (
    (url.protocol !== "https:" && !(local && url.protocol === "http:")) ||
    url.username !== "" ||
    url.password !== "" ||
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== ""
  ) {
    throw new Error("invalid_canonical_origin");
  }
  return url.origin;
}

function validateReturnTargets(values: readonly string[]): ReadonlySet<string> {
  const targets = new Set(values);
  if (
    targets.size === 0 ||
    targets.size > MAX_RETURN_TARGETS ||
    targets.size !== values.length ||
    [...targets].some((target) => !safeReturnTarget(target))
  ) {
    throw new Error("invalid_return_target_allowlist");
  }
  return targets;
}

function validateKey(value: Uint8Array): Buffer {
  if (value.byteLength !== 32) throw new Error("invalid_encryption_key");
  return Buffer.from(value);
}

function safeReturnTarget(value: string): boolean {
  return (
    value.length >= 1 &&
    value.length <= 512 &&
    value.startsWith("/") &&
    !value.startsWith("//") &&
    !value.includes("\\") &&
    !value.includes("//") &&
    !/[\u0000-\u001f\u007f]/u.test(value)
  );
}

function safeSubject(value: string): boolean {
  return value.length >= 1 && value.length <= 128 && !/[\u0000-\u001f\u007f]/u.test(value);
}

function randomValue(): string {
  return randomBytes(32).toString("base64url");
}

function safeRandomValue(value: string): boolean {
  return value.length >= 43 && value.length <= 128 && /^[A-Za-z0-9_-]+$/u.test(value);
}

function sha256(value: string): Buffer {
  return createHash("sha256").update(value).digest();
}

function base64Url(value: Uint8Array): string {
  return Buffer.from(value).toString("base64url");
}

function bytea(value: Uint8Array): string {
  return `\\x${Buffer.from(value).toString("hex")}`;
}

function fromBytea(value: string): Buffer {
  if (!/^\\x[0-9a-f]+$/u.test(value) || (value.length - 2) % 2 !== 0) {
    throw new Error("invalid_bytea");
  }
  return Buffer.from(value.slice(2), "hex");
}

function seal(key: Buffer, domain: string, plaintext: Uint8Array): Buffer {
  const iv = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", key, iv);
  cipher.setAAD(Buffer.from(domain));
  const ciphertext = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  return Buffer.concat([iv, cipher.getAuthTag(), ciphertext]);
}

function open(key: Buffer, domain: string, sealed: Uint8Array): Buffer {
  if (sealed.byteLength < 29 || sealed.byteLength > 16_384) throw new Error("invalid_ciphertext");
  const bytes = Buffer.from(sealed);
  const decipher = createDecipheriv("aes-256-gcm", key, bytes.subarray(0, 12));
  decipher.setAAD(Buffer.from(domain));
  decipher.setAuthTag(bytes.subarray(12, 28));
  return Buffer.concat([decipher.update(bytes.subarray(28)), decipher.final()]);
}

function sealJson(key: Buffer, domain: string, value: unknown): string {
  return base64Url(seal(key, domain, Buffer.from(JSON.stringify(value))));
}

function openJson<T>(key: Buffer, domain: string, value: string): T {
  return JSON.parse(open(key, domain, Buffer.from(value, "base64url")).toString()) as T;
}

function openBytea(key: Buffer, domain: string, value: string): Buffer {
  return open(key, domain, fromBytea(value));
}

function readCookie(request: Request, name: string): string | null {
  const header = request.headers.get("cookie");
  if (header === null || header.length > 16_384) return null;
  const matches = header
    .split(";")
    .map((part) => part.trim())
    .filter((part) => part.startsWith(`${name}=`));
  if (matches.length !== 1 || matches[0] === undefined) return null;
  const value = matches[0].slice(name.length + 1);
  return value.length === 0 ? null : value;
}

function oauthCookie(value: string, maxAge: number): string {
  return `${OAUTH_COOKIE}=${value}; Path=/; Max-Age=${maxAge}; Secure; HttpOnly; SameSite=Lax`;
}

function sessionCookie(value: string, maxAge: number): string {
  return `${SESSION_COOKIE}=${value}; Path=/; Max-Age=${maxAge}; Secure; HttpOnly; SameSite=Lax`;
}

function clearCookie(name: string): string {
  return `${name}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax`;
}

function activitySessionCookie(value: string): string {
  if (!/^wss1:[0-9a-f]{64}$/u.test(value)) throw new Error("invalid_activity_session");
  return (
    `${ACTIVITY_SESSION_COOKIE}=${value}; Path=${ACTIVITY_SESSION_COOKIE_PATH}; ` +
    `Max-Age=${ACTIVITY_SESSION_MAX_AGE}; Secure; HttpOnly; SameSite=Strict`
  );
}

function clearActivitySessionCookie(): string {
  return (
    `${ACTIVITY_SESSION_COOKIE}=; Path=${ACTIVITY_SESSION_COOKIE_PATH}; ` +
    "Max-Age=0; Secure; HttpOnly; SameSite=Strict"
  );
}

function constantEqual(left: string | null, right: string): boolean {
  if (left === null) return false;
  const leftBytes = Buffer.from(left);
  const rightBytes = Buffer.from(right);
  return leftBytes.byteLength === rightBytes.byteLength && timingSafeEqual(leftBytes, rightBytes);
}

function safeJson(status: number, code: string): Response {
  return Response.json({ error: { code } }, { status });
}

function redirect(location: string): Response {
  return new Response(null, { status: 302, headers: { location } });
}

function privateJson(status: number, body: unknown): Response {
  const response = Response.json(body, { status });
  response.headers.set("cache-control", "private, no-store, max-age=0");
  return response;
}

function publicNoStoreJson(status: number, body: unknown): Response {
  const response = Response.json(body, { status });
  // A later integrity, privacy, or purge event must take effect without a
  // previously cached public summary surviving at a browser or shared edge.
  response.headers.set("cache-control", "no-store, max-age=0");
  return response;
}

function privateError(status: number, code: string): Response {
  return privateJson(status, { error: { code } });
}

function participantStatus(state: "usable" | "disconnected"): Response {
  return privateJson(200, {
    version: "participant_console_session.v1",
    state,
    next_action: state === "usable" ? "continue" : "reconnect",
  });
}

function participantError(status: number, code: string): Response {
  const retryable = status >= 500;
  return privateJson(status, {
    code,
    message: retryable
      ? "The Activity Client cannot reach the Room service safely."
      : "This Activity Client session is not available.",
    next_action: retryable ? "reconnect" : "return_to_task_setup",
    retryable,
  });
}

function participantUnavailable(): Response {
  return participantError(503, "participant_session_unavailable");
}

function clearActivitySessionError(status: number, code: string): Response {
  const response = participantError(status, code);
  response.headers.append("set-cookie", clearActivitySessionCookie());
  return response;
}

function temporarilyUnavailable(): Response {
  return privateError(503, "temporarily_unavailable");
}

function clearSessionError(status: number, code: string): Response {
  const response = privateError(status, code);
  response.headers.append("set-cookie", clearCookie(SESSION_COOKIE));
  return response;
}

function refreshRequired(payload: SessionPayload): Response {
  return privateJson(401, {
    error: { code: "session_refresh_required" },
    csrf: payload.csrf,
  });
}

function credentialWasRejected(error: unknown): boolean {
  return error instanceof PlatformCredentialRejectedError;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isExactObject(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  if (!isRecord(value)) return false;
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}
