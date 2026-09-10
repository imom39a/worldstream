import { blake3 } from "@noble/hashes/blake3.js";
import { bytesToHex } from "@noble/hashes/utils.js";

export const ACTIVITY_PACK_HOST_CONTRACT_ID = "worldstream/activity-pack/v1" as const;
export const ACTIVITY_PACK_OPERATION_CODEC_ID =
  "worldstream/activity-pack-operation-codec/v1" as const;
export const CANONICAL_JSON_CODEC_ID = "worldstream/canonical-json/v1" as const;
export const COMPONENT_EXECUTION_PROFILE_ID =
  "worldstream/component-deterministic/v1" as const;
/**
 * The sole v1 hosted-start declaration.  The Host supplies the source ID;
 * Pack authors can only bind one exact pre-start transition.
 */
export const ACTIVITY_START_CONTRACT_ID = "worldstream/activity-start/v1" as const;
/** Fixed Host-owned source for every v1 Activity Start Contract. */
export const ACTIVITY_START_SOURCE_ID = "01ARZ3NDEKTSV4RRFFQ69G5FH1" as const;

export type CanonicalJson =
  | null
  | boolean
  | number
  | string
  | readonly CanonicalJson[]
  | { readonly [key: string]: CanonicalJson };

export type CanonicalObject = { readonly [key: string]: CanonicalJson };

export type PackCallbackFault =
  | {
      readonly callback_fault_type: "callback";
      readonly bounded_safe_detail: string;
    }
  | {
      readonly callback_fault_type: "privacy_contract";
      readonly bounded_safe_detail: string;
    };

export type PackOperationResult<T extends CanonicalJson> =
  | { readonly operation_result_type: "success"; readonly output: T }
  | { readonly operation_result_type: "fault"; readonly fault: PackCallbackFault };

export type PackFaultResult = {
  readonly operation_result_type: "fault";
  readonly fault: PackCallbackFault;
};

export interface ActivityPackExports {
  descriptor(): Uint8Array;
  initialize(input: Uint8Array): Uint8Array;
  reduce(input: Uint8Array): Uint8Array;
  view(input: Uint8Array): Uint8Array;
  observe(input: Uint8Array): Uint8Array;
}

export interface ActivityPackDescriptorDraft {
  readonly packId: string;
  readonly name: string;
  readonly version: string;
  /**
   * Version suffix for every generated schema ID in this Pack revision.
   * Defaults to 1. Increment it when a client-visible schema changes incompatibly.
   */
  readonly schemaVersion?: number;
  /**
   * A string preserves the first authoring form and means exactly one enabled
   * participant. The object form declares a bounded cardinality explicitly.
   */
  readonly roles: readonly ActivityPackRoleDraft[];
  /** A string preserves the first authoring form and uses an object payload schema. */
  readonly actions: readonly ActivityPackActionDraft[];
  readonly rejectionCodes: readonly string[];
  readonly events: readonly ActivityPackEventDraft[];
  readonly attentionReasons: readonly string[];
  /** Defaults to the legacy object schema when omitted. */
  readonly configurationSchema?: ActivityPackSchema;
  /** Defaults to the legacy object schema when omitted. */
  readonly stateSchema?: ActivityPackSchema;
  /**
   * Audience-specific schemas for Pack views. Omitted audience entries use
   * the legacy public or participant object schema as appropriate.
   */
  readonly projectionSchemas?: Readonly<Partial<Record<ActivityPackViewerClass, ActivityPackSchema>>>;
  /** Audience-specific schemas for Pack observations. */
  readonly observationSchemas?: Readonly<Partial<Record<ActivityPackViewerClass, ActivityPackSchema>>>;
  /** Exact schemas for predefined ExternalInput kinds. */
  readonly externalInputSchemas?: Readonly<Record<string, ActivityPackSchema>>;
  /**
   * Optional bounded Host start transition. Omission preserves the legacy
   * descriptor form and grants no start compatibility.
   */
  readonly activityStartContract?: ActivityPackStartContractDraft;
}

/** One exact metadata-derived ExternalInput that may leave a pre-start phase. */
export interface ActivityPackStartContractDraft {
  readonly contract: typeof ACTIVITY_START_CONTRACT_ID;
  /** The root `Activity State.phase` value required before the transition. */
  readonly preStartPhase: string;
  /** A key declared in `externalInputSchemas`. */
  readonly inputType: string;
  /** Canonical immutable payload matched against that input type's schema. */
  readonly canonicalPayload: CanonicalJson;
}

/** The supported canonical JSON Schema document shape. */
export type ActivityPackSchema = CanonicalObject;

export type ActivityPackViewerClass =
  | "public"
  | "participant"
  | "operator"
  | "historical_public"
  | "historical_participant"
  | "historical_operator"
  | "final_reveal";

export type ActivityPackRoleDraft =
  | string
  | Readonly<{
      readonly role: string;
      readonly minimum: number;
      readonly maximum: number;
    }>;

export type ActivityPackActionDraft =
  | string
  | Readonly<{
      readonly actionType: string;
      readonly payloadSchema: ActivityPackSchema;
    }>;

export type ActivityPackEventDraft =
  | string
  | Readonly<{
      readonly eventType: string;
      readonly payloadSchema: ActivityPackSchema;
    }>;

export interface ActivityPackDefinition {
  readonly descriptor: ActivityPackDescriptorDraft;
  initialize(input: CanonicalObject): PackInitializeOutput;
  reduce(input: CanonicalObject): PackReduceOutput;
  view(input: CanonicalObject): PackViewOutput;
  observe(input: CanonicalObject): PackObserveOutput;
}

export type PackInitializeOutput = CanonicalObject & {
  readonly initial_activity_state: CanonicalJson;
  readonly timer_requests: readonly CanonicalJson[];
};

export type PackReduceOutput =
  | (CanonicalObject & {
      readonly activity_disposition_type: "apply";
      readonly next_activity_state: CanonicalJson;
      readonly ordered_domain_events: readonly CanonicalJson[];
      readonly timer_requests: readonly CanonicalJson[];
      readonly ordered_attention_signals: readonly CanonicalJson[];
    })
  | (CanonicalObject & {
      readonly activity_disposition_type: "reject";
      readonly declared_code: string;
      readonly bounded_safe_details: CanonicalJson;
    });

export type PackViewOutput = CanonicalObject & {
  readonly projection_schema: ActivityPackViewerClass;
  readonly projection: CanonicalObject;
  readonly action_offers: readonly PackActionOffer[];
};

/**
 * A string remains the legacy shorthand. An object can bind the offer to one
 * recorded semantic-time window; the Host owns admission against that window.
 */
export type PackActionOffer =
  | string
  | Readonly<{
      readonly actionType: string;
      readonly eligibilityWindow: null | Readonly<{
        readonly opensAt: string;
        readonly deadline: string;
      }>;
    }>;

export type PackObserveOutput =
  | null
  | (CanonicalObject & {
      readonly observation_schema: ActivityPackViewerClass;
      readonly observation: CanonicalObject;
      readonly action_offers: "unchanged" | "reuse_after_view";
    });

export function defineActivityPack<T extends ActivityPackDefinition>(definition: T): T {
  return definition;
}

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder("utf-8", { fatal: true });

export function canonicalStringify(value: CanonicalJson): string {
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value)) {
      throw new TypeError("WorldStream canonical JSON permits safe integers only");
    }
    return String(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(canonicalStringify).join(",")}]`;
  }
  if (typeof value === "object") {
    const record = value as Readonly<Record<string, CanonicalJson>>;
    return `{${Object.keys(record)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalStringify(record[key]!)}`)
      .join(",")}}`;
  }
  throw new TypeError(`unsupported canonical JSON value: ${typeof value}`);
}

export function encodeCanonical(value: CanonicalJson): Uint8Array {
  return textEncoder.encode(canonicalStringify(value));
}

/** BLAKE3 identity of exact bytes, using the retained WorldStream wire tag. */
export function taggedBlake3(bytes: Uint8Array): string {
  return `blake3:${bytesToHex(blake3(bytes))}`;
}

/** BLAKE3 identity of exact UTF-8 text without JSON reserialization. */
export function taggedBlake3Text(value: string): string {
  return taggedBlake3(textEncoder.encode(value));
}

export function decodeCanonical<T extends CanonicalJson>(bytes: Uint8Array): T {
  const text = textDecoder.decode(bytes);
  const parsed: unknown = JSON.parse(text);
  assertCanonicalJson(parsed, "$", 0);
  if (canonicalStringify(parsed) !== text) {
    throw new TypeError("input is valid JSON but not canonical WorldStream JSON");
  }
  return parsed as T;
}

export function success<T extends CanonicalJson>(output: T): PackOperationResult<T> {
  return { operation_result_type: "success", output };
}

export function callbackFault(detail: string): PackFaultResult {
  return {
    operation_result_type: "fault",
    fault: { callback_fault_type: "callback", bounded_safe_detail: boundedDetail(detail) },
  };
}

export function privacyFault(detail: string): PackFaultResult {
  return {
    operation_result_type: "fault",
    fault: {
      callback_fault_type: "privacy_contract",
      bounded_safe_detail: boundedDetail(detail),
    },
  };
}

export function assertSafeInteger(value: number, label: string): number {
  if (!Number.isSafeInteger(value)) {
    throw new TypeError(`${label} must be a safe integer`);
  }
  return value;
}

function boundedDetail(detail: string): string {
  const normalized = detail.replace(/[\u0000-\u001f\u007f]/gu, " ").trim();
  return normalized.slice(0, 256) || "Activity Pack callback failed";
}

function assertCanonicalJson(value: unknown, path: string, depth: number): asserts value is CanonicalJson {
  if (depth > 64) {
    throw new TypeError(`${path} exceeds the canonical nesting limit`);
  }
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    return;
  }
  if (typeof value === "number") {
    assertSafeInteger(value, path);
    return;
  }
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertCanonicalJson(item, `${path}[${index}]`, depth + 1));
    return;
  }
  if (typeof value === "object") {
    for (const [key, item] of Object.entries(value)) {
      assertCanonicalJson(item, `${path}.${key}`, depth + 1);
    }
    return;
  }
  throw new TypeError(`${path} contains unsupported ${typeof value}`);
}
