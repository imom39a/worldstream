import { describe, expect, it, vi } from "vitest";

import {
  instantiateTaskTemplate,
  loadTaskTemplates,
  publishTaskTemplate,
} from "./taskTemplates";

const draft = {
  schema: "worldstream/studio-room-draft/v1",
  draft_id: "independent-draft",
  pack: { id: "counter", version: "2.0.0", digest: `blake3:${"a".repeat(64)}` },
  configuration: { initial_value: 0 },
  seats: [{
    seat_id: "analyst-1", role: "analyst", required: true, display_name: "Analyst",
    principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV", principal_kind: "agent",
    agent_assignment: "managed",
    agent_profile: { profile_id: "careful-counter", revision: "profile-r2" },
    runner_template: { template_id: "local-runner", revision: "runner-r3" },
  }],
  readiness: [{ seat_id: "analyst-1", role: "analyst", required: true }],
  last_valid_step: "readiness",
} as const;

const view = {
  revision: {
    schema: "worldstream/studio-task-template/v1",
    template_id: "counter-team",
    revision: "r1",
    display_name: "Counter team",
    source_draft_id: "source-draft",
    pack: draft.pack,
    configuration: draft.configuration,
    seats: draft.seats,
    readiness: draft.readiness,
  },
  dependencies: { status: "ready", issues: [] },
  used_by_draft_ids: ["independent-draft"],
} as const;

describe("Task Template client", () => {
  it("loads exact immutable revisions with current dependency and usage evidence", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      schema: "worldstream/studio-task-template-catalog/v1",
      revisions: [view],
    }), { status: 200 }));

    await expect(loadTaskTemplates(fetcher)).resolves.toEqual({
      schema: "worldstream/studio-task-template-catalog/v1",
      revisions: [view],
    });
    expect(fetcher).toHaveBeenCalledWith("/api/v1/task-templates", expect.anything());
  });

  it("publishes only a source draft reference and creates only a new draft identity", async () => {
    const publishFetcher = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ ...view, used_by_draft_ids: [] }), { status: 200 }),
    );
    await expect(publishTaskTemplate({
      template_id: "counter-team",
      revision: "r1",
      display_name: "Counter team",
      source_draft_id: "source-draft",
    }, publishFetcher)).resolves.toEqual({ ...view, used_by_draft_ids: [] });
    expect(JSON.parse(String(publishFetcher.mock.calls[0]?.[1]?.body))).toEqual({
      template_id: "counter-team",
      revision: "r1",
      display_name: "Counter team",
      source_draft_id: "source-draft",
    });

    const instantiateFetcher = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(draft), { status: 200 }),
    );
    await expect(instantiateTaskTemplate(
      "counter-team", "r1", "independent-draft", instantiateFetcher,
    )).resolves.toEqual(draft);
    expect(instantiateFetcher).toHaveBeenCalledWith(
      "/api/v1/task-templates/counter-team/revisions/r1/instantiate",
      expect.objectContaining({ method: "POST", body: JSON.stringify({ draft_id: "independent-draft" }) }),
    );
  });

  it("fails closed on unknown fields, raw credential keys, and incoherent dependency status", async () => {
    const cases = [
      { ...view, secret_reference: "a".repeat(64) },
      { ...view, revision: { ...view.revision, configuration: { api_key: "private" } } },
      { ...view, revision: { ...view.revision, configuration: {
        provider: "sk-live-123456789012345678901234567890",
      } } },
      { ...view, dependencies: { status: "ready", issues: [{
        path: "/pack", code: "revision_unavailable", message: "Missing.",
      }] } },
      { ...view, dependencies: { status: "missing", issues: [] } },
      { ...view, dependencies: { status: "missing", issues: [{
        path: "/pack", code: "dependency_check_unavailable", message: "Unavailable.",
      }] } },
      { ...view, dependencies: { status: "unavailable", issues: [{
        path: "/pack", code: "runner_template_missing", message: "Missing.",
      }] } },
      { ...view, used_by_draft_ids: ["same", "same"] },
    ];
    for (const malformed of cases) {
      const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
        schema: "worldstream/studio-task-template-catalog/v1", revisions: [malformed],
      }), { status: 200 }));
      await expect(loadTaskTemplates(fetcher)).resolves.toBeNull();
    }
  });

  it("accepts coherent transient-unavailable evidence and keeps retry failure closed", async () => {
    const unavailable = {
      ...view,
      dependencies: { status: "unavailable", issues: [{
        path: "/seats/0/agent_profile",
        code: "dependency_check_unavailable",
        message: "The exact dependency could not be checked.",
      }] },
    } as const;
    const catalogFetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      schema: "worldstream/studio-task-template-catalog/v1",
      revisions: [unavailable],
    }), { status: 200 }));
    await expect(loadTaskTemplates(catalogFetcher)).resolves.toEqual({
      schema: "worldstream/studio-task-template-catalog/v1",
      revisions: [unavailable],
    });

    const retryableFetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      error: {
        code: "task_template_dependencies_unavailable",
        message: "the exact pinned dependencies block this operation",
        retryable: true,
        dependency_issues: unavailable.dependencies.issues,
      },
    }), { status: 503 }));
    await expect(instantiateTaskTemplate(
      "counter-team", "r1", "retry-draft", retryableFetcher,
    )).resolves.toBeNull();
    expect(retryableFetcher).toHaveBeenCalledTimes(1);
  });
});
