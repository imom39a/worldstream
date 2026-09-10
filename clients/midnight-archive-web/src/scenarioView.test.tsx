// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";

import { MidnightArchiveClientView } from "./MidnightArchiveClientView";
import { projection, readyState } from "./testFixtures";

const lowReserve = {
  scenario: { id: "low-reserve-v1", label: "Low Reserve" },
  initial_power: 2,
  power: 2,
};

afterEach(() => { document.body.innerHTML = ""; });

describe("authored scenario mission surface", () => {
  it.each([
    { overrides: {}, label: "Standard", capacity: 3 },
    { overrides: lowReserve, label: "Low Reserve", capacity: 2 },
  ])("names $label and exposes its initial reserve accessibly", ({ overrides, label, capacity }) => {
    document.body.innerHTML = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(projection(overrides))} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    const section = document.querySelector('section[aria-labelledby="archive-scenario-title"]');
    expect(section).not.toBeNull();
    expect(section?.querySelector("h2")?.textContent).toBe(label);
    expect(section?.textContent).toContain(`Initial reserve: ${capacity} power charges, shared by the crew.`);
    const meter = document.querySelector('[role="img"].charge-meter');
    expect(meter?.getAttribute("aria-label")).toBe(`${capacity} of ${capacity} power charges remain`);
    expect(meter?.children).toHaveLength(capacity);
    expect(document.querySelector('select, input[name="scenario"]')).toBeNull();
  });

  it("explains the Low Reserve constraint without declaring any candidate authentic", () => {
    document.body.innerHTML = renderToStaticMarkup(
      <MidnightArchiveClientView state={readyState(projection(lowReserve))} connection="live" actionsEnabled onAction={vi.fn()} />,
    );
    const scenario = document.querySelector(".scenario-briefing")?.textContent;
    expect(scenario).toContain("Candidate markings, source evidence, and the authentic ledger differ between scenarios.");
    expect(scenario).toContain("Catalog verifier: 1 power. Ordinary service hatch: 2 power.");
    expect(scenario).toContain("Together they need 3 charges, exceeding this expedition’s 2-charge initial reserve.");
    expect(document.querySelectorAll(".candidate-card")).toHaveLength(3);
    expect(document.querySelectorAll(".candidate-card .evidence-unknown")).toHaveLength(3);
    expect(document.querySelectorAll(".candidate-card .confidence-unverified")).toHaveLength(3);
    expect(document.querySelector(".verifier-result")).toBeNull();
    expect(document.body.textContent).not.toMatch(/truth_marker|standard-v1|low-reserve-v1/u);
  });

  it("retains authoritative single-method offers and only updates power from the next Projection", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    const onAction = vi.fn().mockResolvedValue(undefined);
    const render = async (power: number, offered: boolean) => {
      await act(async () => root.render(
        <MidnightArchiveClientView
          state={readyState(projection({ ...lowReserve, power }), offered ? ["stage_use_verifier"] : [])}
          connection="live" actionsEnabled onAction={onAction}
        />,
      ));
    };
    const verifier = () => [...host.querySelectorAll("button")].find((button) => (
      button.getAttribute("aria-label")?.startsWith("Stage Run the catalog verifier")
    ));
    try {
      await render(2, true);
      expect(verifier()?.disabled).toBe(false);
      await act(async () => verifier()?.click());
      expect(onAction).toHaveBeenCalledExactlyOnceWith({ action: "stage_use_verifier" });
      expect(host.querySelector(".charge-meter")?.getAttribute("aria-label")).toBe("2 of 2 power charges remain");
      await render(1, false);
      expect(host.querySelector(".charge-meter")?.getAttribute("aria-label")).toBe("1 of 2 power charges remain");
      expect(host.querySelectorAll(".charge-meter .charged")).toHaveLength(1);
      expect(host.querySelectorAll(".charge-meter .spent")).toHaveLength(1);
      expect(host.querySelector(".scenario-briefing")?.textContent).toContain("Initial reserve: 2 power charges");
      expect(verifier()?.disabled ?? true).toBe(true);
      await act(async () => verifier()?.click());
      expect(onAction).toHaveBeenCalledTimes(1);
    } finally {
      await act(async () => root.unmount());
    }
  });

  it("uses the projected operation cost for card display and local power gating", () => {
    const current = projection({ location: "records", power: 1 });
    const projected = {
      ...current,
      operationCosts: {
        ...current.operationCosts,
        use_verifier: { turns: 0 as const, power: 2 as const },
      },
    };
    document.body.innerHTML = renderToStaticMarkup(
      <MidnightArchiveClientView
        state={readyState(projected, ["stage_use_verifier"])}
        connection="live"
        actionsEnabled
        onAction={vi.fn()}
      />,
    );

    const verifier = [...document.querySelectorAll("button")].find((button) => (
      button.getAttribute("aria-label")?.startsWith("Stage Run the catalog verifier")
    ));
    expect(verifier?.getAttribute("aria-label")).toContain("costs 0 turn and 2 power");
    expect(verifier?.disabled).toBe(true);
    expect(document.querySelector(".context-actions")?.textContent).toContain("2 power charges are required.");
  });
});
