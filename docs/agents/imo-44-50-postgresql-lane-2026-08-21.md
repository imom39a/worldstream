# PostgreSQL/IMO-44–50 lane continuation

Date: 2026-08-21

## Implemented

- PostgreSQL administration now reads the full migration ledger after
  migration 0002 adds metadata columns. Forward-prefix verification therefore
  checks migration identity, checksum, logical history, and schema-fingerprint
  metadata during an upgrade instead of reading only the legacy three-column
  shape.
- Create commits claim the missing `worldstream_room_roots` row with
  `INSERT ... ON CONFLICT DO NOTHING RETURNING` inside the operation
  transaction. Two different operation identities racing for one Room now
  produce one durable Create and one deterministic `Reprepare`.
- Provider failure classification is exposed as a stable SQLSTATE mapping for
  connection loss, deadlock, lock timeout, serialization, constraint, and
  schema failures. The mapping feeds the existing retry/indeterminate
  decision without exposing provider error text.
- The normalized provider-neutral transcript now covers Create, an existing
  Transition commit, durable receipt resolution, and a stale-head `Reprepare`,
  in addition to the existing Create duplicate/conflict vector.
- The live harness requires direct-admin migration restart evidence and the
  direct same-room Create root-guard evidence before reporting adapter pass.

## Verification

Focused local evidence:

```text
python3 -m unittest -q tests/postgres_harness.py       # 6 passed
bash -n scripts/postgres-harness.sh                    # pass
cargo test --locked -p worldstream-postgres --features conformance-tracer --test postgres_commit  # 17 passed
cargo test --locked -p worldstream-postgres            # 19 unit + 13 integration passed
```

A fresh disposable `postgres:17.11-alpine` direct run with separate admin and
runtime roles produced:

```text
LIVE_POSTGRES=PASS direct_admin=migrate+verify major=17
LIVE_POSTGRES=PASS direct_admin=restart-idempotent
LIVE_POSTGRES=PASS runtime_ddl=create_denied
LIVE_POSTGRES=PASS runtime=direct create+duplicate+conflict+resolve
LIVE_POSTGRES=PASS runtime=direct same-room-create=reprepare
LIVE_POSTGRES=PASS runtime=direct guarded-receipt-read
LIVE_POSTGRES=PASS runtime=direct advance+frame+semantic-receipt+same-identity-concurrency
LIVE_POSTGRES=PASS runtime=direct stale-head=reprepare+known-absent
LIVE_POSTGRES=PASS runtime=direct rollback+unknown-commit=guarded-resolution
LIVE_POSTGRES=PASS runtime=direct authority-fence=known-absent
LIVE_POSTGRES=PASS normalized=create+commit+duplicate+conflict+stale+fence+rollback+unknown-resolution
LIVE_POSTGRES_POOLER=SKIP reason=pooler_dsn_unset
```

The run was cleaned up after completion. The pooler image attempts in this
continuation were not accepted as evidence: the stock image generated an
`auth_user` catalog lookup that the least-privileged runtime role correctly
cannot perform. Existing separately recorded PgBouncer evidence remains
subject to its own role/configuration provenance; no new pooler pass is
claimed here.

Workspace clippy remains blocked by two unrelated pre-existing
`worldstream-backup/src/native_sqlite.rs` unused-variable diagnostics. No
files outside the PostgreSQL, harness, and PostgreSQL-doc write scope were
changed by this lane.
