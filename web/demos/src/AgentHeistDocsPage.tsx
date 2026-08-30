import { demoBuildIdentity } from "./buildIdentity";
import { agentHeistEvidence } from "./agentHeistFixture";
import { SiteFooter, SiteHeader, type Navigate } from "./siteChrome";

export function AgentHeistDocsPage({ onNavigate }: { onNavigate: Navigate }) {
  return (
    <div className="site-shell docs-shell">
      <a className="skip-link" href="#docs-content">Skip to content</a>
      <SiteHeader onNavigate={onNavigate} />
      <main id="docs-content">
        <section className="docs-intro">
          <button className="back-link" type="button" onClick={() => onNavigate("/demos/agent-heist")}>← Agent Heist demo</button>
          <span className="eyebrow">Technical documentation</span>
          <h1>Agent Heist browser fixture</h1>
          <p>This page defines the fixture source, controls, identity, and technical boundaries.</p>
        </section>

        <aside className="fixture-notice" aria-label="Documentation boundary">
          <strong>Recorded data</strong>
          <span>The browser uses local summaries and selected retained-evidence metadata. No WorldStream authority or network is connected.</span>
        </aside>

        <div className="docs-layout">
          <article className="docs-panel">
            <span className="section-label">Controls</span>
            <h2>How the fixture works</h2>
            <ol>
              <li><strong>Projection selector:</strong> It filters records for the Public, Navigator, or Operator example.</li>
              <li><strong>Run:</strong> It advances one local fixture step each second.</li>
              <li><strong>Step:</strong> It advances one local fixture step.</li>
              <li><strong>Reset:</strong> It returns to the first local fixture step.</li>
              <li><strong>Record selector:</strong> It opens the selected summary in the detail panel.</li>
            </ol>
          </article>

          <article className="docs-panel">
            <span className="section-label">Boundaries</span>
            <h2>What the browser does not do</h2>
            <ul>
              <li>It does not create or join a Room.</li>
              <li>It does not submit an Action.</li>
              <li>It does not enforce an authorization boundary.</li>
              <li>It does not create or acknowledge a Cursor.</li>
              <li>It does not execute Recovery or Replay.</li>
              <li>It does not execute an Activity Pack Revision.</li>
            </ul>
          </article>

          <article className="docs-panel docs-panel-wide">
            <span className="section-label">Recorded evidence</span>
            <h2>Exact fixture identity</h2>
            <dl className="docs-identity">
              <div><dt>Activity Pack</dt><dd>{agentHeistEvidence.packId}</dd></div>
              <div><dt>Pack version</dt><dd>{agentHeistEvidence.packVersion}</dd></div>
              <div><dt>Semantic revision digest</dt><dd>{agentHeistEvidence.revisionDigest}</dd></div>
              <div><dt>Fixture ID</dt><dd>{agentHeistEvidence.fixtureId}</dd></div>
              <div><dt>Evidence schema</dt><dd>{agentHeistEvidence.sourceSchema}</dd></div>
              <div><dt>Transcript schema</dt><dd>{agentHeistEvidence.transcriptSchema}</dd></div>
              <div><dt>Transcript digest</dt><dd>{agentHeistEvidence.transcriptDigest}</dd></div>
              <div><dt>Final Room sequence</dt><dd>{agentHeistEvidence.finalRoomSequence}</dd></div>
              <div><dt>Final phase</dt><dd>{agentHeistEvidence.finalPhase}</dd></div>
              <div><dt>Recorded Replay</dt><dd>{agentHeistEvidence.replayVerified ? "verified · read only" : "not verified"}</dd></div>
            </dl>
          </article>

          <article className="docs-panel docs-panel-wide">
            <span className="section-label">Browser build</span>
            <h2>Build and protocol identity</h2>
            <dl className="docs-identity">
              <div><dt>Source revision</dt><dd>{demoBuildIdentity.sourceRevision}</dd></div>
              <div><dt>Product version</dt><dd>{demoBuildIdentity.productVersion}</dd></div>
              <div><dt>Catalog schema</dt><dd>{demoBuildIdentity.catalogSchema}</dd></div>
              <div><dt>Wire protocol</dt><dd>{demoBuildIdentity.wireProtocol} · not connected</dd></div>
              <div><dt>WebSocket subprotocol</dt><dd>{demoBuildIdentity.websocketSubprotocol} · not connected</dd></div>
              <div><dt>Connection</dt><dd>none</dd></div>
            </dl>
          </article>
        </div>

        <div className="docs-actions">
          <button type="button" onClick={() => onNavigate("/demos/agent-heist")}>Open recorded demo</button>
          <a href="/">Open demo catalog</a>
        </div>
      </main>
      <SiteFooter />
    </div>
  );
}
