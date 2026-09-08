import assert from "node:assert/strict";
import { test } from "node:test";

import { runHostedSmokeCommand, safeSetupAttention } from "./hosted-production-smoke.mjs";

test("setup attention diagnostics retain only reviewed reason and retryability", () => {
  for (const [code, retryable] of [["daemon_result_ambiguous", true], ["setup_credential_unavailable", true], ["setup_stage_rejected", false]]) {
    assert.deepEqual(safeSetupAttention({ state: "needs_attention", attention: { code, retryable, message: "private-secret" }, seats: ["private-authority"] }), { code, retryable });
    assert.equal(safeSetupAttention({ state: "needs_attention", attention: { code, retryable: !retryable } }), null);
  }
  for (const value of [null, [], {}, { state: "ready", attention: { code: "daemon_result_ambiguous", retryable: true } }, { state: "needs_attention", attention: { code: "private-value", retryable: true } }]) {
    assert.equal(safeSetupAttention(value), null);
  }
});

const setupFailure = {
  schema: "worldstream/operator-command/v1",
  command: "room create",
  status: "partial",
  code: "setup_incomplete",
  stage: "member_capability",
  message: "Room setup is incomplete; retained work has not been rolled back.",
  next_action: "private follow-up instructions",
  operation_id: "private-operation-identifier",
  room_id: "private-room-identifier",
};

async function failCommand(stdout, expectedCommand = "room create") {
  const child = "process.stdout.write(process.argv[1], () => { process.stderr.write('private-stderr-credential', () => process.exit(4)); });";
  let message;
  await assert.rejects(
    runHostedSmokeCommand(process.execPath, ["-e", child, stdout], {}, true, expectedCommand),
    (error) => {
      assert.ok(error instanceof Error);
      message = error.message;
      return true;
    },
  );
  return message;
}

test("an original incomplete Room creation reports only its closed setup stage", async () => {
  for (const stage of ["room_creation", "member_capability", "runner_capability"]) {
    assert.equal(
      await failCommand(`${JSON.stringify({ ...setupFailure, stage })}\n`),
      `hosted_smoke_command_failed:{"command":"room create","status":"partial","code":"setup_incomplete","stage":"${stage}"}`,
    );
  }
});

test("malformed, mismatched, or sensitive response shapes cannot become diagnostics", async () => {
  const invalid = [
    "not JSON",
    `${JSON.stringify(setupFailure)}\n${JSON.stringify(setupFailure)}`,
    JSON.stringify([setupFailure]),
    "null",
    ...[
      { schema: "untrusted/schema" },
      { command: "client export-credentials" },
      { command: "private-command-value" },
      { status: "failed" },
      { status: "private-status-value" },
      { code: "private-code-value" },
      { stage: "private-stage-value" },
      { stage: null },
      { operation_id: {} },
      { room_id: {} },
      { message: { credential: "private-nested-value" } },
      { credentials: "private-extra-value" },
      { message: "private-padding".repeat(2_000) },
    ].map((change) => JSON.stringify({ ...setupFailure, ...change })),
  ];
  for (const stdout of invalid) {
    assert.equal(await failCommand(stdout), "hosted_smoke_command_failed");
  }
  assert.equal(await failCommand(JSON.stringify(setupFailure), "room list"), "hosted_smoke_command_failed");
  assert.equal(await failCommand(JSON.stringify(setupFailure), "untrusted command"), "hosted_smoke_command_failed");
});

test("the real partial Room operation envelope retains its stage without nested details", async () => {
  const report = {
    ...setupFailure,
    room_operation: {
      version: "worldstream/room-setup-operation-status/v1",
      operation: "private-operation-identifier",
      room_id: "private-room-identifier",
      complete: false,
      stage: "member",
      active_stage: "member_capability",
      next_action: "private-operation-instructions",
      assessment: null,
    },
  };
  assert.equal(await failCommand(JSON.stringify(report)),
    'hosted_smoke_command_failed:{"command":"room create","status":"partial","code":"setup_incomplete","stage":"member_capability"}');
});

test("known non-setup failure pairs stay bounded and a failed process cannot claim success", async () => {
  const { operation_id, room_id, stage, ...base } = setupFailure;
  const failure = { ...base, command: "room list", status: "unavailable", code: "controller_unavailable" };
  assert.equal(
    await failCommand(JSON.stringify(failure), "room list"),
    'hosted_smoke_command_failed:{"command":"room list","status":"unavailable","code":"controller_unavailable"}',
  );
  assert.equal(await failCommand(JSON.stringify({ ...failure, status: "complete", code: "complete" }), "room list"), "hosted_smoke_command_failed");
});

test("a successful original child preserves capture semantics without disclosing stderr", async () => {
  const child = "process.stdout.write('successful-output'); process.stderr.write('private-stderr-credential');";
  for (const capture of [true, false]) {
    assert.deepEqual(await runHostedSmokeCommand(process.execPath, ["-e", child], {}, capture), {
      stdout: capture ? "successful-output" : "", stderr: "",
    });
  }
});

test("complete large output is captured after the child streams close", async () => {
  const output = "x".repeat(524_288);
  const child = "process.stdout.write('x'.repeat(524288)); process.exitCode = 0;";
  const result = await runHostedSmokeCommand(process.execPath, ["-e", child], {}, true);
  assert.equal(result.stdout, output);
});

test("truncated output cannot masquerade as a valid failure envelope", async () => {
  const child = "process.stdout.write(' '.repeat(1048577) + process.argv[1]); process.exitCode = 4;";
  await assert.rejects(
    runHostedSmokeCommand(process.execPath, ["-e", child, JSON.stringify(setupFailure)], {}, true, "room create"),
    { message: "hosted_smoke_command_failed" },
  );
});
