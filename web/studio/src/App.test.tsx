import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import type { DaemonStatus } from "./daemonStatus";

const connected: DaemonStatus = {
  schema: "worldstream/studio-daemon-status/v1",
  connectivity: "connected",
  health: "live",
  readiness: "ready",
  version: {
    product: "0.1.0",
    build_version: "0.1.0",
    source_revision: "0123456789abcdef0123456789abcdef01234567",
  },
  unavailable_reason: null,
};

describe("WorldStream Studio portal", () => {
  it("identifies the operator portal and shows live daemon identity", () => {
    const dom = renderToStaticMarkup(<App status={connected} />);

    expect(dom).toContain("WorldStream Studio");
    expect(dom).toContain("Host operator portal");
    expect(dom).toContain("worldstreamd connected");
    expect(dom).toContain("Process live");
    expect(dom).toContain("Runtime ready");
    expect(dom).toContain("Build 0.1.0");
    expect(dom).not.toContain("Participant Console");
  });

  it("shows a clear unavailable state without stale version claims", () => {
    const dom = renderToStaticMarkup(
      <App
        status={{
          schema: "worldstream/studio-daemon-status/v1",
          connectivity: "unavailable",
          health: "unavailable",
          readiness: "unavailable",
          version: null,
          unavailable_reason: "connection_failed",
        }}
      />,
    );

    expect(dom).toContain("worldstreamd unavailable");
    expect(dom).toContain("Start or reconnect the local daemon");
    expect(dom).not.toContain("Build 0.1.0");
  });
});
