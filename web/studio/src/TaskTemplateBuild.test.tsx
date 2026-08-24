import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { TaskTemplateBuild } from "./TaskTemplateBuild";
import type { TaskTemplateCatalog } from "./taskTemplates";

const catalog: TaskTemplateCatalog = {
  schema: "worldstream/studio-task-template-catalog/v1",
  revisions: [{
    revision: {
      schema: "worldstream/studio-task-template/v1",
      template_id: "counter-team",
      revision: "r1",
      display_name: "Counter team",
      source_draft_id: "source-draft",
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
    },
    dependencies: { status: "missing", issues: [{
      path: "/seats/0/runner_template",
      code: "runner_template_missing",
      message: "The exact Runner Template revision is unavailable.",
    }] },
    used_by_draft_ids: ["task-from-template"],
  }],
};

describe("Task Template Build", () => {
  it("shows exact pins, dependency blockers, and usage before successor publication", () => {
    const html = renderToStaticMarkup(
      <TaskTemplateBuild catalog={catalog} reviewedDraft={null} />,
    );
    expect(html).toContain("Task Templates");
    expect(html).toContain("counter 2.0.0");
    expect(html).toContain("careful-counter · profile-r2");
    expect(html).toContain("local-runner · runner-r3");
    expect(html).toContain("The exact Runner Template revision is unavailable.");
    expect(html).toContain("task-from-template");
    expect(html).toContain("disabled");
  });

  it("fails closed when the protected catalog is unavailable", () => {
    const html = renderToStaticMarkup(
      <TaskTemplateBuild catalog={null} reviewedDraft={null} />,
    );
    expect(html).toContain("Task Template catalog unavailable");
    expect(html).not.toContain("Publish immutable revision</button>");
  });
});
