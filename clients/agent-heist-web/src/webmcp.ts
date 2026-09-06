import {
  type HostedLiveActionReceipt,
  type HostedLiveSessionController,
  type HostedLiveSessionSnapshot,
} from "@worldstream/client";

import type {
  AgentHeistActionOffer,
  AgentHeistLiveState,
  AgentHeistReadyState,
} from "./liveAdapter";

const MAX_TOOL_RESULT_CHARACTERS = 1_500;
const WAIT_TIMEOUT_MS = 15_000;
const TOKEN_PATTERN = /^[A-Za-z0-9_-]{16,80}$/u;
const MAX_KNOWN_STATE_TOKENS = 32;

export interface AgentHeistWebMcpExecutionContext {
  readonly signal?: AbortSignal;
}

export interface AgentHeistWebMcpTool {
  readonly name: string;
  readonly title: string;
  readonly description: string;
  readonly inputSchema: Record<string, unknown>;
  readonly annotations: Readonly<Record<string, boolean>>;
  execute(
    input?: unknown,
    context?: AgentHeistWebMcpExecutionContext,
  ): Promise<Record<string, unknown>>;
}

export interface AgentHeistWebMcpModelContext {
  registerTool(
    tool: AgentHeistWebMcpTool,
    options: { readonly signal: AbortSignal },
  ): void | Promise<void>;
}

export interface AgentHeistWebMcpHost {
  readonly modelContext?: AgentHeistWebMcpModelContext;
}

export interface AgentHeistWebMcpRegistration {
  readonly supported: boolean;
  readonly ready: Promise<boolean>;
  dispose(): void;
}

export interface AgentHeistWebMcpBridgeOptions {
  readonly controller: Pick<
    HostedLiveSessionController,
    "state" | "waitFor" | "submitAction"
  >;
  readonly readLiveState: () => AgentHeistLiveState;
  readonly tokenFactory?: (prefix: "state" | "action") => string;
  readonly actionIdFactory?: () => string;
}

interface ActionBinding {
  readonly key: string;
  readonly token: string;
  readonly actionId: string;
  readonly offer: AgentHeistActionOffer;
  readonly roomSequence: number;
  readonly role: string;
  readonly phase: string;
  readonly deadline: string | null;
  task: Promise<Record<string, unknown>> | null;
}

/**
 * Pack-aware, browser-only tool adapter. It reads and mutates through the same
 * authorized live controller as the human UI and never owns Room authority.
 */
export class AgentHeistWebMcpBridge {
  private readonly controller: AgentHeistWebMcpBridgeOptions["controller"];
  private readonly readLiveState: () => AgentHeistLiveState;
  private readonly tokenFactory: (prefix: "state" | "action") => string;
  private readonly actionIdFactory: () => string;
  private stateKey: string | null = null;
  private stateToken: string | null = null;
  private readonly knownStateTokens = new Set<string>();
  private actionBinding: ActionBinding | null = null;

  constructor(options: AgentHeistWebMcpBridgeOptions) {
    this.controller = options.controller;
    this.readLiveState = options.readLiveState;
    this.tokenFactory = options.tokenFactory ?? opaqueToken;
    this.actionIdFactory = options.actionIdFactory ?? createUlid;
  }

  tools(accessMode: "participant" | "spectator"): readonly AgentHeistWebMcpTool[] {
    const tools = [this.readTool(), this.waitTool()];
    return accessMode === "participant"
      ? [...tools, this.commitTool()]
      : tools;
  }

  async read(input: unknown = {}): Promise<Record<string, unknown>> {
    if (!hasExactKeys(input, [])) return failure("invalid_input", "This tool accepts an empty object only.");
    return this.currentOutput();
  }

  async wait(
    input: unknown,
    context: AgentHeistWebMcpExecutionContext = {},
  ): Promise<Record<string, unknown>> {
    if (!hasExactKeys(input, ["after_state_token"])) {
      return failure("invalid_input", "Wait input must contain only after_state_token.");
    }
    const after = (input as Record<string, unknown>).after_state_token;
    if (typeof after !== "string" || !TOKEN_PATTERN.test(after)) {
      return failure("invalid_input", "after_state_token must come from the latest read or wait result.");
    }
    this.refreshBindings();
    if (!this.knownStateTokens.has(after)) {
      return failure("invalid_input", "after_state_token was not issued by this page session.");
    }
    const access = accessFailure(this.controller.state, this.readLiveState());
    if (access !== null) return access;
    if (after !== this.stateToken) {
      return bounded({ ok: true, kind: "changed", state: this.compactState() });
    }

    const baseline = sessionKey(this.controller.state, this.readLiveState());
    try {
      await this.controller.waitFor(
        (snapshot) => sessionKey(snapshot, this.readLiveState()) !== baseline,
        { signal: context.signal, timeoutMs: WAIT_TIMEOUT_MS },
      );
      this.refreshBindings();
      const nextAccess = accessFailure(this.controller.state, this.readLiveState());
      return nextAccess ?? bounded({ ok: true, kind: "changed", state: this.compactState() });
    } catch (error) {
      if (error instanceof Error && error.name === "AbortError") throw error;
      return bounded({
        ok: true,
        kind: "timeout",
        state_token: this.stateToken,
        retry_after_ms: 250,
        message: "No authorized change arrived in 15 seconds. Call wait again.",
      });
    }
  }

  async commit(
    input: unknown,
    context: AgentHeistWebMcpExecutionContext = {},
  ): Promise<Record<string, unknown>> {
    if (context.signal?.aborted === true) throw abortError();
    if (
      !hasExactKeys(input, [
        "action_token",
        "plan_id",
        "contribute_required_resource",
      ])
    ) {
      return failure("invalid_input", "Commit input contains missing or unknown fields.");
    }
    const values = input as Record<string, unknown>;
    if (
      typeof values.action_token !== "string" ||
      !TOKEN_PATTERN.test(values.action_token) ||
      typeof values.plan_id !== "string" ||
      !boundedIdentifier(values.plan_id) ||
      typeof values.contribute_required_resource !== "boolean"
    ) {
      return failure("invalid_input", "Commit input does not match the strict Heist schema.");
    }

    this.refreshBindings();
    const live = this.readLiveState();
    const access = accessFailure(this.controller.state, live);
    if (access !== null) return access;
    if (live.kind !== "ready" || live.authorization.accessMode !== "participant") {
      return failure("participant_required", "This authorized session is read-only.");
    }
    const binding = this.actionBinding;
    if (binding === null || values.action_token !== binding.token) {
      return bounded({
        ...failure(
          "refresh_required",
          "This action token is stale. Read current state before deciding again.",
        ),
        state: this.compactState(),
      });
    }
    if (
      live.roomSequence !== binding.roomSequence ||
      live.authorization.role !== binding.role ||
      live.projection.phase !== binding.phase ||
      live.projection.phaseDeadline !== binding.deadline ||
      !sameOffer(live, binding.offer)
    ) {
      return failure("refresh_required", "The exact Head or Action Offer changed.");
    }
    if (live.projection.phase !== "commitment") {
      return failure("move_unavailable", "Plan commitment is not legal in the current phase.");
    }
    if (!live.projection.plans.some((plan) => plan.planId === values.plan_id)) {
      return bounded({
        ...failure("invalid_plan", "Choose a plan from the current authorized Projection."),
        current_plan_ids: live.projection.plans.slice(0, 8).map((plan) => clip(plan.planId, 80)),
      });
    }
    if (binding.task !== null) return binding.task;
    if (!this.controller.state.canAct) {
      return failure("reconnect_required", "The client is not ready at a synchronized Head.");
    }

    binding.task = this.submitBinding(
      binding,
      values.plan_id,
      values.contribute_required_resource,
    );
    return binding.task;
  }

  private async submitBinding(
    binding: ActionBinding,
    planId: string,
    contributeRequiredResource: boolean,
  ): Promise<Record<string, unknown>> {
    try {
      const receipt = await this.controller.submitAction({
        actionId: binding.actionId,
        basedOnRoomSeq: binding.roomSequence,
        actionType: binding.offer.actionType,
        payload: {
          selected_plan_id: planId,
          contribute_required_resource: contributeRequiredResource,
        },
      });
      return this.receiptOutput(receipt, planId);
    } catch {
      this.refreshBindings();
      const changed = this.actionBinding?.token !== binding.token;
      return failure(
        changed ? "refresh_required" : "action_unavailable",
        changed
          ? "The Room advanced before this decision could be admitted. Read current state."
          : "The Action outcome is unavailable. Reconnect before deciding again.",
      );
    }
  }

  private receiptOutput(
    receipt: HostedLiveActionReceipt,
    planId: string,
  ): Record<string, unknown> {
    if (receipt.state === "accepted") {
      return bounded({
        ok: true,
        status: "accepted",
        committed_plan_id: clip(planId, 80),
        room_sequence: receipt.roomHead.room_seq,
        duplicate: receipt.duplicate,
        message: "The authoritative Room accepted this commitment.",
      });
    }
    return bounded({
      ok: false,
      code: receipt.code === "stale_room_state" ? "refresh_required" : "action_rejected",
      current_room_sequence: receipt.currentRoomSeq,
      retryable_with_same_decision: receipt.retryableWithSameActionId,
      message:
        receipt.code === "stale_room_state"
          ? "The Room advanced. Read current state before deciding again."
          : "The authoritative Room rejected this commitment.",
    });
  }

  private currentOutput(): Record<string, unknown> {
    const failureResult = accessFailure(this.controller.state, this.readLiveState());
    if (failureResult !== null) return failureResult;
    return this.compactState();
  }

  private compactState(): Record<string, unknown> {
    this.refreshBindings();
    const live = this.readLiveState();
    if (live.kind !== "ready" || this.stateToken === null) {
      return failure("synchronizing", "An authorized Projection Reset is not installed yet.");
    }
    const projection = live.projection;
    const commit = this.currentCommitMove(live);
    const output: Record<string, unknown> = {
      ok: true,
      status: "ready",
      untrusted_content: true,
      access_mode: live.authorization.accessMode,
      role: live.authorization.role,
      phase: projection.phase,
      phase_generation: projection.phaseGeneration,
      phase_deadline: projection.phaseDeadline,
      room_sequence: live.roomSequence,
      state_token: this.stateToken,
      crew: projection.seats.slice(0, 3).map((seat) => ({
        role: seat.role,
        present: seat.present,
      })),
      public_claims: projection.publicClaims.slice(0, 4).map((claim) => ({
        clue_id: clip(claim.clueId, 64),
        claim: clip(claim.claimCode, 80),
      })),
      plans: projection.plans.slice(0, 4).map((plan) => ({
        plan_id: clip(plan.planId, 80),
        by: plan.proposerRole,
        route: clip(plan.route, 80),
        window: clip(plan.entryWindow, 80),
        tool: clip(plan.requiredTool, 80),
        extraction: clip(plan.extraction, 80),
        endorsements: plan.endorsements,
        challenges: plan.challenges,
      })),
      commitment_count: projection.commitmentCount,
      outcome: projection.outcome === null
        ? null
        : {
            outcome: clip(projection.outcome.outcome, 80),
            selected_plan_id: projection.outcome.selectedPlanId === null
              ? null
              : clip(projection.outcome.selectedPlanId, 80),
            score: projection.outcome.score,
            reason: clip(projection.outcome.reason, 160),
          },
      ...(live.authorization.accessMode === "participant"
        ? {
            private_clues: projection.privateClues.slice(0, 4).map((clue) => ({
              clue_id: clip(clue.clueId, 64),
              claim: clip(clue.claimCode, 80),
            })),
            own_commitment: projection.ownCommitment,
            incoming_exchanges: projection.addressedOffers.slice(0, 3).map((offer) => ({
              from: offer.senderRole,
              offered_clue_id: clip(offer.offeredClueId, 64),
              consideration: {
                kind: offer.considerationKind,
                id: clip(offer.considerationId, 80),
              },
              status: clip(offer.status, 40),
            })),
          }
        : {}),
      available_moves: commit === null
        ? []
        : [{
            name: "commit_plan",
            action_token: commit.token,
            plan_ids: projection.plans.slice(0, 8).map((plan) => clip(plan.planId, 80)),
          }],
    };
    const minimal = {
      ok: true,
      status: "ready",
      untrusted_content: true,
      access_mode: live.authorization.accessMode,
      role: live.authorization.role,
      phase: projection.phase,
      phase_deadline: projection.phaseDeadline,
      room_sequence: live.roomSequence,
      state_token: this.stateToken,
      plan_ids: projection.plans.slice(0, 8).map((plan) => clip(plan.planId, 64)),
      commitment_count: projection.commitmentCount,
      outcome: projection.outcome === null ? null : clip(projection.outcome.outcome, 80),
      available_moves: commit === null
        ? []
        : [{ name: "commit_plan", action_token: commit.token }],
      truncated: true,
    };
    return bounded(output, minimal);
  }

  private refreshBindings(): void {
    const live = this.readLiveState();
    const nextStateKey = sessionKey(this.controller.state, live);
    if (nextStateKey !== this.stateKey) {
      this.stateKey = nextStateKey;
      this.stateToken = this.newToken("state");
      this.knownStateTokens.add(this.stateToken);
      while (this.knownStateTokens.size > MAX_KNOWN_STATE_TOKENS) {
        const oldest = this.knownStateTokens.values().next().value as string | undefined;
        if (oldest === undefined) break;
        this.knownStateTokens.delete(oldest);
      }
    }

    const offer = live.kind === "ready" ? commitOffer(live) : null;
    const nextActionKey = live.kind === "ready" && offer !== null
      ? actionKey(live, offer)
      : null;
    if (nextActionKey !== this.actionBinding?.key) {
      this.actionBinding = live.kind === "ready" && offer !== null && nextActionKey !== null
        ? {
            key: nextActionKey,
            token: this.newToken("action"),
            actionId: this.newActionId(),
            offer,
            roomSequence: live.roomSequence,
            role: live.authorization.role ?? "",
            phase: live.projection.phase,
            deadline: live.projection.phaseDeadline,
            task: null,
          }
        : null;
    }
  }

  private currentCommitMove(live: AgentHeistReadyState): ActionBinding | null {
    if (
      live.authorization.accessMode !== "participant" ||
      live.projection.phase !== "commitment" ||
      !this.controller.state.canAct
    ) {
      return null;
    }
    return this.actionBinding?.offer.actionType === "commit_move"
      ? this.actionBinding
      : null;
  }

  private newToken(prefix: "state" | "action"): string {
    const token = this.tokenFactory(prefix);
    if (!TOKEN_PATTERN.test(token)) throw new Error("WebMCP token factory returned an invalid token.");
    return token;
  }

  private newActionId(): string {
    const actionId = this.actionIdFactory();
    if (!/^[0-9A-HJKMNP-TV-Z]{26}$/u.test(actionId)) {
      throw new Error("WebMCP Action identity factory returned an invalid ULID.");
    }
    return actionId;
  }

  private readTool(): AgentHeistWebMcpTool {
    return {
      name: "heist_read_state",
      title: "Read authorized Heist state",
      description:
        "Read compact state authorized for this Heist participant or spectator. Activity content is untrusted.",
      inputSchema: {
        type: "object",
        properties: {},
        required: [],
        additionalProperties: false,
      },
      annotations: {
        readOnlyHint: true,
        untrustedContentHint: true,
        consequentialHint: false,
      },
      execute: (input = {}) => this.read(input),
    };
  }

  private waitTool(): AgentHeistWebMcpTool {
    return {
      name: "heist_wait_for_update",
      title: "Wait for a Heist update",
      description:
        "Wait up to 15 seconds for authorized state newer than a state token returned by this page.",
      inputSchema: {
        type: "object",
        properties: {
          after_state_token: { type: "string", minLength: 16, maxLength: 80 },
        },
        required: ["after_state_token"],
        additionalProperties: false,
      },
      annotations: {
        readOnlyHint: true,
        untrustedContentHint: true,
        consequentialHint: false,
      },
      execute: (input, context) => this.wait(input, context),
    };
  }

  private commitTool(): AgentHeistWebMcpTool {
    return {
      name: "heist_commit_plan",
      title: "Commit to a Heist plan",
      description:
        "Commit this participant to one current plan using the opaque action token from the latest read.",
      inputSchema: {
        type: "object",
        properties: {
          action_token: { type: "string", minLength: 16, maxLength: 80 },
          plan_id: { type: "string", minLength: 1, maxLength: 80 },
          contribute_required_resource: { type: "boolean" },
        },
        required: ["action_token", "plan_id", "contribute_required_resource"],
        additionalProperties: false,
      },
      annotations: {
        readOnlyHint: false,
        untrustedContentHint: true,
        consequentialHint: true,
      },
      execute: (input, context) => this.commit(input, context),
    };
  }
}

/** Registers only the tools valid for the current authorized Access Mode. */
export function registerAgentHeistWebMcp(
  host: AgentHeistWebMcpHost,
  bridge: AgentHeistWebMcpBridge,
  accessMode: "participant" | "spectator",
): AgentHeistWebMcpRegistration {
  const context = host.modelContext;
  if (context === undefined) {
    return { supported: false, ready: Promise.resolve(false), dispose() {} };
  }
  const registration = new AbortController();
  const ready = (async () => {
    try {
      for (const tool of bridge.tools(accessMode)) {
        await context.registerTool(tool, { signal: registration.signal });
      }
      return !registration.signal.aborted;
    } catch {
      registration.abort();
      return false;
    }
  })();
  return {
    supported: true,
    ready,
    dispose: () => registration.abort(),
  };
}

function accessFailure(
  session: HostedLiveSessionSnapshot,
  live: AgentHeistLiveState,
): Record<string, unknown> | null {
  if (session.status === "setup_required" || session.status === "closed") {
    return failure("sign_in_required", "Return to the catalog and enter this Run again.");
  }
  if (session.status === "disconnected") {
    return failure("reconnect_required", "Reconnect and install authorized Catch-up before continuing.");
  }
  if (session.status !== "live" || !session.synchronized) {
    return failure("synchronizing", "Wait for the authorized Projection Reset or Catch-up.");
  }
  if (live.kind === "incompatible") {
    return failure("client_incompatible", "This exact Agent Heist revision is not supported.");
  }
  if (live.kind !== "ready") {
    return failure("synchronizing", "An authorized Agent Heist Projection is not installed.");
  }
  return null;
}

function commitOffer(live: AgentHeistReadyState): AgentHeistActionOffer | null {
  if (
    live.authorization.accessMode !== "participant" ||
    live.authorization.role === null ||
    live.projection.phase !== "commitment"
  ) {
    return null;
  }
  return live.offers.find((offer) => offer.actionType === "commit_move") ?? null;
}

function sameOffer(
  live: AgentHeistReadyState,
  expected: AgentHeistActionOffer,
): boolean {
  return live.offers.some((offer) =>
    offer.offerId === expected.offerId &&
    offer.actionType === expected.actionType &&
    offer.schemaDigest === expected.schemaDigest &&
    offer.eligibility === expected.eligibility
  );
}

function sessionKey(
  session: HostedLiveSessionSnapshot,
  live: AgentHeistLiveState,
): string {
  if (live.kind !== "ready") {
    return `${session.status}:${session.synchronized}:${session.canAct}:${live.kind}`;
  }
  return JSON.stringify([
    session.status,
    session.synchronized,
    session.canAct,
    session.lastAcknowledgedFrameSeq,
    live.pack.digest,
    live.roomSequence,
    live.frameHead,
    live.roomHead.genesisOrTransitionHash,
    live.roomHead.authoritativeStateHash,
    live.authorization.accessMode,
    live.authorization.role,
    live.projection.phase,
    live.projection.phaseGeneration,
    live.offers.map((offer) => [
      offer.offerId,
      offer.schemaDigest,
      offer.actionType,
      offer.eligibility,
    ]),
  ]);
}

function actionKey(
  live: AgentHeistReadyState,
  offer: AgentHeistActionOffer,
): string {
  return JSON.stringify([
    live.pack.digest,
    live.roomSequence,
    live.roomHead.genesisOrTransitionHash,
    live.roomHead.authoritativeStateHash,
    live.authorization.role,
    live.projection.phase,
    live.projection.phaseGeneration,
    live.projection.phaseDeadline,
    offer.offerId,
    offer.schemaDigest,
    offer.actionType,
    offer.eligibility,
  ]);
}

function hasExactKeys(value: unknown, keys: readonly string[]): boolean {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const actual = Object.keys(value);
  return actual.length === keys.length && actual.every((key) => keys.includes(key));
}

function boundedIdentifier(value: string): boolean {
  return value.length > 0 && new TextEncoder().encode(value).byteLength <= 80;
}

function clip(value: string, maximum: number): string {
  return value.length <= maximum ? value : `${value.slice(0, Math.max(1, maximum - 1))}…`;
}

function failure(code: string, message: string): Record<string, unknown> {
  return bounded({ ok: false, code, message });
}

function bounded(
  value: Record<string, unknown>,
  fallback: Record<string, unknown> = {
    ok: false,
    code: "output_too_large",
    message: "The Activity Client refused to expose an oversized result.",
  },
): Record<string, unknown> {
  if (JSON.stringify(value).length <= MAX_TOOL_RESULT_CHARACTERS) return value;
  if (JSON.stringify(fallback).length > MAX_TOOL_RESULT_CHARACTERS) {
    throw new Error("WebMCP bounded fallback exceeded its fixed output limit.");
  }
  return fallback;
}

function opaqueToken(prefix: "state" | "action"): string {
  const bytes = crypto.getRandomValues(new Uint8Array(24));
  const encoded = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${prefix === "state" ? "st" : "at"}_${encoded}`;
}

function createUlid(): string {
  const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[0] = (bytes[0] ?? 0) & 0x3f;
  let value = bytes.reduce(
    (result, byte) => (result << 8n) | BigInt(byte),
    0n,
  );
  let encoded = "";
  for (let index = 0; index < 26; index += 1) {
    encoded = alphabet[Number(value & 31n)] + encoded;
    value >>= 5n;
  }
  return encoded;
}

function abortError(): Error {
  const error = new Error("The WebMCP operation was cancelled.");
  error.name = "AbortError";
  return error;
}
