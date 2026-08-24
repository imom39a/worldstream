import { describe, expect, it, vi } from "vitest";
import { isTaskSetupStatus, loadTaskSetup, requestTaskSetup } from "./taskSetup";

const ready = {
  version: "studio_task_setup.v1",
  draft_id: "setup-alpha",
  operation_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
  room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
  state: "ready",
  attempts: 2,
  completed_stages: 2,
  total_stages: 2,
  active_stage: null,
  attention: null,
  seats: [{
    seat_id: "analyst-1", role: "analyst", required: true, display_name: "Analyst",
    principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX", principal_kind: "agent",
    agent_assignment: "external", member_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY",
    agent_profile: { profile_id: "analyst", revision: "rev-1" }, managed_runner: null,
    member_authority: "provisioned", runner_authority: "provisioned",
  }],
  readiness: { ready_to_launch: true, seats: [{ seat_id: "analyst-1", required: true, ready: true, reason: "ready" }] },
  launch: null,
};

describe("Task setup client", () => {
  it("accepts only the bounded secret-free status", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(ready), { status: 200 }));
    await expect(loadTaskSetup("setup-alpha", fetcher)).resolves.toEqual({
      availability: "available", setup: ready,
    });
    expect(fetcher).toHaveBeenCalledWith("/api/v1/task-setups/setup-alpha", expect.anything());
  });

  it("fails closed when a response attempts to expose a retained reference", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      ...ready, secret_reference: "a".repeat(64),
    }), { status: 200 }));
    await expect(loadTaskSetup("setup-alpha", fetcher)).resolves.toEqual({
      availability: "unavailable", setup: null,
    });
  });

  it("rejects credential-bearing fields recursively and unknown nested keys", () => {
    expect(isTaskSetupStatus({
      ...ready,
      seats: [{ ...ready.seats[0], diagnostic: { nested: { bearer: "private" } } }],
    })).toBe(false);
    expect(isTaskSetupStatus({
      ...ready,
      attention: {
        code: "blocked",
        message: "Repair and retry.",
        retryable: true,
        detail: { secret_ref: "private" },
      },
    })).toBe(false);
    expect(isTaskSetupStatus({ ...ready, internal_request: { token_hash: "private" } })).toBe(false);
  });

  it("requires exact keys at the status, seat, stage, and attention levels", () => {
    expect(isTaskSetupStatus({ ...ready, extra: true })).toBe(false);
    expect(isTaskSetupStatus({
      ...ready,
      seats: [{ ...ready.seats[0], extra: true }],
    })).toBe(false);
    expect(isTaskSetupStatus({
      ...ready,
      state: "provisioning",
      completed_stages: 1,
      active_stage: { kind: "runner_capability", seat_id: "analyst-1", extra: true },
      seats: [{ ...ready.seats[0], runner_authority: "pending" }],
    })).toBe(false);
    expect(isTaskSetupStatus({
      ...ready,
      state: "needs_attention",
      completed_stages: 1,
      active_stage: { kind: "runner_capability", seat_id: "analyst-1" },
      attention: { code: "blocked", message: "Repair and retry.", retryable: true, extra: true },
      seats: [{ ...ready.seats[0], runner_authority: "pending" }],
    })).toBe(false);
  });

  it("rejects malformed identifiers, counts, seat authority, and lifecycle coherence", () => {
    const cases = [
      { ...ready, operation_id: "not-a-ulid" },
      { ...ready, completed_stages: 1 },
      { ...ready, total_stages: 3 },
      { ...ready, attempts: -1 },
      { ...ready, state: "ready", active_stage: { kind: "runner_capability", seat_id: "analyst-1" } },
      { ...ready, state: "needs_attention", attention: null },
      { ...ready, seats: [ready.seats[0], ready.seats[0]] },
      {
        ...ready,
        seats: [{
          ...ready.seats[0], required: true, principal_id: null, principal_kind: null,
          agent_assignment: null, member_id: null, member_authority: "unfilled_optional",
          agent_profile: null, managed_runner: null,
          runner_authority: "not_applicable",
        }],
        completed_stages: 0,
        total_stages: 0,
      },
      {
        ...ready,
        state: "provisioning",
        completed_stages: 1,
        active_stage: { kind: "runner_capability", seat_id: "missing-seat" },
        seats: [{ ...ready.seats[0], runner_authority: "pending" }],
      },
    ];
    for (const value of cases) expect(isTaskSetupStatus(value)).toBe(false);
  });

  it("rejects readiness projections that do not exactly describe every setup seat", () => {
    const row = ready.readiness.seats[0];
    const cases = [
      { ...ready, readiness: { ...ready.readiness, seats: [] } },
      { ...ready, readiness: { ...ready.readiness, seats: [row, row] } },
      { ...ready, readiness: { ...ready.readiness, seats: [{ ...row, seat_id: "other-seat" }] } },
      { ...ready, readiness: { ...ready.readiness, seats: [{ ...row, required: false }] } },
      { ...ready, readiness: { ...ready.readiness, seats: [{ ...row, ready: false }] } },
      { ...ready, readiness: { ...ready.readiness, seats: [{ ...row, reason: "runner_missing" }] } },
      { ...ready, readiness: { ...ready.readiness, ready_to_launch: false } },
    ];
    for (const value of cases) expect(isTaskSetupStatus(value)).toBe(false);
  });

  it("accepts only coherent optional-unfilled readiness and launch transitions", () => {
    const optional = {
      seat_id: "observer-1", role: "observer", required: false, display_name: "Observer",
      principal_id: null, principal_kind: null, agent_assignment: null, agent_profile: null,
      managed_runner: null, member_id: null, member_authority: "unfilled_optional",
      runner_authority: "not_applicable",
    };
    const withOptional = {
      ...ready,
      seats: [...ready.seats, optional],
      readiness: {
        ready_to_launch: true,
        seats: [...ready.readiness.seats, {
          seat_id: "observer-1", required: false, ready: true, reason: "optional_unfilled",
        }],
      },
    };
    expect(isTaskSetupStatus(withOptional)).toBe(true);
    expect(isTaskSetupStatus({
      ...withOptional,
      readiness: {
        ...withOptional.readiness,
        seats: [...ready.readiness.seats, {
          seat_id: "observer-1", required: false, ready: false, reason: "optional_unfilled",
        }],
      },
    })).toBe(false);

    const transitions = [
      { state: "waiting", attempts: 1, attention: null, transition_id: null },
      { state: "retrying", attempts: 0, attention: null, transition_id: null },
      { state: "reconciling", attempts: 1, attention: null, transition_id: null },
      { state: "launched", attempts: 1, attention: null, transition_id: null },
      { state: "needs_attention", attempts: 1, attention: null, transition_id: null },
    ];
    for (const launch of transitions) {
      expect(isTaskSetupStatus({ ...ready, launch })).toBe(false);
    }
    expect(isTaskSetupStatus({
      ...ready,
      launch: {
        state: "launched", attempts: 1, attention: null,
        transition_id: "01ARZ3NDEKTSV4RRFFQ69G5FB0",
      },
    })).toBe(true);
  });

  it("binds load, start, and retry responses to the requested draft", async () => {
    const wrong = { ...ready, draft_id: "setup-other" };
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(wrong), { status: 200 }));
    await expect(loadTaskSetup("setup-alpha", fetcher)).resolves.toEqual({
      availability: "unavailable", setup: null,
    });
    await expect(requestTaskSetup("setup-alpha", "start", fetcher)).resolves.toBeNull();
    await expect(requestTaskSetup("setup-alpha", "retry", fetcher)).resolves.toBeNull();
  });

  it("retries only the stable draft-keyed setup endpoint", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(ready), { status: 200 }));
    await expect(requestTaskSetup("setup-alpha", "retry", fetcher)).resolves.toEqual(ready);
    expect(fetcher).toHaveBeenCalledWith(
      "/api/v1/task-setups/setup-alpha:retry",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("launches only through the stable draft-keyed explicit endpoint", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(ready), { status: 200 }));
    await expect(requestTaskSetup("setup-alpha", "launch", fetcher)).resolves.toEqual(ready);
    expect(fetcher).toHaveBeenCalledWith(
      "/api/v1/task-setups/setup-alpha:launch",
      expect.objectContaining({ method: "POST" }),
    );
  });
});
