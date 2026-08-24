import type {
  RunnerInstanceLifecycleAction,
  RunnerInstanceStatus,
  RunnerInstanceStatusResponse,
  RunnerTemplateCatalog,
} from "./runnerTemplates";

export function RunnerOperations({
  catalog,
  instances,
  onLifecycleAction,
}: {
  catalog: RunnerTemplateCatalog | null;
  instances: RunnerInstanceStatusResponse | null;
  onLifecycleAction?: (instanceId: string, action: RunnerInstanceLifecycleAction) => void;
}) {
  return (
    <>
      <section className="runner-templates" id="runner-templates" aria-labelledby="runner-templates-title">
        <div className="runner-heading">
          <div>
            <p className="eyebrow">Build · Owner-installed execution</p>
            <h2 id="runner-templates-title">Installed Runner Templates</h2>
            <p>Each template is bound to an approved executable and one immutable exact revision.</p>
          </div>
          <span className="bounded-chip">Local owner-controlled</span>
        </div>
        {catalog === null ? (
          <div className="runner-unavailable" role="status">
            <strong>Runner Template catalog unavailable</strong>
            <p>Confirm the local Supervisor and owner-installed manifests are available.</p>
          </div>
        ) : catalog.templates.length === 0 ? (
          <p className="status-message">No owner-installed Runner Template revisions were reported.</p>
        ) : (
          <div className="runner-template-grid">
            {catalog.templates.map((template) => (
              <article key={`${template.template_id}:${template.revision}`}>
                <div className="runner-template-title">
                  <div><strong>{template.display_name}</strong><span>{template.template_id} · {template.revision}</span></div>
                  <code title={template.executable_blake3}>{shortDigest(template.executable_blake3)}</code>
                </div>
                <dl>
                  <Fact
                    label="Activity Pack compatibility"
                    value={template.compatibility.map(compatibilityLabel).join(" · ")}
                  />
                  <Fact
                    label="Capacity per instance"
                    value={`${template.capacity.maximum_concurrent_invocations} concurrent Invocations`}
                  />
                  <Fact label="Health freshness bound" value={`${template.health_stale_after_ms} ms`} />
                  <Fact
                    label="Protected settings"
                    value={`${template.secret_settings.filter((setting) => setting.configured).length}/${template.secret_settings.length} configured`}
                  />
                </dl>
              </article>
            ))}
          </div>
        )}
        <p className="runner-boundary-note">
          Registration happens only through owner-controlled local manifests. Studio cannot provide an executable, path, shell, or launch arguments.
        </p>
      </section>

      <section className="runner-processes" id="runner-processes" aria-labelledby="runner-processes-title">
        <div className="runner-heading">
          <div>
            <p className="eyebrow">Operations · Runner processes</p>
            <h2 id="runner-processes-title">Installed Runner instances</h2>
            <p>Observe health and request only bounded lifecycle operations.</p>
          </div>
        </div>
        {instances === null ? (
          <div className="runner-unavailable" role="status">
            <strong>Runner instance status unavailable</strong>
            <p>No cached process state or lifecycle controls are shown.</p>
          </div>
        ) : instances.instances.length === 0 ? (
          <p className="status-message">No installed Runner instances were reported.</p>
        ) : (
          <div className="runner-instance-list">
            {instances.instances.map((instance) => (
              <RunnerInstanceRow
                key={instance.instance_id}
                instance={instance}
                onLifecycleAction={onLifecycleAction}
              />
            ))}
          </div>
        )}
      </section>
    </>
  );
}

function RunnerInstanceRow({
  instance,
  onLifecycleAction,
}: {
  instance: RunnerInstanceStatus;
  onLifecycleAction?: (instanceId: string, action: RunnerInstanceLifecycleAction) => void;
}) {
  return (
    <article className={`runner-instance is-${instance.state}`}>
      <div className="runner-instance-title">
        <div>
          <strong>{instance.instance_id}</strong>
          <span>{instance.template_id} · {instance.template_revision}</span>
        </div>
        <span>{stateLabel(instance.state)}</span>
      </div>
      <dl>
        <Fact label="Compatibility" value={instance.compatibility.map(compatibilityLabel).join(" · ")} />
        <Fact
          label="Capacity"
          value={`${instance.capacity.in_use} in use · ${instance.capacity.available} available · ${instance.capacity.maximum} maximum`}
        />
        <Fact label="Health and freshness" value={`${healthLabel(instance.health)} · ${instance.freshness}`} />
        <Fact label="Last observation" value={observationLabel(instance.observed_at_unix_ms)} />
      </dl>
      {instance.state === "starting" || instance.state === "stopping" ? (
        <p className="runner-progress" role="status">
          Operation {instance.operation_id} is in progress and will be reconciled by the Supervisor.
        </p>
      ) : null}
      {instance.failure ? (
        <div className="runner-failure" role="alert">
          <strong>Runner needs attention</strong>
          <p>{instance.failure.explanation}</p>
          <p>{instance.failure.next_action}</p>
        </div>
      ) : null}
      <div className="runner-actions">
        {instance.state === "stopped" || (instance.state === "failed" && !instance.managed_by_supervisor) ? (
          <RunnerAction
            label={instance.state === "failed" ? "Retry start" : "Start instance"}
            instanceId={instance.instance_id}
            action="start"
            onLifecycleAction={onLifecycleAction}
          />
        ) : null}
        {instance.state === "running" && instance.managed_by_supervisor ? (
          <>
            <RunnerAction label="Stop instance" instanceId={instance.instance_id} action="stop" onLifecycleAction={onLifecycleAction} />
            <RunnerAction label="Restart instance" instanceId={instance.instance_id} action="restart" onLifecycleAction={onLifecycleAction} />
          </>
        ) : null}
        {instance.state === "failed" && instance.managed_by_supervisor ? (
          <RunnerAction label="Retry graceful stop" instanceId={instance.instance_id} action="stop" onLifecycleAction={onLifecycleAction} />
        ) : null}
      </div>
    </article>
  );
}

function RunnerAction({
  label,
  instanceId,
  action,
  onLifecycleAction,
}: {
  label: string;
  instanceId: string;
  action: RunnerInstanceLifecycleAction;
  onLifecycleAction?: (instanceId: string, action: RunnerInstanceLifecycleAction) => void;
}) {
  return <button type="button" onClick={() => onLifecycleAction?.(instanceId, action)}>{label}</button>;
}

function Fact({ label, value }: { label: string; value: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd></div>;
}

function compatibilityLabel(rule: RunnerInstanceStatus["compatibility"][number]): string {
  return `${rule.activity_pack_id} ${rule.exact_revisions.join(", ")}`;
}

function stateLabel(state: RunnerInstanceStatus["state"]): string {
  return state.charAt(0).toUpperCase() + state.slice(1);
}

function healthLabel(health: RunnerInstanceStatus["health"]): string {
  return health.charAt(0).toUpperCase() + health.slice(1);
}

function observationLabel(observedAt: number | null): string {
  return observedAt === null ? "No successful observation" : new Date(observedAt).toLocaleString("en-US");
}

function shortDigest(digest: string): string {
  return `${digest.slice(0, 16)}…`;
}
