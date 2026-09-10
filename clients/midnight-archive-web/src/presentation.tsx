import type { ReactNode } from "react";

import {
  actionCost,
  candidateById,
  candidateConfidence,
  moveOptions,
  type ArchiveGate,
  type ArchiveLocation,
  type ArchiveOutcomeKind,
  type MidnightArchiveActionIntent,
  type MidnightArchiveProjection,
} from "./model";

const MAP_COORDINATES: Readonly<Record<ArchiveLocation, { x: number; y: number }>> = {
  atrium: { x: 49, y: 86 },
  records: { x: 23, y: 52 },
  conservation: { x: 73, y: 55 },
  plant: { x: 18, y: 16 },
  vault: { x: 72, y: 14 },
};

export function ArchiveMap({
  projection,
  enabled,
  moveOffered,
  onAction,
}: {
  readonly projection: MidnightArchiveProjection;
  readonly enabled: boolean;
  readonly moveOffered: boolean;
  readonly onAction: (intent: MidnightArchiveActionIntent) => void;
}) {
  const options = new Map(moveOptions(projection).map((option) => [option.destination, option]));
  const moveCost = actionCost(projection, "stage_move");
  return (
    <section className="archive-map-panel" aria-labelledby="archive-map-title">
      <div className="section-heading">
        <div>
          <p className="archive-kicker">Floor plan</p>
          <h2 id="archive-map-title">The archive after midnight</h2>
        </div>
        <span className="current-location-chip">You are in {locationName(projection, projection.location)}</span>
      </div>
      <div className="archive-map" role="group" aria-label="Five-location archive map">
        <svg className="archive-map-lines" viewBox="0 0 100 100" aria-hidden="true">
          {projection.map.connections.map((connection) => {
            const start = MAP_COORDINATES[connection.from];
            const end = MAP_COORDINATES[connection.to];
            const open = connection.gate === null || projection.gates[connection.gate] === "open";
            return <line
              className={open ? "map-route map-route-open" : "map-route map-route-closed"}
              key={`${connection.from}-${connection.to}`}
              x1={start.x}
              y1={start.y}
              x2={end.x}
              y2={end.y}
            />;
          })}
        </svg>
        {projection.map.locations.map((location) => {
          const option = options.get(location.id);
          const current = location.id === projection.location;
          const reachable = option?.open === true;
          const canMove = enabled && moveOffered && reachable && !current;
          const reason = current
            ? "current location"
            : option === undefined
              ? "not adjacent"
              : option.open
                ? `${moveCost.turns} turn · ${moveCost.power} power`
                : `${gateName(option.gate)} closed`;
          return (
            <button
              className={`map-location map-location-${location.id}${current ? " is-current" : ""}${reachable ? " is-reachable" : ""}`}
              disabled={!canMove}
              key={location.id}
              onClick={() => onAction({ action: "stage_move", destination: location.id })}
              style={{ left: `${MAP_COORDINATES[location.id].x}%`, top: `${MAP_COORDINATES[location.id].y}%` }}
              type="button"
              aria-current={current ? "location" : undefined}
              aria-label={`${current ? "Current location" : "Move to"} ${location.name}: ${reason}`}
            >
              <LocationGlyph location={location.id} />
              <span>{location.name}</span>
              <small>{reason}</small>
            </button>
          );
        })}
        <span className={`map-gate map-gate-archive ${projection.gates.archive_gate}`}>
          {projection.gates.archive_gate === "open" ? "Archive gate open" : "Archive gate sealed"}
        </span>
        <span className={`map-gate map-gate-service ${projection.gates.service_hatch}`}>
          {projection.gates.service_hatch === "open" ? "Service hatch open" : "Service hatch sealed"}
        </span>
      </div>
      <div className="gate-state-grid" aria-label="Vault gate states">
        <GateStateCard gate="archive_gate" state={projection.gates.archive_gate} />
        <GateStateCard gate="service_hatch" state={projection.gates.service_hatch} />
      </div>
    </section>
  );
}

export function ResourceStrip({ projection }: { readonly projection: MidnightArchiveProjection }) {
  const danger = projection.turnsRemaining <= 3;
  return (
    <section className="resource-strip" aria-label="Expedition resources">
      <div className={danger ? "resource-card resource-danger" : "resource-card"}>
        <span>Turns remaining</span>
        <strong>{projection.turnsRemaining}<small> / 16</small></strong>
        <div className="turn-track" aria-hidden="true">
          <i style={{ width: `${(projection.turnsRemaining / 16) * 100}%` }} />
        </div>
      </div>
      <div className="resource-card">
        <span>Power reserve</span>
        <strong>{projection.power}<small> / {projection.initialPower}</small></strong>
        <div className="charge-meter" role="img" aria-label={`${projection.power} of ${projection.initialPower} power charges remain`}>
          {Array.from({ length: projection.initialPower }, (_, charge) => (
            <i className={charge < projection.power ? "charged" : "spent"} key={charge} />
          ))}
        </div>
      </div>
      <div className="resource-card location-resource">
        <span>Current location</span>
        <strong>{locationName(projection, projection.location)}</strong>
        <small>Turn {Math.min(projection.turnsUsed + 1, 16)} of 16</small>
      </div>
    </section>
  );
}

export function ScenarioBriefing({ projection }: { readonly projection: MidnightArchiveProjection }) {
  const combinedCost = projection.methodCosts.verifier + projection.methodCosts.ordinaryServiceHatch;
  return (
    <section className="scenario-briefing" aria-labelledby="archive-scenario-title">
      <div>
        <span className="archive-kicker">This expedition</span>
        <h2 id="archive-scenario-title">{projection.scenario.label}</h2>
      </div>
      <div>
        <p>Candidate markings, source evidence, and the authentic ledger differ between scenarios. Compare evidence from this expedition.</p>
        <p>Initial reserve: {projection.initialPower} power charges, shared by the crew.</p>
        <p>Catalog verifier: {projection.methodCosts.verifier} power. Ordinary service hatch: {projection.methodCosts.ordinaryServiceHatch} power.
          {combinedCost > projection.initialPower
            ? ` Together they need ${combinedCost} charges, exceeding this expedition’s ${projection.initialPower}-charge initial reserve. Choose a different authentication or access method.`
            : ` Together they use all ${projection.initialPower} initial charges.`}
        </p>
      </div>
    </section>
  );
}

export function CarriedLedgerCard({ projection }: { readonly projection: MidnightArchiveProjection }) {
  const candidate = candidateById(projection, projection.carriedCandidate);
  if (candidate === null) {
    return (
      <section className="carried-card empty-carried" aria-label="Carried ledger">
        <span className="archive-kicker">Carried ledger</span>
        <strong>Hands free</strong>
        <p>You may extract without a ledger, but the outcome will record that fact.</p>
      </section>
    );
  }
  const confidence = candidateConfidence(projection, candidate.candidateId);
  return (
    <section className="carried-card" aria-label="Carried ledger">
      <span className="archive-kicker">Carried ledger</span>
      <div className="carried-heading">
        <strong>{candidate.label}</strong>
        <span className={`confidence-badge confidence-${confidence}`}>
          {confidence === "verified" ? "Catalog verified" : "Authenticity uncertain"}
        </span>
      </div>
      <p>Carrying this candidate does not reveal hidden authenticity.</p>
    </section>
  );
}

export function ArchiveOutcomePanel({ projection }: { readonly projection: MidnightArchiveProjection }) {
  if (projection.outcome === null) return null;
  const copy = OUTCOME_COPY[projection.outcome.kind];
  const ledger = candidateById(projection, projection.carriedCandidate);
  return (
    <section className={`archive-outcome outcome-${projection.outcome.kind}`} role="status" aria-labelledby="outcome-title">
      <span className="outcome-mark" aria-hidden="true">{copy.mark}</span>
      <div>
        <p className="archive-kicker">Expedition complete</p>
        <h2 id="outcome-title">{copy.title}</h2>
        <p>{copy.description}</p>
        {projection.debrief === null ? null : <p className={`evidence-debrief debrief-${projection.debrief.evidenceStatus}`}><strong>Evidence debrief:</strong> {projection.debrief.message}</p>}
        <dl>
          <Fact label="Turns used" value={`${projection.turnsUsed} of 16`} />
          <Fact label="Power left" value={`${projection.power} charges`} />
          <Fact label="Ledger carried" value={ledger?.label ?? "None"} />
          <Fact label="Final location" value={locationName(projection, projection.location)} />
          {projection.debrief === null ? null : (
            <>
              <Fact
                label="Agreement"
                value={projection.debrief.agreementCommitment.replace("_", " ")}
              />
              <Fact
                label="Collection preserved"
                value={projection.debrief.optionalObjectives.collectionPreserved ? "Yes" : "No"}
              />
              <Fact
                label="Source record protected"
                value={projection.debrief.optionalObjectives.sourceRecordProtected ? "Yes" : "No"}
              />
              <Fact label="Crew extracted" value={crewNames(projection.crewDebrief.extractedRoles)} />
              <Fact label="Crew left behind" value={projection.crewDebrief.leftBehindRoles.length === 0
                ? "No one" : crewNames(projection.crewDebrief.leftBehindRoles)} />
            </>
          )}
        </dl>
        {projection.crewDebrief.completedWork.length === 0 ? null : <div className="crew-debrief-work">
          <h3>Completed specialist work</h3>
          <ul>{projection.crewDebrief.completedWork.map((work) => <li key={`${work.turn}:${work.role}`}>
            {crewNames([work.role])} · {work.kind.replaceAll("_", " ")} · turn {work.turn}
          </li>)}</ul>
        </div>}
      </div>
    </section>
  );
}

export function CostPills({ turns, power }: { readonly turns: number; readonly power: number }) {
  return (
    <span className="cost-pills" aria-label={`${turns} turn, ${power} power`}>
      <span>{turns} turn</span>
      <span>{power} power</span>
    </span>
  );
}

export function SectionHeading({ eyebrow, title, aside }: {
  readonly eyebrow: string;
  readonly title: string;
  readonly aside?: ReactNode;
}) {
  return (
    <div className="section-heading">
      <div><p className="archive-kicker">{eyebrow}</p><h2>{title}</h2></div>
      {aside}
    </div>
  );
}

function GateStateCard({ gate, state }: { readonly gate: ArchiveGate; readonly state: "open" | "closed" }) {
  return (
    <article className={`gate-state gate-${state}`}>
      <span aria-hidden="true">{state === "open" ? "◇" : "◆"}</span>
      <div><strong>{gateName(gate)}</strong><small>{state === "open" ? "Passage available next turn" : "Passage is sealed"}</small></div>
      <b>{state}</b>
    </article>
  );
}

function LocationGlyph({ location }: { readonly location: ArchiveLocation }) {
  const glyph: Record<ArchiveLocation, string> = {
    atrium: "✦",
    records: "▤",
    conservation: "♧",
    plant: "⚡",
    vault: "▣",
  };
  return <b aria-hidden="true">{glyph[location]}</b>;
}

function Fact({ label, value }: { readonly label: string; readonly value: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd></div>;
}

function locationName(projection: MidnightArchiveProjection, id: ArchiveLocation): string {
  return projection.map.locations.find((location) => location.id === id)?.name ?? id;
}

function gateName(gate: ArchiveGate | null): string {
  return gate === "archive_gate" ? "Conservation vault gate"
    : gate === "service_hatch" ? "Plant service hatch"
      : "Passage";
}

function crewNames(roles: readonly ("lead" | "mira" | "jonah")[]): string {
  return roles.map((role) => role === "lead" ? "Lead" : role === "mira" ? "Mira" : "Jonah").join(", ");
}

const OUTCOME_COPY: Readonly<Record<ArchiveOutcomeKind, {
  readonly mark: string;
  readonly title: string;
  readonly description: string;
}>> = {
  success: {
    mark: "✦",
    title: "The authentic ledger is out",
    description: "You escaped through the Atrium with the verified archive record before the doors sealed.",
  },
  partial_extraction: {
    mark: "◇",
    title: "The ledger is out, but the crew is split",
    description: "The authentic ledger reached the Atrium, but one or more starting specialists were left inside.",
  },
  wrong_ledger: {
    mark: "≠",
    title: "You escaped with a counterfeit",
    description: "The expedition survived, but the carried ledger was not the authentic archive record.",
  },
  no_ledger: {
    mark: "○",
    title: "You escaped empty-handed",
    description: "You reached the Atrium and chose extraction without recovering a ledger.",
  },
  exhausted_inside: {
    mark: "×",
    title: "The archive sealed with you inside",
    description: "The sixteenth turn ended away from a successful extraction. The expedition is over.",
  },
};
