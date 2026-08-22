# IMO-49 SQLite migration follow-up

This follow-up covers the SQLite migration, ownership, and native-backup
publication evidence surface.

## Implemented evidence

- `opens_issued_v1_database_through_forward_authority_migration` exercises the
  frozen v0.1-style schema prefix and verifies all five forward migration
  identities, retired authority data, and the v2 membership-generation
  backfill.
- `migration_failure_is_atomic_and_restart_retries_the_same_forward_prefix`
  injects invalid legacy Room history, proves the immediate migration
  transaction leaves no partial v2-v5 schema behind, repairs the fixture, and
  retries the identical forward prefix successfully.
- `writer_lock_rejects_a_second_owner_and_releases_for_restart` proves the
  sidecar writer lease is exclusive and that a clean owner release permits a
  subsequent startup.
- `migration_ledger_rejects_mixed_gapped_downgraded_and_future_prefixes`
  covers a gapped prefix, a downgraded prefix, a future migration, and a
  mismatched v1 identity. Each fails closed before Room serving starts.
- Native bundled SQLite online backup now runs before pending migration
  publication. The backup is verified, published through an atomic sibling
  rename, and removed on failed migration/restart paths; the resulting image
  can be restored and verified through the shared native verifier.

The migration implementation remains forward-only: migrations run inside one
`BEGIN IMMEDIATE` transaction, each identity is checked before application,
and the complete schema corpus is compared against the frozen DDL before
commit. No compatibility manifest, protocol, server, SDK, UI, transfer, or
release surface was changed.

## Verification

Run from the repository root:

```text
cargo fmt --manifest-path crates/worldstream-sqlite/Cargo.toml -- --check  # pass
cargo test --locked -p worldstream-sqlite                              # 75 passed
cargo clippy --locked -p worldstream-sqlite --all-targets -- -D warnings # pass
git diff --check                                                        # pass
```

The test command also completed SQLite doctests (`0 passed, 0 failed`).

## Remaining gaps

The 75 tests are deterministic in-process/native-API evidence, not a
substitute for process-level failure testing. A production-complete IMO-49
acceptance run still needs WAL/locking and disk-full cases, process-kill or
power-loss interruption, and full operational-row semantic verification after
restore. The separate PostgreSQL 17.11 direct-admin/runtime/transaction-pool
evidence is recorded in `docs/agents/imo-44-50-live-postgres-followup.md`; it is
not repeated here.
