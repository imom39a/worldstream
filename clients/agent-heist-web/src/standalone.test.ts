import { afterEach, beforeEach, expect, test, vi } from "vitest";

const spies = vi.hoisted(() => ({ render: vi.fn(), authority: vi.fn(), resume: vi.fn(), startup: vi.fn() }));
vi.mock("react-dom/client", () => ({ createRoot: () => ({ render: spies.render }) }));
vi.mock("./StandaloneAgentHeistClient", () => ({ StandaloneAgentHeistClient: () => null }));
vi.mock("@worldstream/client", () => ({
  selectActivityClientStartup: spies.startup,
  resumeRetainedActivityClient: spies.resume,
  ActivityClientHandoffClient: class {
    constructor(...args: unknown[]) { spies.authority(...args); }
  },
}));

beforeEach(() => {
  vi.resetModules();
  vi.clearAllMocks();
  spies.startup.mockReturnValue({ kind: "direct" });
  spies.resume.mockResolvedValue({ kind: "direct" });
  vi.stubGlobal("document", { getElementById: () => ({}) });
  vi.stubGlobal("fetch", vi.fn(() => { throw new Error("standalone must not request platform authentication"); }));
});
afterEach(() => vi.unstubAllGlobals());

test("the local entrypoint uses the retained Controller handoff without a platform session", async () => {
  vi.stubGlobal("window", { location: { origin: "http://127.0.0.1:5173" } });
  await import("./standalone");
  await vi.waitFor(() => expect(spies.render).toHaveBeenCalledOnce());
  expect(spies.authority).toHaveBeenCalledExactlyOnceWith("http://127.0.0.1:9420");
  expect(spies.resume).toHaveBeenCalledOnce();
  expect(fetch).not.toHaveBeenCalled();
});

test("the local entrypoint refuses remote hosting instead of bypassing hosted authentication", async () => {
  vi.stubGlobal("window", { location: { origin: "https://worldstream-demos.vercel.app" } });
  await import("./standalone");
  expect(spies.startup).toHaveBeenCalledOnce();
  expect(spies.render).toHaveBeenCalledOnce();
  expect(spies.authority).not.toHaveBeenCalled();
  expect(spies.resume).not.toHaveBeenCalled();
  expect(fetch).not.toHaveBeenCalled();
});
