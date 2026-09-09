import { describe, expect, it } from "vitest";

import { connectionFor } from "./MidnightArchiveClient";

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
});
