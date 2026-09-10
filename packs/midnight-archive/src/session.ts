import type { CanonicalJson, CanonicalObject } from "@worldstream/pack-sdk";
import { type ArchiveState, record, reject } from "./model.js";
import { closeMiraAtTerminal, MIRA_PLAN_TIMER_ID, JONAH_PLAN_TIMER_ID } from "./companions.js";
import { addSecondsToTimestamp, compareTimestamps } from "./time.js";

export const SESSION_TIMER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FA5";
export const SESSION_SECONDS = 86_400;

export function sessionDeadline(startedAt: string): string {
  return addSecondsToTimestamp(startedAt, SESSION_SECONDS);
}

/** Only recorded admission time determines legality; reading a Projection never expires a Room. */
export function requireSessionAdmission(state: ArchiveState, admittedAt: string): void {
  if (state.phase !== "active") reject("inactive", "the expedition is not active");
  if (state.session_deadline === "none" || compareTimestamps(admittedAt, state.session_deadline) >= 0) {
    reject("inactive", "the recorded session deadline has passed");
  }
}

/** The fired Timer has already been consumed; cancel only outstanding generations. */
export function finalizeTerminal(
  state: ArchiveState,
  scheduled: CanonicalObject,
  consumedTimerId?: string,
): { readonly state: ArchiveState; readonly timerRequests: readonly CanonicalJson[] } {
  const timerRequests: CanonicalJson[] = [];
  for (const timerId of [MIRA_PLAN_TIMER_ID, JONAH_PLAN_TIMER_ID, SESSION_TIMER_ID]) {
    if (timerId === consumedTimerId || scheduled[timerId] === undefined) continue;
    const timer = record(scheduled[timerId], "scheduled Archive Timer");
    if (!Number.isSafeInteger(timer.generation) || Number(timer.generation) < 1) {
      throw new TypeError("scheduled Archive Timer generation is invalid");
    }
    timerRequests.push({ timer_request_type: "cancel_current", timer_id: timerId, expected_generation: timer.generation! });
  }
  return { state: closeMiraAtTerminal(closeMiraAtTerminal(state), "jonah"), timerRequests };
}

export function isCurrentSessionExpiry(state: ArchiveState, stimulus: CanonicalObject): boolean {
  if (state.phase !== "active" || state.session_deadline === "none") return false;
  const payload = record(stimulus.canonical_payload, "session Timer payload");
  return stimulus.generation === 1 && stimulus.scheduled_for === state.session_deadline
    && Object.keys(payload).length === 2 && payload.timer === "archive_session"
    && payload.deadline === state.session_deadline;
}
