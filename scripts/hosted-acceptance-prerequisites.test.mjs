import assert from "node:assert/strict";
import { test } from "node:test";
import { HOSTED_ACCEPTANCE_PREREQUISITES, runHostedAcceptancePrerequisites } from "./hosted-acceptance-prerequisites.mjs";

test("acceptance runs the complete prerequisite list and stops on a failed gate", async () => {
  const seen = [];
  await runHostedAcceptancePrerequisites(async (command, args) => { seen.push([command, args]); });
  assert.deepEqual(seen, HOSTED_ACCEPTANCE_PREREQUISITES);
  let calls = 0;
  await assert.rejects(() => runHostedAcceptancePrerequisites(async () => {
    if (++calls === 2) throw new Error("failed gate");
  }), /failed gate/u);
  assert.equal(calls, 2);
  assert.ok(seen.some(([command, args]) => command === "supabase" && args[0] === "test"));
  assert.ok(seen.some(([, args]) => args.includes("hosted_browser_stream")));
});
