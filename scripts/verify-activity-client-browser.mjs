import { chromium } from "playwright";
import { access } from "node:fs/promises";
import { join } from "node:path";

import { startActivityClientHost } from "./serve-activity-clients.mjs";

const host = await startActivityClientHost({ port: 0 });
const browser = await chromium.launch({ headless: true, ...await localBrowserOptions() });

try {
  if ((await fetch(`${host.origin}/agent-heist/`)).status !== 404) throw new Error("unavailable retained Heist path must not serve replacement bytes");
  await verifySurface("Agent Heist local", "/agent-heist-v8/", "Agent Heist · WorldStream Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Waiting for authorized Projection" }).waitFor();
    const body = await page.locator("body").innerText();
    reject(body, /Canal Shift|route_service|Recorded fixture|Fixture mode/i, "Heist live client exposed recorded data");
  });
  await verifySurface("Agent Heist hosted", "/agent-heist-v8/hosted/", "Agent Heist · Hosted Activity Client", async (page) => {
    await page.getByRole("heading", { name: "Unable to enter this Run" }).waitFor();
  }, true);
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
  await verifySurface("Inspector", "/inspector-v2/", "WorldStream Client Host", async (page) => {
    await page.getByText("This participant client session is missing or expired.", { exact: true }).waitFor();
    await page.getByText(
      "Ask the Host Operator to run worldstreamctl client open again for this Room setup operation.",
      { exact: true },
    ).waitFor();
  });
  console.log("Activity Client browser acceptance passed for exact Heist, Negotiate 0.1/0.2, and Inspector artifacts.");
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
