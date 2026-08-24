export type SecretKind =
  | "host_authority"
  | "membership_authority"
  | "runner_authority"
  | "model_provider";

export type SecretAvailability = "configured" | "missing" | "unavailable";

export interface SecretKindStatus {
  kind: SecretKind;
  availability: SecretAvailability;
}

export interface SecretStatusResponse {
  schema: "worldstream/studio-secret-kind-status/v1";
  credentials: SecretKindStatus[];
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const unavailable: SecretStatusResponse = {
  schema: "worldstream/studio-secret-kind-status/v1",
  credentials: [
    { kind: "host_authority", availability: "unavailable" },
    { kind: "membership_authority", availability: "unavailable" },
    { kind: "runner_authority", availability: "unavailable" },
    { kind: "model_provider", availability: "unavailable" },
  ],
};

export async function loadSecretStatus(
  fetcher: Fetcher = fetch,
): Promise<SecretStatusResponse> {
  try {
    const response = await fetcher("/api/v1/secrets", {
      headers: { accept: "application/json" },
    });
    if (!response.ok) return unavailable;
    const value: unknown = await response.json();
    return isSecretStatus(value) ? value : unavailable;
  } catch {
    return unavailable;
  }
}

function isSecretStatus(value: unknown): value is SecretStatusResponse {
  if (!isRecord(value) || value.schema !== "worldstream/studio-secret-kind-status/v1") {
    return false;
  }
  if (!Array.isArray(value.credentials) || value.credentials.length !== 4) return false;
  const kinds = new Set<SecretKind>();
  for (const row of value.credentials) {
    if (
      !isRecord(row) ||
      Object.keys(row).some((key) => key !== "kind" && key !== "availability") ||
      !isKind(row.kind) ||
      !isAvailability(row.availability)
    ) {
      return false;
    }
    kinds.add(row.kind);
  }
  return kinds.size === 4;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isKind(value: unknown): value is SecretKind {
  return (
    value === "host_authority" ||
    value === "membership_authority" ||
    value === "runner_authority" ||
    value === "model_provider"
  );
}

function isAvailability(value: unknown): value is SecretAvailability {
  return value === "configured" || value === "missing" || value === "unavailable";
}
