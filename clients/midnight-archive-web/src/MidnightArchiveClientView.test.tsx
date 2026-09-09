// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";

import { MidnightArchiveClientView } from "./MidnightArchiveClientView";
import { projection, readyState } from "./testFixtures";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("Midnight Archive mission surface", () => {
  it("renders the complete objective, map, resources, gates, candidates, and controls without typed identifiers", () => {
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView
        state={readyState()}
        connection="live"
        actionsEnabled
        onAction={vi.fn()}
      />,
    );
    for (const label of ["Atrium", "Records", "Conservation", "Plant", "Vault"]) {
      expect(markup).toContain(label);
    }
    expect(markup).toContain("Recover the authentic ledger");
    expect(markup).toContain("15<small> / 16</small>");
    expect(markup).toContain("3<small> / 3</small>");
    expect(markup).toContain("Archive gate sealed");
    expect(markup).toContain("Service hatch sealed");
    expect(markup).toContain("Amber Folio");
    expect(markup).toContain("Cobalt Register");
    expect(markup).toContain("Violet Ledger");
    expect(markup).toContain("Commit Turn");
    expect(markup).not.toContain("<input");
    expect(markup).not.toMatch(/room_id|member_id|Bearer\s|wsh1:|wsb1:/u);
  });

  it("keeps staging separate from commit and never spends local resources", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    const onAction = vi.fn().mockResolvedValue(undefined);
    const initial = readyState();
    await act(async () => {
      root.render(
        <MidnightArchiveClientView state={initial} connection="live" actionsEnabled onAction={onAction} />,
      );
    });
    const verifier = [...host.querySelectorAll("button")].find((button) => (
      button.getAttribute("aria-label")?.startsWith("Stage Run the catalog verifier")
    ));
    expect(verifier).toBeDefined();
    await act(async () => verifier?.click());
    expect(onAction).toHaveBeenCalledExactlyOnceWith({ action: "stage_use_verifier" });
    expect(initial.projection.power).toBe(3);
    expect(initial.projection.turnsRemaining).toBe(15);
    expect(host.textContent).toContain("3 / 3");
    expect(host.textContent).toContain("15 / 16");

    const staged = readyState(projection({
      staged_action: { action_type: "stage_use_verifier", turn_cost: 1, power_cost: 1 },
    }), ["stage_move", "stage_use_verifier", "stage_wait", "commit_turn"]);
    await act(async () => {
      root.render(
        <MidnightArchiveClientView state={staged} connection="live" actionsEnabled onAction={onAction} />,
      );
    });
    const commit = [...host.querySelectorAll("button")].find((button) => button.textContent?.includes("Commit Turn"));
    expect(commit?.disabled).toBe(false);
    await act(async () => commit?.click());
    expect(onAction).toHaveBeenLastCalledWith({ action: "commit_turn" });
    expect(staged.projection.power).toBe(3);
    expect(staged.projection.turnsRemaining).toBe(15);
    await act(async () => root.unmount());
  });

  it("shows the documented technical-route controls as location state advances", () => {
    const cases = [
      { location: "atrium", text: "Move to Records" },
      { location: "records", text: "Run the catalog verifier" },
      { location: "plant", text: "Open the service hatch" },
      { location: "vault", text: "Stage this ledger" },
      { location: "atrium", carried_candidate: "ledger-amber", text: "Extract from the archive" },
    ] as const;
    for (const item of cases) {
      const current = projection({
        location: item.location,
        carried_candidate: "carried_candidate" in item ? item.carried_candidate : null,
        gates: item.location === "vault"
          ? { archive_gate: "closed", service_hatch: "open" }
          : { archive_gate: "closed", service_hatch: "closed" },
      });
      const actions = item.location === "records"
        ? ["stage_move", "stage_use_verifier", "stage_wait"] as const
        : item.location === "plant"
          ? ["stage_move", "stage_open_service_hatch", "stage_wait"] as const
          : item.location === "vault"
            ? ["stage_move", "stage_recover_candidate", "stage_wait"] as const
            : ["stage_move", "stage_extract", "stage_wait"] as const;
      const markup = renderToStaticMarkup(
        <MidnightArchiveClientView state={readyState(current, actions)} connection="live" actionsEnabled onAction={vi.fn()} />,
      );
      expect(markup).toContain(item.text);
    }
  });

  it("lets the player exchange a carried ledger for another Vault candidate", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    const onAction = vi.fn().mockResolvedValue(undefined);
    const vault = projection({
      location: "vault",
      carried_candidate: "ledger-amber",
      gates: { archive_gate: "closed", service_hatch: "open" },
    });
    await act(async () => {
      root.render(
        <MidnightArchiveClientView
          state={readyState(vault, ["stage_move", "stage_recover_candidate", "stage_wait"])}
          connection="live"
          actionsEnabled
          onAction={onAction}
        />,
      );
    });

    const amber = host.querySelector('button[aria-label^="Stage recovery of Amber Folio"]');
    const cobalt = host.querySelector('button[aria-label^="Stage recovery of Cobalt Register"]');
    expect(amber).toBeInstanceOf(HTMLButtonElement);
    expect(cobalt).toBeInstanceOf(HTMLButtonElement);
    expect((amber as HTMLButtonElement).disabled).toBe(true);
    expect((cobalt as HTMLButtonElement).disabled).toBe(false);
    await act(async () => (cobalt as HTMLButtonElement).click());
    expect(onAction).toHaveBeenCalledExactlyOnceWith({
      action: "stage_recover_candidate",
      candidate_id: "ledger-cobalt",
    });
    await act(async () => root.unmount());
  });

  it("renders briefing safely with all gameplay controls disabled", () => {
    const briefing = projection({
      phase: "briefing",
      location: "atrium",
      turns_used: 0,
      turns_remaining: 16,
    });
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(briefing, [])} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    expect(markup).toContain("Preparing the archive");
    expect(markup).toContain("Activity Start");
    expect(markup).toMatch(/<button[^>]*disabled=""/u);
  });

  it("renders four distinct factual endings", () => {
    const endings = {
      success: "The authentic ledger is out",
      wrong_ledger: "You escaped with a counterfeit",
      no_ledger: "You escaped empty-handed",
      exhausted_inside: "The archive sealed with you inside",
    } as const;
    for (const [kind, title] of Object.entries(endings)) {
      const remaining = kind === "exhausted_inside" ? 0 : 6;
      const current = projection({
        phase: "complete",
        turns_used: 16 - remaining,
        turns_remaining: remaining,
        outcome: { kind },
      });
      const markup = renderToStaticMarkup(
        <MidnightArchiveClientView state={readyState(current, [])} connection="live" actionsEnabled={false} onAction={vi.fn()} />,
      );
      expect(markup).toContain(title);
      expect(markup).toContain("Replay this expedition");
    }
  });

  it("renders a clear exact-head Replay verification status", () => {
    const current = projection({
      phase: "complete",
      turns_used: 10,
      turns_remaining: 6,
      outcome: { kind: "success" },
    });
    const state = readyState(current, []);
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView
        state={state}
        connection="live"
        actionsEnabled={false}
        onAction={vi.fn()}
        replay={{
          kind: "verified",
          summary: {
            roomSequence: state.roomSequence,
            packDigest: state.pack.digest,
            lineageHash: state.roomHead.genesisOrTransitionHash,
            authoritativeStateHash: state.roomHead.authoritativeStateHash,
            projectionHash: `blake3:${"5".repeat(64)}`,
            verification: "verified",
          },
        }}
      />,
    );
    expect(markup).toContain("Replay verified");
    expect(markup).toContain("Verified at Room sequence 7");
  });

  it("shows no game data before Reset and no actions while stale", () => {
    const awaiting = renderToStaticMarkup(
      <MidnightArchiveClientView state={{ kind: "awaiting" }} connection="connecting" actionsEnabled={false} onAction={vi.fn()} />,
    );
    expect(awaiting).toContain("Waiting for authorized Projection");
    expect(awaiting).not.toContain("Amber Folio");

    const stale = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState()} connection="live" actionsEnabled={false} onAction={vi.fn()} />,
    );
    expect(stale).toContain("Actions are locked");
    expect(stale).toContain("disabled");
  });

  it("offers reconnect while awaiting an authorized Projection", () => {
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView
        state={{ kind: "awaiting" }}
        connection="disconnected"
        actionsEnabled={false}
        message="Activity Client is disconnected."
        onAction={vi.fn()}
        onReconnect={vi.fn()}
      />,
    );
    expect(markup).toContain("Waiting for authorized Projection");
    expect(markup).toContain("Activity Client is disconnected.");
    expect(markup).toContain("Reconnect securely");
  });

  it("prioritizes disconnected recovery over a stale briefing message", () => {
    const briefing = projection({
      phase: "briefing",
      location: "atrium",
      turns_used: 0,
      turns_remaining: 16,
    });
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView
        state={readyState(briefing, [])}
        connection="disconnected"
        actionsEnabled={false}
        message="Realtime connection was interrupted."
        onAction={vi.fn()}
        onReconnect={vi.fn()}
      />,
    );
    expect(markup).toContain("Reconnect required");
    expect(markup).toContain("Realtime connection was interrupted.");
    expect(markup).toContain("Reconnect securely");
    expect(markup).not.toContain("Preparing the archive");
  });
});
