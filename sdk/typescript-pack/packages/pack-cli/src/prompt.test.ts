import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import {
  access,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { createServer, type IncomingHttpHeaders } from "node:http";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { inspectPackBundle, readCanonicalUstar } from "./bundle.js";
import { parseCanonical } from "./canonical.js";
import { PackCliError, formatDiagnostic } from "./diagnostics.js";
import { PROMPT_STAGING_ROOT } from "./prompt.js";
import { SCAFFOLD_RELATIVE_PATHS } from "./scaffold.js";
import { WorldStreamPackToolchain } from "./toolchain.js";

interface FakeResponse {
  readonly body: string;
  readonly contentType?: string;
  readonly status?: number;
}

interface CapturedRequest {
  readonly body: unknown;
  readonly headers: IncomingHttpHeaders;
}

test("prompt creation stages a closed scaffold, requires review, and uses the normal pipeline", async () => {
  const blueprint = {
    description: "A bounded calibration price agreement between one buyer and one supplier.",
    display_name: "Calibration Agreement Room",
    format: "worldstream/prompt-pack-blueprint/v1",
    template: "two_party_price_negotiation",
  };
  await withFakeProvider([structuredResponse(blueprint)], async (endpoint, captured) => {
    const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
    await mkdir(scratch, { recursive: true });
    const parent = await mkdtemp(join(scratch, "prompt-success-"));
    const prompted = join(parent, "calibration-room");
    const offline = join(parent, "offline-room");
    const prompt = "Build a calibration negotiation. PROMPT_TRANSCRIPT_71d2 must stay private.";
    const model = "private-model-IMO117-92f4";
    const apiKey = "test-only-test-only";
    const toolchain = new WorldStreamPackToolchain({
      productionHostProver: async (bundlePath) => {
        const inspection = await inspectPackBundle(bundlePath);
        const members = readCanonicalUstar(new Uint8Array(await readFile(bundlePath)));
        const descriptor = parseCanonical(members.get("descriptor.json")!) as Record<
          string,
          unknown
        >;
        return {
          accepted_action: true,
          bundle_digest: inspection.bundleDigest,
          declared_rejection: true,
          pack_id: String(descriptor.pack_id),
          private_views: 2,
          proof_type: "complete",
          retained_old_revision: true,
          revision_digest: inspection.manifest.revision_digest,
          roles: 2,
          room_id: "01ARZ3NDEKTSV4RRFFQ69G5FPR",
          status: "passed",
          transcript_digest: `blake3:${"f".repeat(64)}`,
        };
      },
    });

    const review = await toolchain.createPrompt(prompted, {
      apiKey,
      endpoint,
      model,
      prompt,
    });
    assert.equal(review.status, "review_required");
    assert.ok(review.reviewDirectory.startsWith(`${PROMPT_STAGING_ROOT}/`));
    assert.deepEqual(review.files, [...SCAFFOLD_RELATIVE_PATHS].sort());
    await assert.rejects(() => access(prompted));
    await assert.rejects(() => access(join(review.reviewDirectory, "dist")));

    await toolchain.new(offline);
    assert.deepEqual(await relativeFiles(review.reviewDirectory), await relativeFiles(offline));
    const promptedContract = JSON.parse(
      await readFile(join(review.reviewDirectory, "worldstream-pack.json"), "utf8"),
    ) as unknown;
    const offlineContract = JSON.parse(
      await readFile(join(offline, "worldstream-pack.json"), "utf8"),
    ) as unknown;
    assert.deepEqual(Object.keys(promptedContract as object), Object.keys(offlineContract as object));
    const promptedPackage = JSON.parse(
      await readFile(join(review.reviewDirectory, "package.json"), "utf8"),
    ) as { readonly scripts: unknown };
    const offlinePackage = JSON.parse(await readFile(join(offline, "package.json"), "utf8")) as {
      readonly scripts: unknown;
    };
    assert.deepEqual(promptedPackage.scripts, offlinePackage.scripts);

    const secrets = [prompt, model, apiKey, endpoint];
    await assertTreeOmits(review.reviewDirectory, secrets);
    assertReceiptOmits(review, secrets);
    assert.equal(captured.length, 1);
    assert.equal(captured[0]?.headers.authorization, `Bearer ${apiKey}`);
    const request = captured[0]?.body as {
      readonly model?: unknown;
      readonly response_format?: { readonly type?: unknown; readonly json_schema?: unknown };
    };
    assert.equal(request.model, model);
    assert.equal(request.response_format?.type, "json_schema");
    assert.equal(typeof request.response_format?.json_schema, "object");

    const promoted = await toolchain.confirmPrompt(prompted, review.reviewId);
    assert.equal(promoted.status, "created");
    assertReceiptOmits(promoted, secrets);
    await assert.rejects(() => access(review.reviewDirectory));
    await assert.rejects(() => access(join(prompted, "dist")));

    const checked = await toolchain.check(prompted);
    const tested = await toolchain.test(prompted);
    const proved = await toolchain.prove(prompted);
    assert.equal(checked.status, "passed");
    assert.equal(tested.status, "passed");
    assert.equal(proved.status, "passed");
    assertReceiptOmits(checked, secrets);
    assertReceiptOmits(tested, secrets);
    assertReceiptOmits(proved, secrets);
    await assertTreeOmits(prompted, secrets);
  });
});

test("provider failures and hostile structured output fail without leaking context", async () => {
  const prompt = "PROMPT_TRANSCRIPT_failure_62aa";
  const model = "private-model-failure-449b";
  const apiKey = "test-only-test-only";
  const hostile = {
    command: "npm install hostile-package",
    description: "Hostile output",
    display_name: "Hostile Pack",
    files: [{ contents: "bad", path: "../../escape" }],
    format: "worldstream/prompt-pack-blueprint/v1",
    template: "two_party_price_negotiation",
  };
  const echoed = {
    description: `Unsafe echo ${model}`,
    display_name: "Echo Pack",
    format: "worldstream/prompt-pack-blueprint/v1",
    template: "two_party_price_negotiation",
  };
  await withFakeProvider(
    [
      { body: `${prompt} ${model} ${apiKey}`, status: 500 },
      structuredResponse("```json\n{}\n```"),
      structuredResponse(hostile),
      structuredResponse(echoed),
    ],
    async (endpoint) => {
      const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
      await mkdir(scratch, { recursive: true });
      const parent = await mkdtemp(join(scratch, "prompt-failure-"));
      const toolchain = new WorldStreamPackToolchain();
      const expectedCodes = [
        "WSP-PROMPT-004",
        "WSP-PROMPT-005",
        "WSP-PROMPT-006",
        "WSP-PROMPT-007",
      ];
      for (const [index, expected] of expectedCodes.entries()) {
        const target = join(parent, `rejected-${index}`);
        await assert.rejects(
          () => toolchain.createPrompt(target, { apiKey, endpoint, model, prompt }),
          (error: unknown) => {
            assert.ok(error instanceof PackCliError);
            assert.equal(error.diagnostics[0]?.code, expected);
            const visible = `${error.message}\n${error.diagnostics.map(formatDiagnostic).join("\n")}`;
            for (const secret of [prompt, model, apiKey, endpoint]) {
              assert.equal(visible.includes(secret), false);
            }
            return true;
          },
        );
        await assert.rejects(() => access(target));
      }
    },
  );
});

test("promotion rejects traversal identifiers, staged symlinks, and destination symlinks", async () => {
  const blueprint = {
    description: "A safe staged negotiation scaffold.",
    display_name: "Safe Review Pack",
    format: "worldstream/prompt-pack-blueprint/v1",
    template: "two_party_price_negotiation",
  };
  await withFakeProvider(
    [structuredResponse(blueprint), structuredResponse(blueprint)],
    async (endpoint) => {
      const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
      await mkdir(scratch, { recursive: true });
      const parent = await mkdtemp(join(scratch, "prompt-security-"));
      const toolchain = new WorldStreamPackToolchain();

      await assert.rejects(
        () => toolchain.confirmPrompt(join(parent, "escape"), "../../escape"),
        hasCode("WSP-PROMPT-009"),
      );

      const symlinkTarget = join(parent, "symlink-project");
      const symlinkReview = await toolchain.createPrompt(symlinkTarget, {
        endpoint,
        prompt: "Create a safe negotiation scaffold for symlink testing.",
      });
      const outside = join(parent, "outside.ts");
      await writeFile(outside, "outside\n");
      const stagedEntrypoint = join(symlinkReview.reviewDirectory, "src", "pack.ts");
      await rm(stagedEntrypoint);
      await symlink(outside, stagedEntrypoint);
      await assert.rejects(
        () => toolchain.confirmPrompt(symlinkTarget, symlinkReview.reviewId),
        hasCode("WSP-PROMPT-010"),
      );
      await assert.rejects(() => access(symlinkTarget));

      const destinationTarget = join(parent, "destination-project");
      const destinationReview = await toolchain.createPrompt(destinationTarget, {
        endpoint,
        prompt: "Create another safe negotiation scaffold for destination testing.",
      });
      const outsideDirectory = join(parent, "outside-directory");
      await mkdir(outsideDirectory);
      await symlink(outsideDirectory, destinationTarget);
      await assert.rejects(
        () => toolchain.confirmPrompt(destinationTarget, destinationReview.reviewId),
        hasCode("WSP-PROMPT-011"),
      );
      assert.deepEqual(await readdir(outsideDirectory), []);

      await rm(join(PROMPT_STAGING_ROOT, symlinkReview.reviewId), {
        force: true,
        recursive: true,
      });
      await rm(join(PROMPT_STAGING_ROOT, destinationReview.reviewId), {
        force: true,
        recursive: true,
      });
    },
  );
});

test("the create --prompt CLI emits only review and promotion receipts", async () => {
  const blueprint = {
    description: "A bounded procurement negotiation scaffold.",
    display_name: "Procurement Review Room",
    format: "worldstream/prompt-pack-blueprint/v1",
    template: "two_party_price_negotiation",
  };
  await withFakeProvider([structuredResponse(blueprint)], async (endpoint) => {
    const scratch = fileURLToPath(new URL("../.worldstream/", import.meta.url));
    await mkdir(scratch, { recursive: true });
    const parent = await mkdtemp(join(scratch, "prompt-cli-"));
    const target = join(parent, "procurement-room");
    const prompt = "Create procurement negotiation PROMPT_CLI_PRIVATE_78e4";
    const model = "private-cli-model-91b7";
    const apiKey = "sk-private-cli-25ca";
    const first = await runCliProcess(
      ["create", "--prompt", prompt, target, "--endpoint", endpoint, "--model", model],
      { WORLDSTREAM_PACK_OPENAI_API_KEY: apiKey },
    );
    assert.equal(first.code, 0, first.stderr);
    assert.equal(first.stderr, "");
    const review = JSON.parse(first.stdout) as {
      readonly reviewId: string;
      readonly status: string;
    };
    assert.equal(review.status, "review_required");
    for (const secret of [prompt, model, apiKey, endpoint]) {
      assert.equal(first.stdout.includes(secret), false);
      assert.equal(first.stderr.includes(secret), false);
    }
    await assert.rejects(() => access(target));

    const second = await runCliProcess(["create", target, "--confirm", review.reviewId]);
    assert.equal(second.code, 0, second.stderr);
    assert.equal(second.stderr, "");
    assert.equal((JSON.parse(second.stdout) as { readonly status: string }).status, "created");
    await access(join(target, "worldstream-pack.json"));
  });
});

function structuredResponse(value: unknown): FakeResponse {
  const content = typeof value === "string" ? value : JSON.stringify(value);
  return {
    body: JSON.stringify({ choices: [{ message: { content, role: "assistant" } }] }),
  };
}

async function withFakeProvider(
  responses: readonly FakeResponse[],
  run: (endpoint: string, captured: readonly CapturedRequest[]) => Promise<void>,
): Promise<void> {
  const pending = [...responses];
  const captured: CapturedRequest[] = [];
  const server = createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => chunks.push(chunk));
    request.on("end", () => {
      const raw = Buffer.concat(chunks).toString("utf8");
      captured.push({ body: JSON.parse(raw) as unknown, headers: request.headers });
      const next = pending.shift() ?? { body: "missing fake response", status: 500 };
      response.statusCode = next.status ?? 200;
      response.setHeader("content-type", next.contentType ?? "application/json");
      response.end(next.body);
    });
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("fake provider did not bind TCP");
  const endpoint = `http://127.0.0.1:${address.port}/v1/chat/completions`;
  try {
    await run(endpoint, captured);
    assert.equal(pending.length, 0);
  } finally {
    await new Promise<void>((resolve, reject) => {
      server.close((error) => (error ? reject(error) : resolve()));
    });
  }
}

async function relativeFiles(root: string, prefix = ""): Promise<string[]> {
  const files: string[] = [];
  for (const entry of await readdir(join(root, prefix), { withFileTypes: true })) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) files.push(...(await relativeFiles(root, path)));
    else if (entry.isFile()) files.push(path);
  }
  return files.sort();
}

async function assertTreeOmits(root: string, forbidden: readonly string[]): Promise<void> {
  for (const path of await relativeFiles(root)) {
    const bytes = await readFile(join(root, path));
    for (const value of forbidden) {
      assert.equal(bytes.includes(Buffer.from(value)), false, `${path} retained private context`);
    }
  }
}

function assertReceiptOmits(receipt: object, forbidden: readonly string[]): void {
  const visible = JSON.stringify(receipt);
  for (const value of forbidden) assert.equal(visible.includes(value), false);
}

function hasCode(expected: string): (error: unknown) => boolean {
  return (error: unknown) => {
    assert.ok(error instanceof PackCliError);
    assert.equal(error.diagnostics[0]?.code, expected);
    return true;
  };
}

async function runCliProcess(
  args: readonly string[],
  environment: Readonly<Record<string, string>> = {},
): Promise<{ readonly code: number | null; readonly stderr: string; readonly stdout: string }> {
  const executable = fileURLToPath(new URL("./main.js", import.meta.url));
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [executable, ...args], {
      env: { ...process.env, ...environment },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => (stdout += chunk));
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => (stderr += chunk));
    child.once("error", reject);
    child.once("exit", (code) => resolve({ code, stderr, stdout }));
  });
}
