import { describe, expect, it, vi } from "vitest";

import {
  isRunnerAttentionOperations,
  isTaskAgentAttention,
  loadTaskAgentAttention,
  newRunnerRestartOperationId,
  requestManagedHostSeatAction,
  requestRunnerRestart,
} from "./runnerAttention";

const ROOM = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const RUNNER = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const OPERATION = "01ARZ3NDEKTSV4RRFFQ69G5FB1";

describe("runner attention browser boundary", () => {
  it("accepts coherent live operations and rejects nested private or unknown fields", () => {
    const value = operations();
    expect(isRunnerAttentionOperations(value)).toBe(true);
    expect(isRunnerAttentionOperations({ ...value, command: "ps aux" })).toBe(false);
    expect(isRunnerAttentionOperations({
      ...value,
      runners: [{ ...value.runners[0], nested: { invocation_context: { prompt: "private" } } }],
    })).toBe(false);
  });

  it("rejects malformed capacity freshness and Activation coherence", () => {
    const value = task();
    expect(isTaskAgentAttention(value)).toBe(true);
    expect(isTaskAgentAttention({ ...value, observed_at_unix_ms: null })).toBe(false);
    expect(isTaskAgentAttention({
      ...value,
      seats: [{ ...value.seats[0], capacity: { advertised: 2, in_use: 2, available: 1 } }],
    })).toBe(false);
    expect(isTaskAgentAttention({
      ...value,
      seats: [{ ...value.seats[0], activation: { state: "leased", waiting: 0, leased: 0 } }],
    })).toBe(false);
  });

  it("binds Task and restart responses to the requested exact identity", async () => {
    const taskFetcher = vi.fn(async () => new Response(JSON.stringify(task()), { status: 200 }));
    expect((await loadTaskAgentAttention(ROOM, taskFetcher))?.seats[0]?.seat_id).toBe("navigator-agent");
    expect(taskFetcher).toHaveBeenCalledWith(
      `/api/v1/rooms/${ROOM}/agent-attention`,
      expect.objectContaining({ cache: "no-store" }),
    );

    const wrong = { ...restart(), operation_id: "01ARZ3NDEKTSV4RRFFQ69G5FB2" };
    const wrongFetcher = vi.fn(async () => new Response(JSON.stringify(wrong), { status: 200 }));
    expect(await requestRunnerRestart("managed-runner-01", OPERATION, wrongFetcher)).toBeNull();

    let restartBody: BodyInit | null | undefined;
    const fetcher = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      restartBody = init?.body;
      return new Response(JSON.stringify(restart()), { status: 200 });
    });
    expect((await requestRunnerRestart("managed-runner-01", OPERATION, fetcher))?.state).toBe("reconciling");
    expect(JSON.parse(String(restartBody))).toEqual({ operation_id: OPERATION });
  });

  it("creates a canonical ULID without accepting caller scope", () => {
    expect(newRunnerRestartOperationId(1_700_000_000_000, new Uint8Array(10))).toMatch(
      /^[0-9A-HJKMNP-TV-Z]{26}$/,
    );
    expect(() => newRunnerRestartOperationId(-1, new Uint8Array(10))).toThrow();
  });

  it("uses the strict CSRF-resistant managed-host action DTO", async () => {
    const fetcher = vi.fn(async () => new Response("{}", { status: 202 }));
    await expect(requestManagedHostSeatAction(ROOM, "navigator-agent", "start", fetcher)).resolves.toBe(true);
    expect(fetcher).toHaveBeenCalledWith(
      `/api/v1/rooms/${ROOM}/agent-seats/navigator-agent/managed-host/start`,
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({ "Content-Type": "application/json" }),
        body: JSON.stringify({ schema: "worldstream/studio-managed-agent-host-action/v1" }),
      }),
    );
  });
});

function operations() {
  return {
    schema: "worldstream/studio-runner-attention/v1",
    freshness: "live",
    observed_at_unix_ms: 1_000,
    runners: [{
      runner_id: RUNNER,
      instance_id: "managed-runner-01",
      connection: "connected",
      freshness: "live",
      capacity: { advertised: 2, in_use: 1, available: 1 },
      compatible_assignments: 1,
      incompatible_assignments: 0,
      observed_at_unix_ms: 1_000,
      next_action: "No operator action is required.",
    }],
    managed_hosts: [{
      schema: "worldstream/managed-agent-host-status/v1",
      assignment_id: "01ARZ3NDEKTSV4RRFFQ69G5FB2",
      host_id: "reference-agent-host",
      host_revision: "r1",
      state: "running",
      ready: true,
      capacity: 1,
      active_invocations: 1,
      freshness: "fresh",
    }],
    restart_attempts: [restart()],
  };
}

function task() {
  return {
    schema: "worldstream/studio-task-agent-attention/v1",
    freshness: "live",
    observed_at_unix_ms: 1_000,
    seats: [{
      seat_id: "navigator-agent",
      instance_id: "managed-runner-01",
      compatibility: "compatible",
      capacity: { advertised: 2, in_use: 1, available: 1 },
      activation: { state: "leased", waiting: 0, leased: 1 },
      freshness: "live",
      next_action: "Wait for the current lease to complete.",
    }],
  };
}

function restart() {
  return {
    schema: "worldstream/studio-runner-restart-operation/v1",
    operation_id: OPERATION,
    instance_id: "managed-runner-01",
    attempts: 1,
    state: "reconciling",
    explanation: "Restart was issued; authoritative state is being reconciled.",
    next_action: "Wait for reconciliation.",
  };
}
