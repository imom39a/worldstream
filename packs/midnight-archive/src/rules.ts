import type { CanonicalJson, CanonicalObject } from "@worldstream/pack-sdk";

import {
  type ArchiveOutcome,
  type ArchiveState,
  type CandidateId,
  type Location,
  type StagedAction,
  asCanonical,
  cloneState,
  isCandidateId,
  isLocation,
  record,
  reject,
  stringValue,
} from "./model.js";

export const OBJECTIVE =
  "Recover the authentic ledger, return to the Atrium, and extract before turn 16 ends.";

function visibleCandidates() {
  return [
    { candidate_id: "ledger-amber", binding: "calfskin", marking: "compass_rose", year: 1891 },
    { candidate_id: "ledger-cobalt", binding: "linen", marking: "split_star", year: 1891 },
    { candidate_id: "ledger-violet", binding: "calfskin", marking: "split_star", year: 1904 },
  ] as const;
}

function emptyStage(): StagedAction {
  return {
    kind: "none",
    destination: "none",
    candidate_id: "none",
    turn_cost: 0,
    power_cost: 0,
  };
}

function pendingOutcome(): ArchiveOutcome {
  return {
    kind: "pending",
    factual_reason: "The expedition is still inside the archive.",
    extracted_candidate_id: "none",
  };
}

export function initializeArchiveState(configuration: CanonicalObject): ArchiveState {
  if (
    Object.keys(configuration).length !== 1 ||
    configuration.scenario_id !== "standard-v1"
  ) {
    throw new TypeError("configuration must select the exact standard-v1 scenario");
  }
  return {
    phase: "briefing",
    scenario_id: "standard-v1",
    objective: OBJECTIVE,
    location: "atrium",
    turn_limit: 16,
    turns_used: 0,
    power_remaining: 3,
    gates: {
      conservation_vault_open: false,
      plant_vault_open: false,
    },
    candidates: visibleCandidates(),
    truth_marker: "ledger-cobalt",
    verifier_result: "none",
    carried_candidate_id: "none",
    carried_confidence: "none",
    staged_action: emptyStage(),
    outcome: pendingOutcome(),
    role_notes: {
      lead: "You decide which ledger to recover and when to extract.",
      mira: "Mira wants every claim grounded in visible evidence.",
      jonah: "Jonah wants the source record protected.",
    },
  };
}

export function startArchive(state: ArchiveState): ArchiveState {
  if (state.phase !== "briefing") reject("inactive", "the archive briefing is already closed");
  return { ...cloneState(state), phase: "active" };
}

export interface AppliedLeadAction {
  readonly state: ArchiveState;
  readonly event: CanonicalObject;
}

export function applyLeadAction(
  current: ArchiveState,
  actionType: string,
  payload: CanonicalObject,
): AppliedLeadAction {
  if (current.phase !== "active") reject("inactive", "the expedition is not active");
  if (actionType === "commit_turn") return commitTurn(current, payload);
  const staged = stagedAction(current, actionType, payload);
  const state = { ...cloneState(current), staged_action: staged };
  return {
    state,
    event: asCanonical({
      event_type: "action_staged",
      staged_action_type: publicActionType(staged),
      turn_cost: staged.turn_cost,
      power_cost: staged.power_cost,
    }),
  };
}

function stagedAction(
  state: ArchiveState,
  actionType: string,
  payload: CanonicalObject,
): StagedAction {
  if (actionType === "stage_move") {
    exactKeys(payload, ["destination"]);
    const destination = stringValue(payload.destination, "destination");
    if (!isLocation(destination)) reject("invalid_payload", "destination is not a Location");
    if (!legalDestinations(state).includes(destination)) {
      if (isVaultEdge(state.location, destination)) reject("gate_closed", "the selected vault gate is closed");
      reject("illegal_action", "destination is not adjacent to the current Location");
    }
    return {
      kind: "move",
      destination,
      candidate_id: "none",
      turn_cost: 1,
      power_cost: 0,
    };
  }
  if (actionType === "stage_use_verifier") {
    exactKeys(payload, []);
    if (state.location !== "records") reject("illegal_action", "the verifier is in Records");
    requirePower(state, 1);
    return {
      kind: "use_verifier",
      destination: "none",
      candidate_id: "none",
      turn_cost: 1,
      power_cost: 1,
    };
  }
  if (actionType === "stage_open_service_hatch") {
    exactKeys(payload, []);
    if (state.location !== "plant") reject("illegal_action", "the service hatch controls are in Plant");
    if (state.gates.plant_vault_open) reject("illegal_action", "the service hatch is already open");
    requirePower(state, 2);
    return {
      kind: "open_service_hatch",
      destination: "none",
      candidate_id: "none",
      turn_cost: 1,
      power_cost: 2,
    };
  }
  if (actionType === "stage_recover_candidate") {
    exactKeys(payload, ["candidate_id"]);
    if (state.location !== "vault") reject("illegal_action", "candidate ledgers are in the Vault");
    const candidateId = stringValue(payload.candidate_id, "candidate_id");
    if (!isCandidateId(candidateId)) {
      reject("unknown_candidate", "candidate_id is not present in this scenario");
    }
    return {
      kind: "recover_candidate",
      destination: "none",
      candidate_id: candidateId,
      turn_cost: 1,
      power_cost: 0,
    };
  }
  if (actionType === "stage_extract") {
    exactKeys(payload, []);
    if (state.location !== "atrium") reject("illegal_action", "extraction is available only in the Atrium");
    return {
      kind: "extract",
      destination: "none",
      candidate_id: "none",
      turn_cost: 1,
      power_cost: 0,
    };
  }
  if (actionType === "stage_wait") {
    exactKeys(payload, []);
    return {
      kind: "wait",
      destination: "none",
      candidate_id: "none",
      turn_cost: 1,
      power_cost: 0,
    };
  }
  reject("invalid_payload", "Action type is not declared by Midnight Archive");
}

function commitTurn(current: ArchiveState, payload: CanonicalObject): AppliedLeadAction {
  exactKeys(payload, []);
  if (current.staged_action.kind === "none") {
    reject("nothing_staged", "stage one personal action before committing the turn");
  }
  const staged = current.staged_action;
  validateStagedAtTurnStart(current, staged);
  const next = cloneState(current) as MutableArchiveState;
  const kind = staged.kind;
  if (kind === "move") {
    next.location = staged.destination as Location;
  } else if (kind === "use_verifier") {
    next.power_remaining -= 1;
    next.verifier_result = next.truth_marker;
    if (next.carried_candidate_id !== "none") {
      next.carried_confidence = next.carried_candidate_id === next.verifier_result
        ? "verified"
        : "unverified";
    }
  } else if (kind === "open_service_hatch") {
    next.power_remaining -= 2;
    next.gates = { ...next.gates, plant_vault_open: true };
  } else if (kind === "recover_candidate") {
    next.carried_candidate_id = staged.candidate_id;
    next.carried_confidence = staged.candidate_id === next.verifier_result
      ? "verified"
      : "unverified";
  }

  next.turns_used += 1;
  next.staged_action = emptyStage();
  if (kind === "extract") {
    next.phase = "complete";
    next.outcome = extractionOutcome(next);
  } else if (next.turns_used >= next.turn_limit) {
    next.phase = "complete";
    next.outcome = {
      kind: "exhausted_inside",
      factual_reason: "The turn budget ended with the crew inside the archive.",
      extracted_candidate_id: "none",
    };
  }
  return {
    state: next,
    event: asCanonical({
      event_type: "turn_committed",
      committed_action_type: publicActionType(staged),
      turn: next.turns_used,
      location: next.location,
      power: next.power_remaining,
      outcome_kind: next.outcome.kind,
    }),
  };
}

function validateStagedAtTurnStart(state: ArchiveState, staged: StagedAction): void {
  if (staged.kind === "move") {
    if (!isLocation(staged.destination) || !legalDestinations(state).includes(staged.destination)) {
      if (isLocation(staged.destination) && isVaultEdge(state.location, staged.destination)) {
        reject("gate_closed", "the selected vault gate was closed at the beginning of the turn");
      }
      reject("illegal_action", "the staged destination is no longer adjacent");
    }
  } else if (staged.kind === "use_verifier") {
    if (state.location !== "records") reject("illegal_action", "the verifier is no longer reachable");
    requirePower(state, 1);
  } else if (staged.kind === "open_service_hatch") {
    if (state.location !== "plant" || state.gates.plant_vault_open) {
      reject("illegal_action", "the service hatch action is no longer eligible");
    }
    requirePower(state, 2);
  } else if (staged.kind === "recover_candidate") {
    if (state.location !== "vault") reject("illegal_action", "the Vault is no longer occupied");
    if (!isCandidateId(staged.candidate_id)) reject("unknown_candidate", "the staged candidate is absent");
  } else if (staged.kind === "extract" && state.location !== "atrium") {
    reject("illegal_action", "the crew is no longer at the Atrium");
  }
}

function extractionOutcome(state: ArchiveState): ArchiveOutcome {
  if (state.carried_candidate_id === "none") {
    return {
      kind: "no_ledger",
      factual_reason: "The crew extracted without a ledger.",
      extracted_candidate_id: "none",
    };
  }
  if (state.carried_candidate_id === state.truth_marker) {
    return {
      kind: "success",
      factual_reason: "The extracted ledger matches the archive's authentic record.",
      extracted_candidate_id: state.carried_candidate_id,
    };
  }
  return {
    kind: "wrong_ledger",
    factual_reason: "The extracted ledger does not match the archive's authentic record.",
    extracted_candidate_id: state.carried_candidate_id,
  };
}

export function legalDestinations(state: ArchiveState): Location[] {
  const destinations: Location[] = [];
  const add = (left: Location, right: Location, open = true): void => {
    if (!open) return;
    if (state.location === left) destinations.push(right);
    if (state.location === right) destinations.push(left);
  };
  add("atrium", "records");
  add("atrium", "conservation");
  add("records", "conservation");
  add("records", "plant");
  add("conservation", "vault", state.gates.conservation_vault_open);
  add("plant", "vault", state.gates.plant_vault_open);
  return destinations.sort();
}

function isVaultEdge(left: Location, right: Location): boolean {
  return (
    (left === "conservation" && right === "vault") ||
    (left === "vault" && right === "conservation") ||
    (left === "plant" && right === "vault") ||
    (left === "vault" && right === "plant")
  );
}

function requirePower(state: ArchiveState, cost: number): void {
  if (state.power_remaining < cost) {
    reject("insufficient_power", `the staged action requires ${cost} power`);
  }
}

function exactKeys(payload: CanonicalObject, expected: readonly string[]): void {
  const actual = Object.keys(payload).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((value, index) => value !== wanted[index])) {
    reject("invalid_payload", "Action payload fields do not match the declared action");
  }
}

function publicActionType(staged: StagedAction): string {
  if (staged.kind === "move") return "stage_move";
  if (staged.kind === "use_verifier") return "stage_use_verifier";
  if (staged.kind === "open_service_hatch") return "stage_open_service_hatch";
  if (staged.kind === "recover_candidate") return "stage_recover_candidate";
  if (staged.kind === "extract") return "stage_extract";
  if (staged.kind === "wait") return "stage_wait";
  return "none";
}

type MutableArchiveState = {
  -readonly [Key in keyof ArchiveState]: ArchiveState[Key];
};

export function stateAsCanonical(state: ArchiveState): CanonicalObject {
  return state as unknown as CanonicalObject;
}

export function payload(value: CanonicalJson | undefined): CanonicalObject {
  return record(value, "Action payload");
}
