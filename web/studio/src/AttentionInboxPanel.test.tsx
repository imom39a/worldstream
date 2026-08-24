import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { AttentionInboxPanel } from "./AttentionInboxPanel";
import type { AttentionInboxResponse } from "./attentionInbox";

describe("Home attention inbox", () => {
  it("groups Tasks and processes with freshness, time, reason, and closed deep links", () => {
    const dom = renderToStaticMarkup(
      <AttentionInboxPanel
        inbox={inbox()}
        notificationsEnabled
        notificationsAvailable
      />,
    );
    expect(dom).toContain("Needs your attention");
    expect(dom).toContain("Tasks");
    expect(dom).toContain("Processes");
    expect(dom).toContain("Stale · last known");
    expect(dom).toContain("Last observed");
    expect(dom).toContain("Open safe detail");
    expect(dom).toContain('href="#runner-attention"');
    expect(dom).toContain('href="#room-creation"');
    expect(dom).toContain('href="#backups"');
    expect(dom).toContain("Room creation needs attention");
    expect(dom).toContain("Backup operation failed");
    expect(dom).toContain("Recently resolved (1)");
    expect(dom).toContain("Closing a notification changes no Task, Room, lease, or process state.");
  });

  it("shows closed unavailable and empty states without private material", () => {
    const unavailable = renderToStaticMarkup(
      <AttentionInboxPanel inbox={null} notificationsEnabled={false} notificationsAvailable={false} />,
    );
    expect(unavailable).toContain("Attention status unavailable");
    const empty = inbox();
    empty.items = [];
    empty.recently_resolved = [];
    const dom = renderToStaticMarkup(
      <AttentionInboxPanel inbox={empty} notificationsEnabled={false} notificationsAvailable />,
    );
    expect(dom).toContain("No active conditions");
    for (const forbidden of ["bearer", "secret_reference", "invocation", "payload", "prompt", "memory"]) {
      expect(dom.toLowerCase()).not.toContain(forbidden);
    }
  });
});

function inbox(): AttentionInboxResponse {
  return {
    schema: "worldstream/studio-attention-inbox/v1",
    observed_at_unix_ms: 2_000,
    items: [
      {
        attention_id: `blake3:${"a".repeat(64)}`,
        target_kind: "task",
        condition: "activation_lease",
        target_id: "task-01:navigator-agent",
        title: "Activation lease is delayed",
        reason: "A retained lease is waiting for reconciliation.",
        freshness: "stale",
        deep_link: "#tasks",
        next_action: "Open Task detail.",
        first_seen_at_unix_ms: 1_000,
        last_seen_at_unix_ms: 2_000,
      },
      {
        attention_id: `blake3:${"b".repeat(64)}`,
        target_kind: "process",
        condition: "runner_capacity",
        target_id: "runner-01",
        title: "Runner capacity is exhausted",
        reason: "All advertised capacity is in use.",
        freshness: "live",
        deep_link: "#runner-attention",
        next_action: "Restore compatible capacity.",
        first_seen_at_unix_ms: 1_000,
        last_seen_at_unix_ms: 2_000,
      },
      {
        attention_id: `blake3:${"d".repeat(64)}`,
        target_kind: "process",
        condition: "room_creation",
        target_id: "draft-01",
        title: "Room creation needs attention",
        reason: "The daemon rejected the approved creation request.",
        freshness: "live",
        deep_link: "#room-creation",
        next_action: "Review the safe reason and correct the draft.",
        first_seen_at_unix_ms: 1_000,
        last_seen_at_unix_ms: 2_000,
      },
      {
        attention_id: `blake3:${"e".repeat(64)}`,
        target_kind: "process",
        condition: "backup_operation",
        target_id: "backup-01",
        title: "Backup operation failed",
        reason: "The backup did not pass verification.",
        freshness: "unavailable",
        deep_link: "#backups",
        next_action: "Open Backups and retry with the approved profile.",
        first_seen_at_unix_ms: 1_000,
        last_seen_at_unix_ms: 2_000,
      },
    ],
    recently_resolved: [{
      attention_id: `blake3:${"c".repeat(64)}`,
      target_kind: "task",
      condition: "task_setup",
      target_id: "task-02",
      resolved_at_unix_ms: 1_500,
    }],
  };
}
