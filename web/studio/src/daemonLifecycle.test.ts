import { describe, expect, it, vi } from "vitest";

import { loadDaemonLifecycle, requestDaemonLifecycle } from "./daemonLifecycle";

const running = {
  schema: "worldstream/studio-daemon-lifecycle/v1",
  state: "running",
  operation_id: 7,
  managed_by_supervisor: true,
  failure: null,
} as const;

describe("Supervisor daemon lifecycle client", () => {
  it("loads only the typed lifecycle endpoint", async () => {
    const fetcher = vi.fn(async () => Response.json(running));

    await expect(loadDaemonLifecycle(fetcher)).resolves.toEqual(running);
    expect(fetcher).toHaveBeenCalledWith("/api/v1/daemon/lifecycle", {
      headers: { accept: "application/json" },
    });
  });

  it("sends bodyless actions to the closed configured-daemon routes", async () => {
    const fetcher = vi.fn(async () => Response.json(running));

    await expect(requestDaemonLifecycle("restart", fetcher)).resolves.toEqual(running);
    expect(fetcher).toHaveBeenCalledWith("/api/v1/daemon/restart", {
      method: "POST",
      headers: { accept: "application/json" },
    });
  });

  it("fails closed without exposing transport details", async () => {
    const lifecycle = await requestDaemonLifecycle("start", async () => {
      throw new Error("token=secret /private/config.toml");
    });

    expect(lifecycle.state).toBe("unavailable");
    expect(lifecycle.failure?.code).toBe("operation_failed");
    expect(JSON.stringify(lifecycle)).not.toContain("secret");
    expect(JSON.stringify(lifecycle)).not.toContain("/private");
  });
});
