// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";

import { MidnightArchiveClientView } from "./MidnightArchiveClientView";
import { projection, rawMira, rawProjection, readyState } from "./testFixtures";

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

  it("shows the Archivist's fixed agreement and contextual preservation controls", () => {
    const current = projection({ location: "conservation" });
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView
        state={readyState(current, [
          "stage_move",
          "stage_inspect_conservation",
          "stage_accept_preservation_agreement",
          "stage_prepare_collection",
          "stage_wait",
        ])}
        connection="live"
        actionsEnabled
        onAction={vi.fn()}
      />,
    );
    expect(markup).toContain("Authored offer · Archivist");
    expect(markup).toContain("Preserve the threatened collection and I will open the Conservation–Vault gate.");
    expect(markup).toContain("Accept the preservation agreement");
    expect(markup).toContain("Prepare the threatened collection");
    expect(markup).toContain("Prepare the collection first.");
    expect(markup).toContain("cannot be rewritten in free text");
    expect(markup).not.toContain("<textarea");
  });

  it("submits the fixed agreement as a typed empty-payload intent", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    const onAction = vi.fn().mockResolvedValue(undefined);
    await act(async () => {
      root.render(
        <MidnightArchiveClientView
          state={readyState(projection({ location: "conservation" }), [
            "stage_accept_preservation_agreement",
            "stage_wait",
          ])}
          connection="live"
          actionsEnabled
          onAction={onAction}
        />,
      );
    });
    const accept = [...host.querySelectorAll("button")].find((button) => (
      button.getAttribute("aria-label")?.startsWith("Stage Accept the preservation agreement")
    ));
    expect(accept).toBeDefined();
    await act(async () => accept?.click());
    expect(onAction).toHaveBeenCalledExactlyOnceWith({
      action: "stage_accept_preservation_agreement",
    });
    await act(async () => root.unmount());
  });

  it("explains insufficient power and locked source-protection controls", () => {
    const raw = rawProjection();
    const agreement = raw.preservation_agreement as Record<string, unknown>;
    const conditions = agreement.conditions as Array<Record<string, unknown>>;
    const objectives = raw.optional_objectives as Record<string, unknown>;
    const collection = objectives.collection_preserved as Record<string, unknown>;
    const conservation = projection({
      location: "conservation",
      power: 0,
      preservation_agreement: {
        ...agreement,
        commitment: "accepted",
        conditions: conditions.map((condition, index) => ({
          ...condition,
          status: index < 2 ? "complete" : "pending",
        })),
      },
      optional_objectives: {
        ...objectives,
        collection_preserved: { ...collection, status: "prepared" },
      },
    });
    const conservationMarkup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(conservation, ["stage_move", "stage_wait"])} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    expect(conservationMarkup).toContain("Energize preservation equipment");
    expect(conservationMarkup).toContain("One power charge is required.");

    const plant = projection({ location: "plant", power: 1 });
    const plantMarkup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(plant, ["stage_move", "stage_wait"])} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    expect(plantMarkup).toContain("Protect the source&#x27;s identifying record");
    expect(plantMarkup).toContain("Recover a ledger before protecting the source record.");
  });

  it("renders a committed wait as an authoritative one-turn setback", () => {
    const current = projection({ turns_used: 2, turns_remaining: 14, staged_action: null });
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(current, ["stage_wait"])} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    expect(markup).toContain("14<small> / 16</small>");
    expect(markup).toContain("No Action staged");
    expect(markup).toContain("Wait for one turn");
  });

  it("renders observed source evidence beside attributes and keeps unknown, observed, and recommended separate", () => {
    const raw = rawProjection();
    const candidates = raw.candidates as Array<Record<string, unknown>>;
    const current = projection({
      candidates: candidates.map((candidate) => ({
        ...candidate,
        evidence_assessment: candidate.candidate_id === "ledger-violet" ? "recommended" : "observed",
        observed_evidence: [{
          source_id: "records",
          source_label: "Records intake card",
          attribute_label: "Binding",
          observed_value: "calfskin",
          candidate_value: candidate.candidate_id === "ledger-cobalt" ? "linen" : "calfskin",
          relation: candidate.candidate_id === "ledger-cobalt" ? "does_not_match" : "matches",
        }, {
          source_id: "conservation",
          source_label: "Conservation restoration note",
          attribute_label: "Marking",
          observed_value: "split_star",
          candidate_value: candidate.candidate_id === "ledger-amber" ? "compass_rose" : "split_star",
          relation: candidate.candidate_id === "ledger-amber" ? "does_not_match" : "matches",
        }],
      })),
    });
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(current, ["stage_move", "stage_inspect_records", "stage_wait"])} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    expect(markup).toContain("Observed source evidence");
    expect(markup).toContain("Records intake card");
    expect(markup).toContain("Evidence recommendation");
    expect(markup).toContain("Inspect the intake evidence");
    expect(markup).not.toContain("truth_marker");
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

  it("renders an honest partial-evidence debrief only from the authorized terminal Projection", () => {
    const candidates = (rawProjection().candidates as Array<Record<string, unknown>>).map((candidate) => {
      const visibleAttributes = candidate.visible_attributes as Array<{ label: string; value: string }>;
      const candidateValue = visibleAttributes.find((attribute) => attribute.label === "Binding")?.value;
      return {
        ...candidate,
        evidence_assessment: "observed",
        observed_evidence: [{
          source_id: "records",
          source_label: "Records intake card",
          attribute_label: "Binding",
          observed_value: "calfskin",
          candidate_value: candidateValue,
          relation: candidateValue === "calfskin" ? "matches" : "does_not_match",
        }],
      };
    });
    const current = projection({
      phase: "complete",
      turns_used: 5,
      turns_remaining: 11,
      outcome: { kind: "no_ledger" },
      candidates,
      debrief: {
        evidence_status: "partial",
        message: "Only one authored source was inspected; it did not uniquely identify a candidate.",
        agreement_commitment: "not_accepted",
        optional_objectives: {
          collection_preserved: false,
          source_record_protected: false,
        },
      },
    });
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(current, [])} connection="live" actionsEnabled={false} onAction={vi.fn()} />,
    );
    expect(markup).toContain("Evidence debrief:");
    expect(markup).toContain("Only one authored source was inspected");
    expect(markup).not.toContain("truth_marker");
  });

  it("renders terminal agreement and optional-objective facts", () => {
    const raw = rawProjection();
    const agreement = raw.preservation_agreement as Record<string, unknown>;
    const conditions = agreement.conditions as Array<Record<string, unknown>>;
    const objectives = raw.optional_objectives as Record<string, unknown>;
    const collection = objectives.collection_preserved as Record<string, unknown>;
    const source = objectives.source_record_protected as Record<string, unknown>;
    const current = projection({
      phase: "complete",
      location: "atrium",
      turns_used: 15,
      turns_remaining: 1,
      carried_candidate: "ledger-violet",
      outcome: { kind: "success" },
      gates: { archive_gate: "open", service_hatch: "closed" },
      preservation_agreement: {
        ...agreement,
        commitment: "honored",
        conditions: conditions.map((condition) => ({ ...condition, status: "complete" })),
      },
      optional_objectives: {
        collection_preserved: { ...collection, status: "complete" },
        source_record_protected: { ...source, status: "complete" },
      },
      debrief: {
        evidence_status: "none",
        message: "No authored source was inspected.",
        agreement_commitment: "honored",
        optional_objectives: {
          collection_preserved: true,
          source_record_protected: true,
        },
      },
    });
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(current, [])} connection="live" actionsEnabled={false} onAction={vi.fn()} />,
    );
    expect(markup).toContain("Agreement</dt><dd>honored");
    expect(markup).toContain("Collection preserved</dt><dd>Yes");
    expect(markup).toContain("Source record protected</dt><dd>Yes");
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

  it("preserves the solo mission surface when Mira is absent", () => {
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState()} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    expect(markup).not.toContain("data-testid=\"mira-crew-card\"");
    expect(markup).toContain("Commit Turn");
    expect(markup).toContain("Stage action");
  });

  it("shows Mira waiting and ready progress without rendering private plan payloads", () => {
    const task = {
      status: "assigned",
      revision: 3,
      kind: "investigate_records",
      power_allowance: 1,
      power_spent: 0,
    };
    const waiting = projection({
      mira: rawMira({
        presence: "active", location: "records", mode: "tasked", task,
        planning: {
          status: "waiting", opportunity_revision: 5, plan_revision: 0,
          steps_total: 0, steps_completed: 0, deadline: "2026-09-09T12:34:56.789Z",
        },
        knowledge: { records: "private", conservation: "unknown", verifier_result: null },
      }),
    });
    const waitingMarkup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(waiting, ["set_mira_hold", "defer_mira_contribution", "stage_wait"])} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    expect(waitingMarkup).toContain("Mira is preparing a bounded plan");
    expect(waitingMarkup).toContain("Mira has private Records findings; their values have not been disclosed.");
    expect(waitingMarkup).not.toContain("step_type");
    expect(waitingMarkup).not.toContain("destination&quot;");

    const ready = projection({
      mira: rawMira({
        presence: "active", location: "records", mode: "tasked", task,
        planning: {
          status: "ready", opportunity_revision: 5, plan_revision: 5,
          steps_total: 3, steps_completed: 1, deadline: "none",
        },
        preparation: {
          status: "prepared", for_turn: 2, summary: "Mira will inspect the assigned source.",
        },
      }),
    });
    const readyMarkup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(ready, ["prepare_mira_contribution", "defer_mira_contribution", "stage_wait"])} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    expect(readyMarkup).toContain("1 of 3 complete");
    expect(readyMarkup).toContain("Mira will inspect the assigned source.");
    expect(readyMarkup).toContain("Fenced to turn 2");
  });

  it("dispatches structured assignment, mode, cancellation, preparation, and deferral controls", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    const onAction = vi.fn().mockResolvedValue(undefined);
    const current = projection({
      mira: rawMira({
        presence: "active", location: "atrium", mode: "tasked",
        task: { status: "assigned", revision: 1, kind: "investigate_records", power_allowance: 1, power_spent: 0 },
        planning: {
          status: "ready", opportunity_revision: 2, plan_revision: 2,
          steps_total: 2, steps_completed: 0, deadline: "none",
        },
      }),
    });
    await act(async () => {
      root.render(<MidnightArchiveClientView
        state={readyState(current, [
          "assign_mira_task", "cancel_mira_task", "set_mira_follow", "set_mira_hold",
          "set_mira_regroup", "request_mira_plan", "prepare_mira_contribution",
          "defer_mira_contribution", "stage_wait",
        ])}
        connection="live"
        actionsEnabled
        onAction={onAction}
      />);
    });
    const click = async (selector: string) => {
      const button = host.querySelector<HTMLButtonElement>(selector);
      expect(button?.disabled).toBe(false);
      await act(async () => button?.click());
    };
    await click('button[data-action-type="assign_mira_task"][aria-label="Assign Investigate Conservation with 1 power allowance"]');
    expect(onAction).toHaveBeenLastCalledWith({
      action: "assign_mira_task", task_kind: "investigate_conservation", power_allowance: 1,
    });
    for (const action of [
      "cancel_mira_task", "set_mira_follow", "set_mira_hold", "set_mira_regroup",
      "request_mira_plan", "prepare_mira_contribution", "defer_mira_contribution",
    ]) {
      await click(`button[data-action-type="${action}"]`);
      expect(onAction).toHaveBeenLastCalledWith({ action });
    }
    await act(async () => root.unmount());
  });

  it("explains ineligible and replaced companion work while retaining the personal staged Action", () => {
    const current = projection({
      staged_action: { action_type: "stage_wait", turn_cost: 1, power_cost: 0 },
      mira: rawMira({
        presence: "active", location: "conservation", mode: "holding",
        task: { status: "cancelled", revision: 8, kind: "none", power_allowance: 0, power_spent: 0 },
        planning: {
          status: "not_requested", opportunity_revision: 9, plan_revision: 0,
          steps_total: 0, steps_completed: 0, deadline: "none",
        },
      }),
    });
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(current, ["assign_mira_task", "set_mira_follow", "stage_wait", "commit_turn"])} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    expect(markup).toContain("cancelled");
    expect(markup).toContain("not requested · revision 0");
    expect(markup).toContain("No current bounded plan has an eligible step.");
    expect(markup).toContain("Wait in place");
    expect(markup).toContain("Commit Turn");
  });

  it("shows a recorded parallel contribution and only Pack-disclosed verifier evidence", () => {
    const current = projection({
      power: 2,
      verifier_result: { candidate_id: "ledger-amber", confidence: "verified" },
      mira: rawMira({
        presence: "active", location: "records", mode: "holding",
        task: { status: "complete", revision: 2, kind: "investigate_records", power_allowance: 1, power_spent: 1 },
        planning: {
          status: "complete", opportunity_revision: 4, plan_revision: 4,
          steps_total: 3, steps_completed: 3, deadline: "none",
        },
        knowledge: {
          records: "private", conservation: "unknown",
          verifier_result: { candidate_id: "ledger-amber", confidence: "verified" },
        },
        last_contribution: {
          turn: 1, kind: "use_verifier", summary: "Mira ran the catalog verifier and disclosed its result.",
        },
      }),
    });
    const markup = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(current, ["assign_mira_task", "stage_wait"])} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    expect(markup).toContain("Mira ran the catalog verifier and disclosed its result.");
    expect(markup).toContain("Catalog verifier disclosed: Amber Folio · verified");
    expect(markup).not.toContain("truth_marker");
    expect(markup).not.toContain("private_plan");
  });
});
