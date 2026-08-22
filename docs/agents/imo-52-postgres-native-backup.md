# IMO-52 PostgreSQL-native backup/restore evidence

Status: locally verified `ready`; `release_evidence` remains `false`.

## Boundary and architecture

The provider-neutral `worldstream-backup` crate owns `BackupImageV1`,
`NativeRestoreEvidenceV1`, and `verify_native_restore`. It has no dependency on
`worldstream-postgres`. PostgreSQL owns the provider adapter in
`crates/worldstream-postgres/src/native_restore.rs`, which depends on the
provider-neutral verifier. This keeps the graph acyclic (`postgres -> transfer
-> backup`, with no reverse edge).

The PostgreSQL module accepts only non-secret endpoint coordinates and an
owner-only `PGPASSFILE`. `pg_dump`, `pg_restore`, and `psql` receive host, port,
database, and user arguments; passwords and URI-shaped credentials never enter
argv, report JSON, stdout, or stderr. The trusted witness is a non-serializable
Rust value and is minted only after the native restore and unified verification
complete.

## Evidence path

1. The Docker smoke uses the digest-pinned PostgreSQL 17.11 Alpine image
   `postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73`.
2. The source is seeded by the existing PostgreSQL transfer smoke and reviewed
   SQLite/Core fixture, including migrations, deployment identity, pack
   identity, canonical Room history, materializations, semantic receipts, and
   operational rows. It is not a blank database or hand-written substitute.
3. `postgres-native-prepare` calls the existing PostgreSQL admin snapshot
   rebuild path to create the disposable paired snapshot cache.
4. The lane performs a provider-native PostgreSQL custom-format dump and
   `pg_restore` into an isolated target.
5. Restored rows are strictly adapted into `BackupImageV1`. Canonical Room
   bytes, exact heads and hashes, migration checksums, deployment identity
   canonical bytes, pack/resource set digests, semantic receipt envelopes,
   timer/frame relations, and provider version are checked. The existing
   `verify_native_restore` function is then called directly.
6. Snapshot cache rows are deleted only after the verifier is ready and source
   and target durable digests match. Durable state is recaptured afterward;
   only then is the result `ready` and the trusted witness minted.

## Docker result

Command:

```text
WORLDSTREAM_NATIVE_PG_DEBUG=1 scripts/postgres-native-restore-smoke.sh
```

Exit status: `0`.

The emitted redacted evidence reported:

```json
{
  "status": "ready",
  "reason": "postgres_native_restore_verified_by_unified_verifier",
  "release_evidence": false,
  "native_dump_restore": "pass",
  "verifier": {
    "readiness": "Ready",
    "rooms": {"01ARZ3NDEKTSV4RRFFQ69G5FAV": "Verified"},
    "diagnostics": []
  },
  "source_unchanged": true,
  "exact_restored_row_set": true,
  "snapshots_disposable": true,
  "target_isolated": true,
  "target_published": false,
  "native_witness_minted": true,
  "secrets_emitted": false,
  "source_version_num": 170011,
  "restored_version_num": 170011,
  "semantic_receipts_verified": true,
  "restored_snapshot_count_before": 2,
  "restored_snapshot_count_after": 0
}
```

The script removes the temporary dump, owner-only Docker credential files,
wrappers, and both disposable containers on success or failure. It performs no
target authority/publication operation.

## Negative/security boundaries

- Missing Docker/tools, malformed endpoints, unsafe passfile permissions, wrong
  PostgreSQL version, incomplete migration history, missing identity metadata,
  pack/resource identity drift, malformed canonical rows, stale/tampered
  receipts, and invalid timer/frame relations fail closed without a witness.
- A fake-tool Python test captures argv/stdout/stderr and proves that the
  password and URI forms are absent.
- The fixture has an explicit empty external-resource set. That set is
  authenticated as present in the canonical deployment identity; nonempty
  resource rows currently fail closed because this PostgreSQL schema does not
  provide a reviewed resource-byte extraction surface for this lane.
- Telemetry-lane overlap was observed. This work leaves PostgreSQL
  `transfer.rs`, migrations, SQLite, UI, workflows, and packaging untouched.

## Gates run

- `cargo metadata --locked --format-version 1 --no-deps`: pass
- `cargo clippy --locked -p worldstream-backup --all-targets -- -D warnings`: pass
- `cargo clippy --locked -p worldstream-postgres --all-targets -- -D warnings`: pass
- `cargo test --locked -p worldstream-backup --all-targets`: 55 passed
- `cargo test --locked -p worldstream-transfer`: 27 passed
- `cargo test --locked -p worldstream-postgres --all-targets`: 27 unit, 13 commit, and 4 telemetry tests passed; 1 provider-dependent telemetry test ignored
- `python3 -m unittest discover -s tests -p '*native*smoke.py' -v`: 9 passed
- `bash -n scripts/postgres-native-restore-smoke.sh`: pass
- Digest-pinned Docker smoke: exit 0, ready evidence above

## Remaining evidence gaps

This lane proves PostgreSQL 17.11 custom logical dump/restore. It does not yet
prove physical base-backup/WAL replay, PITR target selection, crash or power
loss at arbitrary dump/restore phases, disk-full recovery, replication or
pooler failover, filesystem durability across platforms, or long-running
backup/restore soak behavior. Those require separate provider-operated test
surfaces and remain outside this local Docker acceptance result.
