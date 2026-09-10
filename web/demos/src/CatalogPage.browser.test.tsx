// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  createLaunch: vi.fn(),
  readCatalog: vi.fn(),
  readRecentResults: vi.fn(),
}));

vi.mock("./hostedApi", () => ({
  createLaunch: mocks.createLaunch,
  readCatalog: mocks.readCatalog,
  readRecentResults: mocks.readRecentResults,
  developmentSignIn: vi.fn(),
  githubSignIn: vi.fn(),
  usePlatformSession: () => ({
    session: { state: "authenticated", csrf: "csrf" },
    developmentSignInAvailable: false,
    reload: vi.fn(),
  }),
}));
vi.mock("./siteChrome", () => ({
  SiteHeader: () => <header>Platform header</header>,
  SiteFooter: () => <footer>Platform footer</footer>,
}));

import { CatalogPage } from "./CatalogPage";

const activity = {
  slug: "agent-heist",
  title: "Agent Heist",
  description: "A reviewed activity.",
  availability: "available",
  availabilityMessage: "Ready",
  seatSummary: "1 required seat",
  seats: [{ key: "navigator", label: "Navigator", required: true }],
  creatorMaySpectate: false,
  houseFillAvailable: false,
  publicViewingAvailable: true,
  resultPublication: "Results are reviewed.",
  attribution: "Seats are pseudonymous.",
  clientPath: "/agent-heist/",
  houseTerms: null,
} as const;

afterEach(() => {
  document.body.replaceChildren();
  window.localStorage.clear();
  mocks.createLaunch.mockReset();
  mocks.readCatalog.mockReset();
  mocks.readRecentResults.mockReset();
  vi.restoreAllMocks();
});

it("rotates a stale terminal setup key and creates a fresh launch in one action", async () => {
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", {
    configurable: true,
    value: vi.fn(),
  });
  Object.defineProperty(HTMLDialogElement.prototype, "close", {
    configurable: true,
    value: vi.fn(),
  });
  vi.spyOn(crypto, "randomUUID").mockReturnValue("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
  const storageKey = "worldstream.launch.v1:agent-heist:navigator:people_only";
  window.localStorage.setItem(storageKey, "a".repeat(32));
  mocks.readCatalog.mockResolvedValue([activity]);
  mocks.readRecentResults.mockResolvedValue({ results: [] });
  mocks.createLaunch
    .mockResolvedValueOnce({
      state: "closed_by_creator",
      launch_id: "10000000-0000-4000-8000-000000000001",
    })
    .mockResolvedValueOnce({
      state: "collecting",
      launch_id: "20000000-0000-4000-8000-000000000002",
    });
  const navigate = vi.fn();
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<CatalogPage onNavigate={navigate} />);
    await Promise.resolve();
  });

  const choose = [...container.querySelectorAll("button")]
    .find((candidate) => candidate.textContent === "Enter activity ↗") as HTMLButtonElement;
  await act(async () => {
    choose.click();
    await Promise.resolve();
  });
  const create = [...container.querySelectorAll("button")]
    .find((candidate) => candidate.textContent === "Create waiting room") as HTMLButtonElement;
  await act(async () => {
    create.click();
    await Promise.resolve();
    await Promise.resolve();
  });

  expect(mocks.createLaunch).toHaveBeenCalledTimes(2);
  expect(mocks.createLaunch.mock.calls[0]?.[1].idempotencyKey).toBe("a".repeat(32));
  expect(mocks.createLaunch.mock.calls[1]?.[1].idempotencyKey).toBe(
    "bbbbbbbbbbbb4bbb8bbbbbbbbbbbbbbb",
  );
  expect(navigate).toHaveBeenCalledWith(
    "/launches/20000000-0000-4000-8000-000000000002",
  );
  expect(window.localStorage.getItem(storageKey)).toBeNull();
  await act(async () => root.unmount());
});
