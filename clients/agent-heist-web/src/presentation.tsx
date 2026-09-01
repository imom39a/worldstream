import type { ReactNode } from "react";

export interface AgentHeistWorkspaceMetric {
  readonly label: string;
  readonly value: string;
  readonly tone: "amber" | "blue" | "green" | "violet";
}

export interface AgentHeistWorkspaceRecord {
  readonly id: string;
  readonly sequence: string;
  readonly semanticTime: string;
  readonly kind: string;
  readonly title: string;
  readonly detail: string;
  readonly tone: AgentHeistWorkspaceMetric["tone"];
}

export function AgentHeistWorkspace({
  metrics,
  left,
  heading,
  eyebrow,
  status,
  center,
  footer,
  right,
}: {
  readonly metrics: readonly AgentHeistWorkspaceMetric[];
  readonly left: ReactNode;
  readonly heading: string;
  readonly eyebrow: string;
  readonly status: ReactNode;
  readonly center: ReactNode;
  readonly footer: ReactNode;
  readonly right: ReactNode;
}) {
  return (
    <>
      <section className="heist-status" aria-label="Agent Heist status">
        {metrics.map((metric) => <StatusMetric key={metric.label} {...metric} />)}
      </section>
      <section className="inspector" id="heist-inspector">
        <aside className="inspector-controls">{left}</aside>
        <section className="record-panel" aria-labelledby="record-panel-title">
          <div className="record-panel-heading">
            <div>
              <span>{eyebrow}</span>
              <h2 id="record-panel-title">{heading}</h2>
            </div>
            {status}
          </div>
          {center}
          <footer className="record-footer">{footer}</footer>
        </section>
        <aside className="record-detail" aria-live="polite">{right}</aside>
      </section>
    </>
  );
}

export function RecordedAgentHeistWorkspace({
  metrics,
  lenses,
  selectedLens,
  onSelectLens,
  playing,
  canStep,
  onTogglePlayback,
  onStep,
  onReset,
  progress,
  progressLabel,
  boundary,
  records,
  selectedRecord,
  onSelectRecord,
  footer,
  identity,
  identityNote,
}: {
  readonly metrics: readonly AgentHeistWorkspaceMetric[];
  readonly lenses: readonly { id: string; label: string; description: string }[];
  readonly selectedLens: string;
  readonly onSelectLens: (id: string) => void;
  readonly playing: boolean;
  readonly canStep: boolean;
  readonly onTogglePlayback: () => void;
  readonly onStep: () => void;
  readonly onReset: () => void;
  readonly progress: number;
  readonly progressLabel: string;
  readonly boundary: string;
  readonly records: readonly AgentHeistWorkspaceRecord[];
  readonly selectedRecord: AgentHeistWorkspaceRecord | undefined;
  readonly onSelectRecord: (id: string) => void;
  readonly footer: readonly string[];
  readonly identity: readonly { label: string; value: string }[];
  readonly identityNote: string;
}) {
  return (
    <AgentHeistWorkspace
      metrics={metrics}
      left={<>
        <PanelHeading number="01" title="Select Projection" />
        <div className="lens-switch">
          {lenses.map((lens) => (
            <button
              aria-pressed={selectedLens === lens.id}
              className={selectedLens === lens.id ? "is-active" : ""}
              key={lens.id}
              type="button"
              onClick={() => onSelectLens(lens.id)}
            >
              <span>{lens.label}</span>
              <small>{lens.description}</small>
            </button>
          ))}
        </div>
        <PanelHeading number="02" title="Control playback" />
        <div className="playback-controls">
          <button aria-pressed={playing} type="button" onClick={onTogglePlayback}>{playing ? "Pause" : "Run"}</button>
          <button disabled={!canStep} type="button" onClick={onStep}>Step</button>
          <button type="button" onClick={onReset}>Reset</button>
        </div>
        <div className="playback-progress">
          <span aria-label="Fixture playback progress" aria-valuemax={100} aria-valuemin={0} aria-valuenow={Math.round(progress)} role="progressbar">
            <i style={{ width: `${progress}%` }} />
          </span>
          <small>{progressLabel}</small>
        </div>
        <div className="control-note"><span>Current view</span><p>{boundary}</p></div>
      </>}
      eyebrow="Selected retained-story data"
      heading="Recorded parity inspector"
      status={<span className="play-state" aria-live="polite"><i className={playing ? "is-running" : ""} />{playing ? "Running" : "Paused"}</span>}
      center={<>
        <div className="record-columns" aria-hidden="true">
          <span>Room sequence</span><span>Semantic Time</span><span>Summary type and data</span><span>Open</span>
        </div>
        <RecordList records={records} selectedRecord={selectedRecord} onSelectRecord={onSelectRecord} />
      </>}
      footer={footer.map((item) => <span key={item}>{item}</span>)}
      right={<>
        <PanelHeading number="03" title="Record detail" />
        {selectedRecord === undefined ? <p>No record is visible.</p> : <RecordDetail record={selectedRecord} />}
        <PanelHeading number="04" title="Fixture identity" />
        <dl className="identity-list">
          {identity.map((item) => <div key={item.label}><dt>{item.label}</dt><dd>{item.value}</dd></div>)}
        </dl>
        <p className="identity-note">{identityNote}</p>
      </>}
    />
  );
}

export function PanelHeading({ number, title }: { readonly number: string; readonly title: string }) {
  return <div className="panel-heading"><span>{number}</span><strong>{title}</strong></div>;
}

export function RecordList({
  records,
  selectedRecord,
  onSelectRecord,
}: {
  readonly records: readonly AgentHeistWorkspaceRecord[];
  readonly selectedRecord?: AgentHeistWorkspaceRecord;
  readonly onSelectRecord?: (id: string) => void;
}) {
  return (
    <div className="record-list">
      {records.map((record) => (
        <button
          aria-pressed={selectedRecord?.id === record.id}
          className={selectedRecord?.id === record.id ? "is-selected" : ""}
          key={record.id}
          type="button"
          onClick={() => onSelectRecord?.(record.id)}
        >
          <span className="record-sequence">{record.sequence}</span>
          <span className="record-time">{record.semanticTime}</span>
          <span className="record-copy">
            <i className={`record-tone tone-${record.tone}`} />
            <span><small>{record.kind}</small><strong>{record.title}</strong><em>{record.detail}</em></span>
          </span>
          <span className="record-open">•••</span>
        </button>
      ))}
    </div>
  );
}

export function RecordDetail({ record }: { readonly record: AgentHeistWorkspaceRecord }) {
  return (
    <div className="record-detail-card">
      <div><span>Room sequence</span><strong>{record.sequence === "—" ? "Not applicable" : record.sequence}</strong></div>
      <div><span>Record type</span><strong>{record.kind}</strong></div>
      <div><span>Semantic Time</span><strong>{record.semanticTime === "—" ? "Not applicable" : record.semanticTime}</strong></div>
      <p>{record.detail}</p>
    </div>
  );
}

function StatusMetric({ label, value, tone }: AgentHeistWorkspaceMetric) {
  return <div><span>{label}</span><strong><i className={`metric-${tone}`} />{value}</strong></div>;
}
