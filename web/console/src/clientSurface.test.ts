import { describe, expect, it } from "vitest";

import { clientSurfaceForPath } from "./clientSurface";

describe("Client Host path selection", () => {
  it("selects the generic Inspector only on its closed path", () => {
    expect(clientSurfaceForPath("/inspector/")).toBe("inspector");
    expect(clientSurfaceForPath("/inspector")).toBe("inspector");
    expect(clientSurfaceForPath("/inspector/anything")).toBe("legacy");
  });

  it("preserves the recorded gallery and does not dispatch Pack-specific paths", () => {
    expect(clientSurfaceForPath("/")).toBe("legacy");
    expect(clientSurfaceForPath("/unknown")).toBe("legacy");
    expect(clientSurfaceForPath("/agent-heist/")).toBe("legacy");
    expect(clientSurfaceForPath("/negotiate/")).toBe("legacy");
  });
});
