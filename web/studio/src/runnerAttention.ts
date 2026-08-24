export type RunnerAttentionFreshness = "live" | "stale" | "unavailable";
export type RunnerCompatibility = "compatible" | "incompatible" | "unavailable";
export type ActivationAttentionState =
  | "idle"
  | "waiting"
  | "leased"
  | "delayed"
  | "attention"
  | "unavailable";
export type RunnerRestartState =
  | "requested"
  | "restarting"
  | "reconciling"
  | "succeeded"
  | "failed";

export interface RunnerAttentionCapacity {
  advertised: number;
  in_use: number;
  available: number;
}

export interface RunnerAttentionStatus {
  runner_id: string;
  instance_id: string | null;
  connection: "connected" | "disconnected";
  freshness: RunnerAttentionFreshness;
  capacity: RunnerAttentionCapacity;
  compatible_assignments: number;
  incompatible_assignments: number;
  observed_at_unix_ms: number | null;
  next_action: string;
}

export interface RunnerRestartOperation {
  schema: "worldstream/studio-runner-restart-operation/v1";
  operation_id: string;
  instance_id: string;
  attempts: number;
  state: RunnerRestartState;
  explanation: string;
  next_action: string;
}

export interface RunnerAttentionOperations {
  schema: "worldstream/studio-runner-attention/v1";
  freshness: RunnerAttentionFreshness;
  observed_at_unix_ms: number | null;
  runners: RunnerAttentionStatus[];
  restart_attempts: RunnerRestartOperation[];
}

export interface AgentSeatAttention {
  seat_id: string;
  instance_id: string | null;
  compatibility: RunnerCompatibility;
  capacity: RunnerAttentionCapacity;
  activation: {
    state: ActivationAttentionState;
    waiting: number;
    leased: number;
  };
  freshness: RunnerAttentionFreshness;
  next_action: string;
}

export interface TaskAgentAttention {
  schema: "worldstream/studio-task-agent-attention/v1";
  freshness: RunnerAttentionFreshness;
  observed_at_unix_ms: number | null;
  seats: AgentSeatAttention[];
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const maxRows = 256;
const sensitiveKeys = new Set([
  "activation_id", "claim_id", "invocation", "invocation_context", "context",
  "payload", "prompt", "response", "memory", "bearer", "token", "token_hash",
  "secret", "secret_reference", "secret_ref", "credential", "credentials",
  "command", "executable", "process_list",
]);

export async function loadRunnerAttention(
  fetcher: Fetcher = fetch,
): Promise<RunnerAttentionOperations | null> {
  return loadJson("/api/v1/runner-attention", isRunnerAttentionOperations, fetcher);
}

export async function loadTaskAgentAttention(
  roomId: string,
  fetcher: Fetcher = fetch,
): Promise<TaskAgentAttention | null> {
  if (!isUlid(roomId)) return null;
  return loadJson(
    `/api/v1/rooms/${encodeURIComponent(roomId)}/agent-attention`,
    isTaskAgentAttention,
    fetcher,
  );
}

export async function requestRunnerRestart(
  instanceId: string,
  operationId: string,
  fetcher: Fetcher = fetch,
): Promise<RunnerRestartOperation | null> {
  if (!isIdentifier(instanceId) || !isUlid(operationId)) return null;
  try {
    const response = await fetcher(
      `/api/v1/runner-attention/${encodeURIComponent(instanceId)}/restart`,
      {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({ operation_id: operationId }),
      },
    );
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isRunnerRestartOperation(value)
      && value.operation_id === operationId
      && value.instance_id === instanceId
      ? value
      : null;
  } catch {
    return null;
  }
}

export function newRunnerRestartOperationId(
  now: number = Date.now(),
  random: Uint8Array = crypto.getRandomValues(new Uint8Array(10)),
): string {
  if (!Number.isSafeInteger(now) || now < 0 || now > 0xffffffffffff || random.length !== 10) {
    throw new Error("cannot create bounded restart operation identity");
  }
  const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let time = BigInt(now);
  let prefix = "";
  for (let index = 0; index < 10; index += 1) {
    prefix = alphabet[Number(time & 31n)] + prefix;
    time >>= 5n;
  }
  let entropy = 0n;
  for (const byte of random) entropy = (entropy << 8n) | BigInt(byte);
  let suffix = "";
  for (let index = 0; index < 16; index += 1) {
    suffix = alphabet[Number(entropy & 31n)] + suffix;
    entropy >>= 5n;
  }
  return `${prefix}${suffix}`;
}

async function loadJson<T>(
  path: string,
  validate: (value: unknown) => value is T,
  fetcher: Fetcher,
): Promise<T | null> {
  try {
    const response = await fetcher(path, {
      cache: "no-store",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return validate(value) ? value : null;
  } catch {
    return null;
  }
}

export function isRunnerAttentionOperations(value: unknown): value is RunnerAttentionOperations {
  if (containsSensitiveField(value) || !isExactRecord(value, [
    "schema", "freshness", "observed_at_unix_ms", "runners", "restart_attempts",
  ]) || value.schema !== "worldstream/studio-runner-attention/v1" ||
    !isFreshness(value.freshness) || !isObservedAt(value.observed_at_unix_ms) ||
    !isBoundedArray(value.runners, isRunnerStatus) ||
    !isBoundedArray(value.restart_attempts, isRunnerRestartOperation)) return false;
  if ((value.freshness === "unavailable") !== (value.observed_at_unix_ms === null)) return false;
  const runners = value.runners as RunnerAttentionStatus[];
  const attempts = value.restart_attempts as RunnerRestartOperation[];
  return unique(runners.map((runner) => runner.runner_id))
    && unique(attempts.map((attempt) => attempt.operation_id));
}

export function isTaskAgentAttention(value: unknown): value is TaskAgentAttention {
  if (containsSensitiveField(value) || !isExactRecord(value, [
    "schema", "freshness", "observed_at_unix_ms", "seats",
  ]) || value.schema !== "worldstream/studio-task-agent-attention/v1" ||
    !isFreshness(value.freshness) || !isObservedAt(value.observed_at_unix_ms) ||
    !isBoundedArray(value.seats, isSeat)) return false;
  if ((value.freshness === "unavailable") !== (value.observed_at_unix_ms === null)) return false;
  return unique((value.seats as AgentSeatAttention[]).map((seat) => seat.seat_id));
}

function isRunnerStatus(value: unknown): value is RunnerAttentionStatus {
  if (!isExactRecord(value, [
    "runner_id", "instance_id", "connection", "freshness", "capacity",
    "compatible_assignments", "incompatible_assignments", "observed_at_unix_ms", "next_action",
  ]) || !isUlid(value.runner_id) || !(value.instance_id === null || isIdentifier(value.instance_id)) ||
    !["connected", "disconnected"].includes(String(value.connection)) ||
    !isFreshness(value.freshness) || !isCapacity(value.capacity) ||
    !isCount(value.compatible_assignments) || !isCount(value.incompatible_assignments) ||
    !isObservedAt(value.observed_at_unix_ms) || !isText(value.next_action)) return false;
  return value.connection === "connected"
    ? value.freshness !== "unavailable" && value.observed_at_unix_ms !== null
    : value.freshness === "unavailable";
}

function isRunnerRestartOperation(value: unknown): value is RunnerRestartOperation {
  if (!isExactRecord(value, [
    "schema", "operation_id", "instance_id", "attempts", "state", "explanation", "next_action",
  ]) || value.schema !== "worldstream/studio-runner-restart-operation/v1" ||
    !isUlid(value.operation_id) || !isIdentifier(value.instance_id) || !isCount(value.attempts) ||
    !["requested", "restarting", "reconciling", "succeeded", "failed"].includes(String(value.state)) ||
    !isText(value.explanation) || !isText(value.next_action)) return false;
  return value.state === "requested" ? value.attempts === 0 : value.attempts === 1;
}

function isSeat(value: unknown): value is AgentSeatAttention {
  if (!isExactRecord(value, [
    "seat_id", "instance_id", "compatibility", "capacity", "activation", "freshness", "next_action",
  ]) || !isIdentifier(value.seat_id) || !(value.instance_id === null || isIdentifier(value.instance_id)) ||
    !["compatible", "incompatible", "unavailable"].includes(String(value.compatibility)) ||
    !isCapacity(value.capacity) || !isFreshness(value.freshness) || !isText(value.next_action) ||
    !isExactRecord(value.activation, ["state", "waiting", "leased"]) ||
    !["idle", "waiting", "leased", "delayed", "attention", "unavailable"].includes(String(value.activation.state)) ||
    !isCount(value.activation.waiting) || !isCount(value.activation.leased)) return false;
  const activation = value.activation as AgentSeatAttention["activation"];
  if (activation.state === "idle" || activation.state === "unavailable") {
    return activation.waiting === 0 && activation.leased === 0;
  }
  if (activation.state === "waiting" || activation.state === "attention") {
    return activation.waiting > 0 && activation.leased === 0;
  }
  return activation.leased > 0;
}

function isCapacity(value: unknown): value is RunnerAttentionCapacity {
  return isExactRecord(value, ["advertised", "in_use", "available"])
    && isPositiveCount(value.advertised)
    && isCount(value.in_use)
    && isCount(value.available)
    && Number(value.in_use) + Number(value.available) === value.advertised;
}

function containsSensitiveField(value: unknown, depth = 0): boolean {
  if (depth > 12) return true;
  if (Array.isArray(value)) return value.some((item) => containsSensitiveField(item, depth + 1));
  if (!isRecord(value)) return false;
  return Object.entries(value).some(([key, nested]) =>
    sensitiveKeys.has(key.toLowerCase()) || containsSensitiveField(nested, depth + 1));
}

function isExactRecord(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return isRecord(value) && Object.keys(value).length === keys.length && keys.every((key) => key in value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isBoundedArray<T>(value: unknown, validate: (item: unknown) => item is T): value is T[] {
  return Array.isArray(value) && value.length <= maxRows && value.every(validate);
}

function isUlid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9A-HJKMNP-TV-Z]{26}$/.test(value);
}

function isIdentifier(value: unknown): value is string {
  return typeof value === "string" && /^[a-z0-9][a-z0-9_-]{0,63}$/.test(value);
}

function isFreshness(value: unknown): value is RunnerAttentionFreshness {
  return ["live", "stale", "unavailable"].includes(String(value));
}

function isObservedAt(value: unknown): value is number | null {
  return value === null || (isCount(value) && value > 0);
}

function isCount(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) >= 0 && Number(value) <= 0xffffffff;
}

function isPositiveCount(value: unknown): value is number {
  return isCount(value) && value > 0;
}

function isText(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 512 && !/[\0\r\n]/.test(value);
}

function unique(values: string[]): boolean {
  return new Set(values).size === values.length;
}
