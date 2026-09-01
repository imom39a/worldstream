export async function openParticipantClient(
  draftId: string,
  seatId: string,
  fetcher: typeof fetch = fetch,
  opener: Pick<typeof window, "open"> = window,
): Promise<boolean> {
  if (!isId(draftId) || !isId(seatId)) return false;
  try {
    const response = await fetcher("/api/v1/participant-console/handoffs", {
      method: "POST",
      headers: { Accept: "application/json", "Content-Type": "application/json" },
      body: JSON.stringify({ draft_id: draftId, seat_id: seatId }),
    });
    if (!response.ok) return false;
    const value: unknown = await response.json();
    if (!isResponse(value)) return false;
    opener.open(value.console_url, "_blank", "noopener,noreferrer");
    return true;
  } catch {
    return false;
  }
}

function isResponse(value: unknown): value is { version: "participant_handoff.v1"; console_url: string } {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  if (Object.keys(record).length !== 2 || record.version !== "participant_handoff.v1" ||
    typeof record.console_url !== "string") return false;
  try {
    const url = new URL(record.console_url);
    return url.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname) &&
      ["/agent-heist/", "/inspector/"].includes(url.pathname) && url.search === "" &&
      /^#handoff=wsh1:[0-9a-f]{64}$/.test(url.hash) && url.username === "" && url.password === "";
  } catch {
    return false;
  }
}

function isId(value: string): boolean {
  return /^[a-z0-9][a-z0-9_-]{0,127}$/.test(value);
}
