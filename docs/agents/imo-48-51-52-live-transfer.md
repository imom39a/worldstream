# IMO-48/51/52 live SQLite-to-PostgreSQL transfer smoke

## Scope and disposition

This document describes the bounded evidence runner in
`scripts/postgres-transfer-smoke.sh`. It is an operational smoke harness, not
a release gate and not a manifest update. The runner writes only its optional
JSON evidence file; it never changes `release_ready`, compatibility manifests,
or release artifacts.

The runner requires an explicit native SQLite source file:

```text
WORLDSTREAM_PG_TRANSFER_SQLITE=/path/to/source.sqlite \
  scripts/postgres-transfer-smoke.sh --evidence /tmp/worldstream-transfer.json
```

The ephemeral Rust driver is bounded by
`WORLDSTREAM_PG_TRANSFER_TIMEOUT_SECONDS` (180 seconds by default); timeout is
an incomplete result.

By default it starts a disposable `postgres:17.11-alpine` container, creates
separate generated admin and least-privileged runtime credentials, and removes
the owned container on exit. The generated credentials, DSNs, container name,
source path, and raw Room identifiers never enter stdout, stderr, or the JSON
evidence. The target is published on an ephemeral loopback port and is isolated
to that invocation.

An explicitly supplied target is opt-in and must already be isolated by its
caller:

```text
WORLDSTREAM_PG_TRANSFER_MODE=external \
WORLDSTREAM_PG_TRANSFER_ADMIN_DSN='...' \
WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN='...' \
WORLDSTREAM_PG_TRANSFER_SQLITE=/path/to/source.sqlite \
  scripts/postgres-transfer-smoke.sh
```

The external mode does not claim to create or own the target. It fails closed
when either credential is absent.

## Provider path exercised

The runner generates a short-lived Cargo driver outside the checkout. That
driver uses the existing APIs directly:

1. `worldstream_backup::native_sqlite::verify_file` opens the source through
   the bundled SQLite read-only seam, followed by bounded
   `extract_operational_rows`.
2. `NativeSqliteTransferAdapterV1` builds and validates the exact
   operational-row bundle and its manifest. The bundle retains exact record
   bytes and deterministic per-chunk digests.
3. `PostgresAdmin` performs forward migrations and read-only schema
   verification through the admin DSN.
4. `PostgresRoomStore` performs runtime read-only schema verification, and
   `PostgresTransferDestination` stages chunks through its transaction-safe
   target-fence/import/chunk tables.
5. `TransferImportSessionV1` verifies the target epoch/fingerprint, applies
   bounded chunks, serializes a checkpoint, resumes from that checkpoint, and
   confirms an exact replay as `AlreadyApplied`.
6. The destination verifies complete exact staged-byte coverage. A separate
   runtime connection performs read-only state/chunk-count queries and a final
   read-only schema check.

The current adapter intentionally stops at operational rows. It cannot prove
that those rows have hydrated the full Core-owned Room Genesis, Transitions,
materializations, Memberships, Activity Pack state, resources, and
BackupImage semantics. Therefore the runner refuses `finalize` and target
authority, aborts the pending/verified staged import, and checks that the
target rows are gone. A successful provider run is consequently reported as
`status: incomplete`, with `release_evidence: false`.

## Deterministic redacted evidence

The final stdout line and optional evidence file are one canonical JSON object
with sorted keys and compact separators:

- `schema` is `worldstream/sqlite-postgresql-transfer-evidence/v1`;
- `postgres` records only the required/version/schema checks, never DSNs;
- `source` records bounded counts and engine/query-only status, never paths or
  Room IDs;
- `transfer` records the bundle digest, target epoch, record/chunk counts,
  checkpoint replay disposition, target-fence state, read-only witness, and
  abort cleanup; and
- `secrets_emitted` and `release_evidence` are always `false`.

Unavailable Docker, an absent source, a wrong PostgreSQL version, malformed
native SQLite evidence, a target-fence mismatch, a provider error, or a
redaction violation produces a nonzero result and an explicit incomplete or
unavailable reason. Provider error text is not copied into evidence.

## Boundary verification

The scoped checks are:

```text
bash -n scripts/postgres-transfer-smoke.sh
python3 -m py_compile tests/postgres_transfer_smoke.py
python3 -m unittest tests/postgres_transfer_smoke.py -v
```

The Python suite uses fake Cargo/Docker boundaries only to verify fail-closed
classification, timeout handling, canonical JSON, and credential/path
redaction. It does not
claim a live provider result. A live run should be recorded only when Docker
can pull and start PostgreSQL 17.11 and an actual verified native SQLite source
is available. If either is absent, the observed result remains incomplete or
unavailable by design.

Parent verification also created a real bundled SQLite database containing one
Counter-v2 Room through the live daemon route and passed that database to the
runner. The native source inspection succeeded and reported one Room, one
semantic receipt, and one paired snapshot, but the current native semantics
adapter still reported `sqlite_native_semantics_not_ready` before PostgreSQL
transfer could begin. The harness returned `incomplete` with
`release_evidence: false`; this is live fail-closed evidence, not transfer
acceptance.

## Limitations still open

- The source-side exact SQLite backend/migration fingerprint, Activity Pack
  bytes, resource identities and resource blobs, complete `BackupImageV1`
  identity, and native restore-point witness are not persisted by the current
  native bridge. The harness now refuses the run instead of copying the target
  schema or inventing a SQLite build identity, and reports each missing input
  as redacted incomplete evidence.
- The PostgreSQL destination currently persists exact transfer chunks and
  fences but does not hydrate them into all provider-native Core Room tables.
- Because of that adapter boundary, finalization and target-authority promotion
  are deliberately refused even after exact staged-byte verification.
- Explicitly faulted or quarantined Rooms are checked against the source
  integrity/export witness and never enter the healthy canonical record set;
  the current PostgreSQL publication seam cannot yet prove their isolated
  operational preservation, so their presence is an incomplete result.
- No disk-full, process-kill, power-loss, PITR/native PostgreSQL snapshot, or
  one-hour durability claim is made.
- External mode proves only the target actually supplied by the caller and
  cannot claim target isolation or cleanup ownership.

## Scoped lane continuation (2026-08-21)

The transfer/restore lane added the following fail-closed behavior within its
authorized files:

- `worldstream-backup::extract_restore_evidence` now reads an explicitly
  persisted `canonical_export_metadata.storage_epoch`; it never derives an
  epoch from a Room or file timestamp. A missing or multiply populated row is
  still incomplete/invalid evidence.
- `NativeSqliteTransferAdapterV1` carries an explicitly isolated/faulted Room
  through its exact operational and integrity rows without requiring that Room
  to be promoted into healthy canonical serving evidence. A focused test proves
  the healthy/isolated policy separation.
- The transfer runner now includes exact persisted Transition bytes and pack
  revision-lock bytes in the staged canonical bundle, and checks the source
  Transition count against native verification.
- After target authority, the runner uses the PostgreSQL public read-only Room
  verifier to compare exact Genesis, Head, materialization, pack-lock, and
  Transition bytes. A mismatch is an incomplete provider result, never a pass.

The native source/target integration still requires a real verified SQLite
source and isolated PostgreSQL 17.11 target. No provider success or release
evidence is claimed when those inputs are absent. The current PostgreSQL
publication seam must also hydrate every staged operational ledger and exact
canonical Transition/Membership/pack-lock byte before this lane can report a
full round trip; that provider-side work is outside this lane's write scope.
