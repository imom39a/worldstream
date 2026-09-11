import { encodeCanonical, type CanonicalObject } from "@worldstream/pack-sdk";
import { reportPlatformFailure } from "./diagnostics.js";

const MAX_GATEWAY_RESPONSE_BYTES = 16 * 1024;
const BLAKE3_PATTERN = /^blake3:[0-9a-f]{64}$/u;
const SHA256_PATTERN = /^sha256:[0-9a-f]{64}$/u;
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const ULID_PATTERN = /^[0-9A-HJKMNP-TV-Z]{26}$/u;
const SAFE_REFERENCE_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u;
const SAFE_OPERATION_PATTERN = /^[a-z][a-z0-9-]{0,63}$/u;
const HANDOFF_PATTERN = /^wsh1:[0-9a-f]{64}$/u;
const SESSION_PATTERN = /^wss1:[0-9a-f]{64}$/u;
const TICKET_PATTERN = /^wst1:[0-9a-f]{64}$/u;

/** Private server-only result of resolving an account, Run, and entry selector. */
export interface OwnedRunMembershipCorrespondence {
  readonly runId: string;
  readonly listingRevisionDigest: string;
  readonly hostInstallationId: string;
  readonly roomSetupOperationId: string;
  readonly roomId: string;
  readonly pack: {
    readonly id: string;
    readonly version: string;
    readonly digest: string;
  };
  readonly clientReleaseDigest: string;
  readonly clientSurfaceId: string;
  readonly accessMode: "participant" | "spectator";
  readonly purpose: "participant" | "creator_spectator";
  readonly seatId: string | null;
  readonly role: string | null;
  readonly principalKind: "human" | "agent";
  readonly principalId: string;
  readonly membershipId: string;
}

export interface HostedBrowserHandoff {
  readonly clientUrl: string;
}

export interface HostedBrowserSessionStatus {
  readonly state: "usable" | "disconnected";
}

export interface HostedBrowserStreamTicket {
  readonly ticket: string;
  readonly expiresInMs: number;
}

/** Narrow service client used by the Vercel BFF. */
export interface HostedBrowserSessionClient {
  issueHandoff(
    platformAccountId: string,
    binding: OwnedRunMembershipCorrespondence,
  ): Promise<HostedBrowserHandoff>;
  redeemHandoff(
    platformAccountId: string,
    handoff: string,
    priorSession: string | null,
  ): Promise<string>;
  sessionStatus(session: string): Promise<HostedBrowserSessionStatus>;
  issueStreamTicket(
    session: string,
    afterFrameSeq: number | null,
  ): Promise<HostedBrowserStreamTicket>;
  logoutSession(session: string): Promise<void>;
}

export class HostedBrowserSessionRejectedError extends Error {
  constructor(message = "hosted_browser_session_rejected") {
    super(message);
    this.name = "HostedBrowserSessionRejectedError";
  }
}

export class HostedBrowserSessionMissingError extends HostedBrowserSessionRejectedError {
  constructor() {
    super("hosted_browser_session_missing");
    this.name = "HostedBrowserSessionMissingError";
  }
}

export class HostedBrowserSessionUnavailableError extends Error {
  constructor(message = "hosted_browser_session_unavailable", options?: ErrorOptions) {
    super(message, options);
    this.name = "HostedBrowserSessionUnavailableError";
  }
}

/** Literal authenticated client for the four Fly hosted browser-session routes. */
export class HttpHostedBrowserSessionClient implements HostedBrowserSessionClient {
  readonly #base: URL;
  readonly #clientOrigin: string;
  readonly #serviceAuthority: string;
  readonly #timeoutMs: number;
  readonly #fetch: typeof fetch;

  constructor(input: {
    baseUrl: string;
    clientOrigin: string;
    serviceAuthority: string;
    timeoutMs?: number;
    fetchImplementation?: typeof fetch;
  }) {
    const base = exactServiceBase(input.baseUrl);
    const clientOrigin = exactClientOrigin(input.clientOrigin);
    if (
      input.serviceAuthority.length < 32 ||
      input.serviceAuthority.length > 512 ||
      !/^[\x21-\x7e]+$/u.test(input.serviceAuthority)
    ) {
      throw new HostedBrowserSessionRejectedError("invalid_browser_session_configuration");
    }
    // The Fly Controller hop owns a 25s budget, including bounded Runtime
    // membership retries. Let it return its result before aborting the BFF hop.
    const timeoutMs = input.timeoutMs ?? 30_000;
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 100 || timeoutMs > 30_000) {
      throw new HostedBrowserSessionRejectedError("invalid_browser_session_timeout");
    }
    this.#base = base;
    this.#clientOrigin = clientOrigin;
    this.#serviceAuthority = input.serviceAuthority;
    this.#timeoutMs = timeoutMs;
    this.#fetch = input.fetchImplementation ?? fetch;
  }

  async issueHandoff(
    platformAccountId: string,
    binding: OwnedRunMembershipCorrespondence,
  ): Promise<HostedBrowserHandoff> {
    if (!UUID_PATTERN.test(platformAccountId)) throw new HostedBrowserSessionRejectedError();
    validateCorrespondence(binding);
    const value = await this.#call("/v1/hosted/browser-handoffs/issue", {
      schema: "worldstream/hosted-browser-handoff-request/v1",
      platform_account_id: platformAccountId,
      run_id: binding.runId,
      listing_revision_digest: binding.listingRevisionDigest,
      host_installation_id: binding.hostInstallationId,
      room_setup_operation_id: binding.roomSetupOperationId,
      room_id: binding.roomId,
      pack: binding.pack,
      client_release_digest: binding.clientReleaseDigest,
      client_surface_id: binding.clientSurfaceId,
      access_mode: binding.accessMode,
      purpose: binding.purpose,
      seat_id: binding.seatId,
      role: binding.role,
      principal_kind: binding.principalKind,
      principal_id: binding.principalId,
      membership_id: binding.membershipId,
    });
    if (!isExactRecord(value, ["schema", "client_url"])) throw invalidResponse();
    if (value.schema !== "worldstream/hosted-browser-handoff-response/v1") {
      throw invalidResponse();
    }
    const clientUrl = requiredString(value.client_url);
    const parsed = new URL(clientUrl);
    if (
      parsed.origin !== this.#clientOrigin ||
      parsed.username !== "" ||
      parsed.password !== "" ||
      parsed.search !== "" ||
      !parsed.pathname.endsWith("/") ||
      parsed.pathname.split("/").includes("..") ||
      !parsed.hash.startsWith("#handoff=") ||
      !HANDOFF_PATTERN.test(parsed.hash.slice("#handoff=".length))
    ) {
      throw invalidResponse();
    }
    return { clientUrl };
  }

  async redeemHandoff(
    platformAccountId: string,
    handoff: string,
    priorSession: string | null,
  ): Promise<string> {
    if (
      !UUID_PATTERN.test(platformAccountId) ||
      !HANDOFF_PATTERN.test(handoff) ||
      (priorSession !== null && !SESSION_PATTERN.test(priorSession))
    ) {
      throw new HostedBrowserSessionRejectedError();
    }
    const value = await this.#call("/v1/hosted/browser-sessions/admit", {
      schema: "worldstream/hosted-browser-handoff-redeem-request/v1",
      platform_account_id: platformAccountId,
      handoff,
      prior_session: priorSession,
    });
    if (!isExactRecord(value, ["schema", "session"])) throw invalidResponse();
    if (
      value.schema !== "worldstream/hosted-browser-handoff-redeem-response/v1" ||
      typeof value.session !== "string" ||
      !SESSION_PATTERN.test(value.session)
    ) {
      throw invalidResponse();
    }
    return value.session;
  }

  async sessionStatus(session: string): Promise<HostedBrowserSessionStatus> {
    const value = await this.#sessionCall(
      "/v1/hosted/browser-sessions/status",
      session,
    );
    if (!isExactRecord(value, ["schema", "state"])) throw invalidResponse();
    if (
      value.schema !== "worldstream/hosted-browser-session-status/v1" ||
      (value.state !== "usable" && value.state !== "disconnected")
    ) {
      throw invalidResponse();
    }
    return { state: value.state };
  }

  async issueStreamTicket(
    session: string,
    afterFrameSeq: number | null,
  ): Promise<HostedBrowserStreamTicket> {
    if (
      !SESSION_PATTERN.test(session) ||
      (afterFrameSeq !== null &&
        (!Number.isSafeInteger(afterFrameSeq) || afterFrameSeq < 0))
    ) {
      throw new HostedBrowserSessionRejectedError();
    }
    const value = await this.#call(
      "/v1/hosted/browser-sessions/stream-ticket",
      {
        schema: "worldstream/hosted-browser-stream-ticket-request/v1",
        session,
        after_frame_seq: afterFrameSeq,
      },
    );
    if (!isExactRecord(value, ["schema", "ticket", "expires_in_ms"])) {
      throw invalidResponse();
    }
    if (
      value.schema !== "worldstream/hosted-browser-stream-ticket-response/v1" ||
      typeof value.ticket !== "string" ||
      !TICKET_PATTERN.test(value.ticket) ||
      typeof value.expires_in_ms !== "number" ||
      !Number.isSafeInteger(value.expires_in_ms) ||
      value.expires_in_ms <= 0 ||
      value.expires_in_ms > 15_000
    ) {
      throw invalidResponse();
    }
    return { ticket: value.ticket, expiresInMs: value.expires_in_ms };
  }

  async logoutSession(session: string): Promise<void> {
    const value = await this.#sessionCall(
      "/v1/hosted/browser-sessions/logout",
      session,
    );
    if (
      !isExactRecord(value, ["schema", "logged_out"]) ||
      value.schema !== "worldstream/hosted-browser-session-logout/v1" ||
      value.logged_out !== true
    ) {
      throw invalidResponse();
    }
  }

  async #sessionCall(path: string, session: string): Promise<unknown> {
    if (!SESSION_PATTERN.test(session)) {
      throw new HostedBrowserSessionMissingError();
    }
    return this.#call(path, {
      schema: "worldstream/hosted-browser-session-request/v1",
      session,
    });
  }

  async #call(path: string, body: Record<string, unknown>): Promise<unknown> {
    let response: Response;
    try {
      response = await this.#fetch(new URL(path, this.#base), {
        method: "POST",
        headers: {
          accept: "application/json",
          authorization: `Bearer ${this.#serviceAuthority}`,
          "content-type": "application/json",
        },
        body: Buffer.from(encodeCanonical(body as CanonicalObject)),
        cache: "no-store",
        redirect: "error",
        signal: AbortSignal.timeout(this.#timeoutMs),
      });
    } catch (error) {
      reportPlatformFailure("fly_browser_session", error);
      throw new HostedBrowserSessionUnavailableError(undefined, { cause: error });
    }
    if (!response.ok) {
      if ([401, 404].includes(response.status)) throw new HostedBrowserSessionMissingError();
      if ([400, 403, 409, 422].includes(response.status)) {
        throw new HostedBrowserSessionRejectedError();
      }
      reportPlatformFailure("fly_browser_session", undefined, { status: response.status });
      throw new HostedBrowserSessionUnavailableError();
    }
    const bytes = await readBoundedBody(response, MAX_GATEWAY_RESPONSE_BYTES);
    try {
      return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)) as unknown;
    } catch (error) {
      throw new HostedBrowserSessionUnavailableError("invalid_browser_session_response", {
        cause: error,
      });
    }
  }
}

function validateCorrespondence(value: OwnedRunMembershipCorrespondence): void {
  if (
    !UUID_PATTERN.test(value.runId) ||
    !BLAKE3_PATTERN.test(value.listingRevisionDigest) ||
    !SAFE_REFERENCE_PATTERN.test(value.hostInstallationId) ||
    !SAFE_OPERATION_PATTERN.test(value.roomSetupOperationId) ||
    !ULID_PATTERN.test(value.roomId) ||
    !SAFE_REFERENCE_PATTERN.test(value.pack.id) ||
    !SAFE_REFERENCE_PATTERN.test(value.pack.version) ||
    !BLAKE3_PATTERN.test(value.pack.digest) ||
    !SHA256_PATTERN.test(value.clientReleaseDigest) ||
    !SAFE_REFERENCE_PATTERN.test(value.clientSurfaceId) ||
    (value.principalKind !== "human" && value.principalKind !== "agent") ||
    !ULID_PATTERN.test(value.principalId) ||
    !ULID_PATTERN.test(value.membershipId)
  ) {
    throw new HostedBrowserSessionRejectedError("invalid_membership_correspondence");
  }
  const participant =
    value.purpose === "participant" &&
    value.accessMode === "participant" &&
    value.seatId !== null &&
    SAFE_REFERENCE_PATTERN.test(value.seatId) &&
    value.role !== null &&
    SAFE_REFERENCE_PATTERN.test(value.role);
  const spectator =
    value.purpose === "creator_spectator" &&
    value.principalKind === "human" &&
    value.accessMode === "spectator" &&
    value.seatId === null &&
    value.role === null;
  if (!participant && !spectator) {
    throw new HostedBrowserSessionRejectedError("invalid_membership_correspondence");
  }
}

function exactServiceBase(value: string): URL {
  const url = new URL(value);
  const local = url.hostname === "127.0.0.1" || url.hostname === "localhost";
  if (
    (!local && url.protocol !== "https:") ||
    (local && !["http:", "https:"].includes(url.protocol)) ||
    url.username !== "" ||
    url.password !== "" ||
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== ""
  ) {
    throw new HostedBrowserSessionRejectedError("invalid_browser_session_configuration");
  }
  return url;
}

function exactClientOrigin(value: string): string {
  const url = new URL(value);
  const local = url.hostname === "127.0.0.1" || url.hostname === "localhost";
  if (
    (!local && url.protocol !== "https:") ||
    (local && url.protocol !== "http:") ||
    url.username !== "" ||
    url.password !== "" ||
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== ""
  ) {
    throw new HostedBrowserSessionRejectedError("invalid_browser_client_origin");
  }
  return url.origin;
}

async function readBoundedBody(response: Response, maximum: number): Promise<Uint8Array> {
  const reader = response.body?.getReader();
  if (reader === undefined) return new Uint8Array();
  const chunks: Uint8Array[] = [];
  let used = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    used += value.byteLength;
    if (used > maximum) {
      await reader.cancel();
      throw new HostedBrowserSessionUnavailableError("browser_session_response_too_large");
    }
    chunks.push(value);
  }
  const result = new Uint8Array(used);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return result;
}

function requiredString(value: unknown): string {
  if (typeof value !== "string" || value.length === 0) throw invalidResponse();
  return value;
}

function isExactRecord(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

function invalidResponse(): HostedBrowserSessionUnavailableError {
  return new HostedBrowserSessionUnavailableError("invalid_browser_session_response");
}
