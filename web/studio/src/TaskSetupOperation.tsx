import type { TaskSetupStatus } from "./taskSetup";

export function TaskSetupOperation({
  setup,
  statusAvailable,
  roomCreated,
  loading,
  onStart,
  onRetry,
}: {
  setup: TaskSetupStatus | null;
  statusAvailable: boolean;
  roomCreated: boolean;
  loading: boolean;
  onStart?: () => void;
  onRetry?: () => void;
}) {
  if (!roomCreated && setup === null) return null;
  const retryable = setup?.state === "needs_attention" && setup.attention?.retryable === true;
  return (
    <section className="task-setup-card" id="task-setup">
      <p className="eyebrow">Task setup</p>
      <div className="task-setup-heading">
        <div>
          <h2>{heading(setup, statusAvailable)}</h2>
          <p>{detail(setup, statusAvailable)}</p>
        </div>
        {setup !== null ? (
          <strong>{setup.completed_stages}/{setup.total_stages}</strong>
        ) : null}
      </div>
      {setup !== null ? (
        <ul className="task-setup-seats">
          {setup.seats.map((seat) => (
            <li key={seat.seat_id}>
              <span>
                <strong>{seat.display_name}</strong>
                <small>{seat.role} · {seat.required ? "Required" : "Optional"}</small>
              </span>
              <span>{seatLabel(seat)}</span>
            </li>
          ))}
        </ul>
      ) : null}
      {setup?.attention !== null && setup?.attention !== undefined ? (
        <div className="task-setup-attention" role="status">
          <strong>Setup needs attention</strong>
          <p>{setup.attention.message}</p>
          {setup.active_stage !== null ? <small>Failed stage: {stageLabel(setup.active_stage.kind)}</small> : null}
        </div>
      ) : null}
      {setup === null ? (
        <button type="button" disabled={loading || !statusAvailable} onClick={onStart}>
          {loading ? "Starting setup…" : "Provision participant access"}
        </button>
      ) : retryable ? (
        <button type="button" disabled={loading} onClick={onRetry}>
          {loading ? "Retrying exact stage…" : "Retry original setup"}
        </button>
      ) : null}
      <p className="task-setup-boundary">
        Participant Action authority and Runner-control authority are provisioned separately. Credentials remain in the protected Supervisor vault.
      </p>
    </section>
  );
}

function heading(setup: TaskSetupStatus | null, available: boolean) {
  if (!available) return "Setup status unavailable";
  if (setup === null) return "Participant access not provisioned";
  if (setup.state === "ready") return "Setup ready";
  if (setup.state === "needs_attention") return "Setup needs attention";
  return "Provisioning participant access";
}

function detail(setup: TaskSetupStatus | null, available: boolean) {
  if (!available) return "A current setup status could not be verified; prior success is not shown as current.";
  if (setup === null) return "The Room exists; start the resumable setup operation for its filled seats.";
  if (setup.state === "ready") return "All filled seats have durable participant authority; Agent seats also have separate Runner control.";
  if (setup.state === "needs_attention") return "Completed stages remain checkpointed. A safe retry reuses the same identities and credentials.";
  return "Each authority stage is durably checkpointed before the next stage begins.";
}

function seatLabel(seat: TaskSetupStatus["seats"][number]) {
  if (seat.member_authority === "unfilled_optional") return "Optional · unfilled";
  if (seat.member_authority === "pending") return "Participant authority provisioning";
  if (seat.runner_authority === "provisioned") {
    return "Participant authority provisioned · Runner authority provisioned";
  }
  if (seat.runner_authority === "pending") {
    return "Participant authority provisioned · Runner authority provisioning";
  }
  return "Participant authority provisioned";
}

function stageLabel(kind: "member_capability" | "runner_capability") {
  return kind === "member_capability" ? "Participant authority" : "Runner control";
}
