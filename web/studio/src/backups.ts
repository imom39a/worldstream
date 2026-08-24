export type BackupStorageProfile = "sqlite_bundled" | "postgres_primary" | "ephemeral";
export type BackupStorageHealth = "healthy" | "unhealthy" | "unavailable";
export type BackupVerification = "pass" | "failed" | "unavailable";
export type BackupFreshness =
  | { status: "fresh"; observed_at: string }
  | { status: "stale"; observed_at: string; reason: string }
  | { status: "unavailable"; reason: string };

export interface BackupProfileStatus {
  schema: "worldstream/studio-backup-profile-status/v1";
  storage_profile: BackupStorageProfile;
  storage_health: BackupStorageHealth;
  live_backup_supported: boolean;
  verification: BackupVerification;
  freshness: BackupFreshness;
}

export interface BackupOperationStatus {
  schema: "worldstream/studio-backup-operation/v1";
  operation_id: string;
  storage_profile: BackupStorageProfile;
  storage_health: BackupStorageHealth;
  phase: "running" | "retrying" | "complete" | "failed" | "unsupported";
  native_verification: BackupVerification;
  semantic_verification: BackupVerification;
  semantic_verification_reason: string | null;
  freshness: BackupFreshness;
  destination: null | {
    kind: "studio_managed_local";
    artifact_name: "backup.sqlite3";
    byte_length: number;
    blake3_digest: string;
  };
  failure_reason: string | null;
}

export type BackupOperationLoadResult =
  | { availability: "available"; operation: BackupOperationStatus }
  | { availability: "unavailable"; operation: null };

const ACTIVE_BACKUP_OPERATION_KEY = "worldstream.studio.active-backup-operation.v1";

export function loadBackupOperationId(storage: Storage = window.localStorage): string | null {
  const value = storage.getItem(ACTIVE_BACKUP_OPERATION_KEY);
  return isOperationId(value) ? value : null;
}

export function saveBackupOperationId(
  operationId: string | null,
  storage: Storage = window.localStorage,
) {
  if (operationId === null) storage.removeItem(ACTIVE_BACKUP_OPERATION_KEY);
  else if (isOperationId(operationId)) storage.setItem(ACTIVE_BACKUP_OPERATION_KEY, operationId);
}

export async function loadBackupProfile(
  fetcher: typeof fetch = fetch,
): Promise<BackupProfileStatus | null> {
  return loadJson("/api/v1/backups/health", isBackupProfile, fetcher);
}

export async function loadBackupOperation(
  operationId: string,
  fetcher: typeof fetch = fetch,
): Promise<BackupOperationStatus | null> {
  return (await loadBackupOperationState(operationId, fetcher)).operation;
}

export async function loadBackupOperationState(
  operationId: string,
  fetcher: typeof fetch = fetch,
): Promise<BackupOperationLoadResult> {
  if (!isOperationId(operationId)) return { availability: "unavailable", operation: null };
  try {
    const response = await fetcher(`/api/v1/backups/${operationId}`, {
      method: "GET",
      cache: "no-store",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) return { availability: "unavailable", operation: null };
    const value: unknown = await response.json();
    return isBackupOperation(value) && value.operation_id === operationId
      ? { availability: "available", operation: value }
      : { availability: "unavailable", operation: null };
  } catch {
    return { availability: "unavailable", operation: null };
  }
}

export async function runBackupOperation(
  operationId: string,
  fetcher: typeof fetch = fetch,
): Promise<BackupOperationStatus | null> {
  if (!isOperationId(operationId)) return null;
  return loadJson(`/api/v1/backups/${operationId}`, isBackupOperation, fetcher, "POST");
}

export function newBackupOperationId(now = Date.now()) {
  return `backup-${now.toString(36)}`;
}

async function loadJson<T>(
  url: string,
  guard: (value: unknown) => value is T,
  fetcher: typeof fetch,
  method = "GET",
): Promise<T | null> {
  try {
    const response = await fetcher(url, {
      method,
      cache: "no-store",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) return null;
    const value: unknown = await response.json();
    return guard(value) ? value : null;
  } catch {
    return null;
  }
}

function isBackupProfile(value: unknown): value is BackupProfileStatus {
  return isRecord(value)
    && value.schema === "worldstream/studio-backup-profile-status/v1"
    && isProfile(value.storage_profile)
    && isStorageHealth(value.storage_health)
    && typeof value.live_backup_supported === "boolean"
    && isVerification(value.verification)
    && isFreshness(value.freshness);
}

function isBackupOperation(value: unknown): value is BackupOperationStatus {
  if (!isRecord(value) || value.schema !== "worldstream/studio-backup-operation/v1") return false;
  if (!isOperationId(value.operation_id) || !isProfile(value.storage_profile)) return false;
  if (!isStorageHealth(value.storage_health)) return false;
  if (!["running", "retrying", "complete", "failed", "unsupported"].includes(String(value.phase))) return false;
  if (!isVerification(value.native_verification) || !isVerification(value.semantic_verification)) return false;
  if (value.semantic_verification_reason !== null
    && !isFailureReason(value.semantic_verification_reason)) return false;
  if (!isFreshness(value.freshness)) return false;
  if (value.failure_reason !== null && !isFailureReason(value.failure_reason)) return false;
  const destinationValid = value.destination === null || (isRecord(value.destination)
    && hasOnlyKeys(value.destination, ["kind", "artifact_name", "byte_length", "blake3_digest"])
    && value.destination.kind === "studio_managed_local"
    && value.destination.artifact_name === "backup.sqlite3"
    && typeof value.destination.byte_length === "number"
    && Number.isSafeInteger(value.destination.byte_length)
    && value.destination.byte_length >= 0
    && isDigest(value.destination.blake3_digest));
  if (!destinationValid) return false;

  const semanticReasonCoherent = value.semantic_verification === "unavailable"
    ? value.semantic_verification_reason !== null
    : value.semantic_verification_reason === null;
  if (!semanticReasonCoherent) return false;
  if (value.phase === "complete") {
    return value.storage_health === "healthy"
      && value.destination !== null
      && value.native_verification === "pass"
      && value.semantic_verification !== "failed"
      && value.freshness.status === "fresh"
      && value.failure_reason === null;
  }
  if (value.phase === "failed") {
    return value.destination === null
      && value.native_verification === "failed"
      && value.semantic_verification === "unavailable"
      && value.freshness.status === "unavailable"
      && value.failure_reason !== null;
  }
  if (value.phase === "unsupported") {
    return value.destination === null
      && value.native_verification === "unavailable"
      && value.semantic_verification === "unavailable"
      && value.freshness.status === "unavailable"
      && value.failure_reason === null;
  }
  return value.destination === null
    && value.native_verification === "unavailable"
    && value.semantic_verification === "unavailable"
    && value.freshness.status === "unavailable"
    && (value.phase === "running" ? value.failure_reason === null : value.failure_reason !== null);
}

function isFreshness(value: unknown): value is BackupFreshness {
  if (!isRecord(value)) return false;
  if (value.status === "fresh") return isBoundedText(value.observed_at);
  if (value.status === "stale") return isBoundedText(value.observed_at) && isBoundedText(value.reason);
  return value.status === "unavailable" && isBoundedText(value.reason);
}
function isVerification(value: unknown): value is BackupVerification {
  return ["pass", "failed", "unavailable"].includes(String(value));
}
function isProfile(value: unknown): value is BackupStorageProfile {
  return ["sqlite_bundled", "postgres_primary", "ephemeral"].includes(String(value));
}
function isStorageHealth(value: unknown): value is BackupStorageHealth {
  return ["healthy", "unhealthy", "unavailable"].includes(String(value));
}
function isOperationId(value: unknown): value is string {
  return typeof value === "string" && /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/.test(value);
}
function isDigest(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
}
function isFailureReason(value: unknown): value is string {
  return typeof value === "string" && /^[a-z0-9_]{1,64}$/.test(value);
}
function isBoundedText(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 256;
}
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function hasOnlyKeys(value: Record<string, unknown>, keys: string[]) {
  return Object.keys(value).every((key) => keys.includes(key));
}
