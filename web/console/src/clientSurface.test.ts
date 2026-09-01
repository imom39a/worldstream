import { describe, expect, it } from "vitest";

import { clientSurfaceForPath } from "./clientSurface";

describe("Client Host path selection", () => {
  it("selects the Agent Heist client only on its closed path", () => {
    expect(clientSurfaceForPath("/agent-heist/")).toBe("agent-heist");
    expect(clientSurfaceForPath("/agent-heist")).toBe("agent-heist");
    expect(clientSurfaceForPath("/agent-heist/anything")).toBe("legacy");
    expect(clientSurfaceForPath("/agent-heist-spoof/")).toBe("legacy");
  });

  it("selects the generic Inspector only on its closed path", () => {
    expect(clientSurfaceForPath("/inspector/")).toBe("inspector");
    expect(clientSurfaceForPath("/inspector")).toBe("inspector");
    expect(clientSurfaceForPath("/inspector/anything")).toBe("legacy");
  });

  it("preserves the existing direct bootstrap only outside Activity Client paths", () => {
    expect(clientSurfaceForPath("/")).toBe("legacy");
    expect(clientSurfaceForPath("/prototype/studio")).toBe("legacy");
  });
});
