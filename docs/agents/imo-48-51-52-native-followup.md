# IMO-48/51/52 native follow-up evidence

## Disposition

This is a local evidence update for the dirty checkout, not release or
provider acceptance. Writes in this follow-up are limited to
`crates/worldstream-backup/**`, `crates/worldstream-transfer/**`,
`crates/worldstream-postgres/**`, and this document. No core, SQLite adapter,
UI, packaging, compatibility manifest, or unrelated documentation was
changed.

The acceptance criteria require backend-native backup/restore, full semantic
verification, and a two-phase SQLite-to-empty-PostgreSQL transfer. The
repository already had useful provider-neutral foundations. This follow-up
adds a real bundled SQLite native backup/restore path, native
locking/interruption evidence, and a durable-destination transfer contract
while keeping provider-backed evidence fail-closed.

## Orchestrator addendum (2026-08-20)

The parent reviewed the Luna extensions to this scope. The native backup suite
now has 21 passing tests, including a deterministic destination-appears-before-
publish race fault through the real no-replace publication path. The transfer
suite now has 17 passing tests, and `worldstream-postgres` contains 16 passing
tests, a transactional `PostgresTransferDestination`, structural Room
verification, and forward transfer migrations through the target-fence
revision. The transfer lane now includes an explicit
`NativeSqliteTransferAdapterV1` seam consuming bounded
`NativeSqliteOperationalRowsV1` evidence. These additions preserve the
documented boundary: no disk-full, process-kill, power-loss, full
`BackupImageV1` translation, or live adapter-to-adapter PostgreSQL transfer is
claimed.

## Implemented local evidence

### IMO-48 — integrity and recovery

The existing acceptance contract requires immutable Genesis plus ordered
Transitions, all three state hashes, disposable paired snapshots, and
reconstruction after deleting snapshots. The new
`worldstream_backup::native_sqlite::verify_file` path:

- rejects non-regular and symlink inputs;
- opens the bundled SQLite engine through `rusqlite` and enforces connection-
  level query-only mode;
- checks `PRAGMA integrity_check`, required WorldStream
  tables, and the five-row migration prefix;
- reads bounded hex projections of `rooms`, `room_genesis`, `transitions`,
  `room_materializations`, and `room_snapshots`;
- recomputes Genesis and Transition BLAKE3 hashes, checks predecessor links,
  sequence continuity, Complete Head identity, and Core/Activity
  materialization hashes;
- treats absent or corrupt paired snapshots as nonblocking disposable-cache
  findings after canonical lineage verification; and
- returns redacted stable diagnostics without changing the input file.

The native suite also exercises an active writer/WAL read-only boundary,
exclusive lock rejection, existing/unusable destination rejection, and
interrupted-copy cleanup without publishing a partial target.

The native tests use temporary SQLite files and prove bundled-engine
read-only verification, real online-backup/restore publication, atomic sibling
output, byte-preserving verification, and canonical readiness after deleting
all snapshots or corrupting a snapshot's Core bytes.

### IMO-51 — SQLite-to-PostgreSQL transfer

The existing contract already provides deterministic `WSTRANS1` bytes,
canonical/derived record separation, byte-preserving record payloads,
checkpoint chaining, and the pending/verified/finalized/authoritative epoch
state machine. The follow-up adds:

- bounded `TransferBundleV1::export_to_path` using a flushed temporary sibling
  and rename;
- bounded `TransferBundleV1::import_from_path` with regular-file, symlink,
  size, and complete-wire validation;
- deterministic temporary-file export/import evidence;
- a bounded resumable import session with serialized checkpoint state;
- idempotent chunk replay handling and destination-side verification; and
- canonical source/target backend fingerprints, deployment/Room byte parity,
  canonical record ordering, and repair/resume after a missing destination
  chunk; and
- verification-gated finalization and target-authority commit, with abort
  cleanup before source rollback.

The new `NativeSqliteTransferAdapterV1` consumes the existing bounded native
row evidence through an explicit seam. It requires all nine modeled
operational relations, fixed column shapes, Room-integrity ownership, member /
Activation cross-relations, duplicate-versus-conflicting identity detection,
and exact storage-class-preserving row bytes. Its manifest binds table counts,
healthy-versus-isolated Room policy, and an exact row digest. The PostgreSQL
destination revalidates that native manifest, contiguous chunk coverage,
exact chunk bytes/digests, and the target-wide bundle/fingerprint fence during
both complete-import verification and finalization.

The local tests exercise the adapter and the PostgreSQL SQL destination seam,
but no live PostgreSQL connection, provider API, target semantic verification
against all native rows, provider-backed finalization record, or live
cross-backend byte-parity run is claimed.

### IMO-52 — restore and verifier

The existing immutable `BackupImageV1` verifier remains the full metadata seam:
it checks manifest/native-point identity, resources, Rooms, receipts, timers,
Frames, Activations, context retention, redaction, and permitted pre-existing
isolation. The new native path supplies a local file-to-evidence bridge for
the SQLite canonical subset and deliberately leaves the returned report
read-only.

## Exact scoped commands and results

Run from `/Users/vinothshanmugam/code/agent-streamer`:

```text
cargo fmt --manifest-path crates/worldstream-backup/Cargo.toml -- --check   # pass
cargo fmt --manifest-path crates/worldstream-transfer/Cargo.toml -- --check # pass
cargo test --manifest-path crates/worldstream-backup/Cargo.toml --lib       # 21 passed
cargo test --manifest-path crates/worldstream-transfer/Cargo.toml --lib     # 17 passed
cargo check --workspace --locked                                             # pass
cargo test --locked -p worldstream-postgres                                # 16 passed
cargo clippy --manifest-path crates/worldstream-backup/Cargo.toml --all-targets -- -D warnings   # pass
cargo clippy --manifest-path crates/worldstream-transfer/Cargo.toml --all-targets -- -D warnings # pass
cargo clippy --locked -p worldstream-postgres --all-targets -- -D warnings                         # pass
```

The native SQLite tests are part of the backup crate's twenty-one passing tests.
They use temporary files and the bundled SQLite engine; no external service,
PostgreSQL process, or destructive power-loss harness was used.

## Unresolved acceptance gaps

- No disk-full exercise, process kill, filesystem fault, or power-loss run
  exists here. WAL/active-writer/read-only and interrupted native-copy cases
  are now covered locally.
- The native bridge does not extract Storage Epoch or deployment lineage from a
  durable adapter-owned deployment record; epoch/lineage authority is checked
  by the transfer contract's target fingerprint and lifecycle fence only.
- No PostgreSQL 17 direct-admin import, native snapshot/PITR/dump restore,
  provider durability, transaction-pooler run, or PostgreSQL semantic verifier
  run exists.
- No full native-file translation into `BackupImageV1` is claimed. The local
  SQLite bridge checks canonical lineage/materialization evidence and the
  existing immutable verifier checks its typed metadata image; joining those
  surfaces to canonical Room records, pack executor state, and a complete
  `BackupImageV1` remains outside this operational-row bridge.
- The native transfer bridge preserves and validates receipts, timers, Frames,
  Membership-addressed cursors, Activation intents/lease fields/operation
  receipts/context tombstones, consequences, decisions, and integrity rows as
  bounded operational evidence; it does not claim to hydrate those rows into
  provider-native tables.
- No provider-backed durable source/target finalization record, nonterminal
  Activation lease fencing run against the storage adapters, live PostgreSQL
  target import checkpoint persistence, or full cross-backend byte parity run
  exists.

These gaps remain intentionally visible. They require changes or harnesses in
the storage adapters, provider environments, or native platform test matrix
and are outside this scoped follow-up.

## Parent live-provider follow-up (2026-08-20)

The parent orchestrator separately ran a fresh PostgreSQL 17.11 container and
transaction-mode PgBouncer. The live conformance test passed direct-admin
migration/schema verification, least-privileged runtime DDL denial, direct
create/duplicate/conflict/resolve, direct advance/frame/semantic-receipt
persistence, and pooled duplicate/resolve. This closes the PostgreSQL smoke
cell only; it does not upgrade the native SQLite extraction bridge into a live
SQLite-to-PostgreSQL import, full `BackupImageV1` restore, or cross-backend
canonical parity claim.
