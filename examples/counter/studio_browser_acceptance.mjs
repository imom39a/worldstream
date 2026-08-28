/**
 * Reusable, UI-only Counter Studio acceptance flow.
 *
 * The caller supplies a selected browser tab plus locators grounded from its
 * current DOM.  This module never receives a Room ID, a handoff value, a
 * bearer, or agent-private invocation content.  The protected Console is
 * opened only by clicking Studio's participant-view control.
 *
 * A tab is deliberately small and Playwright-shaped:
 *   { playwright: { getByRole(), getByLabel(), locator() }, domSnapshot() }
 * Optional tab methods are `goto(path)`, `waitForPopup()`, and
 * `observedRequestUrls()`.  The popup callback is normally backed by the
 * browser's tab-claim API, so the protected URL is never reported here.
 */

export const COUNTER_STUDIO_BROWSER_ACCEPTANCE_V1 =
  "worldstream/counter-studio-browser-acceptance/v1";

/**
 * Defaults use accessible names that exist in the Studio and Console source.
 * Root may replace any entry with a DOM-observed descriptor before execution.
 * Descriptors intentionally describe UI semantics rather than routes or IDs.
 */
export const defaultSelectors = Object.freeze({
  studio: {
    agentProfilesHeading: { role: "heading", name: "Agent Profiles", exact: true },
    profileId: { label: "Profile ID", exact: true },
    profileExecutionKind: { label: "Execution kind", exact: true },
    profileHostContractRevision: { label: "Host contract revision", exact: true },
    profileRunnerTemplate: { role: "combobox", name: "Approved Runner Template", exact: true },
    profileProviderAddress: { label: "Provider loopback address", exact: true },
    profileModelId: { label: "Model ID", exact: true },
    profileCredential: { role: "combobox", name: "Owner-installed provider credential", exact: true },
    profileRevision: { label: "Revision", exact: true },
    profileDisplayName: { label: "Display name", exact: true, nth: 0 },
    publishProfile: { role: "button", name: "Publish immutable revision", exact: true, nth: 0 },
    profilePublished: { text: /Published exact revision/ },

    taskTemplatesHeading: { role: "heading", name: "Task Templates", exact: true },
    templateId: { label: "Template ID", exact: true },
    templateRevision: { label: "New revision", exact: true },
    templateDisplayName: { label: "Display name", exact: true, nth: 1 },
    publishTemplate: { role: "button", name: "Publish immutable revision", exact: true, nth: 1 },
    draftId: { label: "New independent draft ID", exact: true },
    createEditableDraft: { role: "button", name: "Create editable draft", exact: true },
    editableDraftCreated: { text: /Created editable draft/ },

    activityStep: { role: "button", name: "1. Activity", exact: true },
    configurationStep: { role: "button", name: "2. Configuration", exact: true },
    seatsStep: { role: "button", name: "3. Seats", exact: true },
    readinessStep: { role: "button", name: "4. Readiness", exact: true },
    reviewStep: { role: "button", name: "5. Review", exact: true },
    useExactRevision: {
      within: { css: "article", hasText: /worldstream\.counter 4\.0\.0/ },
      role: "button", name: "Use exact revision", exact: true,
    },
    continueDraft: { role: "button", name: "Continue", exact: true },
    operatorView: { label: "Include read-only operator view", exact: true },
    saveReview: { role: "button", name: /^(Save Review|Review saved)$/, exact: false },
    reviewSaved: { role: "button", name: "Review saved", exact: true },
    createRoom: { role: "button", name: "Create from reviewed draft", exact: true },
    roomCreated: { text: /Room creation succeeded/ },
    provisionAccess: { role: "button", name: "Provision participant access", exact: true },
    roomActive: { text: "Room active" },
    openParticipantView: { role: "button", name: "Open Participant View ↗", exact: true },
    enableOperatorView: { role: "button", name: "Enable operator view", exact: true },
    counterValue: { text: /Counter value:\s*2\b/ },
    startManagedHost: { role: "button", name: "Start managed host", exact: true },
    stopManagedHost: { role: "button", name: "Stop managed host", exact: true },
    retryManagedHost: { role: "button", name: "Retry managed host", exact: true },
    managedHostCompletionStable: {
      css: "article.runner-attention-row",
      hasText: /Host state\s*Running[\s\S]*Activation\s*Idle[\s\S]*Confirmed result\s*Handled/,
    },
  },
  console: {
    title: { role: "heading", name: "Participant session", exact: true },
    projection: { role: "heading", name: "Authorized Room projection", exact: true },
    privateAck: { role: "button", name: "Submit private_ack", exact: true },
    privateAckCommitted: { text: /Current sequence:\s*1\b/ },
    currentSequenceTwo: { text: /Current sequence:\s*2\b/ },
    valueTwo: { text: /"value"\s*:\s*2\b/ },
    replayHeading: { role: "heading", name: "Authorized Replay", exact: true },
    verifyReplay: { role: "button", name: "Verify Replay at current sequence", exact: true },
    replayVerified: { role: "heading", name: "Verified Canonical History at sequence 2", exact: true },
    authoritativeStateHash: { text: /Authoritative state hash:/ },
    transitionHash: { text: /Lineage hash:/ },
  },
});

const CONSOLE_FORBIDDEN = [
  /wsb1:[0-9a-f]+/i,
  /wsh1:[0-9a-f]+/i,
  /\b(?:room_id|member_id|membership_id|bearer|token_hash|secret_reference|host_authority)\b/i,
  /\b(?:invocation_context|provider_response|private_context_canary|prompt|memory)\b/i,
];

/**
 * Run all phases.  `onStep` is called after each public milestone and receives
 * only fixed, bounded evidence.  It is the intended seam for a harness that
 * pauses after a managed-host start, terminates the process at a defined
 * boundary, and then resumes through the visible retry control.
 */
export async function runStudioBrowserAcceptance({
  tab,
  selectors = defaultSelectors,
  fixture,
  openProtectedConsole,
  onStep,
  pauseAfter,
  beforeManagedHostRetry,
  waitForManagedHostInFlight,
  afterCompletedHostRestart,
  beforeOpenProtectedConsole,
  acceptanceTimeoutMs = 120_000,
} = {}) {
  requireValue(tab, "tab");
  requireValue(fixture, "fixture");
  if (!Number.isInteger(acceptanceTimeoutMs) || acceptanceTimeoutMs < 1_000) {
    throw new TypeError("acceptanceTimeoutMs must be a bounded positive integer");
  }
  tab.textTimeoutMs ??= acceptanceTimeoutMs;
  const evidence = [];
  const step = async (name, action) => {
    const value = await action();
    const entry = { schema: COUNTER_STUDIO_BROWSER_ACCEPTANCE_V1, step: name };
    evidence.push(entry);
    await onStep?.(entry);
    if (pauseAfter === name) return { paused: true, evidence };
    return value;
  };

  let result = await step("managed_profile_published", () =>
    publishManagedProfile(tab, selectors, fixture.profile));
  if (result) return result;
  result = await step("source_draft_reviewed", () =>
    reviewDraft(tab, selectors, fixture.sourceDraft));
  if (result) return result;
  result = await step("task_template_published", () =>
    publishTaskTemplate(tab, selectors, fixture.template));
  if (result) return result;
  result = await step("editable_draft_created", () =>
    createEditableDraft(tab, selectors, fixture.editableDraftId));
  if (result) return result;
  result = await step("editable_draft_reviewed", () =>
    reviewDraft(tab, selectors, fixture.editableDraft ?? fixture.sourceDraft));
  if (result) return result;
  result = await step("room_created_and_ready_at_genesis", () =>
    createRoomAndProvisionAccess(tab, selectors));
  if (result) return result;

  const consoleTab = await step("protected_console_opened", () =>
    openAndVerifyProtectedConsole(tab, selectors, openProtectedConsole, beforeOpenProtectedConsole));
  // A pause before a callback result needs the Console for the next invocation.
  if (consoleTab?.paused) return { ...consoleTab, evidence };
  const participantConsole = consoleTab;
  result = await step("human_private_ack_committed", () =>
    submitPrivateAck(participantConsole, selectors));
  if (result) return result;
  result = await step("managed_host_start_requested", () =>
    requestManagedHostStartOrRetry(tab, selectors));
  if (result) return result;
  await waitForManagedHostInFlight?.({
    schema: COUNTER_STUDIO_BROWSER_ACCEPTANCE_V1,
    step: "managed_host_provider_boundary",
  });
  if (beforeManagedHostRetry) {
    await beforeManagedHostRetry({
      schema: COUNTER_STUDIO_BROWSER_ACCEPTANCE_V1,
      step: "managed_host_restart_boundary",
    });
  } else {
    await stopManagedHost(tab, selectors);
  }
  result = await step("managed_host_restart_requested", () =>
    requestManagedHostStartOrRetry(tab, selectors));
  if (result) return result;
  result = await step("public_counter_value_two", () =>
    verifyPublicCounterValue(tab, selectors));
  if (result) return result;
  result = await step("managed_host_completion_stable", () =>
    waitForManagedHostCompletionStable(tab, selectors));
  if (result) return result;
  result = await step("managed_host_completed_stop_requested", () =>
    stopManagedHost(tab, selectors));
  if (result) return result;
  result = await step("managed_host_completed_restart_requested", () =>
    requestManagedHostStartOrRetry(tab, selectors));
  if (result) return result;
  await waitForManagedHostCompletionStable(tab, selectors);
  await afterCompletedHostRestart?.({
    schema: COUNTER_STUDIO_BROWSER_ACCEPTANCE_V1,
    step: "managed_host_completed_restart_boundary",
  });
  await step("console_replay_verified", () => verifyReplay(participantConsole, selectors));
  return { paused: false, evidence };
}

export async function publishManagedProfile(tab, selectors, profile) {
  requireValue(profile, "fixture.profile");
  await visible(tab, selectors.studio.agentProfilesHeading);
  await fill(tab, selectors.studio.profileId, profile.id);
  await choose(tab, selectors.studio.profileExecutionKind, "managed_reference");
  await fillOptional(tab, selectors.studio.profileHostContractRevision, profile.hostContractRevision);
  await chooseOptional(tab, selectors.studio.profileRunnerTemplate, profile.runnerTemplate);
  await fillOptional(tab, selectors.studio.profileProviderAddress, profile.providerAddress);
  await fillOptional(tab, selectors.studio.profileModelId, profile.modelId);
  await chooseOptional(tab, selectors.studio.profileCredential, profile.credentialId);
  await fill(tab, selectors.studio.profileRevision, profile.revision);
  await fill(tab, selectors.studio.profileDisplayName, profile.displayName);
  await click(tab, selectors.studio.publishProfile);
  await visible(tab, selectors.studio.profilePublished);
}

/** Complete and save an editable draft entirely through the five Studio steps. */
export async function reviewDraft(tab, selectors, draft) {
  requireValue(draft, "draft fixture");
  await click(tab, selectors.studio.activityStep);
  if (draft.selectExactRevision !== false) {
    await click(tab, draft.useExactRevision ?? selectors.studio.useExactRevision);
  }
  await click(tab, selectors.studio.configurationStep);
  await fillFields(tab, draft.configuration ?? []);
  await click(tab, selectors.studio.seatsStep);
  await fillSeats(tab, draft.seats ?? []);
  await click(tab, selectors.studio.readinessStep);
  await click(tab, selectors.studio.reviewStep);
  if (draft.includeOperatorView !== false) await check(tab, selectors.studio.operatorView);
  await click(tab, selectors.studio.saveReview);
  await visible(tab, selectors.studio.reviewSaved);
}

export async function publishTaskTemplate(tab, selectors, template) {
  requireValue(template, "fixture.template");
  await visible(tab, selectors.studio.taskTemplatesHeading);
  await fill(tab, selectors.studio.templateId, template.id);
  await fill(tab, selectors.studio.templateRevision, template.revision);
  await fill(tab, selectors.studio.templateDisplayName, template.displayName);
  await click(tab, selectors.studio.publishTemplate);
}

export async function createEditableDraft(tab, selectors, draftId) {
  if (draftId !== undefined) await fill(tab, selectors.studio.draftId, draftId);
  await click(tab, selectors.studio.createEditableDraft);
  await visible(tab, selectors.studio.editableDraftCreated);
}

export async function createRoomAndProvisionAccess(tab, selectors) {
  await click(tab, selectors.studio.createRoom);
  await visible(tab, selectors.studio.roomCreated);
  await click(tab, selectors.studio.provisionAccess);
  await visible(tab, selectors.studio.roomActive);
}

export async function openAndVerifyProtectedConsole(tab, selectors, openProtectedConsole, beforeOpenProtectedConsole) {
  if (typeof openProtectedConsole !== "function") {
    throw new TypeError("openProtectedConsole callback is required to claim the protected popup");
  }
  await beforeOpenProtectedConsole?.();
  await click(tab, selectors.studio.openParticipantView);
  const consoleTab = await openProtectedConsole();
  requireValue(consoleTab, "protected Console tab");
  await visible(consoleTab, selectors.console.title);
  await visible(consoleTab, selectors.console.projection);
  await assertSafeConsoleSurface(consoleTab);
  return consoleTab;
}

export async function submitPrivateAck(consoleTab, selectors) {
  await click(consoleTab, selectors.console.privateAck);
  // The isolated Counter fixture transitions from Genesis sequence 0 to 1.
  // Do not launch the managed host until the authorized projection confirms it.
  await visible(consoleTab, selectors.console.privateAckCommitted);
  await assertSafeConsoleSurface(consoleTab);
}

/**
 * Request the next managed-host run through the control the current public
 * Activation state exposes. Idle, waiting, and delayed states offer Start;
 * attention and unavailable states offer Retry.
 */
export async function requestManagedHostStartOrRetry(tab, selectors) {
  const deadline = Date.now() + (tab.textTimeoutMs ?? 10_000);
  do {
    if (await isVisible(tab, selectors.studio.startManagedHost)) {
      await click(tab, selectors.studio.startManagedHost);
      return;
    }
    if (await isVisible(tab, selectors.studio.retryManagedHost)) {
      await click(tab, selectors.studio.retryManagedHost);
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  } while (Date.now() < deadline);
  throw new Error("managed_host_start_or_retry_not_visible");
}

export async function stopManagedHost(tab, selectors) {
  await click(tab, selectors.studio.stopManagedHost);
}

/** Wait for the public host row to report a completed, healthy managed turn. */
export async function waitForManagedHostCompletionStable(tab, selectors) {
  await visible(tab, selectors.studio.managedHostCompletionStable);
}

export async function verifyPublicCounterValue(tab, selectors) {
  if (!(await isVisible(tab, selectors.studio.counterValue))) {
    await click(tab, selectors.studio.enableOperatorView);
  }
  await visible(tab, selectors.studio.counterValue);
}

export async function verifyReplay(consoleTab, selectors) {
  await visible(consoleTab, selectors.console.currentSequenceTwo);
  await visible(consoleTab, selectors.console.valueTwo);
  await visible(consoleTab, selectors.console.replayHeading);
  await click(consoleTab, selectors.console.verifyReplay);
  await visible(consoleTab, selectors.console.replayVerified);
  await visible(consoleTab, selectors.console.authoritativeStateHash);
  await visible(consoleTab, selectors.console.transitionHash);
  await assertSafeConsoleSurface(consoleTab);
}

/** Assert only known-safe Console shape; no snapshot is logged on failure. */
export async function assertSafeConsoleSurface(tab) {
  const snapshot = await snapshotOf(tab);
  for (const forbidden of CONSOLE_FORBIDDEN) {
    if (forbidden.test(snapshot)) throw new Error("protected_console_surface_disclosed_forbidden_material");
  }
  if (typeof tab.observedRequestUrls !== "function") {
    throw new Error("protected_console_url_evidence_required");
  }
  const urls = await tab.observedRequestUrls();
  if (!Array.isArray(urls)) throw new Error("protected_console_url_evidence_invalid");
  for (const url of urls) {
    if (CONSOLE_FORBIDDEN.some((forbidden) => forbidden.test(String(url)))) {
      throw new Error("protected_console_request_disclosed_forbidden_material");
    }
  }
}

async function fillSeats(tab, seats) {
  for (const seat of seats) {
    const values = seat.values;
    const controls = seat.controls;
    requireValue(values, "seat values");
    requireValue(controls, "seat controls");
    requireValue(values.principalId, "seat principalId");
    await fill(tab, descriptor(controls, "principalId"), values.principalId);
    await choose(tab, descriptor(controls, "principalKind"), values.principalKind);
    if (values.required !== undefined) {
      await setChecked(tab, descriptor(controls, "required"), values.required);
    }
    if (values.principalKind === "agent") {
      await choose(
        tab,
        descriptor(controls, "agentAssignment"),
        values.agentAssignment ?? "managed",
      );
      await choose(tab, descriptor(controls, "agentProfile"), values.agentProfile);
      if ((values.agentAssignment ?? "managed") === "managed") {
        await choose(tab, descriptor(controls, "runnerTemplate"), values.runnerTemplate);
      }
    }
  }
}

async function fillFields(tab, fields) {
  for (const field of fields) {
    if (field.kind === "select") await choose(tab, field.selector, field.value);
    else if (field.kind === "checkbox") await setChecked(tab, field.selector, field.value === true);
    else await fill(tab, field.selector, field.value);
  }
}

function descriptor(value, key) {
  const selector = value[key];
  if (selector === undefined || selector === null) throw new TypeError(`missing seat selector: ${key}`);
  return selector;
}

async function click(tab, selector) {
  await element(tab, selector).click();
}

async function fill(tab, selector, value) {
  requireValue(value, "field value");
  await element(tab, selector).fill(String(value));
}

async function fillOptional(tab, selector, value) {
  if (value !== undefined && value !== null) await fill(tab, selector, value);
}

async function choose(tab, selector, value) {
  requireValue(value, "select value");
  if (typeof value !== "string" && (typeof value !== "object" || value === null || typeof value.label !== "string")) {
    throw new TypeError("select value must be a string or bounded option label");
  }
  await element(tab, selector).selectOption(value);
}

async function chooseOptional(tab, selector, value) {
  if (value !== undefined && value !== null) await choose(tab, selector, value);
}

async function check(tab, selector) {
  const control = element(tab, selector);
  if (typeof control.check === "function") await control.check();
  else if (!(await control.isChecked())) await control.click();
}

async function setChecked(tab, selector, checked) {
  const control = element(tab, selector);
  if (typeof control.setChecked === "function") await control.setChecked(checked);
  else if ((await control.isChecked()) !== checked) await control.click();
}

async function visible(tab, selector) {
  const control = element(tab, selector);
  if (typeof control.waitFor === "function") await control.waitFor({ state: "visible" });
  if (typeof control.isVisible === "function" && !(await control.isVisible())) {
    throw new Error("required_browser_element_not_visible");
  }
  const expectedText = selector?.text ?? selector?.hasText;
  if (expectedText !== undefined) await waitForText(tab, control, expectedText);
}

async function isVisible(tab, selector) {
  try {
    const control = element(tab, selector);
    if (typeof control.isVisible !== "function") return false;
    if (!(await control.isVisible())) return false;
    const expectedText = selector?.text ?? selector?.hasText;
    if (expectedText === undefined) return true;
    return matches((await control.textContent()) ?? "", expectedText);
  } catch {
    return false;
  }
}

function element(tab, selector) {
  const page = tab.playwright ?? tab;
  if (selector?.within) return elementWithin(tab, selector);
  if (selector?.role) {
    return nth(page.getByRole(selector.role, { name: selector.name, exact: selector.exact }), selector.nth);
  }
  if (selector?.label) {
    return nth(page.getByLabel(selector.label, { exact: selector.exact }), selector.nth);
  }
  if (selector?.text) return page.locator("body");
  if (selector?.css) return nth(filtered(page.locator(selector.css), selector.hasText), selector.nth);
  if (typeof selector === "string") return page.locator(selector);
  throw new TypeError("invalid semantic browser selector");
}

function elementWithin(tab, selector) {
  const container = element(tab, selector.within);
  if (selector.role) {
    return nth(container.getByRole(selector.role, { name: selector.name, exact: selector.exact }), selector.nth);
  }
  if (selector.label) {
    return nth(container.getByLabel(selector.label, { exact: selector.exact }), selector.nth);
  }
  if (selector.css) return nth(filtered(container.locator(selector.css), selector.hasText), selector.nth);
  throw new TypeError("scoped browser selector requires role, label, or css");
}

function nth(control, index) {
  return index === undefined ? control : control.nth(index);
}

function filtered(control, hasText) {
  return hasText === undefined ? control : control.filter({ hasText });
}

function matches(value, expected) {
  return expected instanceof RegExp ? expected.test(value) : value.includes(String(expected));
}

async function waitForText(tab, control, expected) {
  const deadline = Date.now() + (tab.textTimeoutMs ?? 10_000);
  do {
    if (matches((await control.textContent()) ?? "", expected)) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  } while (Date.now() < deadline);
  throw new Error("required_browser_text_not_visible");
}

async function snapshotOf(tab) {
  if (typeof tab.domSnapshot === "function") return String(await tab.domSnapshot());
  const page = tab.playwright ?? tab;
  if (typeof page.content === "function") return String(await page.content());
  throw new TypeError("tab must provide domSnapshot() or playwright.content()");
}

function requireValue(value, name) {
  if (value === undefined || value === null || value === "") {
    throw new TypeError(`${name} is required`);
  }
}
