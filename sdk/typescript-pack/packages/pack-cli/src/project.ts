import { readFile } from "node:fs/promises";
import { isAbsolute, join, relative, resolve } from "node:path";

import { PROJECT_FORMAT_ID } from "./constants.js";
import { PackCliError, diagnostic } from "./diagnostics.js";

export interface PackProject {
  readonly format: typeof PROJECT_FORMAT_ID;
  readonly entrypoint: string;
  readonly output: string;
  readonly goldenFixture: string;
  readonly privacyFixture: string;
}

export interface ResolvedPackProject {
  readonly root: string;
  readonly manifestPath: string;
  readonly config: PackProject;
  readonly entrypoint: string;
  readonly output: string;
  readonly goldenFixture: string;
  readonly privacyFixture: string;
}

const EXPECTED_KEYS = new Set([
  "format",
  "entrypoint",
  "output",
  "goldenFixture",
  "privacyFixture",
]);

export async function loadProject(start = process.cwd()): Promise<ResolvedPackProject> {
  const root = resolve(start);
  const manifestPath = join(root, "worldstream-pack.json");
  let parsed: unknown;
  try {
    parsed = JSON.parse(await readFile(manifestPath, "utf8"));
  } catch (error) {
    throw new PackCliError(
      diagnostic("WSP-PROJECT-001", "Cannot read worldstream-pack.json", {
        file: manifestPath,
        detail: error instanceof Error ? error.message : String(error),
        hint: "Run `worldstream-pack new <directory>` or pass the Activity Pack project directory.",
      }),
    );
  }
  if (!isRecord(parsed)) {
    throw new PackCliError(
      diagnostic("WSP-PROJECT-002", "Project contract must be a JSON object", {
        file: manifestPath,
      }),
    );
  }
  const unknown = Object.keys(parsed).filter((key) => !EXPECTED_KEYS.has(key));
  const required = [...EXPECTED_KEYS].filter((key) => !(key in parsed));
  const diagnostics = [];
  if (unknown.length > 0) {
    diagnostics.push(
      diagnostic("WSP-PROJECT-003", "Project contract has unknown fields", {
        file: manifestPath,
        detail: unknown.sort().join(", "),
        hint: "Remove fields that are not part of worldstream/pack-project/v1.",
      }),
    );
  }
  if (required.length > 0) {
    diagnostics.push(
      diagnostic("WSP-PROJECT-004", "Project contract is missing required fields", {
        file: manifestPath,
        detail: required.sort().join(", "),
      }),
    );
  }
  if (parsed.format !== PROJECT_FORMAT_ID) {
    diagnostics.push(
      diagnostic("WSP-PROJECT-005", "Unsupported project contract version", {
        file: manifestPath,
        detail: `Expected ${PROJECT_FORMAT_ID}.`,
      }),
    );
  }
  for (const key of ["entrypoint", "output", "goldenFixture", "privacyFixture"] as const) {
    if (typeof parsed[key] !== "string" || parsed[key].length === 0) {
      diagnostics.push(
        diagnostic("WSP-PROJECT-006", `${key} must be a non-empty relative path`, {
          file: manifestPath,
        }),
      );
    }
  }
  if (diagnostics.length > 0) throw new PackCliError(diagnostics);
  const config = parsed as unknown as PackProject;
  return {
    root,
    manifestPath,
    config,
    entrypoint: resolveInside(root, config.entrypoint, "entrypoint"),
    output: resolveInside(root, config.output, "output"),
    goldenFixture: resolveInside(root, config.goldenFixture, "goldenFixture"),
    privacyFixture: resolveInside(root, config.privacyFixture, "privacyFixture"),
  };
}

function resolveInside(root: string, input: string, label: string): string {
  const target = resolve(root, input);
  if (isAbsolute(input) || relative(root, target).startsWith("..")) {
    throw new PackCliError(
      diagnostic("WSP-PROJECT-007", `${label} must remain inside the project`, {
        detail: input,
      }),
    );
  }
  return target;
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
