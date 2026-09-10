import type { CanonicalJson, CanonicalObject, PackActionOffer } from "@worldstream/pack-sdk";

import {
  type ArchiveState,
  type CompanionRole,
  type EvidenceSourceId,
  type Location,
  type MiraPlanStep,
  type MiraState,
  type MiraTaskKind,
  type StagedAction,
  asCanonical,
  cloneState,
  isLocation,
  record,
  reject,
  stringValue,
} from "./model.js";
import { legalDestinationsFrom, nextLocationToward } from "./world.js";

export const JONAH_PLAN_TIMER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FA4";
export const MIRA_PLAN_TIMER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FA3";
export const MIRA_PLAN_ATTENTION_REASON = "companion_plan_requested";

export interface CompanionTransition {
  readonly state: ArchiveState;
  readonly event: CanonicalObject;
  readonly timerRequests: readonly CanonicalJson[];
  readonly attentionSignals: readonly CanonicalJson[];
}

export interface MiraResolution {
  readonly kind: "none" | "plan" | "follow" | "regroup";
  readonly step: MiraPlanStep | null;
  readonly destination: Location | null;
}

export function initialMiraState(memberId: string | null, companionRole: CompanionRole = "mira"): MiraState {
  const present = memberId !== null;
  return {
    presence: present ? "active" : "absent",
    member_id: memberId ?? "none",
    location: present ? "atrium" : "none",
    mode: present ? "following" : "unavailable",
    task: emptyTask(),
    opportunity: emptyOpportunity(),
    plan: emptyPlan(),
    preparation: emptyPreparation(),
    knowledge: { records: "unknown", conservation: "unknown", verifier_result: "none" },
    field_assay: { steps_completed: 0, result: "none" },
    last_contribution: {
      turn: 0,
      kind: "none",
      summary: `No ${companionRole === "mira" ? "Mira" : "Jonah"} contribution has completed.`,
    },
  };
}

export function currentMiraMember(core: CanonicalObject, companionRole: CompanionRole = "mira"): CanonicalObject | null {
  const memberships = record(core.memberships, "Core memberships");
  for (const value of Object.values(memberships)) {
    const membership = record(value, "Core membership");
    if (membership.role === companionRole) return membership;
  }
  return null;
}

export function initialCompanionFromCore(core: CanonicalObject, role: CompanionRole): MiraState {
  const membership = currentMiraMember(core, role);
  if (membership === null) return initialMiraState(null, role);
  const state = initialMiraState(stringValue(membership.member_id, "member_id"), role);
  return membership.standing === "enabled" ? state : { ...state, presence: "suspended", mode: "unavailable" };
}

export function activeMiraMemberId(core: CanonicalObject, companionRole: CompanionRole = "mira"): string | null {
  const membership = currentMiraMember(core, companionRole);
  if (
    membership === null ||
    membership.access_mode !== "participant" ||
    membership.principal_kind !== "agent" ||
    membership.standing !== "enabled"
  ) return null;
  return stringValue(membership.member_id, "Mira member_id");
}

export function reconcileMira(
  state: ArchiveState,
  core: CanonicalObject,
  scheduled: CanonicalObject,
  companionRole: CompanionRole = "mira",
): { readonly state: ArchiveState; readonly timerRequests: readonly CanonicalJson[] } {
  const membership = currentMiraMember(core, companionRole);
  if (membership === null) {
    return {
      state: { ...cloneState(state), [companionRole]: initialMiraState(null, companionRole) },
      timerRequests: cancelMiraPlanningTimer(scheduled, companionRole),
    };
  }
  const memberId = stringValue(membership.member_id, "Mira member_id");
  const enabled = membership.access_mode === "participant" &&
    membership.principal_kind === "agent" && membership.standing === "enabled";
  if (state[companionRole].member_id !== memberId) {
    const replacement = initialMiraState(enabled ? memberId : null, companionRole);
    const mira = enabled
      ? replacement
      : { ...replacement, presence: "suspended" as const, member_id: memberId };
    return {
      state: { ...cloneState(state), [companionRole]: mira },
      timerRequests: cancelMiraPlanningTimer(scheduled, companionRole),
    };
  }
  if (!enabled) {
    const mira: MiraState = {
      ...state[companionRole],
      presence: "suspended",
      mode: "unavailable",
      task: cancelledTask(state[companionRole].task.revision + 1),
      opportunity: { ...emptyOpportunity(), revision: state[companionRole].opportunity.revision },
      plan: emptyPlan(),
      preparation: emptyPreparation(),
      field_assay: resetPendingAssay(state[companionRole]),
    };
    return {
      state: { ...cloneState(state), [companionRole]: mira },
      timerRequests: cancelMiraPlanningTimer(scheduled, companionRole),
    };
  }
  if (state[companionRole].presence !== "active") {
    const mira: MiraState = {
      ...state[companionRole],
      presence: "active",
      location: state[companionRole].location === "none" ? "atrium" : state[companionRole].location,
      mode: "holding",
      task: cancelledTask(state[companionRole].task.revision),
      opportunity: { ...emptyOpportunity(), revision: state[companionRole].opportunity.revision },
      plan: emptyPlan(),
      preparation: emptyPreparation(),
      field_assay: resetPendingAssay(state[companionRole]),
    };
    return { state: { ...cloneState(state), [companionRole]: mira }, timerRequests: [] };
  }
  return { state: cloneState(state), timerRequests: [] };
}

export function isMiraLeadControl(actionType: string): boolean {
  actionType = actionType.replace("_jonah_", "_mira_");
  return actionType === "assign_mira_task" || actionType === "cancel_mira_task" ||
    actionType === "set_mira_follow" || actionType === "set_mira_hold" ||
    actionType === "set_mira_regroup" || actionType === "request_mira_plan" ||
    actionType === "prepare_mira_contribution" || actionType === "defer_mira_contribution";
}

export function applyMiraLeadControl(
  current: ArchiveState,
  actionType: string,
  payload: CanonicalObject,
  core: CanonicalObject,
  admittedAt: string,
  scheduled: CanonicalObject,
  companionRole: CompanionRole = "mira",
): CompanionTransition {
  actionType = actionType.replace("_jonah_", "_mira_");
  if (current.phase !== "active") reject("inactive", "the expedition is not active");
  requireActiveMira(current, core, companionRole);
  let mira = current[companionRole];
  let timerRequests: readonly CanonicalJson[] = [];
  let attentionSignals: readonly CanonicalJson[] = [];

  if (actionType === "assign_mira_task") {
    exactKeys(payload, ["task_kind", "power_allowance"]);
    const taskKind = stringValue(payload.task_kind, "task_kind");
    if (taskKind !== "investigate_records" && taskKind !== "investigate_conservation" &&
      taskKind !== "open_service_hatch" && !(taskKind === "field_assay" && companionRole === "mira")) {
      reject("invalid_payload", "task_kind is not a bounded Mira investigation task");
    }
    const maximum = companionRole === "mira" && taskKind === "open_service_hatch" ? 2 : 1;
    const powerAllowance = boundedInteger(payload.power_allowance, "power_allowance", 0, maximum) as 0 | 1 | 2;
    timerRequests = cancelMiraPlanningTimer(scheduled, companionRole);
    mira = {
      ...mira,
      mode: "tasked",
      task: {
        status: "assigned",
        revision: mira.task.revision + 1,
        kind: taskKind,
        power_allowance: powerAllowance,
        power_spent: 0,
      },
      opportunity: { ...emptyOpportunity(), revision: mira.opportunity.revision },
      plan: emptyPlan(),
      preparation: emptyPreparation(),
      field_assay: resetPendingAssay(mira),
    };
  } else if (actionType === "cancel_mira_task") {
    exactKeys(payload, []);
    timerRequests = cancelMiraPlanningTimer(scheduled, companionRole);
    mira = {
      ...mira,
      mode: "holding",
      task: cancelledTask(mira.task.revision + 1),
      opportunity: { ...emptyOpportunity(), revision: mira.opportunity.revision },
      plan: emptyPlan(),
      preparation: emptyPreparation(),
      field_assay: resetPendingAssay(mira),
    };
  } else if (
    actionType === "set_mira_follow" || actionType === "set_mira_hold" ||
    actionType === "set_mira_regroup"
  ) {
    exactKeys(payload, []);
    timerRequests = cancelMiraPlanningTimer(scheduled, companionRole);
    mira = {
      ...mira,
      mode: actionType === "set_mira_follow"
        ? "following"
        : actionType === "set_mira_regroup"
        ? "regrouping"
        : "holding",
      task: cancelledTask(mira.task.revision + 1),
      opportunity: { ...emptyOpportunity(), revision: mira.opportunity.revision },
      plan: emptyPlan(),
      preparation: emptyPreparation(),
      field_assay: resetPendingAssay(mira),
    };
  } else if (actionType === "request_mira_plan") {
    exactKeys(payload, []);
    if (mira.mode !== "tasked" || mira.task.status !== "assigned") {
      reject("task_violation", "assign Mira an investigation task before requesting a plan");
    }
    if (current.mira.opportunity.status === "open" || current.jonah.opportunity.status === "open") {
      reject("task_violation", "Mira already has one open planning opportunity");
    }
    if (mira.plan.status === "active") {
      reject("task_violation", "Mira's current Companion Plan still has eligible steps");
    }
    const deadline = addSecondsToTimestamp(admittedAt, 15);
    const revision = mira.opportunity.revision + 1;
    mira = {
      ...mira,
      task: { ...mira.task, status: "assigned" },
      opportunity: {
        status: "open",
        revision,
        task_revision: mira.task.revision,
        opened_at: admittedAt,
        deadline,
      },
      plan: emptyPlan(),
      preparation: emptyPreparation(),
    };
    timerRequests = [{
      timer_request_type: "schedule_next",
      timer_id: companionRole === "mira" ? MIRA_PLAN_TIMER_ID : JONAH_PLAN_TIMER_ID,
      due: deadline,
      canonical_payload: {
        timer: "companion_plan_opportunity",
        opportunity_revision: revision,
        task_revision: mira.task.revision,
      },
    }];
    attentionSignals = [{
      action_types: ["submit_companion_plan"],
      deadline,
      deduplication_key: `companion_plan_requested:${mira.task.revision}:${revision}`,
      priority: 1,
      reason: MIRA_PLAN_ATTENTION_REASON,
      target_member_id: mira.member_id,
    }];
  } else if (actionType === "prepare_mira_contribution") {
    exactKeys(payload, []);
    const step = currentPlanStep(mira);
    validateStepAtTurnStart(current, mira, step, companionRole);
    mira = {
      ...mira,
      preparation: {
        status: "prepared",
        for_turn: current.turns_used + 1,
        task_revision: mira.task.revision,
        plan_revision: mira.plan.revision,
        step_index: mira.plan.next_step_index,
        summary: preparationSummary(step.step_type).replaceAll("Mira", companionRole === "mira" ? "Mira" : "Jonah"),
      },
    };
  } else if (actionType === "defer_mira_contribution") {
    exactKeys(payload, []);
    if (mira.mode !== "tasked") reject("task_violation", "Mira has no tasked contribution to defer");
    mira = {
      ...mira,
      preparation: {
        status: "deferred",
        for_turn: current.turns_used + 1,
        task_revision: mira.task.revision,
        plan_revision: mira.plan.revision,
        step_index: mira.plan.next_step_index,
        summary: `${companionRole === "mira" ? "Mira" : "Jonah"} will not contribute this turn.`,
      },
    };
  } else {
    reject("invalid_payload", "Action type is not a Mira control");
  }

  const state = { ...cloneState(current), [companionRole]: mira };
  return {
    state,
    event: asCanonical({
      event_type: "companion_state_updated",
      role: companionRole,
      action_type: actionType.replace("_mira_", `_${companionRole}_`),
      task_revision: mira.task.revision,
      planning_status: planningStatus(mira),
    }),
    timerRequests,
    attentionSignals,
  };
}

export function submitMiraPlan(
  current: ArchiveState,
  payload: CanonicalObject,
  core: CanonicalObject,
  memberId: string,
  admittedAt: string,
  scheduled: CanonicalObject,
  companionRole: CompanionRole = "mira",
): CompanionTransition {
  if (current.phase !== "active") reject("inactive", "the expedition is not active");
  const activeMemberId = requireActiveMira(current, core, companionRole);
  if (memberId !== activeMemberId) reject("role_violation", "only the current Mira Membership may submit its plan");
  exactKeys(payload, ["task_revision", "opportunity_revision", "steps"]);
  const taskRevision = boundedInteger(payload.task_revision, "task_revision", 1, 65_535);
  const opportunityRevision = boundedInteger(payload.opportunity_revision, "opportunity_revision", 1, 65_535);
  if (
    current[companionRole].mode !== "tasked" ||
    current[companionRole].task.status !== "assigned" ||
    current[companionRole].opportunity.status !== "open" ||
    taskRevision !== current[companionRole].task.revision ||
    taskRevision !== current[companionRole].opportunity.task_revision ||
    opportunityRevision !== current[companionRole].opportunity.revision ||
    compareTimestamps(admittedAt, current[companionRole].opportunity.deadline) >= 0
  ) {
    reject("stale_plan", "the submitted Companion Plan does not match the current open opportunity");
  }
  if (!Array.isArray(payload.steps) || payload.steps.length < 1 || payload.steps.length > 3) {
    reject("plan_invalid", "a Companion Plan requires one to three typed steps");
  }
  const steps = payload.steps.map((value, index) => parsePlanStep(value, `steps[${index}]`));
  validatePlannedSequence(current, current[companionRole], steps, companionRole);
  const mira: MiraState = {
    ...current[companionRole],
    opportunity: {
      ...current[companionRole].opportunity,
      status: "fulfilled",
      opened_at: "none",
      deadline: "none",
    },
    plan: {
      status: "active",
      revision: opportunityRevision,
      origin_member_id: memberId,
      origin_task_revision: taskRevision,
      origin_opportunity_revision: opportunityRevision,
      steps,
      next_step_index: 0,
    },
    preparation: emptyPreparation(),
  };
  return {
    state: { ...cloneState(current), [companionRole]: mira },
    event: asCanonical({
      event_type: "companion_state_updated",
      role: companionRole,
      action_type: "submit_companion_plan",
      task_revision: taskRevision,
      planning_status: "ready",
    }),
    timerRequests: cancelMiraPlanningTimer(scheduled, companionRole),
    attentionSignals: [],
  };
}

export function expireMiraOpportunity(
  current: ArchiveState,
  stimulus: CanonicalObject,
  companionRole: CompanionRole = "mira",
): CompanionTransition {
  if (stimulus.timer_id !== (companionRole === "mira" ? MIRA_PLAN_TIMER_ID : JONAH_PLAN_TIMER_ID)) {
    throw new TypeError("Midnight Archive received an unknown Timer");
  }
  const timer = record(stimulus.canonical_payload, "Timer payload");
  exactTypeKeys(timer, ["timer", "opportunity_revision", "task_revision"]);
  if (timer.timer !== "companion_plan_opportunity") throw new TypeError("Mira Timer payload kind is invalid");
  const opportunityRevision = boundedTypeInteger(timer.opportunity_revision, "opportunity_revision", 1, 65_535);
  const taskRevision = boundedTypeInteger(timer.task_revision, "task_revision", 1, 65_535);
  let mira = current[companionRole];
  if (
    mira.opportunity.status === "open" &&
    mira.opportunity.revision === opportunityRevision &&
    mira.opportunity.task_revision === taskRevision
  ) {
    mira = {
      ...mira,
      opportunity: {
        ...mira.opportunity,
        status: "expired",
        opened_at: "none",
        deadline: "none",
      },
      plan: emptyPlan(),
      preparation: emptyPreparation(),
    };
  }
  return {
    state: { ...cloneState(current), [companionRole]: mira },
    event: asCanonical({
      event_type: "companion_state_updated",
      role: companionRole,
      action_type: "expire_companion_plan",
      task_revision: mira.task.revision,
      planning_status: planningStatus(mira),
    }),
    timerRequests: [],
    attentionSignals: [],
  };
}

export function closeMiraAtTerminal(state: ArchiveState, companionRole: CompanionRole = "mira"): ArchiveState {
  const opportunity = state[companionRole].opportunity.status === "open"
    ? {
      ...state[companionRole].opportunity,
      status: "expired" as const,
      opened_at: "none",
      deadline: "none",
    }
    : state[companionRole].opportunity;
  return {
    ...cloneState(state),
    [companionRole]: {
      ...state[companionRole],
      opportunity,
      preparation: emptyPreparation(),
    },
  };
}

export function prepareMiraResolution(
  state: ArchiveState,
  staged: StagedAction,
  core: CanonicalObject | undefined,
  companionRole: CompanionRole = "mira",
): MiraResolution {
  if (state[companionRole].presence !== "active") return { kind: "none", step: null, destination: null };
  if (core === undefined) reject("companion_unavailable", "current Core Membership is required for Mira's turn");
  requireActiveMira(state, core, companionRole);
  if (state[companionRole].mode === "tasked") {
    const preparation = state[companionRole].preparation;
    if (preparation.status === "none" || preparation.for_turn !== state.turns_used + 1) {
      return { kind: "none", step: null, destination: null };
    }
    if (preparation.status === "deferred") {
      return { kind: "none", step: null, destination: null };
    }
    const step = currentPlanStep(state[companionRole]);
    if (
      preparation.task_revision !== state[companionRole].task.revision ||
      preparation.plan_revision !== state[companionRole].plan.revision ||
      preparation.step_index !== state[companionRole].plan.next_step_index
    ) reject("stale_plan", "Mira's prepared contribution no longer matches the current plan");
    validateStepAtTurnStart(state, state[companionRole], step, companionRole);
    return { kind: "plan", step, destination: null };
  }
  if (state[companionRole].mode === "following") {
    const location = state[companionRole].location as Location;
    if (
      staged.kind === "move" &&
      location === state.location &&
      isLocation(staged.destination) &&
      legalDestinationsFrom(state, location).includes(staged.destination)
    ) return { kind: "follow", step: null, destination: staged.destination };
    return {
      kind: "follow",
      step: null,
      destination: nextLocationToward(state, location, state.location),
    };
  }
  if (state[companionRole].mode === "regrouping") {
    return {
      kind: "regroup",
      step: null,
      destination: nextLocationToward(state, state[companionRole].location as Location, "atrium"),
    };
  }
  return { kind: "none", step: null, destination: null };
}

export function applyMiraResolution(
  before: ArchiveState,
  afterLead: ArchiveState,
  resolution: MiraResolution,
  companionRole: CompanionRole = "mira",
): ArchiveState {
  let mira = afterLead[companionRole];
  let evidence = afterLead.evidence;
  let verifierResult = afterLead.verifier_result;
  let powerRemaining = afterLead.power_remaining;
  const completesPendingAssay = companionRole === "mira" && resolution.kind === "plan" &&
    resolution.step?.step_type === "complete_field_assay";
  if (!completesPendingAssay) mira = { ...mira, field_assay: resetPendingAssay(mira) };
  if (resolution.kind === "none") {
    if (mira.mode === "tasked") mira = { ...mira, preparation: emptyPreparation() };
    return { ...afterLead, [companionRole]: mira };
  }
  if (resolution.kind === "follow" || resolution.kind === "regroup") {
    if (resolution.destination === null) return afterLead;
    const kind = resolution.kind === "follow" ? "follow_move" : "regroup_move";
    mira = {
      ...mira,
      location: resolution.destination,
      last_contribution: {
        turn: before.turns_used + 1,
        kind,
        summary: contributionSummary(kind).replaceAll("Mira", companionRole === "mira" ? "Mira" : "Jonah"),
      },
    };
    if (resolution.kind === "regroup" && resolution.destination === "atrium") {
      mira = { ...mira, mode: "holding" };
    }
    return { ...afterLead, [companionRole]: mira };
  }
  const step = resolution.step!;
  if (step.step_type === "move") {
    mira = { ...mira, location: step.destination as Location };
  } else if (step.step_type === "inspect_source") {
    const source = step.source_id as EvidenceSourceId;
    mira = {
      ...mira,
      knowledge: { ...mira.knowledge, [source]: "private" },
    };
  } else if (step.step_type === "share_source") {
    const source = step.source_id as EvidenceSourceId;
    evidence = { ...evidence, [source]: "observed" };
    mira = {
      ...mira,
      knowledge: { ...mira.knowledge, [source]: "shared" },
    };
  } else if (step.step_type === "collect_assay_sample") {
    mira = { ...mira, field_assay: { steps_completed: 1, result: "none" } };
  } else if (step.step_type === "complete_field_assay") {
    // The authored assay compares both source attributes only after the second work step.
    // Publish the same legitimate source facts as manual investigation, never private truth.
    evidence = { records: "observed", conservation: "observed" };
    mira = { ...mira, field_assay: { steps_completed: 2, result: afterLead.truth_marker } };
  } else if (step.step_type === "open_service_hatch") {
    powerRemaining -= step.power_cost;
    mira = { ...mira, task: { ...mira.task, power_spent: (mira.task.power_spent + step.power_cost) as 0 | 1 | 2 } };
  } else {
    powerRemaining -= 1;
    verifierResult = afterLead.truth_marker;
    mira = {
      ...mira,
      task: { ...mira.task, power_spent: (mira.task.power_spent + 1) as 0 | 1 },
      knowledge: { ...mira.knowledge, verifier_result: afterLead.truth_marker },
    };
  }
  const nextStepIndex = mira.plan.next_step_index + 1;
  const complete = nextStepIndex >= mira.plan.steps.length;
  const taskComplete = complete && miraTaskGoalComplete(mira, afterLead.gates.plant_vault_open || step.step_type === "open_service_hatch");
  mira = {
    ...mira,
    mode: taskComplete ? "holding" : mira.mode,
    task: taskComplete ? { ...mira.task, status: "complete" } : mira.task,
    plan: { ...mira.plan, status: complete ? "complete" : "active", next_step_index: nextStepIndex },
    preparation: emptyPreparation(),
    last_contribution: {
      turn: before.turns_used + 1,
      kind: step.step_type,
      summary: contributionSummary(step.step_type).replaceAll("Mira", companionRole === "mira" ? "Mira" : "Jonah"),
    },
  };
  return { ...afterLead, evidence, verifier_result: verifierResult, power_remaining: powerRemaining,
    gates: step.step_type === "open_service_hatch" ? { ...afterLead.gates, plant_vault_open: true } : afterLead.gates,
    [companionRole]: mira };
}

export function miraActionOffers(
  state: ArchiveState,
  role: "lead" | "mira" | "jonah",
  viewerMemberId: string,
  companionRole: CompanionRole = "mira",
): PackActionOffer[] {
  if (state.phase !== "active" || state[companionRole].presence !== "active") return [];
  if (role === companionRole) {
    if (viewerMemberId !== state[companionRole].member_id || state[companionRole].opportunity.status !== "open") return [];
    return [{
      actionType: "submit_companion_plan",
      eligibilityWindow: {
        opensAt: state[companionRole].opportunity.opened_at,
        deadline: state[companionRole].opportunity.deadline,
      },
    }];
  }
  if (role !== "lead") return [];
  const actions = ["assign_mira_task"];
  const tasked = state[companionRole].mode === "tasked" && state[companionRole].task.status === "assigned";
  if (tasked) actions.push("cancel_mira_task");
  actions.push("set_mira_follow", "set_mira_hold", "set_mira_regroup");
  if (tasked) {
    if (state.mira.opportunity.status !== "open" && state.jonah.opportunity.status !== "open" && state[companionRole].plan.status !== "active") {
      actions.push("request_mira_plan");
    }
    if (state[companionRole].plan.status === "active") actions.push("prepare_mira_contribution");
    actions.push("defer_mira_contribution");
  }
  return actions.map((actionType) => ({ actionType: actionType.replace("_mira_", `_${companionRole}_`), eligibilityWindow: null }));
}

export function planningStatus(mira: MiraState): "not_requested" | "waiting" | "ready" | "expired" | "complete" {
  if (mira.plan.status === "active") return "ready";
  if (mira.plan.status === "complete") return "complete";
  if (mira.opportunity.status === "open") return "waiting";
  if (mira.opportunity.status === "expired") return "expired";
  return "not_requested";
}

function validatePlannedSequence(state: ArchiveState, mira: MiraState, steps: readonly MiraPlanStep[], companionRole: CompanionRole): void {
  let location = mira.location as Location;
  let records = mira.knowledge.records;
  let conservation = mira.knowledge.conservation;
  let verifierUsed = state.verifier_result !== "none";
  let plannedPower = 0;
  let taskComplete = false;
  let assayProgress = mira.field_assay.steps_completed;
  if (assayProgress === 1 && (
    steps[0]?.step_type !== "complete_field_assay" ||
    mira.last_contribution.kind !== "collect_assay_sample" ||
    mira.last_contribution.turn !== state.turns_used
  )) reject("plan_invalid", "Mira's sampled assay must continue on the immediately following turn");
  for (const step of steps) {
    if (taskComplete) reject("plan_invalid", "a Companion Plan cannot continue after completing its task");
    validateStepForTask(mira.task.kind, step, companionRole);
    if (step.step_type === "move") {
      if (!legalDestinationsFrom(state, location).includes(step.destination as Location)) {
        reject("plan_invalid", "a planned move is not one currently open passage");
      }
      location = step.destination as Location;
    } else if (step.step_type === "inspect_source") {
      if (location !== step.source_id) reject("plan_invalid", "Mira must be at the planned evidence source");
      const knowledge = step.source_id === "records" ? records : conservation;
      if (knowledge !== "unknown") reject("plan_invalid", "the planned source is already known to Mira");
      if (step.source_id === "records") records = "private";
      else conservation = "private";
    } else if (step.step_type === "share_source") {
      const knowledge = step.source_id === "records" ? records : conservation;
      if (knowledge !== "private") reject("plan_invalid", "Mira may share only a privately inspected source");
      if (step.source_id === "records") records = "shared";
      else conservation = "shared";
      taskComplete = true;
    } else if (step.step_type === "open_service_hatch") {
      if (location !== "plant" || state.gates.plant_vault_open) reject("plan_invalid", "the hatch is not eligible in this plan");
      taskComplete = true;
    } else if (step.step_type === "collect_assay_sample" || step.step_type === "complete_field_assay") {
      const expectedProgress = step.step_type === "collect_assay_sample" ? 0 : 1;
      if (location !== "vault" || assayProgress !== expectedProgress) reject("plan_invalid", "the assay sequence requires two ordered Vault work steps");
      assayProgress += 1;
      taskComplete = assayProgress === 2;
    } else {
      if (location !== "records" || verifierUsed) reject("plan_invalid", "the verifier is not eligible in this plan");
      verifierUsed = true;
      taskComplete = true;
    }
    plannedPower += step.power_cost;
  }
  if (mira.task.power_spent + plannedPower > mira.task.power_allowance) {
    reject("task_violation", "the Companion Plan exceeds its task power allowance");
  }
}

function miraTaskGoalComplete(mira: MiraState, hatchOpen: boolean): boolean {
  if (mira.task.kind === "field_assay") return mira.field_assay.steps_completed === 2;
  if (mira.task.kind === "open_service_hatch") return hatchOpen;
  if (mira.task.kind === "investigate_records") {
    return mira.knowledge.records === "shared" || mira.knowledge.verifier_result !== "none";
  }
  return mira.task.kind === "investigate_conservation" && mira.knowledge.conservation === "shared";
}

function validateStepAtTurnStart(state: ArchiveState, mira: MiraState, step: MiraPlanStep, companionRole: CompanionRole): void {
  validateStepForTask(mira.task.kind, step, companionRole);
  if (
    mira.plan.origin_member_id !== mira.member_id ||
    mira.plan.origin_task_revision !== mira.task.revision ||
    mira.plan.origin_opportunity_revision !== mira.plan.revision ||
    mira.opportunity.status !== "fulfilled" ||
    mira.opportunity.revision !== mira.plan.origin_opportunity_revision
  ) {
    reject("stale_plan", "the Companion Plan origin no longer matches Mira's task");
  }
  const location = mira.location as Location;
  if (step.step_type === "move") {
    if (!legalDestinationsFrom(state, location).includes(step.destination as Location)) {
      reject("task_violation", "Mira's next planned passage is no longer open and adjacent");
    }
  } else if (step.step_type === "inspect_source") {
    const source = step.source_id as EvidenceSourceId;
    if (location !== source || mira.knowledge[source] !== "unknown") {
      reject("task_violation", "Mira's assigned source is no longer eligible for inspection");
    }
  } else if (step.step_type === "share_source") {
    const source = step.source_id as EvidenceSourceId;
    if (mira.knowledge[source] !== "private") {
      reject("task_violation", "Mira has no private inspected source to share");
    }
  } else if (step.step_type === "open_service_hatch") {
    if (location !== "plant" || state.gates.plant_vault_open ||
      mira.task.power_spent + step.power_cost > mira.task.power_allowance) {
      reject("task_violation", "the hatch is no longer eligible within this task");
    }
  } else if (step.step_type === "collect_assay_sample" || step.step_type === "complete_field_assay") {
    const continuesSample = step.step_type !== "complete_field_assay" || (
      mira.last_contribution.kind === "collect_assay_sample" &&
      mira.last_contribution.turn === state.turns_used
    );
    if (location !== "vault" || !continuesSample ||
      mira.field_assay.steps_completed !== (step.step_type === "collect_assay_sample" ? 0 : 1)) {
      reject("task_violation", "the current Vault assay prerequisite is unmet");
    }
  } else {
    if (
      location !== "records" || state.verifier_result !== "none" ||
      mira.task.power_spent + 1 > mira.task.power_allowance
    ) reject("task_violation", "Mira's verifier step is no longer eligible within its task limits");
  }
}

function validateStepForTask(taskKind: MiraTaskKind, step: MiraPlanStep, companionRole: CompanionRole): void {
  if (step.step_type === "open_service_hatch") {
    if (taskKind !== "open_service_hatch" || step.destination !== "none" || step.source_id !== "none" ||
      step.power_cost !== (companionRole === "jonah" ? 1 : 2)) reject("plan_invalid", "hatch work must match the current specialist method");
    return;
  }
  if (step.step_type === "collect_assay_sample" || step.step_type === "complete_field_assay") {
    if (companionRole !== "mira" || taskKind !== "field_assay" || step.destination !== "none" ||
      step.source_id !== "none" || step.power_cost !== 0) reject("plan_invalid", "only Mira's Vault assay task admits assay work");
    return;
  }
  const source = taskKind === "investigate_records"
    ? "records"
    : taskKind === "investigate_conservation"
    ? "conservation"
    : "none";
  if (step.step_type === "move") {
    if (step.destination === "none" || step.source_id !== "none" || step.power_cost !== 0) {
      reject("plan_invalid", "move requires a Location, source none, and zero power");
    }
    return;
  }
  if (step.step_type === "inspect_source" || step.step_type === "share_source") {
    if (step.destination !== "none" || step.source_id !== source || step.power_cost !== 0) {
      reject("plan_invalid", "source work must match the assigned source and use zero power");
    }
    return;
  }
  if (
    step.destination !== "none" || step.source_id !== "records" || step.power_cost !== 1 ||
    taskKind !== "investigate_records"
  ) reject("plan_invalid", "verifier work requires the Records task and one power");
}

function parsePlanStep(value: CanonicalJson, label: string): MiraPlanStep {
  const step = record(value, label);
  exactTypeKeys(step, ["step_type", "destination", "source_id", "power_cost"]);
  const stepType = stringValue(step.step_type, `${label}.step_type`);
  if (stepType !== "move" && stepType !== "inspect_source" && stepType !== "share_source" && stepType !== "use_verifier" &&
    stepType !== "open_service_hatch" && stepType !== "collect_assay_sample" && stepType !== "complete_field_assay") {
    reject("plan_invalid", `${label}.step_type is not a bounded plan step`);
  }
  const destination = stringValue(step.destination, `${label}.destination`);
  if (destination !== "none" && !isLocation(destination)) reject("plan_invalid", `${label}.destination is invalid`);
  const sourceId = stringValue(step.source_id, `${label}.source_id`);
  if (sourceId !== "none" && sourceId !== "records" && sourceId !== "conservation") {
    reject("plan_invalid", `${label}.source_id is invalid`);
  }
  return {
    step_type: stepType,
    destination,
    source_id: sourceId,
    power_cost: boundedInteger(step.power_cost, `${label}.power_cost`, 0, 2) as 0 | 1 | 2,
  };
}

function currentPlanStep(mira: MiraState): MiraPlanStep {
  if (mira.plan.status !== "active") reject("task_violation", "Mira has no active Companion Plan");
  const step = mira.plan.steps[mira.plan.next_step_index];
  if (step === undefined) reject("stale_plan", "Mira's Companion Plan progress is invalid");
  return step;
}

function requireActiveMira(state: ArchiveState, core: CanonicalObject, companionRole: CompanionRole = "mira"): string {
  const memberId = activeMiraMemberId(core, companionRole);
  if (
    state[companionRole].presence !== "active" || memberId === null ||
    state[companionRole].member_id !== memberId
  ) reject("companion_unavailable", "Mira is not the current enabled agent Participant");
  return memberId;
}

export function cancelMiraPlanningTimer(scheduled: CanonicalObject, companionRole: CompanionRole = "mira"): CanonicalJson[] {
  const current = scheduled[companionRole === "mira" ? MIRA_PLAN_TIMER_ID : JONAH_PLAN_TIMER_ID];
  if (current === undefined) return [];
  const timer = record(current, "scheduled Mira Timer");
  return [{
    timer_request_type: "cancel_current",
    timer_id: companionRole === "mira" ? MIRA_PLAN_TIMER_ID : JONAH_PLAN_TIMER_ID,
    expected_generation: boundedTypeInteger(timer.generation, "Timer generation", 1, Number.MAX_SAFE_INTEGER),
  }];
}

function emptyTask(): MiraState["task"] {
  return { status: "none", revision: 0, kind: "none", power_allowance: 0, power_spent: 0 };
}

function cancelledTask(revision: number): MiraState["task"] {
  return { status: "cancelled", revision, kind: "none", power_allowance: 0, power_spent: 0 };
}

function emptyOpportunity(): MiraState["opportunity"] {
  return { status: "none", revision: 0, task_revision: 0, opened_at: "none", deadline: "none" };
}

function emptyPlan(): MiraState["plan"] {
  return {
    status: "none",
    revision: 0,
    origin_member_id: "none",
    origin_task_revision: 0,
    origin_opportunity_revision: 0,
    steps: [],
    next_step_index: 0,
  };
}

function emptyPreparation(): MiraState["preparation"] {
  return {
    status: "none",
    for_turn: 0,
    task_revision: 0,
    plan_revision: 0,
    step_index: 0,
    summary: "none",
  };
}

function resetPendingAssay(mira: MiraState): MiraState["field_assay"] {
  return mira.field_assay.steps_completed === 1
    ? { steps_completed: 0, result: "none" }
    : mira.field_assay;
}

function preparationSummary(stepType: MiraPlanStep["step_type"]): string {
  if (stepType === "open_service_hatch") return "Mira will open the Plant service hatch.";
  if (stepType === "collect_assay_sample") return "Mira will collect a Vault assay sample.";
  if (stepType === "complete_field_assay") return "Mira will complete and disclose the Vault assay.";
  if (stepType === "move") return "Mira will move one open passage.";
  if (stepType === "inspect_source") return "Mira will inspect the assigned source.";
  if (stepType === "share_source") return "Mira will share one inspected source.";
  return "Mira will run the catalog verifier.";
}

function contributionSummary(kind: MiraState["last_contribution"]["kind"]): string {
  if (kind === "open_service_hatch") return "Mira opened the Plant service hatch.";
  if (kind === "collect_assay_sample") return "Mira collected a Vault assay sample; no result is available yet.";
  if (kind === "complete_field_assay") return "Mira completed the Vault assay and disclosed the verified result.";
  if (kind === "move") return "Mira moved one open passage.";
  if (kind === "inspect_source") return "Mira inspected the assigned source privately.";
  if (kind === "share_source") return "Mira shared one inspected source with the crew.";
  if (kind === "use_verifier") return "Mira ran the catalog verifier and disclosed its result.";
  if (kind === "follow_move") return "Mira followed the lead through one open passage.";
  if (kind === "regroup_move") return "Mira moved one open passage toward the Atrium.";
  return "No Mira contribution has completed.";
}

function boundedInteger(value: CanonicalJson | undefined, label: string, minimum: number, maximum: number): number {
  if (!Number.isInteger(value) || (value as number) < minimum || (value as number) > maximum) {
    reject("invalid_payload", `${label} must be an integer from ${minimum} through ${maximum}`);
  }
  return value as number;
}

function boundedTypeInteger(value: CanonicalJson | undefined, label: string, minimum: number, maximum: number): number {
  if (!Number.isInteger(value) || (value as number) < minimum || (value as number) > maximum) {
    throw new TypeError(`${label} must be an integer from ${minimum} through ${maximum}`);
  }
  return value as number;
}

function exactKeys(payload: CanonicalObject, expected: readonly string[]): void {
  const actual = Object.keys(payload).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((value, index) => value !== wanted[index])) {
    reject("invalid_payload", "Action payload fields do not match the declared action");
  }
}

function exactTypeKeys(payload: CanonicalObject, expected: readonly string[]): void {
  const actual = Object.keys(payload).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((value, index) => value !== wanted[index])) {
    throw new TypeError("payload fields do not match the declared contract");
  }
}

function compareTimestamps(left: string, right: string): number {
  return semanticMillisecond(left) - semanticMillisecond(right);
}

function addSecondsToTimestamp(value: string, seconds: number): string {
  const next = semanticMillisecond(value) + seconds * 1_000;
  const wholeSeconds = Math.floor(next / 1_000);
  const millisecond = next - wholeSeconds * 1_000;
  const days = Math.floor(wholeSeconds / 86_400);
  const secondOfDay = wholeSeconds - days * 86_400;
  const z = days + 719_468;
  const era = Math.floor((z >= 0 ? z : z - 146_096) / 146_097);
  const dayOfEra = z - era * 146_097;
  const yearOfEra = Math.floor(
    (dayOfEra - Math.floor(dayOfEra / 1_460) + Math.floor(dayOfEra / 36_524) -
      Math.floor(dayOfEra / 146_096)) / 365,
  );
  let year = yearOfEra + era * 400;
  const dayOfYear = dayOfEra -
    (365 * yearOfEra + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100));
  const monthPrime = Math.floor((5 * dayOfYear + 2) / 153);
  const day = dayOfYear - Math.floor((153 * monthPrime + 2) / 5) + 1;
  const month = monthPrime + (monthPrime < 10 ? 3 : -9);
  year += month <= 2 ? 1 : 0;
  if (year < 1 || year > 9999) throw new TypeError("semantic time is outside the supported range");
  const hour = Math.floor(secondOfDay / 3_600);
  const minute = Math.floor((secondOfDay % 3_600) / 60);
  const second = secondOfDay % 60;
  return `${pad(year, 4)}-${pad(month, 2)}-${pad(day, 2)}T${pad(hour, 2)}:${pad(minute, 2)}:${pad(second, 2)}.${pad(millisecond, 3)}Z`;
}

function semanticMillisecond(value: string): number {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?Z$/u.exec(value);
  if (match === null) throw new TypeError("semantic time must be normalized UTC RFC 3339");
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const hour = Number(match[4]);
  const minute = Number(match[5]);
  const second = Number(match[6]);
  if (
    year === 0 || month < 1 || month > 12 || day < 1 || day > daysInMonth(year, month) ||
    hour > 23 || minute > 59 || second > 59
  ) throw new TypeError("semantic time must be normalized UTC RFC 3339");
  const adjustedYear = year - (month <= 2 ? 1 : 0);
  const era = Math.floor(adjustedYear / 400);
  const yearOfEra = adjustedYear - era * 400;
  const adjustedMonth = month + (month > 2 ? -3 : 9);
  const dayOfYear = Math.floor((153 * adjustedMonth + 2) / 5) + day - 1;
  const dayOfEra = yearOfEra * 365 + Math.floor(yearOfEra / 4) -
    Math.floor(yearOfEra / 100) + dayOfYear;
  const days = era * 146_097 + dayOfEra - 719_468;
  const fraction = Number((match[7] ?? "").padEnd(3, "0").slice(0, 3));
  const result = (days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000 + fraction;
  if (!Number.isSafeInteger(result) || result < 0) throw new TypeError("semantic time is outside the supported range");
  return result;
}

function pad(value: number, length: number): string {
  return String(value).padStart(length, "0");
}

function daysInMonth(year: number, month: number): number {
  if (month === 2) return year % 400 === 0 || (year % 4 === 0 && year % 100 !== 0) ? 29 : 28;
  return month === 4 || month === 6 || month === 9 || month === 11 ? 30 : 31;
}
