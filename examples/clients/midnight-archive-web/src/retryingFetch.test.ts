import { describe, expect, it, vi } from "vitest";

import { createMidnightArchiveRetryingFetch } from "./retryingFetch";

const actionUrl = "http://127.0.0.1:9420/api/v1/participant-console/session:act";
const actionInit = {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ action_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY" }),
} satisfies RequestInit;

function unavailable() {
  return Response.json({
    code: "participant_session_unavailable",
    message: "The participant client cannot reach the Room service safely.",
    next_action: "reconnect",
    retryable: true,
  }, { status: 502 });
}

describe("Midnight Archive retrying fetch", () => {
  it("retries only the bounded unavailable response while preserving the exact request", async () => {
    vi.useFakeTimers();
    try {
      const upstream = vi.fn()
        .mockResolvedValueOnce(unavailable())
        .mockResolvedValueOnce(unavailable())
        .mockResolvedValueOnce(Response.json({ state: "accepted" }));
      const retryingFetch = createMidnightArchiveRetryingFetch(upstream);

      const result = retryingFetch(actionUrl, actionInit);
      await vi.runAllTimersAsync();
      await expect(result).resolves.toMatchObject({ status: 200 });
      expect(upstream).toHaveBeenCalledTimes(3);
      expect(upstream.mock.calls).toEqual([
        [actionUrl, actionInit],
        [actionUrl, actionInit],
        [actionUrl, actionInit],
      ]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not retry network, unsafe, invalid, or non-idempotent responses", async () => {
    const cases: readonly [string, () => Promise<Response>][] = [
      ["network", () => Promise.reject(new TypeError("offline"))],
      ["unsafe", () => Promise.resolve(Response.json({
        code: "participant_session_unavailable",
        message: "safe",
        next_action: "reconnect",
        retryable: true,
        room_id: "private",
      }, { status: 502 }))],
      ["invalid", () => Promise.resolve(new Response("not json", { status: 502 }))],
      ["other path", () => Promise.resolve(unavailable())],
    ];
    for (const [kind, implementation] of cases) {
      const upstream = vi.fn(implementation);
      const retryingFetch = createMidnightArchiveRetryingFetch(upstream);
      if (kind === "network") await expect(retryingFetch(actionUrl, actionInit)).rejects.toThrow("offline");
      else await expect(retryingFetch(kind === "other path" ? "http://127.0.0.1:9420/api/v1/participant-console/handoffs:redeem" : actionUrl, actionInit)).resolves.toMatchObject({ status: 502 });
      expect(upstream).toHaveBeenCalledTimes(1);
    }
  });

  it("stops after its two bounded delays", async () => {
    vi.useFakeTimers();
    try {
      const upstream = vi.fn(() => Promise.resolve(unavailable()));
      const retryingFetch = createMidnightArchiveRetryingFetch(upstream);

      const result = retryingFetch(actionUrl, actionInit);
      await vi.runAllTimersAsync();
      await expect(result).resolves.toMatchObject({ status: 502 });
      expect(upstream).toHaveBeenCalledTimes(3);
    } finally {
      vi.useRealTimers();
    }
  });
});
