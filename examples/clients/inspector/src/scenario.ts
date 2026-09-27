import { completeFixture, resultFixture, type HeistFixture, type RoomHealthState, type RuntimeRecoveryState, type ViewId } from "./fixture";

export type ConsoleScenarioName = "healthy" | "complete" | "loading" | "catching-up" | "faulted" | "quarantined";

export interface ConsoleScenario {
  fixture: HeistFixture;
  initialView: ViewId;
  name: ConsoleScenarioName;
}

const views: readonly ViewId[] = ["public", "participant", "operator", "replay"];
const scenarios: readonly ConsoleScenarioName[] = ["healthy", "complete", "loading", "catching-up", "faulted", "quarantined"];

/** Select deterministic, credential-free reference scenarios for browser smoke only. */
export function scenarioFromSearch(search: string): ConsoleScenario {
  const params = new URLSearchParams(search);
  const requestedScenario = params.get("scenario");
  const requestedView = params.get("view");
  const name = scenarios.includes(requestedScenario as ConsoleScenarioName) ? requestedScenario as ConsoleScenarioName : "healthy";
  const initialView = views.includes(requestedView as ViewId) ? requestedView as ViewId : "public";
  return { name, initialView, fixture: fixtureForScenario(name) };
}

export function fixtureForScenario(name: ConsoleScenarioName): HeistFixture {
  if (name === "complete") return completeFixture;
  if (name === "healthy") return resultFixture;
  return {
    ...resultFixture,
    fixtureLabel: "Fixture-backed " + name + " reference · no server connection",
    runtime: runtimeForScenario(name),
    operator: operatorForScenario(name),
  };
}

function runtimeForScenario(name: Exclude<ConsoleScenarioName, "healthy" | "complete">): HeistFixture["runtime"] {
  const base = resultFixture.runtime;
  const common = { ...base, transport: "Unavailable" as const, roomHealth: "Healthy" as RoomHealthState };
  if (name === "loading") {
    return { ...common, recovery: "Loading" as RuntimeRecoveryState, frame: { ...base.frame, delivery: "Unavailable", reset: "Unavailable", syncAck: "Unavailable", observationAck: "Unavailable" }, reason: "Room state is loading; no current Projection or mutation controls are trusted." };
  }
  if (name === "catching-up") {
    return { ...common, transport: "Attached", recovery: "CatchingUp" as RuntimeRecoveryState, frame: { ...base.frame, delivery: "Catching up", reset: "Required", syncAck: "Pending", observationAck: "Pending" }, reason: "Retained Frames or a Projection Reset are being installed before the Session becomes live." };
  }
  if (name === "faulted") {
    return { ...common, transport: "Disconnected", recovery: "Active" as RuntimeRecoveryState, frame: { ...base.frame, delivery: "Reset required", reset: "Required", syncAck: "Unavailable", observationAck: "Unavailable" }, roomHealth: "Faulted", reason: "The last verified Projection remains available; canonical mutation is disabled." };
  }
  return { ...common, transport: "Disconnected", recovery: "Inactive" as RuntimeRecoveryState, frame: { ...base.frame, delivery: "Unavailable", reset: "Unavailable", syncAck: "Unavailable", observationAck: "Unavailable" }, roomHealth: "Quarantined", reason: "Room integrity cannot be established; normal Projection, Catch-up, and Replay are withheld." };
}

function operatorForScenario(name: Exclude<ConsoleScenarioName, "healthy" | "complete">): HeistFixture["operator"] {
  const state = name === "faulted" ? "Faulted" : name === "quarantined" ? "Quarantined" : "Healthy";
  return {
    ...resultFixture.operator,
    integrity: {
      ...resultFixture.operator.integrity,
      state,
      safeReason: state === "Healthy" ? resultFixture.operator.integrity.safeReason : "Room is " + state.toLowerCase() + "; canonical mutation is disabled",
    },
  };
}
