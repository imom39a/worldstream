# PostgreSQL prerequisite audit — IMO-44, IMO-50, IMO-51

Date: 2026-08-21

## Disposition

The PostgreSQL implementation and its fail-closed harness boundaries pass the
bounded local checks below. The three acceptance rows remain open. A usable
direct-admin/runtime PostgreSQL 17.11+ environment with separately supplied
credentials, plus a verified canonical SQLite source for IMO-51, is still
required. No Linear status was changed.

No new local implementation defect was isolated by this audit. The blockers
are missing live evidence and acceptance coverage, not a failing local test.

## Host availability check

All checks were read-only and used no database credentials, cloud resources,
or provider APIs.

- Docker is available through the local `desktop-linux` context (`Docker
  Server 29.4.0`).
- A real local PostgreSQL 17.11 container is already running:
  `worldstream-postgres17-11-pooler-evidence`, image
  `postgres:17.11-alpine`, with `postgres (PostgreSQL) 17.11` reported by the
  container and `pg_isready` accepting on `127.0.0.1:55436`.
- Local PgBouncer endpoints are accepting readiness probes on ports `55435`
  and `55437`.
- Older PostgreSQL 17.6 containers are also present on ports `55432` and
  `55434`; they are below the manifest minimum patch and cannot satisfy the
  release prerequisite.
- The native Homebrew client and readiness tools are PostgreSQL `14.13`.
  Nothing is listening on `127.0.0.1:5432`; `pg_isready` returned exit 2.
- Podman is unavailable because its machine/socket is not connected.
- No `WORLDSTREAM_POSTGRES_TEST_*`, `WORLDSTREAM_PG_HARNESS_*`,
  `WORLDSTREAM_PG_TRANSFER_*`, `PGHOST`, `PGPORT`, `PGUSER`,
  `PGDATABASE`, or `PGSERVICE` environment variables were set.

The running 17.11 container proves that a PostgreSQL 17 server is present,
but it does not prove that this worker has the separate direct-admin and
least-privileged runtime DSNs or their role/privilege setup. I did not inspect
container environment variables or attempt an authenticated connection.
Therefore a usable direct-admin/runtime evidence environment was not
available within the requested credential-free boundary.

## Repository checks

All commands ran from the repository root.

```text
cargo test --locked -p worldstream-postgres                         # pass: 19 unit, 13 integration
cargo test --locked -p worldstream-postgres --features conformance-tracer --test postgres_commit
                                                                    # pass: 17 tests
cargo clippy --locked -p worldstream-postgres --all-targets -- -D warnings
                                                                    # pass
cargo test --locked -p worldstream-transfer                         # pass: 21 tests
cargo clippy --locked -p worldstream-transfer --all-targets -- -D warnings
                                                                    # pass
python3 -m unittest -q tests/postgres_harness.py tests/postgres_transfer_smoke.py
                                                                    # pass: 17 tests
bash -n scripts/postgres-harness.sh scripts/postgres-transfer-smoke.sh # pass
git diff --check -- crates/worldstream-postgres scripts/postgres-harness.sh \
  scripts/postgres-transfer-smoke.sh tests/postgres_harness.py \
  tests/postgres_transfer_smoke.py compatibility.toml compatibility.json
                                                                    # pass
```

The conformance-tracer target's live test was not exercised: its required
`WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN` and
`WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN` variables were unset. The optional
pooler path additionally requires `WORLDSTREAM_POSTGRES_TEST_POOLER_DSN`.

The harness was invoked in external mode with all DSNs removed. It failed
closed as expected:

```text
exit 12
status=configuration_error
error=admin_and_runtime_dsns_required
release_evidence=false
secrets_emitted=false
```

The transfer runner was also not allowed to create a source or target for
this audit. An absent source was rejected before any provider operation:

```text
exit 13
status=incomplete
reason=sqlite_source_not_a_regular_file
postgres.status=not_checked
transfer.finalization=not_attempted
release_evidence=false
secrets_emitted=false
```

## Issue audit

### IMO-44 — PostgreSQL core-commit parity and uncertain recovery

The current adapter has explicit `PostgresAdmin` and
`PostgresRoomStore` profiles. The live integration test requires separate
direct-admin and runtime DSNs and has an optional transaction-pooler path.
Fixture and boundary coverage is green, including duplicate/conflict,
contention, stale-head, rollback/unknown-commit classification, and guarded
receipt resolution.

The acceptance row is not closed because this audit did not run the live
direct-admin/runtime path, and the retained Wave 10 report records that no
separate normalized SQLite-vs-PostgreSQL transcript artifact was produced for
every parity row. The external prerequisite is:

1. PostgreSQL 17.11 or newer, isolated for this evidence run.
2. A direct-admin DSN for migration/schema verification.
3. A separate least-privileged runtime DSN with DDL and migration-ledger
   writes denied.
4. A transaction-pooler DSN in transaction mode for the pooler acceptance
   row.
5. Retained redacted harness output and normalized SQLite/PostgreSQL
   transcripts covering the full IMO-44 list.

### IMO-50 — full SQLite/PostgreSQL kernel conformance

The local PostgreSQL fixture and migration-contract tests are green. The
current source exposes schema/metadata, guarded receipt, commit, and
Activation seams. The existing conformance report explicitly keeps the
public observation attach/ack/prune, snapshot post-commit, integrity/repair,
authorized replay, and full recovery parity boundaries fail-closed; table
shape alone is not treated as implementation evidence.

The acceptance row is therefore not closed. It needs the real PostgreSQL
17.11 direct and transaction-pool runs, the unchanged black-box suite against
both storage profiles, and evidence for timers, frames/Cursors, observation,
Activation, paired snapshots, integrity, deletion/corruption recovery,
replay, restart, and backend error classification. These are beyond the
fixture-only tests run here.

### IMO-51 — SQLite-to-PostgreSQL deployment transfer

The provider-neutral transfer crate passes 21 deterministic tests and clippy.
Those tests prove bundle encoding, byte preservation, resumable chunks,
verification gates, abort/finalization, and deployment-epoch fencing in local
fixtures.

They do not prove a real transfer. The retained Wave 10 evidence records that
the real disposable WorldStream SQLite source failed canonical/native
verification before PostgreSQL import, finalization, target authority, or
epoch cutover. The exact recorded blockers include missing/invalid deployment
lineage and storage epoch, unverifiable Head/Core/Activity bytes, unavailable
canonical export/Transition evidence, and incomplete canonical Room evidence.

The external prerequisite is a real disposable SQLite deployment that passes
the native canonical verifier, plus the PostgreSQL 17.11 admin/runtime
environment above. Only then can the bounded transfer runner prove import,
semantic verification, source retirement, target first-write fencing, and
irreversible cutover.

## Final handoff

Current status is **blocked on external evidence prerequisites**:

- PostgreSQL 17.11 server: present locally and readiness-checked.
- Separate direct-admin/runtime credentials and role provenance: not
  available to this worker and intentionally not inspected.
- Full IMO-44/IMO-50 live parity evidence: not produced by this audit.
- Verified canonical SQLite source for IMO-51: not available to this audit.

No Rust, script, manifest, or other agent file was edited. The only file added
by this worker is this report.
