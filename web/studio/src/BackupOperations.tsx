import type { BackupOperationStatus, BackupProfileStatus } from "./backups";
import "./backupOperations.css";

export function BackupOperations({
  profile,
  operation,
  operationId = null,
  operationStatusAvailable = true,
  loading = false,
  onStart,
  onRetry,
}: {
  profile: BackupProfileStatus | null;
  operation: BackupOperationStatus | null;
  operationId?: string | null;
  operationStatusAvailable?: boolean;
  loading?: boolean;
  onStart?: () => void;
  onRetry?: () => void;
}) {
  return (
    <section className="backup-operations" id="backups" aria-busy={loading}>
      <p className="eyebrow">Operations · Backups</p>
      <h2>Live backup preparation</h2>
      {profile === null ? (
        <p role="status">Backup profile health unavailable.</p>
      ) : (
        <dl className="backup-facts">
          <Fact label="Storage profile" value={profileLabel(profile.storage_profile)} />
          <Fact label="Storage health" value={label(profile.storage_health)} />
          <Fact label="Live backup" value={profile.live_backup_supported ? "Supported" : "Provider-managed or unsupported"} />
          <Fact label="Profile verification" value={label(profile.verification)} />
          <Fact label="Profile freshness" value={freshnessLabel(profile)} />
          {operation ? <Fact label="Operation verification" value={verificationLabel(operation)} /> : null}
          {operation ? <Fact label="Operation freshness" value={freshnessLabel(operation)} /> : null}
        </dl>
      )}
      {profile !== null && profile.storage_health !== "healthy" ? (
        <p role="alert">
          Storage is {profile.storage_health}. Verify daemon storage health, then retry when it is healthy.
        </p>
      ) : null}
      {!operationStatusAvailable && operationId !== null ? (
        <div role="alert" className="backup-result">
          <strong>Backup operation status unavailable</strong>
          <p>The active operation identity is retained, but its current result could not be verified. Retry the original operation; Studio will not create a replacement.</p>
          <button type="button" disabled={loading} onClick={onRetry}>
            {loading ? "Retrying original backup…" : "Retry original backup"}
          </button>
        </div>
      ) : operation ? <OperationResult operation={operation} onRetry={onRetry} loading={loading} /> : null}
      {profile?.live_backup_supported && canStartNewOperation(
        operationId,
        operationStatusAvailable,
        operation,
      ) ? (
        <button type="button" disabled={loading || operation?.phase === "running" || operation?.phase === "retrying"} onClick={onStart}>
          {loading || operation?.phase === "running" ? "Preparing backup…" : "Prepare backup"}
        </button>
      ) : null}
      <div className="backup-boundary">
        <strong>Safety boundary</strong>
        <p>Studio uses its fixed protected backup destination. It does not browse paths, restore data, schedule provider snapshots, or modify the running store on failure.</p>
      </div>
    </section>
  );
}

function canStartNewOperation(
  operationId: string | null,
  available: boolean,
  operation: BackupOperationStatus | null,
) {
  if (operationId === null) return true;
  return available && operation !== null
    && ["complete", "failed", "unsupported"].includes(operation.phase);
}

function OperationResult({
  operation,
  onRetry,
  loading,
}: {
  operation: BackupOperationStatus;
  onRetry?: () => void;
  loading: boolean;
}) {
  if (operation.phase === "unsupported") return <p role="status">This profile requires a provider-managed backup.</p>;
  if (operation.phase === "failed") return (
    <p role="alert">
      Backup failed safely ({failureLabel(operation.failure_reason)}). The running store was left untouched. Verify daemon storage health, then retry with a new operation.
    </p>
  );
  if (operation.phase === "running") return <p role="status">The durable operation is running and will reconcile after restart.</p>;
  if (operation.phase === "retrying") return (
    <div role="status">
      <p>The original backup operation is unresolved. Retry it once daemon storage health is available; Studio will reconcile any artifact already published.</p>
      <button type="button" disabled={loading} onClick={onRetry}>
        {loading ? "Retrying original backup…" : "Retry original backup"}
      </button>
    </div>
  );
  const fullyVerified = operation.destination !== null
    && operation.native_verification === "pass"
    && operation.semantic_verification === "pass";
  return (
    <div className="backup-result" role="status">
      <strong>{fullyVerified ? "Backup complete and verified" : "Backup artifact prepared"}</strong>
      {!fullyVerified ? <p>Native artifact verified. Full semantic restore verification was not run.</p> : null}
      <p>{operation.destination?.artifact_name} · {formatBytes(operation.destination?.byte_length ?? 0)}</p>
      <code>{operation.destination?.blake3_digest}</code>
    </div>
  );
}

function Fact({ label: factLabel, value }: { label: string; value: string }) {
  return <div><dt>{factLabel}</dt><dd>{value}</dd></div>;
}
function profileLabel(profile: BackupProfileStatus["storage_profile"]) {
  if (profile === "sqlite_bundled") return "SQLite bundled";
  if (profile === "postgres_primary") return "PostgreSQL primary";
  return "Ephemeral";
}
function label(value: string) { return value.charAt(0).toUpperCase() + value.slice(1); }
function verificationLabel(operation: BackupOperationStatus) {
  return `${label(operation.native_verification)} native · ${label(operation.semantic_verification)} semantic`;
}
function freshnessLabel(value: BackupOperationStatus | BackupProfileStatus) {
  if (value.freshness.status === "fresh") return `Fresh · ${value.freshness.observed_at}`;
  if (value.freshness.status === "stale") return `Stale · ${value.freshness.observed_at} · ${labelReason(value.freshness.reason)}`;
  return `Unavailable · ${labelReason(value.freshness.reason)}`;
}
function failureLabel(reason: string | null) { return labelReason(reason ?? "reason_unavailable"); }
function labelReason(reason: string) { return reason.replaceAll("_", " "); }
function formatBytes(bytes: number) { return `${bytes.toLocaleString("en-US")} bytes`; }
