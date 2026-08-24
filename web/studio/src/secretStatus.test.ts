import { describe, expect, it, vi } from "vitest";

import { loadSecretStatus } from "./secretStatus";

describe("Supervisor secret configuration client", () => {
  it("loads only browser-safe configuration states", async () => {
    const payload = {
      schema: "worldstream/studio-secret-kind-status/v1",
      credentials: [
        { kind: "host_authority", availability: "configured" },
        { kind: "membership_authority", availability: "missing" },
        { kind: "runner_authority", availability: "configured" },
        { kind: "model_provider", availability: "missing" },
      ],
    };
    const fetcher = vi.fn(async () => Response.json(payload));

    await expect(loadSecretStatus(fetcher)).resolves.toEqual(payload);
    expect(fetcher).toHaveBeenCalledWith("/api/v1/secrets", {
      headers: { accept: "application/json" },
    });
  });

  it("fails closed when a response contains credential material", async () => {
    const status = await loadSecretStatus(async () =>
      Response.json({
        schema: "worldstream/studio-secret-kind-status/v1",
        credentials: [
          { kind: "host_authority", availability: "configured", secret: "bearer-value" },
          { kind: "membership_authority", availability: "missing" },
          { kind: "runner_authority", availability: "missing" },
          { kind: "model_provider", availability: "missing" },
        ],
      }),
    );

    expect(status.credentials.every((row) => row.availability === "unavailable")).toBe(true);
    expect(JSON.stringify(status)).not.toContain("bearer-value");
  });
});
