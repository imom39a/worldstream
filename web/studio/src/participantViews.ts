export interface ActivityClientCandidate {
  candidate_id: string;
  deployment_id: string;
  client_id: string;
  release_digest: string;
  surface_id: string;
  trust_level: "verified" | "externally_trusted";
}

export type ActivityClientOpenResult =
  | { state: "opened" }
  | { state: "selection_required"; candidates: ActivityClientCandidate[] }
  | { state: "failed" };

export async function openParticipantClient(
  draftId: string,
  seatId: string,
  candidateId: string | null = null,
  fetcher: typeof fetch = fetch,
  opener: Pick<typeof window, "open"> = window,
): Promise<ActivityClientOpenResult> {
  if (!isId(draftId) || !isId(seatId) || (candidateId !== null && !isId(candidateId))) {
    return { state: "failed" };
  }
  const request = candidateId === null
    ? { draft_id: draftId, seat_id: seatId }
    : { draft_id: draftId, seat_id: seatId, candidate_id: candidateId };
  try {
    const response = await fetcher("/api/v1/participant-console/handoffs", {
      method: "POST",
      headers: { Accept: "application/json", "Content-Type": "application/json" },
      body: JSON.stringify(request),
    });
    if (!response.ok) return { state: "failed" };
    const value: unknown = await response.json();
    if (isReadyResponse(value)) {
      opener.open(value.client_url, "_blank", "noopener,noreferrer");
      return { state: "opened" };
    }
    if (isSelectionRequiredResponse(value)) {
      return { state: "selection_required", candidates: value.candidates };
    }
    return { state: "failed" };
  } catch {
    return { state: "failed" };
  }
}

function isReadyResponse(value: unknown): value is {
  version: "activity_client_handoff.v1";
  state: "ready";
  client_url: string;
} {
  if (!isExactRecord(value, ["version", "state", "client_url"])) return false;
  if (
    value.version !== "activity_client_handoff.v1"
    || value.state !== "ready"
    || typeof value.client_url !== "string"
  ) return false;
  try {
    const url = new URL(value.client_url);
    return url.protocol === "http:"
      && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname)
      && url.pathname.startsWith("/")
      && url.pathname.endsWith("/")
      && url.search === ""
      && /^#handoff=wsh1:[0-9a-f]{64}$/.test(url.hash)
      && url.username === ""
      && url.password === "";
  } catch {
    return false;
  }
}

function isSelectionRequiredResponse(value: unknown): value is {
  version: "activity_client_handoff.v1";
  state: "selection_required";
  candidates: ActivityClientCandidate[];
} {
  if (!isExactRecord(value, ["version", "state", "candidates"])) return false;
  if (
    value.version !== "activity_client_handoff.v1"
    || value.state !== "selection_required"
    || !Array.isArray(value.candidates)
    || value.candidates.length < 2
    || value.candidates.length > 32
    || !value.candidates.every(isCandidate)
  ) return false;
  return new Set(value.candidates.map((candidate) => candidate.candidate_id)).size
    === value.candidates.length;
}

function isCandidate(value: unknown): value is ActivityClientCandidate {
  return isExactRecord(value, [
    "candidate_id",
    "deployment_id",
    "client_id",
    "release_digest",
    "surface_id",
    "trust_level",
  ])
    && typeof value.candidate_id === "string" && isId(value.candidate_id)
    && typeof value.deployment_id === "string" && isId(value.deployment_id)
    && typeof value.client_id === "string" && isId(value.client_id)
    && typeof value.release_digest === "string"
    && /^(?:blake3|sha256):[0-9a-f]{64}$/.test(value.release_digest)
    && typeof value.surface_id === "string" && isId(value.surface_id)
    && (value.trust_level === "verified" || value.trust_level === "externally_trusted");
}

function isExactRecord(
  value: unknown,
  keys: readonly string[],
): value is Record<string, unknown> {
  return value !== null
    && typeof value === "object"
    && !Array.isArray(value)
    && Object.keys(value).length === keys.length
    && keys.every((key) => key in value);
}

function isId(value: string): boolean {
  return /^[a-z0-9][a-z0-9._-]{0,127}$/.test(value);
}
