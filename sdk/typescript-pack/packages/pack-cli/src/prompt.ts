import { createHash, randomBytes } from "node:crypto";
import { constants as fsConstants } from "node:fs";
import {
  chmod,
  copyFile,
  lstat,
  mkdir,
  readFile,
  readdir,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";

import { canonicalBytes, type JsonValue } from "./canonical.js";
import { PackCliError, diagnostic } from "./diagnostics.js";
import { isRecord, loadProject } from "./project.js";
import {
  SCAFFOLD_RELATIVE_PATHS,
  normalizeSlug,
  renderScaffoldFiles,
  scaffoldProject,
} from "./scaffold.js";

const PROMPT_BLUEPRINT_FORMAT = "worldstream/prompt-pack-blueprint/v1";
const PROMPT_TEMPLATE = "two_party_price_negotiation";
const REVIEW_FORMAT = "worldstream/prompt-pack-review/v1";
const DEFAULT_ENDPOINT = "http://127.0.0.1:11434/v1/chat/completions";
const DEFAULT_MODEL = "worldstream-pack-author";
const MAX_PROMPT_BYTES = 8 * 1024;
const MAX_RESPONSE_BYTES = 256 * 1024;
const MAX_PROJECT_BYTES = 1024 * 1024;
const REVIEW_ID = /^[0-9a-f]{32}$/u;
const SAFE_GENERATED_TEXT = /^[A-Za-z0-9][A-Za-z0-9 .,;:!?()&+/_'-]*$/u;

const userScope = typeof process.getuid === "function" ? String(process.getuid()) : "user";

/** Fixed, user-private staging authority for prompt-assisted authoring. */
export const PROMPT_STAGING_ROOT = join(
  tmpdir(),
  `worldstream-pack-create-v1-${userScope}`,
);

export interface PromptCreateOptions {
  readonly apiKey?: string;
  readonly endpoint?: string;
  readonly model?: string;
  readonly prompt: string;
}

export interface PromptReviewReceipt {
  readonly command: "create";
  readonly destination: string;
  readonly files: readonly string[];
  readonly next: {
    readonly command: "create";
    readonly confirm: string;
    readonly directory: string;
  };
  readonly reviewDirectory: string;
  readonly reviewId: string;
  readonly status: "review_required";
}

export interface PromptPromotionReceipt {
  readonly command: "create";
  readonly directory: string;
  readonly files: readonly string[];
  readonly status: "created";
}

interface PromptBlueprint {
  readonly description: string;
  readonly display_name: string;
  readonly format: typeof PROMPT_BLUEPRINT_FORMAT;
  readonly template: typeof PROMPT_TEMPLATE;
}

interface ReviewFile {
  readonly path: string;
  readonly sha256: string;
  readonly size: number;
}

interface ReviewRecord {
  readonly blueprint: PromptBlueprint;
  readonly destination: string;
  readonly files: readonly ReviewFile[];
  readonly format: typeof REVIEW_FORMAT;
  readonly review_id: string;
}

export async function createPromptReview(
  directory: string,
  options: PromptCreateOptions,
): Promise<PromptReviewReceipt> {
  const prompt = validatePrompt(options.prompt);
  const endpoint = validateEndpoint(
    options.endpoint ?? process.env.WORLDSTREAM_PACK_OPENAI_ENDPOINT ?? DEFAULT_ENDPOINT,
  );
  const model = validateModel(
    options.model ?? process.env.WORLDSTREAM_PACK_OPENAI_MODEL ?? DEFAULT_MODEL,
  );
  const apiKey = options.apiKey ?? process.env.WORLDSTREAM_PACK_OPENAI_API_KEY;
  validateApiKey(apiKey);
  const blueprint = await requestBlueprint({ apiKey, endpoint, model, prompt });
  rejectSensitiveEcho(blueprint, { apiKey, endpoint, model, prompt });

  const destination = resolve(directory);
  await ensurePrivateStagingRoot();
  const reviewId = randomBytes(16).toString("hex");
  const stage = join(PROMPT_STAGING_ROOT, reviewId);
  const project = join(stage, "project");
  await mkdir(stage, { mode: 0o700 });
  try {
    await scaffoldProject(project, {
      description: blueprint.description,
      displayName: blueprint.display_name,
      slug: normalizeSlug(basename(destination)),
    });
    const expected = renderScaffoldFiles(normalizeSlug(basename(destination)), {
      description: blueprint.description,
      displayName: blueprint.display_name,
    });
    const files = await inspectStagedProject(project, expected);
    const review: ReviewRecord = {
      blueprint,
      destination,
      files,
      format: REVIEW_FORMAT,
      review_id: reviewId,
    };
    await writeFile(join(stage, "review.json"), canonicalBytes(review as unknown as JsonValue), {
      flag: "wx",
      mode: 0o600,
    });
    return {
      command: "create",
      destination,
      files: files.map((file) => file.path),
      next: { command: "create", confirm: reviewId, directory: destination },
      reviewDirectory: project,
      reviewId,
      status: "review_required",
    };
  } catch (error) {
    await rm(stage, { force: true, recursive: true });
    throw error;
  }
}

export async function promotePromptReview(
  directory: string,
  reviewId: string,
): Promise<PromptPromotionReceipt> {
  if (!REVIEW_ID.test(reviewId)) {
    throw promptError("WSP-PROMPT-009", "Prompt review identifier is invalid");
  }
  await ensurePrivateStagingRoot();
  const destination = resolve(directory);
  const stage = join(PROMPT_STAGING_ROOT, reviewId);
  const review = await readReview(stage, reviewId);
  if (review.destination !== destination) {
    throw promptError(
      "WSP-PROMPT-009",
      "Prompt review belongs to a different destination",
      "Run the exact confirmation shown in the review receipt.",
    );
  }
  const project = join(stage, "project");
  const expected = renderScaffoldFiles(normalizeSlug(basename(destination)), {
    description: review.blueprint.description,
    displayName: review.blueprint.display_name,
  });
  const inspected = await inspectStagedProject(project, expected);
  if (!sameReviewFiles(review.files, inspected)) {
    throw promptError(
      "WSP-PROMPT-010",
      "Staged prompt project changed after review",
      "Request a new review instead of promoting modified or replaced staging files.",
    );
  }
  await prepareDestination(destination);
  for (const file of inspected) {
    const source = join(project, file.path);
    const target = join(destination, file.path);
    await mkdir(dirname(target), { recursive: true });
    await copyFile(source, target, fsConstants.COPYFILE_EXCL);
  }
  await loadProject(destination);
  await rm(stage, { recursive: true });
  return {
    command: "create",
    directory: destination,
    files: inspected.map((file) => file.path),
    status: "created",
  };
}

async function requestBlueprint(input: {
  readonly apiKey: string | undefined;
  readonly endpoint: URL;
  readonly model: string;
  readonly prompt: string;
}): Promise<PromptBlueprint> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (input.apiKey) headers.authorization = `Bearer ${input.apiKey}`;
  let response: Response;
  try {
    response = await fetch(input.endpoint, {
      body: JSON.stringify({
        messages: [
          {
            content:
              "Design one bounded WorldStream Activity Pack scaffold. Return only the requested JSON schema. Never return source files, paths, commands, dependencies, credentials, provider metadata, model metadata, or the user's prompt.",
            role: "system",
          },
          { content: input.prompt, role: "user" },
        ],
        model: input.model,
        response_format: {
          json_schema: {
            name: "worldstream_prompt_pack_blueprint_v1",
            schema: BLUEPRINT_JSON_SCHEMA,
            strict: true,
          },
          type: "json_schema",
        },
        temperature: 0,
      }),
      headers,
      method: "POST",
      redirect: "error",
      signal: AbortSignal.timeout(30_000),
    });
  } catch {
    throw promptError(
      "WSP-PROMPT-004",
      "Prompt provider request failed",
      "No provider response or request details were retained.",
    );
  }
  if (!response.ok) {
    await response.body?.cancel();
    throw promptError(
      "WSP-PROMPT-004",
      "Prompt provider rejected the request",
      `Provider returned HTTP ${response.status}; its response body was discarded.`,
    );
  }
  const contentType = response.headers.get("content-type") ?? "";
  if (!contentType.toLowerCase().includes("application/json")) {
    await response.body?.cancel();
    throw promptError(
      "WSP-PROMPT-005",
      "Prompt provider did not return structured JSON",
    );
  }
  const responseText = await readBoundedResponse(response);
  let envelope: unknown;
  try {
    envelope = JSON.parse(responseText);
  } catch {
    throw promptError(
      "WSP-PROMPT-005",
      "Prompt provider returned malformed structured JSON",
    );
  }
  const content = extractStructuredContent(envelope);
  let generated: unknown;
  try {
    generated = JSON.parse(content);
  } catch {
    throw promptError(
      "WSP-PROMPT-005",
      "Prompt provider message was not a structured JSON value",
    );
  }
  return validateBlueprint(generated);
}

async function readBoundedResponse(response: Response): Promise<string> {
  const reader = response.body?.getReader();
  if (!reader) {
    throw promptError("WSP-PROMPT-005", "Prompt provider response body is missing");
  }
  const chunks: Uint8Array[] = [];
  let total = 0;
  while (true) {
    const item = await reader.read();
    if (item.done) break;
    total += item.value.length;
    if (total > MAX_RESPONSE_BYTES) {
      await reader.cancel();
      throw promptError(
        "WSP-PROMPT-005",
        "Prompt provider response exceeded the bounded structured-response limit",
      );
    }
    chunks.push(item.value);
  }
  const joined = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    joined.set(chunk, offset);
    offset += chunk.length;
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(joined);
  } catch {
    throw promptError(
      "WSP-PROMPT-005",
      "Prompt provider response was not valid UTF-8 structured JSON",
    );
  }
}

function extractStructuredContent(value: unknown): string {
  if (!isRecord(value) || !Array.isArray(value.choices) || value.choices.length !== 1) {
    throw promptError("WSP-PROMPT-005", "Prompt provider response envelope is invalid");
  }
  const choice = value.choices[0];
  if (!isRecord(choice) || !isRecord(choice.message) || typeof choice.message.content !== "string") {
    throw promptError("WSP-PROMPT-005", "Prompt provider response has no structured content");
  }
  return choice.message.content;
}

function validateBlueprint(value: unknown): PromptBlueprint {
  const expected = ["description", "display_name", "format", "template"];
  if (!isRecord(value) || !sameStrings(Object.keys(value).sort(), expected)) {
    throw invalidBlueprint();
  }
  if (
    value.format !== PROMPT_BLUEPRINT_FORMAT ||
    value.template !== PROMPT_TEMPLATE ||
    !validGeneratedText(value.display_name, 80) ||
    !validGeneratedText(value.description, 400)
  ) {
    throw invalidBlueprint();
  }
  return value as unknown as PromptBlueprint;
}

function validGeneratedText(value: unknown, maxLength: number): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= maxLength &&
    SAFE_GENERATED_TEXT.test(value)
  );
}

function rejectSensitiveEcho(
  blueprint: PromptBlueprint,
  sensitive: {
    readonly apiKey: string | undefined;
    readonly endpoint: URL;
    readonly model: string;
    readonly prompt: string;
  },
): void {
  const generated = `${blueprint.display_name}\n${blueprint.description}`.toLowerCase();
  const forbidden = [
    sensitive.apiKey,
    sensitive.endpoint.href,
    sensitive.endpoint.host,
    sensitive.model,
    sensitive.prompt,
  ].filter((value): value is string => typeof value === "string" && value.length >= 8);
  if (forbidden.some((value) => generated.includes(value.toLowerCase()))) {
    throw promptError(
      "WSP-PROMPT-007",
      "Generated project content echoed private prompt-provider context",
      "No generated files were staged.",
    );
  }
}

async function ensurePrivateStagingRoot(): Promise<void> {
  await mkdir(PROMPT_STAGING_ROOT, { mode: 0o700, recursive: true });
  const status = await lstat(PROMPT_STAGING_ROOT);
  if (status.isSymbolicLink() || !status.isDirectory()) {
    throw promptError(
      "WSP-PROMPT-008",
      "Fixed prompt staging root is not a real directory",
    );
  }
  await chmod(PROMPT_STAGING_ROOT, 0o700);
}

async function readReview(stage: string, reviewId: string): Promise<ReviewRecord> {
  const path = join(stage, "review.json");
  let bytes: Buffer;
  try {
    const stageStatus = await lstat(stage);
    if (stageStatus.isSymbolicLink() || !stageStatus.isDirectory()) {
      throw new Error("unsafe review directory");
    }
    const status = await lstat(path);
    if (status.isSymbolicLink() || !status.isFile() || status.size > 64 * 1024) {
      throw new Error("unsafe review metadata");
    }
    bytes = await readFile(path);
  } catch {
    throw promptError("WSP-PROMPT-009", "Prompt review was not found or is invalid");
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    throw promptError("WSP-PROMPT-009", "Prompt review metadata is malformed");
  }
  if (
    !isRecord(parsed) ||
    !sameStrings(Object.keys(parsed).sort(), [
      "blueprint",
      "destination",
      "files",
      "format",
      "review_id",
    ]) ||
    parsed.format !== REVIEW_FORMAT ||
    parsed.review_id !== reviewId ||
    typeof parsed.destination !== "string" ||
    !Array.isArray(parsed.files)
  ) {
    throw promptError("WSP-PROMPT-009", "Prompt review metadata is invalid");
  }
  const files = parsed.files.map(validateReviewFile);
  return {
    blueprint: validateBlueprint(parsed.blueprint),
    destination: parsed.destination,
    files,
    format: REVIEW_FORMAT,
    review_id: reviewId,
  };
}

function validateReviewFile(value: unknown): ReviewFile {
  if (
    !isRecord(value) ||
    !sameStrings(Object.keys(value).sort(), ["path", "sha256", "size"]) ||
    typeof value.path !== "string" ||
    !SCAFFOLD_RELATIVE_PATHS.includes(value.path as (typeof SCAFFOLD_RELATIVE_PATHS)[number]) ||
    typeof value.sha256 !== "string" ||
    !/^sha256:[0-9a-f]{64}$/u.test(value.sha256) ||
    typeof value.size !== "number" ||
    !Number.isSafeInteger(value.size) ||
    value.size < 0
  ) {
    throw promptError("WSP-PROMPT-009", "Prompt review file record is invalid");
  }
  return { path: value.path, sha256: value.sha256, size: value.size };
}

async function inspectStagedProject(
  project: string,
  expectedContents: Readonly<Record<string, string>>,
): Promise<ReviewFile[]> {
  const rootStatus = await lstat(project).catch(() => undefined);
  if (!rootStatus || rootStatus.isSymbolicLink() || !rootStatus.isDirectory()) {
    throw promptError("WSP-PROMPT-010", "Staged project root is missing or unsafe");
  }
  const discovered = await walkFiles(project);
  if (!sameStrings(discovered, [...SCAFFOLD_RELATIVE_PATHS].sort())) {
    throw promptError(
      "WSP-PROMPT-010",
      "Staged project contains an unknown or missing path",
      "Only the fixed scaffold file set can be promoted.",
    );
  }
  const result: ReviewFile[] = [];
  let total = 0;
  for (const path of discovered) {
    const target = join(project, path);
    const status = await lstat(target);
    if (status.isSymbolicLink() || !status.isFile()) {
      throw promptError("WSP-PROMPT-010", "Staged project contains a symlink or non-file");
    }
    total += status.size;
    if (total > MAX_PROJECT_BYTES) {
      throw promptError("WSP-PROMPT-010", "Staged project exceeds the bounded size limit");
    }
    const bytes = await readFile(target);
    const expected = expectedContents[path];
    if (expected === undefined || !bytes.equals(Buffer.from(expected, "utf8"))) {
      throw promptError(
        "WSP-PROMPT-010",
        "Staged project content is not the fixed scaffold rendering",
      );
    }
    result.push({
      path,
      sha256: `sha256:${createHash("sha256").update(bytes).digest("hex")}`,
      size: bytes.length,
    });
  }
  return result;
}

async function walkFiles(root: string, prefix = ""): Promise<string[]> {
  const entries = await readdir(join(root, prefix), { withFileTypes: true });
  const files: string[] = [];
  for (const entry of entries) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isSymbolicLink()) {
      throw promptError("WSP-PROMPT-010", "Staged project contains a symlink");
    }
    if (entry.isDirectory()) {
      files.push(...(await walkFiles(root, path)));
    } else if (entry.isFile()) {
      files.push(path);
    } else {
      throw promptError("WSP-PROMPT-010", "Staged project contains an unsupported entry");
    }
  }
  return files.sort();
}

async function prepareDestination(destination: string): Promise<void> {
  const status = await lstat(destination).catch(() => undefined);
  if (status) {
    if (status.isSymbolicLink() || !status.isDirectory()) {
      throw promptError(
        "WSP-PROMPT-011",
        "Prompt project destination must be a real directory",
      );
    }
    if ((await readdir(destination)).length > 0) {
      throw promptError("WSP-PROMPT-011", "Prompt project destination is not empty");
    }
    return;
  }
  await mkdir(destination, { recursive: true });
}

function sameReviewFiles(left: readonly ReviewFile[], right: readonly ReviewFile[]): boolean {
  return (
    left.length === right.length &&
    left.every(
      (item, index) =>
        item.path === right[index]?.path &&
        item.sha256 === right[index]?.sha256 &&
        item.size === right[index]?.size,
    )
  );
}

function validatePrompt(value: string): string {
  const bytes = new TextEncoder().encode(value);
  if (value.trim().length === 0 || bytes.length > MAX_PROMPT_BYTES || value.includes("\0")) {
    throw promptError(
      "WSP-PROMPT-001",
      "Prompt must be non-empty, bounded UTF-8 text",
    );
  }
  return value;
}

function validateEndpoint(value: string): URL {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw promptError("WSP-PROMPT-002", "Prompt endpoint is not a valid URL");
  }
  const local = ["127.0.0.1", "::1", "localhost"].includes(url.hostname.toLowerCase());
  if (
    (url.protocol !== "https:" && !(url.protocol === "http:" && local)) ||
    url.username.length > 0 ||
    url.password.length > 0 ||
    url.hash.length > 0 ||
    url.search.length > 0
  ) {
    throw promptError(
      "WSP-PROMPT-002",
      "Prompt endpoint must be HTTPS, or HTTP on loopback, without credentials or query data",
    );
  }
  return url;
}

function validateModel(value: string): string {
  if (value.length === 0 || value.length > 160 || /[\u0000-\u001f\u007f]/u.test(value)) {
    throw promptError("WSP-PROMPT-003", "Prompt model identifier is invalid");
  }
  return value;
}

function validateApiKey(value: string | undefined): void {
  if (value !== undefined && (value.length > 4096 || /[\r\n\0]/u.test(value))) {
    throw promptError("WSP-PROMPT-003", "Prompt API key configuration is invalid");
  }
}

function sameStrings(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

function invalidBlueprint(): PackCliError {
  return promptError(
    "WSP-PROMPT-006",
    "Generated project blueprint violates the closed scaffold schema",
    "No source files, paths, commands, dependencies, or unknown fields are accepted.",
  );
}

function promptError(code: string, summary: string, detail?: string): PackCliError {
  return new PackCliError(
    diagnostic(code, summary, detail === undefined ? {} : { detail }),
  );
}

const BLUEPRINT_JSON_SCHEMA = {
  additionalProperties: false,
  properties: {
    description: { maxLength: 400, minLength: 1, type: "string" },
    display_name: { maxLength: 80, minLength: 1, type: "string" },
    format: { const: PROMPT_BLUEPRINT_FORMAT, type: "string" },
    template: { const: PROMPT_TEMPLATE, type: "string" },
  },
  required: ["description", "display_name", "format", "template"],
  type: "object",
} as const;
