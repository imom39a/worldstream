import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { BackupOperations } from "./BackupOperations";
import type { BackupOperationStatus, BackupProfileStatus } from "./backups";

const profile: BackupProfileStatus = {
  schema: "worldstream/studio-backup-profile-status/v1", storage_profile: "sqlite_bundled",
  storage_health: "healthy", live_backup_supported: true, verification: "unavailable",
  freshness: { status: "unavailable", reason: "no_operation_selected" },
};
const complete: BackupOperationStatus = {
  schema: "worldstream/studio-backup-operation/v1", operation_id: "backup-stable",
  storage_profile: "sqlite_bundled", storage_health: "healthy", phase: "complete",
  native_verification: "pass", semantic_verification: "pass",
  semantic_verification_reason: null,
  freshness: { status: "fresh", observed_at: "2026-08-23T12:00:00Z" },
  destination: { kind: "studio_managed_local", artifact_name: "backup.sqlite3", byte_length: 4096, blake3_digest: "a".repeat(64) }, failure_reason: null,
};

describe("Backup Operations", () => {
  it("keeps profile health, verification, and freshness separate", () => {
    const dom = renderToStaticMarkup(<BackupOperations profile={profile} operation={complete} />);
    expect(dom).toContain("SQLite bundled"); expect(dom).toContain("Healthy");
    expect(dom).toContain("Pass native · Pass semantic"); expect(dom).toContain("Fresh · 2026");
    expect(dom).toContain("Backup complete and verified"); expect(dom).toContain("backup.sqlite3");
    expect(dom).toContain("does not browse paths"); expect(dom).not.toContain("/Users/");
  });

  it("surfaces profile verification and freshness before any operation", () => {
    const dom = renderToStaticMarkup(<BackupOperations profile={{
      ...profile,
      storage_health: "unhealthy",
      live_backup_supported: false,
      verification: "failed",
      freshness: { status: "stale", observed_at: "2026-08-23T11:00:00Z", reason: "verification_expired" },
    }} operation={null} />);
    expect(dom).toContain("Failed"); expect(dom).toContain("Stale · 2026");
    expect(dom).toContain("Verify daemon storage health");
  });

  it("truthfully renders provider-managed and safely failed states", () => {
    const unsupported = renderToStaticMarkup(<BackupOperations profile={{ ...profile, storage_profile: "postgres_primary", live_backup_supported: false }} operation={{ ...complete, storage_profile: "postgres_primary", phase: "unsupported", native_verification: "unavailable", semantic_verification: "unavailable", semantic_verification_reason: "profile_managed_backup_required", freshness: { status: "unavailable", reason: "profile_managed_backup_required" }, destination: null }} />);
    expect(unsupported).toContain("Provider-managed or unsupported"); expect(unsupported).toContain("requires a provider-managed backup");
    const failed = renderToStaticMarkup(<BackupOperations profile={profile} operation={{ ...complete, phase: "failed", destination: null, native_verification: "failed", semantic_verification: "unavailable", semantic_verification_reason: "verification_failed", freshness: { status: "unavailable", reason: "verification_failed" }, failure_reason: "verification_failed" }} />);
    expect(failed).toContain("running store was left untouched");
    expect(failed).toContain("verification failed"); expect(failed).toContain("retry with a new operation");
  });

  it("exposes retry of the original operation after an unavailable transport", () => {
    const retrying = renderToStaticMarkup(<BackupOperations profile={profile} operation={{
      ...complete,
      phase: "retrying",
      storage_health: "unavailable",
      native_verification: "unavailable",
      semantic_verification: "unavailable",
      semantic_verification_reason: "operation_reconciliation_required",
      freshness: { status: "unavailable", reason: "storage_unavailable" },
      destination: null,
      failure_reason: "storage_unavailable",
    }} />);
    expect(retrying).toContain("Retry original backup");
    expect(retrying).toContain("artifact already published");
  });

  it("does not claim full verification when semantic restore verification was not run", () => {
    const nativeOnly = renderToStaticMarkup(<BackupOperations profile={profile} operation={{
      ...complete,
      semantic_verification: "unavailable",
      semantic_verification_reason: "full_semantic_restore_verification_not_run",
    }} />);
    expect(nativeOnly).toContain("Backup artifact prepared");
    expect(nativeOnly).toContain("Full semantic restore verification was not run");
    expect(nativeOnly).not.toContain("Backup complete and verified");
  });

  it("retains an unavailable active identity without showing stale verification or a new operation", () => {
    const unavailable = renderToStaticMarkup(<BackupOperations
      profile={profile}
      operation={null}
      operationId="backup-stable"
      operationStatusAvailable={false}
    />);
    expect(unavailable).toContain("Backup operation status unavailable");
    expect(unavailable).toContain("Retry original backup");
    expect(unavailable).toContain("will not create a replacement");
    expect(unavailable).not.toContain("Backup complete and verified");
    expect(unavailable).not.toContain(">Prepare backup<");
  });
});
