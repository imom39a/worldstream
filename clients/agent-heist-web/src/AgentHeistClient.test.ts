import { describe, expect, it } from "vitest";

import { connectionFor } from "./AgentHeistClient";

describe("AgentHeistClient live-session mapping", () => {
  it("keeps synchronization and idle states non-actionable", () => {
    expect(connectionFor("idle")).toBe("connecting");
    expect(connectionFor("connecting")).toBe("connecting");
    expect(connectionFor("synchronizing")).toBe("connecting");
  });

  it("preserves live and recovery boundaries", () => {
    expect(connectionFor("live")).toBe("live");
    expect(connectionFor("disconnected")).toBe("disconnected");
    expect(connectionFor("closed")).toBe("disconnected");
    expect(connectionFor("setup_required")).toBe("setup_required");
  });
});
