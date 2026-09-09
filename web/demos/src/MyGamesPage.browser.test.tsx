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

it("moves a mounted personal-history page from publication pending to a verified result", async () => {
  vi.useFakeTimers();
  const launchId = "10000000-0000-4000-8000-000000000004";
  const publicId = "b".repeat(32);
  mocks.readMyGames
    .mockResolvedValueOnce({
      version: "platform_my_games.v1",
      items: [{ launch_id: launchId, title: "Schema-neutral result", state: "publication_pending", updated_at: "2026-09-08T00:00:00Z", participation: "human", action: "none" }],
      next: null,
    })
    .mockResolvedValueOnce({
      version: "platform_my_games.v1",
      items: [{ launch_id: launchId, title: "Schema-neutral result", state: "verified_result", updated_at: "2026-09-08T00:00:01Z", participation: "human", action: "view_result", result_public_id: publicId }],
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
  const result = Array.from(container.querySelectorAll("button"))
    .find((element) => element.textContent === "View result") as HTMLButtonElement;
  await act(async () => result.click());
  expect(navigate).toHaveBeenCalledWith(`/runs/${publicId}`);
  await act(async () => root.unmount());
});

it("renders authenticated My games actions and navigates only to reviewed platform destinations", async () => {
  mocks.readMyGames.mockResolvedValue({
    version: "platform_my_games.v1",
    items: [
      { launch_id: "10000000-0000-4000-8000-000000000001", title: "Setup", state: "setup_pending", updated_at: "2026-09-08T00:00:00Z", participation: "human", action: "continue_setup" },
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
  expect(button("Return to game")).toBeTruthy();
  expect(button("View result")).toBeTruthy();
  await act(async () => {
    button("Continue setup").click();
    button("Return to game").click();
    button("View result").click();
  });
  expect(navigate.mock.calls).toEqual([
    ["/launches/10000000-0000-4000-8000-000000000001"],
    ["/launches/10000000-0000-4000-8000-000000000002"],
    [`/runs/${"a".repeat(32)}`],
  ]);
  expect(container.textContent).not.toMatch(/entry_selector|membership_id|principal_id/iu);
  await act(async () => root.unmount());
});
