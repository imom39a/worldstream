import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { RunnerOperations } from "./RunnerOperations";
import type {
  RunnerInstanceStatusResponse,
  RunnerTemplateCatalog,
} from "./runnerTemplates";

const catalog: RunnerTemplateCatalog = {
  schema: "worldstream/studio-runner-template-catalog/v1",
  templates: [{
    template_id: "local-mcp-helper",
    revision: "r2",
    display_name: "Local MCP Helper",
    executable_blake3: "b".repeat(64),
    compatibility: [{ activity_pack_id: "agent-heist", exact_revisions: ["1.0", "1.1"] }],
    capacity: { maximum_concurrent_invocations: 4 },
    health_stale_after_ms: 5_000,
    non_secret_settings: ["LOG_LEVEL"],
    secret_settings: [{ key: "MODEL_TOKEN", kind: "model_provider", configured: true }],
    instances: ["local-mcp-helper-1"],
  }],
};

const running: RunnerInstanceStatusResponse = {
  schema: "worldstream/studio-runner-instance-status/v1",
  instances: [{
    instance_id: "local-mcp-helper-1",
    template_id: "local-mcp-helper",
    template_revision: "r2",
    state: "running",
    operation_id: 4,
    managed_by_supervisor: true,
    compatibility: catalog.templates[0].compatibility,
    capacity: { maximum: 4, in_use: 1, available: 3 },
    health: "healthy",
    freshness: "fresh",
    observed_at_unix_ms: 1_800_000_000_000,
    failure: null,
  }],
};

describe("Runner Templates and processes", () => {
  it("shows immutable template identity, compatibility, capacity, health, and bounded controls", () => {
    const dom = renderToStaticMarkup(
      <RunnerOperations catalog={catalog} instances={running} />,
    );

    expect(dom).toContain("Installed Runner Templates");
    expect(dom).toContain("Local MCP Helper");
    expect(dom).toContain("local-mcp-helper · r2");
    expect(dom).toContain("agent-heist 1.0, 1.1");
    expect(dom).toContain("1 in use · 3 available · 4 maximum");
    expect(dom).toContain("Healthy · fresh");
    expect(dom).toContain("Stop instance");
    expect(dom).toContain("Restart instance");
    expect(dom).not.toContain("Executable path");
    expect(dom).not.toContain("Upload");
    expect(dom).not.toContain("Command");
  });

  it("shows an actionable startup failure without exposing credential material", () => {
    const failed: RunnerInstanceStatusResponse = {
      ...running,
      instances: [{
        ...running.instances[0],
        state: "failed",
        managed_by_supervisor: false,
        health: "unavailable",
        freshness: "stale",
        failure: {
          code: "health_unavailable",
          explanation: "The Runner did not satisfy its installed health contract.",
          next_action: "Check the installed Runner service and retry start.",
        },
      }],
    };
    const dom = renderToStaticMarkup(
      <RunnerOperations catalog={catalog} instances={failed} />,
    );

    expect(dom).toContain("Runner needs attention");
    expect(dom).toContain("did not satisfy its installed health contract");
    expect(dom).toContain("Check the installed Runner service and retry start.");
    expect(dom).toContain("Retry start");
    expect(dom).not.toContain("MODEL_TOKEN");
  });

  it("fails closed when Supervisor Runner state is unavailable", () => {
    const dom = renderToStaticMarkup(
      <RunnerOperations catalog={null} instances={null} />,
    );

    expect(dom).toContain("Runner Template catalog unavailable");
    expect(dom).toContain("Runner instance status unavailable");
    expect(dom).not.toContain("Start instance");
  });
});
