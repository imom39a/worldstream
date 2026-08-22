# IMO-52 backup verifier foundation

## Scope

`worldstream-backup` is a bounded verifier plus a native bundled-SQLite
backup/restore bridge. It models the versioned backup identity,
storage epoch, SQLite online-backup or PostgreSQL-native restore point,
migration contract, exact retained pack identities, content-addressed resource
identities, canonical Room records, Complete Head/materialization hashes, and
operational ledgers for receipts, timers, Frames, Activations, and integrity.

The verifier accepts an immutable `BackupImageV1` and returns a bounded
`VerificationReportV1`. The `native_sqlite` module additionally opens the
bundled SQLite engine in query-only mode, performs a real online backup and
restore to an atomically published sibling, and runs post-copy verification.
The crate still does not call PostgreSQL/provider APIs, repair a Room, fire a
timer, deliver a Frame, claim an Activation, write a receipt, or change
readiness outside returned reports.

## Fail-closed rules

- The manifest schema, backend-native point, storage epoch, migration prefix,
  pack/resource identities, and global lineage digest must agree exactly.
- Resource sizes and BLAKE3 digests are checked against the manifest; missing,
  unexpected, duplicate, or changed resources block readiness.
- Healthy Rooms require a Genesis-to-Head sequence, predecessor digest chain,
  Complete Head, and paired Core/Activity/Authoritative materialization hashes.
- Receipt, timer, Frame, and Activation rows verify exact byte hashes,
  identities, generations, causal sequences, cursors, lease parts, and context
  retention relations.
- Diagnostics contain stable codes, hashed subjects, and actionable next steps;
  raw payload bytes, credentials, DSNs, and provider details are not exposed.
- Readiness is `Ready` only when no blocking diagnostic exists. The only Room
  exception is a byte-preserved Room whose source and restored statuses are the
  same explicitly pre-existing isolated `Faulted` or `Quarantined` status with
  the same integrity generation.

## Deterministic fixtures

The unit fixtures in `crates/worldstream-backup/src/lib.rs` cover:

| Fixture | Expected result |
| --- | --- |
| clean | Ready; healthy Room verified |
| missing resource | NotReady; resource diagnostic |
| new mismatch | NotReady even when the source Room was unhealthy |
| global corruption | NotReady; deployment lineage diagnostic |
| invalid operational relation | NotReady; Frame relation/hash diagnostic |
| permitted pre-existing isolation | Ready; unhealthy Room remains isolated |
| side-effect/redaction check | Input bytes unchanged; subjects do not expose Room IDs |

## Evidence and gaps

The focused crate suite has 15 passing tests and clippy passes. Local tests
prove bundled SQLite online-backup/restore publication, query-only integrity
verification, WAL/active-writer locking behavior, interrupted-transfer cleanup,
and deterministic metadata fixtures. They do not claim disk-full,
crash/power-loss, PostgreSQL snapshot/PITR/dump, provider durability, exact
Activity Pack replay across every operational row, or full storage-adapter
translation. Those remain adapter and integration evidence.

The repository did not expose a GitHub issue or project item resolvable as
`IMO-52` through the available CLI; this document therefore traces the seam to
the normative backup/restore requirements in `docs/architecture.md`, ADR 0004,
ADR 0011, and the decision index entry for IMO-52.

## Scoped lane continuation (2026-08-21)

Native restore now performs an exact source/isolated-target comparison for
operational rows, canonical records, materializations, valid paired-snapshot
evidence, integrity membership, and the typed restore projection. Healthy
Rooms are replayed through the retained Counter executor in the bounded
conformance driver and their replayed Core/Activity bytes must match the
restored materializations. Pre-existing faulted/quarantined Rooms are counted
and preserved as isolated rather than replayed or made serving-eligible.

The native verifier also requires the migration-6
`canonical_export_metadata` table to contain exactly one non-empty deployment
lineage row with a nonzero storage epoch. The typed projection independently
rechecks each healthy Room's record owner, terminal Complete Head,
materialization hashes, and selected paired snapshot before preserving it.
The smoke lane accepts an explicit target metadata sidecar only as exact
adapter evidence: it no longer converts present fields into synthetic empty
row vectors, and it checks the sidecar's source/restored status, generation,
and isolation flags against the extracted integrity ledger. Missing or
conflicting isolated membership fails closed.

The runner remains fail-closed and emits `release_evidence: false`. The
versioned native envelope now supplies the bounded companion facts the SQLite
schema cannot authoritatively store, then constructs and verifies a complete
manifest-backed `BackupImageV1`. Pack executor/schema/codec/resource facts
must match the checked-in compatibility authority, and the JSON envelope must
be accompanied by the non-serializable witness minted by the actual bundled
online-backup/restore operation; resealing JSON cannot mint native-point or
companion authority. The lane does not claim crash/power-loss, disk-full,
provider durability, or provider-native PostgreSQL snapshot/PITR/dump
coverage.
