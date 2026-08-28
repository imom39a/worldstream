export const PARTICIPANT_HANDOFF_VERSION = "participant_handoff.v1" as const;
export const PARTICIPANT_SESSION_VERSION = "participant_console_session.v1" as const;

const HANDOFF_PATTERN = /^wsh1:[0-9a-f]{64}$/;
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

export type ParticipantConsoleStartup =
  | { kind: "direct" }
  | { kind: "handoff"; handoff: string }
  | { kind: "retained"; status: ParticipantSessionStatus }
  | { kind: "retained_error"; error: ParticipantHandoffError }
  | { kind: "invalid_handoff" };

export interface ParticipantSessionStatus {
  version: typeof PARTICIPANT_SESSION_VERSION;
  state: "usable" | "disconnected";
  nextAction: "continue" | "reconnect";
}

export class ParticipantHandoffError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly nextAction: "return_to_task_setup" | "retry" | "reconnect",
    readonly retryable: boolean,
  ) {
    super(message);
    this.name = "ParticipantHandoffError";
  }
}

/**
 * Consumes one local fragment-only handoff and scrubs it synchronously. The
 * token never enters history state, storage, query parameters, or logging.
 */
export function consumeParticipantHandoffFragment(target: BrowserNavigationTarget): ParticipantConsoleStartup {
  const { hash, pathname, search } = target.location;
  if (!hash.startsWith("#handoff=")) return { kind: "direct" };
  target.history.replaceState(null, "", `${pathname}${search}`);
  const handoff = hash.slice("#handoff=".length);
  return HANDOFF_PATTERN.test(handoff) ? { kind: "handoff", handoff } : { kind: "invalid_handoff" };
}

/** Preserves the existing direct bootstrap whenever no handoff is present. */
export function selectParticipantConsoleStartup(target: BrowserNavigationTarget): ParticipantConsoleStartup {
  return consumeParticipantHandoffFragment(target);
}

export interface ParticipantActionInput {
  action_id: string;
  based_on_room_seq: number;
  offer_id: string;
  schema_digest: string;
  action_type: string;
  payload: unknown;
}

export interface ParticipantBrowserRoomHead {
  room_seq: number;
  genesis_or_transition_hash: string;
  core_schema_version: string;
  pack_digest: string;
  core_state_hash: string;
  activity_state_hash: string;
  authoritative_state_hash: string;
}

export interface ParticipantBrowserDelivery {
  kind: "projection_reset" | "observation";
  body: Record<string, unknown>;
}

export interface ParticipantBrowserObservation {
  room_head: ParticipantBrowserRoomHead;
  frame_head: number;
  delivery: ParticipantBrowserDelivery[];
}

/** Read-only, verified historical projection returned by the local session proxy. */
export interface ParticipantBrowserReplay {
  requested_room_seq: number;
  room_head: ParticipantBrowserRoomHead;
  projection: Record<string, unknown>;
  projection_hash: string;
  verification: "verified";
  room_health: string;
  integrity_generation: number;
}

/** Tries a retained HttpOnly session before preserving the direct Console flow. */
export async function resumeRetainedParticipantConsole(
  startup: ParticipantConsoleStartup,
  client: Pick<ParticipantHandoffClient, "resume">,
): Promise<ParticipantConsoleStartup> {
  if (startup.kind !== "direct") return startup;
  try {
    return { kind: "retained", status: await client.resume() };
  } catch (error) {
    if (error instanceof ParticipantHandoffError && error.code === "participant_session_authority_invalid") {
      return { kind: "retained_error", error };
    }
    return startup;
  }
}

/** Cookie-backed client whose requests contain no Room, Membership, or bearer. */
export class ParticipantHandoffClient {
  private readonly endpoint: string;
  private readonly fetch: typeof globalThis.fetch;

  constructor(endpoint: string, fetch?: typeof globalThis.fetch) {
    this.endpoint = exactLoopbackEndpoint(endpoint);
    this.fetch = fetch === undefined
      ? (input, init) => globalThis.fetch(input, init)
      : (input, init) => fetch(input, init);
  }

  async redeem(handoff: string): Promise<ParticipantSessionStatus> {
    if (!HANDOFF_PATTERN.test(handoff)) {
      throw new ParticipantHandoffError(
        "participant_handoff_invalid",
        "This Participant View handoff is invalid.",
        "return_to_task_setup",
        false,
      );
    }
    return this.statusRequest("/api/v1/participant-console/handoffs:redeem", {
      method: "POST",
      credentials: "include",
      cache: "no-store",
      headers: { "X-WorldStream-Participant-Handoff": handoff },
    });
  }

  async resume(): Promise<ParticipantSessionStatus> {
    return this.statusRequest("/api/v1/participant-console/session", {
      method: "GET",
      credentials: "include",
      cache: "no-store",
    });
  }

  async observe(afterFrameSeq: number | null): Promise<ParticipantBrowserObservation> {
    const value = await this.jsonRequest("/api/v1/participant-console/session:observe", {
      method: "POST",
      credentials: "include",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ after_frame_seq: afterFrameSeq }),
    });
    if (!isParticipantObservation(value)) {
      throw new ParticipantHandoffError(
        "participant_session_invalid_response",
        "Participant View returned an invalid observation.",
        "return_to_task_setup",
        false,
      );
    }
    return value;
  }

  async act(request: ParticipantActionInput): Promise<unknown> {
    return this.jsonRequest("/api/v1/participant-console/session:act", {
      method: "POST",
      credentials: "include",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
    });
  }

  async replay(atRoomSeq: number): Promise<ParticipantBrowserReplay> {
    if (!Number.isSafeInteger(atRoomSeq) || atRoomSeq < 0) {
      throw new ParticipantHandoffError(
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
    if (!isParticipantReplay(value)) {
      throw new ParticipantHandoffError(
        "participant_replay_invalid_response",
        "Participant View returned an invalid historical Replay.",
        "return_to_task_setup",
        false,
      );
    }
    return value;
  }

  private async statusRequest(path: string, init: RequestInit): Promise<ParticipantSessionStatus> {
    const value = await this.jsonRequest(path, init);
    if (!isSessionStatus(value)) {
      throw new ParticipantHandoffError(
        "participant_session_invalid_response",
        "Participant View returned an invalid session response.",
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

  private async jsonRequest(path: string, init: RequestInit): Promise<unknown> {
    let response: Response;
    try {
      response = await this.fetch(`${this.endpoint}${path}`, init);
    } catch {
      throw new ParticipantHandoffError(
        "participant_session_unavailable",
        "Participant View could not reach the local Supervisor.",
        "reconnect",
        true,
      );
    }
    let value: unknown;
    try {
      value = await response.json();
    } catch {
      throw new ParticipantHandoffError(
        "participant_session_invalid_response",
        "Participant View returned an invalid response.",
        "return_to_task_setup",
        false,
      );
    }
    if (!response.ok) throw readSafeError(value);
    if (containsProhibitedMaterial(value)) {
      throw new ParticipantHandoffError(
        "participant_session_invalid_response",
        "Participant View returned unsafe routing or authority material.",
        "return_to_task_setup",
        false,
      );
    }
    return value;
  }
}

function exactLoopbackEndpoint(value: string): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new TypeError("Participant Console Supervisor endpoint must be a fixed loopback origin");
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
    throw new TypeError("Participant Console Supervisor endpoint must be a fixed loopback origin");
  }
  return url.origin;
}

function isSessionStatus(value: unknown): value is {
  version: typeof PARTICIPANT_SESSION_VERSION;
  state: "usable" | "disconnected";
  next_action: "continue" | "reconnect";
} {
  if (!isExactObject(value, ["version", "state", "next_action"])) return false;
  if (value.version !== PARTICIPANT_SESSION_VERSION) return false;
  return (value.state === "usable" && value.next_action === "continue")
    || (value.state === "disconnected" && value.next_action === "reconnect");
}

function isParticipantObservation(value: unknown): value is ParticipantBrowserObservation {
  if (!isExactObject(value, ["room_head", "frame_head", "delivery"])) return false;
  if (!Number.isSafeInteger(value.frame_head) || (value.frame_head as number) < 0) return false;
  if (!isParticipantRoomHead(value.room_head)) return false;
  if (!Array.isArray(value.delivery) || value.delivery.length > 10_000) return false;
  return value.delivery.every((item) => isExactObject(item, ["kind", "body"])
    && (item.kind === "projection_reset" || item.kind === "observation")
    && isRecord(item.body));
}

function isParticipantReplay(value: unknown): value is ParticipantBrowserReplay {
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
    && isParticipantRoomHead(value.room_head)
    && isRecord(value.projection)
    && typeof value.projection_hash === "string" && value.projection_hash.length > 0
    && value.verification === "verified"
    && typeof value.room_health === "string" && value.room_health.length > 0
    && Number.isSafeInteger(value.integrity_generation) && (value.integrity_generation as number) >= 0;
}

function isParticipantRoomHead(value: unknown): value is ParticipantBrowserRoomHead {
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

function readSafeError(value: unknown): ParticipantHandoffError {
  if (
    isExactObject(value, ["code", "message", "next_action", "retryable"])
    && typeof value.code === "string"
    && typeof value.message === "string"
    && (value.next_action === "return_to_task_setup" || value.next_action === "retry" || value.next_action === "reconnect")
    && typeof value.retryable === "boolean"
    && !containsProhibitedMaterial(value)
  ) {
    return new ParticipantHandoffError(value.code, value.message, value.next_action, value.retryable);
  }
  return new ParticipantHandoffError(
    "participant_session_invalid_response",
    "Participant View returned an invalid error response.",
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
