import type { CanonicalJson, CanonicalObject } from "@worldstream/pack-sdk";

type SummaryField =
  | { readonly output: string; readonly source: string; readonly kind: "enum"; readonly values: readonly string[] }
  | { readonly output: string; readonly source: string; readonly kind: "nullable_identifier"; readonly maximum_bytes: number }
  | { readonly output: string; readonly source: string; readonly kind: "integer"; readonly minimum: number; readonly maximum: number };

export interface RuntimeProjectorRevision {
  readonly program: {
    readonly terminal: { readonly field: string; readonly equals: string };
    readonly outcome_field: string;
    readonly summary_fields: readonly SummaryField[];
  };
  readonly output: { readonly schema: string };
}

export class ProjectorRuntimeViolation extends Error {}

export function interpretProjectorV1(
  revision: RuntimeProjectorRevision,
  value: CanonicalJson | undefined,
): CanonicalObject {
  const projection = record(value);
  const terminal = stringValue(projection[revision.program.terminal.field]);
  if (terminal !== revision.program.terminal.equals) return { status: "not_terminal" };
  const outcomeValue = projection[revision.program.outcome_field];
  if (outcomeValue === null) return { status: "terminal_without_outcome" };
  const outcome = record(outcomeValue);
  const summary: Record<string, CanonicalJson> = { schema: revision.output.schema };
  for (const field of revision.program.summary_fields) {
    const source = outcome[field.source];
    switch (field.kind) {
      case "enum": {
        const selected = stringValue(source);
        if (!field.values.includes(selected)) throw new ProjectorRuntimeViolation();
        summary[field.output] = selected;
        break;
      }
      case "nullable_identifier":
        summary[field.output] = source === null ? null : publicReference(source, field.maximum_bytes);
        break;
      case "integer": {
        const selected = integer(source);
        if (selected < field.minimum || selected > field.maximum) throw new ProjectorRuntimeViolation();
        summary[field.output] = selected;
        break;
      }
    }
  }
  return { status: "summary", summary };
}

function record(value: CanonicalJson | undefined): CanonicalObject {
  if (value === null || value === undefined || Array.isArray(value) || typeof value !== "object") {
    throw new ProjectorRuntimeViolation();
  }
  return value as CanonicalObject;
}

function stringValue(value: CanonicalJson | undefined): string {
  if (typeof value !== "string") throw new ProjectorRuntimeViolation();
  return value;
}

function integer(value: CanonicalJson | undefined): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) throw new ProjectorRuntimeViolation();
  return value;
}

function publicReference(value: CanonicalJson | undefined, maximumBytes: number): string {
  const selected = stringValue(value);
  if (
    selected.length === 0
    || new TextEncoder().encode(selected).byteLength > maximumBytes
    || !/^[A-Za-z0-9][A-Za-z0-9._:-]*$/.test(selected)
  ) {
    throw new ProjectorRuntimeViolation();
  }
  return selected;
}
