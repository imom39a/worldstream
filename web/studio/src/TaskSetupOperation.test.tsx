import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { TaskSetupOperation } from "./TaskSetupOperation";
import type { TaskSetupStatus } from "./taskSetup";

const attention: TaskSetupStatus = {
  version: "studio_task_setup.v1", draft_id: "setup-alpha",
  operation_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV", room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
  state: "needs_attention", attempts: 2, completed_stages: 1, total_stages: 2,
  active_stage: { kind: "runner_capability", seat_id: "analyst-1" },
  attention: { code: "setup_credential_unavailable", message: "Repair credentials and retry.", retryable: true },
  seats: [{ seat_id: "analyst-1", role: "analyst", required: true, display_name: "Analyst",
    principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX", principal_kind: "agent", agent_assignment: "managed",
    member_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY", member_authority: "provisioned", runner_authority: "pending" }],
};

describe("TaskSetupOperation", () => {
  it("shows failed stage and offers only the original safe retry", () => {
    const html = renderToStaticMarkup(<TaskSetupOperation setup={attention} statusAvailable roomCreated loading={false} onRetry={vi.fn()} />);
    expect(html).toContain("Setup needs attention");
    expect(html).toContain("Failed stage: Runner control");
    expect(html).toContain("Retry original setup");
    expect(html).not.toContain("secret_reference");
  });

  it("does not present stale Ready after refresh becomes unavailable", () => {
    const html = renderToStaticMarkup(<TaskSetupOperation setup={null} statusAvailable={false} roomCreated loading={false} />);
    expect(html).toContain("Setup status unavailable");
    expect(html).not.toContain("Setup ready");
  });

  it("labels each provisioned authority without overclaiming seat readiness", () => {
    const ready: TaskSetupStatus = {
      ...attention,
      state: "ready",
      completed_stages: 2,
      active_stage: null,
      attention: null,
      seats: [{
        ...attention.seats[0],
        member_authority: "provisioned",
        runner_authority: "provisioned",
      }],
    };
    const agent = renderToStaticMarkup(
      <TaskSetupOperation setup={ready} statusAvailable roomCreated loading={false} />,
    );
    expect(agent).toContain("Participant authority provisioned");
    expect(agent).toContain("Runner authority provisioned");
    expect(agent).not.toContain("agent · ready");

    const participant = renderToStaticMarkup(
      <TaskSetupOperation
        setup={{
          ...ready,
          completed_stages: 1,
          total_stages: 1,
          seats: [{
            ...ready.seats[0], principal_kind: "human", agent_assignment: null,
            runner_authority: "not_applicable",
          }],
        }}
        statusAvailable
        roomCreated
        loading={false}
      />,
    );
    expect(participant).toContain("Participant authority provisioned");
    expect(participant).not.toContain("Runner authority provisioned");
  });
});
