import { afterEach, expect, it, vi } from "vitest";
import { readMyGames } from "./hostedApi";

afterEach(() => vi.unstubAllGlobals());

it("uses the history freshness header without making it Room authority", async () => {
  const fetch = vi.fn().mockResolvedValue(Response.json({
    version: "platform_my_games.v1", items: [], next: null,
  }, { headers: { "x-worldstream-refresh": "delayed" } }));
  vi.stubGlobal("fetch", fetch);
  const controller = new AbortController();
  expect(await readMyGames(undefined, controller.signal)).toEqual({
    version: "platform_my_games.v1", items: [], next: null, refresh_delayed: true,
  });
  expect(fetch).toHaveBeenCalledOnce();
  expect(fetch).toHaveBeenCalledWith("/api/my-games", { credentials: "same-origin", signal: controller.signal });
});

it("never treats a failed private read as a successful delayed history", async () => {
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json({ error: { code: "temporarily_unavailable" } }, {
    status: 503, headers: { "x-worldstream-refresh": "delayed" },
  })));
  await expect(readMyGames()).rejects.toThrow("temporarily_unavailable");
});
