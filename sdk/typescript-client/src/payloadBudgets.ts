/** Explicit fresh-work checks for compact Rooms. Accepted retries keep their original contract. */
export const PAYLOAD_BUDGET_V1_ID = "worldstream/payload-budget/v1" as const;
export const PAYLOAD_BUDGET_V1_LIMITS = Object.freeze({
  control_metadata: 4096, action_payload: 32768,
  external_input_payload: 32768, creation_configuration: 32768,
  domain_event_item: 8192, domain_events_array: 262144,
  timer_change_item: 4096, timer_changes_array: 65536,
  attention_signal_item: 4096, attention_signals_array: 65536,
  effects: 393216, transition: 524288,
  activity_state: 262144, core_state: 131072,
  authoritative_state: 524288, genesis: 786432,
  observation: 32768, projection: 262144, artifact_reference: 2048,
});
export type FreshPayloadKindV1 = Exclude<keyof typeof PAYLOAD_BUDGET_V1_LIMITS, "artifact_reference">;

const encoder = new TextEncoder();
function validString(value: string): void {
  for (let index = 0; index < value.length; index += 1) {
    const unit = value.charCodeAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const low = value.charCodeAt(++index);
      if (!(low >= 0xdc00 && low <= 0xdfff)) throw new Error("unpaired Unicode surrogate");
    } else if (unit >= 0xdc00 && unit <= 0xdfff) {
      throw new Error("unpaired Unicode surrogate");
    }
  }
}
function compareUtf8(left: string, right: string): number {
  const a = encoder.encode(left);
  const b = encoder.encode(right);
  for (let index = 0; index < Math.min(a.length, b.length); index += 1) {
    if (a[index] !== b[index]) return a[index]! - b[index]!;
  }
  return a.length - b.length;
}
function canonical(value: unknown, ancestors: Set<object>): string {
  if (value === null || typeof value === "boolean") return JSON.stringify(value);
  if (typeof value === "string") { validString(value); return JSON.stringify(value); }
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value)) throw new Error("canonical values require safe integers");
    return JSON.stringify(value);
  }
  if (typeof value !== "object" || ancestors.has(value)) throw new Error("value is not acyclic canonical JSON");
  ancestors.add(value);
  try {
    if (Array.isArray(value)) {
      const items: string[] = [];
      for (let index = 0; index < value.length; index += 1) {
        if (!Object.hasOwn(value, index)) throw new Error("sparse arrays are not canonical JSON");
        items.push(canonical(value[index], ancestors));
      }
      return `[${items.join(",")}]`;
    }
    if (Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) {
      throw new Error("canonical objects must be plain JSON objects");
    }
    if (Object.getOwnPropertySymbols(value).length !== 0) throw new Error("symbol keys are not canonical JSON");
    const record = value as Record<string, unknown>;
    const keys = Object.keys(record);
    keys.forEach(validString);
    keys.sort(compareUtf8);
    return `{${keys.map((key) => `${JSON.stringify(key)}:${canonical(record[key], ancestors)}`).join(",")}}`;
  } finally { ancestors.delete(value); }
}

/** Exact canonical UTF-8 bytes. This does not replace the existing transport wire guard. */
export function canonicalPayloadBytes(value: unknown): Uint8Array {
  return encoder.encode(canonical(value, new Set()));
}

function checkPolicy(policyId: string): void {
  if (policyId !== PAYLOAD_BUDGET_V1_ID) throw new Error("unsupported payload policy identity");
}
/** Opt-in local check for unresolved fresh work. It grants no authority or retry disposition. */
export function checkFreshPayload(value: unknown, kind: FreshPayloadKindV1, policyId: string): number {
  checkPolicy(policyId);
  if (typeof kind !== "string" || !Object.hasOwn(PAYLOAD_BUDGET_V1_LIMITS, kind) || (kind as string) === "artifact_reference") {
    throw new Error("unknown fresh payload kind; Artifact references need an explicit schema");
  }
  const count = canonicalPayloadBytes(value).byteLength;
  const maximum = PAYLOAD_BUDGET_V1_LIMITS[kind];
  if (count > maximum) throw new Error(`${kind} has ${count} canonical bytes; limit is ${maximum}`);
  return count;
}
/** The application declares and validates the reference schema; this grants no download authority. */
export function checkDeclaredArtifactReference(value: unknown, schemaId: string, policyId: string): number {
  checkPolicy(policyId);
  if (typeof schemaId !== "string" || schemaId.length === 0) throw new Error("explicit Artifact reference schema identity required");
  const count = canonicalPayloadBytes(value).byteLength;
  const maximum = PAYLOAD_BUDGET_V1_LIMITS.artifact_reference;
  if (count > maximum) throw new Error(`artifact_reference has ${count} canonical bytes; limit is ${maximum}`);
  return count;
}
