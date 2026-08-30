import { useEffect, useMemo, useState } from "react";

import { demoBuildIdentity } from "./buildIdentity";
import {
  agentHeistEvidence,
  agentHeistRecords,
  visibleFixtureRecords,
  type AgentHeistFixtureRecord,
  type ProjectionLens,
} from "./agentHeistFixture";
import { SiteFooter, SiteHeader, type Navigate } from "./siteChrome";

export function AgentHeistPage({ onNavigate }: { onNavigate: Navigate }) {
  const [lens, setLens] = useState<ProjectionLens>("navigator");
  const [revealedThroughStep, setRevealedThroughStep] = useState(4);
  const [playing, setPlaying] = useState(false);
  const [selectedRecordId, setSelectedRecordId] = useState<string | null>(null);
  const visibleRecords = useMemo(
    () => visibleFixtureRecords(agentHeistRecords, lens, revealedThroughStep),
    [lens, revealedThroughStep],
  );
  const selectedRecord = visibleRecords.find((record) => record.id === selectedRecordId)
    ?? visibleRecords.at(-1);
  const phase = agentHeistRecords[revealedThroughStep]?.phase ?? "briefing";

  useEffect(() => {
    if (!playing) return;
    if (revealedThroughStep >= agentHeistRecords.length - 1) {
      setPlaying(false);
      return;
    }

    const timer = window.setInterval(() => {
      setRevealedThroughStep((current) => Math.min(current + 1, agentHeistRecords.length - 1));
    }, 1050);
    return () => window.clearInterval(timer);
  }, [playing, revealedThroughStep]);

  const resetFixture = () => {
    setPlaying(false);
    setRevealedThroughStep(0);
    setSelectedRecordId(null);
  };

  return (
    <div className="site-shell heist-shell">
      <a className="skip-link" href="#heist-inspector">Skip to inspector</a>
      <SiteHeader onNavigate={onNavigate} />
      <main>
        <section className="heist-intro">
          <button className="back-link" type="button" onClick={() => onNavigate("/")}>← Demo catalog</button>
          <div className="heist-title-row">
            <div>
              <span className="eyebrow">Recorded technical fixture</span>
              <h1>Agent Heist</h1>
            </div>
            <span className="fixture-mode"><i /> Recorded fixture · no network</span>
          </div>
          <p>
            Compare illustrative Projection views from one retained parity story. Inspect Semantic Time, Attention summaries, sealed commitments, and Replay evidence.
          </p>
        </section>

        <aside className="fixture-notice" aria-label="Demo boundary">
          <strong>Browser fixture</strong>
          <span>Controls filter local records. This browser is not an authorization control. It does not create a Room, submit an Action, or contact a Runner.</span>
        </aside>

        <section className="guide" aria-labelledby="guide-title">
          <div>
            <span className="section-label">60-second guide</span>
            <h2 id="guide-title">Inspect the fixture</h2>
          </div>
          <ol>
            <GuideStep complete={true} number="01" text="Start with the Navigator view." />
            <GuideStep complete={revealedThroughStep >= 2} number="02" text="Find the private clue at Room sequence 0002." />
            <GuideStep complete={lens === "public" || lens === "operator"} number="03" text="Select Public or Operator. Confirm that the clue is absent." />
            <GuideStep complete={revealedThroughStep === agentHeistRecords.length - 1} number="04" text="Run the fixture to the recorded Replay evidence." />
          </ol>
        </section>

        <section className="heist-status" aria-label="Fixture status">
          <StatusMetric label="Fixture outcome" value={`recorded · ${agentHeistEvidence.finalOutcome}`} tone="green" />
          <StatusMetric
            label="Fixture Replay status"
            value={agentHeistEvidence.replayVerified ? "recorded · verified" : "recorded · not verified"}
            tone={agentHeistEvidence.replayVerified ? "green" : "amber"}
          />
          <StatusMetric label="Fixture phase" value={`recorded · ${phase}`} tone="amber" />
          <StatusMetric label="Fixture Pack version" value={`recorded · ${agentHeistEvidence.packVersion}`} tone="blue" />
          <StatusMetric label="Visible fixture records" value={String(visibleRecords.length).padStart(2, "0")} tone="violet" />
        </section>

        <section className="inspector" id="heist-inspector">
          <aside className="inspector-controls">
            <PanelHeading number="01" title="Select Projection" />
            <div className="lens-switch">
              {(["public", "navigator", "operator"] as const).map((item) => (
                <button
                  aria-pressed={lens === item}
                  className={lens === item ? "is-active" : ""}
                  key={item}
                  type="button"
                  onClick={() => { setLens(item); setSelectedRecordId(null); }}
                >
                  <span>{item === "navigator" ? "Navigator" : capitalize(item)}</span>
                  <small>{lensDescription(item)}</small>
                </button>
              ))}
            </div>

            <PanelHeading number="02" title="Control playback" />
            <div className="playback-controls">
              <button aria-pressed={playing} type="button" onClick={() => setPlaying((current) => !current)}>
                {playing ? "Pause" : "Run"}
              </button>
              <button
                disabled={revealedThroughStep >= agentHeistRecords.length - 1}
                type="button"
                onClick={() => setRevealedThroughStep((current) => Math.min(current + 1, agentHeistRecords.length - 1))}
              >
                Step
              </button>
              <button type="button" onClick={resetFixture}>Reset</button>
            </div>
            <div className="playback-progress">
              <span
                aria-label="Fixture playback progress"
                aria-valuemax={agentHeistRecords.length}
                aria-valuemin={1}
                aria-valuenow={revealedThroughStep + 1}
                role="progressbar"
              >
                <i style={{ width: `${((revealedThroughStep + 1) / agentHeistRecords.length) * 100}%` }} />
              </span>
              <small>Step {revealedThroughStep + 1} of {agentHeistRecords.length}</small>
            </div>

            <div className="control-note">
              <span>Current view</span>
              <p>{lensBoundary(lens)}</p>
            </div>
          </aside>

          <section className="record-panel" aria-labelledby="record-panel-title">
            <div className="record-panel-heading">
              <div>
                <span>Selected retained-story data</span>
                <h2 id="record-panel-title">Recorded parity inspector</h2>
              </div>
              <span className="play-state" aria-live="polite"><i className={playing ? "is-running" : ""} />{playing ? "Running" : "Paused"}</span>
            </div>
            <div className="record-columns" aria-hidden="true">
              <span>Room sequence</span><span>Semantic Time</span><span>Summary type and data</span><span>Open</span>
            </div>
            <div className="record-list">
              {visibleRecords.map((record) => (
                <button
                  aria-pressed={selectedRecord?.id === record.id}
                  className={selectedRecord?.id === record.id ? "is-selected" : ""}
                  key={record.id}
                  type="button"
                  onClick={() => setSelectedRecordId(record.id)}
                >
                  <span className="record-sequence">{record.roomSequence?.value ?? "—"}</span>
                  <span className="record-time">{record.semanticTime?.value ?? "—"}</span>
                  <span className="record-copy">
                    <i className={`record-tone tone-${record.tone}`} />
                    <span><small>{record.kind}</small><strong>{record.title}</strong><em>{record.detail}</em></span>
                  </span>
                  <span className="record-open">•••</span>
                </button>
              ))}
            </div>
            <footer className="record-footer">
              <span>{visibleRecords.length} visible fixture records</span>
              <span>Cursor: not simulated</span>
              <span>Connection: no network</span>
              <span>Replay: not run here</span>
            </footer>
          </section>

          <aside className="record-detail" aria-live="polite">
            <PanelHeading number="03" title="Record detail" />
            {selectedRecord !== undefined ? <RecordDetail record={selectedRecord} /> : <p>No record is visible.</p>}

            <PanelHeading number="04" title="Fixture identity" />
            <dl className="identity-list">
              <div><dt>Pack</dt><dd>{agentHeistEvidence.packId}</dd></div>
              <div><dt>Pack version</dt><dd>{agentHeistEvidence.packVersion}</dd></div>
              <div><dt>Revision digest</dt><dd>{agentHeistEvidence.revisionDigest}</dd></div>
              <div><dt>Fixture ID</dt><dd>{agentHeistEvidence.fixtureId}</dd></div>
              <div><dt>Final Room sequence</dt><dd>{agentHeistEvidence.finalRoomSequence}</dd></div>
              <div><dt>Evidence schema</dt><dd>{agentHeistEvidence.sourceSchema}</dd></div>
              <div><dt>Transcript digest</dt><dd>{agentHeistEvidence.transcriptDigest}</dd></div>
              <div><dt>Browser build</dt><dd>{demoBuildIdentity.sourceRevision}</dd></div>
              <div><dt>Wire protocol</dt><dd>{demoBuildIdentity.wireProtocol} · not connected</dd></div>
              <div><dt>WebSocket subprotocol</dt><dd>{demoBuildIdentity.websocketSubprotocol} · not connected</dd></div>
            </dl>
            <p className="identity-note">The browser reads selected metadata from the retained parity fixture. It does not execute this Activity Pack Revision.</p>
          </aside>
        </section>

        <section className="what-it-shows" id="what-this-shows" aria-labelledby="shows-title">
          <div className="shows-copy">
            <span className="section-label">Technical boundary</span>
            <h2 id="shows-title">What this demo shows</h2>
            <ul>
              <li>The browser illustrates different Projection outputs for three views.</li>
              <li>The Public and Operator examples omit the Navigator clue.</li>
              <li>Recorded Semantic Time values identify phase changes.</li>
              <li>Recorded Attention summaries identify requests for an agent response.</li>
              <li>The parity fixture reports verified, read-only Replay of {agentHeistEvidence.verifiedTransitionCount} Transitions.</li>
            </ul>
            <a className="inline-doc-link" href="/docs/agent-heist/">Read how this fixture works</a>
          </div>
          <div className="data-flow-wrap">
            <div className="data-flow" aria-label="Authorization model represented by the fixture">
              <div><span>01</span><strong>Retained parity fixture</strong><small>Checked-in evidence metadata</small></div>
              <i aria-hidden="true">→</i>
              <div><span>02</span><strong>Authorization boundary</strong><small>WorldStream authority · not executed here</small></div>
              <i aria-hidden="true">→</i>
              <div><span>03</span><strong>Illustrative view</strong><small>Browser visibility filter</small></div>
            </div>
            <p className="data-flow-note">The browser represents this boundary. It does not enforce authorization.</p>
          </div>
        </section>
      </main>
      <SiteFooter />
    </div>
  );
}

function GuideStep({ complete, number, text }: { complete: boolean; number: string; text: string }) {
  return <li className={complete ? "is-complete" : ""}><span>{complete ? "✓" : number}</span><p>{text}</p></li>;
}

function StatusMetric({ label, value, tone }: { label: string; value: string; tone: string }) {
  return <div><span>{label}</span><strong><i className={`metric-${tone}`} />{value}</strong></div>;
}

function PanelHeading({ number, title }: { number: string; title: string }) {
  return <div className="panel-heading"><span>{number}</span><strong>{title}</strong></div>;
}

function RecordDetail({ record }: { record: AgentHeistFixtureRecord }) {
  return (
    <div className="record-detail-card">
      <div><span>Room sequence</span><strong>{record.roomSequence?.value ?? "Not applicable"}</strong></div>
      <div><span>Record type</span><strong>{record.kind}</strong></div>
      <div><span>Semantic Time</span><strong>{record.semanticTime?.value ?? "Not applicable"}</strong></div>
      <p>{record.detail}</p>
    </div>
  );
}

function lensDescription(lens: ProjectionLens) {
  if (lens === "public") return "Public Projection";
  if (lens === "navigator") return "Participant Projection";
  return "Bounded diagnostics";
}

function lensBoundary(lens: ProjectionLens) {
  if (lens === "public") return "This example contains public phase, plan, commitment-count, and Result summaries.";
  if (lens === "navigator") return "This example also contains the Navigator clue. The browser does not enforce authorization.";
  return "This example contains bounded operational summaries. It omits raw Activity State and private clues.";
}

function capitalize(value: string) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}
