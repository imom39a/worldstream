import { describe, expect, it, vi } from "vitest";

import { loadDaemonStatus } from "./daemonStatus";

describe("Supervisor daemon status client", () => {
  it("obtains status only from the typed Supervisor endpoint", async () => {
    const fetcher = vi.fn(async () =>
      new Response(
        JSON.stringify({
          schema: "worldstream/studio-daemon-status/v1",
          connectivity: "connected",
          health: "live",
          readiness: "ready",
          version: {
            product: "0.1.0",
            build_version: "0.1.0",
            source_revision: "0123456789abcdef0123456789abcdef01234567",
          },
          unavailable_reason: null,
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    );

    const status = await loadDaemonStatus(fetcher);

    expect(fetcher).toHaveBeenCalledWith("/api/v1/daemon/status", {
      headers: { accept: "application/json" },
    });
    expect(status.connectivity).toBe("connected");
    expect(status.version?.build_version).toBe("0.1.0");
  });

  it("fails closed when the Supervisor response is unavailable or malformed", async () => {
    const unavailable = await loadDaemonStatus(async () => {
      throw new Error("offline");
    });
    const malformed = await loadDaemonStatus(
      async () => new Response('{"connectivity":"connected"}', { status: 200 }),
    );

    expect(unavailable.connectivity).toBe("unavailable");
    expect(unavailable.version).toBeNull();
    expect(malformed.connectivity).toBe("unavailable");
    expect(malformed.version).toBeNull();
  });
});
