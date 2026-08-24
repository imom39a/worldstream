import { describe, expect, it, vi } from "vitest";
import {
  loadBackupOperation,
  loadBackupOperationId,
  loadBackupOperationState,
  loadBackupProfile,
  newBackupOperationId,
  runBackupOperation,
  saveBackupOperationId,
} from "./backups";

describe("backup client", () => {
  it("loads independent profile health and a verified pathless result", async () => {
    const fetcher = vi.fn(async (input: string | URL | Request, _init?: RequestInit) => new Response(JSON.stringify(
      String(input).endsWith("health") ? {
        schema: "worldstream/studio-backup-profile-status/v1",
        storage_profile: "sqlite_bundled", storage_health: "healthy",
        live_backup_supported: true, verification: "unavailable",
        freshness: { status: "unavailable", reason: "no_operation_selected" },
      } : {
        schema: "worldstream/studio-backup-operation/v1", operation_id: "backup-stable",
        storage_profile: "sqlite_bundled", storage_health: "healthy", phase: "complete",
        native_verification: "pass", semantic_verification: "pass",
        semantic_verification_reason: null,
        freshness: { status: "fresh", observed_at: "2026-08-23T12:00:00Z" },
        destination: { kind: "studio_managed_local", artifact_name: "backup.sqlite3", byte_length: 4096, blake3_digest: "a".repeat(64) },
        failure_reason: null,
      }), { status: 200 }));
    expect((await loadBackupProfile(fetcher as typeof fetch))?.storage_health).toBe("healthy");
    const result = await runBackupOperation("backup-stable", fetcher as typeof fetch);
    expect(result?.native_verification).toBe("pass");
    expect(JSON.stringify(result)).not.toContain("path");
    expect(fetcher.mock.calls[1]?.[1]).toMatchObject({ method: "POST", cache: "no-store" });
  });

  it("rejects path-like operation identities before making a request", async () => {
    const fetcher = vi.fn();
    expect(await runBackupOperation("../../backup", fetcher as typeof fetch)).toBeNull();
    expect(fetcher).not.toHaveBeenCalled();
    expect(newBackupOperationId(1234)).toBe("backup-ya");
  });

  it("rejects incomplete operation health, failure, and destination shapes", async () => {
    const invalid = [{ storage_health: "unsupported" }, { failure_reason: "/private/path" }, {
      destination: { kind: "browser_path", artifact_name: "backup.sqlite3", byte_length: 1, blake3_digest: "a".repeat(64) },
    }];
    for (const override of invalid) {
      const fetcher = vi.fn(async (_input: string | URL | Request, _init?: RequestInit) => new Response(JSON.stringify({
        schema: "worldstream/studio-backup-operation/v1", operation_id: "backup-stable",
        storage_profile: "sqlite_bundled", storage_health: "healthy", phase: "failed",
        native_verification: "failed", semantic_verification: "unavailable",
        semantic_verification_reason: "verification_failed",
        freshness: { status: "unavailable", reason: "verification_failed" },
        destination: null, failure_reason: "verification_failed", ...override,
      }), { status: 200 }));
      expect(await runBackupOperation("backup-stable", fetcher as typeof fetch)).toBeNull();
    }
  });

  it("rejects incoherent terminal, destination, and semantic verification claims", async () => {
    const base = {
      schema: "worldstream/studio-backup-operation/v1", operation_id: "backup-stable",
      storage_profile: "sqlite_bundled", storage_health: "healthy", phase: "complete",
      native_verification: "pass", semantic_verification: "pass",
      semantic_verification_reason: null,
      freshness: { status: "fresh", observed_at: "2026-08-23T12:00:00Z" },
      destination: { kind: "studio_managed_local", artifact_name: "backup.sqlite3", byte_length: 1, blake3_digest: "a".repeat(64) },
      failure_reason: null,
    };
    const invalid = [
      { destination: null },
      { semantic_verification: "unavailable", semantic_verification_reason: null },
      { semantic_verification: "pass", semantic_verification_reason: "not_run" },
      { phase: "retrying", failure_reason: "storage_unavailable" },
      { phase: "complete", native_verification: "failed" },
    ];
    for (const override of invalid) {
      const fetcher = vi.fn(async () => new Response(JSON.stringify({ ...base, ...override }), { status: 200 }));
      expect(await runBackupOperation("backup-stable", fetcher as typeof fetch)).toBeNull();
    }
  });

  it("accepts a restart-reconciled operation as retrying under the original identity", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify({
      schema: "worldstream/studio-backup-operation/v1", operation_id: "retry-original",
      storage_profile: "sqlite_bundled", storage_health: "unavailable", phase: "retrying",
      native_verification: "unavailable", semantic_verification: "unavailable",
      semantic_verification_reason: "operation_reconciliation_required",
      freshness: { status: "unavailable", reason: "operation_reconciliation_required" },
      destination: null, failure_reason: "operation_reconciliation_required",
    }), { status: 200 }));
    const status = await loadBackupOperation("retry-original", fetcher as typeof fetch);
    expect(status?.phase).toBe("retrying");
    expect(status?.operation_id).toBe("retry-original");
  });

  it("marks an active operation unavailable when refresh cannot verify it", async () => {
    const unavailable = vi.fn(async () => new Response("unavailable", { status: 503 }));
    await expect(loadBackupOperationState("retry-original", unavailable as typeof fetch))
      .resolves.toEqual({ availability: "unavailable", operation: null });
    const malformed = vi.fn(async () => new Response("not-json", { status: 200 }));
    await expect(loadBackupOperationState("retry-original", malformed as typeof fetch))
      .resolves.toEqual({ availability: "unavailable", operation: null });
  });

  it("retains the stable active identity when a POST result is unavailable", async () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value); },
      removeItem: (key: string) => { values.delete(key); },
      clear: () => { values.clear(); },
      key: (index: number) => [...values.keys()][index] ?? null,
      get length() { return values.size; },
    } satisfies Storage;
    saveBackupOperationId("retry-original", storage);
    const unavailable = vi.fn(async () => new Response("unavailable", { status: 503 }));
    expect(await runBackupOperation("retry-original", unavailable as typeof fetch)).toBeNull();
    expect(loadBackupOperationId(storage)).toBe("retry-original");
  });
});
