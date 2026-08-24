import type { DaemonStatus } from "./daemonStatus";

export interface AppProps {
  status: DaemonStatus | null;
}

export function App({ status }: AppProps) {
  const connected = status?.connectivity === "connected";
  const loading = status === null;

  return (
    <main className="studio-shell">
      <aside className="studio-sidebar">
        <div className="brand-mark" aria-hidden="true"><span /></div>
        <div>
          <strong>WorldStream Studio</strong>
          <span>Host operator portal</span>
        </div>
        <nav aria-label="Studio navigation">
          <a aria-current="page" href="#home">Home</a>
          <span aria-disabled="true">Tasks</span>
          <span aria-disabled="true">Build</span>
          <span aria-disabled="true">Operations</span>
        </nav>
        <p>Companion control plane</p>
      </aside>

      <section className="studio-content" id="home">
        <header>
          <div>
            <p className="eyebrow">Local installation</p>
            <h1>Operator overview</h1>
            <p>Observe the authoritative Room runtime through the local Supervisor.</p>
          </div>
          <StatusBadge connected={connected} loading={loading} />
        </header>

        <section className={`daemon-card ${connected ? "is-connected" : "is-unavailable"}`}>
          <div className="daemon-heading">
            <div>
              <p className="eyebrow">Authoritative runtime</p>
              <h2>{loading ? "Checking worldstreamd…" : connected ? "worldstreamd connected" : "worldstreamd unavailable"}</h2>
            </div>
            <span className="status-orb" aria-hidden="true" />
          </div>

          {connected && status ? (
            <dl className="status-grid">
              <StatusFact label="Health" value={status.health === "live" ? "Process live" : "Unavailable"} />
              <StatusFact label="Readiness" value={readinessLabel(status.readiness)} />
              <StatusFact label="Version" value={`Build ${status.version?.build_version ?? "Unavailable"}`} />
              <StatusFact label="Source" value={shortRevision(status.version?.source_revision)} />
            </dl>
          ) : loading ? (
            <p className="status-message">Asking the Supervisor for live daemon health and version information.</p>
          ) : (
            <div className="unavailable-message" role="status">
              <strong>Start or reconnect the local daemon</strong>
              <p>The Supervisor cannot establish live health. No cached version is shown.</p>
            </div>
          )}
        </section>

        <section className="authority-note">
          <p className="eyebrow">Authority boundary</p>
          <h2>Rooms stay with worldstreamd</h2>
          <p>
            Studio is an operator-facing companion. It observes and requests work only through supported daemon APIs; it does not own or directly mutate Authoritative Room State.
          </p>
        </section>
      </section>
    </main>
  );
}

function StatusBadge({ connected, loading }: { connected: boolean; loading: boolean }) {
  return <span className={`top-status ${connected ? "good" : "muted"}`}>{loading ? "Checking" : connected ? "Daemon online" : "Daemon offline"}</span>;
}

function StatusFact({ label, value }: { label: string; value: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd></div>;
}

function readinessLabel(readiness: DaemonStatus["readiness"]) {
  if (readiness === "ready") return "Runtime ready";
  if (readiness === "not_ready") return "Runtime not ready";
  return "Unavailable";
}

function shortRevision(revision: string | undefined) {
  return revision ? revision.slice(0, 12) : "Unavailable";
}
