// @vitest-environment jsdom
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { MidnightArchiveClientView } from "./MidnightArchiveClientView";
import { readMidnightArchiveProjection } from "./model";
import { projection, rawMira, rawProjection, readyState } from "./testFixtures";

describe("recorded session expiry", () => {
  it("parses expiry with no Outcome and retains deadline, resources, evidence, and debrief", () => {
    const expired = readMidnightArchiveProjection(rawProjection({ phase: "expired", turns_used: 4, turns_remaining: 12, power: 2,
      carried_candidate: "ledger-amber", companion_dialogue: [{ speaker: "mira", turn: 3, text: "Recommend Records." }] }));
    expect(expired).not.toBeNull();
    expect(expired?.phase).toBe("expired");
    expect(expired?.outcome).toBeNull();
    expect(expired?.sessionDeadline).toBe("2026-09-11T12:00:00.123456789Z");
    expect(expired?.turnsUsed).toBe(4); expect(expired?.turnsRemaining).toBe(12); expect(expired?.power).toBe(2);
    expect(expired?.carriedCandidate).toBe("ledger-amber"); expect(expired?.debrief).not.toBeNull();
    expect(expired?.crewDebrief.extractedRoles).toEqual([]);
    expect(expired?.crewDebrief.leftBehindRoles).toEqual([]);
    expect(expired?.companionDialogue[0]?.text).toBe("Recommend Records.");
  });

  it("rejects missing or malformed deadlines, expiry Outcomes, and stale response windows", () => {
    const expired = rawProjection({ phase: "expired" });
    const missing = { ...expired }; delete missing.session_deadline;
    for (const bad of [missing, { ...expired, session_deadline: "none" }, { ...expired, session_deadline: "2026-02-30T12:00:00Z" },
      { ...expired, session_deadline: "2026-09-11T12:00:00.1234567890Z" },
      { ...expired, outcome: { kind: "exhausted_inside" } }, { ...expired, outcome: { kind: "no_ledger" } },
      { ...expired, debrief: null }, { ...expired, session_started_at: "hidden authority" },
      { ...expired, mira: rawMira({ planning: { status: "waiting", opportunity_revision: 1, plan_revision: 0,
        steps_total: 0, steps_completed: 0, deadline: "2026-09-11T12:00:05Z" } }) }]) {
      expect(readMidnightArchiveProjection(bad)).toBeNull();
    }
    expect(readMidnightArchiveProjection(rawProjection({ phase: "briefing", turns_used: 0, turns_remaining: 16 }))?.sessionDeadline).toBeNull();
    expect(readMidnightArchiveProjection(rawProjection({ phase: "briefing", turns_used: 0, turns_remaining: 16,
      session_deadline: "2026-09-11T12:00:00Z" }))).toBeNull();
  });

  it("renders Session expired separately with retained facts and Replay navigation, without enabled turns", () => {
    const markup = renderToStaticMarkup(<MidnightArchiveClientView
      state={readyState(projection({ phase: "expired", turns_used: 4, turns_remaining: 12, power: 2 }), ["commit_turn", "stage_wait"])}
      connection="live" actionsEnabled onAction={vi.fn()} onOpenReplay={vi.fn()}
      replay={{ kind: "available" }} />);
    expect(markup).toContain("Session expired");
    expect(markup).toContain("No extraction outcome was recorded");
    expect(markup).toContain("Evidence debrief"); expect(markup).toContain("Verify Replay");
    expect(markup).toContain("2026-09-11T12:00:00.123456789Z");
    expect(markup).toContain("4 of 16"); expect(markup).toContain("2 charges");
    expect(markup).toContain("Records"); expect(markup).toContain("Violet Ledger");
    expect(markup).not.toContain("Crew extracted"); expect(markup).not.toContain("Crew left behind");
    expect(markup).not.toContain("Expedition complete"); expect(markup).not.toContain("Actions are locked");
    const surface = document.createElement("div"); surface.innerHTML = markup;
    expect(surface.querySelector<HTMLButtonElement>(".commit-button")?.disabled).toBe(true);
    expect(markup).not.toMatch(/truth_marker|speaker_member_id|<input/u);
  });

  it("renders the same active deadline after disconnect and re-entry without local clock expiry", () => {
    const state = readyState(projection());
    for (const connection of ["live", "disconnected"] as const) {
      const markup = renderToStaticMarkup(<MidnightArchiveClientView state={state} connection={connection} actionsEnabled onAction={vi.fn()} />);
      expect(markup).toContain("Re-entry does not pause the deadline");
      expect(markup).toContain("2026-09-11T12:00:00.123456789Z");
      expect(markup).not.toContain("Session expired");
    }
    expect(state.projection.turnsUsed).toBe(1);
  });
});
