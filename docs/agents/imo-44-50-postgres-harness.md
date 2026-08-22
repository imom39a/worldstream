# IMO-44/IMO-49/IMO-50 PostgreSQL lane audit

Date: 2026-08-21
Lane: Luna / PG-DOCS

## Audit boundary

This is a documentation-only audit of the current checkout. I inspected the
acceptance surface represented by `compatibility.toml`, `compatibility.json`,
`docs/gates.md`, the PostgreSQL source and integration target, the PostgreSQL
harness, and the existing `docs/agents/` reports. I ran no tests, harnesses,
Docker commands, `psql` commands, provider calls, or hosted checks in this
audit. The live results below are retained results quoted from existing lane
reports, not new results produced by this audit.

No authoritative issue body or acceptance-criteria export for IMO-44/49/50 is
present in the checkout. The issue mapping below is therefore reconstructed
from the manifest and the existing lane reports; it should not be treated as a
Linear status update.

## Current acceptance surface and disposition

| Issue | Current acceptance surface | Local evidence in checkout | Disposition after audit |
|---|---|---|---|
| IMO-44 | PostgreSQL 17.11+ direct-admin migration, least-privilege runtime, transaction-pool path, Room commit idempotency/contention, and guarded recovery resolution. | The retained Wave 6 report records a disposable `postgres:17.11-alpine` plus PgBouncer transaction-pool run, exit `0`, and redacted direct/runtime/pooler PASS lines. | Narrow local provider lane is evidenced by the retained report. Full cross-backend parity and remote/hosted acceptance are not established here. |
| IMO-49 | Forward-only migration history, reviewed IDs/checksums/fingerprint, migration restart safety, and the corresponding SQLite/PostgreSQL migration contract. | The Wave 7 report records focused SQLite migration-contract checks and the seven PostgreSQL descriptors/checksums. The harness source requires seven PostgreSQL ledger rows and validates their metadata. | Implementation and fixture evidence are present; the manifest’s `all-prior-forward-migrations-both-backends` release row remains unresolved. No full release acceptance is claimed. |
| IMO-50 | The PostgreSQL adapter must match the SQLite kernel contract beyond table shape: commit/receipt semantics plus timers, frames/cursors, observations, Activation, snapshots, integrity, replay/recovery, restart, and failure behavior. | The adapter has provider-neutral fixture vectors and a retained live core subset. The existing IMO-50 report explicitly lists public lifecycle and cross-adapter gaps. | Partial/core evidence only. Full cross-backend acceptance remains open. |

The manifest currently records `postgresql-direct-and-transaction-pooler-
conformance` as resolved with digest
`sha256:6e1391fd036fa5b9fa7cb58c164778e1acfb6ece0b907340295a4e300afad8ea`
(`compatibility.toml:487-491`), but `release_ready = false` and the broader
migration/SQLite/transfer/restore rows remain unresolved. The latest retained
Wave 6 report names a different redacted evidence artifact digest,
`c4819dc2accd595f720534cf05c27abd53b476faa900499d861d146ac2c5fc4a`.
The inspected docs do not identify the artifact represented by either digest;
that provenance must be reconciled before using the manifest row as closure
evidence.

## Code and harness audit

### IMO-44 implementation

- `PostgresConnectionConfig` separates direct-admin from least-privileged
  runtime profiles and labels direct, session-pool, and transaction-pool paths
  (`crates/worldstream-postgres/src/lib.rs:154-237`). Runtime configuration
  rejects non-local DSNs without an explicit TLS mode, but all current adapter
  connections use `NoTls` (`lib.rs:1200-1202`, `lib.rs:1623-1625`). Therefore
  remote TLS connectivity is a configuration policy boundary, not a proven
  hosted-runtime result.
- `PostgresAdmin::migrate` is direct-admin-only, forward-only, and commits each
  migration body with its ledger row in one transaction; schema verification is
  read-only (`lib.rs:361-444`).
- The runtime commit path inserts/locks the operation guard, returns an
  idempotent stored receipt or conflict, and classifies provider failures into
  retryable versus indeterminate outcomes (`lib.rs:2098-2140`, and the
  `PostgresStorageFailure` mapping at `lib.rs:262-350`).
- The live integration target is feature-gated and seeds authority through the
  `conformance-tracer` test seam (`tests/postgres_commit.rs:675-717`,
  `lib.rs:1601-1621`). Its live PASS transcript consequently proves the
  configured adapter/conformance fixture path, not the complete production host
  authority integration.

### IMO-49 migration contract

- PostgreSQL has seven reviewed migration descriptors and checksums; the
  harness independently requires seven contiguous ledger rows with the reviewed
  IDs, 32-byte checksums, logical history, and schema fingerprint
  (`scripts/postgres-harness.sh:694-718`).
- The retained Wave 7 report records local migration-contract checks, including
  six SQLite descriptors, seven PostgreSQL descriptors, identity/checksum drift,
  mixed/gapped/future history, and restart behavior
  (`docs/agents/imo-49-migration-contract-wave7-2026-08-21.md:21-77`).
- The existing report also states that SQLite’s persisted ledger has no checksum
  column and therefore does not claim persisted SQLite checksum verification
  (`...wave7...md:79-90`). This is a real acceptance boundary, not a documentation
  omission.

### IMO-50 conformance

- `src/conformance.rs` supplies a provider-neutral black-box vector for create,
  same-identity duplicate, changed-request conflict, and guarded receipt
  resolution. That vector is valuable local contract evidence but is not, by
  itself, a two-backend run.
- PostgreSQL schema additions for observations, Activation, semantic receipts,
  snapshots, integrity, timers, and transfer metadata are not equivalent to all
  public operations being implemented. The existing IMO-50 report explicitly
  leaves observation attach/ack/prune, Activation lifecycle APIs, authorized
  receipt reads, replay/recovery, snapshot post-commit, and integrity repair
  open (`docs/agents/imo-50-postgresql-conformance.md:38-54`).

### Harness behavior

The current shell harness is fail-closed:

- Default managed mode owns a temporary local cluster; external mode requires
  explicit admin and runtime DSNs (`scripts/postgres-harness.sh:387-438`).
- It accepts only PostgreSQL major 17 with patch 11 or newer and rejects other
  versions (`scripts/postgres-harness.sh:506-532`).
- It runs provider-neutral fixture checks and then the feature-gated live adapter
  test; fixtures are reported as provider-neutral rather than provider evidence
  (`scripts/postgres-harness.sh:650-681`).
- Runtime verification requires exactly 21 enumerated tables, denies migration
  ledger writes and DDL, and keeps schema verification read-only
  (`scripts/postgres-harness.sh:721-777`). Existing lane prose saying “twenty”
  tables is stale.
- A pooler DSN is required for a pass; absent pooler is exit `12` with
  `transaction_pooler_not_configured` (`scripts/postgres-harness.sh:807-827`).
  The harness does not provision PgBouncer, so a managed-mode local cluster
  still needs a separately supplied transaction-pooler endpoint.
- Boundary tests use fake `psql` and explicitly do not claim provider
  availability or live migration (`tests/postgres_harness.py:1-7`).

## Retained local evidence

The strongest retained local evidence is
`docs/agents/imo-44-50-pooler-wave6-2026-08-21.md:39-127`:

```text
cargo test --locked -p worldstream-postgres --features conformance-tracer \
  --test postgres_commit live_direct_runtime_and_optional_pooler_conformance -- --nocapture
=> exit 0

LIVE_POSTGRES=PASS direct_admin=migrate+verify major=17
LIVE_POSTGRES=PASS direct_admin=restart-idempotent
LIVE_POSTGRES=PASS runtime_ddl=create_denied
LIVE_POSTGRES=PASS runtime=direct create+duplicate+conflict+resolve
LIVE_POSTGRES=PASS runtime=direct same-room-create=reprepare
LIVE_POSTGRES=PASS runtime=direct advance+frame+semantic-receipt+same-identity-concurrency
LIVE_POSTGRES=PASS runtime=direct rollback+unknown-commit=guarded-resolution
LIVE_POSTGRES_POOLER=PASS path=transaction_pool duplicate+resolve

scripts/postgres-harness.sh --evidence <redacted>
=> exit 0; redacted evidence artifact SHA-256=c4819dc2accd595f720534cf05c27abd53b476faa900499d861d146ac2c5fc4a
```

The retained run used local disposable images `postgres:17.11-alpine` and
`edoburu/pgbouncer:latest`, with recorded image digests in that report. It is
local disposable evidence only; it is not a Supabase, RDS, hosted, HA,
failover, power-loss, or remote-TLS result.

The same report records local boundary checks of six harness unit tests,
`bash -n`, 17 feature-enabled integration tests, SQLSTATE lock/deadlock/
serialization probes, identity separation, and cleanup. Those are retained
report claims and were not rerun during this documentation-only audit.

## Explicit blockers and next evidence

1. Reconcile the manifest digest with the retained evidence artifact and retain
   the exact machine-readable artifact under an auditable path.
2. Produce the complete IMO-44 normalized SQLite/PostgreSQL transcript, not only
   the retained PostgreSQL core subset.
3. Complete IMO-49 evidence for both backends’ forward migration history,
   restart/interruption behavior, and the unresolved release evidence row.
4. Run the unchanged black-box contract against both storage profiles for the
   IMO-50 operations still called out by the existing report: observations,
   Activation lifecycle, timers/frames/cursors, snapshots, integrity,
   replay/recovery, restart, and backend failures.
5. For any hosted/provider claim, supply a separately authorized PostgreSQL
   17.11+ service with distinct direct-admin and runtime identities, a
   transaction-mode pooler identity, runtime migration-ledger/DDL denial, and
   retained redacted output. No such hosted result is claimed here.
6. Remote runtime TLS remains blocked at the connector boundary: the adapter
   currently connects with `NoTls`; a reviewed TLS-capable connector and its
   acceptance run are still required for the manifest’s `remote_tls_required`
   policy.

## Audit conclusion

The checkout contains a credible local PostgreSQL adapter/harness and retained
disposable PostgreSQL 17.11 transaction-pool evidence. IMO-44 is supported only
at that narrow local-provider boundary; IMO-49 has implementation/fixture
evidence but unresolved release-level both-backend proof; and IMO-50 remains
partial because the richer SQLite/PostgreSQL contract has not been exercised
end-to-end. No hosted result, provider SLA, release readiness, or issue closure
is claimed.

## Parent verification addendum

The parent orchestration reran the locally achievable checks after the Luna
lanes completed:

```text
bash -n scripts/postgres-harness.sh                         # pass
uv run --project sdk/python --locked python tests/postgres_harness.py
                                                               # pass: 13 tests
cargo fmt --manifest-path crates/worldstream-postgres/Cargo.toml -- --check
                                                               # pass
cargo test --locked -p worldstream-postgres --lib --tests   # pass: 21 + 13
cargo test --locked -p worldstream-postgres --features conformance-tracer \
  --test postgres_commit -- --nocapture                       # pass: 17;
                                                               # live provider skipped: admin DSN unset
```

The native Homebrew PostgreSQL tools are 14.13. The managed harness therefore
failed closed with exit `11`, `evidence_class=unsupported_provider`,
`server_version_num=140013`, `release_evidence=false`, and owned-cluster
cleanup `pass`.

A separate disposable Docker attempt reached PostgreSQL 17.11 and verified
distinct admin/runtime roles. The adapter emitted all direct-admin/runtime
markers successfully, then the locally supplied PgBouncer endpoint failed at
pooler schema admission; the direct test exited `101` at
`live pooler schema verification`, and the harness classified the run as exit
`13` with `release_evidence=false`. This is a local pooler-configuration
blocker, not a PostgreSQL 17 or hosted-provider result. The harness now keeps
successful direct markers when a later pooler marker fails, and boundary tests
cover that normalization. No compatibility manifest or Linear status was
changed.
