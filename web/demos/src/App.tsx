import { useEffect, useMemo, useState } from "react";

import {
  defaultDemoCatalogFilter,
  demos,
  filterDemos,
  getDemoById,
  type DemoActivityPackFilter,
  type DemoAvailabilityFilter,
  type DemoCapabilityFilter,
  type DemoCatalogFilter,
  type DemoCategoryFilter,
  type DemoDefinition,
  type DemoExperienceFilter,
  type DemoPerspectiveFilter,
} from "./catalog";
import { demoBuildIdentity } from "./buildIdentity";
import {
  agentHeistEvidence,
  agentHeistRecords,
  visibleFixtureRecords,
  type AgentHeistFixtureRecord,
  type ProjectionLens,
} from "./agentHeistFixture";

const capabilityNotes = [
  {
    label: "Ordered change",
    term: "Action → Stimulus → Transition",
  },
  {
    label: "Authorized visibility",
    term: "Projection → Observation Frame",
  },
  {
    label: "Durable continuity",
    term: "Cursor → Catch-up / Projection Reset",
  },
  {
    label: "Agent attention",
    term: "Attention Signal → Activation Intent",
  },
] as const;

const catalogFacetOptions = {
  activityPack: [
    { value: "all", label: "All Activity Packs" },
    { value: "worldstream.agent-heist", label: "Agent Heist" },
    { value: "worldstream.negotiate", label: "WorldStream Negotiate" },
  ] satisfies readonly { value: DemoActivityPackFilter; label: string }[],
  availability: [
    { value: "all", label: "All availability" },
    { value: "available", label: "Available" },
    { value: "preview", label: "Preview" },
    { value: "planned", label: "Planned" },
  ] satisfies readonly { value: DemoAvailabilityFilter; label: string }[],
  capability: [
    { value: "all", label: "All capabilities" },
    { value: "Action Offers", label: "Action Offers" },
    { value: "Attention", label: "Attention" },
    { value: "Evidence", label: "Evidence" },
    { value: "Recorded Replay evidence", label: "Recorded Replay evidence" },
    { value: "Scoped Projections", label: "Scoped Projections" },
    { value: "Sealed commitments", label: "Sealed commitments" },
    { value: "Semantic Time", label: "Semantic Time" },
  ] satisfies readonly { value: DemoCapabilityFilter; label: string }[],
  category: [
    { value: "all", label: "All demo types" },
    { value: "technical-fixture", label: "Technical fixture" },
    { value: "application", label: "Application" },
  ] satisfies readonly { value: DemoCategoryFilter; label: string }[],
  experience: [
    { value: "all", label: "All experiences" },
    { value: "interactive-fixture", label: "Interactive fixture" },
    { value: "guided-replay", label: "Guided Replay" },
    { value: "live-shared-room", label: "Live shared Room" },
  ] satisfies readonly { value: DemoExperienceFilter; label: string }[],
  perspective: [
    { value: "all", label: "All perspectives" },
    { value: "participant", label: "Participant" },
    { value: "spectator", label: "Spectator" },
    { value: "operator", label: "Operator" },
    { value: "integrator", label: "Integrator" },
    { value: "pack-author", label: "Pack Author" },
  ] satisfies readonly { value: DemoPerspectiveFilter; label: string }[],
} as const;

const socialPreviewUrl = "https://worldstream-demos.vercel.app/og.png";

function usePathname(): [string, (path: string) => void] {
  const [pathname, setPathname] = useState(window.location.pathname);

  useEffect(() => {
    const handlePopState = () => setPathname(window.location.pathname);
    window.addEventListener("popstate", handlePopState);
    return () => window.removeEventListener("popstate", handlePopState);
  }, []);

  const navigate = (path: string) => {
    window.history.pushState(null, "", path);
    setPathname(path);
    window.scrollTo({ top: 0, behavior: "auto" });
  };

  return [pathname, navigate];
}

export function App() {
  const [pathname, navigate] = usePathname();
  const demoId = pathname.match(/^\/demos\/([^/]+)\/?$/)?.[1];
  const page = pathname === "/"
    ? "catalog"
    : demoId === "agent-heist"
      ? "agent-heist"
      : "not-found";

  usePageMetadata(page);

  if (demoId !== undefined) {
    const demo = getDemoById(demos, demoId);
    return demo?.id === "agent-heist" ? (
      <AgentHeistPage onNavigate={navigate} />
    ) : (
      <NotFoundPage onNavigate={navigate} />
    );
  }

  return pathname === "/" ? (
    <CatalogPage onNavigate={navigate} />
  ) : (
    <NotFoundPage onNavigate={navigate} />
  );
}

function usePageMetadata(page: "catalog" | "agent-heist" | "not-found") {
  useEffect(() => {
    const catalogPage = page === "catalog";
    const agentHeistPage = page === "agent-heist";
    const title = catalogPage
      ? "WorldStream technical demos"
      : agentHeistPage
        ? "Agent Heist recorded demo | WorldStream"
        : "Page not found | WorldStream";
    const description = catalogPage
      ? "Browse recorded and planned demos for WorldStream Rooms, Projections, Attention, and Replay evidence."
      : agentHeistPage
        ? "Inspect selected Agent Heist parity records through Public, Navigator, and Operator examples."
        : "The requested WorldStream demo page does not exist.";

    document.title = title;
    setMetaContent('meta[name="description"]', "name", "description", description);
    setMetaContent('meta[property="og:title"]', "property", "og:title", title);
    setMetaContent('meta[property="og:description"]', "property", "og:description", description);
    setMetaContent('meta[name="twitter:title"]', "name", "twitter:title", title);
    setMetaContent('meta[name="twitter:description"]', "name", "twitter:description", description);
    setMetaContent('meta[name="twitter:card"]', "name", "twitter:card", catalogPage ? "summary_large_image" : "summary");

    if (catalogPage) {
      setMetaContent('meta[property="og:image"]', "property", "og:image", socialPreviewUrl);
      setMetaContent('meta[name="twitter:image"]', "name", "twitter:image", socialPreviewUrl);
    } else {
      document.querySelector('meta[property="og:image"]')?.remove();
      document.querySelector('meta[name="twitter:image"]')?.remove();
    }
  }, [page]);
}

function setMetaContent(selector: string, attribute: "name" | "property", key: string, content: string) {
  let element = document.head.querySelector<HTMLMetaElement>(selector);
  if (element === null) {
    element = document.createElement("meta");
    element.setAttribute(attribute, key);
    document.head.append(element);
  }
  element.content = content;
}

function SiteHeader({ onNavigate }: { onNavigate: (path: string) => void }) {
  return (
    <header className="site-header">
      <button className="brand" type="button" onClick={() => onNavigate("/")}>
        <span className="brand-mark" aria-hidden="true"><i /><b /><em /></span>
        <span><strong>WorldStream</strong><small>Technical demos</small></span>
      </button>
      <nav aria-label="Primary navigation">
        <a href="/#demos">Demos</a>
        <a href="/#capabilities">Capabilities</a>
        <span className="source-status"><LockIcon /> Source private</span>
      </nav>
    </header>
  );
}

function CatalogPage({ onNavigate }: { onNavigate: (path: string) => void }) {
  const [filters, setFilters] = useState<DemoCatalogFilter>(defaultDemoCatalogFilter);
  const visibleDemos = useMemo(
    () => filterDemos(demos, filters),
    [filters],
  );

  const updateFilter = <Key extends keyof DemoCatalogFilter>(
    key: Key,
    value: DemoCatalogFilter[Key],
  ) => setFilters((current) => ({ ...current, [key]: value }));

  return (
    <div className="site-shell">
      <a className="skip-link" href="#main-content">Skip to content</a>
      <SiteHeader onNavigate={onNavigate} />
      <main id="main-content">
        <section className="catalog-intro">
          <span className="eyebrow">WorldStream</span>
          <h1>Technical demo <em>catalog.</em></h1>
          <p>
            Use these demos to inspect Room behavior. The first demo runs a deterministic browser fixture and does not contact a WorldStream server.
          </p>
          <i aria-hidden="true" />
        </section>

        <section className="catalog" id="demos" aria-labelledby="catalog-title">
          <div className="catalog-toolbar">
            <div>
              <span className="section-label">Browse demos</span>
              <h2 id="catalog-title">Select a demo</h2>
            </div>
            <label className="search-field">
              <span>Search</span>
              <input
                type="search"
                value={filters.query}
                placeholder="Search capabilities"
                onChange={(event) => updateFilter("query", event.target.value)}
              />
            </label>
          </div>

          <div className="catalog-filters" aria-label="Demo filters">
            <FacetSelect label="Demo type" value={filters.category} options={catalogFacetOptions.category} onChange={(value) => updateFilter("category", value)} />
            <FacetSelect label="Activity Pack" value={filters.activityPack} options={catalogFacetOptions.activityPack} onChange={(value) => updateFilter("activityPack", value)} />
            <FacetSelect label="Capability" value={filters.capability} options={catalogFacetOptions.capability} onChange={(value) => updateFilter("capability", value)} />
            <FacetSelect label="Experience" value={filters.experience} options={catalogFacetOptions.experience} onChange={(value) => updateFilter("experience", value)} />
            <FacetSelect label="Perspective" value={filters.perspective} options={catalogFacetOptions.perspective} onChange={(value) => updateFilter("perspective", value)} />
            <FacetSelect label="Availability" value={filters.availability} options={catalogFacetOptions.availability} onChange={(value) => updateFilter("availability", value)} />
          </div>

          <div className="results-heading">
            <span>{visibleDemos.length} of {demos.length} demos</span>
            <span className="fixture-key"><i /> Recorded browser data</span>
          </div>

          <div className="demo-grid">
            {visibleDemos.map((demo) => (
              <DemoCard key={demo.id} demo={demo} onNavigate={onNavigate} />
            ))}
          </div>

          {visibleDemos.length === 0 ? (
            <div className="empty-state">
              <p>No demo matches the current filter.</p>
              <button type="button" onClick={() => setFilters(defaultDemoCatalogFilter)}>Reset filters</button>
            </div>
          ) : null}

          <aside className="planned-note" id="planned-demo" aria-label="Planned demo requirement">
            <strong>WorldStream Negotiate is planned.</strong>
            <span>It needs a persistent WorldStream authority. The catalog does not report it as available.</span>
          </aside>
        </section>

        <section className="status-band" aria-label="Catalog status">
          <StatusItem label="Catalog schema" value={demoBuildIdentity.catalogSchema} />
          <StatusItem label="Build revision" value={demoBuildIdentity.sourceRevision} />
          <StatusItem label="Backend" value="Not connected" />
          <StatusItem label="Source" value="Private repository" />
        </section>

        <section className="capabilities" id="capabilities" aria-labelledby="capabilities-title">
          <div>
            <span className="section-label">Runtime functions</span>
            <h2 id="capabilities-title">Capability reference</h2>
            <p>Each demo names the WorldStream functions that it shows.</p>
          </div>
          <div className="capability-grid">
            {capabilityNotes.map((item, index) => (
              <article key={item.label}>
                <span>{String(index + 1).padStart(2, "0")}</span>
                <h3>{item.label}</h3>
                <p>{item.term}</p>
              </article>
            ))}
          </div>
        </section>
      </main>
      <SiteFooter />
    </div>
  );
}

function DemoCard({ demo, onNavigate }: { demo: DemoDefinition; onNavigate: (path: string) => void }) {
  const agentHeist = demo.thumbnail.variant === "agent-heist";

  return (
    <article className={`demo-card demo-card-${demo.id}`}>
      <div className="demo-visual" role="img" aria-label={demo.thumbnail.description}>
        <span className="visual-room">Room<br /><b>{agentHeist ? "017" : "019"}</b></span>
        <span className="visual-node node-a">{agentHeist ? "Nav" : "Buy"}</span>
        <span className="visual-node node-b">{agentHeist ? "In" : "Sell"}</span>
        <span className="visual-node node-c">{agentHeist ? "Br" : "OK"}</span>
        <i className="visual-line line-a" />
        <i className="visual-line line-b" />
        <i className="visual-line line-c" />
      </div>
      <div className="demo-card-copy">
        <div className="card-status-row">
          <span>{demo.categoryLabel}</span>
          <strong className={`availability availability-${demo.availability}`}>{demo.availability}</strong>
        </div>
        <h3>{demo.title}</h3>
        <p>{demo.summary}</p>
        <dl className="card-facts">
          <div><dt>Activity Pack</dt><dd>{demo.activityPack.id}</dd></div>
          <div><dt>Experience</dt><dd>{demo.experienceLabel}</dd></div>
          <div><dt>Backend</dt><dd>{demo.backendRequirement.label}</dd></div>
        </dl>
        <div className="tag-list">
          {demo.capabilities.map((capability) => <span key={capability}>{capability}</span>)}
        </div>
      </div>
      <div className="demo-card-actions">
        <div>
          {demo.availability === "available" ? (
            <button type="button" onClick={() => onNavigate(demo.route)}>Open recorded demo <span>↗</span></button>
          ) : (
            <span className="planned-action">Live demo planned</span>
          )}
          <a href={demo.documentationRoute}>How it works</a>
        </div>
        <small>{demo.availability === "available" ? "Recorded data · no network" : demo.backendRequirement.label}</small>
      </div>
    </article>
  );
}

function FacetSelect<Value extends string>({
  label,
  onChange,
  options,
  value,
}: {
  readonly label: string;
  readonly onChange: (value: Value) => void;
  readonly options: readonly { readonly value: Value; readonly label: string }[];
  readonly value: Value;
}) {
  return (
    <label className="facet-select">
      <span>{label}</span>
      <select value={value} onChange={(event) => onChange(event.target.value as Value)}>
        {options.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
      </select>
    </label>
  );
}

function StatusItem({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>;
}

function AgentHeistPage({ onNavigate }: { onNavigate: (path: string) => void }) {
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
          <StatusMetric label="Fixture Pack Revision" value={`recorded · ${agentHeistEvidence.packVersion}`} tone="blue" />
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
              <span>Room seq</span><span>Semantic Time</span><span>Summary type and data</span><span>Open</span>
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

function NotFoundPage({ onNavigate }: { onNavigate: (path: string) => void }) {
  return (
    <div className="site-shell not-found-shell">
      <a className="skip-link" href="#not-found-content">Skip to content</a>
      <SiteHeader onNavigate={onNavigate} />
      <main className="not-found" id="not-found-content">
        <span>404</span>
        <h1>This page does not exist.</h1>
        <button type="button" onClick={() => onNavigate("/")}>Open demo catalog</button>
      </main>
    </div>
  );
}

function SiteFooter() {
  return (
    <footer className="site-footer">
      <strong>WorldStream</strong>
      <span>Product {demoBuildIdentity.productVersion} · build {demoBuildIdentity.sourceRevision}</span>
      <span>STE-based draft</span>
    </footer>
  );
}

function LockIcon() {
  return <span className="lock-icon" aria-hidden="true"><i /></span>;
}
