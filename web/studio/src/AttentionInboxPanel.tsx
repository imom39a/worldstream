import type { AttentionInboxResponse, AttentionItem } from "./attentionInbox";

export function AttentionInboxPanel({
  inbox,
  notificationsEnabled,
  notificationsAvailable,
  onNotificationPreference,
}: {
  inbox: AttentionInboxResponse | null;
  notificationsEnabled: boolean;
  notificationsAvailable: boolean;
  onNotificationPreference?: (enabled: boolean) => void;
}) {
  const tasks = inbox?.items.filter((item) => item.target_kind === "task") ?? [];
  const processes = inbox?.items.filter((item) => item.target_kind === "process") ?? [];
  return (
    <section className="attention-inbox" id="attention" aria-labelledby="attention-title">
      <div className="attention-inbox-heading">
        <div>
          <p className="eyebrow">Home · Attention</p>
          <h2 id="attention-title">Needs your attention</h2>
          <p>Derived from current and reconciled operational status.</p>
        </div>
        <label>
          <input
            type="checkbox"
            checked={notificationsEnabled}
            disabled={!notificationsAvailable}
            onChange={(event) => onNotificationPreference?.(event.currentTarget.checked)}
          />
          Local notifications
        </label>
      </div>
      {inbox === null ? (
        <div className="runner-unavailable" role="status">
          <strong>Attention status unavailable</strong>
          <p>Retry status; no cached condition is presented as current fact.</p>
        </div>
      ) : inbox.items.length === 0 ? (
        <p className="status-message">No active conditions need operator attention.</p>
      ) : (
        <div className="attention-groups">
          <AttentionGroup title="Tasks" items={tasks} />
          <AttentionGroup title="Processes" items={processes} />
        </div>
      )}
      {inbox !== null && inbox.recently_resolved.length > 0 ? (
        <details className="attention-history">
          <summary>Recently resolved ({inbox.recently_resolved.length})</summary>
          <ul>
            {inbox.recently_resolved.map((item) => (
              <li key={item.attention_id}>{conditionLabel(item.condition)} · {item.target_id}</li>
            ))}
          </ul>
        </details>
      ) : null}
      <p className="attention-boundary-note">
        Closing a notification changes no Task, Room, lease, or process state.
      </p>
    </section>
  );
}

function AttentionGroup({ title, items }: { title: string; items: AttentionItem[] }) {
  if (items.length === 0) return null;
  return (
    <section aria-label={`${title} needing attention`}>
      <h3>{title}</h3>
      <div className="attention-item-list">
        {items.map((item) => (
          <article key={item.attention_id} className={`attention-item is-${item.freshness}`}>
            <div>
              <strong>{item.title}</strong>
              <span className={`freshness-chip is-${item.freshness}`}>{freshnessLabel(item.freshness)}</span>
            </div>
            <small>{item.target_id} · Last observed {new Date(item.last_seen_at_unix_ms).toLocaleString("en-US")}</small>
            <p>{item.reason}</p>
            <p>{item.next_action}</p>
            <a href={item.deep_link}>Open safe detail</a>
          </article>
        ))}
      </div>
    </section>
  );
}

function freshnessLabel(value: AttentionItem["freshness"]): string {
  if (value === "live") return "Live";
  if (value === "stale") return "Stale · last known";
  return "Unavailable";
}

function conditionLabel(value: AttentionItem["condition"]): string {
  return value.replaceAll("_", " ").replace(/^./, (character) => character.toUpperCase());
}
