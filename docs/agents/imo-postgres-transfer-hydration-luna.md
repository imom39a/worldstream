# PostgreSQL transfer hydration — parent verification

Date: 2026-08-21

## Scope

The PostgreSQL destination now publishes the exact canonical and native
operational records carried by a verified transfer bundle. The change is
limited to `crates/worldstream-postgres/src/transfer.rs`.

It persists pack-revision-lock bytes, Room Transitions, and the native
operational ledgers for integrity, Membership, timers, observations,
Activations, and semantic receipts. Existing rows are compared byte-for-byte;
conflicts fail closed in the same transaction. A missing pack-revision lock is
rejected rather than derived from the pack identity.

## Parent verification

```text
cargo test --locked -p worldstream-postgres --all-targets
20 unit tests + 13 integration tests passed

cargo clippy --locked -p worldstream-postgres --all-targets -- -D warnings
passed

cargo fmt --all -- --check
passed

git diff --check
passed
```

The parent independently reran the disposable PostgreSQL 17.11 transfer
against a temporary SQLite source containing one healthy Room, one member,
one semantic receipt, exact canonical records including `ArtifactMetadata`,
and no transitions:

```text
WORLDSTREAM_PG_TRANSFER_TIMEOUT_SECONDS=300 \
  scripts/postgres-transfer-smoke.sh --sqlite <ephemeral-source>
```

Result: exit `0`; PostgreSQL admin/runtime verification, migration and
read-only checks, checkpoint replay, finalization, target authority, and exact
canonical Room readback all passed. The report remained
`release_evidence=false` and emitted no credentials.

## Boundaries

This proves the reviewed healthy-source PostgreSQL transfer seam only. It does
not promote the compatibility manifest, prove native Linux/Windows/OCI
release artifacts, signatures/SBOM/provenance, provider-native snapshot/PITR,
or long-duration resilience.
