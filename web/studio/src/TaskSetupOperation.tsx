import type { TaskSetupStatus } from "./taskSetup";

export function TaskSetupOperation({
  setup,
  statusAvailable,
  roomCreated,
  loading,
  onStart,
  onRetry,
  onLaunch,
  onOpenParticipantClient,
}: {
  setup: TaskSetupStatus | null;
  statusAvailable: boolean;
  roomCreated: boolean;
  loading: boolean;
  onStart?: () => void;
  onRetry?: () => void;
  onLaunch?: () => void;
  onOpenParticipantClient?: (seatId: string) => void;
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
              <small className={`task-readiness ${readinessFor(setup, seat.seat_id)?.ready ? "is-ready" : "is-blocked"}`}>
                {readinessLabel(readinessFor(setup, seat.seat_id)?.reason)}
              </small>
              {seat.principal_kind === "human" && seat.member_authority === "provisioned" ? (
                <button type="button" onClick={() => onOpenParticipantClient?.(seat.seat_id)}>Open participant client ↗</button>
              ) : null}
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
      ) : retryable || (setup?.state === "ready" && setup.launch_applicability === "unknown") ? (
        <button type="button" disabled={loading} onClick={onRetry}>
          {loading ? "Retrying exact stage…" : "Retry original setup"}
        </button>
      ) : setup?.state === "ready" && setup.launch_applicability === "lobby_launch" && setup.launch?.state !== "launched" ? (
        <button type="button" disabled={loading || !setup.readiness.ready_to_launch} onClick={onLaunch}>
          {loading ? "Reconciling launch…" : setup.launch === null ? "Launch Task" : "Retry original launch"}
        </button>
      ) : null}
      {setup?.launch?.state === "launched" ? (
        <div className="task-launch-committed" role="status"><strong>Task launched</strong><p>The committed Lobby transition is now observable.</p></div>
      ) : setup?.state === "ready" && setup.launch_applicability === "lobby_launch" && !setup.readiness.ready_to_launch ? (
        <p className="task-launch-blocked" role="status">Launch is blocked until every required seat is live-ready.</p>
      ) : setup?.state === "ready" && setup.launch_applicability === "active_at_genesis" ? (
        <div className="task-launch-committed" role="status"><strong>Room active</strong><p>Genesis created the active Room. Open an available participant handoff to connect.</p></div>
      ) : setup?.state === "ready" && setup.launch_applicability === "unknown" ? (
        <p className="task-launch-blocked" role="status">The exact Activity Pack launch contract is unavailable. Retry setup after restoring catalog access.</p>
      ) : null}
      <p className="task-setup-boundary">
        Participant Action authority and Runner-control authority are provisioned separately. Credentials remain in the protected Supervisor vault.
      </p>
    </section>
  );
}

function readinessFor(setup: TaskSetupStatus, seatId: string) {
  return setup.readiness.seats.find((seat) => seat.seat_id === seatId);
}

function readinessLabel(reason: TaskSetupStatus["readiness"]["seats"][number]["reason"] | undefined) {
  return ({
    ready: "Ready", optional_unfilled: "Optional · unfilled", setup_incomplete: "Setup incomplete",
    console_missing: "Console session missing", console_stale: "Console session stale",
    console_invalid: "Console session invalid", console_disconnected: "Console disconnected",
    runner_assignment_missing: "Runner assignment missing", runner_missing: "Runner missing",
    runner_stale: "Runner presence stale", runner_disconnected: "Runner disconnected",
    runner_over_capacity: "Runner at capacity", runner_incompatible: "Runner incompatible",
  } as const)[reason ?? "setup_incomplete"];
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
