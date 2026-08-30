// PROTOTYPE ONLY. The deliberately visible module counter proves whether the
// host creates a fresh Component instance for every callback.
let callbackOrdinal = 0;

type Role = "buyer" | "seller";

interface Proposal {
  revision: number;
  proposedBy: Role;
  price: number;
}

interface PackState {
  phase: "negotiating" | "agreed";
  revision: number;
  currentProposal: Proposal | null;
  buyerCeiling: number;
  sellerFloor: number;
  outcome: { agreedPrice: number; acceptedBy: Role } | null;
}

interface Action {
  role: Role;
  type: "propose" | "counter" | "accept" | "__probe_burn" | "__probe_allocate" | "__probe_recurse" | "__probe_oversize_output";
  price?: number;
  basisRevision?: number;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function touch(): number {
  callbackOrdinal += 1;
  return callbackOrdinal;
}

function decode<T>(bytes: Uint8Array): T {
  return JSON.parse(decoder.decode(bytes)) as T;
}

function canonicalStringify(value: unknown): string {
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value)) throw new Error("prototype canonical JSON permits safe integers only");
    return String(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(canonicalStringify).join(",")}]`;
  }
  if (typeof value === "object") {
    const record = value as Record<string, unknown>;
    return `{${Object.keys(record)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalStringify(record[key])}`)
      .join(",")}}`;
  }
  throw new Error(`unsupported prototype canonical JSON value: ${typeof value}`);
}

function encode(value: unknown): Uint8Array {
  return encoder.encode(canonicalStringify(value));
}

function reject(state: PackState, code: string, message: string, ordinal: number): Uint8Array {
  return encode({ kind: "rejected", code, message, state, instanceOrdinal: ordinal });
}

function recurseForever(depth: number): number {
  return recurseForever(depth + 1) + 1;
}

export function descriptor(): Uint8Array {
  const ordinal = touch();
  return encode({
    packId: "worldstream.prototype.negotiation",
    semanticVersion: "0.1.0-prototype",
    roles: ["buyer", "seller"],
    actions: ["propose", "counter", "accept"],
    operations: ["descriptor", "initialize", "reduce", "view", "observe"],
    instanceOrdinal: ordinal,
  });
}

export function initialize(input: Uint8Array): Uint8Array {
  const ordinal = touch();
  const request = decode<{ configuration: { buyerCeiling: number; sellerFloor: number } }>(input);
  const state: PackState = {
    phase: "negotiating",
    revision: 0,
    currentProposal: null,
    buyerCeiling: request.configuration.buyerCeiling,
    sellerFloor: request.configuration.sellerFloor,
    outcome: null,
  };
  return encode({ state, events: [{ type: "negotiation-opened" }], instanceOrdinal: ordinal });
}

export function reduce(input: Uint8Array): Uint8Array {
  const ordinal = touch();
  const request = decode<{ state: PackState; action: Action }>(input);
  const state = request.state;
  const action = request.action;

  if (action.type === "__probe_burn") {
    while (true) {
      callbackOrdinal += 1;
    }
  }
  if (action.type === "__probe_allocate") {
    const allocation = new Uint8Array(512 * 1024 * 1024);
    allocation.fill(7);
    return encode({ allocation: allocation.length });
  }
  if (action.type === "__probe_recurse") {
    return encode({ recursion: recurseForever(0) });
  }
  if (action.type === "__probe_oversize_output") {
    return encoder.encode("x".repeat(128 * 1024));
  }

  if (state.phase !== "negotiating") {
    return reject(state, "terminal-phase", "The negotiation already has an agreement.", ordinal);
  }

  if (action.type === "propose" || action.type === "counter") {
    const requiredRole: Role = action.type === "propose" ? "buyer" : "seller";
    if (action.role !== requiredRole) {
      return reject(state, "wrong-role", `${action.type} is reserved for the ${requiredRole}.`, ordinal);
    }
    if (!Number.isSafeInteger(action.price) || (action.price ?? 0) <= 0) {
      return reject(state, "invalid-price", "Price must be a positive integer.", ordinal);
    }
    if ((action.basisRevision ?? state.revision) !== state.revision) {
      return reject(state, "stale-revision", "The proposal basis is no longer current.", ordinal);
    }
    const nextRevision = state.revision + 1;
    const proposal: Proposal = {
      revision: nextRevision,
      proposedBy: action.role,
      price: action.price as number,
    };
    const next: PackState = { ...state, revision: nextRevision, currentProposal: proposal };
    return encode({
      kind: "applied",
      state: next,
      events: [{ type: "proposal-revised", revision: nextRevision, proposedBy: action.role, price: action.price }],
      instanceOrdinal: ordinal,
    });
  }

  if (action.type === "accept") {
    const proposal = state.currentProposal;
    if (proposal === null) {
      return reject(state, "nothing-to-accept", "No proposal is currently open.", ordinal);
    }
    if (proposal.proposedBy === action.role) {
      return reject(state, "self-acceptance", "The proposing party cannot accept its own proposal.", ordinal);
    }
    if (action.basisRevision !== proposal.revision) {
      return reject(state, "stale-revision", "Acceptance must name the exact current proposal.", ordinal);
    }
    if (action.role === "buyer" && proposal.price > state.buyerCeiling) {
      return reject(state, "outside-private-limit", "The current proposal exceeds the buyer's private limit.", ordinal);
    }
    if (action.role === "seller" && proposal.price < state.sellerFloor) {
      return reject(state, "outside-private-limit", "The current proposal is below the seller's private limit.", ordinal);
    }
    const outcome = { agreedPrice: proposal.price, acceptedBy: action.role };
    const next: PackState = { ...state, phase: "agreed", outcome };
    return encode({
      kind: "applied",
      state: next,
      events: [{ type: "agreement-reached", revision: proposal.revision, agreedPrice: proposal.price }],
      instanceOrdinal: ordinal,
    });
  }

  return reject(state, "unknown-action", "The action is not part of this Activity Pack.", ordinal);
}

export function view(input: Uint8Array): Uint8Array {
  const ordinal = touch();
  const request = decode<{ state: PackState; viewerRole: Role }>(input);
  const state = request.state;
  const viewerRole = request.viewerRole;
  const privateLimit = viewerRole === "buyer" ? state.buyerCeiling : state.sellerFloor;
  const actionOffers: string[] = [];

  if (state.phase === "negotiating") {
    actionOffers.push(viewerRole === "buyer" ? "propose" : "counter");
    if (state.currentProposal !== null && state.currentProposal.proposedBy !== viewerRole) {
      actionOffers.push("accept");
    }
  }

  return encode({
    projection: {
      phase: state.phase,
      revision: state.revision,
      currentProposal: state.currentProposal,
      outcome: state.outcome,
      privateLimit,
    },
    actionOffers,
    instanceOrdinal: ordinal,
  });
}

export function observe(input: Uint8Array): Uint8Array {
  const ordinal = touch();
  const request = decode<{ before: PackState; after: PackState; viewerRole: Role }>(input);
  const changed = request.before.revision !== request.after.revision || request.before.phase !== request.after.phase;
  return encode({
    visible: changed,
    summary: changed
      ? { phase: request.after.phase, revision: request.after.revision, currentProposal: request.after.currentProposal, outcome: request.after.outcome }
      : null,
    instanceOrdinal: ordinal,
  });
}
