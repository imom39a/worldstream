import { expect, it } from "vitest";

import { houseFillFailureDetail, isTerminalLaunchState, stateDetail, stateTitle } from "./LaunchPage";
import type { HostedLaunch } from "./hostedApi";

const terminalLaunch: HostedLaunch = {
  version: "hosted_launch.v1", launch_id: "10000000-0000-4000-8000-000000000001",
  activity_slug: "agent-heist", activity_title: "Agent Heist", state: "cancelled",
  expires_at: "2026-09-08T00:00:00.000Z", can_manage: true, fill_mode: "people_only",
  recovery_state: "not_started", available_actions: [], house_fill: null, seats: [], run: null,
};

it("does not describe terminal no-Genesis history as setup recovery", () => {
  expect(stateDetail(terminalLaunch)).toContain("ended before Genesis");
  expect(stateDetail(terminalLaunch)).not.toContain("checking the original setup");
});

it("models evidence-backed pre-start abandonment as terminal history", () => {
  const abandoned: HostedLaunch = { ...terminalLaunch, state: "abandoned_prestart" };
  expect(isTerminalLaunchState(abandoned.state)).toBe(true);
  expect(stateTitle(abandoned.state)).toBe("This room did not start");
  expect(stateDetail(abandoned)).toContain("ended before Genesis");
  expect(stateDetail(abandoned)).not.toContain("checking the original setup");
});

it("maps only reviewed House-fill failure codes to safe actionable copy", () => {
  expect(houseFillFailureDetail("house_runner_capacity_exhausted"))
    .toBe("House Agents are busy. Invite a person or external agent, or try again later.");
  expect(houseFillFailureDetail("house_credential_unavailable"))
    .toBe("House Agents are unavailable right now. Invite a person or external agent instead.");
  expect(houseFillFailureDetail("untrusted_server_text"))
    .toBe("A reviewed House Agent could not join. Invite a person or external agent instead.");
});
