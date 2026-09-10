// Browser regression for the built client. The public stream is mocked; this
// is presentation evidence, not a live server or gameplay acceptance claim.
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright";
import { activityClientMounts, startActivityClientHost } from "./serve-activity-clients.mjs";

const host = await startActivityClientHost({ port: 0 });
const browser = await chromium.launch({ headless: true, ...(process.platform === "darwin"
  ? { executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" } : {}) });
const output = await mkdtemp(join(tmpdir(), "worldstream-countdown-browser-"));
const publicId = "a".repeat(32);
const prefix = activityClientMounts.find((mount) => mount.root.endsWith("/clients/agent-heist-web/dist")).prefix;
const url = `${host.origin}${prefix}hosted/?public_run=${publicId}&platform_return=%2F&platform_result=%2Fruns%2F${publicId}`;
const streamUrl = `ws://127.0.0.1:9999/v1/hosted/public-runs/${publicId}/stream`;
const digest = (character) => `blake3:${character.repeat(64)}`;
const packDigest = "blake3:56449d0830d1137d69b1b7c11ed25e8f0d9b7188d40e8290c58e5a2caff2bef9";

try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1100 } });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => { if (message.type() === "error") errors.push(message.text()); });
  await page.clock.install({ time: new Date("2026-09-10T12:00:00Z") });
  await page.clock.pauseAt(new Date("2026-09-10T12:00:00Z"));
  await page.route(`${host.origin}/api/runs/${publicId}`, (route) => route.fulfill({
    contentType: "application/json", body: JSON.stringify({ state: "live",
      live: { stream_url: streamUrl }, client: { launch_url: url } }),
  }));
  let socket;
  await page.routeWebSocket(streamUrl, (connection) => {
    socket = connection;
    connection.send(JSON.stringify(frame("briefing", "2026-09-10T12:01:30Z", 1)));
  });
  await page.goto(url);
  const timer = page.getByRole("timer");
  await timer.waitFor().catch(async (error) => {
    console.error({ body: await page.locator("body").innerText(), errors });
    throw error;
  });
  assert.equal(await timer.innerText(), "01:30");
  await page.clock.runFor(1_000);
  assert.equal(await timer.innerText(), "01:29");
  await page.screenshot({ path: join(output, "desktop.png"), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await timer.scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(output, "mobile.png") });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
  await page.clock.runFor(89_000);
  assert.equal(await timer.innerText(), "00:00");
  await page.getByText("Waiting for the next phase…", { exact: true }).waitFor();
  assert.match(await page.locator(".phase-window").innerText(), /Briefing/);
  socket.send(JSON.stringify(frame("negotiation", "2026-09-10T12:03:00Z", 2)));
  await page.getByRole("timer").filter({ hasText: "01:30" }).waitFor({ timeout: 5_000 }).catch(async (error) => {
    console.error({ body: await page.locator("body").innerText(), errors });
    throw error;
  });
  assert.match(await page.locator(".phase-window").innerText(), /Negotiation/);
  socket.send(JSON.stringify(frame("complete", null, 3)));
  await page.getByText("No active timer", { exact: true }).waitFor();
  assert.equal(await timer.count(), 0);
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ status: "passed", lane: "built-client-mocked-public-stream",
    checks: ["ticking", "zero_waits_for_server", "phase_reset", "terminal_no_timer", "mobile_no_horizontal_overflow", "no_browser_errors"],
    screenshots: output, provider_calls: 0 }));
} finally {
  await browser.close();
  await host.close();
}

function frame(phase, deadline, generation) {
  const head = { room_seq: generation, genesis_or_transition_hash: digest("1"),
    core_schema_version: "worldstream/core-room-state/v1", pack_digest: packDigest,
    core_state_hash: digest("3"), activity_state_hash: digest("4"), authoritative_state_hash: digest("5") };
  const result = { version: "worldstream/public-projection-stream/v1", batch: {
    pack: { id: "worldstream.agent-heist", version: "0.5.0", digest: packDigest },
    room_head: head, frame_head: generation, delivery: [{ kind: "projection_reset", body: {
      room_head: head, room_health: "healthy", integrity_generation: 0,
      baseline_frame_head: generation, reset_reason: "first_attach",
      projection_schema: "agent-heist/projection/v1", projection_hash: digest("6"),
      projection: { core: { access_mode: "spectator", standing: "enabled", role: null,
        room_status: "active", viewer_class: "public" }, action_offers: [], activity: {
        phase, phase_generation: generation, phase_start: "2026-09-10T12:00:00Z", phase_deadline: deadline,
        seats: ["navigator", "insider", "broker"].map((role) => ({ role, present: true })),
        public_claims: [], plans: [], endorsements: {}, challenges: [], commitment_count: 0, outcome: null,
      } },
    } }],
  } };
  if (generation > 1) {
    const activity = result.batch.delivery[0].body.projection.activity;
    result.batch.delivery = [{ kind: "observation", body: { frame_seq: generation,
      cause_room_seq: generation, frame_kind: "activity", observation_schema: "agent-heist/observation/v1",
      observation: activity, frame_payload_hash: digest("7") } }];
  }
  return result;
}
