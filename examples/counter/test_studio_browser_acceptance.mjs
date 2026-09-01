import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  assertSafeConsoleSurface,
  requestManagedHostStartOrRetry,
  reviewDraft,
  runStudioBrowserAcceptance,
  submitPrivateAck,
  verifyPublicCounterValue,
  waitForManagedHostCompletionStable,
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

test("managed host completion waits for the running public idle and handled status", async () => {
  let reads = 0;
  const tab = {
    textTimeoutMs: 200,
    playwright: {
      locator(selector) {
        assert.equal(selector, "article.runner-attention-row");
        return {
          filter() { return this; },
          async waitFor() {},
          async isVisible() { return true; },
          async textContent() {
            reads += 1;
            return reads >= 3
              ? "Host state Running Activation Idle Confirmed result Handled"
              : "Host state Starting Activation Leased Confirmed result Unknown";
          },
        };
      },
    },
  };
  await waitForManagedHostCompletionStable(tab, {
    studio: {
      managedHostCompletionStable: {
        css: "article.runner-attention-row",
        hasText: /Host state\s*Running[\s\S]*Activation\s*Idle[\s\S]*Confirmed result\s*Handled/,
      },
    },
  });
  assert.equal(reads, 3);
});

test("managed host initial and restart requests use the public control available for Activation", async () => {
  for (const { scenario, available } of [
    { scenario: "initial Attention", available: "Retry managed host" },
    { scenario: "idle restart", available: "Start managed host" },
    { scenario: "attention restart", available: "Retry managed host" },
  ]) {
    const actions = [];
    const tab = {
      playwright: {
        getByRole(role, { name }) {
          assert.equal(role, "button");
          return {
            async isVisible() { return name === available; },
            async click() { actions.push(name); },
          };
        },
      },
    };
    const selectors = {
      studio: {
        startManagedHost: { role: "button", name: "Start managed host", exact: true },
        retryManagedHost: { role: "button", name: "Retry managed host", exact: true },
      },
    };
    await requestManagedHostStartOrRetry(tab, selectors);
    assert.deepEqual(actions, [available], scenario);
  }
});

test("private acknowledgement waits for its committed sequence before the next phase", async () => {
  const actions = [];
  let reads = 0;
  const tab = {
    textTimeoutMs: 200,
    playwright: {
      locator(selector) {
        if (selector === "body") {
          return {
            async waitFor() {},
            async isVisible() { return true; },
            async textContent() {
              reads += 1;
              return reads >= 3 ? "Current sequence: 1" : "Current sequence: 0";
            },
          };
        }
        assert.equal(selector, "private-ack");
        return { async click() { actions.push("private-ack"); } };
      },
    },
    async domSnapshot() { return "Authorized Room session"; },
    async observedRequestUrls() { return []; },
  };
  await submitPrivateAck(tab, {
    console: {
      privateAck: "private-ack",
      privateAckCommitted: { text: /Current sequence:\s*1\b/ },
    },
  });
  assert.deepEqual(actions, ["private-ack"]);
  assert.equal(reads, 3);
});

test("whole acceptance flow keeps initial Attention and both restarts in the public control path", async () => {
  let managedPhase = "initial_attention";
  const actions = [];
  const control = (selector) => ({
    async click() {
      actions.push(selector);
      if (selector === "retry") managedPhase = "running";
      if (selector === "stop") managedPhase = managedPhase === "completed" ? "completed_idle" : "idle";
      if (selector === "start") managedPhase = "running";
    },
    async fill() {},
    async selectOption() {},
    async check() {},
    async setChecked() {},
    filter() { return this; },
    async waitFor() {},
    async isVisible() {
      if (selector === "start") return managedPhase === "idle" || managedPhase === "completed_idle";
      if (selector === "retry") return managedPhase === "initial_attention";
      return true;
    },
    async textContent() {
      return "Current sequence: 1 Current sequence: 2 Counter value: 2 Host state Running Activation Idle Confirmed result Handled";
    },
  });
  const tab = {
    playwright: { locator: control },
    async domSnapshot() { return "Authorized Room session"; },
    async observedRequestUrls() { return []; },
  };
  const selectors = {
    studio: Object.fromEntries([
      "agentProfilesHeading", "profileId", "profileExecutionKind", "profileHostContractRevision",
      "profileRunnerTemplate", "profileProviderAddress", "profileModelId", "profileCredential",
      "profileRevision", "profileDisplayName", "publishProfile", "profilePublished",
      "taskTemplatesHeading", "templateId", "templateRevision", "templateDisplayName", "publishTemplate",
      "draftId", "createEditableDraft", "editableDraftCreated", "activityStep", "configurationStep",
      "seatsStep", "readinessStep", "reviewStep", "operatorView", "saveReview", "reviewSaved",
      "createRoom", "roomCreated", "provisionAccess", "roomActive", "openParticipantView",
      "enableOperatorView", "startManagedHost", "stopManagedHost", "retryManagedHost",
    ].map((key) => [key, key.replaceAll(/([A-Z])/g, "-$1").toLowerCase()])),
    console: Object.fromEntries([
      "title", "projection", "privateAck", "privateAckCommitted", "currentSequenceTwo", "valueTwo",
      "replayHeading", "verifyReplay", "replayVerified", "authoritativeStateHash", "transitionHash",
    ].map((key) => [key, `console-${key}`])),
  };
  selectors.studio.counterValue = { text: /Counter value:\s*2\b/ };
  selectors.studio.startManagedHost = "start";
  selectors.studio.stopManagedHost = "stop";
  selectors.studio.retryManagedHost = "retry";
  selectors.console.verifyReplay = "verify-replay";
  selectors.studio.managedHostCompletionStable = {
    css: "completion",
    hasText: /Host state\s*Running[\s\S]*Activation\s*Idle[\s\S]*Confirmed result\s*Handled/,
  };
  const consoleTab = {
    playwright: { locator: control },
    async domSnapshot() { return "Authorized Room session"; },
    async observedRequestUrls() { return []; },
  };
  const result = await runStudioBrowserAcceptance({
    tab,
    selectors,
    fixture: {
      profile: { id: "counter", revision: "v1", displayName: "Counter" },
      sourceDraft: { selectExactRevision: false },
      template: { id: "counter-template", revision: "v1", displayName: "Counter template" },
      editableDraftId: "counter-draft",
      editableDraft: { selectExactRevision: false },
    },
    openProtectedConsole: async () => consoleTab,
  });

  assert.equal(result.paused, false);
  assert.equal(actions.filter((value) => value === "retry").length, 1);
  assert.equal(actions.filter((value) => value === "start").length, 2);
  assert.equal(actions.filter((value) => value === "stop").length, 2);
  assert.ok(actions.includes("verify-replay"));
});

test("protected Console evidence requires captured request URLs", async () => {
  await assert.rejects(
    assertSafeConsoleSurface({ async domSnapshot() { return "Authorized Room session"; } }),
    /protected_console_url_evidence_required/,
  );
});

test("public fixture uses observed non-exact configuration spinbuttons", async () => {
  const fixture = JSON.parse(
    await readFile(new URL("./counter_studio_browser_fixture.json", import.meta.url), "utf8"),
  );
  assert.deepEqual(
    fixture.sourceDraft.configuration.map(({ selector }) => selector),
    [
      { role: "spinbutton", name: "initial_value", exact: false },
      { role: "spinbutton", name: "maximum_value", exact: false },
    ],
  );
});
