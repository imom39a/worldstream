import { describe, expect, it } from "vitest";

import { ActivityClientRequestQueue } from "./index";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("ActivityClientRequestQueue", () => {
  it("runs one Replay click after an active refresh without overlapping transport", async () => {
    const queue = new ActivityClientRequestQueue();
    const refreshGate = deferred<void>();
    const events: string[] = [];
    let active = 0;
    let maximumActive = 0;
    const refresh = queue.run(async () => {
      events.push("refresh:start");
      maximumActive = Math.max(maximumActive, ++active);
      await refreshGate.promise;
      active -= 1;
      events.push("refresh:end");
    });
    const replay = queue.run(async () => {
      events.push("replay:start");
      maximumActive = Math.max(maximumActive, ++active);
      active -= 1;
      events.push("replay:end");
    });

    await Promise.resolve();
    expect(events).toEqual(["refresh:start"]);
    refreshGate.resolve();
    await Promise.all([refresh, replay]);
    expect(events).toEqual(["refresh:start", "refresh:end", "replay:start", "replay:end"]);
    expect(maximumActive).toBe(1);
  });

  it("continues to the queued user request after a failed refresh", async () => {
    const queue = new ActivityClientRequestQueue();
    const refresh = queue.run(async () => { throw new Error("refresh failed"); });
    const replay = queue.run(async () => "replayed");

    await expect(refresh).rejects.toThrow("refresh failed");
    await expect(replay).resolves.toBe("replayed");
  });
});
