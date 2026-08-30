import { access, mkdir, readFile, rm } from "node:fs/promises";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import ts from "typescript";

import { TOOLCHAIN_VERSIONS } from "./constants.js";
import { PackCliError, diagnostic, type OwnedDiagnostic } from "./diagnostics.js";
import type { ResolvedPackProject } from "./project.js";

const BANNED_GLOBALS = new Set([
  "Date",
  "fetch",
  "WebSocket",
  "XMLHttpRequest",
  "process",
  "globalThis",
  "crypto",
  "performance",
  "setTimeout",
  "setInterval",
  "queueMicrotask",
]);

export interface CompileResult {
  readonly program: ts.Program;
  readonly sourceFiles: readonly ts.SourceFile[];
}

export async function checkTypeScript(project: ResolvedPackProject): Promise<CompileResult> {
  if (ts.version !== TOOLCHAIN_VERSIONS.typescript) {
    throw new PackCliError(
      diagnostic("WSP-TOOLCHAIN-001", "The pinned TypeScript compiler is not active", {
        detail: `Expected ${TOOLCHAIN_VERSIONS.typescript}; loaded ${ts.version}.`,
      }),
    );
  }
  const configPath = join(project.root, "tsconfig.json");
  const configFile = ts.readConfigFile(configPath, ts.sys.readFile);
  if (configFile.error) throw new PackCliError([typescriptDiagnostic(configFile.error)]);
  const parsed = ts.parseJsonConfigFileContent(configFile.config, ts.sys, project.root, {
    noEmit: true,
    strict: true,
    noImplicitAny: true,
    noUncheckedIndexedAccess: true,
    exactOptionalPropertyTypes: true,
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.NodeNext,
    moduleResolution: ts.ModuleResolutionKind.NodeNext,
    typeRoots: [
      join(project.root, "node_modules", "@types"),
      fileURLToPath(new URL("../node_modules/@types", import.meta.url)),
    ],
  });
  const program = ts.createProgram(parsed.fileNames, parsed.options);
  const diagnostics = ts.getPreEmitDiagnostics(program).map(typescriptDiagnostic);
  const sourceFiles = program
    .getSourceFiles()
    .filter((source) => !source.isDeclarationFile && isInside(project.root, source.fileName));
  diagnostics.push(...analyzeCapabilities(project, sourceFiles));
  if (diagnostics.length > 0) throw new PackCliError(diagnostics);
  return { program, sourceFiles };
}

export async function emitForExecution(
  project: ResolvedPackProject,
  directoryName: string,
): Promise<{
  readonly outDir: string;
  readonly entrypoint: string;
  readonly golden: string;
  readonly privacy: string;
  readonly customTest?: string;
}> {
  await checkTypeScript(project);
  const outDir = join(project.root, ".worldstream", directoryName);
  await rm(outDir, { recursive: true, force: true });
  await mkdir(outDir, { recursive: true });
  const customTest = join(project.root, "test", "pack.test.ts");
  const roots = [project.entrypoint, project.goldenFixture, project.privacyFixture];
  try {
    await access(customTest);
    roots.push(customTest);
  } catch {
    // A custom test is optional for hand-authored projects. The scaffold includes one.
  }
  const options: ts.CompilerOptions = {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.NodeNext,
    moduleResolution: ts.ModuleResolutionKind.NodeNext,
    strict: true,
    noUncheckedIndexedAccess: true,
    exactOptionalPropertyTypes: true,
    skipLibCheck: true,
    rootDir: project.root,
    outDir,
    declaration: false,
    sourceMap: false,
    typeRoots: [
      join(project.root, "node_modules", "@types"),
      fileURLToPath(new URL("../node_modules/@types", import.meta.url)),
    ],
  };
  const program = ts.createProgram(roots, options);
  const result = program.emit();
  const diagnostics = [...ts.getPreEmitDiagnostics(program), ...result.diagnostics].map(typescriptDiagnostic);
  if (diagnostics.length > 0) throw new PackCliError(diagnostics);
  return {
    outDir,
    entrypoint: emittedPath(project.root, outDir, project.entrypoint),
    golden: emittedPath(project.root, outDir, project.goldenFixture),
    privacy: emittedPath(project.root, outDir, project.privacyFixture),
    ...(roots.includes(customTest) ? { customTest: emittedPath(project.root, outDir, customTest) } : {}),
  };
}

export async function readEntrypoint(project: ResolvedPackProject): Promise<string> {
  try {
    return await readFile(project.entrypoint, "utf8");
  } catch (error) {
    throw new PackCliError(
      diagnostic("WSP-SOURCE-001", "Cannot read the configured Activity Pack entrypoint", {
        file: project.entrypoint,
        detail: error instanceof Error ? error.message : String(error),
      }),
    );
  }
}

function analyzeCapabilities(
  project: ResolvedPackProject,
  sourceFiles: readonly ts.SourceFile[],
): OwnedDiagnostic[] {
  const results: OwnedDiagnostic[] = [];
  for (const source of sourceFiles) {
    const normalized = relative(project.root, source.fileName);
    if (!normalized.startsWith("src/")) continue;
    for (const statement of source.statements) {
      if (ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier)) {
        const specifier = statement.moduleSpecifier.text;
        const publicSdk = specifier === "@worldstream/pack-sdk";
        if (!specifier.startsWith(".") && !publicSdk) {
          results.push(
            nodeDiagnostic(
              "WSP-CAPABILITY-001",
              "Pack source imports an ambient or unowned module",
              source,
              statement.moduleSpecifier,
              `Import ${JSON.stringify(specifier)} is not available inside a WASI-free Activity Pack.`,
              "Use relative deterministic modules or the public deterministic @worldstream/pack-sdk surface.",
            ),
          );
        }
      }
      if (ts.isVariableStatement(statement)) {
        const isConst = (statement.declarationList.flags & ts.NodeFlags.Const) !== 0;
        if (!isConst || statement.declarationList.declarations.some((item) => !isStaticInitializer(item.initializer))) {
          results.push(
            nodeDiagnostic(
              "WSP-DETERMINISM-001",
              "Mutable module state is forbidden",
              source,
              statement,
              "A fresh Component instance is created for each callback; module state is not a durable state channel.",
              "Move changing data into the returned Activity State. Keep only primitive compile-time constants at module scope.",
            ),
          );
        }
      }
    }
    const visit = (node: ts.Node): void => {
      if (ts.isIdentifier(node) && BANNED_GLOBALS.has(node.text) && isRuntimeIdentifier(node)) {
        results.push(
          nodeDiagnostic(
            "WSP-CAPABILITY-002",
            `Ambient API ${node.text} is forbidden`,
            source,
            node,
            "Activity Pack callbacks receive no clock, network, filesystem, process, or entropy authority.",
            "Use only request data and deterministic helpers supplied by WorldStream.",
          ),
        );
      }
      if (
        ts.isPropertyAccessExpression(node) &&
        ts.isIdentifier(node.expression) &&
        node.expression.text === "Math" &&
        node.name.text === "random"
      ) {
        results.push(
          nodeDiagnostic(
            "WSP-DETERMINISM-002",
            "Math.random() is forbidden",
            source,
            node,
            "Ambient entropy would make replay diverge.",
            "Use a host-provided deterministic value from the operation request.",
          ),
        );
      }
      if (ts.isNumericLiteral(node)) {
        const value = Number(node.text);
        if (!Number.isSafeInteger(value)) {
          results.push(
            nodeDiagnostic(
              "WSP-CANONICAL-001",
              "Numeric literal is outside the canonical safe-integer range",
              source,
              node,
              node.text,
              "Represent large identifiers as strings.",
            ),
          );
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  return deduplicate(results);
}

function isStaticInitializer(node: ts.Expression | undefined): boolean {
  if (!node) return false;
  const value = unwrapExpression(node);
  return (
    ts.isStringLiteral(value) ||
    ts.isNumericLiteral(value) ||
    value.kind === ts.SyntaxKind.TrueKeyword ||
    value.kind === ts.SyntaxKind.FalseKeyword ||
    value.kind === ts.SyntaxKind.NullKeyword
  );
}

function unwrapExpression(node: ts.Expression): ts.Expression {
  if (ts.isAsExpression(node) || ts.isSatisfiesExpression(node) || ts.isParenthesizedExpression(node)) {
    return unwrapExpression(node.expression);
  }
  return node;
}

function isRuntimeIdentifier(node: ts.Identifier): boolean {
  const parent = node.parent;
  if (
    (ts.isPropertyAccessExpression(parent) && parent.name === node) ||
    (ts.isPropertyAssignment(parent) && parent.name === node) ||
    (ts.isTypeReferenceNode(parent) && parent.typeName === node) ||
    (ts.isImportSpecifier(parent) || ts.isImportClause(parent))
  ) {
    return false;
  }
  return true;
}

function nodeDiagnostic(
  code: string,
  summary: string,
  source: ts.SourceFile,
  node: ts.Node,
  detail: string,
  hint: string,
): OwnedDiagnostic {
  const position = source.getLineAndCharacterOfPosition(node.getStart(source));
  return diagnostic(code, summary, {
    file: source.fileName,
    line: position.line + 1,
    column: position.character + 1,
    detail,
    hint,
  });
}

function typescriptDiagnostic(item: ts.Diagnostic): OwnedDiagnostic {
  const message = ts.flattenDiagnosticMessageText(item.messageText, "\n");
  if (!item.file || item.start === undefined) {
    return diagnostic("WSP-TYPESCRIPT-001", "Strict TypeScript compilation failed", {
      detail: `TS${item.code}: ${message}`,
    });
  }
  const position = item.file.getLineAndCharacterOfPosition(item.start);
  return diagnostic("WSP-TYPESCRIPT-001", "Strict TypeScript compilation failed", {
    file: item.file.fileName,
    line: position.line + 1,
    column: position.character + 1,
    detail: `TS${item.code}: ${message}`,
  });
}

function emittedPath(root: string, outDir: string, source: string): string {
  return join(outDir, relative(root, source).replace(/\.tsx?$/u, ".js"));
}

function isInside(root: string, file: string): boolean {
  const path = relative(resolve(root), resolve(file));
  return path === "" || (!path.startsWith("..") && !path.startsWith(`/`) && !path.startsWith(`\\`));
}

function deduplicate(items: readonly OwnedDiagnostic[]): OwnedDiagnostic[] {
  const seen = new Set<string>();
  return items.filter((item) => {
    const key = `${item.code}:${item.file}:${item.line}:${item.column}:${item.detail}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
