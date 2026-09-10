import { strict as assert } from "node:assert";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { test } from "vitest";

import {
  isSelectedPublicViewerLaunch,
  readPlatformReturnTarget,
  readPublicViewerContext,
} from "./publicViewer";

const publicId = "a".repeat(32);
const href = `https://arena.example/agent-heist-v7/hosted/?public_run=${publicId}&platform_return=%2F&platform_result=%2Fruns%2F${publicId}`;

function target(value = href) {
  return { location: new URL(value) } as unknown as Window;
}

test("accepts only the bounded server-shaped public viewer context", () => {
  assert.deepEqual(readPublicViewerContext(target()), {
    publicId,
    backToGames: "/",
    resultPath: `/runs/${publicId}`,
  });
  for (const malicious of [
    `https://arena.example/agent-heist-v7/hosted/?public_run=${publicId}&platform_return=https%3A%2F%2Fattacker.example&platform_result=%2Fruns%2F${publicId}`,
    `https://arena.example/agent-heist-v7/hosted/?public_run=${publicId}&platform_return=%2F&platform_result=%2Fruns%2F${"b".repeat(32)}`,
    `https://arena.example/agent-heist-v7/hosted/?public_run=${publicId}&platform_return=%2F&platform_result=%2Fruns%2F${publicId}&handoff=secret`,
  ]) assert.equal(readPublicViewerContext(target(malicious)), null);
});

test("requires the BFF to select this exact Activity Client release", () => {
  assert.equal(isSelectedPublicViewerLaunch(target(), href), true);
  assert.equal(isSelectedPublicViewerLaunch(target(), href.replace("agent-heist-v7", "negotiate-v1")), false);
  assert.equal(isSelectedPublicViewerLaunch(target(), `https://attacker.example/agent-heist-v7/hosted/?public_run=${publicId}`), false);
});

test("Back to games never accepts a browser-selected destination", () => {
  assert.equal(readPlatformReturnTarget(target()), "/");
  assert.equal(
    readPlatformReturnTarget(target("https://arena.example/agent-heist-v7/hosted/?platform_return=https%3A%2F%2Fattacker.example")),
    "/",
  );
});

test("Back to games is navigation only and cannot depart a Membership", async () => {
  const source = await readFile(resolve(import.meta.dirname, "main.tsx"), "utf8");
  assert.match(source, /window\.location\.assign\(returnTarget\)/u);
  assert.doesNotMatch(source, /session:logout|Membership departure|leave membership/iu);
});
