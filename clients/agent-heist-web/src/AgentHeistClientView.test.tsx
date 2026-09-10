// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { AgentHeistClientView } from "./AgentHeistClientView";
import { AGENT_HEIST_REVISION_0_5, type AgentHeistReadyState } from "./liveAdapter";

const hash = (character: string) => `blake3:${character.repeat(64)}`;
function state(role: "navigator" | "insider" | "broker" = "navigator"): AgentHeistReadyState {
  return { kind: "ready", pack: { id: "worldstream.agent-heist", version: "0.5.0", digest: AGENT_HEIST_REVISION_0_5 }, roomHead: { genesisOrTransitionHash: hash("1"), authoritativeStateHash: hash("2") }, roomSequence: 7, frameHead: 9, authorization: { accessMode: "participant", role }, projection: { phase: "briefing", phaseGeneration: 2, phaseStart: "2026-08-15T12:00:30Z", phaseDeadline: "2026-08-15T12:02:00Z", seats: [{ role: "navigator", present: true }, { role: "insider", present: true }, { role: "broker", present: false }], publicClaims: [], plans: [], challenges: [], commitmentCount: 0, outcome: null, privateClues: [], ownCommitment: null, addressedOffers: [] }, offers: [{ offerId: "7:inspect_clue:0", actionType: "inspect_clue", schemaDigest: hash("a"), eligibility: "Current synchronized phase" }] };
}
const render = (ready: AgentHeistReadyState, connection: "live" | "disconnected" = "live") => renderToStaticMarkup(<AgentHeistClientView state={ready} connection={connection} onAct={vi.fn()} />);

describe("Mission focus live participant surface", () => {
  it.each([
    ["navigator", "Route dossier"], ["insider", "Entry time dossier"], ["broker", "Equipment dossier"],
  ] as const)("shows only the reviewed unopened dossier objects for %s", (role, label) => {
    const markup = render(state(role));
    expect(markup).toContain(label);
    expect(markup).toContain('name="clue_id"');
    expect(markup).not.toMatch(/<input[^>]*name="clue_id"/);
    expect(markup).not.toContain("route_service");
  });

  it("removes an already inspected dossier and uses the exact current offer and sequence", async () => {
    const ready = state();
    const host = document.createElement("div"); document.body.append(host);
    const root = createRoot(host); const onAct = vi.fn().mockResolvedValue(undefined);
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    try {
      await act(async () => root.render(<AgentHeistClientView state={ready} connection="live" onAct={onAct} />));
      const select = host.querySelector<HTMLSelectElement>('select[name="clue_id"]')!;
      select.value = "route";
      await act(async () => host.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
      expect(onAct).toHaveBeenCalledWith(expect.objectContaining({ basedOnRoomSeq: 7, offerId: "7:inspect_clue:0", actionType: "inspect_clue", payload: { clue_id: "route" } }));
      const inspected = { ...ready, projection: { ...ready.projection, privateClues: [{ clueId: "route", ownerRole: "navigator" as const, claimCode: "route_private" }] } };
      await act(async () => root.render(<AgentHeistClientView state={inspected} connection="live" onAct={onAct} />));
      expect(host.textContent).toContain("no sealed dossier available");
      expect(host.querySelector('form')).toBeNull();
    } finally { await act(async () => root.unmount()); host.remove(); (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = false; }
  });

  it("fails closed for unreviewed revisions and spectators", () => {
    const unknown = { ...state(), pack: { ...state().pack, digest: hash("f") } };
    expect(render(unknown)).not.toContain("Route dossier");
    const spectator = { ...state(), authorization: { accessMode: "spectator" as const, role: null }, offers: [] };
    const markup = render(spectator);
    expect(markup).toContain("Public spectator view");
    expect(markup).not.toContain("Your dossiers");
    expect(markup).not.toContain("Route dossier");
  });

  it("keeps timed mission status and disables actions while disconnected", () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date("2026-08-15T12:00:30Z"));
    try { const markup = render(state(), "disconnected"); expect(markup).toContain("Reconnect to update timer"); expect(markup).toContain("Reconnect before acting"); expect(markup).toContain("disabled"); } finally { vi.useRealTimers(); }
  });

  it("uses selectors for current plans and exposes a readable rulebook", () => {
    const ready = state(); const plan = { planId: "opaque-plan", proposerRole: "navigator" as const, route: "service", entryWindow: "early", requiredTool: "thermal_key", extraction: "boat", endorsements: 0, challenges: 0 };
    const markup = render({ ...ready, projection: { ...ready.projection, phase: "commitment", plans: [plan] }, offers: [{ ...ready.offers[0]!, actionType: "commit_move" }] });
    expect(markup).toContain('<option value="opaque-plan">');
    expect(markup).not.toMatch(/<input[^>]*name="selected_plan_id"/);
    expect(markup).toContain("At least two sealed choices must name the same plan");
  });

  it("keeps an unsubmitted plan draft through an unrelated synchronized update", async () => {
    const ready = { ...state(), projection: { ...state().projection, phase: "negotiation" as const }, offers: [{ ...state().offers[0]!, offerId: "7:propose_plan:0", actionType: "propose_plan" as const }] };
    const host = document.createElement("div"); document.body.append(host); const root = createRoot(host);
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    try {
      await act(async () => root.render(<AgentHeistClientView state={ready} connection="live" onAct={vi.fn()} />));
      const route = host.querySelectorAll<HTMLButtonElement>('button[role="radio"]')[1]!;
      await act(async () => route.click());
      await act(async () => root.render(<AgentHeistClientView state={{ ...ready, roomSequence: 8, offers: [{ ...ready.offers[0]!, offerId: "8:propose_plan:0" }] }} connection="live" onAct={vi.fn()} />));
      expect(host.querySelectorAll<HTMLButtonElement>('button[role="radio"]')[1]?.getAttribute("aria-checked")).toBe("true");
    } finally { await act(async () => root.unmount()); host.remove(); (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = false; }
  });

  it("puts the authorized result debrief in the mission focus, not behind the crew board", () => {
    const ready = state();
    const markup = render({ ...ready, projection: { ...ready.projection, phase: "result", outcome: {
      outcome: "partial_failure", selectedPlanId: "opaque-plan", voteCounts: { "opaque-plan": 2 }, missingRoles: ["broker"],
      checks: { route: true, entryWindow: false, requiredTool: true, extraction: true, resourceContributed: false }, score: 3, reason: "scored_selected_plan",
    } } });
    const debrief = markup.indexOf("Mission debrief");
    const board = markup.indexOf("Open crew board");
    expect(debrief).toBeGreaterThan(-1);
    expect(debrief).toBeLessThan(board);
    expect(markup).toContain("Missing sealed choices: Broker");
    expect(markup).toContain("× Entry time");
  });

  it("keeps all four plan controls keyboard-focusable and gates the submit while disconnected", async () => {
    const ready = { ...state(), projection: { ...state().projection, phase: "negotiation" as const }, offers: [{ ...state().offers[0]!, actionType: "propose_plan" as const }] };
    const host = document.createElement("div"); document.body.append(host); const root = createRoot(host);
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    try {
      await act(async () => root.render(<AgentHeistClientView state={ready} connection="disconnected" onAct={vi.fn()} />));
      for (const control of host.querySelectorAll<HTMLButtonElement>('button[role="tab"]')) {
        control.focus();
        expect(document.activeElement).toBe(control);
        expect(control.disabled).toBe(false);
      }
      expect(host.querySelector<HTMLButtonElement>('button[type="submit"]')!.disabled).toBe(true);
    } finally { await act(async () => root.unmount()); host.remove(); (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = false; }
  });

  it("builds a four-part illustrated plan one decision at a time before submitting", async () => {
    const ready = { ...state(), projection: { ...state().projection, phase: "negotiation" as const }, offers: [{ ...state().offers[0]!, actionType: "propose_plan" as const }] };
    const host = document.createElement("div"); document.body.append(host); const root = createRoot(host); const onAct = vi.fn().mockResolvedValue(undefined);
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    try {
      await act(async () => root.render(<AgentHeistClientView state={ready} connection="live" onAct={onAct} />));
      expect(host.textContent).toContain("Part 1 of 4");
      expect(host.querySelector<HTMLButtonElement>('button[type="submit"]')!.disabled).toBe(true);
      for (let part = 0; part < 4; part += 1) {
        await act(async () => host.querySelector<HTMLButtonElement>('button[role="radio"]')!.click());
        if (part < 3) await act(async () => host.querySelector<HTMLButtonElement>('.plan-step-controls button:last-child')!.click());
      }
      expect(host.textContent).toContain("Part 4 of 4");
      expect(host.querySelector<HTMLButtonElement>('button[type="submit"]')!.disabled).toBe(false);
      await act(async () => host.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
      expect(onAct).toHaveBeenCalledWith(expect.objectContaining({ actionType: "propose_plan", payload: { route: "canal", entry_window: "late", required_tool: "disguise", extraction: "van" } }));
    } finally { await act(async () => root.unmount()); host.remove(); (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = false; }
  });

  it("moves past an opened dossier to the next useful offered action", () => {
    const ready = state();
    const markup = render({ ...ready, projection: { ...ready.projection, phase: "negotiation", privateClues: [{ clueId: "route", ownerRole: "navigator", claimCode: "route_canal" }] }, offers: [{ ...ready.offers[0]!, actionType: "inspect_clue" }, { ...ready.offers[0]!, offerId: "7:publish_clue:1", actionType: "publish_clue" }] });
    expect(markup).toContain("Your move</span><h3>Share intel");
    expect(markup).toContain('aria-pressed="true">Share intel');
  });

  it("keeps both exchange considerations reachable without raw identifiers", async () => {
    const ready = state(); const active = { ...ready, projection: { ...ready.projection, phase: "negotiation" as const, privateClues: [{ clueId: "route", ownerRole: "navigator" as const, claimCode: "route_canal" }] }, offers: [{ ...ready.offers[0]!, actionType: "offer_exchange" as const }] };
    const host = document.createElement("div"); document.body.append(host); const root = createRoot(host); const onAct = vi.fn().mockResolvedValue(undefined);
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    try {
      await act(async () => root.render(<AgentHeistClientView state={active} connection="live" onAct={onAct} />));
      const recipient = host.querySelector<HTMLSelectElement>('select[name="recipient_role"]')!;
      expect(recipient.textContent).not.toContain("Navigator");
      recipient.value = "insider"; await act(async () => recipient.dispatchEvent(new Event("change", { bubbles: true })));
      const consideration = host.querySelector<HTMLSelectElement>('select[name="consideration"]')!;
      expect(consideration.innerHTML).toContain('value="clue_disclosure:entry_window"');
      consideration.value = "clue_disclosure:entry_window";
      host.querySelector<HTMLSelectElement>('select[name="offered_clue_id"]')!.value = "route";
      await act(async () => host.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
      expect(onAct).toHaveBeenCalledWith(expect.objectContaining({ actionType: "offer_exchange", payload: { recipient_role: "insider", offered_clue_id: "route", consideration: { kind: "clue_disclosure", clue_id: "entry_window" } } }));
    } finally { await act(async () => root.unmount()); host.remove(); (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = false; }
  });

  it("prevents duplicate submissions in the same synchronous click burst", async () => {
    const host = document.createElement("div"); document.body.append(host); const root = createRoot(host);
    const onAct = vi.fn(() => new Promise<void>(() => {}));
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    try {
      await act(async () => root.render(<AgentHeistClientView state={state()} connection="live" onAct={onAct} />));
      host.querySelector<HTMLSelectElement>('select[name="clue_id"]')!.value = "route";
      const form = host.querySelector("form")!;
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await act(async () => {});
      expect(onAct).toHaveBeenCalledTimes(1);
      expect(host.querySelector<HTMLButtonElement>('button[type="submit"]')!.disabled).toBe(true);
      expect(host.textContent).toContain("Sending move…");
    } finally { await act(async () => root.unmount()); host.remove(); (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = false; }
  });

  it("turns a synchronous submit failure into a recoverable notice", async () => {
    const host = document.createElement("div"); document.body.append(host); const root = createRoot(host);
    const onAct = vi.fn(() => { throw new Error("offline"); });
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    try {
      await act(async () => root.render(<AgentHeistClientView state={state()} connection="live" onAct={onAct} />));
      host.querySelector<HTMLSelectElement>('select[name="clue_id"]')!.value = "route";
      await act(async () => host.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
      expect(host.textContent).toContain("could not be submitted");
      expect(host.querySelector<HTMLButtonElement>('button[type="submit"]')!.disabled).toBe(false);
    } finally { await act(async () => root.unmount()); host.remove(); (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = false; }
  });

  it("does not carry a pending receipt message onto a replacement offer", async () => {
    const host = document.createElement("div"); document.body.append(host); const root = createRoot(host);
    let resolve!: () => void; const onAct = vi.fn(() => new Promise<void>((done) => { resolve = done; })); const ready = state();
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    try {
      await act(async () => root.render(<AgentHeistClientView state={ready} connection="live" onAct={onAct} />));
      host.querySelector<HTMLSelectElement>('select[name="clue_id"]')!.value = "route";
      await act(async () => host.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
      const replacement = { ...ready, roomSequence: 8, offers: [{ ...ready.offers[0]!, offerId: "8:inspect_clue:0" }] };
      await act(async () => root.render(<AgentHeistClientView state={replacement} connection="live" onAct={onAct} />));
      expect(host.querySelector<HTMLButtonElement>('button[type="submit"]')!.disabled).toBe(false);
      await act(async () => resolve());
      expect(host.textContent).not.toContain("Sent. Waiting for the Room to confirm the result.");
    } finally { await act(async () => root.unmount()); host.remove(); (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = false; }
  });
});
