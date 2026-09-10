import { afterEach, beforeEach, expect, test, vi } from "vitest";

const spies = vi.hoisted(() => ({
  render: vi.fn(),
  authority: vi.fn(),
  controller: vi.fn(),
  publicController: vi.fn(),
}));

vi.mock("react-dom/client", () => ({ createRoot: () => ({ render: spies.render }) }));
vi.mock("./AgentHeistClient", () => ({ AgentHeistClient: () => null }));
vi.mock("@worldstream/client", () => ({
  selectActivityClientStartup: () => ({ kind: "resume" }),
  ActivityClientHandoffClient: class {
    constructor(...args: unknown[]) { spies.authority(...args); }
  },
  HostedLiveSessionController: class {
    constructor(options: unknown) { spies.controller(options); }
  },
  PublicProjectionSessionController: class {
    constructor(options: unknown) { spies.publicController(options); }
  },
}));

const browserOrigin = "https://worldstream-demos.vercel.app";
const flyStream = "wss://worldstream-preview.fly.dev/v1/hosted/browser-stream";

beforeEach(() => {
  vi.resetModules();
  vi.clearAllMocks();
  vi.stubGlobal("document", { getElementById: () => ({}) });
  vi.stubGlobal("window", {
    location: {
      origin: browserOrigin,
      protocol: "https:",
      host: "worldstream-demos.vercel.app",
      search: "?browser_stream_url=wss://other.example/v1/hosted/browser-stream",
    },
  });
});

afterEach(() => vi.unstubAllGlobals());

test("the browser entrypoint connects to authenticated deployment configuration, not its Vercel origin", async () => {
  const csrf = "c".repeat(43);
  const fetchSession = vi.fn().mockResolvedValue({
    ok: true,
    json: async () => ({ authenticated: true, csrf, browser_stream_url: flyStream }),
  });
  vi.stubGlobal("fetch", fetchSession);

  await import("./main");
  await vi.waitFor(() => expect(spies.controller).toHaveBeenCalledOnce());

  expect(fetchSession).toHaveBeenCalledExactlyOnceWith("/api/auth/session", {
    credentials: "include",
    cache: "no-store",
    headers: { Accept: "application/json" },
  });
  expect(spies.authority).toHaveBeenCalledExactlyOnceWith(browserOrigin, undefined, {
    browserOrigin,
    csrf,
  });
  expect(spies.controller.mock.calls[0]?.[0]).toMatchObject({ streamUrl: flyStream });
});

test("missing deployment stream configuration fails closed without a same-origin fallback", async () => {
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue({
    ok: true,
    json: async () => ({ authenticated: true, csrf: "c".repeat(43) }),
  }));

  await import("./main");
  await vi.waitFor(() => expect(spies.render).toHaveBeenCalledOnce());

  expect(spies.authority).not.toHaveBeenCalled();
  expect(spies.controller).not.toHaveBeenCalled();
});

test("the selected public viewer uses a credential-free public projection, not participant admission", async () => {
  const publicId = "a".repeat(32);
  const viewerUrl = `${browserOrigin}/agent-heist-v7/hosted/?public_run=${publicId}&platform_return=%2F&platform_result=%2Fruns%2F${publicId}`;
  vi.stubGlobal("window", {
    location: new URL(viewerUrl),
    history: { replaceState: vi.fn() },
  });
  const publicStream = `wss://worldstream-preview.fly.dev/v1/hosted/public-runs/${publicId}/stream`;
  const fetchRun = vi.fn().mockResolvedValue({
    ok: true,
    json: async () => ({
      version: "public_run.v1",
      state: "live",
      live: { stream_url: publicStream },
      client: { launch_url: viewerUrl },
    }),
  });
  vi.stubGlobal("fetch", fetchRun);

  await import("./main");
  await vi.waitFor(() => expect(spies.publicController).toHaveBeenCalledOnce());

  expect(fetchRun).toHaveBeenCalledExactlyOnceWith(`/api/runs/${publicId}`, {
    credentials: "omit",
    cache: "no-store",
    headers: { Accept: "application/json" },
  });
  expect(spies.publicController).toHaveBeenCalledWith({ streamUrl: publicStream });
  expect(spies.authority).not.toHaveBeenCalled();
  expect(spies.controller).not.toHaveBeenCalled();
});
