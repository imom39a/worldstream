import type { MidnightArchiveActionType } from "./actionContract";
import {
  candidateById,
  type ArchiveLocation,
  type MidnightArchiveActionIntent,
  type MidnightArchiveProjection,
  type MiraTaskKind,
} from "./model";
import { CostPills } from "./presentation";

export function MiraCrewCard({
  projection,
  enabled,
  offerTypes,
  onAction,
}: {
  readonly projection: MidnightArchiveProjection;
  readonly enabled: boolean;
  readonly offerTypes: ReadonlySet<MidnightArchiveActionType>;
  readonly onAction: (intent: MidnightArchiveActionIntent) => void;
}) {
  const mira = projection.mira;
  if (mira.presence === "absent") return null;
  const active = mira.presence === "active";
  const taskKind = taskLabel(mira.task.kind);
  const planProgress = mira.planning.stepsTotal === 0
    ? "No accepted steps"
    : `${mira.planning.stepsCompleted} of ${mira.planning.stepsTotal} complete`;
  return (
    <section className="mira-crew-card" aria-labelledby="mira-crew-title" data-testid="mira-crew-card">
      <header>
        <div>
          <p className="archive-kicker">Agent companion</p>
          <h2 id="mira-crew-title">Mira</h2>
        </div>
        <span className={`mira-presence presence-${mira.presence}`}>{mira.presence}</span>
      </header>

      <dl className="mira-facts">
        <Fact label="Location" value={locationLabel(projection, mira.location)} />
        <Fact label="Mode" value={modeLabel(mira.mode)} />
        <Fact label="Task" value={taskKind} />
        <Fact label="Task status" value={mira.task.status} />
        <Fact label="Task revision" value={String(mira.task.revision)} />
        <Fact label="Power allowance" value={`${mira.task.powerSpent} spent · ${mira.task.powerAllowance} allowed`} />
        <Fact label="Plan" value={`${planningLabel(mira.planning.status)} · revision ${mira.planning.planRevision}`} />
        <Fact label="Opportunity" value={`revision ${mira.planning.opportunityRevision}`} />
        <Fact label="Progress" value={planProgress} />
      </dl>

      {mira.planning.status === "waiting" ? (
        <p className="mira-plan-status" role="status">Mira is preparing a bounded plan. You may continue staging your own turn.</p>
      ) : null}

      <div className={`mira-preparation preparation-${mira.preparation.status}`}>
        <small>Prepared contribution</small>
        <strong>{preparationLabel(mira.preparation.summary)}</strong>
        {mira.preparation.status === "none" ? null : (
          <span>Fenced to turn {mira.preparation.forTurn}</span>
        )}
      </div>

      <MiraKnowledge projection={projection} />

      <div className="mira-last-contribution">
        <small>Last recorded contribution · turn {mira.lastContribution.turn}</small>
        <p>{mira.lastContribution.summary}</p>
      </div>

      <div className="mira-controls" aria-label="Mira task controls">
        {!active ? <p className="mira-unavailable">Mira's current Membership cannot accept crew orders.</p> : null}
        <h3>Standing task</h3>
        <p>Choose a Pack-defined investigation and a maximum shared-power allowance.</p>
        {offerTypes.has("assign_mira_task") ? null : (
          <p className="mira-unavailable">Task assignment is unavailable at this Room Head.</p>
        )}
        <div className="mira-task-grid">
          <TaskButton kind="investigate_records" allowance={0} current={mira.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_mira_task")} onAction={onAction} />
          <TaskButton kind="investigate_records" allowance={1} current={mira.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_mira_task")} onAction={onAction} />
          <TaskButton kind="investigate_conservation" allowance={0} current={mira.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_mira_task")} onAction={onAction} />
          <TaskButton kind="investigate_conservation" allowance={1} current={mira.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_mira_task")} onAction={onAction} />
        </div>
        <ControlButton
          action="cancel_mira_task"
          label="Cancel current task"
          enabled={active && enabled}
          offered={offerTypes.has("cancel_mira_task")}
          reason={mira.task.status !== "assigned" ? "Mira has no active investigation to cancel." : "Task cancellation is unavailable at this Room Head."}
          onAction={onAction}
        />

        <h3>Movement order</h3>
        <div className="mira-control-row">
          <ControlButton action="set_mira_follow" label="Follow lead" enabled={active && enabled} offered={offerTypes.has("set_mira_follow")} reason="Following is unavailable at this Room Head." onAction={onAction} />
          <ControlButton action="set_mira_regroup" label="Regroup at Atrium" enabled={active && enabled} offered={offerTypes.has("set_mira_regroup")} reason="Regrouping is unavailable at this Room Head." onAction={onAction} />
          <ControlButton action="set_mira_hold" label="Hold position" enabled={active && enabled} offered={offerTypes.has("set_mira_hold")} reason="Holding is unavailable at this Room Head." onAction={onAction} />
        </div>

        <h3>Bounded plan</h3>
        <div className="mira-control-row">
          <ControlButton
            action="request_mira_plan"
            label="Request a plan"
            enabled={active && enabled}
            offered={offerTypes.has("request_mira_plan")}
            reason={planUnavailableReason(projection)}
            onAction={onAction}
          />
          <ControlButton
            action="prepare_mira_contribution"
            label="Prepare next eligible step"
            enabled={active && enabled}
            offered={offerTypes.has("prepare_mira_contribution")}
            reason={prepareUnavailableReason(projection)}
            onAction={onAction}
          />
          <ControlButton
            action="defer_mira_contribution"
            label="Defer Mira this turn"
            enabled={active && enabled}
            offered={offerTypes.has("defer_mira_contribution")}
            reason="Deferral is unavailable at this Room Head."
            onAction={onAction}
          />
        </div>
        <p className="mira-control-note">Crew controls cost no turn or power. A prepared contribution resolves beside your staged personal Action only when you commit.</p>
      </div>
    </section>
  );
}

function MiraKnowledge({ projection }: { readonly projection: MidnightArchiveProjection }) {
  const knowledge = projection.mira.knowledge;
  const verifierCandidate = knowledge.verifierResult === null
    ? null
    : candidateById(projection, knowledge.verifierResult.candidateId);
  return (
    <div className="mira-knowledge" aria-label="Evidence disclosed by Mira">
      <small>Disclosed evidence</small>
      <p>{knowledge.records === "shared"
        ? "Records evidence is disclosed on the candidate board."
        : knowledge.records === "private"
          ? "Mira has private Records findings; their values have not been disclosed."
          : "No Records finding has been disclosed."}</p>
      <p>{knowledge.conservation === "shared"
        ? "Conservation evidence is disclosed on the candidate board."
        : knowledge.conservation === "private"
          ? "Mira has private Conservation findings; their values have not been disclosed."
          : "No Conservation finding has been disclosed."}</p>
      {verifierCandidate === null ? null : (
        <p className="mira-verified">Catalog verifier disclosed: {verifierCandidate.label} · verified</p>
      )}
    </div>
  );
}

function TaskButton({ kind, allowance, current, enabled, offered, onAction }: {
  readonly kind: Exclude<MiraTaskKind, "none">;
  readonly allowance: 0 | 1;
  readonly current: MiraTaskKind;
  readonly enabled: boolean;
  readonly offered: boolean;
  readonly onAction: (intent: MidnightArchiveActionIntent) => void;
}) {
  const label = current === kind ? `Update ${taskLabel(kind)}` : `Assign ${taskLabel(kind)}`;
  const available = enabled && offered;
  return (
    <button
      type="button"
      data-action-type="assign_mira_task"
      disabled={!available}
      aria-label={`${label} with ${allowance} power allowance`}
      title={available ? undefined : "Task assignment is unavailable at this Room Head."}
      onClick={() => onAction({ action: "assign_mira_task", task_kind: kind, power_allowance: allowance })}
    >
      <span>{label}</span>
      <small>Allowance: {allowance} power</small>
      <CostPills turns={0} power={0} />
    </button>
  );
}

function ControlButton({ action, label, enabled, offered, reason, onAction }: {
  readonly action: Exclude<MidnightArchiveActionType, `stage_${string}` | "commit_turn" | "assign_mira_task">;
  readonly label: string;
  readonly enabled: boolean;
  readonly offered: boolean;
  readonly reason: string;
  readonly onAction: (intent: MidnightArchiveActionIntent) => void;
}) {
  const available = enabled && offered;
  return (
    <div className="mira-control">
      <button
        type="button"
        data-action-type={action}
        disabled={!available}
        title={available ? undefined : reason}
        aria-label={available ? label : `${label}. Unavailable: ${reason}`}
        onClick={() => onAction({ action })}
      >{label}</button>
      {available ? null : <small>{reason}</small>}
    </div>
  );
}

function Fact({ label, value }: { readonly label: string; readonly value: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd></div>;
}

function locationLabel(projection: MidnightArchiveProjection, location: ArchiveLocation | "none"): string {
  if (location === "none") return "Not in expedition";
  return projection.map.locations.find((candidate) => candidate.id === location)?.name ?? location;
}

function taskLabel(kind: MiraTaskKind): string {
  if (kind === "investigate_records") return "Investigate Records";
  if (kind === "investigate_conservation") return "Investigate Conservation";
  return "No standing task";
}

function modeLabel(mode: MidnightArchiveProjection["mira"]["mode"]): string {
  return mode === "unavailable" ? "Unavailable" : `${mode[0]?.toUpperCase() ?? ""}${mode.slice(1)}`;
}

function planningLabel(status: MidnightArchiveProjection["mira"]["planning"]["status"]): string {
  return status.replace("_", " ");
}

function preparationLabel(summary: MidnightArchiveProjection["mira"]["preparation"]["summary"]): string {
  return summary === "none" ? "No contribution selected" : summary;
}

function planUnavailableReason(projection: MidnightArchiveProjection): string {
  if (projection.mira.task.status !== "assigned") return "Assign an investigation task first.";
  if (projection.mira.planning.status === "waiting") return "Mira is already preparing a plan.";
  if (projection.mira.planning.status === "ready") return "The current bounded plan is ready.";
  return "Plan requests are unavailable at this Room Head.";
}

function prepareUnavailableReason(projection: MidnightArchiveProjection): string {
  if (projection.mira.planning.status === "waiting") return "Wait for Mira's plan response.";
  if (projection.mira.planning.status !== "ready") return "No current bounded plan has an eligible step.";
  if (projection.mira.preparation.status !== "none") return "A Mira contribution is already selected for this turn.";
  return "The next planned step is currently ineligible.";
}
