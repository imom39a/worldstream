# IMO-44/50 live PostgreSQL follow-up

Date: 2026-08-20

## Scope and evidence boundary

This follow-up covers `crates/worldstream-postgres` and its executable live
evidence boundary in `scripts/postgres-harness.sh`. The compatibility
manifest remains unchanged and remains fail-closed with `release_ready = false`.
The reviewed schema/persistence corpus now includes activation decisions and
intents, activation operation receipts, observation consequences, semantic
receipts, integrity incidents, and deployment metadata in addition to the Room
lineage and materialization tables. The harness expects the complete seven-
migration history through `0007-deployment-metadata-v1` and all 21 reviewed
schema tables; runtime checks remain read-only and fail closed on mismatch.

The live test is
`live_direct_runtime_and_optional_pooler_conformance` in
`crates/worldstream-postgres/tests/postgres_commit.rs`. It is compiled only
with the explicit `conformance-tracer` test feature because the current
production adapter intentionally fences writes until its host authority
snapshot is wired; the test seeds that witness through the existing
conformance-only seam.

When configured, the test proves:

- direct-admin migration and read-only schema verification on PostgreSQL 17;
- runtime read-only schema verification and denial of a runtime `CREATE TABLE`;
- runtime create, same-identity duplicate with byte-equal receipt, changed-
  request conflict, and guarded receipt resolution;
- a real transaction-pooler path when `WORLDSTREAM_POSTGRES_TEST_POOLER_DSN`
  is supplied. The pooler check uses the already-created operation and proves
  duplicate resolution through that path.

`WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN` and
`WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN` are required for the live test. The
runtime DSN may omit TLS only for an explicitly local endpoint (`localhost`,
`127.0.0.1`, `[::1]`, or the standard local socket); remote runtime DSNs still
fail configuration without `sslmode=require`, `verify-ca`, or `verify-full`.
No credential value is recorded here.

## Environment inspection

Host PostgreSQL tools were PostgreSQL 14.13, with no server listening on
127.0.0.1:5432. Docker supplied an isolated `postgres:17.11-alpine` server
(the authored minimum patch) and a disposable PgBouncer transaction pooler.
The PostgreSQL image digest was
`sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73`;
the pooler image was `edoburu/pgbouncer:latest` at digest
`sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd`.
The database reported server version `17.11`. Generated credentials were
test-only and were not printed or persisted in the repository. The containers
were disposable and published only on local ports 55436 (PostgreSQL) and
55435 (PgBouncer).

## Exact live command output

After starting the isolated container, creating the test-only runtime role,
and granting it only `CONNECT`, schema `USAGE`, table `SELECT/INSERT/UPDATE`,
and sequence `USAGE/SELECT`, the following command was run:

```text
cargo test --locked -p worldstream-postgres --features conformance-tracer --test postgres_commit live_direct_runtime_and_optional_pooler_conformance -- --nocapture
```

Exact output from that command:

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.14s
     Running tests/postgres_commit.rs (target/debug/deps/postgres_commit-b87a924d12a33542)

running 1 test
LIVE_POSTGRES=PASS direct_admin=migrate+verify major=17
LIVE_POSTGRES=PASS runtime_ddl=create_denied
LIVE_POSTGRES=PASS runtime=direct create+duplicate+conflict+resolve
LIVE_POSTGRES_POOLER=PASS path=transaction_pool duplicate+resolve
test live_direct_runtime_and_optional_pooler_conformance ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 13 filtered out; finished in 0.89s
```

The run also fixed two PostgreSQL-only defects found by the live path: fresh
`to_regclass` probing now casts to text, and schema fingerprint verification
uses the reviewed migration-contract order rather than PostgreSQL's
alphabetical catalog order.

The parent fixture gates are also green after this expansion:

```text
cargo test --locked -p worldstream-postgres                              # 12 passed
cargo test --locked -p worldstream-postgres --features conformance-tracer # 14 passed
cargo clippy --locked -p worldstream-postgres --all-targets -- -D warnings # pass
```

## Remaining gap

Direct-admin, direct least-privileged runtime, and transaction-pooler evidence
is now executable and recorded for PostgreSQL 17.11. The local pooler used
`pool_mode=transaction` and the same least-privileged runtime role; the live
test verified duplicate resolution through that path. Remote TLS runtime
connection behavior is not claimed: the adapter currently uses the `NoTls`
connector, so only the configuration rejection of non-TLS remote DSNs is
covered. A real TLS runtime connector needs a separately reviewed PostgreSQL
dependency/change. The compatibility manifest remains fail-closed and release
readiness is not claimed for the other unresolved provider and release gates.
