import { cp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { componentize, version as componentizeVersion } from "@bytecodealliance/componentize-js";
import { componentWit } from "@bytecodealliance/jco";
import ts from "typescript";

import { REQUIRED_COMPONENT_EXPORTS, TOOLCHAIN_VERSIONS } from "./constants.js";
import { PackCliError, diagnostic } from "./diagnostics.js";
import type { ResolvedPackProject } from "./project.js";
import type { JsonValue } from "./canonical.js";

export interface ComponentBuild {
  readonly bytes: Uint8Array;
  readonly wit: string;
  readonly imports: readonly string[];
  readonly exports: readonly string[];
  readonly wrapperPath: string;
}

export async function buildComponent(
  project: ResolvedPackProject,
  generatedDir: string,
  descriptorContent: JsonValue,
  schemaIds: Readonly<Record<string, string>>,
  actionDigests: Readonly<Record<string, string>>,
): Promise<ComponentBuild> {
  if (componentizeVersion !== TOOLCHAIN_VERSIONS.componentizeJs) {
    throw new PackCliError(
      diagnostic("WSP-TOOLCHAIN-002", "The pinned ComponentizeJS compiler is not active", {
        detail: `Expected ${TOOLCHAIN_VERSIONS.componentizeJs}; loaded ${componentizeVersion}.`,
      }),
    );
  }
  await mkdir(generatedDir, { recursive: true });
  const wrapperPath = `${generatedDir}/activity-pack-wrapper.ts`;
  const witPath = `${generatedDir}/worldstream-activity-pack.wit`;
  const packageWit = fileURLToPath(new URL("../assets/worldstream-activity-pack.wit", import.meta.url));
  const wit = await readFile(packageWit, "utf8");
  const entrypoint = relative(dirname(wrapperPath), project.entrypoint)
    .replaceAll("\\", "/")
    .replace(/\.tsx?$/u, ".js");
  const importPath = entrypoint.startsWith(".") ? entrypoint : `./${entrypoint}`;
  await writeFile(
    wrapperPath,
    wrapperSource(importPath, descriptorContent, schemaIds, actionDigests),
    "utf8",
  );
  await writeFile(witPath, wit, "utf8");
  const componentJs = join(project.root, ".worldstream", "component-js");
  await rm(componentJs, { force: true, recursive: true });
  const emittedWrapper = join(componentJs, relative(project.root, wrapperPath)).replace(/\.ts$/u, ".js");
  const program = ts.createProgram([wrapperPath], {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.NodeNext,
    moduleResolution: ts.ModuleResolutionKind.NodeNext,
    rootDir: project.root,
    outDir: componentJs,
    noCheck: true,
    skipLibCheck: true,
  });
  const emit = program.emit();
  if (emit.emitSkipped) {
    throw new PackCliError(
      diagnostic("WSP-COMPONENT-003", "Generated Component wrapper could not be emitted", {
        file: wrapperPath,
      }),
    );
  }
  await vendorPublicSdkRuntime(componentJs);
  let output;
  try {
    output = await componentize({
      sourcePath: emittedWrapper,
      witPath,
      worldName: "activity-pack-v1",
      disableFeatures: ["stdio", "random", "clocks", "http", "fetch-event"],
    });
  } catch (error) {
    throw new PackCliError(
      diagnostic("WSP-COMPONENT-001", "Activity Pack componentization failed", {
        file: project.entrypoint,
        detail: error instanceof Error ? error.message : String(error),
        hint: "Run `worldstream-pack check`; raw Jco or ComponentizeJS invocation is not required.",
      }),
    );
  }
  const bytes = new Uint8Array(output.component);
  const componentInterface = await componentWit(bytes);
  const imports = [...componentInterface.matchAll(/^\s*import\s+([^;{]+)/gmu)].map((match) =>
    match[1]!.trim(),
  );
  const exports = [...componentInterface.matchAll(/^\s*export\s+([a-z][a-z0-9-]*):\s*func/gmu)].map(
    (match) => match[1]!,
  );
  const expected = [...REQUIRED_COMPONENT_EXPORTS].sort();
  if (imports.length > 0 || JSON.stringify([...exports].sort()) !== JSON.stringify(expected)) {
    throw new PackCliError(
      diagnostic("WSP-COMPONENT-002", "Generated Component violates the frozen host interface", {
        detail: `imports=${JSON.stringify(imports)} exports=${JSON.stringify(exports)}`,
        hint: "Keep the generated wrapper and WIT under @worldstream/pack-cli control.",
      }),
    );
  }
  return { bytes, wit: componentInterface, imports, exports, wrapperPath };
}

async function vendorPublicSdkRuntime(componentJs: string): Promise<void> {
  const sdkSource = fileURLToPath(import.meta.resolve("@worldstream/pack-sdk"));
  const nobleSource = dirname(
    fileURLToPath(import.meta.resolve("@noble/hashes/blake3.js")),
  );
  const vendorRoot = join(componentJs, ".worldstream", "vendor");
  const sdkTarget = join(vendorRoot, "pack-sdk.js");
  const nobleTarget = join(vendorRoot, "noble");
  const emittedSources = await javascriptFiles(componentJs);
  await mkdir(vendorRoot, { recursive: true });
  const sdkRuntime = (await readFile(sdkSource, "utf8"))
    .replaceAll('"@noble/hashes/blake3.js"', '"./noble/blake3.js"')
    .replaceAll('"@noble/hashes/utils.js"', '"./noble/utils.js"');
  await writeFile(sdkTarget, sdkRuntime, "utf8");
  await cp(nobleSource, nobleTarget, { recursive: true, force: true });
  for (const file of emittedSources) {
    const relativeSdk = relative(dirname(file), sdkTarget).replaceAll("\\", "/");
    const specifier = relativeSdk.startsWith(".") ? relativeSdk : `./${relativeSdk}`;
    const source = await readFile(file, "utf8");
    const rewritten = source
      .replaceAll('"@worldstream/pack-sdk"', JSON.stringify(specifier))
      .replaceAll("'@worldstream/pack-sdk'", JSON.stringify(specifier));
    if (rewritten !== source) await writeFile(file, rewritten, "utf8");
  }
}

async function javascriptFiles(root: string): Promise<string[]> {
  const result: string[] = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) result.push(...await javascriptFiles(path));
    else if (entry.isFile() && entry.name.endsWith(".js")) result.push(path);
  }
  return result;
}

function wrapperSource(
  entrypoint: string,
  descriptorContent: JsonValue,
  schemaIds: Readonly<Record<string, string>>,
  actionDigests: Readonly<Record<string, string>>,
): string {
  return `// Generated by @worldstream/pack-cli. Do not edit.
import pack from ${JSON.stringify(entrypoint)};

const DESCRIPTOR = ${JSON.stringify(descriptorContent)};
const SCHEMA_IDS = ${JSON.stringify(schemaIds)};
const ACTION_DIGESTS = ${JSON.stringify(actionDigests)};
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

function stringify(value: unknown): string {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value)) throw new TypeError("unsafe canonical integer");
    return String(value);
  }
  if (Array.isArray(value)) return \`[\${value.map(stringify).join(",")}]\`;
  if (typeof value === "object") {
    const record = value as Record<string, unknown>;
    return \`{\${Object.keys(record).sort().map((key) => \`\${JSON.stringify(key)}:\${stringify(record[key])}\`).join(",")}\}\`;
  }
  throw new TypeError("unsupported canonical JSON value");
}

function encode(value: unknown): Uint8Array {
  return encoder.encode(stringify(value));
}

function decode(input: Uint8Array): Record<string, unknown> {
  const text = decoder.decode(input);
  const value = JSON.parse(text) as Record<string, unknown>;
  if (stringify(value) !== text) throw new TypeError("operation input is not canonical");
  return value;
}

function invoke(callback: (input: Record<string, unknown>) => unknown, input: Uint8Array): Uint8Array {
  try {
    return encode({ operation_result_type: "success", output: callback(decode(input)) });
  } catch {
    return encode({
      operation_result_type: "fault",
      fault: { callback_fault_type: "callback", bounded_safe_detail: "Activity Pack callback failed" },
    });
  }
}

function normalizeView(output: unknown): unknown {
  const value = output as { projection_schema: string; projection: unknown; action_offers: string[] };
  return {
    projection_schema: SCHEMA_IDS[\`projection:\${value.projection_schema}\`],
    projection: value.projection,
    action_offers: value.action_offers.map((actionType) => ({
      action_type: actionType,
      domain: "worldstream/action-offer/v1",
      eligibility_window: null,
      payload_schema_digest: ACTION_DIGESTS[actionType],
    })),
  };
}

function normalizeObservation(output: unknown): unknown {
  if (output === null) return null;
  const value = output as { observation_schema: string; observation: unknown; action_offers: string };
  return {
    observation_schema: SCHEMA_IDS[\`observation:\${value.observation_schema}\`],
    observation: value.observation,
    action_offers: value.action_offers,
  };
}

export function descriptor(): Uint8Array {
  return encode(DESCRIPTOR);
}

export function initialize(input: Uint8Array): Uint8Array {
  return invoke(pack.initialize, input);
}

export function reduce(input: Uint8Array): Uint8Array {
  return invoke(pack.reduce, input);
}

export function view(input: Uint8Array): Uint8Array {
  return invoke((request) => normalizeView(pack.view(request)), input);
}

export function observe(input: Uint8Array): Uint8Array {
  return invoke((request) => normalizeObservation(pack.observe(request)), input);
}
`;
}
