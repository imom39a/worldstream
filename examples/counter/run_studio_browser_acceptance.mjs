/** Run the UI-only Counter acceptance against either shipped browser adapter. */

import { spawnSync } from "node:child_process";
import { chmod, lstat, mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { runStudioBrowserAcceptance } from "./studio_browser_acceptance.mjs";

const URL_SCHEMA = "worldstream/counter-browser-url-evidence/v1";
const TRANSIENT_COMPLETION_REASONS = new Set([
  "completion_receipt_count_invalid",
  "managed_host_operation_missing",
  "managed_host_restart_not_retained",
  "active_launch_missing",
  "launch_record_missing",
  "activation_launch_record_missing",
  "activation_lease_recovery_not_retained",
  "increment_action_receipt_missing",
]);

export class SidecarVerificationError extends Error {
  constructor(reason) {
    super(reason);
    this.reason = reason;
  }
}

function requireValue(value, name) {
  if (value === undefined || value === null || value === "") throw new TypeError(`${name} is required`);
}

async function writeOwnerOnly(path, value) {
  await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  try {
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      throw new Error("browser_evidence_path_unsafe");
    }
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  await writeFile(path, value, { mode: 0o600 });
  await chmod(path, 0o600);
}

async function captureEvidence(adapter, consoleTab, paths) {
  if (!consoleTab) throw new Error("protected_console_evidence_missing");
  const studio = String(await adapter.tab.domSnapshot());
  const console = String(await consoleTab.domSnapshot());
  const studioUrls = await adapter.observedRequestUrls(adapter.tab);
  const consoleUrls = await adapter.observedRequestUrls(consoleTab);
  const studioCurrentUrl = await adapter.currentUrl(adapter.tab);
  const consoleCurrentUrl = await adapter.currentUrl(consoleTab);
  if (!Array.isArray(studioUrls) || !Array.isArray(consoleUrls)
      || typeof studioCurrentUrl !== "string" || typeof consoleCurrentUrl !== "string") {
    throw new Error("browser_request_url_evidence_required");
  }
  await writeOwnerOnly(paths.studioDom, studio);
  await writeOwnerOnly(paths.consoleDom, console);
  await writeOwnerOnly(paths.urls, `${JSON.stringify({
    schema: URL_SCHEMA,
    studio_current_url: studioCurrentUrl,
    console_current_url: consoleCurrentUrl,
    studio_request_urls: studioUrls,
    console_request_urls: consoleUrls,
  })}\n`);
}

function verify(mode, options, witness, beforeWitness) {
  const command = [
    "uv", "run", "--project", "sdk/python", "--python", "3.14.7", "python",
    "examples/counter/verify_studio_acceptance.py", mode,
    "--control-file", options.controlFile,
    "--state-dir", options.stateDir,
    "--draft-name", options.draftName,
    "--provider-status-url", options.providerStatusUrl,
    "--studio-dom", options.paths.studioDom,
    "--console-dom", options.paths.consoleDom,
    "--url-evidence", options.paths.urls,
    "--witness", witness,
  ];
  if (beforeWitness) command.push("--before-witness", beforeWitness);
  const run = spawnSync(command[0], command.slice(1), { cwd: options.repository, encoding: "utf8" });
  if (run.status !== 0) throw new SidecarVerificationError(sidecarReason(run.stdout));
  return JSON.parse(run.stdout);
}

function sidecarReason(output) {
  try {
    const value = JSON.parse(output);
    const reason = value?.status === "blocked" ? value.reason_code : undefined;
    if (typeof reason === "string" && /^[a-z0-9_]{1,120}$/.test(reason)) return reason;
  } catch {
    // The process output is deliberately not included in a browser-run error.
  }
  return "sidecar_result_unavailable";
}

async function providerCounts(url) {
  const response = await fetch(`${url.replace(/\/$/, "")}/fixture/status`);
  const value = await response.json();
  const received = value?.received_requests;
  const accepted = value?.accepted_requests;
  if (!response.ok || !Number.isInteger(received) || !Number.isInteger(accepted)) {
    throw new Error("provider_status_invalid");
  }
  return [received, accepted];
}

const ACCEPTANCE_TIMEOUT_MS = 120_000;
const PROVIDER_STABILITY_WINDOW_MS = 2_000;

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function waitForProviderRequest(url, timeoutMs = ACCEPTANCE_TIMEOUT_MS) {
  const deadline = Date.now() + timeoutMs;
  do {
    const [received] = await providerCounts(url);
    if (received > 0) return;
    await delay(50);
  } while (Date.now() < deadline);
  throw new Error("managed_host_provider_boundary_not_reached");
}

/** Require a stable post-restart observation window, not a single fast sample. */
export async function assertCountsRemainStable(
  readCounts,
  expected,
  { observeMs = PROVIDER_STABILITY_WINDOW_MS, pollMs = 100 } = {},
) {
  const deadline = Date.now() + observeMs;
  do {
    const actual = await readCounts();
    if (actual[0] !== expected[0] || actual[1] !== expected[1]) {
      throw new Error("completed_host_restart_contacted_provider");
    }
    if (Date.now() >= deadline) return;
    await delay(Math.min(pollMs, deadline - Date.now()));
  } while (true);
}

export async function waitForCompletedEvidence(
  capture,
  { timeoutMs = ACCEPTANCE_TIMEOUT_MS, pollMs = 250 } = {},
) {
  const deadline = Date.now() + timeoutMs;
  let lastReason = "completion_receipt_pending";
  do {
    try {
      return await capture();
    } catch (error) {
      if (!(error instanceof SidecarVerificationError)) {
        throw new Error("studio_acceptance_sidecar_unavailable");
      }
      lastReason = error.reason;
      if (!TRANSIENT_COMPLETION_REASONS.has(lastReason)) {
        throw new Error(`studio_acceptance_sidecar_failed:${lastReason}`);
      }
      await delay(pollMs);
    }
  } while (Date.now() < deadline);
  throw new Error(`managed_host_completion_receipt_not_stable:${lastReason}`);
}

async function assertProviderUnchanged(url, expected) {
  await assertCountsRemainStable(() => providerCounts(url), expected);
}

function captureOptions(options, repository, paths) {
  return { ...options, repository, paths };
}

async function captureCompletedEvidence(adapter, consoleTab, options, repository, paths) {
  await captureEvidence(adapter, consoleTab, paths);
  return waitForCompletedEvidence(
    () => verify("capture", captureOptions(options, repository, paths), options.captureWitness),
  );
}

function isCompletionMilestone(entry) {
  return entry.step === "managed_host_completion_stable";
}

function requireBaselineProvider(report) {
  if (!Number.isInteger(report?.provider?.received) || !Number.isInteger(report?.provider?.accepted)) {
    throw new Error("studio_acceptance_provider_baseline_missing");
  }
  return [report.provider.received, report.provider.accepted];
}

/** Keep the pre-click capture and post-click popup claim as separate operations. */
export function protectedConsoleCallbacks(adapter, onConsole) {
  return {
    beforeOpenProtectedConsole: () => adapter.beforeOpenProtectedConsole?.(),
    openProtectedConsole: async () => {
      const tab = await adapter.openProtectedConsole();
      onConsole(tab);
      return tab;
    },
  };
}

/** Public orchestration seam shared by Codex and Playwright adapters. */
export async function runCounterStudioBrowserAcceptance(adapter, options) {
  requireValue(adapter, "adapter");
  requireValue(options, "options");
  for (const name of ["artifactDir", "controlFile", "stateDir", "draftName", "providerStatusUrl", "captureWitness", "checkWitness"]) {
    requireValue(options[name], `options.${name}`);
  }
  const repository = options.repository ?? fileURLToPath(new URL("../../", import.meta.url));
  const paths = {
    studioDom: resolve(options.artifactDir, "studio-dom.html"),
    consoleDom: resolve(options.artifactDir, "console-dom.html"),
    urls: resolve(options.artifactDir, "browser-urls.json"),
  };
  let consoleTab;
  let captured = false;
  let baselineProvider;
  const consoleCallbacks = protectedConsoleCallbacks(adapter, (tab) => { consoleTab = tab; });
  const result = await runStudioBrowserAcceptance({
    tab: adapter.tab,
    fixture: adapter.fixture,
    ...consoleCallbacks,
    waitForManagedHostInFlight: async () => waitForProviderRequest(options.providerStatusUrl),
    beforeManagedHostRetry: adapter.beforeManagedHostRetry,
    afterCompletedHostRestart: async (entry) => {
      await adapter.afterCompletedHostRestart?.(entry);
      if (!baselineProvider) throw new Error("completed_host_restart_without_baseline");
      await assertProviderUnchanged(options.providerStatusUrl, baselineProvider);
    },
    acceptanceTimeoutMs: ACCEPTANCE_TIMEOUT_MS,
    onStep: async (entry) => {
      if (isCompletionMilestone(entry)) {
        const report = await captureCompletedEvidence(adapter, consoleTab, options, repository, paths);
        baselineProvider = requireBaselineProvider(report);
        captured = true;
      }
    },
  });
  if (!captured || result.paused) throw new Error("studio_acceptance_capture_not_reached");
  await captureEvidence(adapter, consoleTab, paths);
  const report = verify("check", { ...options, repository, paths }, options.checkWitness, options.captureWitness);
  await writeOwnerOnly(options.report ?? resolve(options.artifactDir, "report.json"), `${JSON.stringify(report)}\n`);
  return report;
}

export async function loadFixture(path) {
  const fixture = JSON.parse(await readFile(path, "utf8"));
  if (fixture?.schema !== "worldstream/counter-studio-browser-fixture/v1") {
    throw new TypeError("Counter browser fixture schema is invalid");
  }
  const encoded = JSON.stringify(fixture);
  if (/\b(?:room_id|member_id|membership_id|bearer|secret|token)\b|ws[bh]1:/i.test(encoded)) {
    throw new TypeError("Counter browser fixture contains protected material");
  }
  return fixture;
}

function optionsFromArgs(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!key?.startsWith("--") || value === undefined) throw new TypeError("expected --name value arguments");
    values.set(key.slice(2), value);
  }
  const required = (name) => {
    const value = values.get(name);
    requireValue(value, `--${name}`);
    return value;
  };
  return {
    adapter: required("adapter"),
    fixtureFile: required("fixture"),
    studioUrl: required("studio-url"),
    controlFile: required("control-file"),
    stateDir: required("state-dir"),
    draftName: required("draft-name"),
    providerStatusUrl: required("provider-status-url"),
    artifactDir: required("artifact-dir"),
    captureWitness: required("capture-witness"),
    checkWitness: required("check-witness"),
    report: values.get("report"),
  };
}

async function main() {
  const options = optionsFromArgs(process.argv.slice(2));
  if (options.adapter !== "playwright") {
    throw new TypeError("CLI supports --adapter playwright; use createCodexBrowserAdapter in the in-app runtime");
  }
  const fixture = await loadFixture(options.fixtureFile);
  const { createPlaywrightBrowserAdapter } = await import("./playwright_browser_adapter.mjs");
  const adapter = await createPlaywrightBrowserAdapter({ studioUrl: options.studioUrl, fixture });
  try {
    const report = await runCounterStudioBrowserAcceptance(adapter, options);
    process.stdout.write(`${JSON.stringify(report)}\n`);
  } finally {
    await adapter.close();
  }
}

if (typeof process !== "undefined" && process.argv?.[1]
    && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : "studio browser acceptance failed"}\n`);
    process.exitCode = 2;
  });
}
