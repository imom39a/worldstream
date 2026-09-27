import { strict as assert } from "node:assert";
import { test } from "vitest";

import { readPlatformSession } from "./platformSession";

const csrf = "c".repeat(43);
const configuration = (browser_stream_url: unknown) => ({
  authenticated: true,
  csrf,
  browser_stream_url,
});

test("authenticated deployment configuration selects direct Fly or loopback streams", () => {
  for (const browserStreamUrl of [
    "wss://worldstream-preview.fly.dev/v1/hosted/browser-stream",
    "ws://127.0.0.1:8080/v1/hosted/browser-stream",
    "ws://localhost:8080/v1/hosted/browser-stream",
    "ws://[::1]:8080/v1/hosted/browser-stream",
  ]) {
    assert.deepEqual(readPlatformSession(configuration(browserStreamUrl)), {
      csrf,
      browserStreamUrl,
    });
  }
});

test("the client rejects absent, retargeted, or unsafe deployment stream configuration", () => {
  for (const url of [
    undefined,
    null,
    8080,
    "",
    "/v1/hosted/browser-stream",
    "https://worldstream-preview.fly.dev/v1/hosted/browser-stream",
    "ws://worldstream-preview.fly.dev/v1/hosted/browser-stream",
    "ws://127.0.0.1.evil.example/v1/hosted/browser-stream",
    "wss://worldstream-preview.fly.dev/v1/stream",
    "wss://worldstream-preview.fly.dev/v1/hosted/browser-stream?room=other",
    "wss://worldstream-preview.fly.dev/v1/hosted/browser-stream#ticket",
    "wss://worldstream-preview.fly.dev/v1/hosted/browser-stream?",
    "wss://worldstream-preview.fly.dev/v1/hosted/browser-stream#",
    "wss://user:password@worldstream-preview.fly.dev/v1/hosted/browser-stream",
    " wss://worldstream-preview.fly.dev/v1/hosted/browser-stream",
    `wss://${"x".repeat(2048)}.example/v1/hosted/browser-stream`,
  ]) {
    assert.throws(() => readPlatformSession(configuration(url)), /platform_session_configuration_unavailable/u);
  }
});

test("stream configuration does not replace an authenticated platform session and CSRF", () => {
  const valid = configuration("wss://worldstream-preview.fly.dev/v1/hosted/browser-stream");
  for (const value of [null, [], {}, { ...valid, authenticated: false },
    { ...valid, csrf: "short" }, { ...valid, csrf: "c".repeat(129) },
    { ...valid, csrf: `${"c".repeat(43)}\n` }]) {
    assert.throws(() => readPlatformSession(value), /platform_session_configuration_unavailable/u);
  }
});
