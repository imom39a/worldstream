import { chromium } from "playwright";
import { access } from "node:fs/promises";
import { join } from "node:path";

import { startActivityClientHost } from "./serve-activity-clients.mjs";

const host = await startActivityClientHost({ port: 0 });
const browser = await chromium.launch({ headless: true, ...await localBrowserOptions() });

try {
  await verifyInitialTicketRecovery();
  if ((await fetch(`${host.origin}/agent-heist/`)).status !== 404) throw new Error("unavailable retained Heist path must not serve replacement bytes");
  await verifySurface("Agent Heist current local", "/agent-heist-v11/", "Agent Heist · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Re-enter your mission" }).waitFor();
    const body = await page.locator("body").innerText();
    reject(body, /Canal Shift|route_service|Recorded fixture|Fixture mode/i, "Heist live client exposed recorded data");
  });
  await verifySurface("Agent Heist current hosted", "/agent-heist-v11/hosted/", "Agent Heist · Hosted Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Unable to enter this Run" }).waitFor();
  }, true);
  await verifySurface("Agent Heist retained v10", "/agent-heist-v10/", "Agent Heist · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Agent Heist retained v8", "/agent-heist-v8/", "Agent Heist · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
    const body = await page.locator("body").innerText();
    reject(body, /Canal Shift|route_service|Recorded fixture|Fixture mode/i, "Heist live client exposed recorded data");
  });
  await verifySurface("Agent Heist retained v5", "/agent-heist-v5/hosted/", "Agent Heist · Hosted Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Unable to enter this Run" }).waitFor();
  }, true);
  await verifySurface("Agent Heist retained v3", "/agent-heist-v3/hosted/", "Agent Heist · Hosted Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Unable to enter this Run" }).waitFor();
  }, true);
  await verifySurface("Negotiate 0.1", "/negotiate/", "Negotiate · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
    const body = await page.locator("body").innerText();
    reject(body, /fixture|sample offer|demo negotiation/i, "Negotiate 0.1 live client exposed recorded data");
  });
  await verifySurface("Negotiate 0.2", "/negotiate-v3/", "Negotiate · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
    const body = await page.locator("body").innerText();
    reject(body, /fixture|sample offer|demo negotiation/i, "Negotiate 0.2 live client exposed recorded data");
  });
  await verifySurface("Midnight Archive current", "/midnight-archive-v14/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
    const body = await page.locator("body").innerText();
    reject(body, /authentic_candidate_id|is_authentic|truth_marker|fixture/i, "Midnight Archive live client exposed private or recorded data");
  });
  await verifySurface("Midnight Archive retained v13", "/midnight-archive-v13/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v12", "/midnight-archive-v12/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v11", "/midnight-archive-v11/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v10", "/midnight-archive-v10/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v9", "/midnight-archive-v9/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v8", "/midnight-archive-v8/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v7", "/midnight-archive-v7/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v6", "/midnight-archive-v6/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v5", "/midnight-archive-v5/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v4", "/midnight-archive-v4/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v3", "/midnight-archive-v3/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v2", "/midnight-archive-v2/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Midnight Archive retained v1", "/midnight-archive-v1/", "Midnight Archive · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
  });
  await verifySurface("Inspector", "/inspector-v2/", "WorldStream Client Host", async (page) => {
    await page.getByText("This participant client session is missing or expired.", { exact: true }).waitFor();
    await page.getByText(
      "Ask the Host Operator to run worldstreamctl client open again for this Room setup operation.",
      { exact: true },
    ).waitFor();
  });
  console.log("Activity Client browser acceptance passed for exact Heist, Negotiate 0.1/0.2, current and retained Midnight Archive, and Inspector artifacts.");
} finally {
  await browser.close();
  await host.close();
}

async function verifySurface(label, path, expectedTitle, assertPage, hosted = false) {
  const page = await browser.newPage();
  const failures = [];
  page.on("request", (request) => {
    if (!hosted && request.url() === `${host.origin}/api/auth/session`) {
      failures.push(`${label} unexpectedly requested platform authentication`);
    }
    if (hosted && request.url().startsWith("http://127.0.0.1:9420/")) {
      failures.push(`${label} fell back to local Controller authority`);
    }
  });
  page.on("pageerror", (error) => failures.push(`${label} page error: ${error.message}`));
  page.on("console", (message) => {
    const expectedExpiredSessionFetch = message.text()
      === "Failed to load resource: the server responded with a status of 401 (Unauthorized)";
    if (message.type() === "error" && !expectedExpiredSessionFetch) {
      failures.push(`${label} console error: ${message.text()}`);
    }
  });
  if (hosted) await page.route(`${host.origin}/api/auth/session`, (route) => route.fulfill({
    status: 401,
    contentType: "application/json",
    body: JSON.stringify({
      authenticated: false,
    }),
  }));
  await page.route("http://127.0.0.1:9420/**", (route) => route.fulfill({
      status: 401,
      contentType: "application/json",
      headers: {
        "Access-Control-Allow-Credentials": "true",
        "Access-Control-Allow-Origin": host.origin,
      },
      body: JSON.stringify({
        code: "participant_session_missing",
        message: "This participant client session is missing or expired.",
        next_action: "return_to_task_setup",
        retryable: false,
      }),
    }));
  const response = await page.goto(`${host.origin}${path}`, { waitUntil: "networkidle" });
  if (response === null || !response.ok()) throw new Error(`${label} artifact did not load`);
  if (await page.title() !== expectedTitle) throw new Error(`${label} was not served from its retained standalone artifact`);
  await assertPage(page);
  if (failures.length > 0) throw new Error(failures.join("\n"));
  await page.close();
}

async function verifyInitialTicketRecovery() {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  const counts = { redeem: 0, resume: 0, ticket: 0, actions: 0 };
  const errors = [];
  let sessionMissing = false;
  page.on("pageerror", (error) => errors.push(error.message));
  const fulfill = (route, status, body) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
  const usable = { version: "participant_console_session.v1", state: "usable", next_action: "continue" };
  await page.route(`${host.origin}/api/**`, (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/auth/session") return fulfill(route, 200, {
      authenticated: true, csrf: "c".repeat(43), browser_stream_url: "ws://127.0.0.1:8080/v1/hosted/browser-stream",
    });
    if (path.endsWith("handoffs:redeem")) { counts.redeem += 1; return fulfill(route, 200, usable); }
    if (path.endsWith("/session")) {
      counts.resume += 1;
      return sessionMissing
        ? fulfill(route, 401, { code: "participant_session_missing", message: "Your session expired.", next_action: "return_to_task_setup", retryable: false })
        : fulfill(route, 200, usable);
    }
    if (path.endsWith("session:stream-ticket")) {
      counts.ticket += 1;
      return fulfill(route, 503, { code: "participant_session_unavailable", message: "The Activity Client cannot reach the Room service safely.", next_action: "reconnect", retryable: true });
    }
    counts.actions += 1;
    return fulfill(route, 500, {});
  });
  try {
    await page.goto(`${host.origin}/agent-heist-v11/hosted/#handoff=wsh1:${"ab".repeat(32)}`, { waitUntil: "networkidle" });
    await page.getByRole("button", { name: "Reconnect", exact: true }).waitFor();
    if (await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)) throw new Error("initial recovery screen overflows the mobile viewport");
    if (page.url().includes("#handoff")) throw new Error("initial handoff was retained in the URL");
    await page.getByRole("button", { name: "Reconnect", exact: true }).click();
    await page.waitForFunction(() => !document.querySelector(".heist-boundary-shell")?.getAttribute("aria-busy")?.includes("true"));
    if (counts.redeem !== 1 || counts.resume !== 1 || counts.ticket !== 2 || counts.actions !== 0) {
      throw new Error("recovery did not use the existing session and a single fresh ticket request");
    }
    if (await page.locator("form").count() !== 0) throw new Error("an unsynchronized client exposed an action form");
    sessionMissing = true;
    await page.getByRole("button", { name: "Reconnect", exact: true }).click();
    await page.getByRole("heading", { name: "Re-enter your mission", exact: true }).waitFor();
    if (await page.getByRole("button", { name: "Reconnect", exact: true }).count() !== 0) throw new Error("expired authority remained reconnectable");
    if (counts.redeem !== 1 || counts.ticket !== 2 || errors.length > 0) throw new Error("expired-session recovery replayed authority or threw");
    console.log("Initial Heist ticket failure: explicit reconnect, no handoff/action replay, expired-session re-entry, mobile layout passed.");
  } finally { await page.close(); }
}

function reject(value, pattern, message) {
  if (pattern.test(value)) throw new Error(message);
}

async function localBrowserOptions() {
  const candidates = [
    process.env.WORLDSTREAM_BROWSER_BINARY,
    process.platform === "darwin" ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : undefined,
    process.platform === "linux" ? "/usr/bin/google-chrome" : undefined,
    process.platform === "linux" ? "/usr/bin/chromium" : undefined,
    process.platform === "win32" && process.env.PROGRAMFILES
      ? join(process.env.PROGRAMFILES, "Google", "Chrome", "Application", "chrome.exe")
      : undefined,
  ].filter((candidate) => candidate !== undefined && candidate !== "");
  for (const executablePath of candidates) {
    try {
      await access(executablePath);
      return { executablePath };
    } catch {
      // Continue to the next explicit local browser candidate.
    }
  }
  return {};
}
