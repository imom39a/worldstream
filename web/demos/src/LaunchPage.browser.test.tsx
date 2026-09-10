// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  readLaunch: vi.fn(),
  launchMutation: vi.fn(),
  seatMutation: vi.fn(),
  enterRun: vi.fn(),
}));

vi.mock("./hostedApi", () => ({
  readLaunch: mocks.readLaunch,
  launchMutation: mocks.launchMutation,
  seatMutation: mocks.seatMutation,
  enterRun: mocks.enterRun,
  githubSignIn: vi.fn(),
  usePlatformSession: () => ({
    session: { state: "authenticated", csrf: "csrf" },
  }),
}));
vi.mock("./siteChrome", () => ({
  SiteHeader: () => <header>Platform header</header>,
  SiteFooter: () => <footer>Platform footer</footer>,
}));

import { LaunchPage } from "./LaunchPage";

const launch = {
  version: "hosted_launch.v1", launch_id: "10000000-0000-4000-8000-000000000001",
  activity_slug: "agent-heist", activity_title: "Agent Heist", state: "abandoned_prestart",
  expires_at: "2026-09-08T00:00:00.000Z", can_manage: true, fill_mode: "house_agents",
  recovery_state: "not_started", available_actions: [], house_fill: null, seats: [], run: null,
} as const;

afterEach(() => {
  vi.useRealTimers();
  document.body.replaceChildren();
  mocks.readLaunch.mockReset();
  mocks.launchMutation.mockReset();
  mocks.seatMutation.mockReset();
  mocks.enterRun.mockReset();
});

it("renders an abandoned pre-start Room as terminal without start, abandon, or polling", async () => {
  vi.useFakeTimers();
  mocks.readLaunch.mockResolvedValue(launch);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<LaunchPage launchId={launch.launch_id} onNavigate={vi.fn()} />);
    await Promise.resolve();
  });
  expect(container.textContent).toContain("This room did not start");
  expect(container.textContent).toContain("ended before Genesis");
  expect(container.textContent).not.toContain("Start activity");
  expect(container.textContent).not.toContain("Abandon room");
  await act(async () => {
    await vi.advanceTimersByTimeAsync(6_000);
  });
  expect(mocks.readLaunch).toHaveBeenCalledTimes(1);
  await act(async () => root.unmount());
});

it("shows safe House-fill failure guidance without reflecting the server code", async () => {
  mocks.readLaunch.mockResolvedValue({
    ...launch,
    state: "failed_pre_genesis",
    house_fill: {
      state: "failed_pre_genesis",
      claim_window_closes_at: "2026-09-08T00:00:00.000Z",
      failure_code: "house_runner_capacity_exhausted",
    },
  });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<LaunchPage launchId={launch.launch_id} onNavigate={vi.fn()} />);
    await Promise.resolve();
  });
  expect(container.textContent).toContain("House Agents are busy");
  expect(container.textContent).not.toContain("house_runner_capacity_exhausted");
  await act(async () => root.unmount());
});

it("keeps waiting-room controls stable and shows the House-fill countdown while starting", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-09-10T12:00:00.000Z"));
  const collecting = {
    ...launch,
    state: "collecting",
    available_actions: ["start", "cancel_setup"],
    expires_at: "2026-09-10T12:30:00.000Z",
    retry_after_seconds: 30,
    house_fill: {
      state: "claim_window_open",
      claim_window_closes_at: "2026-09-10T12:00:30.000Z",
      failure_code: null,
    },
    seats: [{
      seat_key: "navigator", label: "Navigator", required: true,
      status: "claimed", participation: "external_agent", house_display_name: null,
    }],
  } as const;
  mocks.readLaunch.mockResolvedValue(collecting);
  mocks.launchMutation.mockResolvedValue(collecting);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<LaunchPage launchId={launch.launch_id} onNavigate={vi.fn()} />);
    await Promise.resolve();
  });

  const button = (label: string) => [...container.querySelectorAll("button")]
    .find((candidate) => candidate.textContent === label) as HTMLButtonElement | undefined;
  await act(async () => button("Start activity")?.click());

  expect(container.textContent).toContain("House Agents join in 30 seconds");
  expect(button("Starting safely…")?.disabled).toBe(true);
  expect(button("Release seat")?.disabled).toBe(true);
  expect(button("Cancel setup")?.disabled).toBe(true);

  await act(async () => {
    await vi.advanceTimersByTimeAsync(1_000);
  });
  expect(container.textContent).toContain("House Agents join in 29 seconds");
  expect(button("Release seat")?.disabled).toBe(true);
  expect(button("Cancel setup")?.disabled).toBe(true);
  await act(async () => root.unmount());
});

it("re-reads a timed-out close as the same recoverable closing operation", async () => {
  const provisioning = {
    ...launch,
    state: "provisioning",
    recovery_state: "genesis_not_proven",
    available_actions: ["start", "stop_setup"],
  } as const;
  const closing = {
    ...provisioning,
    state: "closing",
    recovery_state: "closing",
    available_actions: ["finish_closing"],
  } as const;
  mocks.readLaunch
    .mockResolvedValueOnce(provisioning)
    .mockResolvedValueOnce(closing);
  mocks.launchMutation.mockRejectedValue(new Error("temporarily_unavailable"));
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<LaunchPage launchId={launch.launch_id} onNavigate={vi.fn()} />);
    await Promise.resolve();
  });

  const stop = [...container.querySelectorAll("button")]
    .find((candidate) => candidate.textContent === "Stop setup") as HTMLButtonElement;
  await act(async () => {
    stop.click();
    await Promise.resolve();
    await Promise.resolve();
  });

  expect(mocks.launchMutation).toHaveBeenCalledWith("csrf", launch.launch_id, "close");
  expect(mocks.readLaunch).toHaveBeenCalledTimes(2);
  expect(container.textContent).toContain("Finish closing");
  expect(container.textContent).toContain("Retrying Finish closing is safe");
  await act(async () => root.unmount());
});
