import type { MidnightArchiveActionType } from "./actionContract";
import {
  candidateById,
  type ArchiveLocation,
  type MidnightArchiveActionIntent,
  type MidnightArchiveProjection,
  type MiraTaskKind,
} from "./model";
import { CostPills } from "./presentation";
import {
  SpecialistCrewCard,
  type SpecialistCrewCardDefinition,
} from "./SpecialistCrewCard";

const MIRA_CARD: SpecialistCrewCardDefinition = {
  name: "Mira",
  titleId: "mira-crew-title",
  testId: "mira-crew-card",
  classNames: {
    card: "mira-crew-card",
    presence: "mira-presence",
    facts: "mira-facts",
    planStatus: "mira-plan-status",
    preparation: "mira-preparation",
    lastContribution: "mira-last-contribution",
    controls: "mira-controls",
  },
  controlsLabel: "Mira task controls",
};

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
    <SpecialistCrewCard
      definition={MIRA_CARD}
      view={{
        presence: mira.presence,
        location: locationLabel(projection, mira.location),
        mode: modeLabel(mira.mode),
        task: taskKind,
        taskStatus: mira.task.status,
        taskRevision: mira.task.revision,
        powerAllowance: `${mira.task.powerSpent} spent · ${mira.task.powerAllowance} allowed`,
        plan: `${planningLabel(mira.planning.status)} · revision ${mira.planning.planRevision}`,
        opportunity: `revision ${mira.planning.opportunityRevision}`,
        progress: planProgress,
        planningNotice: planningNotice("Mira", mira, offerTypes),
        preparation: {
          status: mira.preparation.status,
          summary: preparationLabel(mira.preparation.summary),
          forTurn: mira.preparation.status === "none" ? null : mira.preparation.forTurn,
        },
        lastContribution: mira.lastContribution,
      }}
      knowledge={<MiraKnowledge projection={projection} />}
      controls={<>
        {!active ? <p className="mira-unavailable">Mira's current Membership cannot accept crew orders.</p> : null}
        <h3>Standing task</h3>
        <p>Choose Pack-defined specialist work and its maximum shared-power allowance.</p>
        {offerTypes.has("assign_mira_task") ? null : (
          <p className="mira-unavailable">Task assignment is unavailable at this Room Head.</p>
        )}
        <div className="mira-task-grid">
          <TaskButton kind="investigate_records" allowance={0} current={mira.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_mira_task")} onAction={onAction} />
          <TaskButton kind="investigate_records" allowance={1} current={mira.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_mira_task")} onAction={onAction} />
          <TaskButton kind="investigate_conservation" allowance={0} current={mira.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_mira_task")} onAction={onAction} />
          <TaskButton kind="investigate_conservation" allowance={1} current={mira.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_mira_task")} onAction={onAction} />
          <TaskButton kind="open_service_hatch" allowance={2} current={mira.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_mira_task")} onAction={onAction} />
          <TaskButton kind="field_assay" allowance={0} current={mira.task.kind} enabled={active && enabled} offered={offerTypes.has("assign_mira_task")} onAction={onAction} />
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
            label={mira.planning.status === "expired" ? "Request plan again" : "Request a plan"}
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
      </>}
    />
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
      <p>Vault field assay: {projection.mira.fieldAssay.stepsCompleted} of 2 work steps complete.</p>
      {projection.mira.fieldAssay.result === null ? null : (
        <p className="mira-verified">Field assay disclosed: {candidateById(projection, projection.mira.fieldAssay.result.candidateId)?.label ?? "Known candidate"} · verified</p>
      )}
    </div>
  );
}

function TaskButton({ kind, allowance, current, enabled, offered, onAction }: {
  readonly kind: Exclude<MiraTaskKind, "none">;
  readonly allowance: 0 | 1 | 2;
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
  readonly action:
    | "cancel_mira_task"
    | "set_mira_follow"
    | "set_mira_hold"
    | "set_mira_regroup"
    | "request_mira_plan"
    | "prepare_mira_contribution"
    | "defer_mira_contribution";
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

function locationLabel(projection: MidnightArchiveProjection, location: ArchiveLocation | "none"): string {
  if (location === "none") return "Not in expedition";
  return projection.map.locations.find((candidate) => candidate.id === location)?.name ?? location;
}

function taskLabel(kind: MiraTaskKind): string {
  if (kind === "investigate_records") return "Investigate Records";
  if (kind === "investigate_conservation") return "Investigate Conservation";
  if (kind === "open_service_hatch") return "Open service hatch";
  if (kind === "field_assay") return "Field assay";
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
  if (projection.mira.planning.status === "expired") return "This expired opportunity can reopen only through a new Room-offered request.";
  return "Plan requests are unavailable at this Room Head.";
}

function prepareUnavailableReason(projection: MidnightArchiveProjection): string {
  if (projection.mira.planning.status === "waiting") return "Wait for Mira's plan response.";
  if (projection.mira.planning.status !== "ready") return "No current bounded plan has an eligible step.";
  if (projection.mira.preparation.status !== "none") return "A Mira contribution is already selected for this turn.";
  return "The next planned step is currently ineligible.";
}

function planningNotice(
  name: string,
  specialist: MidnightArchiveProjection["mira"],
  offers: ReadonlySet<MidnightArchiveActionType>,
): string | null {
  if (specialist.planning.status === "waiting") {
    return `${name}'s planning window is open until ${specialist.planning.deadline}. You may stage your own turn, defer this contribution, or use an offered follow/regroup order.`;
  }
  if (specialist.planning.status === "expired") {
    return `No ${name} plan was recorded before opportunity ${specialist.planning.opportunityRevision} expired. No turn or power was spent. Continue solo, defer this turn, use an offered follow/regroup order, or ${offers.has("request_mira_plan") ? "explicitly request a new plan" : "wait for a new Room-offered request"}.`;
  }
  if (specialist.task.status === "cancelled") {
    return `${name}'s standing task was cancelled. No plan opportunity remains open.`;
  }
  if (specialist.planning.status === "ready") {
    return `${name}'s bounded plan is recorded. Prepare its next eligible step or defer it for this turn.`;
  }
  if (specialist.planning.status === "complete") {
    return `${name}'s recorded plan is complete. Assign a new task only when the Room offers it.`;
  }
  return null;
}
