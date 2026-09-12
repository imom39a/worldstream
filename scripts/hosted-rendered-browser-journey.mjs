/**
 * Rendered, local-only hosted-platform qualification.
 *
 * This module deliberately drives the product with Playwright instead of
 * calling the formation or participant APIs from Node.  It is an additional
 * proof beside the protocol acceptance: the browser sees the reviewed
 * platform pages and the independently executing Activity Client surface.
 */
import { chromium } from "playwright";

export const HOSTED_RENDERED_BROWSER_JOURNEY_SCHEMA =
  "worldstream/hosted-rendered-browser-journey/v1";

// The browser journey's returned receipt is deliberately safe to include in
// acceptance evidence.  The exact Run identifier is retained only in this
// process so the local platform acceptance can ask the owner-only service
// data client about that one Run's cleanup receipts without emitting it.
const runIdentityByReceipt = new WeakMap();

const LOCAL_PRODUCT_HOSTS = new Set(["127.0.0.1", "localhost", "[::1]"]);
const DEFAULT_ACTION_TIMEOUT_MS = 60_000;
const DEFAULT_FORMATION_TIMEOUT_MS = 150_000;
const MAX_BROWSER_FAILURES = 8;
const MAX_BROWSER_DIAGNOSTIC_TEXT = 240;
const MAX_RENDERED_STALE_ACTION_ATTEMPTS = 5;
const RENDERED_COMMITMENT_ACTION_WINDOW_MS = 30_000;
const RENDERED_COMMITMENT_OBSERVATION_GRACE_MS = 2_000;
const RENDERED_STALE_ROOM_MESSAGE = "The Room advanced. Reconnect to synchronize before acting.";
const RENDERED_AGENT_HEIST_CLIENT_PATH = "/agent-heist-v12/hosted/";

/**
 * Runs one browser-visible, House-filled local activity.  The local fake
 * provider is part of the hosted-dev contract; this function never accepts a
 * remote product origin or an alternate identity mechanism.
 *
 * A caller owns process startup/shutdown and should put the returned, redacted
 * receipt in its broader acceptance evidence.  The function never prints
 * launch IDs, invite capabilities, handoffs, cookies, or session material.
 */
export async function runHostedRenderedBrowserJourney({
  productOrigin,
  browserBinary = process.env.WORLDSTREAM_BROWSER_BINARY,
  actionTimeoutMs = DEFAULT_ACTION_TIMEOUT_MS,
  formationTimeoutMs = DEFAULT_FORMATION_TIMEOUT_MS,
  existingLaunchId,
  launchBrowser = defaultLaunchBrowser,
} = {}) {
  const origin = localProductOrigin(productOrigin);
  const timeouts = validateTimeouts({ actionTimeoutMs, formationTimeoutMs });
  const retainedLaunchId = existingLaunchId === undefined
    ? null
    : validateLaunchId(existingLaunchId);
  const browser = await launchBrowser(browserBinary);
  const failures = [];
  let participantContext;
  let spectatorContext;
  let publicRunPage;

  try {
    participantContext = await browser.newContext();
    // The waiting room must stay open while its client is attached in a
    // separate tab. Activity Start deliberately waits for that client-side
    // synchronization, so the popup becomes the participant game surface.
    const waitingRoom = await participantContext.newPage();
    collectBrowserFailures(waitingRoom, failures, "waiting-room");
    let participant = waitingRoom;

    await waitingRoom.goto(origin, { waitUntil: "domcontentloaded" });
    await waitingRoom.getByRole("heading", { name: /Pick your world/i }).waitFor({
      timeout: timeouts.action,
    });
    await waitingRoom.getByText("Sign in to start", { exact: true }).waitFor({
      timeout: timeouts.action,
    });

    // Catalog and identity are intentionally visible.  The only local
    // substitute is named on the button; production OAuth is never simulated.
    await waitingRoom.getByRole("button", { name: /Enter activity/i }).click();
    await waitingRoom.getByRole("dialog").waitFor({ timeout: timeouts.action });
    await waitingRoom.getByRole("button", {
      name: "Use visible local-development sign-in",
    }).click();
    await waitingRoom.getByText("Signed in", { exact: true }).waitFor({
      timeout: timeouts.action,
    });

    let launchId;
    let runCapture;
    if (retainedLaunchId === null) {
      const launchDialog = waitingRoom.getByRole("dialog");
      await assertChecked(launchDialog.locator('input[name="seat"][value="seat-1"]'), "Navigator seat");
      await assertChecked(launchDialog.locator('input[name="fill"]:checked'), "House Agent fill mode");
      await launchDialog.getByRole("button", { name: "Create waiting room" }).click();
      await waitingRoom.waitForURL(/\/launches\/[0-9a-f-]{36}\/?$/u, { timeout: timeouts.action });
      launchId = launchIdFromUrl(waitingRoom.url());
      runCapture = capturePublicRunId(waitingRoom, origin, launchId);

      await waitingRoom.getByRole("heading", { name: "Gather your crew" }).waitFor({
        timeout: timeouts.action,
      });
      // Issuing an invite proves the invite/roster surface without leaking its
      // opaque capability into evidence or attempting an unsupported local
      // multi-account impersonation.
      await waitingRoom.getByRole("button", { name: "Copy invite" }).first().click();
      const invitation = waitingRoom.locator('input[aria-label$="invitation URL"]');
      await invitation.first().waitFor({ timeout: timeouts.action });
      const invitationUrl = await invitation.first().inputValue();
      if (!/^http:\/\/(?:127\.0\.0\.1|localhost|\[::1\]):[0-9]{1,5}\/join#invite=[0-9a-f]{64}$/u.test(invitationUrl)) {
        throw new Error("rendered waiting-room invitation did not have the reviewed local shape");
      }
      await waitingRoom.getByRole("heading", { name: "Room roster" }).waitFor({ timeout: timeouts.action });

      await waitingRoom.getByRole("button", { name: "Start activity" }).click();
      participant = await attachRenderedNavigatorBeforeStart({
        waitingRoom,
        productOrigin: origin,
        actionTimeoutMs: timeouts.action,
        formationTimeoutMs: timeouts.formation,
        failures,
      });
    } else {
      // A prior local acceptance can be interrupted after Genesis. Re-enter
      // that exact owner-authorized Launch instead of creating another live
      // Run or releasing the retained one without terminal evidence.
      launchId = retainedLaunchId;
      runCapture = capturePublicRunId(waitingRoom, origin, launchId);
      await waitingRoom.goto(`${origin}/launches/${launchId}`, { waitUntil: "domcontentloaded" });
      participant = await resumeRetainedRenderedNavigator({
        waitingRoom,
        productOrigin: origin,
        actionTimeoutMs: timeouts.action,
        formationTimeoutMs: timeouts.formation,
        failures,
      });
    }
    const runIdentity = await runCapture.wait(timeouts.action);
    const publicId = runIdentity.publicId;

    // A fresh or pre-start retained journey has already redeemed one handoff
    // in the popup above. A post-start retained journey can use the ordinary
    // single-page re-entry path.
    if (participant === waitingRoom) {
      await participant.getByRole("button", { name: /^Enter Navigator$/ }).click();
      await waitForIndependentActivityClient(
        participant,
        timeouts.action,
        "Navigator participant",
        failures,
        origin,
      );
    }
    const routeClaim = await ensureRenderedNavigatorRouteClaim(participant, timeouts.action);
    await ensureRenderedPublishedRouteClaim(participant, routeClaim, timeouts.action);
    const navigatorPlan = navigatorPlanForRouteClaim(routeClaim);
    await ensureRenderedNavigatorPlan(participant, navigatorPlan, timeouts.action);
    await waitForHouseEndorsement(participant, navigatorPlan, timeouts.formation);

    // Leaving is a platform navigation action, not a destructive Membership
    // mutation.  Re-enter before the terminal phase to prove that the current
    // Run Membership, rather than a replacement seat, remains usable.
    await participant.getByRole("button", { name: "Back to games" }).click();
    await participant.waitForURL(`${origin}/`, { timeout: timeouts.action });
    await participant.getByRole("button", { name: "My games" }).click();
    await participant.getByRole("heading", { name: "My games" }).waitFor({ timeout: timeouts.action });
    await participant.getByRole("button", { name: "Return to game" }).click();
    await participant.waitForURL(new RegExp(`/launches/${launchId}/?$`, "u"), { timeout: timeouts.action });
    await participant.getByRole("button", { name: /^Enter Navigator$/ }).click();
    await waitForIndependentActivityClient(
      participant,
      timeouts.action,
      "Navigator participant",
      failures,
      origin,
    );

    // A fresh context is the anonymous browser.  It has neither the platform
    // sign-in cookie nor a Browser Activity Session from the participant.
    spectatorContext = await browser.newContext();
    const spectator = await spectatorContext.newPage();
    collectBrowserFailures(spectator, failures, "spectator");
    await spectator.goto(`${origin}/runs/${publicId}`, { waitUntil: "domcontentloaded" });
    await spectator.getByRole("heading", {
      name: "Open the reviewed Activity Client",
    }).waitFor({ timeout: timeouts.action });
    await spectator.getByRole("link", { name: "Watch live Run" }).click();
    await waitForIndependentActivityClient(
      spectator,
      timeouts.action,
      "Spectator view",
      failures,
      origin,
    );
    const participantActionForms = await renderedParticipantActionForms(spectator).count();
    if (participantActionForms !== 0) {
      throw new Error("anonymous spectator client rendered participant Action controls");
    }

    // Keep the public Run page open beside the spectator Activity Client.  It
    // must reconcile its own live -> verified-result transition; a reload
    // would only prove a fresh read, not the public page's polling contract.
    publicRunPage = await spectatorContext.newPage();
    collectBrowserFailures(publicRunPage, failures, "public-run");
    await publicRunPage.goto(publicRunPath(origin, publicId), { waitUntil: "domcontentloaded" });
    await publicRunPage.getByRole("heading", {
      name: "Open the reviewed Activity Client",
    }).waitFor({ timeout: timeouts.action });
    const publicRunUrlBeforeTerminal = publicRunPage.url();

    // The reviewed House seats must progress the same real Room.  The browser
    // does not forge their Actions; it only waits for its own next rendered
    // offer after the House commitment advances the authoritative state.
    await ensureRenderedCurrentPlanCommitment(participant, timeouts.formation);
    // Agent Heist may auto-complete Result before the acknowledgement control
    // can produce a distinct durable Projection. Observe terminal Complete;
    // do not claim an acknowledgement from a disappearing form.
    await waitForRenderedTerminalComplete(participant, timeouts.formation);

    // The platform's result index is deliberately asynchronous.  The visible
    // My Games page is the acceptance boundary, not an out-of-band result
    // poll, and it must lead to the anonymous-safe public result surface.
    await participant.getByRole("button", { name: "Back to games" }).click();
    await participant.waitForURL(`${origin}/`, { timeout: timeouts.action });
    await participant.getByRole("button", { name: "My games" }).click();
    await participant.getByRole("heading", { name: "My games" }).waitFor({ timeout: timeouts.action });
    const exactResultCard = participant.locator(
      `article[data-launch-id="${launchId}"][data-result-public-id="${publicId}"]`,
    );
    await exactResultCard.waitFor({ state: "attached", timeout: timeouts.formation });
    await exactResultCard.getByRole("button", { name: "View result", exact: true }).waitFor({
      timeout: timeouts.formation,
    });
    await exactResultCard.getByRole("button", { name: "View result", exact: true }).click();
    await participant.getByRole("heading", { name: "Activity complete" }).waitFor({
      timeout: timeouts.formation,
    });

    await publicRunPage.getByRole("heading", { name: "Activity complete" }).waitFor({
      timeout: timeouts.formation,
    });
    if (publicRunPage.url() !== publicRunUrlBeforeTerminal) {
      throw new Error("public Run page navigated during live-to-result reconciliation");
    }

    if (failures.length > 0) throw new Error(failures.join("\n"));
    const receipt = Object.freeze({
      schema: HOSTED_RENDERED_BROWSER_JOURNEY_SCHEMA,
      outcome: "passed",
      completed: true,
      checks: Object.freeze([
        "rendered_catalog",
        "visible_local_identity",
        "rendered_role_and_fill_selection",
        "rendered_waiting_room_invitation_and_roster",
        "rendered_room_start",
        "independent_participant_activity_client",
        "rendered_navigator_completion_actions",
        "house_endorsed_and_committed",
        "leave_to_my_games",
        "original_membership_reentry",
        "anonymous_public_spectator_client",
        "anonymous_public_run_page_reconciled_without_reload",
        "anonymous_public_result",
      ]),
      provider: "local_fake_provider_only",
    });
    runIdentityByReceipt.set(receipt, runIdentity);
    return receipt;
  } finally {
    await spectatorContext?.close();
    await participantContext?.close();
    await browser.close();
  }
}

/**
 * Recovers one retained local Run without replaying a fresh game's Action
 * script. A retained Run may have advanced past the offers that the fresh
 * qualification follows, so this path only observes the authorized terminal
 * Activity Client Projection and then verifies the exact owner's My Games
 * entry. It is deliberately not a full rendered-journey receipt.
 */
export async function runHostedRenderedRetainedRecovery({
  productOrigin,
  existingLaunchId,
  browserBinary = process.env.WORLDSTREAM_BROWSER_BINARY,
  actionTimeoutMs = DEFAULT_ACTION_TIMEOUT_MS,
  formationTimeoutMs = DEFAULT_FORMATION_TIMEOUT_MS,
  launchBrowser = defaultLaunchBrowser,
} = {}) {
  const origin = localProductOrigin(productOrigin);
  const timeouts = validateTimeouts({ actionTimeoutMs, formationTimeoutMs });
  const launchId = validateLaunchId(existingLaunchId);
  const browser = await launchBrowser(browserBinary);
  const failures = [];
  let participantContext;

  try {
    participantContext = await browser.newContext();
    const participant = await participantContext.newPage();
    collectBrowserFailures(participant, failures, "retained-participant");

    await participant.goto(origin, { waitUntil: "domcontentloaded" });
    await participant.getByRole("heading", { name: /Pick your world/i }).waitFor({
      timeout: timeouts.action,
    });
    await participant.getByText("Sign in to start", { exact: true }).waitFor({
      timeout: timeouts.action,
    });
    await participant.getByRole("button", { name: /Enter activity/i }).click();
    await participant.getByRole("dialog").waitFor({ timeout: timeouts.action });
    await participant.getByRole("button", {
      name: "Use visible local-development sign-in",
    }).click();
    await participant.getByText("Signed in", { exact: true }).waitFor({
      timeout: timeouts.action,
    });

    const runCapture = capturePublicRunId(participant, origin, launchId);
    await participant.goto(`${origin}/launches/${launchId}`, { waitUntil: "domcontentloaded" });
    await participant.getByRole("heading", { name: "Your activity is ready" }).waitFor({
      timeout: timeouts.formation,
    });
    const runIdentity = await runCapture.wait(timeouts.action);

    await participant.getByRole("button", { name: /^Enter Navigator$/ }).click();
    await waitForIndependentActivityClient(
      participant,
      timeouts.action,
      "Navigator participant",
      failures,
      origin,
    );
    await waitForRenderedTerminalComplete(participant, timeouts.formation);

    const verifiedHistory = captureVerifiedMyGamesLaunch(participant, origin, launchId);
    await participant.getByRole("button", { name: "Back to games" }).click();
    await participant.waitForURL(`${origin}/`, { timeout: timeouts.action });
    await participant.getByRole("button", { name: "My games" }).click();
    await participant.getByRole("heading", { name: "My games" }).waitFor({
      timeout: timeouts.action,
    });
    await verifiedHistory.wait(timeouts.formation);

    if (failures.length > 0) throw new Error(failures.join("\n"));
    const receipt = Object.freeze({
      schema: HOSTED_RENDERED_BROWSER_JOURNEY_SCHEMA,
      outcome: "passed",
      completed: true,
      recovery: "retained_terminal_only",
      checks: Object.freeze([
        "retained_launch_reentry",
        "authorized_terminal_complete",
        "exact_launch_verified_result",
      ]),
      provider: "local_fake_provider_only",
    });
    runIdentityByReceipt.set(receipt, runIdentity);
    return receipt;
  } finally {
    await participantContext?.close();
    await browser.close();
  }
}

/**
 * Returns the exact local Run identity associated with one in-process
 * rendered-journey receipt. It is intentionally not serializable evidence.
 */
export function renderedJourneyRunId(receipt) {
  const identity = runIdentityByReceipt.get(receipt);
  if (typeof identity?.runId !== "string") {
    throw new Error("rendered journey receipt has no local Run identity");
  }
  return identity.runId;
}

/**
 * Returns the opaque creator entry selector only to the in-process local
 * acceptance assertion. It is never serialized into the safe receipt.
 */
export function renderedJourneyEntrySelector(receipt) {
  const identity = runIdentityByReceipt.get(receipt);
  if (typeof identity?.entrySelector !== "string") {
    throw new Error("rendered journey receipt has no local entry selector");
  }
  return identity.entrySelector;
}

export function localProductOrigin(value) {
  if (typeof value !== "string" || value.length === 0 || value.length > 512) {
    throw new Error("rendered browser qualification requires a local product origin");
  }
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    throw new Error("rendered browser qualification requires a local product origin");
  }
  if (
    parsed.protocol !== "http:" ||
    !LOCAL_PRODUCT_HOSTS.has(parsed.hostname) ||
    parsed.port === "" ||
    parsed.username !== "" ||
    parsed.password !== "" ||
    parsed.pathname !== "/" ||
    parsed.search !== "" ||
    parsed.hash !== "" ||
    parsed.origin !== value
  ) {
    throw new Error("rendered browser qualification requires an exact loopback HTTP product origin");
  }
  return parsed.origin;
}

export function launchIdFromUrl(value) {
  const parsed = new URL(value);
  const match = parsed.pathname.match(/^\/launches\/([0-9a-f-]{36})\/?$/u);
  if (match?.[1] === undefined) throw new Error("rendered launch did not navigate to one exact waiting room");
  return match[1];
}

export function validateLaunchId(value) {
  if (
    typeof value !== "string" ||
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(value)
  ) {
    throw new Error("rendered retained Launch identity is invalid");
  }
  return value;
}

export function publicRunPath(origin, publicId) {
  if (
    typeof publicId !== "string" ||
    !/^[0-9a-f]{32}$/u.test(publicId)
  ) {
    throw new Error("rendered public Run identity is invalid");
  }
  return `${origin}/runs/${publicId}`;
}

/**
 * Validates the exact owner index item for one retained Launch. This prevents
 * an unrelated verified result from satisfying retained recovery evidence.
 */
export function verifiedMyGamesLaunch(value, launchId) {
  validateLaunchId(launchId);
  if (
    value === null ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    value.version !== "platform_my_games.v1" ||
    !Array.isArray(value.items)
  ) {
    throw new Error("rendered My Games response is invalid");
  }
  const matches = value.items.filter((item) =>
    item !== null && typeof item === "object" && !Array.isArray(item) && item.launch_id === launchId,
  );
  if (matches.length !== 1) {
    throw new Error("rendered My Games did not contain exactly one retained Launch");
  }
  const item = matches[0];
  if (
    item.state !== "verified_result" ||
    item.action !== "view_result" ||
    typeof item.result_public_id !== "string" ||
    !/^[0-9a-f]{32}$/u.test(item.result_public_id)
  ) {
    throw new Error("rendered retained Launch has no verified result");
  }
  return item.result_public_id;
}

export function validateTimeouts({ actionTimeoutMs, formationTimeoutMs }) {
  for (const [label, value] of Object.entries({ actionTimeoutMs, formationTimeoutMs })) {
    if (!Number.isSafeInteger(value) || value < 1_000 || value > 300_000) {
      throw new Error(`${label} must be a bounded whole number of milliseconds`);
    }
  }
  return Object.freeze({ action: actionTimeoutMs, formation: formationTimeoutMs });
}

async function defaultLaunchBrowser(browserBinary) {
  // Do not try an HTTP substitute if Playwright/the browser is unavailable.
  // Rendered qualification is absent in that case and must fail the candidate.
  return chromium.launch({
    headless: true,
    ...(browserBinary === undefined || browserBinary === "" ? {} : { executablePath: browserBinary }),
  });
}

function collectBrowserFailures(page, failures, label) {
  const record = (value) => {
    if (failures.length < MAX_BROWSER_FAILURES) failures.push(safeBrowserDiagnosticText(value));
  };
  page.on("pageerror", (error) => record(`${label} page error: ${safeBrowserError(error)}`));
  page.on("console", (message) => {
    const failure = browserConsoleFailure(message.type(), message.text());
    if (failure !== null) record(`${label} ${failure}`);
  });
  page.on("requestfailed", (request) => {
    const failure = browserRequestFailure(request.url(), request.failure()?.errorText);
    if (failure !== null) record(`${label} ${failure}`);
  });
  page.on("response", (response) => {
    const failure = sameOriginBrowserResponseFailure(page.url(), response.url(), response.status());
    if (failure !== null) record(`${label} ${failure}`);
  });
}

/**
 * Chromium emits a generic console error for every failed HTTP resource. The
 * response listener above retains the actionable same-origin status and path,
 * so retaining this URL-free duplicate only creates false failures for the
 * catalog's deliberate anonymous 401 probe.
 */
export function browserConsoleFailure(type, value) {
  if (type !== "error") return null;
  const message = safeBrowserDiagnosticText(value);
  if (/^Failed to load resource: the server responded with a status of [45][0-9]{2}(?: \([^)]*\))?$/u.test(message)) {
    return null;
  }
  return `console error: ${message}`;
}

/**
 * A navigation or React development StrictMode cleanup intentionally aborts
 * an in-flight fetch. Playwright exposes that as requestfailed even though no
 * transport or application request failed. Other network failures stay
 * visible with their bounded, query-free endpoint.
 */
export function browserRequestFailure(url, errorText) {
  if (errorText === "net::ERR_ABORTED") return null;
  return `request failed: ${safeBrowserUrl(url)}`;
}

function safeBrowserError(error) {
  return error instanceof Error ? error.message : "unknown browser page error";
}

async function assertChecked(locator, label) {
  await locator.waitFor({ state: "attached" });
  if (!(await locator.isChecked())) throw new Error(`rendered ${label} was not selected by the reviewed listing`);
}

function capturePublicRunId(page, origin, launchId) {
  let runIdentity = null;
  const expectedPath = `/api/launches/${launchId}`;
  const listener = (response) => {
    void (async () => {
      const url = new URL(response.url());
      if (url.origin !== origin || url.pathname !== expectedPath || !response.ok()) return;
      const body = await response.json().catch(() => null);
      const publicId = body?.run?.public_id;
      const runId = body?.run?.run_id;
      const entries = Array.isArray(body?.run?.entries) ? body.run.entries : [];
      const entrySelector = entries.find((entry) => entry?.label === "Navigator")?.entry_selector;
      if (
        typeof publicId === "string" && /^[0-9a-f]{32}$/u.test(publicId) &&
        typeof runId === "string" &&
        /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(runId) &&
        typeof entrySelector === "string" && /^[0-9a-f]{32}$/u.test(entrySelector)
      ) {
        runIdentity = Object.freeze({ publicId, runId, entrySelector });
      }
    })();
  };
  page.on("response", listener);
  return {
    async wait(timeoutMs) {
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        if (runIdentity !== null) {
          page.off("response", listener);
          return runIdentity;
        }
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      page.off("response", listener);
      throw new Error("rendered waiting room did not expose a public Run through its reviewed response");
    },
  };
}

function captureVerifiedMyGamesLaunch(page, origin, launchId) {
  let resultPublicId = null;
  const expectedPath = "/api/my-games";
  const listener = (response) => {
    void (async () => {
      const url = new URL(response.url());
      if (url.origin !== origin || url.pathname !== expectedPath || !response.ok()) return;
      const body = await response.json().catch(() => null);
      try {
        resultPublicId = verifiedMyGamesLaunch(body, launchId);
      } catch {
        // The owner index may still be publication_pending. The page polls
        // again; only the exact verified item satisfies retained recovery.
      }
    })();
  };
  page.on("response", listener);
  return {
    async wait(timeoutMs) {
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        if (resultPublicId !== null) {
          page.off("response", listener);
          return resultPublicId;
        }
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      page.off("response", listener);
      throw new Error("rendered retained Launch did not reach an exact verified My Games result");
    },
  };
}

/**
 * A new launch reaches a retained Room before its Activity Client has
 * synchronized. The product preserves the waiting room and opens the client
 * in a one-use popup; the popup is the one authority-bearing participant
 * surface for the rest of this journey.
 */
export async function attachRenderedNavigatorBeforeStart({
  waitingRoom,
  productOrigin,
  actionTimeoutMs,
  formationTimeoutMs,
  failures,
}) {
  const openNavigator = waitingRoom.getByRole("button", { name: "Open Navigator to sync" });
  await openNavigator.waitFor({ timeout: formationTimeoutMs });
  const [participant] = await Promise.all([
    waitingRoom.waitForEvent("popup", { timeout: actionTimeoutMs }),
    openNavigator.click(),
  ]);
  collectBrowserFailures(participant, failures, "participant");
  await waitForIndependentActivityClient(
    participant,
    actionTimeoutMs,
    "Navigator participant",
    failures,
    productOrigin,
  );
  await waitForRenderedParticipantLive(participant, actionTimeoutMs);
  await waitingRoom.getByRole("heading", { name: "Your activity is ready" }).waitFor({
    timeout: formationTimeoutMs,
  });
  return participant;
}

/**
 * A retained rendered journey may resume either before or after Activity
 * Start. Wait for the first usable owner entry action, synchronize it when
 * the Host is still waiting for readiness, and otherwise leave ordinary
 * post-start entry to the caller.
 */
export async function resumeRetainedRenderedNavigator({
  waitingRoom,
  productOrigin,
  actionTimeoutMs,
  formationTimeoutMs,
  failures,
}) {
  const entryAction = waitingRoom.getByRole("button", {
    name: /^(?:Start activity|Open Navigator to sync|Enter Navigator)$/u,
  }).first();
  await entryAction.waitFor({ timeout: formationTimeoutMs });
  const label = (await entryAction.innerText()).trim();
  if (label === "Start activity") {
    await entryAction.click();
  }
  if (label === "Start activity" || label === "Open Navigator to sync") {
    return attachRenderedNavigatorBeforeStart({
      waitingRoom,
      productOrigin,
      actionTimeoutMs,
      formationTimeoutMs,
      failures,
    });
  }
  if (label !== "Enter Navigator") {
    throw new Error("rendered retained Launch exposed an unknown Navigator entry action");
  }
  await waitingRoom.getByRole("heading", { name: "Your activity is ready" }).waitFor({
    timeout: actionTimeoutMs,
  });
  return waitingRoom;
}

async function waitForRenderedParticipantLive(page, timeoutMs) {
  await page.locator("div.mission-connection").getByText("Mission link live", {
    exact: true,
  }).waitFor({ timeout: timeoutMs });
}

export async function waitForIndependentActivityClient(
  page,
  timeoutMs,
  expectedRole,
  failures,
  productOrigin,
) {
  try {
    const expectedOrigin = localProductOrigin(productOrigin);
    await page.waitForURL(
      (url) => url.origin === expectedOrigin
        && url.pathname === RENDERED_AGENT_HEIST_CLIENT_PATH,
      { timeout: timeoutMs },
    );
    await page.getByRole("heading", { name: "Agent Heist" }).waitFor({ timeout: timeoutMs });
    const authorization = expectedRole === "Navigator participant"
      ? {
          selector: "section.role-card.role-navigator",
          label: "Your role",
          heading: "Navigator",
        }
      : expectedRole === "Spectator view"
        ? {
            selector: "section.role-card.spectator-card",
            label: "Public spectator view",
            heading: "Follow the crew",
          }
        : null;
    if (authorization === null) {
      throw new Error("rendered Activity Client expected an unreviewed authorization surface");
    }
    const roleCard = page.locator(authorization.selector);
    await roleCard.getByText(authorization.label, { exact: true }).waitFor({ timeout: timeoutMs });
    await roleCard.getByRole("heading", {
      name: authorization.heading,
      exact: true,
    }).waitFor({ timeout: timeoutMs });
    if (!isReviewedRenderedActivityClientUrl(page.url(), expectedOrigin, expectedRole)) {
      throw new Error("rendered Activity Client URL did not match its reviewed launch context");
    }
  } catch (error) {
    const heading = await page.locator("main h1").first().textContent().catch(() => null);
    throw new Error(activityClientBootstrapDiagnostic({
      expectedRole,
      url: page.url(),
      heading,
      failures,
      cause: safeBrowserError(error),
    }));
  }
}

/**
 * Accepts only the two query shapes produced by the platform for the current
 * immutable Activity Client. The participant handoff itself must already be
 * scrubbed from the fragment before the authorization surface is accepted.
 */
export function isReviewedRenderedActivityClientUrl(value, expectedOrigin, expectedRole) {
  let url;
  try {
    url = new URL(value);
  } catch {
    return false;
  }
  if (
    url.origin !== expectedOrigin ||
    url.pathname !== RENDERED_AGENT_HEIST_CLIENT_PATH ||
    url.hash !== ""
  ) return false;
  const params = url.searchParams;
  if (expectedRole === "Navigator participant") {
    return params.size === 1 &&
      params.getAll("platform_return").length === 1 &&
      params.get("platform_return") === "/";
  }
  if (expectedRole !== "Spectator view") return false;
  const publicId = params.get("public_run");
  return typeof publicId === "string" && /^[0-9a-f]{32}$/u.test(publicId) &&
    params.size === 3 &&
    params.getAll("public_run").length === 1 &&
    params.getAll("platform_return").length === 1 &&
    params.getAll("platform_result").length === 1 &&
    params.get("platform_return") === "/" &&
    params.get("platform_result") === `/runs/${publicId}`;
}

/**
 * Keeps a failed rendered-client bootstrap debuggable without retaining a
 * handoff fragment, query capability, response body, or unbounded browser log.
 */
export function activityClientBootstrapDiagnostic({ expectedRole, url, heading, failures, cause }) {
  const observedFailures = Array.isArray(failures)
    ? failures.slice(0, MAX_BROWSER_FAILURES).map(safeBrowserDiagnosticText)
    : [];
  return [
    "independent Activity Client bootstrap did not become ready",
    `expected_role=${safeBrowserDiagnosticText(expectedRole)}`,
    `url=${safeBrowserUrl(url)}`,
    `heading=${safeBrowserDiagnosticText(heading ?? "<none>")}`,
    `cause=${safeBrowserDiagnosticText(cause)}`,
    `browser_failures=${observedFailures.length === 0 ? "<none>" : observedFailures.join(" | ")}`,
  ].join("; ");
}

/**
 * Browser diagnostics may identify only a failed same-origin endpoint. A
 * status is useful for cookie/session diagnosis; its response body is not.
 */
export function sameOriginBrowserResponseFailure(pageUrl, responseUrl, status) {
  if (!Number.isInteger(status) || status < 400 || status > 599) return null;
  try {
    const page = new URL(pageUrl);
    const response = new URL(responseUrl);
    if (page.origin !== response.origin) return null;
    // The platform session endpoint deliberately uses 401 to represent the
    // normal anonymous/guest state. The catalog probes it before local sign-in
    // and may probe it again while a page is mounting. It is not a browser
    // failure; an authenticated Activity Client still has to pass its own
    // bootstrap and handoff waits below. Keep all other same-origin failures
    // visible, including participant admission failures.
    if (status === 401 && response.pathname === "/api/auth/session") return null;
    return `response ${status}: ${safeBrowserUrl(responseUrl)}`;
  } catch {
    return null;
  }
}

function safeBrowserUrl(value) {
  try {
    const url = new URL(value);
    return `${url.origin}${url.pathname}`;
  } catch {
    return "<invalid-url>";
  }
}

function safeBrowserDiagnosticText(value) {
  return String(value)
    .replace(/(wsh1|wst1|wsb1):[0-9a-f]+/giu, "$1:[redacted]")
    .replace(/Bearer\s+[^\s]+/giu, "Bearer [redacted]")
    .replace(/[\r\n\t]+/gu, " ")
    .slice(0, MAX_BROWSER_DIAGNOSTIC_TEXT);
}

export async function ensureRenderedNavigatorRouteClaim(page, timeoutMs) {
  const existing = await renderedRouteClaim(page);
  if (existing !== null) return existing;
  await submitRenderedChoiceAction(
    page,
    "Open a dossier",
    "clue_id",
    "route",
    timeoutMs,
    async () => (await renderedRouteClaim(page)) !== null,
  );
  const routeClaim = await renderedRouteClaim(page, timeoutMs);
  if (routeClaim === null) throw new Error("rendered Open a dossier Action did not install an authorized Route claim");
  return routeClaim;
}

async function renderedRouteClaim(page, timeoutMs) {
  const clue = page.locator(".private-intel .known-clue").filter({
    has: page.getByText("Route", { exact: true }),
  });
  if (timeoutMs === undefined) {
    if (await clue.count() === 0) return null;
  } else {
    await clue.waitFor({ state: "attached", timeout: timeoutMs });
  }
  const routeClaim = canonicalRouteClaim(await clue.locator("strong").textContent());
  navigatorPlanForRouteClaim(routeClaim);
  return routeClaim;
}

async function ensureRenderedPublishedRouteClaim(page, routeClaim, timeoutMs) {
  if (await hasRenderedPublishedRouteClaim(page, routeClaim)) return;
  await submitRenderedChoiceAction(
    page,
    "Share intel",
    "clue_id",
    "route",
    timeoutMs,
    async () => hasRenderedPublishedRouteClaim(page, routeClaim),
  );
}

async function ensureRenderedNavigatorPlan(page, plan, timeoutMs) {
  if (await hasRenderedNavigatorPlan(page, plan)) return;
  await submitRenderedRecoverableAction(
    page,
    "Build a plan",
    timeoutMs,
    async (form, deadlineMs) => {
      for (const [label, value] of [
        ["Route", plan.route],
        ["When to enter", plan.entry_window],
        ["Equipment", plan.required_tool],
        ["Extraction", plan.extraction],
      ]) {
        await form.getByRole("tab").filter({ hasText: label }).click({
          timeout: remainingRenderedActionDeadlineMs(deadlineMs, Date.now()),
        });
        const choices = form.getByRole("radiogroup", { name: label, exact: true });
        await choices.locator('button[role="radio"]').filter({
          has: page.getByText(humanizeRenderedValue(value), { exact: true }),
        }).click({
          timeout: remainingRenderedActionDeadlineMs(deadlineMs, Date.now()),
        });
      }
    },
    async () => hasRenderedNavigatorPlan(page, plan),
  );
}

async function submitRenderedChoiceAction(page, actionLabel, name, value, timeoutMs, postcondition) {
  await submitRenderedRecoverableAction(
    page,
    actionLabel,
    timeoutMs,
    async (form, deadlineMs) => {
      const choice = form.locator(`select[name="${name}"]`);
      await choice.waitFor({
        state: "visible",
        timeout: remainingRenderedActionDeadlineMs(deadlineMs, Date.now()),
      });
      await choice.selectOption(value, {
        timeout: remainingRenderedActionDeadlineMs(deadlineMs, Date.now()),
      });
    },
    postcondition,
  );
}

/**
 * Concurrent House work may advance the Room through a private transition
 * without changing this participant's Projection. The authoritative stale
 * rejection consumes its Action ID. Retry only after the visible reconnect
 * installs a higher synchronized Room basis and a fresh form; every click
 * therefore creates a new Action ID from current reviewed choices.
 */
export async function submitRenderedRecoverableAction(
  page,
  actionLabel,
  timeoutMs,
  prepare,
  postcondition,
  { now = Date.now, wait = (delayMs) => new Promise((resolve) => setTimeout(resolve, delayMs)) } = {},
) {
  const deadlineMs = now() + timeoutMs;
  let attemptedRoomSequence = null;
  await retryRenderedStaleAction(
    async () => {
      if (await postcondition()) return "committed";
      const basis = await readRenderedMissionBasis(page, deadlineMs, now);
      attemptedRoomSequence = basis.roomSequence;
      const form = await waitForRenderedActionFormBefore(page, actionLabel, deadlineMs, now, wait);
      await prepare(form, deadlineMs);
      return submitRenderedForm(
        form,
        actionLabel,
        remainingRenderedActionDeadlineMs(deadlineMs, now()),
        async () => waitForRenderedActionOutcome(
          page,
          postcondition,
          deadlineMs,
          { now, wait },
        ),
        { deadlineMs, now },
      );
    },
    async () => reconnectRenderedAction(
      page,
      actionLabel,
      attemptedRoomSequence,
      deadlineMs,
      { now, wait },
    ),
    { deadlineMs, now },
  );
}

export async function waitForRenderedActionOutcome(
  page,
  postcondition,
  deadlineMs,
  { now = Date.now, wait = (delayMs) => new Promise((resolve) => setTimeout(resolve, delayMs)) } = {},
) {
  while (now() < deadlineMs) {
    if (await postcondition()) return "committed";
    if (await hasExplicitRenderedStaleRoom(page)) return "stale";
    await wait(Math.min(100, remainingRenderedActionDeadlineMs(deadlineMs, now())));
  }
  throw new Error("rendered Action did not produce an authorized Projection before its deadline");
}

async function ensureRenderedCurrentPlanCommitment(page, timeoutMs) {
  const formationDeadlineMs = Date.now() + timeoutMs;
  const initial = await readRenderedCommitmentSnapshot(page, formationDeadlineMs);
  if (initial.ownCommitmentCount === 1) {
    if (renderedCrewCommitmentCompleted(initial.ownCommitmentCount, initial.current)) return;
    if (renderedHouseCommitmentMissed(initial.basis.phase, initial.current)) {
      throw renderedHouseCommitmentDeadlineError(initial.current);
    }
    const deadlines = renderedCommitmentDeadlines(initial.basis.phaseDeadline);
    await waitForRenderedCrewCommitments(page, deadlines.observationDeadlineMs);
    return;
  }

  // Formation may legitimately spend most of Negotiation. Once the current
  // commitment offer renders, all retries share the Pack's one absolute
  // commitment deadline instead of multiplying the broad formation timeout.
  await waitForRenderedActionFormBefore(page, "Seal your choice", formationDeadlineMs);
  const commitmentWindow = (
    await readRenderedCommitmentSnapshot(page, formationDeadlineMs)
  ).basis;
  if (commitmentWindow.phase !== "commitment") {
    throw new Error("rendered Seal your choice Action was outside Commitment");
  }
  const deadlines = renderedCommitmentDeadlines(commitmentWindow.phaseDeadline);
  let attemptedRoomSequence = commitmentWindow.roomSequence;
  await retryRenderedStaleAction(
    async (actionDeadlineMs) => {
      const attempt = await submitRenderedCurrentPlanCommitmentAttempt(
        page,
        commitmentWindow,
        actionDeadlineMs,
        deadlines.observationDeadlineMs,
      );
      attemptedRoomSequence = attempt.roomSequence;
      return attempt.outcome;
    },
    async (actionDeadlineMs) => reconnectRenderedCommitment(
      page,
      commitmentWindow,
      attemptedRoomSequence,
      actionDeadlineMs,
    ),
    { deadlineMs: deadlines.actionDeadlineMs },
  );
  await waitForRenderedCrewCommitments(page, deadlines.observationDeadlineMs);
}

async function submitRenderedCurrentPlanCommitmentAttempt(
  page,
  commitmentWindow,
  actionDeadlineMs,
  observationDeadlineMs,
) {
  const basis = (
    await readRenderedCommitmentSnapshot(page, actionDeadlineMs)
  ).basis;
  if (!renderedSameCommitmentWindow(commitmentWindow, basis)) {
    throw new Error("rendered commitment window changed before the Navigator could act");
  }
  const form = await waitForRenderedActionFormBefore(
    page,
    "Seal your choice",
    actionDeadlineMs,
  );
  const plan = form.locator('select[name="selected_plan_id"]');
  await plan.waitFor({
    state: "visible",
    timeout: remainingRenderedDeadlineMs(actionDeadlineMs, Date.now()),
  });
  const planCount = await plan.locator('option:not([value=""])').count();
  if (planCount === 0) throw new Error("rendered current-plan control did not contain a reviewed plan");
  await plan.selectOption(
    { index: 1 },
    { timeout: remainingRenderedDeadlineMs(actionDeadlineMs, Date.now()) },
  );
  const resource = form.locator('input[name="contribute_required_resource"]');
  if (await resource.isChecked({
    timeout: remainingRenderedDeadlineMs(actionDeadlineMs, Date.now()),
  })) {
    await resource.uncheck({
      timeout: remainingRenderedDeadlineMs(actionDeadlineMs, Date.now()),
    });
  }
  const outcome = await submitRenderedForm(
    form,
    "Seal your choice",
    remainingRenderedDeadlineMs(actionDeadlineMs, Date.now()),
    async () => waitForRenderedCommitmentOutcome(page, observationDeadlineMs),
    { deadlineMs: actionDeadlineMs },
  );
  return Object.freeze({ outcome, roomSequence: basis.roomSequence });
}

/**
 * A stale-room rejection is the sole reviewed browser retry signal. The
 * runtime consumes the original Action ID, so each subsequent form submit
 * creates a new client Action ID only after a visible reconnect.
 */
export async function retryRenderedStaleAction(attempt, reconnect, {
  deadlineMs = null,
  now = Date.now,
} = {}) {
  for (let index = 0; index < MAX_RENDERED_STALE_ACTION_ATTEMPTS; index += 1) {
    if (deadlineMs !== null) remainingRenderedDeadlineMs(deadlineMs, now());
    const outcome = await attempt(deadlineMs ?? undefined);
    if (outcome === "committed") return;
    if (outcome !== "stale") {
      throw new Error("rendered Action returned an unreviewed retry outcome");
    }
    if (index === MAX_RENDERED_STALE_ACTION_ATTEMPTS - 1) break;
    if (deadlineMs !== null) remainingRenderedDeadlineMs(deadlineMs, now());
    await reconnect(deadlineMs ?? undefined);
  }
  throw new Error("rendered Action remained stale after five visible reconnect attempts");
}

async function waitForRenderedCommitmentOutcome(page, observationDeadlineMs) {
  while (Date.now() < observationDeadlineMs) {
    const snapshot = await readRenderedCommitmentSnapshot(page, observationDeadlineMs);
    if (snapshot.ownCommitmentCount === 1) return "committed";
    if (await hasExplicitRenderedStaleRoom(page)) return "stale";
    const remainingObservationMs = Math.ceil(observationDeadlineMs - Date.now());
    if (remainingObservationMs <= 0) break;
    await new Promise((resolve) => setTimeout(
      resolve,
      Math.min(250, remainingObservationMs),
    ));
  }
  throw new Error("rendered Seal your choice Action did not produce an authorized commitment");
}

export async function waitForRenderedCrewCommitments(page, observationDeadlineMs, {
  now = Date.now,
  wait = (delayMs) => new Promise((resolve) => setTimeout(resolve, delayMs)),
} = {}) {
  let lastObserved = null;
  while (now() < observationDeadlineMs) {
    let snapshot;
    try {
      snapshot = await readRenderedCommitmentSnapshot(
        page,
        observationDeadlineMs,
        now,
      );
    } catch (error) {
      if (now() >= observationDeadlineMs) break;
      throw error;
    }
    lastObserved = snapshot.current;
    if (renderedCrewCommitmentCompleted(
      snapshot.ownCommitmentCount,
      snapshot.current,
    )) return;
    if (renderedHouseCommitmentMissed(snapshot.basis.phase, snapshot.current)) {
      throw renderedHouseCommitmentDeadlineError(snapshot.current);
    }
    const remainingObservationMs = Math.ceil(observationDeadlineMs - now());
    if (remainingObservationMs <= 0) break;
    await wait(Math.min(250, remainingObservationMs));
  }
  throw renderedHouseCommitmentDeadlineError(lastObserved);
}

export async function hasExplicitRenderedStaleRoom(page) {
  const staleMessage = page.locator(".mission-connection").filter({
    has: page.getByText(RENDERED_STALE_ROOM_MESSAGE, { exact: true }),
  });
  const reconnect = page.getByRole("button", { name: "Reconnect", exact: true });
  return (await staleMessage.count()) > 0 && (await reconnect.count()) > 0;
}

export async function reconnectRenderedCommitment(
  page,
  commitmentWindow,
  rejectedRoomSequence,
  actionDeadlineMs,
  {
    now = Date.now,
    wait = (delayMs) => new Promise((resolve) => setTimeout(resolve, delayMs)),
  } = {},
) {
  const reconnect = page.getByRole("button", { name: "Reconnect", exact: true });
  await reconnect.waitFor({
    timeout: remainingRenderedDeadlineMs(actionDeadlineMs, now()),
  });
  await reconnect.click({
    timeout: remainingRenderedDeadlineMs(actionDeadlineMs, now()),
  });
  const form = renderedActionForm(page, "Seal your choice");
  const live = page.locator(".mission-connection").filter({
    has: page.getByText("Mission link live", { exact: true }),
  });
  while (now() < actionDeadlineMs) {
    const action = form.locator('button[type="submit"]');
    const basis = (
      await readRenderedCommitmentSnapshot(page, actionDeadlineMs, now)
    ).basis;
    if (!renderedSameCommitmentWindow(commitmentWindow, basis)) {
      throw new Error("rendered reconnect left the current commitment window");
    }
    if (
      renderedFreshCommitmentBasis(commitmentWindow, basis, rejectedRoomSequence)
      && await live.count() > 0
      && await form.count() > 0
      && await action.isEnabled({
        timeout: remainingRenderedDeadlineMs(actionDeadlineMs, now()),
      })
    ) return;
    await wait(Math.min(250, remainingRenderedDeadlineMs(actionDeadlineMs, now())));
  }
  throw new Error("rendered reconnect did not restore a current live Seal your choice Action");
}

export async function reconnectRenderedAction(
  page,
  actionLabel,
  rejectedRoomSequence,
  actionDeadlineMs,
  {
    now = Date.now,
    wait = (delayMs) => new Promise((resolve) => setTimeout(resolve, delayMs)),
  } = {},
) {
  if (!Number.isSafeInteger(rejectedRoomSequence) || rejectedRoomSequence < 0) {
    throw new Error("rendered stale Action has no exact rejected Room basis");
  }
  const reconnect = page.getByRole("button", { name: "Reconnect", exact: true });
  await reconnect.waitFor({
    timeout: remainingRenderedActionDeadlineMs(actionDeadlineMs, now()),
  });
  await reconnect.click({
    timeout: remainingRenderedActionDeadlineMs(actionDeadlineMs, now()),
  });
  const live = page.locator(".mission-connection").filter({
    has: page.getByText("Mission link live", { exact: true }),
  });
  while (now() < actionDeadlineMs) {
    const basis = await readRenderedMissionBasis(page, actionDeadlineMs, now);
    const form = renderedActionForm(page, actionLabel);
    if (
      basis.roomSequence > rejectedRoomSequence
      && await live.count() > 0
      && await form.count() > 0
    ) {
      const action = form.locator('button[type="submit"]');
      if (await action.isEnabled({
        timeout: remainingRenderedActionDeadlineMs(actionDeadlineMs, now()),
      })) return;
    }
    await wait(Math.min(100, remainingRenderedActionDeadlineMs(actionDeadlineMs, now())));
  }
  throw new Error(`rendered reconnect did not restore a fresh live ${actionLabel} Action`);
}

/**
 * Clicks a reviewed Action then proves its committed outcome from a durable
 * authorized Projection/result condition. A form-local status is not a
 * receipt: a successful projection can consume and unmount that form before
 * React resolves the Action promise.
 */
export async function submitRenderedForm(
  form,
  actionLabel,
  timeoutMs,
  postcondition,
  { deadlineMs = null, now = Date.now } = {},
) {
  if (typeof postcondition !== "function") {
    throw new Error(`rendered ${actionLabel} Action lacks a durable postcondition`);
  }
  const action = form.locator('button[type="submit"]');
  const operationDeadlineMs = deadlineMs ?? now() + timeoutMs;
  await action.waitFor({
    state: "visible",
    timeout: deadlineMs === null
      ? timeoutMs
      : remainingRenderedDeadlineMs(operationDeadlineMs, now()),
  });
  let enabled = await action.isEnabled(deadlineMs === null ? undefined : {
    timeout: remainingRenderedDeadlineMs(operationDeadlineMs, now()),
  });
  while (!enabled && now() < operationDeadlineMs) {
    await new Promise((resolve) => setTimeout(resolve, 50));
    enabled = await action.isEnabled(deadlineMs === null ? undefined : {
      timeout: remainingRenderedDeadlineMs(operationDeadlineMs, now()),
    });
  }
  if (!enabled) throw new Error(`rendered ${actionLabel} Action was not enabled after live admission`);
  await action.click(deadlineMs === null ? undefined : {
    timeout: remainingRenderedDeadlineMs(operationDeadlineMs, now()),
  });
  try {
    return await postcondition();
  } catch {
    // Do not trust an ephemeral form-local receipt, or leak a locator's DOM
    // snapshot into retained acceptance output. The browser failure collector
    // still retains bounded endpoint/console diagnostics for this boundary.
    throw new Error(`rendered ${actionLabel} Action did not produce its durable authorized Projection/result postcondition`);
  }
}

export function renderedParticipantActionForms(page) {
  return page.locator('#mission-action form.mission-action-surface');
}

export function renderedActionForm(page, actionLabel) {
  return page.locator("section#mission-action").filter({
    has: page.getByRole("heading", { name: actionLabel, exact: true }),
  }).locator("form.mission-action-surface");
}

async function waitForRenderedActionForm(page, actionLabel, timeoutMs) {
  return waitForRenderedActionFormBefore(
    page,
    actionLabel,
    Date.now() + timeoutMs,
  );
}

async function waitForRenderedActionFormBefore(
  page,
  actionLabel,
  deadlineMs,
  now = Date.now,
  wait = (delayMs) => new Promise((resolve) => setTimeout(resolve, delayMs)),
) {
  const form = renderedActionForm(page, actionLabel);
  const switcher = page.locator("nav.mission-moves").getByRole("button", {
    name: actionLabel,
    exact: true,
  });
  while (now() < deadlineMs) {
    if (await form.count() > 0) {
      await form.waitFor({
        state: "visible",
        timeout: remainingRenderedActionDeadlineMs(deadlineMs, now()),
      });
      return form;
    }
    if (await switcher.count() > 0) {
      await switcher.click({
        timeout: remainingRenderedActionDeadlineMs(deadlineMs, now()),
      });
    }
    await wait(Math.min(100, remainingRenderedActionDeadlineMs(deadlineMs, now())));
  }
  throw new Error(`rendered ${actionLabel} Action did not become available before the bounded deadline`);
}

async function hasRenderedPublishedRouteClaim(page, routeClaim) {
  return (await renderedPublishedRouteClaim(page, routeClaim).count()) > 0;
}

async function waitForRenderedPublishedRouteClaim(page, routeClaim, timeoutMs) {
  await renderedPublishedRouteClaim(page, routeClaim).waitFor({ state: "attached", timeout: timeoutMs });
}

function renderedPublishedRouteClaim(page, routeClaim) {
  return page.locator(".crew-board-content section").filter({
    has: page.getByText("Shared clues", { exact: true }),
  }).locator("li").filter({
    has: page.getByText("Route", { exact: true }),
  }).filter({
    hasText: humanizeRenderedValue(routeClaim),
  });
}

async function hasRenderedNavigatorPlan(page, plan) {
  return (await renderedNavigatorPlan(page, plan).count()) > 0;
}

async function waitForRenderedNavigatorPlan(page, plan, timeoutMs) {
  await renderedNavigatorPlan(page, plan).waitFor({ state: "attached", timeout: timeoutMs });
}

export function renderedNavigatorPlan(page, plan) {
  return page.locator(".crew-plan-list article").filter({
    has: page.getByText(/^Plan [1-9][0-9]* · Navigator$/u),
  }).filter({
    hasText: `${humanizeRenderedValue(plan.route)} · ${humanizeRenderedValue(plan.entry_window)}`,
  }).filter({
    hasText: `${humanizeRenderedValue(plan.required_tool)} → ${humanizeRenderedValue(plan.extraction)}`,
  });
}

function renderedCompletePhase(page) {
  return page.locator(".mission-action .no-action").filter({
    hasText: "This operation is complete.",
  });
}

export async function waitForRenderedTerminalComplete(page, timeoutMs) {
  await renderedCompletePhase(page).waitFor({ state: "visible", timeout: timeoutMs });
}

async function readRenderedCommitmentSnapshot(page, deadlineMs, now = Date.now) {
  const shell = page.locator("main.mission-focus-shell");
  const rendered = await shell.evaluate((element) => ({
    roomSequence: element.getAttribute("data-room-sequence"),
    phase: element.getAttribute("data-phase"),
    phaseGeneration: element.getAttribute("data-phase-generation"),
    phaseDeadline: element.getAttribute("data-phase-deadline"),
    commitmentCount: element.querySelector(".crew-commitments strong")?.textContent ?? null,
    ownCommitmentCount: element.querySelectorAll(".own-commitment").length,
  }), undefined, {
    timeout: remainingRenderedDeadlineMs(deadlineMs, now()),
  });
  const basis = parseRenderedMissionBasis(rendered);
  const current = parseRenderedCommitmentCount(rendered.commitmentCount);
  if (
    !Number.isSafeInteger(rendered.ownCommitmentCount)
    || rendered.ownCommitmentCount < 0
    || rendered.ownCommitmentCount > 1
  ) {
    throw new Error("rendered participant commitment receipt was ambiguous");
  }
  return Object.freeze({
    basis,
    current,
    ownCommitmentCount: rendered.ownCommitmentCount,
  });
}

async function readRenderedMissionBasis(page, deadlineMs, now = Date.now) {
  const shell = page.locator("main.mission-focus-shell");
  const rendered = await shell.evaluate((element) => ({
    roomSequence: element.getAttribute("data-room-sequence"),
    phase: element.getAttribute("data-phase"),
    phaseGeneration: element.getAttribute("data-phase-generation"),
    phaseDeadline: element.getAttribute("data-phase-deadline"),
  }), undefined, {
    timeout: remainingRenderedActionDeadlineMs(deadlineMs, now()),
  });
  return parseRenderedMissionBasis(rendered);
}

export function parseRenderedMissionBasis({
  roomSequence,
  phase,
  phaseGeneration,
  phaseDeadline,
}) {
  const parsedRoomSequence = parseRenderedNonnegativeInteger(roomSequence);
  const parsedPhaseGeneration = parseRenderedNonnegativeInteger(phaseGeneration);
  const parsedPhaseDeadline = typeof phaseDeadline === "string" && phaseDeadline.length > 0
    ? phaseDeadline
    : null;
  if (
    parsedRoomSequence === null
    || parsedPhaseGeneration === null
    || typeof phase !== "string"
    || !/^[a-z][a-z_]*$/u.test(phase)
    || (parsedPhaseDeadline !== null && !Number.isFinite(Date.parse(parsedPhaseDeadline)))
  ) {
    throw new Error("rendered mission synchronization basis is invalid");
  }
  return Object.freeze({
    roomSequence: parsedRoomSequence,
    phase,
    phaseGeneration: parsedPhaseGeneration,
    phaseDeadline: parsedPhaseDeadline,
  });
}

function parseRenderedNonnegativeInteger(value) {
  if (typeof value !== "string" || !/^(?:0|[1-9][0-9]*)$/u.test(value)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

export function renderedCommitmentDeadlines(phaseDeadline, nowMs = Date.now()) {
  const authoritativeDeadlineMs = typeof phaseDeadline === "string"
    ? Date.parse(phaseDeadline)
    : Number.NaN;
  if (!Number.isFinite(authoritativeDeadlineMs) || !Number.isFinite(nowMs)) {
    throw new Error("rendered commitment deadline is invalid");
  }
  const actionDeadlineMs = Math.min(
    authoritativeDeadlineMs,
    nowMs + RENDERED_COMMITMENT_ACTION_WINDOW_MS,
  );
  if (actionDeadlineMs <= nowMs) {
    throw new Error("rendered commitment window closed before the Navigator could act");
  }
  return Object.freeze({
    actionDeadlineMs,
    observationDeadlineMs: actionDeadlineMs + RENDERED_COMMITMENT_OBSERVATION_GRACE_MS,
  });
}

export function remainingRenderedDeadlineMs(deadlineMs, nowMs) {
  return remainingRenderedDeadline(deadlineMs, nowMs, "commitment window");
}

export function remainingRenderedActionDeadlineMs(deadlineMs, nowMs) {
  return remainingRenderedDeadline(deadlineMs, nowMs, "Action deadline");
}

function remainingRenderedDeadline(deadlineMs, nowMs, label) {
  const remaining = Math.ceil(deadlineMs - nowMs);
  if (!Number.isSafeInteger(remaining) || remaining <= 0) {
    throw new Error(`rendered ${label} closed before the reviewed action completed`);
  }
  return remaining;
}

export function renderedSameCommitmentWindow(expected, current) {
  return expected.phase === "commitment"
    && current.phase === expected.phase
    && current.phaseGeneration === expected.phaseGeneration
    && current.phaseDeadline === expected.phaseDeadline;
}

export function renderedFreshCommitmentBasis(expected, current, rejectedRoomSequence) {
  return renderedSameCommitmentWindow(expected, current)
    && Number.isSafeInteger(rejectedRoomSequence)
    && current.roomSequence > rejectedRoomSequence;
}

export function parseRenderedCommitmentCount(value) {
  const match = typeof value === "string" ? value.trim().match(/^([0-9]+) of ([1-9][0-9]*)$/u) : null;
  const current = match === null ? Number.NaN : Number(match[1]);
  const total = match === null ? Number.NaN : Number(match[2]);
  if (!Number.isSafeInteger(current) || !Number.isSafeInteger(total) || current > total) {
    throw new Error("rendered commitment count is invalid");
  }
  return Object.freeze({ current, total });
}

export function renderedCrewCommitmentCompleted(ownCommitmentCount, current) {
  return ownCommitmentCount === 1
    && current.total >= 2
    && current.current === current.total;
}

export function renderedHouseCommitmentMissed(phase, current) {
  return ["resolution", "result", "complete"].includes(phase)
    && current.current < current.total;
}

function renderedHouseCommitmentDeadlineError(current) {
  const detail = current === null ? "" : ` (${current.current} of ${current.total} sealed)`;
  return new Error(
    `rendered House seats missed the 30-second commitment deadline${detail}`,
  );
}

export async function hasRenderedOwnCommitment(page) {
  const count = await page.locator(".own-commitment").count();
  if (count > 1) throw new Error("rendered participant commitment receipt was ambiguous");
  return count === 1;
}

function humanizeRenderedValue(value) {
  const replacement = { service: "Service entrance", roof: "Rooftop" }[value];
  return replacement ?? value.replaceAll("_", " ").replace(/\b\w/gu, (letter) => letter.toUpperCase());
}

function canonicalRouteClaim(value) {
  const normalized = typeof value === "string"
    ? value.trim().toLowerCase().replaceAll(" ", "_")
    : "";
  if (!/^route_(?:canal|service|roof)$/u.test(normalized)) {
    throw new Error("rendered Navigator Route claim is invalid");
  }
  return normalized;
}

export async function waitForHouseEndorsement(page, plan, timeoutMs) {
  const endorsements = renderedNavigatorPlan(page, plan).locator("small");
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const values = await endorsements.allTextContents();
    if (hasRenderedHouseEndorsement(values)) return;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("reviewed House Agent did not endorse the rendered Navigator plan before the bounded deadline");
}

export function hasRenderedHouseEndorsement(values) {
  return values.some((value) => /^[1-9][0-9]* backed · [0-9]+ flagged$/u.test(value.trim()));
}

/**
 * The current Activity Pack intentionally gives a human Navigator a small
 * deterministic strategy choice.  The browser reads the authorized private
 * Route claim and fills the visible fields; it never reads a raw projection or
 * invents a plan identifier.
 */
export function navigatorPlanForRouteClaim(routeClaim) {
  const route = typeof routeClaim === "string" ? routeClaim.replace(/^route_/u, "") : "";
  const completion = new Map([
    ["canal", ["late", "disguise", "van"]],
    ["service", ["early", "thermal_key", "boat"]],
    ["roof", ["middle", "jammer", "motorbike"]],
  ]).get(route);
  if (completion === undefined) throw new Error("rendered Navigator Route claim has no reviewed completion plan");
  return Object.freeze({
    route,
    entry_window: completion[0],
    required_tool: completion[1],
    extraction: completion[2],
  });
}
