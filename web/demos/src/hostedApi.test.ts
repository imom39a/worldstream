import { afterEach, expect, it, vi } from "vitest";

import { launchMutation } from "./hostedApi";

afterEach(() => vi.unstubAllGlobals());

it("rejects a versioned successful Start response without the hosted launch contract", async () => {
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response('{"version":"hosted_launch.v1"}', {
    status: 202,
    headers: { "content-type": "application/json" },
  })));

  await expect(launchMutation("csrf", "launch-id", "start"))
    .rejects.toThrow("request_unavailable");
});

it("accepts the narrow completed-close acknowledgement", async () => {
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json({ closed: true })));

  await expect(launchMutation("csrf", "launch-id", "close"))
    .resolves.toEqual({ closed: true });
});

it("accepts a complete hosted launch response", async () => {
  const launch = {
    version: "hosted_launch.v1",
    launch_id: "10000000-0000-4000-8000-000000000001",
    activity_slug: "midnight-archive",
    activity_title: "Midnight Archive",
    state: "reconciling",
    expires_at: "2099-01-01T00:00:00.000Z",
    can_manage: true,
    fill_mode: "people_only",
    recovery_state: "waiting_for_readiness",
    available_actions: ["start", "stop_setup"],
    house_fill: null,
    seats: [{
      seat_key: "lead", label: "Lead", required: true, status: "yours", participation: "human",
    }],
    run: {
      run_id: "20000000-0000-4000-8000-000000000002",
      public_id: null,
      can_enter: true,
      entries: [{ label: "Lead", entry_selector: "opaque-entry" }],
    },
    retry_after_seconds: 2,
  };
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json(launch, { status: 202 })));

  await expect(launchMutation("csrf", launch.launch_id, "start")).resolves.toEqual(launch);
});
