export const PARTICIPANT_HANDOFF_VERSION = "participant_handoff.v1" as const;
export const PARTICIPANT_SESSION_VERSION = "participant_console_session.v1" as const;
export const PARTICIPANT_STREAM_TICKET_VERSION =
  "participant_console_stream_ticket.v1" as const;

const HANDOFF_PATTERN = /^wsh1:[0-9a-f]{64}$/;
const STREAM_TICKET_PATTERN = /^wst1:[0-9a-f]{64}$/;
const PROHIBITED_KEYS = new Set([
  "bearer",
  "token_hash",
  "secret_reference",
  "secret_ref",
  "host_authority",
  "runner_authority",
  "room_id",
  "member_id",
  "membership_id",
  "path",
  "file_path",
  "agent_private_memory",
  "invocation_context",
  "prompt",
  "provider_response",
  "model_response",
]);

export interface BrowserNavigationTarget {
  location: Pick<Location, "origin" | "pathname" | "search" | "hash">;
  history: Pick<History, "replaceState">;
}

export interface HostedActivityClientConnection {
  /** Exact canonical browser origin serving this Activity Client. */
  readonly browserOrigin: string;
  /** Platform-session-bound CSRF value obtained from the same-origin BFF. */
  readonly csrf: string;
}

export type ActivityClientStartup =
  | { kind: "direct" }
  | { kind: "handoff"; handoff: string }
  | { kind: "retained"; status: ActivityClientSessionStatus }
  | { kind: "retained_error"; error: ActivityClientHandoffError }
  | { kind: "invalid_handoff" };

export interface ActivityClientSessionStatus {
  version: typeof PARTICIPANT_SESSION_VERSION;
  state: "usable" | "disconnected";
  nextAction: "continue" | "reconnect";
}

/** One browser-safe, single-use admission value for the direct Fly stream. */
export interface ActivityClientStreamTicket {
  readonly version: typeof PARTICIPANT_STREAM_TICKET_VERSION;
  readonly ticket: string;
  readonly expiresInMs: number;
}

export class ActivityClientHandoffError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly nextAction: "return_to_task_setup" | "retry" | "reconnect",
    readonly retryable: boolean,
  ) {
    super(message);
    this.name = "ActivityClientHandoffError";
  }
}

/**
 * Consumes one local fragment-only handoff and scrubs it synchronously. The
 * token never enters history state, storage, query parameters, or logging.
 */
export function consumeActivityClientHandoffFragment(target: BrowserNavigationTarget): ActivityClientStartup {
  const { hash, pathname, search } = target.location;
  if (!hash.startsWith("#handoff=")) return { kind: "direct" };
  target.history.replaceState(null, "", `${pathname}${search}`);
  const handoff = hash.slice("#handoff=".length);
  return HANDOFF_PATTERN.test(handoff) ? { kind: "handoff", handoff } : { kind: "invalid_handoff" };
}

/** Preserves the existing direct bootstrap whenever no handoff is present. */
export function selectActivityClientStartup(target: BrowserNavigationTarget): ActivityClientStartup {
  return consumeActivityClientHandoffFragment(target);
}

export interface OfferedActionSubmission {
  action_id: string;
  based_on_room_seq: number;
  offer_id: string;
  schema_digest: string;
  action_type: string;
  payload: unknown;
}

export interface ActivityClientRoomHead {
  room_seq: number;
  genesis_or_transition_hash: string;
  core_schema_version: string;
  pack_digest: string;
  core_state_hash: string;
  activity_state_hash: string;
  authoritative_state_hash: string;
}

export interface ActivityClientDelivery {
  kind: "projection_reset" | "observation";
  body: Record<string, unknown>;
}

export interface ActivityClientPack {
  id: string;
  version: string;
  digest: string;
}

export interface AuthorizedRoomDeliveryBatch {
  pack: ActivityClientPack;
  room_head: ActivityClientRoomHead;
  frame_head: number;
  delivery: ActivityClientDelivery[];
}

/** Read-only, verified historical projection returned by the local session proxy. */
export interface VerifiedRoomReplay {
  requested_room_seq: number;
  room_head: ActivityClientRoomHead;
  projection: Record<string, unknown>;
  projection_hash: string;
  verification: "verified";
  room_health: string;
  integrity_generation: number;
}

/** Opaque, browser-safe record acknowledging one submitted offered Action. */
export interface ActivityClientActionReceipt {
  readonly state: "accepted" | "rejected";
  readonly [key: string]: unknown;
}

/** Tries a retained HttpOnly session before preserving the direct client flow. */
export async function resumeRetainedActivityClient(
  startup: ActivityClientStartup,
  client: Pick<ActivityClientHandoffClient, "resume">,
): Promise<ActivityClientStartup> {
  if (startup.kind !== "direct") return startup;
  try {
    return { kind: "retained", status: await client.resume() };
  } catch (error) {
    if (error instanceof ActivityClientHandoffError && error.code === "participant_session_authority_invalid") {
      return { kind: "retained_error", error };
    }
    return startup;
  }
}

/** Cookie-backed client whose requests contain no Room, Membership, or bearer. */
export class ActivityClientHandoffClient {
  private readonly endpoint: string;
  private readonly fetch: typeof globalThis.fetch;
  private readonly hostedCsrf: string | null;

  constructor(
    endpoint: string,
    fetch?: typeof globalThis.fetch,
    hosted?: HostedActivityClientConnection,
  ) {
    this.endpoint = hosted === undefined
      ? exactLoopbackEndpoint(endpoint)
      : exactHostedEndpoint(endpoint, hosted);
    this.hostedCsrf = hosted?.csrf ?? null;
    this.fetch = fetch === undefined
      ? (input, init) => globalThis.fetch(input, init)
      : (input, init) => fetch(input, init);
  }

  async redeem(handoff: string): Promise<ActivityClientSessionStatus> {
    if (!HANDOFF_PATTERN.test(handoff)) {
      throw new ActivityClientHandoffError(
        "participant_handoff_invalid",
        "This Activity Client handoff is invalid.",
        "return_to_task_setup",
        false,
      );
    }
    const headers = new Headers({ "X-WorldStream-Participant-Handoff": handoff });
    let body: string | undefined;
    if (this.hostedCsrf !== null) {
      headers.set("Content-Type", "application/json");
      headers.set("X-WorldStream-CSRF", this.hostedCsrf);
      body = "{}";
    }
    return this.statusRequest("/api/v1/participant-console/handoffs:redeem", {
      method: "POST",
      credentials: "include",
      cache: "no-store",
      headers,
      body,
    });
  }

  async resume(): Promise<ActivityClientSessionStatus> {
    return this.statusRequest("/api/v1/participant-console/session", {
      method: "GET",
      credentials: "include",
      cache: "no-store",
    });
  }

  /**
   * Mints one short-lived ticket from the retained hosted session. The Cursor
   * is the only browser-selected synchronization value; no Room or Membership
   * identifier crosses this request.
   */
  async issueStreamTicket(
    afterFrameSeq: number | null,
  ): Promise<ActivityClientStreamTicket> {
    if (
      this.hostedCsrf === null ||
      (afterFrameSeq !== null &&
        (!Number.isSafeInteger(afterFrameSeq) || afterFrameSeq < 0))
    ) {
      throw new ActivityClientHandoffError(
        "participant_stream_ticket_unavailable",
        "Hosted realtime admission is not configured.",
        "return_to_task_setup",
        false,
      );
    }
    const value = await this.jsonRequest(
      "/api/v1/participant-console/session:stream-ticket",
      {
        method: "POST",
        credentials: "include",
        cache: "no-store",
        headers: {
          "Content-Type": "application/json",
          "X-WorldStream-CSRF": this.hostedCsrf,
        },
        body: JSON.stringify({ after_frame_seq: afterFrameSeq }),
      },
      true,
    );
    if (!isRawStreamTicketResponse(value)) {
      throw new ActivityClientHandoffError(
        "participant_session_invalid_response",
        "Activity Client received invalid realtime admission material.",
        "return_to_task_setup",
        false,
      );
    }
    return {
      version: value.version,
      ticket: value.ticket,
      expiresInMs: value.expires_in_ms,
    };
  }

  /** Retires the hosted Browser Activity Session and clears its HttpOnly cookie. */
  async logout(): Promise<void> {
    if (this.hostedCsrf === null) {
      throw new ActivityClientHandoffError(
        "participant_logout_unavailable",
        "Hosted Activity Client logout is not configured.",
        "return_to_task_setup",
        false,
      );
    }
    const value = await this.jsonRequest("/api/v1/participant-console/session:logout", {
      method: "POST",
      credentials: "include",
      cache: "no-store",
      headers: {
        "Content-Type": "application/json",
        "X-WorldStream-CSRF": this.hostedCsrf,
      },
      body: "{}",
    });
    if (
      !isExactObject(value, ["version", "logged_out"]) ||
      value.version !== "participant_console_logout.v1" ||
      value.logged_out !== true
    ) {
      throw new ActivityClientHandoffError(
        "participant_session_invalid_response",
        "Activity Client received an invalid logout response.",
        "return_to_task_setup",
        false,
      );
    }
  }

  async observe(afterFrameSeq: number | null): Promise<AuthorizedRoomDeliveryBatch> {
    const { value, headers } = await this.jsonResponse("/api/v1/participant-console/session:observe", {
      method: "POST",
      credentials: "include",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ after_frame_seq: afterFrameSeq }),
    });
    if (!isAuthorizedRoomDeliveryBatch(value)) {
      throw new ActivityClientHandoffError(
        "participant_session_invalid_response",
        "Activity Client received an invalid authorized delivery batch.",
        "return_to_task_setup",
        false,
      );
    }
    const acknowledgement = headers.get("X-WorldStream-Delivery-Acknowledgement");
    if (acknowledgement !== null) {
      if (!/^wsa1:[0-9a-f]{64}$/.test(acknowledgement)) {
        throw new ActivityClientHandoffError(
          "participant_session_invalid_response",
          "Activity Client received an invalid delivery acknowledgement.",
          "return_to_task_setup",
          false,
        );
      }
      const status = await this.statusRequest("/api/v1/participant-console/session:acknowledge", {
        method: "POST",
        credentials: "include",
        cache: "no-store",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ acknowledgement, frame_head: value.frame_head }),
      });
      if (status.state !== "usable") {
        throw new ActivityClientHandoffError(
          "participant_session_unavailable",
          "Activity Client delivery acknowledgement was not accepted.",
          "reconnect",
          true,
        );
      }
    }
    return value;
  }

  async act(request: OfferedActionSubmission): Promise<ActivityClientActionReceipt> {
    const value = await this.jsonRequest("/api/v1/participant-console/session:act", {
      method: "POST",
      credentials: "include",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
    });
    if (!isActivityClientActionReceipt(value)) {
      throw new ActivityClientHandoffError(
        "participant_session_invalid_response",
        "Activity Client received an invalid Action receipt.",
        "return_to_task_setup",
        false,
      );
    }
    return value;
  }

  async replay(atRoomSeq: number): Promise<VerifiedRoomReplay> {
    if (!Number.isSafeInteger(atRoomSeq) || atRoomSeq < 0) {
      throw new ActivityClientHandoffError(
        "participant_replay_invalid_request",
        "Historical Replay requires a valid Room sequence.",
        "return_to_task_setup",
        false,
      );
    }
    const value = await this.jsonRequest("/api/v1/participant-console/session:replay", {
      method: "POST",
      credentials: "include",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ at_room_seq: atRoomSeq }),
    });
    if (!isVerifiedRoomReplay(value)) {
      throw new ActivityClientHandoffError(
        "participant_replay_invalid_response",
        "Activity Client received an invalid historical Replay.",
        "return_to_task_setup",
        false,
      );
    }
    return value;
  }

  private async statusRequest(path: string, init: RequestInit): Promise<ActivityClientSessionStatus> {
    const value = await this.jsonRequest(path, init);
    if (!isActivityClientSessionStatus(value)) {
      throw new ActivityClientHandoffError(
        "participant_session_invalid_response",
        "Activity Client received an invalid session response.",
        "return_to_task_setup",
        false,
      );
    }
    return {
      version: value.version,
      state: value.state,
      nextAction: value.next_action,
    };
  }

  private async jsonRequest(
    path: string,
    init: RequestInit,
    allowExactStreamTicket = false,
  ): Promise<unknown> {
    return (await this.jsonResponse(path, init, allowExactStreamTicket)).value;
  }

  private async jsonResponse(
    path: string,
    init: RequestInit,
    allowExactStreamTicket = false,
  ): Promise<{ value: unknown; headers: Headers }> {
    let response: Response;
    try {
      response = await this.fetch(`${this.endpoint}${path}`, init);
    } catch {
      throw new ActivityClientHandoffError(
        "participant_session_unavailable",
        "Activity Client could not reach the local Supervisor.",
        "reconnect",
        true,
      );
    }
    let value: unknown;
    try {
      value = await response.json();
    } catch {
      throw new ActivityClientHandoffError(
        "participant_session_invalid_response",
        "Activity Client received an invalid response.",
        "return_to_task_setup",
        false,
      );
    }
    if (!response.ok) throw readSafeError(value);
    if (
      containsProhibitedMaterial(value) &&
      !(allowExactStreamTicket && isRawStreamTicketResponse(value))
    ) {
      throw new ActivityClientHandoffError(
        "participant_session_invalid_response",
        "Activity Client received unsafe routing or authority material.",
        "return_to_task_setup",
        false,
      );
    }
    return { value, headers: response.headers };
  }
}

function exactLoopbackEndpoint(value: string): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new TypeError("Activity Client Supervisor endpoint must be a fixed loopback origin");
  }
  if (
    url.protocol !== "http:"
    || !["127.0.0.1", "localhost", "[::1]"].includes(url.hostname)
    || url.pathname !== "/"
    || url.search !== ""
    || url.hash !== ""
    || url.username !== ""
    || url.password !== ""
  ) {
    throw new TypeError("Activity Client Supervisor endpoint must be a fixed loopback origin");
  }
  return url.origin;
}

function exactHostedEndpoint(
  endpoint: string,
  connection: HostedActivityClientConnection,
): string {
  let url: URL;
  let browser: URL;
  try {
    url = new URL(endpoint);
    browser = new URL(connection.browserOrigin);
  } catch {
    throw new TypeError("Hosted Activity Client endpoint must be its exact browser origin");
  }
  const local = url.hostname === "127.0.0.1" || url.hostname === "localhost";
  if (
    url.origin !== browser.origin ||
    (!local && url.protocol !== "https:") ||
    (local && url.protocol !== "http:") ||
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== "" ||
    url.username !== "" ||
    url.password !== "" ||
    browser.pathname !== "/" ||
    browser.search !== "" ||
    browser.hash !== "" ||
    connection.csrf.length < 43 ||
    connection.csrf.length > 128 ||
    !/^[A-Za-z0-9_-]+$/u.test(connection.csrf)
  ) {
    throw new TypeError("Hosted Activity Client endpoint must be its exact browser origin");
  }
  return url.origin;
}

function isActivityClientSessionStatus(value: unknown): value is {
  version: typeof PARTICIPANT_SESSION_VERSION;
  state: "usable" | "disconnected";
  next_action: "continue" | "reconnect";
} {
  if (!isExactObject(value, ["version", "state", "next_action"])) return false;
  if (value.version !== PARTICIPANT_SESSION_VERSION) return false;
  return (value.state === "usable" && value.next_action === "continue")
    || (value.state === "disconnected" && value.next_action === "reconnect");
}

function isRawStreamTicketResponse(value: unknown): value is {
  version: typeof PARTICIPANT_STREAM_TICKET_VERSION;
  ticket: string;
  expires_in_ms: number;
} {
  return isExactObject(value, ["version", "ticket", "expires_in_ms"])
    && value.version === PARTICIPANT_STREAM_TICKET_VERSION
    && typeof value.ticket === "string"
    && STREAM_TICKET_PATTERN.test(value.ticket)
    && typeof value.expires_in_ms === "number"
    && Number.isSafeInteger(value.expires_in_ms)
    && value.expires_in_ms > 0
    && value.expires_in_ms <= 15_000;
}

function isAuthorizedRoomDeliveryBatch(value: unknown): value is AuthorizedRoomDeliveryBatch {
  if (!isExactObject(value, ["pack", "room_head", "frame_head", "delivery"])) return false;
  if (!Number.isSafeInteger(value.frame_head) || (value.frame_head as number) < 0) return false;
  if (!isActivityClientPack(value.pack)) return false;
  if (!isActivityClientRoomHead(value.room_head)) return false;
  if (!Array.isArray(value.delivery) || value.delivery.length > 10_000) return false;
  return value.delivery.every((item) => isExactObject(item, ["kind", "body"])
    && (item.kind === "projection_reset" || item.kind === "observation")
    && isRecord(item.body));
}

function isActivityClientPack(value: unknown): value is ActivityClientPack {
  return isExactObject(value, ["id", "version", "digest"])
    && typeof value.id === "string" && value.id.length > 0 && value.id.length <= 256
    && typeof value.version === "string" && value.version.length > 0 && value.version.length <= 256
    && typeof value.digest === "string" && /^(?:blake3|sha256):[0-9a-f]{64}$/.test(value.digest);
}

function isVerifiedRoomReplay(value: unknown): value is VerifiedRoomReplay {
  if (!isExactObject(value, [
    "requested_room_seq",
    "room_head",
    "projection",
    "projection_hash",
    "verification",
    "room_health",
    "integrity_generation",
  ])) return false;
  return Number.isSafeInteger(value.requested_room_seq)
    && (value.requested_room_seq as number) >= 0
    && isActivityClientRoomHead(value.room_head)
    && isRecord(value.projection)
    && typeof value.projection_hash === "string" && value.projection_hash.length > 0
    && value.verification === "verified"
    && typeof value.room_health === "string" && value.room_health.length > 0
    && Number.isSafeInteger(value.integrity_generation) && (value.integrity_generation as number) >= 0;
}

function isActivityClientRoomHead(value: unknown): value is ActivityClientRoomHead {
  if (!isExactObject(value, [
    "room_seq",
    "genesis_or_transition_hash",
    "core_schema_version",
    "pack_digest",
    "core_state_hash",
    "activity_state_hash",
    "authoritative_state_hash",
  ])) return false;
  if (!Number.isSafeInteger(value.room_seq) || (value.room_seq as number) < 0) return false;
  return [value.genesis_or_transition_hash, value.core_schema_version, value.pack_digest, value.core_state_hash, value.activity_state_hash, value.authoritative_state_hash]
    .every((item) => typeof item === "string" && item.length > 0);
}

function isActivityClientActionReceipt(value: unknown): value is ActivityClientActionReceipt {
  return isRecord(value) && (value.state === "accepted" || value.state === "rejected");
}

function readSafeError(value: unknown): ActivityClientHandoffError {
  if (
    isExactObject(value, ["code", "message", "next_action", "retryable"])
    && typeof value.code === "string"
    && typeof value.message === "string"
    && (value.next_action === "return_to_task_setup" || value.next_action === "retry" || value.next_action === "reconnect")
    && typeof value.retryable === "boolean"
    && !containsProhibitedMaterial(value)
  ) {
    return new ActivityClientHandoffError(value.code, value.message, value.next_action, value.retryable);
  }
  return new ActivityClientHandoffError(
    "participant_session_invalid_response",
    "Activity Client received an invalid error response.",
    "return_to_task_setup",
    false,
  );
}

function isExactObject(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function containsProhibitedMaterial(value: unknown): boolean {
  if (typeof value === "string") return value.includes("wsb1:") || value.includes("wst1:");
  if (Array.isArray(value)) return value.some(containsProhibitedMaterial);
  if (value === null || typeof value !== "object") return false;
  return Object.entries(value).some(([key, child]) => PROHIBITED_KEYS.has(key.toLowerCase()) || containsProhibitedMaterial(child));
}
