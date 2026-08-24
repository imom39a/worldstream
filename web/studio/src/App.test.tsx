import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import type { DaemonLifecycle } from "./daemonLifecycle";
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

const managedRunning: DaemonLifecycle = {
  schema: "worldstream/studio-daemon-lifecycle/v1",
  state: "running",
  operation_id: 1,
  managed_by_supervisor: true,
  failure: null,
};

describe("WorldStream Studio portal", () => {
  it("identifies the operator portal and shows live daemon identity", () => {
    const dom = renderToStaticMarkup(
      <App
        status={connected}
        lifecycle={managedRunning}
        secretStatus={{
          schema: "worldstream/studio-secret-kind-status/v1",
          credentials: [
            { kind: "host_authority", availability: "configured" },
            { kind: "membership_authority", availability: "missing" },
            { kind: "runner_authority", availability: "configured" },
            { kind: "model_provider", availability: "missing" },
          ],
        }}
      />,
    );

    expect(dom).toContain("WorldStream Studio");
    expect(dom).toContain("Host operator portal");
    expect(dom).toContain("worldstreamd connected");
    expect(dom).toContain("Process live");
    expect(dom).toContain("Runtime ready");
    expect(dom).toContain("Build 0.1.0");
    expect(dom).toContain("Stop daemon");
    expect(dom).toContain("Restart daemon");
    expect(dom).toContain("Protected local references");
    expect(dom).toContain("Host authority");
    expect(dom).toContain("Configured");
    expect(dom).toContain("Missing");
    expect(dom).not.toContain("bearer");
    expect(dom).not.toContain("Participant Console");
  });

  it("shows a clear stopped state without stale version claims", () => {
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
        lifecycle={{
          schema: "worldstream/studio-daemon-lifecycle/v1",
          state: "stopped",
          operation_id: 2,
          managed_by_supervisor: false,
          failure: null,
        }}
      />,
    );

    expect(dom).toContain("worldstreamd stopped");
    expect(dom).toContain("Start or reconnect the local daemon");
    expect(dom).toContain("Start daemon");
    expect(dom).not.toContain("Build 0.1.0");
  });

  it("shows restart progress without offering duplicate lifecycle operations", () => {
    const dom = renderToStaticMarkup(
      <App
        status={connected}
        lifecycle={{
          ...managedRunning,
          state: "stopping",
          operation_id: 3,
        }}
      />,
    );

    expect(dom).toContain("Stopping worldstreamd…");
    expect(dom).toContain("Lifecycle operation in progress");
    expect(dom).not.toContain("Restart daemon");
  });

  it("renders bounded failure guidance and blocks unsafe control after reconciliation", () => {
    const dom = renderToStaticMarkup(
      <App
        status={connected}
        lifecycle={{
          ...managedRunning,
          state: "failed",
          managed_by_supervisor: false,
          failure: {
            code: "not_managed",
            explanation: "The live daemon was not started by this Supervisor session.",
            next_action: "Stop it from its original process owner, then retry here.",
          },
        }}
      />,
    );

    expect(dom).toContain("Lifecycle control needs attention");
    expect(dom).toContain("Stop it from its original process owner");
    expect(dom).not.toContain("Retry start");
    expect(dom).not.toContain("Stop daemon");
  });

  it("explains the safe next action for a reconciled externally owned daemon", () => {
    const dom = renderToStaticMarkup(
      <App
        status={connected}
        lifecycle={{
          ...managedRunning,
          managed_by_supervisor: false,
        }}
      />,
    );

    expect(dom).toContain("Daemon reconciled after Supervisor restart");
    expect(dom).toContain("Stop it from its original process owner");
    expect(dom).not.toContain("Stop daemon");
    expect(dom).not.toContain("Restart daemon");
  });
});
