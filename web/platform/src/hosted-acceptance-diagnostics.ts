import { open, readdir } from "node:fs/promises";
import { join } from "node:path";

const ATTEMPT_STATES = ["reserved", "completed", "ambiguous", "provider_failed"] as const;
const FAILURE_CODES = ["provider_unavailable", "provider_timeout", "provider_lost_reply",
  "provider_rate_limited", "provider_rejected", "invalid_response", "unoffered_action",
  "output_limit", "route_mismatch", "fallback_detected"] as const;
const PROCESS_STATES = ["starting", "running", "stopped", "needs_attention"] as const;

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown> : {};
}
function count(value: unknown): number | null {
  return Number.isSafeInteger(value) && (value as number) >= 0 && (value as number) <= 1_000_000
    ? value as number : null;
}
function code(value: unknown, allowed: readonly string[]): string {
  return typeof value === "string" && allowed.includes(value) ? value : "unknown";
}
function histogram(values: unknown[], allowed: readonly string[]): Record<string, number> {
  const result = Object.fromEntries([...allowed, "unknown"].map(key => [key, 0]));
  for (const value of values) result[code(value, allowed)]! += 1;
  return result;
}

/** Reconstructs every output field; retained values can never supply keys or free text. */
export function houseWaitDiagnostic(input: {
  sessionStatus: unknown; phase: unknown; plans: unknown;
  ledger: unknown; operations: unknown; providerMetrics: unknown;
}) {
  const plans = Array.isArray(input.plans) ? input.plans.slice(0, 12) : [];
  const ledger = record(input.ledger);
  const assignments = ledger.schema === "worldstream/house-allowance-ledger@1"
    ? Object.values(record(ledger.assignments)).slice(0, 256) : [];
  const attempts = assignments.flatMap(assignment => Object.values(record(record(assignment).attempts)).slice(0, 10));
  const operations = Array.isArray(input.operations) ? input.operations.slice(0, 256) : [];
  return {
    schema: "worldstream/hosted-acceptance-house-wait-diagnostic/v1",
    stage: "house_endorsement",
    session_status: code(input.sessionStatus, ["idle", "connecting", "synchronizing", "live", "disconnected", "failed", "closed"]),
    phase: code(input.phase, ["lobby", "briefing", "negotiation", "commitment", "resolution", "result", "complete", "completed", "abandoned"]),
    plan_count: plans.length,
    endorsement_count: plans.reduce<number>((sum, plan) => sum + Math.min(count(record(plan).endorsements) ?? 0, 3), 0),
    provider_house_completion_count: count(record(input.providerMetrics).house_completion_count),
    ledger_status: ledger.schema === "worldstream/house-allowance-ledger@1" ? "available" : "unavailable",
    assignment_count: assignments.length,
    attempt_states: histogram(attempts.map(attempt => record(attempt).state), ATTEMPT_STATES),
    attempt_failure_codes: histogram(attempts.flatMap(attempt => record(attempt).failure_code == null ? [] : [record(attempt).failure_code]), FAILURE_CODES),
    // These are retained operation states, not a fresh liveness claim.
    retained_operations_status: Array.isArray(input.operations) ? "available" : "unavailable",
    retained_process_states: histogram(operations.map(operation => record(operation).state), PROCESS_STATES),
    retained_process_failure_codes: histogram(operations.flatMap(operation => record(operation).failure == null ? [] : [record(record(operation).failure).code]), ["host_start_failed", "host_exited"]),
  };
}

async function boundedJson(path: string, maximum: number): Promise<unknown> {
  const file = await open(path, "r");
  try {
    const bytes = Buffer.alloc(maximum + 1);
    const { bytesRead } = await file.read(bytes, 0, bytes.length, 0);
    if (bytesRead > maximum) throw new Error("diagnostic_input_limit");
    return JSON.parse(bytes.subarray(0, bytesRead).toString("utf8"));
  } finally { await file.close(); }
}

export async function collectHouseWaitDiagnostic(input: {
  sessionStatus: unknown; phase: unknown; plans: unknown;
  stateDirectory: string; providerMetrics: () => Promise<unknown>;
}) {
  const operationRoot = join(input.stateDirectory, "managed-agent-hosts", "operations");
  const [ledger, operations, providerMetrics] = await Promise.allSettled([
    boundedJson(join(input.stateDirectory, "hosted-house-runners", "units", "allowance", "allowances.json"), 2 * 1024 * 1024),
    (async () => {
      const entries = await readdir(operationRoot, { withFileTypes: true });
      if (entries.length > 256) throw new Error("diagnostic_input_limit");
      return Promise.all(entries.filter(entry => entry.isFile() && entry.name.endsWith(".json"))
        .map(entry => boundedJson(join(operationRoot, entry.name), 64 * 1024)));
    })(),
    // The injected fetch must itself be bounded; errors are never serialized.
    input.providerMetrics(),
  ]);
  return houseWaitDiagnostic({ ...input,
    ledger: ledger.status === "fulfilled" ? ledger.value : null,
    operations: operations.status === "fulfilled" ? operations.value : null,
    providerMetrics: providerMetrics.status === "fulfilled" ? providerMetrics.value : null,
  });
}
