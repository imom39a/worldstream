export type DaemonLifecycleState =
  | "stopped"
  | "starting"
  | "running"
  | "stopping"
  | "failed"
  | "unavailable";

export type DaemonLifecycleAction = "start" | "stop" | "restart";

export interface DaemonLifecycleFailure {
  code: string;
  explanation: string;
  next_action: string;
}

export interface DaemonLifecycle {
  schema: "worldstream/studio-daemon-lifecycle/v1";
  state: DaemonLifecycleState;
  operation_id: number;
  managed_by_supervisor: boolean;
  failure: DaemonLifecycleFailure | null;
}

type Fetcher = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

const unavailableLifecycle: DaemonLifecycle = {
  schema: "worldstream/studio-daemon-lifecycle/v1",
  state: "unavailable",
  operation_id: 0,
  managed_by_supervisor: false,
  failure: {
    code: "operation_failed",
    explanation: "Studio could not obtain a lifecycle response from the Supervisor.",
    next_action: "Reconnect or restart Studio Supervisor, then retry.",
  },
};

export async function loadDaemonLifecycle(
  fetcher: Fetcher = fetch,
): Promise<DaemonLifecycle> {
  return request("/api/v1/daemon/lifecycle", undefined, fetcher);
}

export async function requestDaemonLifecycle(
  action: DaemonLifecycleAction,
  fetcher: Fetcher = fetch,
): Promise<DaemonLifecycle> {
  return request(`/api/v1/daemon/${action}`, "POST", fetcher);
}

async function request(
  path: string,
  method: "POST" | undefined,
  fetcher: Fetcher,
): Promise<DaemonLifecycle> {
  try {
    const response = await fetcher(path, {
      ...(method === undefined ? {} : { method }),
      headers: { accept: "application/json" },
    });
    if (!response.ok) return unavailableLifecycle;
    const value: unknown = await response.json();
    return isDaemonLifecycle(value) ? value : unavailableLifecycle;
  } catch {
    return unavailableLifecycle;
  }
}

function isDaemonLifecycle(value: unknown): value is DaemonLifecycle {
  if (!isRecord(value)) return false;
  const failure = value.failure;
  return (
    value.schema === "worldstream/studio-daemon-lifecycle/v1" &&
    isOneOf(value.state, [
      "stopped",
      "starting",
      "running",
      "stopping",
      "failed",
      "unavailable",
    ]) &&
    typeof value.operation_id === "number" &&
    Number.isSafeInteger(value.operation_id) &&
    value.operation_id >= 0 &&
    typeof value.managed_by_supervisor === "boolean" &&
    (failure === null ||
      (isRecord(failure) &&
        isBoundedString(failure.code, 64) &&
        isBoundedString(failure.explanation, 256) &&
        isBoundedString(failure.next_action, 256))) &&
    (value.state === "failed" || value.state === "unavailable"
      ? failure !== null
      : failure === null)
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isOneOf<T extends string>(
  value: unknown,
  choices: readonly T[],
): value is T {
  return typeof value === "string" && choices.includes(value as T);
}

function isBoundedString(value: unknown, max: number): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= max;
}
