export type AttentionFreshness = "live" | "stale" | "unavailable";
export type AttentionTargetKind = "task" | "process";
export type AttentionCondition =
  | "daemon_lifecycle"
  | "task_setup"
  | "task_readiness"
  | "task_launch"
  | "runner_presence"
  | "runner_compatibility"
  | "runner_capacity"
  | "activation_backlog"
  | "activation_lease"
  | "runner_restart"
  | "managed_agent_host"
  | "room_creation"
  | "backup_operation";

export interface AttentionItem {
  attention_id: string;
  target_kind: AttentionTargetKind;
  condition: AttentionCondition;
  target_id: string;
  title: string;
  reason: string;
  freshness: AttentionFreshness;
  deep_link: "#operations" | "#task-setup" | "#runner-attention" | "#tasks" | "#room-creation" | "#backups";
  next_action: string;
  first_seen_at_unix_ms: number;
  last_seen_at_unix_ms: number;
}

export interface ResolvedAttentionItem {
  attention_id: string;
  target_kind: AttentionTargetKind;
  condition: AttentionCondition;
  target_id: string;
  resolved_at_unix_ms: number;
}

export interface AttentionInboxResponse {
  schema: "worldstream/studio-attention-inbox/v1";
  observed_at_unix_ms: number;
  items: AttentionItem[];
  recently_resolved: ResolvedAttentionItem[];
}

export interface AttentionNotificationAdapter {
  readonly permission: NotificationPermission;
  requestPermission(): Promise<NotificationPermission>;
  show(title: string, options: { body: string; tag: string; data: { deepLink: AttentionItem["deep_link"] } }): void;
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
type StorageLike = Pick<Storage, "getItem" | "setItem">;

const preferenceKey = "worldstream.studio.attention-notifications.v1";
const deliveryKey = "worldstream.studio.attention-notification-delivery.v1";
const maximumItems = 256;
const maximumHistory = 16;
const rateWindowMs = 60_000;
const maximumPerWindow = 3;
const allowedLinks = new Set([
  "#operations", "#task-setup", "#runner-attention", "#tasks", "#room-creation", "#backups",
]);
const sensitive = [
  "bearer", "wsb1:", "token", "secret", "credential", "password", "prompt", "response",
  "memory", "payload", "invocation", "context", "member_id", "activation_id", "claim_id",
];

export async function loadAttentionInbox(fetcher: Fetcher = fetch): Promise<AttentionInboxResponse | null> {
  try {
    const response = await fetcher("/api/v1/attention-inbox", {
      cache: "no-store",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isAttentionInbox(value) ? value : null;
  } catch {
    return null;
  }
}

export function notificationPreference(storage: StorageLike = window.localStorage): "enabled" | "disabled" {
  return storage.getItem(preferenceKey) === "enabled" ? "enabled" : "disabled";
}

export function disableAttentionNotifications(storage: StorageLike = window.localStorage): void {
  storage.setItem(preferenceKey, "disabled");
}

export async function enableAttentionNotifications(
  adapter: AttentionNotificationAdapter,
  storage: StorageLike = window.localStorage,
): Promise<boolean> {
  const permission = adapter.permission === "granted"
    ? "granted"
    : await adapter.requestPermission();
  const enabled = permission === "granted";
  storage.setItem(preferenceKey, enabled ? "enabled" : "disabled");
  return enabled;
}

export function notifyForAttention(
  inbox: AttentionInboxResponse,
  adapter: AttentionNotificationAdapter,
  storage: StorageLike = window.localStorage,
  now: number = Date.now(),
): number {
  if (notificationPreference(storage) !== "enabled" || adapter.permission !== "granted" ||
    !Number.isSafeInteger(now) || now < 0) return 0;
  const state = loadDeliveryState(storage, now);
  if (now - state.window_started_at_ms >= rateWindowMs) {
    state.window_started_at_ms = now;
    state.sent_in_window = 0;
  }
  let sent = 0;
  for (const item of inbox.items) {
    if (state.sent_in_window >= maximumPerWindow) break;
    const transition = `${item.attention_id}:${item.first_seen_at_unix_ms}`;
    if (state.transitions.includes(transition)) continue;
    adapter.show(safeText(item.title, "WorldStream needs attention"), {
      body: `${freshnessLabel(item.freshness)}: ${safeText(item.reason, "An operational condition needs attention.")} ${safeText(item.next_action, "Open Studio for the safe next action.")}`,
      tag: transition,
      data: { deepLink: item.deep_link },
    });
    state.transitions.push(transition);
    state.sent_in_window += 1;
    sent += 1;
  }
  state.transitions = state.transitions.slice(-maximumItems);
  storage.setItem(deliveryKey, JSON.stringify(state));
  return sent;
}

export function browserAttentionNotifications(): AttentionNotificationAdapter | null {
  if (typeof window === "undefined" || typeof Notification === "undefined") return null;
  return {
    get permission() { return Notification.permission; },
    requestPermission: () => Notification.requestPermission(),
    show(title, options) {
      const notification = new Notification(title, {
        body: options.body,
        tag: options.tag,
        data: options.data,
      });
      notification.onclick = () => {
        window.location.hash = options.data.deepLink;
        window.focus();
        notification.close();
      };
    },
  };
}

export function isAttentionInbox(value: unknown): value is AttentionInboxResponse {
  if (containsSensitive(value) || !exact(value, ["schema", "observed_at_unix_ms", "items", "recently_resolved"]) ||
    value.schema !== "worldstream/studio-attention-inbox/v1" || !timestamp(value.observed_at_unix_ms) ||
    !Array.isArray(value.items) || value.items.length > maximumItems || !value.items.every(isItem) ||
    !Array.isArray(value.recently_resolved) || value.recently_resolved.length > maximumHistory ||
    !value.recently_resolved.every(isResolved)) return false;
  const items = value.items as AttentionItem[];
  const resolved = value.recently_resolved as ResolvedAttentionItem[];
  return unique(items.map((item) => item.attention_id)) &&
    unique(resolved.map((item) => item.attention_id)) &&
    items.every((item) => item.first_seen_at_unix_ms <= Number(value.observed_at_unix_ms) &&
      item.last_seen_at_unix_ms <= Number(value.observed_at_unix_ms)) &&
    resolved.every((item) => item.resolved_at_unix_ms <= Number(value.observed_at_unix_ms));
}

function isItem(value: unknown): value is AttentionItem {
  return exact(value, [
    "attention_id", "target_kind", "condition", "target_id", "title", "reason", "freshness",
    "deep_link", "next_action", "first_seen_at_unix_ms", "last_seen_at_unix_ms",
  ]) && digest(value.attention_id) && isTargetKind(value.target_kind) && isCondition(value.condition) &&
    text(value.target_id, 128) && text(value.title, 512) && text(value.reason, 512) &&
    isFreshness(value.freshness) && typeof value.deep_link === "string" && allowedLinks.has(value.deep_link) &&
    text(value.next_action, 512) && timestamp(value.first_seen_at_unix_ms) && timestamp(value.last_seen_at_unix_ms);
}

function isResolved(value: unknown): value is ResolvedAttentionItem {
  return exact(value, ["attention_id", "target_kind", "condition", "target_id", "resolved_at_unix_ms"]) &&
    digest(value.attention_id) && isTargetKind(value.target_kind) && isCondition(value.condition) &&
    text(value.target_id, 128) && timestamp(value.resolved_at_unix_ms);
}

function containsSensitive(value: unknown, depth = 0): boolean {
  if (depth > 12) return true;
  if (typeof value === "string") {
    const lower = value.toLowerCase();
    return sensitive.some((needle) => lower.includes(needle));
  }
  if (Array.isArray(value)) return value.some((item) => containsSensitive(item, depth + 1));
  if (!record(value)) return false;
  return Object.entries(value).some(([key, nested]) =>
    sensitive.some((needle) => key.toLowerCase().includes(needle)) || containsSensitive(nested, depth + 1));
}

function safeText(value: string, fallback: string): string {
  const lower = value.toLowerCase();
  return text(value, 512) && !sensitive.some((needle) => lower.includes(needle)) ? value : fallback;
}

function loadDeliveryState(storage: StorageLike, now: number): DeliveryState {
  try {
    const parsed: unknown = JSON.parse(storage.getItem(deliveryKey) ?? "null");
    if (exact(parsed, ["version", "window_started_at_ms", "sent_in_window", "transitions"]) &&
      parsed.version === 1 && Number.isSafeInteger(parsed.window_started_at_ms) &&
      Number(parsed.window_started_at_ms) >= 0 && Number(parsed.window_started_at_ms) <= now &&
      Number.isSafeInteger(parsed.sent_in_window) && Number(parsed.sent_in_window) >= 0 &&
      Number(parsed.sent_in_window) <= maximumPerWindow && Array.isArray(parsed.transitions) &&
      parsed.transitions.length <= maximumItems && parsed.transitions.every((item) => typeof item === "string" && item.length <= 100)) {
      return parsed as unknown as DeliveryState;
    }
  } catch { /* fail closed to a fresh bounded local window */ }
  return { version: 1, window_started_at_ms: now, sent_in_window: 0, transitions: [] };
}

interface DeliveryState {
  version: 1;
  window_started_at_ms: number;
  sent_in_window: number;
  transitions: string[];
}

function exact(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return record(value) && Object.keys(value).length === keys.length && keys.every((key) => key in value);
}
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function text(value: unknown, maximum: number): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= maximum && !/[\0\r\n]/.test(value);
}
function digest(value: unknown): value is string {
  return typeof value === "string" && /^blake3:[0-9a-f]{64}$/.test(value);
}
function timestamp(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) > 0;
}
function isTargetKind(value: unknown): value is AttentionTargetKind {
  return value === "task" || value === "process";
}
function isFreshness(value: unknown): value is AttentionFreshness {
  return value === "live" || value === "stale" || value === "unavailable";
}
function isCondition(value: unknown): value is AttentionCondition {
  return [
    "daemon_lifecycle", "task_setup", "task_readiness", "task_launch", "runner_presence",
    "runner_compatibility", "runner_capacity", "activation_backlog", "activation_lease",
    "runner_restart", "managed_agent_host", "room_creation", "backup_operation",
  ].includes(String(value));
}
function unique(values: string[]): boolean { return new Set(values).size === values.length; }
function freshnessLabel(value: AttentionFreshness): string {
  return value === "live" ? "Live" : value === "stale" ? "Stale last-known state" : "Unavailable state";
}
