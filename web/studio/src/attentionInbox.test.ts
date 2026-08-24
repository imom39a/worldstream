import { describe, expect, it, vi } from "vitest";

import {
  disableAttentionNotifications,
  enableAttentionNotifications,
  isAttentionInbox,
  loadAttentionInbox,
  notificationPreference,
  notifyForAttention,
  type AttentionNotificationAdapter,
} from "./attentionInbox";

describe("attention inbox boundary and local notifications", () => {
  it("loads exact safe items and rejects unknown, malformed, or sensitive nested data", async () => {
    const value = inbox();
    expect(isAttentionInbox(value)).toBe(true);
    expect(isAttentionInbox({ ...value, command: "ps" })).toBe(false);
    expect(isAttentionInbox({ ...value, items: [{ ...value.items[0], detail: { invocation_payload: "private" } }] })).toBe(false);
    expect(isAttentionInbox({ ...value, items: [{ ...value.items[0], reason: "Bearer wsb1:private" }] })).toBe(false);
    expect(isAttentionInbox({ ...value, items: [{ ...value.items[0], deep_link: "https://evil.invalid" }] })).toBe(false);
    expect(isAttentionInbox({ ...value, items: [value.items[0], value.items[0]] })).toBe(false);
    expect(isAttentionInbox({
      ...value,
      items: [{ ...value.items[0], first_seen_at_unix_ms: 2_000, last_seen_at_unix_ms: 500 }],
    })).toBe(true);
    expect(isAttentionInbox({
      ...value,
      items: [
        { ...value.items[0], condition: "room_creation", deep_link: "#room-creation" },
        {
          ...value.items[0],
          attention_id: `blake3:${"b".repeat(64)}`,
          condition: "backup_operation",
          deep_link: "#backups",
        },
      ],
    })).toBe(true);
    const fetcher = vi.fn(async () => new Response(JSON.stringify(value), { status: 200 }));
    expect((await loadAttentionInbox(fetcher))?.items).toHaveLength(1);
    expect(fetcher).toHaveBeenCalledWith("/api/v1/attention-inbox", expect.objectContaining({ cache: "no-store" }));
  });

  it("persists preference locally and asks permission only on explicit enable", async () => {
    const storage = memoryStorage();
    const adapter = fakeNotifications("default");
    expect(notificationPreference(storage)).toBe("disabled");
    expect(notifyForAttention(inbox(), adapter, storage, 1_000)).toBe(0);
    expect(adapter.requestPermission).not.toHaveBeenCalled();

    adapter.permission = "granted";
    expect(await enableAttentionNotifications(adapter, storage)).toBe(true);
    expect(adapter.requestPermission).not.toHaveBeenCalled();
    expect(notificationPreference(storage)).toBe("enabled");
    disableAttentionNotifications(storage);
    expect(notificationPreference(storage)).toBe("disabled");
  });

  it("deduplicates stable transitions, rate limits, and permits a reopened transition", () => {
    const storage = memoryStorage();
    storage.setItem("worldstream.studio.attention-notifications.v1", "enabled");
    const adapter = fakeNotifications("granted");
    const value = inbox();
    value.items = [0, 1, 2, 3].map((index) => ({
      ...value.items[0],
      attention_id: `blake3:${String(index).repeat(64)}`,
      first_seen_at_unix_ms: 1_000,
    }));
    expect(notifyForAttention(value, adapter, storage, 1_000)).toBe(3);
    expect(notifyForAttention(value, adapter, storage, 2_000)).toBe(0);
    expect(adapter.show).toHaveBeenCalledTimes(3);

    value.items[0].first_seen_at_unix_ms = 70_000;
    value.items[0].last_seen_at_unix_ms = 70_000;
    value.observed_at_unix_ms = 70_000;
    expect(notifyForAttention(value, adapter, storage, 70_000)).toBe(2);
    expect(adapter.show).toHaveBeenCalledTimes(5);
  });

  it("redacts notification text and dismissal has no server mutation seam", () => {
    const storage = memoryStorage();
    storage.setItem("worldstream.studio.attention-notifications.v1", "enabled");
    const adapter = fakeNotifications("granted");
    const value = inbox();
    value.items[0].reason = "Bearer wsb1:private";
    value.items[0].next_action = "Reveal secret_reference";
    expect(notifyForAttention(value, adapter, storage, 1_000)).toBe(1);
    const call = adapter.show.mock.calls[0];
    expect(JSON.stringify(call).toLowerCase()).not.toContain("wsb1:");
    expect(JSON.stringify(call).toLowerCase()).not.toContain("secret_reference");
    expect(call?.[2]).toBeUndefined();
  });
});

function inbox() {
  return {
    schema: "worldstream/studio-attention-inbox/v1" as const,
    observed_at_unix_ms: 2_000,
    items: [{
      attention_id: `blake3:${"a".repeat(64)}`,
      target_kind: "process" as const,
      condition: "runner_capacity" as const,
      target_id: "01ARZ3NDEKTSV4RRFFQ69G5FB0",
      title: "Runner capacity is exhausted",
      reason: "All advertised capacity is in use.",
      freshness: "stale" as const,
      deep_link: "#runner-attention" as const,
      next_action: "Open Runner attention.",
      first_seen_at_unix_ms: 1_000,
      last_seen_at_unix_ms: 2_000,
    }],
    recently_resolved: [],
  };
}

function memoryStorage() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); },
  };
}

function fakeNotifications(permission: NotificationPermission) {
  return {
    permission,
    requestPermission: vi.fn(async () => "granted" as NotificationPermission),
    show: vi.fn(),
  } satisfies AttentionNotificationAdapter & { permission: NotificationPermission };
}
