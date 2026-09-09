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
const RENDERED_STALE_ROOM_MESSAGE = "The Room advanced. Reconnect to synchronize before acting.";

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
    const participant = await participantContext.newPage();
    collectBrowserFailures(participant, failures, "participant");

    await participant.goto(origin, { waitUntil: "domcontentloaded" });
    await participant.getByRole("heading", { name: /Pick your world/i }).waitFor({
      timeout: timeouts.action,
    });
    await participant.getByText("Sign in to start", { exact: true }).waitFor({
      timeout: timeouts.action,
    });

    // Catalog and identity are intentionally visible.  The only local
    // substitute is named on the button; production OAuth is never simulated.
    await participant.getByRole("button", { name: /Enter activity/i }).click();
    await participant.getByRole("dialog").waitFor({ timeout: timeouts.action });
    await participant.getByRole("button", {
      name: "Use visible local-development sign-in",
    }).click();
    await participant.getByText("Signed in", { exact: true }).waitFor({
      timeout: timeouts.action,
    });

    let launchId;
    let runCapture;
    if (retainedLaunchId === null) {
      const launchDialog = participant.getByRole("dialog");
      await assertChecked(launchDialog.locator('input[name="seat"][value="seat-1"]'), "Navigator seat");
      await assertChecked(launchDialog.locator('input[name="fill"]:checked'), "House Agent fill mode");
      await launchDialog.getByRole("button", { name: "Create waiting room" }).click();
      await participant.waitForURL(/\/launches\/[0-9a-f-]{36}\/?$/u, { timeout: timeouts.action });
      launchId = launchIdFromUrl(participant.url());
      runCapture = capturePublicRunId(participant, origin, launchId);

      await participant.getByRole("heading", { name: "Gather your crew" }).waitFor({
        timeout: timeouts.action,
      });
      // Issuing an invite proves the invite/roster surface without leaking its
      // opaque capability into evidence or attempting an unsupported local
      // multi-account impersonation.
      await participant.getByRole("button", { name: "Copy invite" }).first().click();
      const invitation = participant.locator('input[aria-label$="invitation URL"]');
      await invitation.first().waitFor({ timeout: timeouts.action });
      const invitationUrl = await invitation.first().inputValue();
      if (!/^http:\/\/(?:127\.0\.0\.1|localhost|\[::1\]):[0-9]{1,5}\/join#invite=[0-9a-f]{64}$/u.test(invitationUrl)) {
        throw new Error("rendered waiting-room invitation did not have the reviewed local shape");
      }
      await participant.getByRole("heading", { name: "Room roster" }).waitFor({ timeout: timeouts.action });

      await participant.getByRole("button", { name: "Start activity" }).click();
      await participant.getByRole("heading", { name: "Your activity is ready" }).waitFor({
        timeout: timeouts.formation,
      });
    } else {
      // A prior local acceptance can be interrupted after Genesis. Re-enter
      // that exact owner-authorized Launch instead of creating another live
      // Run or releasing the retained one without terminal evidence.
      launchId = retainedLaunchId;
      runCapture = capturePublicRunId(participant, origin, launchId);
      await participant.goto(`${origin}/launches/${launchId}`, { waitUntil: "domcontentloaded" });
      await participant.getByRole("heading", { name: "Your activity is ready" }).waitFor({
        timeout: timeouts.formation,
      });
    }
    const runIdentity = await runCapture.wait(timeouts.action);
    const publicId = runIdentity.publicId;

    await participant.getByRole("button", { name: /^Enter Navigator$/ }).click();
    await waitForIndependentActivityClient(participant, timeouts.action, "Navigator participant", failures);
    const routeClaim = await ensureRenderedNavigatorRouteClaim(participant, timeouts.action);
    await ensureRenderedPublishedRouteClaim(participant, routeClaim, timeouts.action);
    const navigatorPlan = navigatorPlanForRouteClaim(routeClaim);
    await ensureRenderedNavigatorPlan(participant, navigatorPlan, timeouts.action);
    await waitForHouseEndorsement(participant, timeouts.formation);

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
    await waitForIndependentActivityClient(participant, timeouts.action, "Navigator participant", failures);

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
    await waitForIndependentActivityClient(spectator, timeouts.action, "Spectator view", failures);
    const participantActionForms = await spectator.locator("form.live-action-form").count();
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
    await ensureRenderedResultAcknowledgement(participant, timeouts.action);

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
    );
    await renderedCompletePhase(participant).waitFor({ timeout: timeouts.formation });

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

async function waitForIndependentActivityClient(page, timeoutMs, expectedRole, failures) {
  try {
    await page.waitForURL(/\/agent-heist-v[0-9]+\/hosted\//u, { timeout: timeoutMs });
    await page.getByRole("heading", { name: "Agent Heist" }).waitFor({ timeout: timeoutMs });
    await page.getByText(expectedRole, { exact: true }).waitFor({ timeout: timeoutMs });
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
  await submitRenderedAction(page, "Inspect clue", { clue_id: "route" }, timeoutMs, async () => {
    await renderedRouteClaim(page, timeoutMs);
  });
  const routeClaim = await renderedRouteClaim(page, timeoutMs);
  if (routeClaim === null) throw new Error("rendered Inspect clue Action did not install an authorized Route claim");
  return routeClaim;
}

async function renderedRouteClaim(page, timeoutMs) {
  const clue = page.locator(".private-clue-list article").filter({
    has: page.getByText("route", { exact: true }),
  });
  if (timeoutMs === undefined) {
    if (await clue.count() === 0) return null;
  } else {
    await clue.waitFor({ timeout: timeoutMs });
  }
  const routeClaim = await clue.locator("code").innerText();
  navigatorPlanForRouteClaim(routeClaim);
  return routeClaim;
}

async function ensureRenderedPublishedRouteClaim(page, routeClaim, timeoutMs) {
  if (await hasRenderedPublishedRouteClaim(page, routeClaim)) return;
  await submitRenderedAction(page, "Publish clue", {
    clue_id: "route",
    claim_code: routeClaim,
  }, timeoutMs, async () => {
    await waitForRenderedPublishedRouteClaim(page, routeClaim, timeoutMs);
  });
}

async function ensureRenderedNavigatorPlan(page, plan, timeoutMs) {
  if (await hasRenderedNavigatorPlan(page, plan)) return;
  await submitRenderedAction(page, "Propose plan", plan, timeoutMs, async () => {
    await waitForRenderedNavigatorPlan(page, plan, timeoutMs);
  });
}

async function submitRenderedAction(page, actionLabel, fields, timeoutMs, postcondition) {
  const form = page.locator("form.live-action-form").filter({ hasText: actionLabel });
  await form.waitFor({ timeout: timeoutMs });
  for (const [name, value] of Object.entries(fields)) {
    await form.locator(`input[name="${name}"]`).fill(value);
  }
  await submitRenderedForm(form, actionLabel, timeoutMs, postcondition);
}

async function ensureRenderedCurrentPlanCommitment(page, timeoutMs) {
  if (await hasRenderedOwnCommitment(page)) return;
  await retryRenderedStaleAction(
    () => submitRenderedCurrentPlanCommitmentAttempt(page, timeoutMs),
    async () => reconnectRenderedCommitment(page, timeoutMs),
  );
}

async function submitRenderedCurrentPlanCommitmentAttempt(page, timeoutMs) {
  const form = page.locator("form.live-action-form").filter({ hasText: "Commit move" });
  await form.waitFor({ timeout: timeoutMs });
  const plan = form.locator('select[name="selected_plan_id"]');
  await plan.waitFor({ state: "visible", timeout: timeoutMs });
  const planCount = await plan.locator('option:not([value=""])').count();
  if (planCount === 0) throw new Error("rendered current-plan control did not contain a reviewed plan");
  await plan.selectOption({ index: 1 });
  const selectedPlanId = await plan.inputValue();
  const resource = form.locator('input[name="contribute_required_resource"]');
  if (await resource.isChecked()) await resource.uncheck();
  return submitRenderedForm(form, "Commit move", timeoutMs, async () => {
    return waitForRenderedCommitmentOutcome(page, selectedPlanId, timeoutMs);
  });
}

/**
 * A stale-room rejection is the sole reviewed browser retry signal. The
 * runtime consumes the original Action ID, so each subsequent form submit
 * creates a new client Action ID only after a visible reconnect.
 */
export async function retryRenderedStaleAction(attempt, reconnect) {
  for (let index = 0; index < MAX_RENDERED_STALE_ACTION_ATTEMPTS; index += 1) {
    const outcome = await attempt();
    if (outcome === "committed") return;
    if (outcome !== "stale") {
      throw new Error("rendered Action returned an unreviewed retry outcome");
    }
    if (index === MAX_RENDERED_STALE_ACTION_ATTEMPTS - 1) break;
    await reconnect();
  }
  throw new Error("rendered Action remained stale after five visible reconnect attempts");
}

async function waitForRenderedCommitmentOutcome(page, selectedPlanId, timeoutMs) {
  const commitment = renderedOwnCommitment(page, selectedPlanId);
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await commitment.count() > 0) return "committed";
    if (await hasExplicitRenderedStaleRoom(page)) return "stale";
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("rendered Commit move Action did not produce an authorized commitment");
}

export async function hasExplicitRenderedStaleRoom(page) {
  const staleMessage = page.locator(".live-client-notice").filter({
    has: page.getByText(RENDERED_STALE_ROOM_MESSAGE, { exact: true }),
  });
  const reconnect = page.getByRole("button", { name: "Reconnect", exact: true });
  return (await staleMessage.count()) > 0 && (await reconnect.count()) > 0;
}

async function reconnectRenderedCommitment(page, timeoutMs) {
  const reconnect = page.getByRole("button", { name: "Reconnect", exact: true });
  await reconnect.waitFor({ timeout: timeoutMs });
  await reconnect.click();
  const form = page.locator("form.live-action-form").filter({ hasText: "Commit move" });
  const live = page.locator(".play-state").filter({ has: page.getByText("Live", { exact: true }) });
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const action = form.locator('button[type="submit"]');
    if (await live.count() > 0 && await form.count() > 0 && await action.isEnabled()) return;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("rendered reconnect did not restore a current live Commit move Action");
}

async function ensureRenderedResultAcknowledgement(page, timeoutMs) {
  const acknowledgement = await waitForRenderedResultAcknowledgementOpportunity(page, timeoutMs);
  if (acknowledgement === null) return;
  await submitRenderedForm(acknowledgement, "Acknowledge result", timeoutMs, async () => {
    // Acknowledgement has no private receipt field. The disappearance of its
    // exact offer from the next authorized Projection is the reviewed durable
    // acknowledgement boundary. It is deliberately queried from the page,
    // never from the form that the Projection may have unmounted.
    await acknowledgement.waitFor({ state: "hidden", timeout: timeoutMs });
  });
}

/**
 * The reviewed House seats can advance Resolution after the Navigator commits.
 * Do not mistake that short interval, when the acknowledgement offer has not
 * arrived, for a prior acknowledgement. Completion is the only authorized
 * state that may legitimately omit this participant's offer.
 */
export async function waitForRenderedResultAcknowledgementOpportunity(page, timeoutMs) {
  const acknowledgement = renderedActionForm(page, "Acknowledge result");
  const complete = renderedCompletePhase(page);
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await acknowledgement.count() > 0) return acknowledgement;
    if (await complete.count() > 0) return null;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("rendered result did not offer Acknowledge result or reach complete before the bounded deadline");
}

/**
 * Clicks a reviewed Action then proves its committed outcome from a durable
 * authorized Projection/result condition. A form-local status is not a
 * receipt: a successful projection can consume and unmount that form before
 * React resolves the Action promise.
 */
export async function submitRenderedForm(form, actionLabel, timeoutMs, postcondition) {
  if (typeof postcondition !== "function") {
    throw new Error(`rendered ${actionLabel} Action lacks a durable postcondition`);
  }
  const action = form.locator('button[type="submit"]');
  await action.waitFor({ state: "visible", timeout: timeoutMs });
  if (!(await action.isEnabled())) throw new Error(`rendered ${actionLabel} Action was not enabled after live admission`);
  await action.click();
  try {
    return await postcondition();
  } catch {
    // Do not trust an ephemeral form-local receipt, or leak a locator's DOM
    // snapshot into retained acceptance output. The browser failure collector
    // still retains bounded endpoint/console diagnostics for this boundary.
    throw new Error(`rendered ${actionLabel} Action did not produce its durable authorized Projection/result postcondition`);
  }
}

function renderedActionForm(page, actionLabel) {
  return page.locator("form.live-action-form").filter({ hasText: actionLabel });
}

async function hasRenderedPublishedRouteClaim(page, routeClaim) {
  return (await renderedPublishedRouteClaim(page, routeClaim).count()) > 0;
}

async function waitForRenderedPublishedRouteClaim(page, routeClaim, timeoutMs) {
  await renderedPublishedRouteClaim(page, routeClaim).waitFor({ timeout: timeoutMs });
}

function renderedPublishedRouteClaim(page, routeClaim) {
  return page.locator(".live-board-grid section").filter({
    has: page.getByRole("heading", { name: "Clue board", exact: true }),
  }).locator("li").filter({
    has: page.getByText("route", { exact: true }),
  }).filter({
    has: page.getByText(humanizeRenderedValue(routeClaim), { exact: true }),
  });
}

async function hasRenderedNavigatorPlan(page, plan) {
  return (await renderedNavigatorPlan(page, plan).count()) > 0;
}

async function waitForRenderedNavigatorPlan(page, plan, timeoutMs) {
  await renderedNavigatorPlan(page, plan).waitFor({ timeout: timeoutMs });
}

export function renderedNavigatorPlan(page, plan) {
  return page.locator(".plan-grid article").filter({
    has: page.getByText("navigator", { exact: true }),
  }).filter({
    has: page.getByText(`${humanizeRenderedValue(plan.route)} · ${humanizeRenderedValue(plan.entry_window)}`, {
      exact: true,
    }),
  }).filter({
    has: page.getByText(`${humanizeRenderedValue(plan.required_tool)} → ${humanizeRenderedValue(plan.extraction)}`, {
      exact: true,
    }),
  });
}

function renderedCompletePhase(page) {
  return page.locator(".phase-window").filter({
    has: page.getByText("Current phase", { exact: true }),
  }).filter({
    has: page.getByText("Complete", { exact: true }),
  });
}

async function hasRenderedOwnCommitment(page) {
  return (await page.locator(".own-commitment").count()) > 0;
}

function renderedOwnCommitment(page, selectedPlanId) {
  return page.locator(".own-commitment").filter({
    has: page.getByText("Your sealed commitment", { exact: true }),
  }).filter({
    has: page.getByText(humanizeRenderedValue(selectedPlanId), { exact: true }),
  }).filter({
    has: page.getByText("No resource committed", { exact: true }),
  });
}

function humanizeRenderedValue(value) {
  return value.replaceAll("_", " ");
}

async function waitForHouseEndorsement(page, timeoutMs) {
  const endorsements = page.locator(".plan-grid article footer span").filter({ hasText: "endorsements" });
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const values = await endorsements.allTextContents();
    if (values.some((value) => /^[1-9][0-9]* endorsements$/u.test(value.trim()))) return;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("reviewed House Agent did not endorse the rendered Navigator plan before the bounded deadline");
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
