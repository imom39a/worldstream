# IMO-49/52/60/61 durable SQLite migration-checksum witness

Date: 2026-08-21
Lane: parent-orchestrated SQLite and native-backup implementation

## Outcome

SQLite now has a forward-only version-7 ledger migration with the distinct
backend-specific identity `0008-sqlite-migration-checksums-v1`. The migration
adds `schema_migrations.source_checksum`, backfills the exact reviewed checksum
for every legacy applied row in the same transaction, and persists the new
migration checksum. Empty databases and the supported legacy prefixes migrate;
future-shaped checksum ledgers, mixed/gapped/unsupported prefixes, identity
drift, and checksum drift fail before migration bodies or backfill writes.

Startup verification reads the stored checksum values and compares them to the
reviewed inventory. It does not treat the current binary's synthesized digest
as durable evidence. The migration failpoint proves the ledger extension and
backfill roll back atomically and retry successfully.

## New migration identity and checksum

The exact raw SQL body is:

```sql
ALTER TABLE schema_migrations ADD COLUMN source_checksum TEXT NOT NULL DEFAULT '';
```

| Version | Identity | BLAKE3 checksum |
|---:|---|---|
| 7 | `0008-sqlite-migration-checksums-v1` | `blake3:ed00960ddbbfbb6a6cb8fde52ce44631ce41c3e0b7dd2e46968552f7538eb33a` |

The compatibility manifest preserves the existing PostgreSQL-only
`0007-deployment-metadata-v1` row with an empty SQLite checksum. The new
SQLite edge is a separate `0008` row with an empty PostgreSQL checksum. The
existing logical schema fingerprint is unchanged because this is a
backend-specific migration-ledger durability extension, not a relabeling or
rewrite of the shared PostgreSQL schema contract.

## Native backup and restore witness

Native SQLite verification now requires all seven ordered rows, exact IDs, and
exact persisted checksums. Native restore extraction emits
`migration_metadata` only for that complete exact contract; absent columns,
missing rows, duplicates, out-of-order rows, and tampered checksums leave the
field absent and therefore keep readiness incomplete. Existing exact-byte,
read-only, online-backup, restore, and isolation behavior is retained.

## Changed files owned by this lane

- `crates/worldstream-sqlite/src/migration_contract.rs`
- `crates/worldstream-sqlite/src/lib.rs`
- `crates/worldstream-backup/src/native_sqlite.rs`
- `compatibility.toml`
- `compatibility.json`
- `docs/agents/imo-49-52-60-61-sqlite-migration-checksums-luna.md`

## Verification

- SQLite library tests: **92 passed**.
- SQLite migration-focused tests: **7 passed**.
- Backup library tests: **53 passed**.
- Backup migration-witness focused test: **1 passed**.
- Scoped all-target Clippy with `-D warnings`: **pass**.
- Scoped rustfmt check: **pass**.
- Scoped `git diff --check`: **pass**.
- `scripts/verify-manifest.py`: **pass**.
- Authored TOML/canonical JSON semantic parity: **pass**.

Parent integration updated the Wave 6 manifest audit and its unit test from six
to seven SQLite migration bodies. The four audit tests and the default
validation command pass. Release readiness remains false, and unresolved
artifact, signature/SBOM/provenance, provider, and full transfer/restore
evidence remain outside this witness.
