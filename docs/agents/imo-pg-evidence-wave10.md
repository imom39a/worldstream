# IMO-44/49/50/51/52 PostgreSQL and transfer evidence — Wave 10

Date: 2026-08-21

## Scope and disposition

This is an evidence-only run against the current checkout. It used the
existing PostgreSQL, native-restore, and SQLite-to-PostgreSQL harnesses. No
server, core, protocol, SDK, UI, manifest, or Linear issue was changed. The
five Linear issues remain `In Progress`; none was marked Done.

The disposable PostgreSQL path produced a complete narrow PostgreSQL
provider result. The transfer and restore paths did not produce acceptance
results because the real SQLite source failed the canonical/native evidence
gate. Those failures are recorded below rather than promoted to passes.

## Fresh disposable PostgreSQL run

The run created a new Docker network, a new `postgres:17.11-alpine`
container, and a new `edoburu/pgbouncer:latest` container. Passwords were
generated in process memory only. The PostgreSQL admin and runtime roles were
different; PgBouncer used the same least-privileged runtime role with
`POOL_MODE=transaction`.

Commands:

```text
docker run --rm --detach ... postgres:17.11-alpine
docker run --rm --detach ... --env POOL_MODE=transaction ... edoburu/pgbouncer:latest

WORLDSTREAM_PG_HARNESS_MODE=external \
WORLDSTREAM_PG_HARNESS_ADMIN_DSN=<generated-local-admin-dsn> \
WORLDSTREAM_PG_HARNESS_RUNTIME_DSN=<generated-local-runtime-dsn> \
WORLDSTREAM_PG_HARNESS_POOLER_DSN=<generated-local-pooler-dsn> \
WORLDSTREAM_PG_HARNESS_POOLER_MODE=transaction \
WORLDSTREAM_PG_HARNESS_EVIDENCE_FILE=<temporary-json> \
scripts/postgres-harness.sh
```

The first setup pass correctly failed closed because the external role
precondition still allowed migration-ledger writes. After the disposable
database was reset, the admin explicitly revoked
`INSERT, UPDATE, DELETE, TRUNCATE` on `worldstream_schema_migrations` from
the runtime role, and the existing harness was rerun from a clean data state.

Final harness result:

```json
{
  "status": "pass",
  "exit_code": 0,
  "release_evidence": true,
  "postgres": {"major": 17, "patch": 11, "server_version_num": "170011"},
  "credentials": {"status": "pass", "direct_admin_and_runtime_are_separate": true},
  "migration": {"status": "pass", "forward_only_contract": "pass"},
  "schema_checks": {
    "admin_verification": "pass",
    "runtime_ddl_denial": true,
    "runtime_migration_ledger_write": "pass",
    "runtime_verification_is_read_only": true
  },
  "paths": {
    "direct_runtime": "pass",
    "transaction_pooler": "pass",
    "transaction_pooler_migration_ledger_write": "pass"
  },
  "adapter_conformance": "pass",
  "cleanup": {"status": "pass"},
  "secrets_emitted": false,
  "errors": []
}
```

Image digests genuinely observed during the run:

```text
postgres:17.11-alpine
  postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73
edoburu/pgbouncer:latest
  edoburu/pgbouncer@sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd
```

The live adapter test passed direct-admin migration and restart-idempotency,
runtime read-only schema verification and DDL denial, direct create/
duplicate/conflict/guarded resolution, same-identity concurrency, Advance /
frame / semantic-receipt persistence, stale-head and authority-fence
handling, rollback and unknown-commit resolution, and the transaction-pooler
duplicate/resolve path. The harness and all owned containers cleaned up.

## Real SQLite source, native restore, and transfer attempts

A temporary `worldstreamd` process was started with an owner-only bootstrap
secret. The existing HTTP route was used to create one Counter Room:

```text
target/debug/worldstreamd --data-dir <temporary-data-dir> --bind 127.0.0.1:<port>
POST /v1/rooms                       HTTP 200
source room count                    1
source member count                  1
```

The source was then passed to the existing harnesses:

```text
scripts/native-restore-smoke.sh \
  --source <temporary-data-dir>/worldstream.sqlite3 \
  --evidence <temporary-json>
# exit 13: native_restore_or_source_verification_failed

WORLDSTREAM_PG_TRANSFER_TIMEOUT_SECONDS=300 \
scripts/postgres-transfer-smoke.sh \
  --sqlite <temporary-data-dir>/worldstream.sqlite3 \
  --evidence <temporary-json>
# exit 13: sqlite_source_canonical_evidence_incomplete
```

The transfer runner reported PostgreSQL `status: not_checked` and
`finalization: not_attempted`; it stopped at the source gate before migration,
staging, target authority, or target read-back. The disposable target was
cleaned up. No transfer bundle hash or target artifact was produced.

Observed source facts and redacted byte digests:

| Fact | Observed value |
|---|---:|
| SQLite engine | 3.53.4 |
| Room count | 1 |
| Transition count | 0 |
| Snapshot count | 1 |
| Semantic receipts | 1 |
| Stored canonical-source bytes | 2,895 |
| Room subject digest | `6dd4a44719afa83c803971e8a462a3b65f23f5d055987bd21f0fded74a822eac` |
| Genesis bytes digest | `5cef20f4d4a8421088e241f4fbb6ef2b639843722e00266f27909615becdae67` |
| Head bytes digest | `74ddedb9a5d71aefe117100f906055e8e46850154442919e609820da595fcd1a` |
| Core bytes digest | `9ea5f6535a2b041f493bea1bf0aaccec7e05b49bf352cb498f9b2ffedf1b2c92` |
| Activity bytes digest | `b7b36297e5778a8c2081870b567b3d880ad05d3eea37743221dfe57b214123a6` |
| Pack-lock bytes digest | `1c5f75068220f65f9017a062dbe40203c540108b572284d9446a329914008a92` |

Exact blocking diagnostics from the transfer evidence were:

- `canonical_export_metadata_invalid`: deployment lineage/storage epoch was
  absent or invalid.
- `room_head_bytes_unverifiable`: Complete Head bytes did not verify against
  the Room root and lineage.
- `room_core_bytes_unverifiable`: Core materialization bytes did not verify
  against `rooms.core_state_hash`.
- `room_activity_bytes_unverifiable`: Activity materialization bytes did not
  verify against `rooms.activity_state_hash`.
- `sqlite_canonical_export_unavailable`: the reviewed canonical export seam
  could not be opened/exported for this source.
- `transition_evidence_unavailable`: the exact persisted Transition set was
  unavailable.
- `sqlite_native_verification_not_ready`: the bounded native verifier did not
  prove complete source semantics.
- `room_canonical_evidence_incomplete`: the Room did not provide the complete
  verified canonical record set required by the transfer contract.

No secret, DSN, Room ID, or temporary path was emitted into evidence. The
native restore smoke also confirmed its no-side-effect boundary in its
fail-closed output: no pack side effect, Runner contact, frame publication,
or policy evaluation.

## Focused repository checks

```text
cargo test --locked -p worldstream-postgres       # 19 unit + 13 integration passed
cargo test --locked -p worldstream-transfer       # 21 passed
cargo test --locked -p worldstream-backup         # 43 passed
bash -n scripts/postgres-harness.sh scripts/postgres-transfer-smoke.sh scripts/native-restore-smoke.sh
python3 -m unittest tests/postgres_harness.py -v          # 6 passed
python3 -m unittest tests/postgres_transfer_smoke.py -v   # 11 passed
git diff --check                                       # passed
```

## Acceptance-row disposition

| Issue | Evidence-backed result | Unresolved acceptance boundary |
|---|---|---|
| IMO-44 | Disposable PostgreSQL 17.11 direct-admin/runtime and transaction-pooler core conformance passed, including contention and uncertain recovery paths. | No separately retained normalized SQLite-vs-PostgreSQL transcript artifact was produced by this run; parent review must map the live tracer output to every SQLite parity row before closure. |
| IMO-49 | Seven-row PostgreSQL forward migration ledger, direct-admin-only migration, read-only runtime verification, DDL denial, and transaction-pool compatibility passed. | The full issue also requires every frozen SQLite fixture, interrupted SQLite startup migration/recovery, and complete compatibility-edge evidence; this PostgreSQL run does not close those rows. |
| IMO-50 | Provider-neutral conformance tests and the live PostgreSQL core subset passed. | Full cross-backend timer, observation/Cursor, Activation, snapshot, recovery, integrity, deletion, replay, and restart parity was not proven against PostgreSQL by this run. |
| IMO-51 | Transfer contract tests passed; a real Room source reached the existing transfer runner. | The real source failed canonical/native verification before PostgreSQL import, finalization, target authority, replay, or epoch cutover. No live transfer acceptance is claimed. |
| IMO-52 | Backup/verifier tests passed and native restore was attempted in an isolated temporary target. | The real source failed native verification; no clean manifest-backed restore, PostgreSQL native backup restore, or full readiness decision was produced. |

These results are intentionally evidence-only. No Linear status was changed,
and no acceptance row was declared complete solely from fixture or harness
tests.
