import assert from "node:assert/strict";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { retainedTranscriptDigest } from "./conformance.js";
import { WorldStreamPackToolchain } from "./toolchain.js";
import { PackCliError } from "./diagnostics.js";

test("fresh scaffold passes strict check and behavioral conformance", async () => {
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const parent = await mkdtemp(join(scratch, "test-"));
  const root = join(parent, "vendor-negotiation");
  const toolchain = new WorldStreamPackToolchain();
  const created = await toolchain.new(root);
  assert.equal(created.status, "created");
  const checked = await toolchain.check(root);
  assert.equal(checked.status, "passed");
  const tested = await toolchain.test(root);
  assert.equal(tested.acceptedActions, 3);
  assert.equal(tested.declaredRejections, 1);
  assert.equal(tested.privateViewsDistinct, true);
});

test("check returns owned capability and module-state diagnostics", async () => {
  const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
  await mkdir(scratch, { recursive: true });
  const parent = await mkdtemp(join(scratch, "diagnostic-"));
  const root = join(parent, "unsafe-pack");
  const toolchain = new WorldStreamPackToolchain();
  await toolchain.new(root);
  await writeFile(
    join(root, "src", "ambient.ts"),
    "export let callbackCount = 0;\nexport function now(): number { return Date.now(); }\n",
  );
  await assert.rejects(
    () => toolchain.check(root),
    (error: unknown) => {
      assert.ok(error instanceof PackCliError);
      const codes = new Set(error.diagnostics.map((item) => item.code));
      assert.ok(codes.has("WSP-DETERMINISM-001"));
      assert.ok(codes.has("WSP-CAPABILITY-002"));
      return true;
    },
  );
});

test("retained golden identity accepts only a Core-authored tagged digest", () => {
  const local = `blake3:${"a".repeat(64)}`;
  const authoritative = `blake3:${"b".repeat(64)}`;
  assert.equal(retainedTranscriptDigest({}, local), local);
  assert.equal(
    retainedTranscriptDigest({ expected_transcript_digest: authoritative }, local),
    authoritative,
  );
  assert.throws(
    () => retainedTranscriptDigest({ expected_transcript_digest: "b".repeat(64) }, local),
    (error: unknown) =>
      error instanceof PackCliError &&
      error.diagnostics.some((item) => item.code === "WSP-GOLDEN-004"),
  );
});
