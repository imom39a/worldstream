import { useMemo, useState } from "react";

import { demoBuildIdentity } from "./buildIdentity";
import {
  defaultDemoCatalogFilter,
  demos,
  deriveDemoCatalogFacets,
  filterDemos,
  getDemoDocumentationLabel,
  type DemoActivityPackFilter,
  type DemoAvailabilityFilter,
  type DemoCapabilityFilter,
  type DemoCatalogFilter,
  type DemoCategoryFilter,
  type DemoDefinition,
  type DemoExperienceFilter,
  type DemoPerspectiveFilter,
} from "./catalog";
import { SiteFooter, SiteHeader, type Navigate } from "./siteChrome";

const capabilityNotes = [
  { label: "Ordered change", term: "Action → Stimulus → Transition" },
  { label: "Authorized visibility", term: "Projection → Observation Frame" },
  { label: "Durable continuity", term: "Cursor → Catch-up / Projection Reset" },
  { label: "Agent attention", term: "Attention Signal → Activation Intent" },
] as const;

const catalogFacets = deriveDemoCatalogFacets(demos);

const catalogFacetOptions = {
  activityPack: [
    { value: "all", label: "All Activity Packs" },
    ...catalogFacets.activityPacks.map((pack) => ({ value: pack.id, label: pack.label })),
  ] satisfies readonly { value: DemoActivityPackFilter; label: string }[],
  availability: [
    { value: "all", label: "All availability" },
    ...catalogFacets.availabilities.map((availability) => ({
      value: availability,
      label: capitalize(availability),
    })),
  ] satisfies readonly { value: DemoAvailabilityFilter; label: string }[],
  capability: [
    { value: "all", label: "All capabilities" },
    ...catalogFacets.capabilities.map((capability) => ({ value: capability, label: capability })),
  ] satisfies readonly { value: DemoCapabilityFilter; label: string }[],
  category: [
    { value: "all", label: "All demo types" },
    ...catalogFacets.categories.map((category) => ({ value: category.id, label: category.label })),
  ] satisfies readonly { value: DemoCategoryFilter; label: string }[],
  experience: [
    { value: "all", label: "All experiences" },
    ...catalogFacets.experiences.map((experience) => ({ value: experience.id, label: experience.label })),
  ] satisfies readonly { value: DemoExperienceFilter; label: string }[],
  perspective: [
    { value: "all", label: "All perspectives" },
    ...catalogFacets.perspectives.map((perspective) => ({
      value: perspective,
      label: capitalize(perspective),
    })),
  ] satisfies readonly { value: DemoPerspectiveFilter; label: string }[],
} as const;

export function CatalogPage({ onNavigate }: { onNavigate: Navigate }) {
  const [filters, setFilters] = useState<DemoCatalogFilter>(defaultDemoCatalogFilter);
  const visibleDemos = useMemo(() => filterDemos(demos, filters), [filters]);

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

function DemoCard({ demo, onNavigate }: { demo: DemoDefinition; onNavigate: Navigate }) {
  return (
    <article className={`demo-card demo-card-${demo.id}`}>
      <DemoVisual demo={demo} />
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
          <a href={demo.documentationUrl}>{getDemoDocumentationLabel(demo)}</a>
        </div>
        <small>{demo.availability === "available" ? "Recorded data · no network" : demo.backendRequirement.label}</small>
      </div>
    </article>
  );
}

function DemoVisual({ demo }: { demo: DemoDefinition }) {
  const agentHeist = demo.thumbnail.variant === "agent-heist";

  return (
    <div className={`demo-visual demo-visual-${demo.thumbnail.variant}`} role="img" aria-label={demo.thumbnail.description}>
      <div className="visual-diagram" aria-hidden="true">
        <div className="visual-meta">
          <span><i />{agentHeist ? "Recorded fixture" : "Planned flow"}</span>
          <strong>{agentHeist ? "Room 017" : "Authority required"}</strong>
        </div>
        <div className="visual-flow">
          <FlowStage index="01" label="Input" value={agentHeist ? "Action" : "Proposal"} detail={agentHeist ? "nav.enter" : "terms.v1"} />
          <i className="visual-arrow">→</i>
          <FlowStage index="02" label={agentHeist ? "Ordered change" : "Decision"} value={agentHeist ? "Transition" : "Approvals"} detail={agentHeist ? "sequence 017" : "0 of 4"} />
          <i className="visual-arrow">→</i>
          <FlowStage index="03" label="Output" value={agentHeist ? "Observation" : "Signed result"} detail={agentHeist ? "scoped view" : "not created"} />
        </div>
        <div className="visual-legend">
          {agentHeist ? <><span>Public view</span><span>Operator view</span><span>No network</span></> : <><span>Four Roles</span><span>Persistent authority</span><span>Not running</span></>}
        </div>
      </div>
    </div>
  );
}

function FlowStage({ detail, index, label, value }: { detail: string; index: string; label: string; value: string }) {
  return <div className="visual-stage"><small>{index} · {label}</small><strong>{value}</strong><span>{detail}</span></div>;
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

function capitalize(value: string) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}
