#!/usr/bin/env node
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { PackCliError, formatDiagnostic } from "./diagnostics.js";
import { WorldStreamPackToolchain } from "./toolchain.js";

export async function runCli(argv: readonly string[]): Promise<number> {
  const [command, ...args] = argv;
  const target = args[0];
  if (!command || command === "help" || command === "--help" || command === "-h") {
    process.stdout.write(help());
    return 0;
  }
  const toolchain = new WorldStreamPackToolchain();
  try {
    let receipt: object;
    switch (command) {
      case "new":
        if (!target) throw new Error("new requires a destination directory");
        receipt = await toolchain.new(target);
        break;
      case "create": {
        const create = parseCreateArgs(args);
        receipt = create.prompt !== undefined
          ? await toolchain.createPrompt(create.directory, {
              ...(create.endpoint === undefined ? {} : { endpoint: create.endpoint }),
              ...(create.model === undefined ? {} : { model: create.model }),
              prompt: create.prompt,
            })
          : await toolchain.confirmPrompt(create.directory, create.confirm!);
        break;
      }
      case "check":
        receipt = await toolchain.check(target);
        break;
      case "test":
        receipt = await toolchain.test(target);
        break;
      case "build":
        receipt = await toolchain.build(target);
        break;
      case "inspect":
        receipt = await toolchain.inspect(target);
        break;
      case "prove":
        receipt = await toolchain.prove(target);
        break;
      default:
        throw new Error(`unknown command ${command}`);
    }
    process.stdout.write(`${JSON.stringify(receipt, null, 2)}\n`);
    return 0;
  } catch (error) {
    if (error instanceof PackCliError) {
      process.stderr.write(`${error.diagnostics.map(formatDiagnostic).join("\n\n")}\n`);
      return 1;
    }
    process.stderr.write(`ERROR WSP-CLI-001: ${error instanceof Error ? error.message : String(error)}\n`);
    return 1;
  }
}

function help(): string {
  return `WorldStream Activity Pack toolchain

Usage:
  worldstream-pack new <directory>
  worldstream-pack create <directory> --prompt <description> [--endpoint <url>] [--model <id>]
  worldstream-pack create <directory> --confirm <review-id>
  worldstream-pack check [project-directory]
  worldstream-pack test [project-directory]
  worldstream-pack build [project-directory]
  worldstream-pack inspect [project-directory|bundle.wspack]
  worldstream-pack prove [project-directory]

The CLI owns strict TypeScript compilation, generated WIT, WASI-free
componentization, canonical .wspack layout, deterministic conformance, and
production-host proof diagnostics.

Prompt-assisted creation accepts only a closed structured blueprint and stages
the fixed scaffold under a private temporary root. Review the returned files,
then run the explicit --confirm command to promote them. API keys are read only
from WORLDSTREAM_PACK_OPENAI_API_KEY; endpoint and model may also come from
WORLDSTREAM_PACK_OPENAI_ENDPOINT and WORLDSTREAM_PACK_OPENAI_MODEL.
`;
}

interface ParsedCreateArgs {
  readonly confirm?: string;
  readonly directory: string;
  readonly endpoint?: string;
  readonly model?: string;
  readonly prompt?: string;
}

function parseCreateArgs(args: readonly string[]): ParsedCreateArgs {
  let directory: string | undefined;
  let prompt: string | undefined;
  let confirm: string | undefined;
  let endpoint: string | undefined;
  let model: string | undefined;
  for (let index = 0; index < args.length; index += 1) {
    const token = args[index]!;
    if (token === "--prompt" || token === "--confirm" || token === "--endpoint" || token === "--model") {
      const value = args[index + 1];
      if (value === undefined) throw new Error("create option requires a value");
      index += 1;
      if (token === "--prompt") {
        if (prompt !== undefined) throw new Error("create option was repeated");
        prompt = value;
      } else if (token === "--confirm") {
        if (confirm !== undefined) throw new Error("create option was repeated");
        confirm = value;
      } else if (token === "--endpoint") {
        if (endpoint !== undefined) throw new Error("create option was repeated");
        endpoint = value;
      } else {
        if (model !== undefined) throw new Error("create option was repeated");
        model = value;
      }
    } else if (token.startsWith("--")) {
      throw new Error("create received an unsupported option");
    } else if (directory === undefined) {
      directory = token;
    } else {
      throw new Error("create accepts exactly one destination directory");
    }
  }
  if (!directory) throw new Error("create requires a destination directory");
  if ((prompt === undefined) === (confirm === undefined)) {
    throw new Error("create requires exactly one of --prompt or --confirm");
  }
  if (confirm !== undefined && (endpoint !== undefined || model !== undefined)) {
    throw new Error("provider options are valid only with --prompt");
  }
  return {
    ...(confirm === undefined ? {} : { confirm }),
    directory,
    ...(endpoint === undefined ? {} : { endpoint }),
    ...(model === undefined ? {} : { model }),
    ...(prompt === undefined ? {} : { prompt }),
  };
}

if (isEntrypoint()) {
  process.exitCode = await runCli(process.argv.slice(2));
}

function isEntrypoint(): boolean {
  if (!process.argv[1]) return false;
  try {
    return realpathSync(process.argv[1]) === fileURLToPath(import.meta.url);
  } catch {
    return false;
  }
}
