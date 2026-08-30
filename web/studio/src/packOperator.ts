import { blake3 } from "@noble/hashes/blake3.js";
import { bytesToHex } from "@noble/hashes/utils.js";

export const PACK_OPERATOR_RECEIPT_SCHEMA =
  "worldstream/pack-operator-receipt/v1" as const;

export const STARTUP_PACK_INVENTORY_SCHEMA =
  "worldstream/startup-pack-inventory/v1" as const;

const DIGEST = /^blake3:[0-9a-f]{64}$/;
const MAX_TEXT_BYTES = 256;
const MAX_INVENTORY_ENTRIES = 256;

export type PackInstallState = "selectable" | "retained_only";
export type PackStorageProfile = "sqlite-bundled" | "postgres-primary";
export type PackOperatorOperation =
  | "inspect"
  | "approve"
  | "install"
  | "inventory"
  | "set_selectable"
  | "restart_readiness";

export interface PackInspectionReceipt {
  readonly schema: typeof PACK_OPERATOR_RECEIPT_SCHEMA;
  readonly status: "complete";
  readonly operation: "inspect";
  readonly pack_id: string;
  readonly explanatory_version: string;
  readonly bundle_digest: string;
  readonly revision_digest: string;
  readonly member_count: number;
  readonly byte_count: number;
  readonly approval_imported: false;
}

export interface PackMutationReceipt {
  readonly schema: typeof PACK_OPERATOR_RECEIPT_SCHEMA;
  readonly status: "complete";
  readonly operation: "approve" | "install" | "set_selectable";
  readonly bundle_digest: string;
  readonly revision_digest?: string;
  readonly install_state?: PackInstallState;
  readonly restart_required: boolean;
  readonly approval_transferred: false;
}

export interface PackInventoryEntry {
  readonly pack_id: string;
  readonly explanatory_version: string;
  readonly bundle_digest: string;
  readonly revision_digest: string;
  readonly install_state: PackInstallState;
}

export interface PackInventoryReceipt {
  readonly schema: typeof PACK_OPERATOR_RECEIPT_SCHEMA;
  readonly status: "complete";
  readonly operation: "inventory";
  readonly installed: number;
  readonly selectable: number;
  readonly retained_only: number;
  readonly storage_profile: PackStorageProfile;
  readonly inventory_digest: string;
  readonly entries: readonly PackInventoryEntry[];
  readonly restart_required_for_pending_changes: boolean;
}

export interface PackReadinessReceipt {
  readonly schema: typeof PACK_OPERATOR_RECEIPT_SCHEMA;
  readonly status: "ready";
  readonly operation: "restart_readiness";
  readonly embedded_revisions: number;
  readonly installed_bundles: number;
  readonly installed_selectable: number;
  readonly installed_retained_only: number;
  readonly storage_profile: PackStorageProfile;
  readonly deployment_binding: string;
  readonly inventory_digest: string;
  readonly total_revisions: number;
  readonly original_bytes_reverified: true;
  readonly production_component_host_admission: true;
  readonly room_replay_checked: boolean;
  readonly rooms_replayed: number;
  readonly isolated_rooms_skipped: number;
}

export type PackOperatorReceipt =
  | PackInspectionReceipt
  | PackMutationReceipt
  | PackInventoryReceipt
  | PackReadinessReceipt;

export interface PackOperatorWorkflowState {
  readonly phase:
    | "awaiting_inspection"
    | "awaiting_approval"
    | "awaiting_install"
    | "awaiting_inventory"
    | "awaiting_selection"
    | "awaiting_selected_inventory"
    | "awaiting_readiness"
    | "ready";
  readonly bundle_digest: string | null;
  readonly revision_digest: string | null;
  readonly pack_id: string | null;
  readonly explanatory_version: string | null;
  readonly selected_inventory_digest: string | null;
  readonly selected_storage_profile: PackStorageProfile | null;
  readonly daemon_stop_confirmed: boolean;
  readonly receipts: readonly PackOperatorReceipt[];
}

export const EMPTY_PACK_OPERATOR_WORKFLOW: PackOperatorWorkflowState = {
  phase: "awaiting_inspection",
  bundle_digest: null,
  revision_digest: null,
  pack_id: null,
  explanatory_version: null,
  selected_inventory_digest: null,
  selected_storage_profile: null,
  daemon_stop_confirmed: false,
  receipts: [],
};

export class PackOperatorReceiptError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "PackOperatorReceiptError";
  }
}

/** Strictly validates one pathless, secret-free worldstreamctl JSON receipt. */
export function readPackOperatorReceipt(value: unknown): PackOperatorReceipt | null {
  if (!isObject(value) || value.schema !== PACK_OPERATOR_RECEIPT_SCHEMA) return null;
  if (value.operation === "inspect") return readInspection(value);
  if (value.operation === "approve" || value.operation === "install" || value.operation === "set_selectable") return readMutation(value);
  if (value.operation === "inventory") return readInventory(value);
  if (value.operation === "restart_readiness") return readReadiness(value);
  return null;
}

export function parsePackOperatorReceipt(text: string): PackOperatorReceipt | null {
  if (new TextEncoder().encode(text).length > 128 * 1024) return null;
  try {
    return readPackOperatorReceipt(JSON.parse(text) as unknown);
  } catch {
    return null;
  }
}

/** Advances only the frozen exact offline lifecycle, in order. */
export function applyPackOperatorReceipt(
  state: PackOperatorWorkflowState,
  receipt: PackOperatorReceipt,
): PackOperatorWorkflowState {
  const expected = expectedOperation(state.phase);
  if (receipt.operation !== expected) {
    throw new PackOperatorReceiptError(`Expected ${expected} receipt before ${receipt.operation}.`);
  }
  if (state.bundle_digest !== null && "bundle_digest" in receipt && receipt.bundle_digest !== state.bundle_digest) {
    throw new PackOperatorReceiptError("Receipt belongs to a different physical Activity Pack Bundle.");
  }
  if (state.revision_digest !== null && "revision_digest" in receipt && receipt.revision_digest !== undefined && receipt.revision_digest !== state.revision_digest) {
    throw new PackOperatorReceiptError("Receipt belongs to a different Activity Pack Revision.");
  }

  if (receipt.operation === "inspect") {
    return {
      phase: "awaiting_approval",
      bundle_digest: receipt.bundle_digest,
      revision_digest: receipt.revision_digest,
      pack_id: receipt.pack_id,
      explanatory_version: receipt.explanatory_version,
      selected_inventory_digest: null,
      selected_storage_profile: null,
      daemon_stop_confirmed: false,
      receipts: [...state.receipts, receipt],
    };
  }
  if (receipt.operation === "approve") {
    if (!state.daemon_stop_confirmed) {
      throw new PackOperatorReceiptError("Stop worldstreamd before recording approval or changing Pack inventory.");
    }
    return withPhase(state, receipt, "awaiting_install");
  }
  if (receipt.operation === "install") {
    if (receipt.install_state !== "retained_only" || !receipt.restart_required) {
      throw new PackOperatorReceiptError("Install receipt did not establish retained-only pending inventory.");
    }
    return withPhase(state, receipt, "awaiting_inventory");
  }
  if (receipt.operation === "inventory") {
    const entry = receipt.entries.find((candidate) => candidate.bundle_digest === state.bundle_digest);
    if (entry === undefined || entry.revision_digest !== state.revision_digest) {
      throw new PackOperatorReceiptError("Inventory did not contain the inspected exact bundle and revision.");
    }
    if (state.phase === "awaiting_inventory") {
      if (entry.install_state !== "retained_only") {
        throw new PackOperatorReceiptError("Inventory did not retain the inspected exact bundle as retained-only.");
      }
      return {
        ...state,
        phase: "awaiting_selection",
        selected_storage_profile: receipt.storage_profile,
        receipts: [...state.receipts, receipt],
      };
    }
    if (state.phase !== "awaiting_selected_inventory" || entry.install_state !== "selectable") {
      throw new PackOperatorReceiptError("Post-selection inventory did not contain the exact bundle as selectable.");
    }
    if (state.selected_storage_profile === null || receipt.storage_profile !== state.selected_storage_profile) {
      throw new PackOperatorReceiptError("Post-selection inventory belongs to a different configured storage profile.");
    }
    return {
      ...state,
      phase: "awaiting_readiness",
      selected_inventory_digest: receipt.inventory_digest,
      selected_storage_profile: receipt.storage_profile,
      receipts: [...state.receipts, receipt],
    };
  }
  if (receipt.operation === "set_selectable") {
    if (receipt.install_state !== "selectable" || !receipt.restart_required) {
      throw new PackOperatorReceiptError("Selection receipt did not mark the exact bundle selectable after restart.");
    }
    return withPhase(state, receipt, "awaiting_selected_inventory");
  }
  if (receipt.operation !== "restart_readiness") {
    throw new PackOperatorReceiptError("Receipt operation is not valid at restart readiness.");
  }
  if (!receipt.room_replay_checked) {
    throw new PackOperatorReceiptError("Restart readiness did not execute Replay for the configured storage profile.");
  }
  if (state.selected_inventory_digest === null || receipt.inventory_digest !== state.selected_inventory_digest) {
    throw new PackOperatorReceiptError("Restart readiness belongs to a different post-selection Pack inventory.");
  }
  if (state.selected_storage_profile === null || receipt.storage_profile !== state.selected_storage_profile) {
    throw new PackOperatorReceiptError("Restart readiness belongs to a different configured storage profile.");
  }
  return withPhase(state, receipt, "ready");
}

export function commandForPackOperatorPhase(state: PackOperatorWorkflowState): string {
  const prefix = "worldstreamctl --config <WORLDSTREAM_CONFIG> pack";
  switch (state.phase) {
    case "awaiting_inspection": return `${prefix} inspect --bundle <BUNDLE.wspack>`;
    case "awaiting_approval": return state.daemon_stop_confirmed
      ? `${prefix} approve --bundle <BUNDLE.wspack> --operator-id <OPERATOR_ID> --decided-at <RFC3339>`
      : "Stop worldstreamd using its normal service manager, then confirm below.";
    case "awaiting_install": return `${prefix} install --bundle <BUNDLE.wspack> --installed-at <RFC3339>`;
    case "awaiting_inventory": return `${prefix} inventory`;
    case "awaiting_selection": return `${prefix} set-selectable --bundle-digest ${state.bundle_digest ?? "<BUNDLE_DIGEST>"} --selectable true`;
    case "awaiting_selected_inventory": return `${prefix} inventory`;
    case "awaiting_readiness": return state.selected_storage_profile === "postgres-primary"
      ? `${prefix} restart-readiness --dsn-file <POSTGRES_ADMIN_DSN_FILE>`
      : `${prefix} restart-readiness`;
    case "ready": return "Start worldstreamd using its normal service manager; the immutable startup registry will publish the exact revision.";
  }
}

export function confirmPackDaemonStopped(
  state: PackOperatorWorkflowState,
): PackOperatorWorkflowState {
  if (state.phase !== "awaiting_approval") {
    throw new PackOperatorReceiptError("Daemon stop confirmation is valid only after exact inspection.");
  }
  return { ...state, daemon_stop_confirmed: true };
}

function expectedOperation(phase: PackOperatorWorkflowState["phase"]): PackOperatorOperation {
  if (phase === "ready") throw new PackOperatorReceiptError("The exact bundle is already restart-ready.");
  return {
    awaiting_inspection: "inspect",
    awaiting_approval: "approve",
    awaiting_install: "install",
    awaiting_inventory: "inventory",
    awaiting_selection: "set_selectable",
    awaiting_selected_inventory: "inventory",
    awaiting_readiness: "restart_readiness",
  }[phase] as PackOperatorOperation;
}

function withPhase(state: PackOperatorWorkflowState, receipt: PackOperatorReceipt, phase: PackOperatorWorkflowState["phase"]): PackOperatorWorkflowState {
  return { ...state, phase, receipts: [...state.receipts, receipt] };
}

function readInspection(value: Record<string, unknown>): PackInspectionReceipt | null {
  if (!hasExactKeys(value, ["schema", "status", "operation", "pack_id", "explanatory_version", "bundle_digest", "revision_digest", "member_count", "byte_count", "approval_imported"])) return null;
  return value.status === "complete" && isText(value.pack_id) && isText(value.explanatory_version)
    && isDigest(value.bundle_digest) && isDigest(value.revision_digest)
    && isCount(value.member_count) && value.member_count > 0 && isCount(value.byte_count) && value.byte_count > 0
    && value.approval_imported === false ? value as unknown as PackInspectionReceipt : null;
}

function readMutation(value: Record<string, unknown>): PackMutationReceipt | null {
  if (!hasOnlyKeys(value, ["schema", "status", "operation", "bundle_digest", "restart_required", "approval_transferred", "revision_digest", "install_state"])) return null;
  if (value.status !== "complete" || !isDigest(value.bundle_digest) || typeof value.restart_required !== "boolean" || value.approval_transferred !== false) return null;
  if (value.revision_digest !== undefined && !isDigest(value.revision_digest)) return null;
  if (value.install_state !== undefined && !isInstallState(value.install_state)) return null;
  if (value.operation === "approve" && (value.revision_digest !== undefined || value.install_state !== undefined || value.restart_required)) return null;
  if ((value.operation === "install" || value.operation === "set_selectable") && (value.revision_digest === undefined || value.install_state === undefined)) return null;
  return value as unknown as PackMutationReceipt;
}

function readInventory(value: Record<string, unknown>): PackInventoryReceipt | null {
  if (!hasExactKeys(value, ["schema", "status", "operation", "installed", "selectable", "retained_only", "storage_profile", "inventory_digest", "entries", "restart_required_for_pending_changes"])) return null;
  if (value.status !== "complete" || !isCount(value.installed) || !isCount(value.selectable) || !isCount(value.retained_only)
    || !isStorageProfile(value.storage_profile) || !isDigest(value.inventory_digest) || !Array.isArray(value.entries)
    || value.entries.length > MAX_INVENTORY_ENTRIES || value.restart_required_for_pending_changes !== true) return null;
  const entries = value.entries.map(readInventoryEntry);
  if (entries.some((entry) => entry === null)) return null;
  const typedEntries = entries as PackInventoryEntry[];
  const selectable = typedEntries.filter((entry) => entry.install_state === "selectable").length;
  const retainedOnly = typedEntries.length - selectable;
  if (value.installed !== typedEntries.length || value.selectable !== selectable || value.retained_only !== retainedOnly) return null;
  if (!isStrictlyBundleDigestSorted(typedEntries)) return null;
  if (value.inventory_digest !== computeStartupPackInventoryDigest(typedEntries)) return null;
  return value as unknown as PackInventoryReceipt;
}

function readInventoryEntry(value: unknown): PackInventoryEntry | null {
  if (!isObject(value) || !hasExactKeys(value, ["pack_id", "explanatory_version", "bundle_digest", "revision_digest", "install_state"])) return null;
  return isText(value.pack_id) && isText(value.explanatory_version) && isDigest(value.bundle_digest)
    && isDigest(value.revision_digest) && isInstallState(value.install_state)
    ? value as unknown as PackInventoryEntry : null;
}

function readReadiness(value: Record<string, unknown>): PackReadinessReceipt | null {
  if (!hasExactKeys(value, ["schema", "status", "operation", "embedded_revisions", "installed_bundles", "installed_selectable", "installed_retained_only", "storage_profile", "deployment_binding", "inventory_digest", "total_revisions", "original_bytes_reverified", "production_component_host_admission", "room_replay_checked", "rooms_replayed", "isolated_rooms_skipped"])) return null;
  const counts = [value.embedded_revisions, value.installed_bundles, value.installed_selectable, value.installed_retained_only, value.total_revisions, value.rooms_replayed, value.isolated_rooms_skipped];
  if (!counts.every(isCount)) return null;
  const embedded = value.embedded_revisions as number;
  const installed = value.installed_bundles as number;
  const selectable = value.installed_selectable as number;
  const retained = value.installed_retained_only as number;
  const total = value.total_revisions as number;
  return value.status === "ready" && isStorageProfile(value.storage_profile) && isDigest(value.deployment_binding)
    && isDigest(value.inventory_digest) && selectable + retained === installed && embedded + installed === total
    && value.original_bytes_reverified === true && value.production_component_host_admission === true
    && typeof value.room_replay_checked === "boolean" ? value as unknown as PackReadinessReceipt : null;
}

/** Recomputes the exact portable identity emitted by the startup Pack registry. */
export function computeStartupPackInventoryDigest(entries: readonly PackInventoryEntry[]): string {
  const identity = {
    entries: entries.map((entry) => ({
      bundle_digest: entry.bundle_digest,
      explanatory_version: entry.explanatory_version,
      install_state: entry.install_state,
      pack_id: entry.pack_id,
      revision_digest: entry.revision_digest,
    })),
    schema: STARTUP_PACK_INVENTORY_SCHEMA,
  };
  const canonicalBytes = new TextEncoder().encode(canonicalStringify(identity));
  return `blake3:${bytesToHex(blake3(canonicalBytes))}`;
}

function canonicalStringify(value: unknown): string {
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value)) throw new TypeError("canonical JSON requires safe integers");
    return String(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalStringify).join(",")}]`;
  if (isObject(value)) {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalStringify(value[key])}`).join(",")}}`;
  }
  throw new TypeError("unsupported canonical JSON value");
}

function isStrictlyBundleDigestSorted(entries: readonly PackInventoryEntry[]): boolean {
  for (let index = 1; index < entries.length; index += 1) {
    if (entries[index - 1]!.bundle_digest >= entries[index]!.bundle_digest) return false;
  }
  return true;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return Object.keys(value).length === keys.length && keys.every((key) => key in value);
}
function hasOnlyKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const required = keys.filter((key) => key !== "revision_digest" && key !== "install_state");
  return Object.keys(value).every((key) => keys.includes(key)) && required.every((key) => key in value);
}
function isText(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && new TextEncoder().encode(value).length <= MAX_TEXT_BYTES;
}
function isDigest(value: unknown): value is string {
  return typeof value === "string" && DIGEST.test(value);
}
function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= 1_000_000;
}
function isInstallState(value: unknown): value is PackInstallState {
  return value === "selectable" || value === "retained_only";
}
function isStorageProfile(value: unknown): value is PackStorageProfile {
  return value === "sqlite-bundled" || value === "postgres-primary";
}
