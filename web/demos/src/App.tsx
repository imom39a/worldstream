import { useEffect, useMemo, useState } from "react";

import {
  demos,
  filterDemos,
  getDemoById,
  type DemoCategoryFilter,
  type DemoDefinition,
} from "./catalog";
import {
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
      ? "Technical demos for WorldStream Rooms, Projections, Attention, Catch-up, and Replay."
      : agentHeistPage
        ? "Inspect an Agent Heist browser fixture through Public, Navigator, and Operator views."
        : "The requested WorldStream demo page does not exist.";

    document.title = title;
    setMetaContent('meta[name="description"]', "name", "description", description);
    setMetaContent('meta[property="og:title"]', "property", "og:title", title);
    setMetaContent('meta[property="og:description"]', "property", "og:description", description);
    setMetaContent('meta[name="twitter:title"]', "name", "twitter:title", title);
    setMetaContent('meta[name="twitter:description"]', "name", "twitter:description", description);
    setMetaContent('meta[name="twitter:card"]', "name", "twitter:card", catalogPage ? "summary_large_image" : "summary");

    if (catalogPage) {
      setMetaContent('meta[property="og:image"]', "property", "og:image", "/og.png");
      setMetaContent('meta[name="twitter:image"]', "name", "twitter:image", "/og.png");
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
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<DemoCategoryFilter>("all");
  const visibleDemos = useMemo(
    () => filterDemos(demos, { category, query }),
    [category, query],
  );

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
                value={query}
                placeholder="Search capabilities"
                onChange={(event) => setQuery(event.target.value)}
              />
            </label>
            <div className="filter-group" aria-label="Demo type">
              <span>Type</span>
              <FilterButton active={category === "all"} label="All" onClick={() => setCategory("all")} />
              <FilterButton active={category === "application"} label="Application" onClick={() => setCategory("application")} />
              <FilterButton active={category === "conformance"} label="Conformance" onClick={() => setCategory("conformance")} />
            </div>
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
              <button type="button" onClick={() => { setQuery(""); setCategory("all"); }}>Reset filters</button>
            </div>
          ) : null}
        </section>

        <section className="status-band" aria-label="Catalog status">
          <StatusItem label="Catalog" value="2 entries" />
          <StatusItem label="Available" value="1 browser fixture" />
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
  return (
    <article className={`demo-card demo-card-${demo.id}`}>
      <div className="demo-visual" aria-hidden="true">
        <span className="visual-room">ROOM<br /><b>{demo.id === "agent-heist" ? "017" : "019"}</b></span>
        <span className="visual-node node-a">{demo.id === "agent-heist" ? "NAV" : "BUY"}</span>
        <span className="visual-node node-b">{demo.id === "agent-heist" ? "IN" : "SELL"}</span>
        <span className="visual-node node-c">{demo.id === "agent-heist" ? "BR" : "OK"}</span>
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
        <div className="tag-list">
          {demo.capabilities.map((capability) => <span key={capability}>{capability}</span>)}
        </div>
      </div>
      <div className="demo-card-actions">
        {demo.availability === "available" ? (
          <button type="button" onClick={() => onNavigate(demo.route)}>Open recorded demo <span>↗</span></button>
        ) : (
          <span>Live demo planned</span>
        )}
        <small>{demo.experience === "fixture" ? "Fixture · no network" : "Persistent backend required"}</small>
      </div>
    </article>
  );
}

function FilterButton({ active, label, onClick }: { active: boolean; label: string; onClick: () => void }) {
  return <button aria-pressed={active} type="button" onClick={onClick}>{label}</button>;
}

function StatusItem({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>;
}

function AgentHeistPage({ onNavigate }: { onNavigate: (path: string) => void }) {
  const [lens, setLens] = useState<ProjectionLens>("navigator");
  const [revealedThroughIndex, setRevealedThroughIndex] = useState(4);
  const [playing, setPlaying] = useState(false);
  const [selectedRecordId, setSelectedRecordId] = useState<string | null>(null);
  const visibleRecords = useMemo(
    () => visibleFixtureRecords(agentHeistRecords, lens, revealedThroughIndex),
    [lens, revealedThroughIndex],
  );
  const selectedRecord = visibleRecords.find((record) => record.id === selectedRecordId)
    ?? visibleRecords.at(-1);
  const phase = fixturePhase(revealedThroughIndex);
  const cursor = visibleRecords
    .filter((record) => record.kind !== "Attention Signal")
    .at(-1)?.roomSequence ?? "0000";

  useEffect(() => {
    if (!playing) return;
    if (revealedThroughIndex >= agentHeistRecords.length - 1) {
      setPlaying(false);
      return;
    }

    const timer = window.setInterval(() => {
      setRevealedThroughIndex((current) => Math.min(current + 1, agentHeistRecords.length - 1));
    }, 1050);
    return () => window.clearInterval(timer);
  }, [playing, revealedThroughIndex]);

  const resetFixture = () => {
    setPlaying(false);
    setRevealedThroughIndex(0);
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
              <span className="eyebrow">Visual conformance Activity Pack</span>
              <h1>Agent Heist</h1>
            </div>
            <span className="fixture-mode"><i /> Recorded fixture · no network</span>
          </div>
          <p>
            Compare authorized views of one deterministic Room. Then inspect timer changes, Attention Signals, sealed commitments, and the Result.
          </p>
        </section>

        <aside className="fixture-notice" aria-label="Demo boundary">
          <strong>Browser fixture</strong>
          <span>Controls change local recorded data. They do not create a Room, submit an Action, or contact a Runner.</span>
        </aside>

        <section className="guide" aria-labelledby="guide-title">
          <div>
            <span className="section-label">60-second guide</span>
            <h2 id="guide-title">Inspect the fixture</h2>
          </div>
          <ol>
            <GuideStep complete={true} number="01" text="Start with the Navigator view." />
            <GuideStep complete={revealedThroughIndex >= 3} number="02" text="Find the private clue at sequence 0003." />
            <GuideStep complete={lens === "public" || lens === "operator"} number="03" text="Select Public or Operator. Confirm that the clue is absent." />
            <GuideStep complete={revealedThroughIndex === agentHeistRecords.length - 1} number="04" text="Run the fixture to the Result." />
          </ol>
        </section>

        <section className="heist-status" aria-label="Fixture status">
          <StatusMetric label="Room Status" value="active" tone="green" />
          <StatusMetric label="Room Integrity State" value="healthy · gen 1" tone="green" />
          <StatusMetric label="Activity Phase" value={phase} tone="amber" />
          <StatusMetric label="Activity Pack Revision" value="pinned" tone="blue" />
          <StatusMetric label="Visible records" value={String(visibleRecords.length).padStart(2, "0")} tone="violet" />
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
                disabled={revealedThroughIndex >= agentHeistRecords.length - 1}
                type="button"
                onClick={() => setRevealedThroughIndex((current) => Math.min(current + 1, agentHeistRecords.length - 1))}
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
                aria-valuenow={revealedThroughIndex + 1}
                role="progressbar"
              >
                <i style={{ width: `${((revealedThroughIndex + 1) / agentHeistRecords.length) * 100}%` }} />
              </span>
              <small>Step {revealedThroughIndex + 1} of {agentHeistRecords.length}</small>
            </div>

            <div className="control-note">
              <span>Current view</span>
              <p>{lensBoundary(lens)}</p>
            </div>
          </aside>

          <section className="record-panel" aria-labelledby="record-panel-title">
            <div className="record-panel-heading">
              <div>
                <span>Authorized delivery data</span>
                <h2 id="record-panel-title">Observation Stream inspector</h2>
              </div>
              <span className="play-state" aria-live="polite"><i className={playing ? "is-running" : ""} />{playing ? "Running" : "Paused"}</span>
            </div>
            <div className="record-columns" aria-hidden="true">
              <span>Seq</span><span>Semantic Time</span><span>Type and authorized data</span><span>Open</span>
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
                  <span className="record-sequence">{record.roomSequence}</span>
                  <span className="record-time">{record.semanticTime}</span>
                  <span className="record-copy">
                    <i className={`record-tone tone-${record.tone}`} />
                    <span><small>{record.kind}</small><strong>{record.title}</strong><em>{record.detail}</em></span>
                  </span>
                  <span className="record-open">•••</span>
                </button>
              ))}
            </div>
            <footer className="record-footer">
              <span>{visibleRecords.length} authorized records</span>
              <span>Cursor {cursor}</span>
              <span>Local memory</span>
            </footer>
          </section>

          <aside className="record-detail" aria-live="polite">
            <PanelHeading number="03" title="Record detail" />
            {selectedRecord !== undefined ? <RecordDetail record={selectedRecord} /> : <p>No record is visible.</p>}

            <PanelHeading number="04" title="Fixture identity" />
            <dl className="identity-list">
              <div><dt>Pack</dt><dd>worldstream.agent-heist</dd></div>
              <div><dt>Revision</dt><dd>v1 · pinned</dd></div>
              <div><dt>Fixture</dt><dd>deterministic browser data</dd></div>
              <div><dt>Network</dt><dd>none</dd></div>
            </dl>
            <p className="identity-note">The displayed values are illustrative. They do not verify a live Room.</p>
          </aside>
        </section>

        <section className="what-it-shows" aria-labelledby="shows-title">
          <div className="shows-copy">
            <span className="section-label">Technical boundary</span>
            <h2 id="shows-title">What this demo shows</h2>
            <ul>
              <li>Each Membership receives an authorized Projection.</li>
              <li>A hidden-only change does not create an unauthorized Observation Frame.</li>
              <li>Semantic Time controls phase deadlines.</li>
              <li>Attention can request a fresh agent Invocation.</li>
              <li>Result data is aggregate. Individual commitments remain sealed.</li>
            </ul>
          </div>
          <div className="data-flow" aria-label="Fixture data flow">
            <div><span>01</span><strong>Recorded fixture</strong><small>Local typed records</small></div>
            <i aria-hidden="true">→</i>
            <div><span>02</span><strong>Projection lens</strong><small>Public · Navigator · Operator</small></div>
            <i aria-hidden="true">→</i>
            <div><span>03</span><strong>Visible records</strong><small>Authorized browser view</small></div>
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
      <div><span>Room sequence</span><strong>{record.roomSequence}</strong></div>
      <div><span>Delivery type</span><strong>{record.kind}</strong></div>
      <div><span>Semantic Time</span><strong>{record.semanticTime}</strong></div>
      <p>{record.detail}</p>
    </div>
  );
}

function fixturePhase(index: number) {
  if (index >= 11) return "result";
  if (index >= 10) return "resolution";
  if (index >= 6) return "commitment";
  if (index >= 1) return "negotiation";
  return "briefing";
}

function lensDescription(lens: ProjectionLens) {
  if (lens === "public") return "Public Projection";
  if (lens === "navigator") return "Participant Projection";
  return "Bounded diagnostics";
}

function lensBoundary(lens: ProjectionLens) {
  if (lens === "public") return "This view contains public phase, plan, commitment-count, and Result data.";
  if (lens === "navigator") return "This view also contains the Navigator Membership's authorized clue and reset data.";
  return "This view contains bounded operational data. It does not contain raw Activity State or private clues.";
}

function capitalize(value: string) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

function NotFoundPage({ onNavigate }: { onNavigate: (path: string) => void }) {
  return (
    <div className="not-found">
      <span>404</span>
      <h1>This page does not exist.</h1>
      <button type="button" onClick={() => onNavigate("/")}>Open demo catalog</button>
    </div>
  );
}

function SiteFooter() {
  return (
    <footer className="site-footer">
      <strong>WorldStream</strong>
      <span>Demo catalog 0.1.0</span>
      <span>STE-based draft</span>
    </footer>
  );
}

function LockIcon() {
  return <span className="lock-icon" aria-hidden="true"><i /></span>;
}
