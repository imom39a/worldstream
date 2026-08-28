import assert from "node:assert/strict";
import test from "node:test";

import {
  reviewDraft,
  verifyPublicCounterValue,
} from "./studio_browser_acceptance.mjs";

function tabWithActions(actions) {
  const control = (selector) => ({
    async click() { actions.push(["click", selector]); },
    async fill(value) { actions.push(["fill", selector, value]); },
    async selectOption(value) { actions.push(["select", selector, value]); },
    async check() { actions.push(["check", selector]); },
    async setChecked(value) { actions.push(["checked", selector, value]); },
    async isVisible() { return true; },
    async waitFor() {},
    async textContent() { return ""; },
  });
  return { playwright: { locator: control } };
}

test("reviewDraft keeps seat values separate from semantic controls", async () => {
  const actions = [];
  const tab = tabWithActions(actions);
  const selectors = {
    studio: {
      activityStep: "activity", configurationStep: "configuration", seatsStep: "seats",
      readinessStep: "readiness", reviewStep: "review", useExactRevision: "exact",
      operatorView: "operator", saveReview: "save", reviewSaved: "saved",
    },
  };
  await reviewDraft(tab, selectors, {
    configuration: [{ selector: "initial-value", value: 0 }],
    seats: [{
      values: {
        principalId: "01ARZ3NDEKTSV4RRFFQ69G5FB0",
        principalKind: "agent",
        required: true,
        agentAssignment: "managed",
        agentProfile: { label: "Counter managed reference · v1" },
        runnerTemplate: "counter-managed-reference:v1",
      },
      controls: {
        principalId: "agent-principal", principalKind: "agent-kind",
        required: "agent-required", agentAssignment: "agent-assignment",
        agentProfile: "agent-profile", runnerTemplate: "agent-runner",
      },
    }],
  });
  assert.deepEqual(actions.filter(([kind]) => kind === "fill"), [
    ["fill", "initial-value", "0"],
    ["fill", "agent-principal", "01ARZ3NDEKTSV4RRFFQ69G5FB0"],
  ]);
  assert.equal(actions.some(([, selector]) => selector === "01ARZ3NDEKTSV4RRFFQ69G5FB0"), false);
  assert.deepEqual(
    actions.find(([kind, selector]) => kind === "select" && selector === "agent-profile"),
    ["select", "agent-profile", { label: "Counter managed reference · v1" }],
  );
});

test("public counter assertion waits for asynchronously rendered value", async () => {
  const actions = [];
  let reads = 0;
  const body = {
    async click() { actions.push("enable"); },
    async waitFor() {},
    async isVisible() { return true; },
    async textContent() {
      reads += 1;
      return reads >= 3 ? "Counter value: 2" : "Counter value: 1";
    },
  };
  const tab = {
    textTimeoutMs: 200,
    playwright: {
      locator(selector) {
        if (selector === "body") return body;
        assert.equal(selector, "enable");
        return { async click() { actions.push("enable"); } };
      },
    },
  };
  await verifyPublicCounterValue(tab, {
    studio: { counterValue: { text: /Counter value:\s*2\b/ }, enableOperatorView: "enable" },
  });
  assert.equal(reads, 3);
  assert.deepEqual(actions, ["enable"]);
});
