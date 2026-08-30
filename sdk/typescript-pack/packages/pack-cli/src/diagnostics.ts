export type DiagnosticSeverity = "error" | "warning";

export interface OwnedDiagnostic {
  readonly code: string;
  readonly severity: DiagnosticSeverity;
  readonly summary: string;
  readonly detail?: string;
  readonly file?: string;
  readonly line?: number;
  readonly column?: number;
  readonly hint?: string;
}

export class PackCliError extends Error {
  readonly diagnostics: readonly OwnedDiagnostic[];

  constructor(diagnostics: OwnedDiagnostic | readonly OwnedDiagnostic[]) {
    const items = Array.isArray(diagnostics) ? diagnostics : [diagnostics];
    super(items.map((item) => `${item.code}: ${item.summary}`).join("\n"));
    this.name = "PackCliError";
    this.diagnostics = items;
  }
}

export function diagnostic(
  code: string,
  summary: string,
  options: Omit<OwnedDiagnostic, "code" | "severity" | "summary"> & {
    readonly severity?: DiagnosticSeverity;
  } = {},
): OwnedDiagnostic {
  return {
    code,
    severity: options.severity ?? "error",
    summary,
    ...(options.detail === undefined ? {} : { detail: options.detail }),
    ...(options.file === undefined ? {} : { file: options.file }),
    ...(options.line === undefined ? {} : { line: options.line }),
    ...(options.column === undefined ? {} : { column: options.column }),
    ...(options.hint === undefined ? {} : { hint: options.hint }),
  };
}

export function formatDiagnostic(item: OwnedDiagnostic): string {
  const location = item.file
    ? `${item.file}${item.line ? `:${item.line}${item.column ? `:${item.column}` : ""}` : ""}`
    : undefined;
  const lines = [`${item.severity.toUpperCase()} ${item.code}: ${item.summary}`];
  if (location) lines.push(`  at ${location}`);
  if (item.detail) lines.push(`  ${item.detail}`);
  if (item.hint) lines.push(`  Fix: ${item.hint}`);
  return lines.join("\n");
}

export function fail(code: string, summary: string, detail?: string, hint?: string): never {
  throw new PackCliError(
    diagnostic(code, summary, {
      ...(detail === undefined ? {} : { detail }),
      ...(hint === undefined ? {} : { hint }),
    }),
  );
}
