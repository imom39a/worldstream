import type {
  AgentSeatAttention,
  RunnerAttentionOperations,
  RunnerAttentionStatus,
  RunnerRestartOperation,
  TaskAgentAttention,
} from "./runnerAttention";

export function RunnerAttentionPanel({
  operations,
  task,
  onRestart,
}: {
  operations: RunnerAttentionOperations | null;
  task: TaskAgentAttention | null;
  onRestart?: (instanceId: string) => void;
}) {
  return (
    <>
      <section className="runner-attention" aria-labelledby="runner-attention-title">
        <div className="runner-heading">
          <div>
            <p className="eyebrow">Operations · Agent execution</p>
            <h2 id="runner-attention-title">Runner attention</h2>
            <p>Live presence, exact compatibility, and bounded capacity for approved Runners.</p>
          </div>
          <div>
            <Freshness value={operations?.freshness ?? "unavailable"} />
            <Observation value={operations?.observed_at_unix_ms ?? null} />
          </div>
        </div>
        {operations === null ? (
          <Unavailable label="Runner attention is unavailable" />
        ) : operations.runners.length === 0 ? (
          <p className="status-message">No assigned Runners were reported.</p>
        ) : (
          <div className="runner-attention-list">
            {operations.runners.map((runner) => (
              <RunnerRow key={runner.runner_id} runner={runner} onRestart={onRestart} />
            ))}
          </div>
        )}
        {operations?.restart_attempts.map((attempt) => (
          <RestartRow key={attempt.operation_id} attempt={attempt} />
        ))}
      </section>

      <section className="task-agent-attention" aria-labelledby="task-agent-attention-title">
        <div className="runner-heading">
          <div>
            <p className="eyebrow">Task detail · Agent seats</p>
            <h2 id="task-agent-attention-title">Agent work attention</h2>
            <p>Aggregate waiting and leased work for each exact assigned seat.</p>
          </div>
          <div>
            <Freshness value={task?.freshness ?? "unavailable"} />
            <Observation value={task?.observed_at_unix_ms ?? null} />
          </div>
        </div>
        {task === null ? (
          <Unavailable label="Agent work status is unavailable" />
        ) : task.seats.length === 0 ? (
          <p className="status-message">No assigned agent seats were reported.</p>
        ) : (
          <div className="runner-attention-list">
            {task.seats.map((seat) => <SeatRow key={seat.seat_id} seat={seat} />)}
          </div>
        )}
      </section>
    </>
  );
}

function RunnerRow({
  runner,
  onRestart,
}: {
  runner: RunnerAttentionStatus;
  onRestart?: (instanceId: string) => void;
}) {
  return (
    <article className={`runner-attention-row is-${runner.freshness}`}>
      <div className="runner-instance-title">
        <div><strong>{runner.runner_id}</strong><span>{runner.connection}</span></div>
        <Freshness value={runner.freshness} />
      </div>
      <dl>
        <Fact label="Capacity" value={`${runner.capacity.in_use} used · ${runner.capacity.available} available · ${runner.capacity.advertised} advertised`} />
        <Fact label="Compatible assignments" value={String(runner.compatible_assignments)} />
        <Fact label="Incompatible assignments" value={String(runner.incompatible_assignments)} />
      </dl>
      <p>{runner.next_action}</p>
      {runner.instance_id !== null ? (
        <button type="button" onClick={() => onRestart?.(runner.instance_id as string)}>
          Restart approved Runner
        </button>
      ) : null}
    </article>
  );
}

function SeatRow({ seat }: { seat: AgentSeatAttention }) {
  return (
    <article className={`runner-attention-row is-${seat.activation.state}`}>
      <div className="runner-instance-title">
        <div><strong>{seat.seat_id}</strong><span>{seat.compatibility}</span></div>
        <Freshness value={seat.freshness} />
      </div>
      <dl>
        <Fact label="Activation state" value={label(seat.activation.state)} />
        <Fact label="Waiting" value={String(seat.activation.waiting)} />
        <Fact label="Leased" value={String(seat.activation.leased)} />
        <Fact label="Compatible capacity" value={`${seat.capacity.available} available of ${seat.capacity.advertised}`} />
      </dl>
      <p>{seat.next_action}</p>
    </article>
  );
}

function RestartRow({ attempt }: { attempt: RunnerRestartOperation }) {
  return (
    <article className={`runner-restart-attempt is-${attempt.state}`} role="status">
      <strong>Restart {label(attempt.state)}</strong>
      <span>{attempt.instance_id} · attempt {attempt.attempts}</span>
      <p>{attempt.explanation}</p>
      <p>{attempt.next_action}</p>
    </article>
  );
}

function Freshness({ value }: { value: "live" | "stale" | "unavailable" }) {
  return <span className={`freshness-chip is-${value}`}>{label(value)}</span>;
}

function Observation({ value }: { value: number | null }) {
  return <small>{value === null ? "No successful observation" : `Last observed ${new Date(value).toLocaleString("en-US")}`}</small>;
}

function Unavailable({ label: unavailableLabel }: { label: string }) {
  return (
    <div className="runner-unavailable" role="status">
      <strong>{unavailableLabel}</strong>
      <p>Retry status; inspect bounded diagnostics if unavailable persists.</p>
    </div>
  );
}

function Fact({ label: factLabel, value }: { label: string; value: string }) {
  return <div><dt>{factLabel}</dt><dd>{value}</dd></div>;
}

function label(value: string): string {
  return value.replaceAll("_", " ").replace(/^./, (character) => character.toUpperCase());
}
