import {
  canonicalStringify,
  encodeCanonical,
  type CanonicalJson,
  type CanonicalObject,
  type PackActionOffer,
} from "@worldstream/pack-sdk";

const RUNNER_INSTRUCTION_BYTES = 4096;
const RUNNER_MAX_CONTEXT_BYTES = 32768;
const RUNNER_MAX_OPEN_WORK = 16;
const RUNNER_MAX_PROJECTION_BYTES = 24576;
const RUNNER_MAX_RECENT_OUTCOMES = 4;
const RUNNER_MAX_RECENT_OUTCOME_BYTES = 512;
const RUNNER_MAX_ACTION_SCHEMA_BYTES = 4096;
const RUNNER_MAX_TOKENS = 8192;
const RUNNER_RULE_BRIEF_BYTES = 4096;

export interface RunnerProjection {
  readonly action_offers: readonly PackActionOffer[];
  readonly projection: CanonicalObject;
  readonly projection_schema: string;
}

export interface RunnerContext {
  readonly action_schemas: Readonly<Record<string, CanonicalObject>>;
  readonly encoded_bytes: {
    readonly action_schemas: number;
    readonly instructions: number;
    readonly projection: number;
    readonly recent_outcomes: number;
    readonly rule_brief: number;
    readonly total: number;
  };
  readonly estimated_tokens: number;
  readonly instructions: string;
  readonly invocation_id: string;
  readonly projection: CanonicalObject;
  readonly recent_outcomes: readonly CanonicalJson[];
  readonly rule_brief: string;
}

export interface RunnerContribution {
  readonly action_id: string;
  readonly action_type: "record_assessment";
  readonly canonical_payload: CanonicalObject;
}

export type RunnerDisposition =
  | { readonly kind: "accepted"; readonly action_id: string }
  | { readonly kind: "rejected"; readonly action_id: string; readonly code: string }
  | { readonly kind: "lost_reply"; readonly action_id: string };

export class RunnerBudgetError extends Error {}

/**
 * A provider-free Runner reference. It keeps one replaceable current view and
 * one bounded outcome tail; no replay or private conversation is retained.
 */
export class DeterministicLateJoinRunner {
  readonly #memberId: string;
  readonly #role: "analyst" | "reviewer";
  readonly #ruleBrief: string;
  readonly #actionSchemas: Readonly<Record<string, CanonicalObject>>;
  #current: RunnerProjection | null = null;
  #outcomes: CanonicalJson[] = [];
  #invocationCounter = 0;
  #contributionCounter = 0;
  #invocation: { readonly context: RunnerContext; readonly contribution: RunnerContribution | null } | null = null;
  #pending = new Map<string, RunnerContribution>();

  constructor(options: {
    readonly actionSchemas: Readonly<Record<string, CanonicalObject>>;
    readonly memberId: string;
    readonly role: "analyst" | "reviewer";
    readonly ruleBrief: string;
  }) {
    this.#actionSchemas = options.actionSchemas;
    this.#memberId = options.memberId;
    this.#role = options.role;
    this.#ruleBrief = options.ruleBrief;
    this.#checkBytes("rule brief", options.ruleBrief);
  }

  get pendingActionIds(): readonly string[] {
    return [...this.#pending.keys()];
  }

  get retainedViewCount(): number {
    return this.#current === null ? 0 : 1;
  }

  /** Installs a Projection Reset, replacing any previous current view. */
  attach(projection: RunnerProjection): RunnerContext {
    this.#replaceProjection(projection);
    return this.#newInvocation();
  }

  /** Full-view observations have explicit replacement semantics for this Pack. */
  applyObservation(observation: CanonicalObject, replacement: RunnerProjection): RunnerContext {
    const changeType = observation.change_type;
    if (changeType !== "projection_replaced") throw new RunnerBudgetError("delta observations are not declared by this Pack");
    this.#replaceProjection(replacement);
    return this.#newInvocation();
  }

  startContribution(): RunnerContribution | null {
    const context = this.#newInvocation();
    const work = context.projection.open_work;
    if (!Array.isArray(work) || work.length === 0) {
      this.#invocation = { context, contribution: null };
      return null;
    }
    const first = work[0];
    if (first === null || typeof first !== "object" || Array.isArray(first)) throw new RunnerBudgetError("open_work item is not an object");
    const item = first as CanonicalObject;
    const connectionId = item.connection_id;
    const source = Array.isArray(context.projection.connections)
      ? context.projection.connections.find((candidate) => candidate !== null && typeof candidate === "object" && !Array.isArray(candidate) && (candidate as CanonicalObject).connection_id === connectionId)
      : undefined;
    if (source === undefined || source === null || typeof source !== "object" || Array.isArray(source)) throw new RunnerBudgetError("open work has no current source fact");
    const sourceRecord = source as CanonicalObject;
    const arrival = sourceRecord.arrival_minute;
    const departure = sourceRecord.departure_minute;
    const minimum = sourceRecord.minimum_transfer_minutes;
    if (typeof arrival !== "number" || typeof departure !== "number" || typeof minimum !== "number") throw new RunnerBudgetError("source fact is missing bounded timing fields");
    const feasible = departure - arrival >= minimum;
    const contribution: RunnerContribution = {
      action_id: `${this.#memberId}:assessment:${++this.#contributionCounter}`,
      action_type: "record_assessment",
      canonical_payload: {
        arrival_version: sourceRecord.arrival_version!,
        claim: feasible ? "connection_feasible" : "connection_at_risk",
        departure_version: sourceRecord.departure_version!,
        expected_work_revision: item.revision!,
        reason_code: feasible ? "transfer_time_sufficient" : "transfer_time_insufficient",
        work_id: item.work_id!,
      },
    };
    this.#pending.set(contribution.action_id, contribution);
    this.#invocation = { context, contribution };
    return contribution;
  }

  /** A lost reply is retryable with the original Action ID and payload. */
  retryLost(actionId: string): RunnerContribution {
    const contribution = this.#pending.get(actionId);
    if (contribution === undefined) throw new RunnerBudgetError("lost reply has no retained Action ID");
    this.#newInvocation();
    this.#invocation = { context: this.#invocation!.context, contribution };
    return contribution;
  }

  resolve(disposition: Exclude<RunnerDisposition, { readonly kind: "lost_reply" }>): RunnerContext {
    if (!this.#pending.has(disposition.action_id)) throw new RunnerBudgetError("reply does not match a pending Action ID");
    this.#pending.delete(disposition.action_id);
    const outcome = disposition.kind === "accepted"
      ? { action_id: disposition.action_id, outcome: "accepted" }
      : { action_id: disposition.action_id, outcome: "rejected", code: disposition.code };
    if (encodeCanonical(outcome).length > RUNNER_MAX_RECENT_OUTCOME_BYTES) throw new RunnerBudgetError("recent outcome exceeds its byte budget");
    this.#outcomes = [...this.#outcomes, outcome].slice(-RUNNER_MAX_RECENT_OUTCOMES);
    this.#invocation = null;
    return this.#newInvocation();
  }

  markLost(actionId: string): RunnerDisposition {
    if (!this.#pending.has(actionId)) throw new RunnerBudgetError("cannot mark an unknown Action ID lost");
    this.#invocation = null;
    return { action_id: actionId, kind: "lost_reply" };
  }

  /** A stale proposal is fenced; the next Invocation must use refreshed state. */
  handleStale(actionId: string): void {
    if (!this.#pending.delete(actionId)) throw new RunnerBudgetError("stale reply does not match a pending Action ID");
    this.#invocation = null;
  }

  #replaceProjection(projection: RunnerProjection): void {
    if (projection.projection_schema !== "participant") throw new RunnerBudgetError("Runner requires the participant Projection schema");
    const bytes = encodeCanonical(projection.projection).length;
    if (bytes > RUNNER_MAX_PROJECTION_BYTES) throw new RunnerBudgetError("Projection exceeds the 24-KiB budget");
    const work = projection.projection.open_work;
    if (!Array.isArray(work) || work.length > RUNNER_MAX_OPEN_WORK) throw new RunnerBudgetError("open work exceeds the 16-item budget");
    this.#current = projection;
  }

  #newInvocation(): RunnerContext {
    if (this.#current === null) throw new RunnerBudgetError("Runner has no current Projection Reset");
    const projectionBytes = encodeCanonical(this.#current.projection).length;
    const actionSchemas = this.#offeredActionSchemas();
    const actionSchemasBytes = encodeCanonical(actionSchemas as unknown as CanonicalJson).length;
    const instructions = "Choose one currently offered, evidence-backed contribution. Re-read the current Projection after every result.";
    const bytes = {
      action_schemas: actionSchemasBytes,
      instructions: new TextEncoder().encode(instructions).length,
      projection: projectionBytes,
      recent_outcomes: encodeCanonical(this.#outcomes).length,
      rule_brief: new TextEncoder().encode(this.#ruleBrief).length,
    };
    const total = Object.values(bytes).reduce((sum, value) => sum + value, 0);
    if (bytes.instructions > RUNNER_INSTRUCTION_BYTES || bytes.rule_brief > RUNNER_RULE_BRIEF_BYTES || total > RUNNER_MAX_CONTEXT_BYTES) throw new RunnerBudgetError("Invocation Context exceeds the aggregate byte budget");
    const estimatedTokens = Math.ceil(total / 4);
    if (estimatedTokens > RUNNER_MAX_TOKENS) throw new RunnerBudgetError("Invocation Context exceeds the token budget");
    const context: RunnerContext = {
      action_schemas: actionSchemas,
      encoded_bytes: { ...bytes, total },
      estimated_tokens: estimatedTokens,
      instructions,
      invocation_id: `${this.#memberId}:invocation:${++this.#invocationCounter}`,
      projection: this.#current.projection,
      recent_outcomes: [...this.#outcomes],
      rule_brief: this.#ruleBrief,
    };
    this.#invocation = { context, contribution: null };
    return context;
  }

  #checkBytes(label: string, value: string): void {
    if (new TextEncoder().encode(value).length > RUNNER_RULE_BRIEF_BYTES) throw new RunnerBudgetError(`${label} exceeds its byte budget`);
  }

  #offeredActionSchemas(): Readonly<Record<string, CanonicalObject>> {
    const offered: Record<string, CanonicalObject> = {};
    for (const offer of this.#current!.action_offers) {
      const actionType = typeof offer === "string" ? offer : offer.actionType;
      const schema = this.#actionSchemas[actionType];
      if (schema === undefined) throw new RunnerBudgetError(`offered Action ${actionType} has no exact schema`);
      if (encodeCanonical(schema as unknown as CanonicalJson).length > RUNNER_MAX_ACTION_SCHEMA_BYTES) {
        throw new RunnerBudgetError(`Action schema ${actionType} exceeds its byte budget`);
      }
      offered[actionType] = schema;
    }
    return offered;
  }
}

export function contextDigest(context: RunnerContext): string {
  return canonicalStringify({
    action_schemas: context.action_schemas,
    instructions: context.instructions,
    projection: context.projection,
    recent_outcomes: context.recent_outcomes,
    rule_brief: context.rule_brief,
  });
}
