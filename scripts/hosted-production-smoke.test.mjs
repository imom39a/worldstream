import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import {
  hostedSmokeClientAssets,
  hostedSmokeBinaryRoot,
  runHostedSmokeCommand,
  runRoomCreateWithRetainedSetupRetry,
} from "./hosted-production-smoke.mjs";

test("the production smoke installs current V9 and every retained runtime client", () => {
  assert.deepEqual(hostedSmokeClientAssets("/repo", "/assets"), [
    {
      source: "/repo/config/activity-clients/releases/agent-heist-web-v9.json",
      destination: "/assets/agent-heist-web.json",
    },
    {
      source: "/repo/config/activity-clients/releases/agent-heist-web-v8.json",
      destination: "/assets/agent-heist-web-v8.json",
    },
    {
      source: "/repo/config/activity-clients/releases/agent-heist-web-v7.json",
      destination: "/assets/agent-heist-web-v7.json",
    },
  ]);
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

test("a retained setup envelope retries the exact Room create command once", async () => {
  const temporary = await mkdtemp(join(tmpdir(), "worldstream-hosted-smoke-retry-"));
  const attemptFile = join(temporary, "attempt");
  const child = [
    "const fs = require('node:fs');",
    "const attemptFile = process.argv[1];",
    "const attempt = fs.existsSync(attemptFile) ? Number(fs.readFileSync(attemptFile, 'utf8')) + 1 : 1;",
    "fs.writeFileSync(attemptFile, String(attempt));",
    `if (attempt === 1) { process.stdout.write(${JSON.stringify(JSON.stringify(setupFailure))}); process.exitCode = 4; }`,
    "else process.stdout.write(JSON.stringify({ room_id: 'retained-room', operation_id: 'retained-operation' }));",
  ].join(" ");
  const command = ["room", "create", "--file", "retained-room.json"];
  const calls = [];
  try {
    const result = await runRoomCreateWithRetainedSetupRetry(async (args) => {
      calls.push(args);
      return runHostedSmokeCommand(
        process.execPath,
        ["-e", child, attemptFile],
        {},
        true,
        `${args[0]} ${args[1]}`,
      );
    }, command);
    assert.equal(result.stdout, JSON.stringify({ room_id: "retained-room", operation_id: "retained-operation" }));
    assert.equal(calls.length, 2);
    assert.deepEqual(calls[0], command);
    assert.deepEqual(calls[1], command);
    assert.strictEqual(calls[0], calls[1]);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});

test("a retained Room create can progress through every staged setup boundary", async () => {
  const temporary = await mkdtemp(join(tmpdir(), "worldstream-hosted-smoke-stages-"));
  const attemptFile = join(temporary, "attempt");
  const stages = ["room_creation", "member_capability", "runner_capability"];
  const child = [
    "const fs = require('node:fs');",
    "const attemptFile = process.argv[1];",
    "const attempt = fs.existsSync(attemptFile) ? Number(fs.readFileSync(attemptFile, 'utf8')) + 1 : 1;",
    "fs.writeFileSync(attemptFile, String(attempt));",
    `if (attempt <= 3) { process.stdout.write(${JSON.stringify(JSON.stringify({ ...setupFailure, stage: "__STAGE__" }))}.replace('__STAGE__', [${stages.map((stage) => JSON.stringify(stage)).join(",")}][attempt - 1])); process.exitCode = 4; }`,
    "else process.stdout.write(JSON.stringify({ room_id: 'retained-room', operation_id: 'retained-operation' }));",
  ].join(" ");
  const command = ["room", "create", "--file", "retained-room.json"];
  const calls = [];
  try {
    const result = await runRoomCreateWithRetainedSetupRetry(async (args) => {
      calls.push(args);
      return runHostedSmokeCommand(
        process.execPath,
        ["-e", child, attemptFile],
        {},
        true,
        `${args[0]} ${args[1]}`,
      );
    }, command);
    assert.equal(result.stdout, JSON.stringify({ room_id: "retained-room", operation_id: "retained-operation" }));
    assert.equal(calls.length, 4);
    assert.ok(calls.every((args) => args === calls[0]));
    assert.deepEqual(calls[0], command);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});

test("the retained Room create retry has a four-attempt total cap", async () => {
  const command = ["room", "create", "--file", "retained-room.json"];
  const failure = new Error("retained setup still incomplete");
  Object.defineProperty(failure, "operatorFailure", {
    value: Object.freeze({
      command: "room create",
      status: "partial",
      code: "setup_incomplete",
      stage: "runner_capability",
    }),
  });
  let calls = 0;
  await assert.rejects(
    runRoomCreateWithRetainedSetupRetry(async () => {
      calls += 1;
      throw failure;
    }, command),
    failure,
  );
  assert.equal(calls, 4);
});

test("unrelated Room create failures are not retried", async () => {
  let calls = 0;
  const command = ["room", "create", "--file", "retained-room.json"];
  const child = "process.stdout.write(JSON.stringify({ schema: 'worldstream/operator-command/v1', command: 'room create', status: 'unavailable', code: 'controller_unavailable', message: 'private', next_action: 'private' })); process.exitCode = 4;";
  await assert.rejects(
    runRoomCreateWithRetainedSetupRetry(async (args) => {
      calls += 1;
      return runHostedSmokeCommand(process.execPath, ["-e", child], {}, true, `${args[0]} ${args[1]}`);
    }, command),
    { message: 'hosted_smoke_command_failed:{"command":"room create","status":"unavailable","code":"controller_unavailable"}' },
  );
  assert.equal(calls, 1);
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

test("the hosted package smoke honors Cargo's isolated target directory", () => {
  assert.equal(
    hostedSmokeBinaryRoot({ CARGO_TARGET_DIR: "/tmp/worldstream-hosted-smoke-target" }),
    "/tmp/worldstream-hosted-smoke-target/debug",
  );
  assert.match(hostedSmokeBinaryRoot({}), /\/target\/debug$/u);
});
