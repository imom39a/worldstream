# WorldStream: portable PostgreSQL constraints for Supabase-first, AWS-later

> **Research snapshot, not a production design.** Reviewed 2026-08-14. This note uses public, first-party SQLite, PostgreSQL, Supabase, and AWS documentation only. It does not create, configure, connect to, or modify Supabase or AWS resources. “AWS-later” here means ordinary managed PostgreSQL (Amazon RDS for PostgreSQL), not an AWS application-service dependency.

## Decision in brief

**Recommendation — adopt a PostgreSQL-shaped, transaction-first persistence contract, with SQLite as a deliberately constrained local conformance target.** One accepted Room mutation should be one short database transaction that durably commits: the conditional Room-head advance and current state, immutable Transition, action/mutation receipt, audience-specific frames, timer changes, and activation intents. Return or publish success only after commit. The repository already names that bundle as the atomic transition/receipt/timer/frame/activation invariant in its [roadmap](../docs/roadmap.md#persistence-and-replay) and protocol.

**Recommendation — make idempotency, rather than a successful network response, the recovery contract.** A client or runner retries the same canonical request and key after a timeout, failover, or lost response; the durable receipt returns the committed result. This avoids claiming that a caller can know whether a connection lost during `COMMIT` committed.

**Recommendation — do not make Supabase Auth, Data API/PostgREST, Realtime, Storage, Edge Functions, Supavisor-specific behavior, RDS roles, extensions, replicas, or backup APIs part of the Room Kernel contract.** Use ordinary PostgreSQL SQL over a standard driver. Supabase can be the first host and RDS a later host without changing the application-domain persistence API.

## Evidence boundary and the Room commit

This research evaluates the concrete WorldStream commit boundary: an admitted stimulus and ordered Transition, updated core/activity state and hashes, scoped Observation Frames, timer changes, Activation Intents, and a durable action disposition must either all commit or none do. It does not choose a driver, ORM, cloud topology, or migration framework.

### 1. Transaction, isolation, and locking semantics

#### Facts

- PostgreSQL’s default isolation level is Read Committed; each `SELECT`, `INSERT`, `UPDATE`, and `DELETE` sees data committed before that statement began. `REPEATABLE READ` and `SERIALIZABLE` provide stronger transaction-level semantics, and PostgreSQL documents that serialization failures at `SERIALIZABLE` must be retried as complete transactions. [PostgreSQL transaction isolation](https://www.postgresql.org/docs/current/transaction-iso.html)
- PostgreSQL row-level locks, including `SELECT ... FOR UPDATE`, block conflicting row lockers/writers until the transaction ends; the lock is released at transaction end. [PostgreSQL explicit locking](https://www.postgresql.org/docs/current/explicit-locking.html#LOCKING-ROWS)
- SQLite normally isolates separate connections so uncommitted changes are invisible. Except for shared-cache connections that explicitly enable `PRAGMA read_uncommitted`, SQLite describes its transactions as serializable and serializes writes: there can be only one writer at a time. [SQLite isolation](https://www.sqlite.org/isolation.html)
- SQLite `BEGIN DEFERRED` starts no write transaction until the first write; `BEGIN IMMEDIATE` starts a write transaction immediately and can fail with `SQLITE_BUSY` if another write transaction is active. In WAL mode, `EXCLUSIVE` and `IMMEDIATE` are equivalent. [SQLite transaction control](https://www.sqlite.org/lang_transaction.html)
- WAL mode lets readers continue while a writer appends to the WAL, but still has one writer at a time; readers see a snapshot from the start of their read transaction. [SQLite isolation](https://www.sqlite.org/isolation.html)
- SQLite WAL requires all database processes to be on the same host and is not a network-filesystem shared-database mechanism. [SQLite WAL](https://www.sqlite.org/wal.html)

#### Recommendations

1. **Keep the Room actor/single-logical-writer invariant above the database.** Serialize mutation admission per Room in the runtime, then use one short transaction to make its result durable. SQLite’s one-writer model is therefore a compatible local test target, not a claim that it models PostgreSQL concurrency throughput.
2. **Protect the Room head in the same transaction.** Store `room_id`, `head_seq`, and state/hash on a root row. Read the expected head and make the state/head update conditional (`... WHERE room_id = ? AND head_seq = ?`); require exactly one changed row before inserting the rest of the commit bundle. A zero-row update is a stale/concurrent-writer conflict: discard the computed candidate and re-load/re-admit, never append a partial Transition.
3. **Use PostgreSQL row locking only as an optional Postgres implementation strengthening, not as the portable application contract.** A later multi-process PostgreSQL deployment may lock the Room root (`FOR UPDATE`) before validating and advancing it. SQLite does not implement `FOR UPDATE`, so the portable conformance contract remains “one Room writer plus conditional-head success,” not a shared SQL string.
4. **Start with PostgreSQL Read Committed plus the explicit Room-head guard.** It is sufficient for the one-actor contract. If a future design permits independent concurrent writers per Room, select a proven locking/isolation strategy, treat PostgreSQL serialization/deadlock errors as retryable whole-transaction outcomes, and add an explicit SQLite-equivalence test; do not silently assume an isolation setting transfers across engines.
5. **For SQLite local integration tests, enable foreign keys for every connection and use WAL only when the test needs concurrent readers.** SQLite foreign-key enforcement is disabled by default unless enabled per connection with `PRAGMA foreign_keys = ON`. [SQLite foreign-key support](https://www.sqlite.org/foreignkeys.html#fk_enable) Keep the SQLite file on a local filesystem.

### 2. Connection and pooling constraints

#### Facts

- Supabase supplies direct PostgreSQL connections for migrations, `pg_dump`, and long-lived backends; its shared pooler offers session mode and transaction mode. The docs identify transaction mode for transient/serverless clients and state that transaction mode does **not** support prepared statements. [Supabase connection methods](https://supabase.com/docs/guides/database/connecting-to-postgres)
- In transaction pooling, a backend connection is shared for the duration of a transaction rather than being a client’s permanent session. Supabase specifically calls out backend session state such as `search_path` and read-only mode as relevant to transaction-pooler behavior. [Supabase transaction-pooler troubleshooting](https://supabase.com/docs/guides/troubleshooting/resolving-cannot-execute-update-in-a-read-only-transaction-on-transaction-pooler-connections-ef582c)
- Amazon RDS for PostgreSQL accepts standard SQL client applications, but it does not grant host access and restricts procedures/tables requiring advanced privileges. [Amazon RDS for PostgreSQL](https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/CHAP_PostgreSQL.html)

#### Recommendations

- Treat a database connection as **transaction scoped** in all application code, even when using a direct connection. Do not rely on connection identity, temporary tables, `LISTEN`/`NOTIFY` as Room correctness machinery, `SET` values that persist across requests, advisory locks held outside the commit transaction, cursors, or prepared statements for the portable commit path.
- Issue transaction characteristics explicitly and locally when needed (for example, `SET LOCAL`, not long-lived `SET`), and reset/avoid session state before the transaction completes. Configure a Supabase transaction-pooler client not to use prepared statements; use a direct/session connection for migrations and logical dump/restore.
- Place app-side pooling limits, timeouts, and retry policy behind configuration. Do not encode Supabase ports, project refs, pooler user naming, IPv4/IPv6 choices, or RDS endpoints in domain code.

### 3. Schema and migration portability

#### Facts

- PostgreSQL supports transactional DDL for most commands: if the surrounding transaction does not commit, the DDL does not take effect. PostgreSQL documents exceptions, including `CREATE DATABASE`, `DROP DATABASE`, and `CREATE TABLESPACE`. [PostgreSQL transactional DDL](https://www.postgresql.org/docs/current/ddl-transactional.html)
- SQLite supports only a limited `ALTER TABLE` subset (rename table, rename column, add column, and—subject to version—drop column); more elaborate changes require the documented create-new-table/copy/drop/rename procedure. [SQLite `ALTER TABLE`](https://www.sqlite.org/lang_altertable.html)
- SQLite uses dynamic typing/type affinity rather than PostgreSQL’s static type system; SQLite’s `STRICT` tables are an optional SQLite feature, not a PostgreSQL portability feature. [SQLite datatypes](https://www.sqlite.org/datatype3.html)
- SQLite foreign keys must be enabled separately for each database connection, and SQLite documents that its default might change in a future release rather than promising it is always off. [SQLite foreign keys](https://www.sqlite.org/foreignkeys.html)
- RDS exposes only a supported set of PostgreSQL extensions, with availability varying by engine version and parameter configuration. [RDS supported extensions](https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/PostgreSQL.Concepts.General.FeatureSupport.Extensions.html)
- The current Supabase changelog says support for PostgreSQL 14 ended on 2026-07-01 and describes automatic upgrades or pausing projects when removed extensions are in use; its PostgreSQL-tagged changelog also records extension-level breaking changes. [Supabase changelog: deprecations](https://supabase.com/changelog?types=deprecation), [PostgreSQL-tagged changes](https://supabase.com/changelog?tags=postgres)

#### Recommended portable subset

| Area | Portable baseline | Deliberately exclude from the Room Kernel schema |
| --- | --- | --- |
| IDs and order | Application-generated text IDs; integer Room/frame sequences guarded by unique constraints; explicit prior/hash columns. | `SERIAL`/identity as the semantic Room order, provider IDs, timestamp-derived order. |
| Scalars | `TEXT`, bounded integer values, `BLOB`/`bytea` through a repository type mapping, canonical JSON stored as text, timestamps as an explicit canonical representation. | PostgreSQL-only enums, arrays, ranges, composite types, `jsonb` operators, SQLite affinity as validation. |
| Integrity | `NOT NULL`, `PRIMARY KEY`, `UNIQUE`, `FOREIGN KEY`, `CHECK`, and commit-time application validation. | Required extensions, triggers/functions that depend on a provider role or an app service. |
| Mutations | Parameterized SQL, explicit column lists, named versioned migrations, `INSERT ... ON CONFLICT` only after both engines’ behavior is tested for that statement. | Database-specific upsert shortcuts, session-state assumptions, implicit casts. |
| Schema evolution | Additive expand → application/backfill → validate → contract migrations; SQLite may rebuild a table for constraint/type changes. | Assuming arbitrary `ALTER TABLE` works on SQLite or relying on PostgreSQL-only transactional-DDL exceptions. |

**Recommendation — retain one migration history, but permit a tiny dialect layer for DDL.** The schema model and migration ordering should be shared; SQLite table-rebuild steps and PostgreSQL-specific operational DDL can be implementation-specific migration operations with the same resulting schema contract. Do not claim byte-for-byte identical SQL is the portability criterion.

**Recommendation — keep canonical room state, stimuli, transition payloads, frame payloads, receipts, and hashes as deterministically serialized application values.** Database JSON conveniences may be added later as non-authoritative read indexes only after a host-by-host migration review.

### 4. Backup, restore, and failure semantics

#### Facts

- `pg_dump` makes a consistent database export and normally does not block other database users, while noting that it needs a shared lock and can conflict with an operation requiring an exclusive lock. Its documentation distinguishes a database dump from cluster-global objects such as roles/tablespaces; `pg_dumpall` is the cluster-wide tool. [PostgreSQL `pg_dump`](https://www.postgresql.org/docs/current/app-pgdump.html), [PostgreSQL backup and restore](https://www.postgresql.org/docs/current/backup.html)
- Supabase documents daily backups by paid tier and optional PITR with recovery points at up-to-second granularity. Restoring makes the project inaccessible during the process; database backups do not include objects kept through the Storage API. [Supabase backups](https://supabase.com/docs/guides/platform/backups)
- RDS automated backups retain according to a configured 1–35 day period; point-in-time restore creates a **new** instance and leaves the original intact. A snapshot restore also creates a new instance rather than restoring into an existing one. [RDS backup/restore overview](https://docs.aws.amazon.com/AmazonRDS/latest/gettingstartedguide/managing-backup-restore.html), [RDS snapshot restore](https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/USER_RestoreFromSnapshot.html)
- RDS PostgreSQL read replicas use asynchronous physical streaming replication. [RDS PostgreSQL read replicas](https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/USER_PostgreSQL.Replication.ReadReplicas.Configuration.html)
- During RDS Multi-AZ cluster failover, the writer endpoint changes DNS target and existing connections must be re-established. [RDS Multi-AZ cluster failover](https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/multi-az-db-clusters-concepts-failover.html)

#### Recommendations

1. **Define recovery in logical terms, independent of host:** restore a PostgreSQL-compatible database, apply the same ordered migrations, verify immutable genesis/Transition hashes to each Room head, then reopen traffic. A backup is a recovery point, not proof that external effects or live client delivery are exactly-once.
2. **Keep durable external content outside the atomic SQL claim unless its storage has an independently verified, crash-safe staging/link protocol.** A database backup cannot by itself restore an external object store’s bytes; Supabase’s Storage exclusion makes the general risk concrete. Record immutable digests and verify referenced content during restore.
3. **Run portable disaster-recovery drills.** For each supported host, restore into an isolated target, run migrations, verify room replay/hash chains and receipt uniqueness, and demonstrate that a request whose response was lost resolves through its idempotency receipt. Do not make the app depend on in-place restore: RDS restore is new-instance based, and Supabase restore has availability downtime.
4. **Write retry behavior for interruption, not a particular cloud.** A failed/closed connection before a known commit is retryable with the same key. If the connection fails during/after commit, treat the outcome as unknown and look up/re-submit the same key. Retry only transactions explicitly classified as transient; never retry a non-idempotent external side effect as though the database transaction made it exactly once.
5. **Use the primary/writer for canonical reads and writes.** Replicas may serve explicitly stale, non-authoritative inspection queries only; never make action admission, Room-head validation, receipt lookup, replay verification, or cursor safety depend on them.

### 5. Hosted-provider divergence that can leak into application design

| Documented divergence | Application-safe boundary |
| --- | --- |
| Supabase transaction pooling does not support prepared statements; direct connections are documented for migrations and dump tools. [Supabase connection methods](https://supabase.com/docs/guides/database/connecting-to-postgres) | Make the runtime correct with transaction-scoped connections and ordinary parameterized statements; configure prepared statements per deployment. |
| Supabase backup/PITR retention and downtime behavior differ from RDS backup retention and new-instance restores. [Supabase backups](https://supabase.com/docs/guides/platform/backups), [RDS backups](https://docs.aws.amazon.com/AmazonRDS/latest/gettingstartedguide/managing-backup-restore.html) | Specify recovery-point objective, recovery-time objective, replay verification, and external-artifact verification; leave retention, restore controls, and endpoint cutover to operations. |
| RDS lacks true PostgreSQL superuser/host access and only supports selected extensions. [RDS roles](https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/Appendix.PostgreSQL.CommonDBATasks.Roles.rds_superuser.html), [RDS extensions](https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/PostgreSQL.Concepts.General.FeatureSupport.Extensions.html) | Require no extension or superuser for correctness. Treat an extension as optional acceleration with a baseline query/path. |
| RDS failover interrupts client connections; replicas are asynchronous. [RDS failover](https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/multi-az-db-clusters-concepts-failover.html), [RDS replication](https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/USER_PostgreSQL.Replication.ReadReplicas.Configuration.html) | Reconnect and use receipt-based retry; use the writer for authoritative work. |
| Supabase’s current changelog demonstrates version and extension lifecycle changes. [Supabase changelog](https://supabase.com/changelog?tags=postgres) | Pin and test supported PostgreSQL majors in CI; keep a version/extension compatibility manifest outside domain logic. |

No documented difference above requires a different Room transition model. They require operational configuration and deliberately conservative client behavior.

## Conformance plan: local SQLite plus local PostgreSQL

**Recommendation — make these database semantics tests mandatory before selecting a managed host.** Run the same black-box test suite against a fresh SQLite database and a local PostgreSQL version supported by the chosen host; run a small PostgreSQL-only suite for row locks/isolation if that optional path is enabled.

1. **Schema contract:** apply every migration from empty; assert required tables, columns, primary/unique/FK/check constraints, and indexes. On SQLite, enable foreign keys on every test connection. Reapply/upgrade from representative prior schemas and verify the SQLite rebuild path preserves data and constraints.
2. **Atomic accepted commit:** force a failure at each write boundary (state/head, transition, receipt, frame, timer, activation) and assert zero of the bundle is visible after rollback. Then assert the successful case has every member, one new Room sequence, and the receipt’s canonical request hash/result.
3. **Idempotency and uncertainty:** submit the same action twice; simulate response loss after commit; retry the same key/body and require the original result. Reuse the key with a changed body and require conflict. Simulate pre-commit failure and require no receipt/sequence consumption.
4. **Contention:** submit two actions from the same expected Room head. Exactly one may advance it; the other must be deterministically retried/rejected without duplicate frames, timers, or activation intents. SQLite `BUSY`/lock outcomes and PostgreSQL lock/deadlock/serialization outcomes are transport/storage retries, not domain acceptance.
5. **Replay and restore:** export/restore (or copy a disposable local database), run migrations, reconstruct every test Room from genesis plus Transitions, and compare recorded head/state/transition hashes. Separately verify that external-artifact references are either present and digest-valid or produce an explicit unhealthy/recovery result.
6. **Pool discipline:** run the commit tests with a PostgreSQL transaction pooler-compatible driver configuration (no prepared statements and no session-state dependence), and repeat migrations/dump tests through a direct PostgreSQL connection.

## Exit criteria for a host decision

**Recommendation — call the storage layer portable only when all of these are true:**

- the SQLite and local PostgreSQL suites pass the shared contract above;
- supported PostgreSQL major versions and optional extensions are written down and tested;
- every production mutation uses a receipt/idempotency key and commit-before-ack;
- migrations, logical backup export, isolated restore, replay/hash verification, and endpoint cutover have been rehearsed without provider app services; and
- any deliberate departure (for example, PostgreSQL row locking, `jsonb` indexes, or an extension) has a baseline behavior, a feature flag/configuration boundary, and a documented SQLite test strategy.

## Sources consulted

Primary documentation links are placed with the claims they support. The review also checked the current [Supabase PostgreSQL changelog](https://supabase.com/changelog?tags=postgres) and [breaking-change/deprecation changelog](https://supabase.com/changelog?types=deprecation), as required for a Supabase-first assessment. Sources were accessed 2026-08-14; provider plan/version availability can change after that date.
