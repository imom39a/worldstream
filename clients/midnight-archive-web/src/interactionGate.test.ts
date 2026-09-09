import { describe, expect, it } from "vitest";

import type { HostedLiveSessionSnapshot } from "@worldstream/client";

import {
  archiveActionsEnabled,
  initialArchiveInteractionGate,
  isAuthorizedProjectionCurrent,
  reduceArchiveInteractionGate,
} from "./interactionGate";
import { authorizedBatch, readyState } from "./testFixtures";

describe("Midnight Archive interaction synchronization", () => {
  it("requires a current acknowledged Projection and locks during submission", () => {
    let gate = initialArchiveInteractionGate();
    expect(archiveActionsEnabled(gate)).toBe(false);
    gate = reduceArchiveInteractionGate(gate, { type: "snapshot", current: true });
    expect(archiveActionsEnabled(gate)).toBe(true);
    gate = reduceArchiveInteractionGate(gate, { type: "submission_started" });
    expect(archiveActionsEnabled(gate)).toBe(false);
    gate = reduceArchiveInteractionGate(gate, { type: "submission_finished", current: false });
    expect(archiveActionsEnabled(gate)).toBe(false);
  });

  it("requires an explicit reconnect after visibility loss", () => {
    let gate = reduceArchiveInteractionGate(initialArchiveInteractionGate(), {
      type: "snapshot",
      current: true,
    });
    gate = reduceArchiveInteractionGate(gate, { type: "visibility_hidden" });
    expect(archiveActionsEnabled(gate)).toBe(false);
    gate = reduceArchiveInteractionGate(gate, { type: "visibility_visible" });
    gate = reduceArchiveInteractionGate(gate, { type: "snapshot", current: true });
    expect(archiveActionsEnabled(gate)).toBe(false);
    gate = reduceArchiveInteractionGate(gate, { type: "reconnect_completed", current: true });
    expect(archiveActionsEnabled(gate)).toBe(true);
  });

  it("distinguishes live transport from a current acknowledged participant Projection", () => {
    const live = readyState();
    const snapshot = (overrides: Partial<HostedLiveSessionSnapshot>): HostedLiveSessionSnapshot => ({
      status: "live",
      synchronized: true,
      canAct: true,
      deliveryBatch: authorizedBatch(),
      lastAcknowledgedFrameSeq: live.frameHead,
      actionReceipt: null,
      message: null,
      ...overrides,
    });
    expect(isAuthorizedProjectionCurrent(snapshot({}), live)).toBe(true);
    expect(isAuthorizedProjectionCurrent(snapshot({ lastAcknowledgedFrameSeq: null }), live)).toBe(true);
    expect(isAuthorizedProjectionCurrent(snapshot({ synchronized: false }), live)).toBe(false);
    expect(isAuthorizedProjectionCurrent(snapshot({ canAct: false }), live)).toBe(false);
    expect(isAuthorizedProjectionCurrent(snapshot({ deliveryBatch: null }), live)).toBe(false);
    expect(isAuthorizedProjectionCurrent(snapshot({
      deliveryBatch: authorizedBatch({ roomSequence: live.roomSequence + 1 }),
    }), live)).toBe(false);
    expect(isAuthorizedProjectionCurrent(snapshot({ status: "disconnected" }), live)).toBe(false);
  });
});
