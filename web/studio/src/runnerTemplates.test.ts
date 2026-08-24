import { describe, expect, it, vi } from "vitest";

import {
  loadRunnerInstances,
  loadRunnerTemplates,
  requestRunnerInstanceLifecycle,
  type RunnerInstanceStatusResponse,
  type RunnerTemplateCatalog,
} from "./runnerTemplates";

const digest = "a".repeat(64);

const catalog: RunnerTemplateCatalog = {
  schema: "worldstream/studio-runner-template-catalog/v1",
  templates: [{
    template_id: "local-mcp-helper",
    revision: "r2",
    display_name: "Local MCP Helper",
    executable_blake3: digest,
    compatibility: [{ activity_pack_id: "agent-heist", exact_revisions: ["1.0", "1.1"] }],
    capacity: { maximum_concurrent_invocations: 4 },
    health_stale_after_ms: 5_000,
    non_secret_settings: ["LOG_LEVEL"],
    secret_settings: [{ key: "MODEL_TOKEN", kind: "model_provider", configured: true }],
    instances: ["local-mcp-helper-1"],
  }],
};

const instances: RunnerInstanceStatusResponse = {
  schema: "worldstream/studio-runner-instance-status/v1",
  instances: [{
    instance_id: "local-mcp-helper-1",
    template_id: "local-mcp-helper",
    template_revision: "r2",
    state: "running",
    operation_id: 3,
    managed_by_supervisor: true,
    compatibility: catalog.templates[0].compatibility,
    capacity: { maximum: 4, in_use: 1, available: 3 },
    health: "healthy",
    freshness: "fresh",
    observed_at_unix_ms: 1_800_000_000_000,
    failure: null,
  }],
};

describe("Runner Template Supervisor client", () => {
  it("loads immutable installed revisions and complete bounded instance status", async () => {
    const fetchCatalog = vi.fn(async () => new Response(JSON.stringify(catalog), { status: 200 }));
    const fetchInstances = vi.fn(async () => new Response(JSON.stringify(instances), { status: 200 }));

    await expect(loadRunnerTemplates(fetchCatalog)).resolves.toEqual(catalog);
    await expect(loadRunnerInstances(fetchInstances)).resolves.toEqual(instances);
    expect(fetchCatalog).toHaveBeenCalledWith("/api/v1/runner-templates", {
      headers: { accept: "application/json" },
    });
    expect(fetchInstances).toHaveBeenCalledWith("/api/v1/runner-instances", {
      headers: { accept: "application/json" },
    });
  });

  it("requests only a closed lifecycle action for an installed stable instance id", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify(instances), { status: 200 }));

    await expect(
      requestRunnerInstanceLifecycle("local-mcp-helper-1", "restart", fetcher),
    ).resolves.toEqual(instances);
    expect(fetcher).toHaveBeenCalledWith(
      "/api/v1/runner-instances/local-mcp-helper-1/restart",
      { method: "POST", headers: { accept: "application/json" } },
    );
  });

  it("fails closed before a request can carry a path, command, or malformed response", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify({ ...instances, command: "sh" }), { status: 200 }));

    await expect(
      requestRunnerInstanceLifecycle("../../bin/sh", "start", fetcher),
    ).resolves.toBeNull();
    await expect(loadRunnerInstances(fetcher)).resolves.toBeNull();
    expect(fetcher).toHaveBeenCalledTimes(1);
  });
});
