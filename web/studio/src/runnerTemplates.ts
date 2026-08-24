export type RunnerInstanceState =
  | "stopped"
  | "starting"
  | "running"
  | "stopping"
  | "failed"
  | "unavailable";

export type RunnerInstanceLifecycleAction = "start" | "stop" | "restart";
export type RunnerInstanceHealth = "healthy" | "stopped" | "unavailable";
export type RunnerFreshness = "fresh" | "stale" | "unavailable";
export type RunnerSecretKind =
  | "host_authority"
  | "membership_authority"
  | "runner_authority"
  | "model_provider";

export interface RunnerCompatibilityRule {
  activity_pack_id: string;
  exact_revisions: string[];
}

export interface RunnerTemplateRevision {
  template_id: string;
  revision: string;
  display_name: string;
  executable_blake3: string;
  compatibility: RunnerCompatibilityRule[];
  capacity: { maximum_concurrent_invocations: number };
  health_stale_after_ms: number;
  non_secret_settings: string[];
  secret_settings: Array<{
    key: string;
    kind: RunnerSecretKind;
    configured: boolean;
  }>;
  instances: string[];
}

export interface RunnerTemplateCatalog {
  schema: "worldstream/studio-runner-template-catalog/v1";
  templates: RunnerTemplateRevision[];
}

export interface RunnerInstanceStatus {
  instance_id: string;
  template_id: string;
  template_revision: string;
  state: RunnerInstanceState;
  operation_id: number;
  managed_by_supervisor: boolean;
  compatibility: RunnerCompatibilityRule[];
  capacity: { maximum: number; in_use: number; available: number };
  health: RunnerInstanceHealth;
  freshness: RunnerFreshness;
  observed_at_unix_ms: number | null;
  failure: {
    code: string;
    explanation: string;
    next_action: string;
  } | null;
}

export interface RunnerInstanceStatusResponse {
  schema: "worldstream/studio-runner-instance-status/v1";
  instances: RunnerInstanceStatus[];
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

const maxRows = 256;

export async function loadRunnerTemplates(
  fetcher: Fetcher = fetch,
): Promise<RunnerTemplateCatalog | null> {
  return loadJson("/api/v1/runner-templates", isRunnerTemplateCatalog, fetcher);
}

export async function loadRunnerInstances(
  fetcher: Fetcher = fetch,
): Promise<RunnerInstanceStatusResponse | null> {
  return loadJson("/api/v1/runner-instances", isRunnerInstanceStatusResponse, fetcher);
}

export async function requestRunnerInstanceLifecycle(
  instanceId: string,
  action: RunnerInstanceLifecycleAction,
  fetcher: Fetcher = fetch,
): Promise<RunnerInstanceStatusResponse | null> {
  if (!isStableId(instanceId) || !["start", "stop", "restart"].includes(action)) return null;
  try {
    const response = await fetcher(
      `/api/v1/runner-instances/${encodeURIComponent(instanceId)}/${action}`,
      { method: "POST", headers: { accept: "application/json" } },
    );
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return isRunnerInstanceStatusResponse(value) ? value : null;
  } catch {
    return null;
  }
}

async function loadJson<T>(
  path: string,
  validate: (value: unknown) => value is T,
  fetcher: Fetcher,
): Promise<T | null> {
  try {
    const response = await fetcher(path, { headers: { accept: "application/json" } });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return validate(value) ? value : null;
  } catch {
    return null;
  }
}

function isRunnerTemplateCatalog(value: unknown): value is RunnerTemplateCatalog {
  return isExactRecord(value, ["schema", "templates"])
    && value.schema === "worldstream/studio-runner-template-catalog/v1"
    && isBoundedArray(value.templates, isRunnerTemplate)
    && unique(value.templates.map((template) => `${template.template_id}\0${template.revision}`));
}

function isRunnerTemplate(value: unknown): value is RunnerTemplateRevision {
  if (!isExactRecord(value, [
    "template_id", "revision", "display_name", "executable_blake3", "compatibility",
    "capacity", "health_stale_after_ms", "non_secret_settings", "secret_settings", "instances",
  ])) return false;
  return isStableId(value.template_id)
    && isRevision(value.revision)
    && isText(value.display_name)
    && typeof value.executable_blake3 === "string"
    && /^[0-9a-f]{64}$/.test(value.executable_blake3)
    && isBoundedArray(value.compatibility, isCompatibilityRule)
    && value.compatibility.length > 0
    && isExactRecord(value.capacity, ["maximum_concurrent_invocations"])
    && isPositiveCount(value.capacity.maximum_concurrent_invocations)
    && isPositiveCount(value.health_stale_after_ms)
    && isBoundedArray(value.non_secret_settings, isEnvironmentKey)
    && isBoundedArray(value.secret_settings, isSecretSetting)
    && isBoundedArray(value.instances, isStableId)
    && value.instances.length > 0
    && unique(value.instances);
}

function isCompatibilityRule(value: unknown): value is RunnerCompatibilityRule {
  return isExactRecord(value, ["activity_pack_id", "exact_revisions"])
    && isStableId(value.activity_pack_id)
    && isBoundedArray(value.exact_revisions, isRevision)
    && value.exact_revisions.length > 0
    && unique(value.exact_revisions);
}

function isSecretSetting(value: unknown): value is RunnerTemplateRevision["secret_settings"][number] {
  return isExactRecord(value, ["key", "kind", "configured"])
    && isEnvironmentKey(value.key)
    && ["host_authority", "membership_authority", "runner_authority", "model_provider"].includes(String(value.kind))
    && typeof value.configured === "boolean";
}

function isRunnerInstanceStatusResponse(value: unknown): value is RunnerInstanceStatusResponse {
  return isExactRecord(value, ["schema", "instances"])
    && value.schema === "worldstream/studio-runner-instance-status/v1"
    && isBoundedArray(value.instances, isRunnerInstanceStatus)
    && unique(value.instances.map((instance) => instance.instance_id));
}

function isRunnerInstanceStatus(value: unknown): value is RunnerInstanceStatus {
  if (!isExactRecord(value, [
    "instance_id", "template_id", "template_revision", "state", "operation_id",
    "managed_by_supervisor", "compatibility", "capacity", "health", "freshness",
    "observed_at_unix_ms", "failure",
  ])) return false;
  if (!isStableId(value.instance_id)
    || !isStableId(value.template_id)
    || !isRevision(value.template_revision)
    || !["stopped", "starting", "running", "stopping", "failed", "unavailable"].includes(String(value.state))
    || !isCount(value.operation_id)
    || typeof value.managed_by_supervisor !== "boolean"
    || !isBoundedArray(value.compatibility, isCompatibilityRule)
    || !isExactRecord(value.capacity, ["maximum", "in_use", "available"])
    || !isPositiveCount(value.capacity.maximum)
    || !isCount(value.capacity.in_use)
    || !isCount(value.capacity.available)
    || Number(value.capacity.in_use) + Number(value.capacity.available) !== value.capacity.maximum
    || !["healthy", "stopped", "unavailable"].includes(String(value.health))
    || !["fresh", "stale", "unavailable"].includes(String(value.freshness))
    || !(value.observed_at_unix_ms === null || isCount(value.observed_at_unix_ms))) return false;
  return value.failure === null || (
    isExactRecord(value.failure, ["code", "explanation", "next_action"])
    && isStableId(value.failure.code)
    && isText(value.failure.explanation)
    && isText(value.failure.next_action)
  );
}

function isExactRecord(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return typeof value === "object"
    && value !== null
    && !Array.isArray(value)
    && Object.keys(value).length === keys.length
    && keys.every((key) => key in value);
}

function isBoundedArray<T>(
  value: unknown,
  validate: (item: unknown) => item is T,
): value is T[] {
  return Array.isArray(value) && value.length <= maxRows && value.every(validate);
}

function isStableId(value: unknown): value is string {
  return typeof value === "string" && /^[a-z0-9][a-z0-9_-]{0,63}$/.test(value);
}

function isRevision(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(value);
}

function isEnvironmentKey(value: unknown): value is string {
  return typeof value === "string" && /^[A-Z_][A-Z0-9_]{0,127}$/.test(value);
}

function isText(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 256 && !/[\0\r\n]/.test(value);
}

function isCount(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) >= 0;
}

function isPositiveCount(value: unknown): value is number {
  return isCount(value) && value > 0;
}

function unique(values: string[]): boolean {
  return new Set(values).size === values.length;
}
