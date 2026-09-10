// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ readMyGames: vi.fn() }));

vi.mock("./hostedApi", () => ({
  usePlatformSession: () => ({
    session: { state: "authenticated", csrf: "csrf" },
    developmentSignInAvailable: false,
    reload: async () => undefined,
  }),
  readMyGames: mocks.readMyGames,
  githubSignIn: vi.fn(),
}));

import { MyGamesPage } from "./MyGamesPage";

afterEach(() => {
  document.body.replaceChildren();
  mocks.readMyGames.mockReset();
  vi.useRealTimers();
});

it("keeps saved history visible with a delayed status and never overlaps slow refreshes", async () => {
  vi.useFakeTimers();
  const saved = {
    version: "platform_my_games.v1", refresh_delayed: true,
    items: [{ launch_id: "10000000-0000-4000-8000-000000000001", title: "Saved activity", state: "live", participation: "human", action: "return_to_game" }], next: null,
  };
  let finish!: (value: unknown) => void;
  mocks.readMyGames.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }))
    .mockRejectedValueOnce(new Error("dependency unavailable"))
    .mockResolvedValue({ ...saved, refresh_delayed: false });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => root.render(<MyGamesPage onNavigate={vi.fn()} />));
  await act(async () => { await vi.advanceTimersByTimeAsync(20_000); });
  expect(mocks.readMyGames).toHaveBeenCalledTimes(1);
  await act(async () => { finish(saved); });
  expect(container.textContent).toContain("Latest status is delayed");
  expect(container.textContent).toContain("Saved activity");
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(container.textContent).toContain("Saved activity");
  expect(container.textContent).toContain("Latest status is delayed");
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(container.textContent).not.toContain("Latest status is delayed");
  await act(async () => root.unmount());
  await act(async () => { await vi.advanceTimersByTimeAsync(20_000); });
  expect(mocks.readMyGames).toHaveBeenCalledTimes(3);
});

it("moves a mounted personal-history page from publication pending to a verified result", async () => {
  vi.useFakeTimers();
  const launchId = "10000000-0000-4000-8000-000000000004";
  const publicId = "b".repeat(32);
  const unrelatedLaunchId = "10000000-0000-4000-8000-000000000005";
  const unrelatedPublicId = "c".repeat(32);
  mocks.readMyGames
    .mockResolvedValueOnce({
      version: "platform_my_games.v1",
      items: [
        { launch_id: launchId, title: "Schema-neutral result", state: "publication_pending", updated_at: "2026-09-08T00:00:00Z", participation: "human", action: "none" },
        { launch_id: unrelatedLaunchId, title: "Older result", state: "publication_pending", updated_at: "2026-09-08T00:00:00Z", participation: "human", action: "none" },
      ],
      next: null,
    })
    .mockResolvedValueOnce({
      version: "platform_my_games.v1",
      items: [
        { launch_id: unrelatedLaunchId, title: "Older result", state: "verified_result", updated_at: "2026-09-08T00:00:01Z", participation: "human", action: "view_result", result_public_id: unrelatedPublicId },
        { launch_id: launchId, title: "Schema-neutral result", state: "verified_result", updated_at: "2026-09-08T00:00:01Z", participation: "human", action: "view_result", result_public_id: publicId },
      ],
      next: null,
    });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const navigate = vi.fn();
  await act(async () => {
    root.render(<MyGamesPage onNavigate={navigate} />);
    await Promise.resolve();
  });
  expect(container.textContent).toContain("WorldStream is verifying its reviewed result");
  expect(container.textContent).not.toContain("View result");
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5_000);
  });
  expect(container.textContent).toContain("Replay-verified result is ready to view");
  const resultCard = container.querySelector(
    `article[data-launch-id="${launchId}"][data-result-public-id="${publicId}"]`,
  );
  expect(resultCard).toBeTruthy();
  const result = resultCard?.querySelector("button") as HTMLButtonElement;
  expect(result?.textContent).toBe("View result");
  await act(async () => result.click());
  expect(navigate).toHaveBeenCalledWith(`/runs/${publicId}`);
  await act(async () => root.unmount());
});

it("renders authenticated My games actions and navigates only to reviewed platform destinations", async () => {
  mocks.readMyGames.mockResolvedValue({
    version: "platform_my_games.v1",
    items: [
      { launch_id: "10000000-0000-4000-8000-000000000001", title: "Setup", state: "setup_pending", updated_at: "2026-09-08T00:00:00Z", participation: "human", action: "continue_setup" },
      { launch_id: "10000000-0000-4000-8000-000000000004", title: "Closing", state: "activity_closing", updated_at: "2026-09-08T00:00:00Z", participation: "human", action: "finish_closing" },
      { launch_id: "10000000-0000-4000-8000-000000000002", title: "Live", state: "live", updated_at: "2026-09-08T00:00:00Z", participation: "external_agent", action: "return_to_game" },
      { launch_id: "10000000-0000-4000-8000-000000000003", title: "Result", state: "verified_result", updated_at: "2026-09-08T00:00:00Z", participation: "human", action: "view_result", result_public_id: "a".repeat(32) },
    ],
    next: null,
  });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const navigate = vi.fn();
  await act(async () => {
    root.render(<MyGamesPage onNavigate={navigate} />);
    await Promise.resolve();
  });
  const button = (label: string) => Array.from(container.querySelectorAll("button"))
    .find((element) => element.textContent === label) as HTMLButtonElement;
  expect(button("Continue setup")).toBeTruthy();
  expect(button("Finish closing")).toBeTruthy();
  expect(button("Return to game")).toBeTruthy();
  expect(button("View result")).toBeTruthy();
  await act(async () => {
    button("Continue setup").click();
    button("Finish closing").click();
    button("Return to game").click();
    button("View result").click();
  });
  expect(navigate.mock.calls).toEqual([
    ["/launches/10000000-0000-4000-8000-000000000001"],
    ["/launches/10000000-0000-4000-8000-000000000004"],
    ["/launches/10000000-0000-4000-8000-000000000002"],
    [`/runs/${"a".repeat(32)}`],
  ]);
  expect(container.textContent).not.toMatch(/entry_selector|membership_id|principal_id/iu);
  await act(async () => root.unmount());
});
