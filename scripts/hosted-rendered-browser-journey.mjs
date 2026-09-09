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
    const routeClaim = await submitRenderedNavigatorAction(participant, timeouts.action);
    await submitRenderedAction(participant, "Publish clue", {
      clue_id: "route",
      claim_code: routeClaim,
    }, timeouts.action);
    await submitRenderedAction(
      participant,
      "Propose plan",
      navigatorPlanForRouteClaim(routeClaim),
      timeouts.action,
    );
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
    await submitCurrentPlanCommitment(participant, timeouts.formation);
    await submitRenderedAction(participant, "Acknowledge result", {}, timeouts.action);

    // The platform's result index is deliberately asynchronous.  The visible
    // My Games page is the acceptance boundary, not an out-of-band result
    // poll, and it must lead to the anonymous-safe public result surface.
    await participant.getByRole("button", { name: "Back to games" }).click();
    await participant.waitForURL(`${origin}/`, { timeout: timeouts.action });
    await participant.getByRole("button", { name: "My games" }).click();
    await participant.getByRole("heading", { name: "My games" }).waitFor({ timeout: timeouts.action });
    await participant.getByRole("button", { name: "View result" }).waitFor({
      timeout: timeouts.formation,
    });
    await participant.getByRole("button", { name: "View result" }).click();
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
    if (message.type() === "error") record(`${label} console error: ${message.text()}`);
  });
  page.on("requestfailed", (request) => record(`${label} request failed: ${safeBrowserUrl(request.url())}`));
  page.on("response", (response) => {
    const failure = sameOriginBrowserResponseFailure(page.url(), response.url(), response.status());
    if (failure !== null) record(`${label} ${failure}`);
  });
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

async function submitRenderedNavigatorAction(page, timeoutMs) {
  await submitRenderedAction(page, "Inspect clue", { clue_id: "route" }, timeoutMs);
  const clue = page.locator(".private-clue-list article").filter({
    has: page.getByText("Route", { exact: true }),
  });
  await clue.waitFor({ timeout: timeoutMs });
  const routeClaim = await clue.locator("code").innerText();
  navigatorPlanForRouteClaim(routeClaim);
  return routeClaim;
}

async function submitRenderedAction(page, actionLabel, fields, timeoutMs) {
  const form = page.locator("form.live-action-form").filter({ hasText: actionLabel });
  await form.waitFor({ timeout: timeoutMs });
  for (const [name, value] of Object.entries(fields)) {
    await form.locator(`input[name="${name}"]`).fill(value);
  }
  await submitRenderedForm(form, actionLabel, timeoutMs);
}

async function submitCurrentPlanCommitment(page, timeoutMs) {
  const form = page.locator("form.live-action-form").filter({ hasText: "Commit move" });
  await form.waitFor({ timeout: timeoutMs });
  const plan = form.locator('select[name="selected_plan_id"]');
  await plan.waitFor({ state: "visible", timeout: timeoutMs });
  const planCount = await plan.locator('option:not([value=""])').count();
  if (planCount === 0) throw new Error("rendered current-plan control did not contain a reviewed plan");
  await plan.selectOption({ index: 1 });
  const resource = form.locator('input[name="contribute_required_resource"]');
  if (await resource.isChecked()) await resource.uncheck();
  await submitRenderedForm(form, "Commit move", timeoutMs);
}

async function submitRenderedForm(form, actionLabel, timeoutMs) {
  const action = form.locator('button[type="submit"]');
  await action.waitFor({ state: "visible", timeout: timeoutMs });
  if (!(await action.isEnabled())) throw new Error(`rendered ${actionLabel} Action was not enabled after live admission`);
  await action.click();
  await form.getByRole("status").getByText("Submitted. Waiting for committed Room output.", {
    exact: true,
  }).waitFor({ timeout: timeoutMs });
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
