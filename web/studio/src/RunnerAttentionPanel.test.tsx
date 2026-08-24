import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { RunnerAttentionPanel } from "./RunnerAttentionPanel";
import type { RunnerAttentionOperations, TaskAgentAttention } from "./runnerAttention";

const RUNNER = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const OPERATION = "01ARZ3NDEKTSV4RRFFQ69G5FB1";

describe("Runner attention", () => {
  it("labels stale exhausted capacity and offers only an approved restart", () => {
    const onRestart = vi.fn();
    const dom = renderToStaticMarkup(
      <RunnerAttentionPanel operations={operations()} task={task()} onRestart={onRestart} />,
    );

    expect(dom).toContain("Runner attention");
    expect(dom).toContain("Stale");
    expect(dom).toContain("Last observed");
    expect(dom).toContain("2 used · 0 available · 2 advertised");
    expect(dom).toContain("Activation state");
    expect(dom).toContain("Attention");
    expect(dom).toContain("Restore compatible Runner capacity");
    expect(dom).toContain("Restart approved Runner");
    for (const forbidden of ["invocation", "prompt", "memory", "bearer", "secret", "payload", "process list", "command"]) {
      expect(dom.toLowerCase()).not.toContain(forbidden);
    }
  });

  it("shows leased delay plus typed restart progress and final failure guidance", () => {
    const delayed = task();
    delayed.seats[0].activation = { state: "delayed", waiting: 0, leased: 1 };
    delayed.seats[0].next_action = "Wait for the retained lease to expire or reconcile.";
    const value = operations();
    value.restart_attempts[0] = {
      ...value.restart_attempts[0],
      state: "failed",
      explanation: "The approved Runner restart failed.",
      next_action: "Inspect bounded Runner diagnostics, then retry restart.",
    };

    const dom = renderToStaticMarkup(<RunnerAttentionPanel operations={value} task={delayed} />);
    expect(dom).toContain("Delayed");
    expect(dom).toContain("Restart Failed");
    expect(dom).toContain("The approved Runner restart failed.");
    expect(dom).toContain("Inspect bounded Runner diagnostics");
  });

  it("renders successful reconciliation and closed unavailable states", () => {
    const value = operations();
    value.restart_attempts[0] = {
      ...value.restart_attempts[0],
      state: "succeeded",
      explanation: "Runner restart reconciled from authoritative state.",
      next_action: "No operator action is required.",
    };
    expect(renderToStaticMarkup(<RunnerAttentionPanel operations={value} task={task()} />))
      .toContain("Restart Succeeded");
    const unavailable = renderToStaticMarkup(<RunnerAttentionPanel operations={null} task={null} />);
    expect(unavailable).toContain("Runner attention is unavailable");
    expect(unavailable).toContain("Agent work status is unavailable");
  });
});

function operations(): RunnerAttentionOperations {
  return {
    schema: "worldstream/studio-runner-attention/v1",
    freshness: "stale",
    observed_at_unix_ms: 1_000,
    runners: [{
      runner_id: RUNNER,
      instance_id: "managed-runner-01",
      connection: "connected",
      freshness: "stale",
      capacity: { advertised: 2, in_use: 2, available: 0 },
      compatible_assignments: 1,
      incompatible_assignments: 0,
      observed_at_unix_ms: 1_000,
      next_action: "Restore Runner capacity or wait for active work to complete.",
    }],
    restart_attempts: [{
      schema: "worldstream/studio-runner-restart-operation/v1",
      operation_id: OPERATION,
      instance_id: "managed-runner-01",
      attempts: 1,
      state: "reconciling",
      explanation: "Restart was issued; authoritative state is being reconciled.",
      next_action: "Wait for reconciliation.",
    }],
  };
}

function task(): TaskAgentAttention {
  return {
    schema: "worldstream/studio-task-agent-attention/v1",
    freshness: "stale",
    observed_at_unix_ms: 1_000,
    seats: [{
      seat_id: "navigator-agent",
      instance_id: "managed-runner-01",
      compatibility: "compatible",
      capacity: { advertised: 2, in_use: 2, available: 0 },
      activation: { state: "attention", waiting: 3, leased: 0 },
      freshness: "stale",
      next_action: "Restore compatible Runner capacity, then retry status.",
    }],
  };
}
