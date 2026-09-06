export type PublicJson =
  | null
  | boolean
  | number
  | string
  | readonly PublicJson[]
  | { readonly [key: string]: PublicJson };

export interface PublicActivityIdentity {
  readonly listing_key: string;
  readonly title: string;
  readonly description: string;
  readonly listing_revision: string;
  readonly pack: {
    readonly id: string;
    readonly version: string;
    readonly revision: string;
  };
}

export type PublicParticipant =
  | {
      readonly seat_label: string;
      readonly role: string;
      readonly kind: "human";
      readonly identity: PublicPersonIdentity;
    }
  | {
      readonly seat_label: string;
      readonly role: string;
      readonly kind: "external_agent";
      readonly identity: PublicPersonIdentity;
      readonly notice: "External agent — unverified";
    }
  | {
      readonly seat_label: string;
      readonly role: string;
      readonly kind: "house_agent";
      readonly notice: "Exhibition — platform-supplied agents";
      readonly house_agent: PublicHouseAgent;
    };

export type PublicPersonIdentity =
  | { readonly kind: "pseudonym"; readonly label: string }
  | {
      readonly kind: "github";
      readonly login: string;
      readonly avatar_url?: string;
      readonly fallback_label: string;
    };

export interface PublicHouseAgent {
  readonly display_name: string;
  readonly revision_digest: string;
  readonly route: {
    readonly gateway: "openrouter";
    readonly provider_slug: string;
    readonly model_slug: string;
  };
  readonly allowance: {
    readonly model_call_attempts: number;
    readonly total_input_tokens: number;
    readonly total_output_tokens: number;
    readonly input_tokens_per_call: number;
    readonly output_tokens_per_call: number;
    readonly concurrent_calls: number;
    readonly call_timeout_seconds: number;
  };
}

interface PublicRunBase {
  readonly version: "public_run.v1";
  readonly public_id: string;
  readonly activity: PublicActivityIdentity;
  readonly started_at: string;
  readonly evidence: {
    readonly class: "unranked" | "exhibition_platform_house_agents";
    readonly label: "Unranked activity" | "Exhibition — platform-supplied agents";
  };
  readonly participants: readonly PublicParticipant[];
}

export interface PublicLiveRun extends PublicRunBase {
  readonly state: "live";
  readonly live: { readonly available: false };
}

export interface PublicResultRun extends PublicRunBase {
  readonly state: "result";
  readonly completed_at: string;
  readonly result: { readonly [key: string]: PublicJson };
}

export type PublicRun =
  | { readonly version: "public_run.v1"; readonly state: "unavailable" }
  | PublicLiveRun
  | PublicResultRun;

export interface RecentResults {
  readonly version: "recent_results.v1";
  readonly activity: "agent-heist";
  readonly order: "newest_first";
  readonly maximum: 20;
  readonly results: readonly PublicResultRun[];
}

export interface PublicRunData {
  readPublicRun(publicId: string): Promise<PublicRun>;
  listRecentResults(limit: number): Promise<RecentResults>;
}

const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:/-]{0,191}$/u;
const BLAKE3 = /^blake3:[0-9a-f]{64}$/u;
const PUBLIC_ID = /^[0-9a-f]{32}$/u;
const FORBIDDEN_PUBLIC_KEYS = new Set([
  "account_id",
  "activity_run_id",
  "auth_user_id",
  "canonical_genesis_evidence",
  "canonical_payload",
  "credential",
  "entry_selector",
  "handoff",
  "host_installation_id",
  "invitation_token",
  "launch_request_id",
  "membership_id",
  "principal_id",
  "prompt",
  "provider_response",
  "refresh_token",
  "replay",
  "room_id",
  "room_setup_operation_id",
  "service_scope_digest",
]);

/** Validates the complete server-produced DTO before it crosses the BFF. */
export function readPublicRunDto(value: unknown): PublicRun {
  const record = recordValue(value);
  if (record.version !== "public_run.v1") return invalid();
  if (record.state === "unavailable") {
    exactRecord(value, ["version", "state"]);
    return { version: "public_run.v1", state: "unavailable" };
  }
  if (record.state === "live") {
    const live = exactRecord(value, [
      "version", "state", "public_id", "activity", "started_at", "evidence",
      "participants", "live",
    ]);
    const liveState = exactRecord(live.live, ["available"]);
    if (liveState.available !== false) return invalid();
    return {
      version: "public_run.v1",
      state: "live",
      public_id: publicId(live.public_id),
      activity: activity(live.activity),
      started_at: dateTime(live.started_at),
      evidence: evidence(live.evidence),
      participants: participants(live.participants),
      live: { available: false },
    };
  }
  if (record.state === "result") {
    const result = exactRecord(value, [
      "version", "state", "public_id", "activity", "started_at", "completed_at",
      "evidence", "participants", "result",
    ]);
    return {
      version: "public_run.v1",
      state: "result",
      public_id: publicId(result.public_id),
      activity: activity(result.activity),
      started_at: dateTime(result.started_at),
      completed_at: dateTime(result.completed_at),
      evidence: evidence(result.evidence),
      participants: participants(result.participants),
      result: publicObject(result.result),
    };
  }
  return invalid();
}

/** Validates the bounded, Agent-Heist-only chronological result envelope. */
export function readRecentResultsDto(value: unknown): RecentResults {
  const record = exactRecord(value, [
    "version", "activity", "order", "maximum", "results",
  ]);
  if (
    record.version !== "recent_results.v1" ||
    record.activity !== "agent-heist" ||
    record.order !== "newest_first" ||
    record.maximum !== 20 ||
    !Array.isArray(record.results) ||
    record.results.length > 20
  ) return invalid();
  const results = record.results.map((item) => {
    const parsed = readPublicRunDto(item);
    if (parsed.state !== "result" || parsed.activity.pack.id !== "worldstream.agent-heist") {
      return invalid();
    }
    return parsed;
  });
  for (let index = 1; index < results.length; index += 1) {
    const prior = Date.parse(results[index - 1]!.completed_at);
    const current = Date.parse(results[index]!.completed_at);
    if (current > prior) return invalid();
  }
  return {
    version: "recent_results.v1",
    activity: "agent-heist",
    order: "newest_first",
    maximum: 20,
    results,
  };
}

function activity(value: unknown): PublicActivityIdentity {
  const record = exactRecord(value, [
    "listing_key", "title", "description", "listing_revision", "pack",
  ]);
  const pack = exactRecord(record.pack, ["id", "version", "revision"]);
  return {
    listing_key: boundedIdentifier(record.listing_key),
    title: boundedText(record.title, 128),
    description: boundedText(record.description, 1_024),
    listing_revision: digest(record.listing_revision),
    pack: {
      id: boundedIdentifier(pack.id),
      version: boundedText(pack.version, 64),
      revision: digest(pack.revision),
    },
  };
}

function evidence(value: unknown): PublicRunBase["evidence"] {
  const record = exactRecord(value, ["class", "label"]);
  if (record.class === "unranked" && record.label === "Unranked activity") {
    return { class: "unranked", label: "Unranked activity" };
  }
  if (
    record.class === "exhibition_platform_house_agents" &&
    record.label === "Exhibition — platform-supplied agents"
  ) {
    return {
      class: "exhibition_platform_house_agents",
      label: "Exhibition — platform-supplied agents",
    };
  }
  return invalid();
}

function participants(value: unknown): readonly PublicParticipant[] {
  if (!Array.isArray(value) || value.length > 32) return invalid();
  return value.map((item): PublicParticipant => {
    const base = recordValue(item);
    if (base.kind === "human") {
      const record = exactRecord(item, ["seat_label", "role", "kind", "identity"]);
      return {
        seat_label: boundedText(record.seat_label, 128),
        role: boundedIdentifier(record.role),
        kind: "human",
        identity: personIdentity(record.identity),
      };
    }
    if (base.kind === "external_agent") {
      const record = exactRecord(item, [
        "seat_label", "role", "kind", "identity", "notice",
      ]);
      if (record.notice !== "External agent — unverified") return invalid();
      return {
        seat_label: boundedText(record.seat_label, 128),
        role: boundedIdentifier(record.role),
        kind: "external_agent",
        identity: personIdentity(record.identity),
        notice: "External agent — unverified",
      };
    }
    if (base.kind === "house_agent") {
      const record = exactRecord(item, [
        "seat_label", "role", "kind", "notice", "house_agent",
      ]);
      if (record.notice !== "Exhibition — platform-supplied agents") return invalid();
      return {
        seat_label: boundedText(record.seat_label, 128),
        role: boundedIdentifier(record.role),
        kind: "house_agent",
        notice: "Exhibition — platform-supplied agents",
        house_agent: houseAgent(record.house_agent),
      };
    }
    return invalid();
  });
}

function personIdentity(value: unknown): PublicPersonIdentity {
  const base = recordValue(value);
  if (base.kind === "pseudonym") {
    const record = exactRecord(value, ["kind", "label"]);
    return { kind: "pseudonym", label: boundedText(record.label, 128) };
  }
  if (base.kind === "github") {
    const record = exactRecord(value, ["kind", "login", "fallback_label"], ["avatar_url"]);
    const avatar = record.avatar_url === undefined
      ? undefined
      : secureUrl(record.avatar_url);
    return {
      kind: "github",
      login: boundedText(record.login, 128),
      ...(avatar === undefined ? {} : { avatar_url: avatar }),
      fallback_label: boundedText(record.fallback_label, 128),
    };
  }
  return invalid();
}

function houseAgent(value: unknown): PublicHouseAgent {
  const record = exactRecord(value, ["display_name", "revision_digest", "route", "allowance"]);
  const route = exactRecord(record.route, ["gateway", "provider_slug", "model_slug"]);
  if (route.gateway !== "openrouter") return invalid();
  const allowance = exactRecord(record.allowance, [
    "model_call_attempts", "total_input_tokens", "total_output_tokens",
    "input_tokens_per_call", "output_tokens_per_call", "concurrent_calls",
    "call_timeout_seconds",
  ]);
  return {
    display_name: boundedText(record.display_name, 128),
    revision_digest: digest(record.revision_digest),
    route: {
      gateway: "openrouter",
      provider_slug: boundedIdentifier(route.provider_slug),
      model_slug: boundedIdentifier(route.model_slug),
    },
    allowance: {
      model_call_attempts: boundedInteger(allowance.model_call_attempts, 1_000),
      total_input_tokens: boundedInteger(allowance.total_input_tokens, 10_000_000),
      total_output_tokens: boundedInteger(allowance.total_output_tokens, 10_000_000),
      input_tokens_per_call: boundedInteger(allowance.input_tokens_per_call, 1_000_000),
      output_tokens_per_call: boundedInteger(allowance.output_tokens_per_call, 1_000_000),
      concurrent_calls: boundedInteger(allowance.concurrent_calls, 100),
      call_timeout_seconds: boundedInteger(allowance.call_timeout_seconds, 3_600),
    },
  };
}

function publicObject(value: unknown): { readonly [key: string]: PublicJson } {
  const parsed = publicJson(value, 0);
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return invalid();
  if (JSON.stringify(parsed).length > 65_536) return invalid();
  return parsed as { readonly [key: string]: PublicJson };
}

function publicJson(value: unknown, depth: number): PublicJson {
  if (depth > 8) return invalid();
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    if (typeof value === "string" && value.length > 2_048) return invalid();
    return value;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value) || !Number.isSafeInteger(value)) return invalid();
    return value;
  }
  if (Array.isArray(value)) {
    if (value.length > 128) return invalid();
    return value.map((item) => publicJson(item, depth + 1));
  }
  const record = recordValue(value);
  const entries = Object.entries(record);
  if (entries.length > 64) return invalid();
  const parsed: Record<string, PublicJson> = {};
  for (const [key, item] of entries) {
    if (FORBIDDEN_PUBLIC_KEYS.has(key)) return invalid();
    parsed[key] = publicJson(item, depth + 1);
  }
  return parsed;
}

function exactRecord(
  value: unknown,
  required: readonly string[],
  optional: readonly string[] = [],
): Record<string, unknown> {
  const record = recordValue(value);
  const keys = Object.keys(record);
  if (
    required.some((key) => !(key in record)) ||
    keys.some((key) => !required.includes(key) && !optional.includes(key))
  ) return invalid();
  return record;
}

function recordValue(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return invalid();
  return value as Record<string, unknown>;
}

function boundedIdentifier(value: unknown): string {
  const text = boundedText(value, 192);
  if (!IDENTIFIER.test(text)) return invalid();
  return text;
}

function boundedText(value: unknown, maximum: number): string {
  if (
    typeof value !== "string" || value.length === 0 || value.length > maximum ||
    /[\u0000-\u001f\u007f]/u.test(value)
  ) return invalid();
  return value;
}

function boundedInteger(value: unknown, maximum: number): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0 || (value as number) > maximum) {
    return invalid();
  }
  return value as number;
}

function digest(value: unknown): string {
  if (typeof value !== "string" || !BLAKE3.test(value)) return invalid();
  return value;
}

function publicId(value: unknown): string {
  if (typeof value !== "string" || !PUBLIC_ID.test(value)) return invalid();
  return value;
}

function dateTime(value: unknown): string {
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value))) return invalid();
  return value;
}

function secureUrl(value: unknown): string {
  const text = boundedText(value, 2_048);
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    return invalid();
  }
  if (
    url.protocol !== "https:" || url.username !== "" || url.password !== "" ||
    url.hash !== ""
  ) return invalid();
  return url.toString();
}

function invalid(): never {
  throw new Error("public_run_dto_rejected");
}
