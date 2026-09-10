import type { ReactNode } from "react";

export interface SpecialistCrewCardDefinition {
  readonly name: string;
  readonly titleId: string;
  readonly testId: string;
  readonly classNames: {
    readonly card: string;
    readonly presence: string;
    readonly facts: string;
    readonly planStatus: string;
    readonly preparation: string;
    readonly lastContribution: string;
    readonly controls: string;
  };
  readonly controlsLabel: string;
}

export interface SpecialistCrewCardView {
  readonly presence: string;
  readonly location: string;
  readonly mode: string;
  readonly task: string;
  readonly taskStatus: string;
  readonly taskRevision: number;
  readonly powerAllowance: string;
  readonly plan: string;
  readonly opportunity: string;
  readonly progress: string;
  /** Derived only from the authorized companion state, never Runner health. */
  readonly planningNotice: string | null;
  readonly preparation: {
    readonly status: string;
    readonly summary: string;
    readonly forTurn: number | null;
  };
  readonly lastContribution: {
    readonly turn: number;
    readonly summary: string;
  };
}

export function SpecialistCrewCard({
  definition,
  view,
  knowledge,
  controls,
}: {
  readonly definition: SpecialistCrewCardDefinition;
  readonly view: SpecialistCrewCardView;
  readonly knowledge: ReactNode;
  readonly controls: ReactNode;
}) {
  return (
    <section className={definition.classNames.card} aria-labelledby={definition.titleId} data-testid={definition.testId}>
      <header>
        <div>
          <p className="archive-kicker">Agent companion</p>
          <h2 id={definition.titleId}>{definition.name}</h2>
        </div>
        <span className={`${definition.classNames.presence} presence-${view.presence}`}>{view.presence}</span>
      </header>

      <dl className={definition.classNames.facts}>
        <Fact label="Location" value={view.location} />
        <Fact label="Mode" value={view.mode} />
        <Fact label="Task" value={view.task} />
        <Fact label="Task status" value={view.taskStatus} />
        <Fact label="Task revision" value={String(view.taskRevision)} />
        <Fact label="Power allowance" value={view.powerAllowance} />
        <Fact label="Plan" value={view.plan} />
        <Fact label="Opportunity" value={view.opportunity} />
        <Fact label="Progress" value={view.progress} />
      </dl>

      {view.planningNotice === null ? null : (
        <p className={definition.classNames.planStatus} role="status">{view.planningNotice}</p>
      )}

      <div className={`${definition.classNames.preparation} preparation-${view.preparation.status}`}>
        <small>Prepared contribution</small>
        <strong>{view.preparation.summary}</strong>
        {view.preparation.forTurn === null ? null : (
          <span>Fenced to turn {view.preparation.forTurn}</span>
        )}
      </div>

      {knowledge}

      <div className={definition.classNames.lastContribution}>
        <small>Last recorded contribution · turn {view.lastContribution.turn}</small>
        <p>{view.lastContribution.summary}</p>
      </div>

      <div className={definition.classNames.controls} aria-label={definition.controlsLabel}>
        {controls}
      </div>
    </section>
  );
}

function Fact({ label, value }: { readonly label: string; readonly value: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd></div>;
}
