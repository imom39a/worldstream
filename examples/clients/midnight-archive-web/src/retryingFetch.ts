const RETRYABLE_PATHS = new Set([
  "/api/v1/participant-console/session:observe",
  "/api/v1/participant-console/session:acknowledge",
  "/api/v1/participant-console/session:act",
  "/api/v1/participant-console/session:replay",
]);
const RETRY_DELAYS_MS = [250, 500] as const;

/**
 * Midnight Archive owns its transient Controller recovery policy. It repeats
 * only exact, already-idempotent Browser Handoff requests and returns the
 * original response unchanged for every other outcome.
 */
export function createMidnightArchiveRetryingFetch(
  fetchImplementation: typeof fetch = globalThis.fetch,
): typeof fetch {
  return async (input, init) => {
    const pathname = requestPathname(input);
    for (let attempt = 0; ; attempt += 1) {
      const response = await fetchImplementation(input, init);
      const delay = RETRY_DELAYS_MS[attempt];
      if (
        delay === undefined
        || !RETRYABLE_PATHS.has(pathname)
        || !await isRetryableUnavailable(response)
      ) return response;
      await new Promise<void>((resolve) => setTimeout(resolve, delay));
    }
  };
}

function requestPathname(input: RequestInfo | URL): string {
  if (typeof input === "string") return new URL(input, "http://activity-client.invalid").pathname;
  if (input instanceof URL) return input.pathname;
  return new URL(input.url).pathname;
}

async function isRetryableUnavailable(response: Response): Promise<boolean> {
  if (response.status !== 502) return false;
  let body: unknown;
  try {
    body = await response.clone().json();
  } catch {
    return false;
  }
  if (body === null || typeof body !== "object" || Array.isArray(body)) return false;
  const error = body as Record<string, unknown>;
  return Object.keys(error).sort().join("\0") === ["code", "message", "next_action", "retryable"].join("\0")
    && error.code === "participant_session_unavailable"
    && error.next_action === "reconnect"
    && error.retryable === true
    && typeof error.message === "string";
}
