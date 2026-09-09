// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ readPublicRun: vi.fn() }));

vi.mock("./hostedApi", () => ({ readPublicRun: mocks.readPublicRun }));
vi.mock("./siteChrome", () => ({
  SiteHeader: () => <header>Platform header</header>,
  SiteFooter: () => <footer>Platform footer</footer>,
}));

import { RunPage } from "./RunPage";

afterEach(() => {
  document.body.replaceChildren();
  mocks.readPublicRun.mockReset();
  vi.useRealTimers();
});

it("opens a server-selected independent client instead of rendering a Pack viewer in the platform shell", async () => {
  const publicId = "a".repeat(32);
  const launchUrl = `/negotiate-v1/hosted/?public_run=${publicId}&platform_return=%2F&platform_result=%2Fruns%2F${publicId}`;
  mocks.readPublicRun.mockResolvedValue({
    version: "public_run.v1",
    state: "live",
    public_id: publicId,
    activity: {
      title: "A non-Heist fixture",
      description: "Reviewed client contract fixture.",
      listing_key: "fixture", listing_revision: "blake3:fixture", pack: { id: "fixture", version: "1", revision: "fixture" },
    },
    started_at: "2026-09-08T00:00:00Z",
    evidence: { class: "unranked", label: "Unranked activity" },
    participants: [],
    live: { available: true, stream_url: "wss://stream.example/public" },
    client: { launch_url: launchUrl, back_to_games: "/", result_url: `/runs/${publicId}` },
  });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<RunPage publicId={publicId} onNavigate={vi.fn()} />);
    await Promise.resolve();
  });
  const link = Array.from(container.querySelectorAll("a"))
    .find((element) => element.textContent === "Watch live Run") as HTMLAnchorElement;
  expect(link).toBeTruthy();
  expect(link.getAttribute("href")).toBe(launchUrl);
  expect(container.textContent).toContain("A non-Heist fixture");
  expect(container.textContent).not.toContain("Authorized participant surface");
  await act(async () => root.unmount());
});

it("reconciles a live public Run into its verified result without a reload", async () => {
  vi.useFakeTimers();
  const publicId = "d".repeat(32);
  const live = {
    version: "public_run.v1" as const,
    state: "live" as const,
    public_id: publicId,
    activity: {
      title: "Live then done", description: "A reconciliation fixture.",
      listing_key: "fixture", listing_revision: "blake3:fixture", pack: { id: "fixture", version: "1", revision: "fixture" },
    },
    started_at: "2026-09-08T00:00:00Z",
    evidence: { class: "unranked" as const, label: "Unranked activity" },
    participants: [],
    live: { available: true as const, stream_url: "wss://stream.example/public" },
  };
  const result = {
    ...live,
    state: "result" as const,
    completed_at: "2026-09-08T00:01:00Z",
    result: { summary: { schema: "fixture/result/v1", verdict: "accepted" } },
  };
  mocks.readPublicRun.mockResolvedValueOnce(live).mockResolvedValueOnce(result);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<RunPage publicId={publicId} onNavigate={vi.fn()} />);
    await Promise.resolve();
  });
  expect(container.textContent).toContain("Open the reviewed Activity Client");
  expect(mocks.readPublicRun).toHaveBeenCalledTimes(1);

  await act(async () => {
    await vi.advanceTimersByTimeAsync(5_000);
  });
  expect(container.textContent).toContain("Activity complete");
  expect(container.textContent).toContain("Accepted");
  expect(mocks.readPublicRun).toHaveBeenCalledTimes(2);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(60_000);
  });
  expect(mocks.readPublicRun).toHaveBeenCalledTimes(2);
  await act(async () => root.unmount());
});

it("continues live reconciliation until publication, beyond the old bounded poll window", async () => {
  vi.useFakeTimers();
  const publicId = "7".repeat(32);
  const live = {
    version: "public_run.v1" as const,
    state: "live" as const,
    public_id: publicId,
    activity: {
      title: "Slow result", description: "A delayed publication fixture.",
      listing_key: "fixture", listing_revision: "blake3:fixture", pack: { id: "fixture", version: "1", revision: "fixture" },
    },
    started_at: "2026-09-08T00:00:00Z",
    evidence: { class: "unranked" as const, label: "Unranked activity" },
    participants: [],
    live: { available: true as const, stream_url: "wss://stream.example/public" },
  };
  const result = {
    ...live,
    state: "result" as const,
    completed_at: "2026-09-08T00:04:00Z",
    result: { summary: { schema: "fixture/result/v1", verdict: "accepted" } },
  };
  for (let poll = 0; poll < 13; poll += 1) mocks.readPublicRun.mockResolvedValueOnce(live);
  mocks.readPublicRun.mockResolvedValueOnce(result);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<RunPage publicId={publicId} onNavigate={vi.fn()} />);
    await Promise.resolve();
  });
  for (let poll = 0; poll < 12; poll += 1) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
  }
  expect(container.textContent).toContain("Open the reviewed Activity Client");
  expect(mocks.readPublicRun).toHaveBeenCalledTimes(13);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(30_000);
  });
  expect(container.textContent).toContain("Activity complete");
  expect(mocks.readPublicRun).toHaveBeenCalledTimes(14);
  await act(async () => root.unmount());
});

it("does not poll a terminal public Run or keep polling after unmount", async () => {
  vi.useFakeTimers();
  const publicId = "e".repeat(32);
  mocks.readPublicRun.mockResolvedValue({
    version: "public_run.v1",
    state: "unavailable",
  });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<RunPage publicId={publicId} onNavigate={vi.fn()} />);
    await Promise.resolve();
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(60_000);
  });
  expect(mocks.readPublicRun).toHaveBeenCalledTimes(1);
  await act(async () => root.unmount());
});

it("cleans the reconciliation timer when the public Run id changes", async () => {
  vi.useFakeTimers();
  const firstId = "f".repeat(32);
  const secondId = "1".repeat(32);
  const live = {
    version: "public_run.v1" as const,
    state: "live" as const,
    public_id: firstId,
    activity: {
      title: "First live Run", description: "A replacement fixture.",
      listing_key: "fixture", listing_revision: "blake3:fixture", pack: { id: "fixture", version: "1", revision: "fixture" },
    },
    started_at: "2026-09-08T00:00:00Z",
    evidence: { class: "unranked" as const, label: "Unranked activity" },
    participants: [],
    live: { available: true as const, stream_url: "wss://stream.example/public" },
  };
  mocks.readPublicRun.mockResolvedValueOnce(live).mockResolvedValueOnce({
    version: "public_run.v1",
    state: "unavailable",
  });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<RunPage publicId={firstId} onNavigate={vi.fn()} />);
    await Promise.resolve();
  });
  await act(async () => {
    root.render(<RunPage publicId={secondId} onNavigate={vi.fn()} />);
    await Promise.resolve();
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5_000);
  });
  expect(mocks.readPublicRun).toHaveBeenCalledTimes(2);
  expect(container.textContent).toContain("This Run is not available.");
  await act(async () => root.unmount());
});

it("renders a verified result through its declared summary shape, not a Heist schema branch", async () => {
  const publicId = "c".repeat(32);
  mocks.readPublicRun.mockResolvedValue({
    version: "public_run.v1",
    state: "result",
    public_id: publicId,
    activity: {
      title: "Different result fixture", description: "A reviewed independent result.",
      listing_key: "fixture", listing_revision: "blake3:fixture", pack: { id: "fixture", version: "1", revision: "fixture" },
    },
    started_at: "2026-09-08T00:00:00Z", completed_at: "2026-09-08T00:01:00Z",
    evidence: { class: "unranked", label: "Unranked activity" }, participants: [],
    result: { summary: { schema: "independent/contract-result/v7", agreement_count: 4, verdict: "accepted" } },
  });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<RunPage publicId={publicId} onNavigate={vi.fn()} />);
    await Promise.resolve();
  });
  expect(container.textContent).toContain("Agreement count");
  expect(container.textContent).toContain("4");
  expect(container.textContent).toContain("Accepted");
  await act(async () => root.unmount());
});
