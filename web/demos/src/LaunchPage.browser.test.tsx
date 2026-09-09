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
  recovery_state: "not_started", house_fill: null, seats: [], run: null,
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
