export type DaemonConnectivity = "connected" | "unavailable";
export type DaemonHealth = "live" | "unavailable";
export type DaemonReadiness = "ready" | "not_ready" | "unavailable";
export type DaemonUnavailableReason =
  | "connection_failed"
  | "invalid_response"
  | "probe_failed";

export interface DaemonVersion {
  product: string;
  build_version: string;
  source_revision: string;
}

export interface DaemonStatus {
  schema: "worldstream/studio-daemon-status/v1";
  connectivity: DaemonConnectivity;
  health: DaemonHealth;
  readiness: DaemonReadiness;
  version: DaemonVersion | null;
  unavailable_reason: DaemonUnavailableReason | null;
}

type Fetcher = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

const unavailableStatus: DaemonStatus = {
  schema: "worldstream/studio-daemon-status/v1",
  connectivity: "unavailable",
  health: "unavailable",
  readiness: "unavailable",
  version: null,
  unavailable_reason: "probe_failed",
};

export async function loadDaemonStatus(
  fetcher: Fetcher = fetch,
): Promise<DaemonStatus> {
  try {
    const response = await fetcher("/api/v1/daemon/status", {
      headers: { accept: "application/json" },
    });
    if (!response.ok) {
      return unavailableStatus;
    }
    const value: unknown = await response.json();
    return isDaemonStatus(value) ? value : unavailableStatus;
  } catch {
    return unavailableStatus;
  }
}

function isDaemonStatus(value: unknown): value is DaemonStatus {
  if (!isRecord(value)) {
    return false;
  }
  const version = value.version;
  return (
    value.schema === "worldstream/studio-daemon-status/v1" &&
    isOneOf(value.connectivity, ["connected", "unavailable"]) &&
    isOneOf(value.health, ["live", "unavailable"]) &&
    isOneOf(value.readiness, ["ready", "not_ready", "unavailable"]) &&
    (value.unavailable_reason === null ||
      isOneOf(value.unavailable_reason, [
        "connection_failed",
        "invalid_response",
        "probe_failed",
      ])) &&
    (version === null ||
      (isRecord(version) &&
        isBoundedString(version.product) &&
        isBoundedString(version.build_version) &&
        isBoundedString(version.source_revision))) &&
    ((value.connectivity === "connected" && version !== null) ||
      (value.connectivity === "unavailable" && version === null))
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

function isBoundedString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 128;
}
