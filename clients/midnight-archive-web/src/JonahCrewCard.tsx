import type { MidnightArchiveActionType } from "./actionContract";
import {
  candidateById,
  type ArchiveLocation,
  type MidnightArchiveActionIntent,
  type MidnightArchiveProjection,
  type SpecialistTaskKind,
} from "./model";
import { CostPills } from "./presentation";
import { SpecialistCrewCard, type SpecialistCrewCardDefinition } from "./SpecialistCrewCard";

const JONAH_CARD: SpecialistCrewCardDefinition = {
  name: "Jonah",
  titleId: "jonah-crew-title",
  testId: "jonah-crew-card",
  classNames: {
    card: "jonah-crew-card",
    presence: "jonah-presence",
    facts: "jonah-facts",
    planStatus: "jonah-plan-status",
    preparation: "jonah-preparation",
    lastContribution: "jonah-last-contribution",
    controls: "jonah-controls",
  },
  controlsLabel: "Jonah task controls",
  waitingMessage: "Jonah is preparing a bounded plan. You may continue staging your own turn.",
};

export function JonahCrewCard({ projection, enabled, offerTypes, onAction }: {
  readonly projection: MidnightArchiveProjection;
  readonly enabled: boolean;
  readonly offerTypes: ReadonlySet<MidnightArchiveActionType>;
  readonly onAction: (intent: MidnightArchiveActionIntent) => void;
}) {
  const jonah = projection.jonah;
  if (jonah.presence === "absent") return null;
  const active = jonah.presence === "active";
  const progress = jonah.planning.stepsTotal === 0
    ? "No accepted steps"
    : `${jonah.planning.stepsCompleted} of ${jonah.planning.stepsTotal} complete`;
  return <SpecialistCrewCard
    definition={JONAH_CARD}
    view={{
      presence: jonah.presence,
      location: locationLabel(projection, jonah.location),
      mode: labelWord(jonah.mode),
      task: taskLabel(jonah.task.kind),
      taskStatus: jonah.task.status,
      taskRevision: jonah.task.revision,
      powerAllowance: `${jonah.task.powerSpent} spent · ${jonah.task.powerAllowance} allowed`,
      plan: `${jonah.planning.status.replace("_", " ")} · revision ${jonah.planning.planRevision}`,
      opportunity: `revision ${jonah.planning.opportunityRevision}`,
      progress,
      waiting: jonah.planning.status === "waiting",
      preparation: {
        status: jonah.preparation.status,
        summary: jonah.preparation.summary === "none" ? "No contribution selected" : jonah.preparation.summary,
        forTurn: jonah.preparation.status === "none" ? null : jonah.preparation.forTurn,
      },
      lastContribution: jonah.lastContribution,
    }}
    knowledge={<JonahKnowledge projection={projection} />}
    controls={<>
      {!active ? <p className="jonah-unavailable">Jonah's current Membership cannot accept crew orders.</p> : null}
      <h3>Standing task</h3>
      <p>Choose a Pack-defined investigation or Jonah's efficient hatch method.</p>
      {offerTypes.has("assign_jonah_task") ? null : <p className="jonah-unavailable">Task assignment is unavailable at this Room Head.</p>}
      <div className="jonah-task-grid">
        <TaskButton kind="investigate_records" allowance={0} current={jonah.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_jonah_task")} onAction={onAction} />
        <TaskButton kind="investigate_records" allowance={1} current={jonah.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_jonah_task")} onAction={onAction} />
        <TaskButton kind="investigate_conservation" allowance={0} current={jonah.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_jonah_task")} onAction={onAction} />
        <TaskButton kind="investigate_conservation" allowance={1} current={jonah.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_jonah_task")} onAction={onAction} />
        <TaskButton kind="open_service_hatch" allowance={1} current={jonah.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_jonah_task")} onAction={onAction} />
      </div>
      <ControlButton action="cancel_jonah_task" label="Cancel current task" enabled={active && enabled} offered={offerTypes.has("cancel_jonah_task")} reason={jonah.task.status !== "assigned" ? "Jonah has no active task to cancel." : "Task cancellation is unavailable at this Room Head."} onAction={onAction} />

      <h3>Movement order</h3>
      <div className="jonah-control-row">
        <ControlButton action="set_jonah_follow" label="Follow lead" enabled={active && enabled} offered={offerTypes.has("set_jonah_follow")} reason="Following is unavailable at this Room Head." onAction={onAction} />
        <ControlButton action="set_jonah_regroup" label="Regroup at Atrium" enabled={active && enabled} offered={offerTypes.has("set_jonah_regroup")} reason="Regrouping is unavailable at this Room Head." onAction={onAction} />
        <ControlButton action="set_jonah_hold" label="Hold position" enabled={active && enabled} offered={offerTypes.has("set_jonah_hold")} reason="Holding is unavailable at this Room Head." onAction={onAction} />
      </div>

      <h3>Bounded plan</h3>
      <div className="jonah-control-row">
        <ControlButton action="request_jonah_plan" label="Request a plan" enabled={active && enabled} offered={offerTypes.has("request_jonah_plan")} reason={jonah.task.status !== "assigned" ? "Assign a task first." : "Plan requests are unavailable at this Room Head."} onAction={onAction} />
        <ControlButton action="prepare_jonah_contribution" label="Prepare next eligible step" enabled={active && enabled} offered={offerTypes.has("prepare_jonah_contribution")} reason={jonah.planning.status !== "ready" ? "No current bounded plan has an eligible step." : "The next planned step is currently ineligible."} onAction={onAction} />
        <ControlButton action="defer_jonah_contribution" label="Defer Jonah this turn" enabled={active && enabled} offered={offerTypes.has("defer_jonah_contribution")} reason="Deferral is unavailable at this Room Head." onAction={onAction} />
      </div>
      <p className="jonah-control-note">Crew controls cost no turn or power. A prepared contribution resolves beside your staged personal Action only when you commit.</p>
    </>}
  />;
}

function JonahKnowledge({ projection }: { readonly projection: MidnightArchiveProjection }) {
  const knowledge = projection.jonah.knowledge;
  const verified = knowledge.verifierResult === null ? null : candidateById(projection, knowledge.verifierResult.candidateId);
  return <div className="jonah-knowledge" aria-label="Evidence disclosed by Jonah">
    <small>Disclosed evidence</small>
    <p>{knowledge.records === "shared" ? "Records evidence is disclosed on the candidate board."
      : knowledge.records === "private" ? "Jonah has private Records findings; their values have not been disclosed."
        : "No Records finding has been disclosed."}</p>
    <p>{knowledge.conservation === "shared" ? "Conservation evidence is disclosed on the candidate board."
      : knowledge.conservation === "private" ? "Jonah has private Conservation findings; their values have not been disclosed."
        : "No Conservation finding has been disclosed."}</p>
    {verified === null ? null : <p className="jonah-verified">Catalog verifier disclosed: {verified.label} · verified</p>}
  </div>;
}

function TaskButton({ kind, allowance, current, enabled, offered, onAction }: {
  readonly kind: Exclude<SpecialistTaskKind, "none" | "field_assay">;
  readonly allowance: 0 | 1;
  readonly current: SpecialistTaskKind;
  readonly enabled: boolean;
  readonly offered: boolean;
  readonly onAction: (intent: MidnightArchiveActionIntent) => void;
}) {
  const label = current === kind ? `Update ${taskLabel(kind)}` : `Assign ${taskLabel(kind)}`;
  const available = enabled && offered;
  return <button type="button" data-action-type="assign_jonah_task" disabled={!available}
    aria-label={`${label} with ${allowance} power allowance`}
    title={available ? undefined : "Task assignment is unavailable at this Room Head."}
    onClick={() => onAction({ action: "assign_jonah_task", task_kind: kind, power_allowance: allowance })}>
    <span>{label}</span><small>Allowance: {allowance} power</small><CostPills turns={0} power={0} />
  </button>;
}

type JonahControlAction = "cancel_jonah_task" | "set_jonah_follow" | "set_jonah_hold" | "set_jonah_regroup"
  | "request_jonah_plan" | "prepare_jonah_contribution" | "defer_jonah_contribution";

function ControlButton({ action, label, enabled, offered, reason, onAction }: {
  readonly action: JonahControlAction;
  readonly label: string;
  readonly enabled: boolean;
  readonly offered: boolean;
  readonly reason: string;
  readonly onAction: (intent: MidnightArchiveActionIntent) => void;
}) {
  const available = enabled && offered;
  return <div className="jonah-control"><button type="button" data-action-type={action} disabled={!available}
    title={available ? undefined : reason} aria-label={available ? label : `${label}. Unavailable: ${reason}`}
    onClick={() => onAction({ action })}>{label}</button>{available ? null : <small>{reason}</small>}</div>;
}

function taskLabel(kind: SpecialistTaskKind): string {
  if (kind === "investigate_records") return "Investigate Records";
  if (kind === "investigate_conservation") return "Investigate Conservation";
  if (kind === "open_service_hatch") return "Open service hatch";
  if (kind === "field_assay") return "Field assay";
  return "No standing task";
}

function locationLabel(projection: MidnightArchiveProjection, location: ArchiveLocation | "none"): string {
  if (location === "none") return "Not in expedition";
  return projection.map.locations.find((candidate) => candidate.id === location)?.name ?? location;
}

function labelWord(value: string): string {
  return value === "unavailable" ? "Unavailable" : `${value[0]?.toUpperCase() ?? ""}${value.slice(1)}`;
}
