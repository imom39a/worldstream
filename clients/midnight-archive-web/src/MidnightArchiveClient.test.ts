import { describe, expect, it } from "vitest";

import { connectionFor, rejectedActionMessage } from "./MidnightArchiveClient";

describe("MidnightArchiveClient transport mapping", () => {
  it("keeps idle and synchronization states non-actionable", () => {
    expect(connectionFor("idle")).toBe("connecting");
    expect(connectionFor("connecting")).toBe("connecting");
    expect(connectionFor("synchronizing")).toBe("connecting");
  });

  it("preserves explicit live and recovery states", () => {
    expect(connectionFor("live")).toBe("live");
    expect(connectionFor("disconnected")).toBe("disconnected");
    expect(connectionFor("closed")).toBe("disconnected");
    expect(connectionFor("setup_required")).toBe("setup_required");
  });

  it("describes a rejected action as an authoritative Room result without inferring provider health", () => {
    const message = rejectedActionMessage("request_mira_plan", "stale_head");
    expect(message).toBe("The authoritative Room rejected request mira plan (stale_head). The board was refreshed; continue only with its current offered controls.");
    expect(message).not.toMatch(/provider|model|service/u);
  });
});
