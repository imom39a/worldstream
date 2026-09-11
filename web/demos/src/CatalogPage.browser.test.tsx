// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  createLaunch: vi.fn(),
  readCatalog: vi.fn(),
}));

vi.mock("./hostedApi", () => ({
  createLaunch: mocks.createLaunch,
  developmentSignIn: vi.fn(),
  githubSignIn: vi.fn(),
  readCatalog: mocks.readCatalog,
  usePlatformSession: () => ({
    session: { state: "authenticated", csrf: "csrf" },
    developmentSignInAvailable: false,
    reload: async () => undefined,
  }),
}));
vi.mock("./RecentResults", () => ({ RecentResultsPanel: () => null }));
vi.mock("./siteChrome", () => ({
  SiteHeader: () => <header>Platform header</header>,
  SiteFooter: () => <footer>Platform footer</footer>,
}));

import { CatalogPage } from "./CatalogPage";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;
const storedLaunchKeys = new Map<string, string>();
Object.defineProperty(window, "localStorage", {
  configurable: true,
  value: {
    clear: () => storedLaunchKeys.clear(),
    getItem: (key: string) => storedLaunchKeys.get(key) ?? null,
    removeItem: (key: string) => storedLaunchKeys.delete(key),
    setItem: (key: string, value: string) => storedLaunchKeys.set(key, value),
  },
});

const agentHeistActivity = {
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
  vi.restoreAllMocks();
});

it("shows reviewed roster descriptions and exact exhibition terms before formation", async () => {
  mocks.readCatalog.mockResolvedValue([{
    slug: "midnight-archive",
    title: "Midnight Archive",
    description: "Recover the authentic ledger and escape.",
    availability: "available",
    availabilityMessage: "Internal four-roster exhibition",
    seatSummary: "1 human lead · up to 2 optional specialists",
    participationKinds: ["human"],
    seats: [
      { key: "seat-1", label: "Expedition lead", required: true },
      { key: "seat-2", label: "Mira", required: false },
      { key: "seat-3", label: "Jonah", required: false },
    ],
    rosterOptions: [
      { key: "solo", label: "Solo", description: "Solve the mission yourself.", seatKeys: ["seat-1"], creatorSeatKeys: ["seat-1"], suppliedAgents: 0 },
      { key: "mira", label: "Mira — evidence specialist", description: "Mira correlates evidence while you lead.", seatKeys: ["seat-1", "seat-2"], creatorSeatKeys: ["seat-1"], suppliedAgents: 1 },
      { key: "jonah", label: "Jonah — service specialist", description: "Jonah prepares service access while you lead.", seatKeys: ["seat-1", "seat-3"], creatorSeatKeys: ["seat-1"], suppliedAgents: 1 },
      { key: "full-crew", label: "Mira + Jonah — full crew", description: "Both specialists work in parallel while you lead.", seatKeys: ["seat-1", "seat-2", "seat-3"], creatorSeatKeys: ["seat-1"], suppliedAgents: 2 },
    ],
    defaultRosterOption: "solo",
    creatorMaySpectate: false,
    houseFillAvailable: true,
    publicViewingAvailable: false,
    resultPublication: "Private debrief in the activity; no public result publication.",
    attribution: "Anonymous viewing is disabled.",
    clientPath: "/midnight-archive-v14/hosted/",
    houseTerms: {
      exhibition: true,
      includedAtNoCharge: true,
      maximumAgents: 2,
      maximumCallsPerAgent: 10,
      maximumInputTokensPerAgent: 120_000,
      maximumOutputTokensPerAgent: 10_000,
      callTimeoutSeconds: 60,
    },
  }]);
  HTMLDialogElement.prototype.showModal = vi.fn();
  HTMLDialogElement.prototype.close = vi.fn();
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<CatalogPage onNavigate={vi.fn()} />);
    await Promise.resolve();
  });
  const enter = Array.from(container.querySelectorAll("button"))
    .find((button) => button.textContent === "Enter activity ↗");
  await act(async () => enter?.click());
  expect(container.textContent).toContain("Solve the mission yourself.");
  expect(container.textContent).toContain("Mira correlates evidence while you lead.");
  expect(container.textContent).toContain("Both specialists work in parallel while you lead.");
  expect(container.textContent).not.toContain("included at no charge");

  const mira = Array.from(container.querySelectorAll("label"))
    .find((label) => label.textContent?.includes("Mira — evidence specialist"))
    ?.querySelector("input");
  await act(async () => mira?.click());
  expect(container.textContent).toContain("included at no charge");
  expect(container.textContent).toContain("permanently marked as unranked exhibitions");
  expect(container.textContent).toContain("10 model calls, 120,000 input tokens, and 10,000 output tokens");
  expect(container.textContent).toContain("Each launch starts fresh");
  expect(container.textContent).toContain("does not carry memory from another Run");
  await act(async () => root.unmount());
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
  mocks.readCatalog.mockResolvedValue([agentHeistActivity]);
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
  const create = container.querySelector("button.primary-launch") as HTMLButtonElement;
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
