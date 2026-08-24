import type { RoomCreationStatus } from "./roomCreation";
import "./roomCreation.css";

export function RoomCreationOperation({
  status,
  statusAvailable = true,
  reviewed,
  loading,
  onStart,
  onRetry,
}: {
  status: RoomCreationStatus | null;
  statusAvailable?: boolean;
  reviewed: boolean;
  loading: boolean;
  onStart?: () => void;
  onRetry?: () => void;
}) {
  return (
    <section className="room-creation-operation" id="room-creation" aria-busy={loading}>
      <div className="room-creation-heading">
        <div>
          <p className="eyebrow">Build · Room creation</p>
          <h2>Durable exact-once creation</h2>
        </div>
        {status ? <span className={`room-creation-state is-${status.state}`}>{stateLabel(status.state)}</span> : null}
      </div>

      {!statusAvailable ? (
        <div className="room-creation-attention" role="status">
          <strong>Room creation status is unavailable</strong>
          <p>Current operation state could not be verified. No prior success is shown as current.</p>
        </div>
      ) : status === null ? (
        reviewed ? (
          <div className="room-creation-callout">
            <p>The exact reviewed snapshot and stable operation identity are persisted before the daemon call.</p>
            <button type="button" disabled={loading} onClick={onStart}>
              {loading ? "Persisting creation intent…" : "Create from reviewed draft"}
            </button>
          </div>
        ) : (
          <p className="room-creation-guidance">Complete and save Review before starting Room creation.</p>
        )
      ) : (
        <OperationStatus status={status} loading={loading} onRetry={onRetry} />
      )}

      <p className="room-creation-boundary">
        One draft owns one operation and one idempotency key. No replacement Room or substituted review is created after an ambiguous result.
      </p>
    </section>
  );
}

function OperationStatus({
  status,
  loading,
  onRetry,
}: {
  status: RoomCreationStatus;
  loading: boolean;
  onRetry?: () => void;
}) {
  return (
    <div className="room-creation-result">
      <dl className="room-creation-facts">
        <Fact label="Operation" value={status.operation_id} code />
        <Fact label="Idempotency" value={status.idempotency_key} code />
        <Fact label="Reviewed snapshot" value={shortDigest(status.review_hash)} code />
        <Fact label="Exact intent" value={shortDigest(status.intent_hash)} code />
        <Fact label="Attempts" value={String(status.attempts)} />
      </dl>

      {status.state === "succeeded" ? (
        <div className="room-creation-success" role="status">
          <strong>Room creation succeeded</strong>
          <p>Room <code>{status.room_id}</code> is permanently bound to draft <code>{status.draft_id}</code>.</p>
        </div>
      ) : status.state === "needs_attention" ? (
        <div className="room-creation-attention" role="alert">
          <strong>Needs operator attention</strong>
          <p>{status.attention?.message}</p>
          <code>{labelCode(status.attention?.code ?? "reviewed_intent_rejected")}</code>
          <p>No replacement operation is available for this draft.</p>
          {status.attention?.retryable ? (
            <button type="button" disabled={loading} onClick={onRetry}>
              {loading ? "Reconciling original operation…" : "Retry original operation"}
            </button>
          ) : null}
        </div>
      ) : (
        <div className="room-creation-retrying" role="status">
          <strong>{status.state === "waiting" ? "Original operation is waiting" : "Original Room result unresolved"}</strong>
          <p>{status.attention?.message ?? "The persisted operation can resume with its exact original request."}</p>
          <button type="button" disabled={loading} onClick={onRetry}>
            {loading ? "Reconciling original operation…" : "Retry original operation"}
          </button>
        </div>
      )}
    </div>
  );
}

function Fact({ label, value, code = false }: { label: string; value: string; code?: boolean }) {
  return <div><dt>{label}</dt><dd>{code ? <code>{value}</code> : value}</dd></div>;
}

function stateLabel(state: RoomCreationStatus["state"]) {
  if (state === "needs_attention") return "Needs attention";
  return state.charAt(0).toUpperCase() + state.slice(1);
}

function shortDigest(value: string) {
  return `${value.slice(0, 18)}…${value.slice(-8)}`;
}

function labelCode(value: string) {
  return value.replaceAll("_", " ");
}
