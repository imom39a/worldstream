# IMO-44 PostgreSQL commit evidence

The PostgreSQL implementation is in `crates/worldstream-postgres` and owns
only the Core `RoomCommitStorageV1` adapter plus its conformance harness.

The runtime profile is least-privileged and never migrates. Offline schema
work is exposed through `PostgresAdmin` and requires the direct-admin profile.
Every mutation and `resolve` operation creates one transaction, serializes the
canonical Operation Identity with a durable unique-key row and `FOR UPDATE`,
and Existing mutations then lock the Room root with `FOR UPDATE` under the
database default Read Committed isolation. No advisory lock, session state,
named prepared statement, connection affinity, extension, replica, or provider
API is part of correctness. The transaction-pool path opens a fresh client for
each operation.

`FixturePostgresStore` runs the same Core-prepared create/advance and receipt
resolution corpus in-process for deterministic local evidence. It is clearly
classified as `UnavailableEnvironment`, not as a PostgreSQL provider pass.
`probe_from_environment` only reports `LiveProvider` when
`WORLDSTREAM_POSTGRES_TEST_DSN` connects and reports PostgreSQL major 17;
otherwise it fails closed.

The release compatibility manifest remains unresolved and `release_ready =
false`. No live PostgreSQL 17, direct-admin, or pooler evidence is claimed by
this change unless the opt-in environment variable is supplied and the live
conformance suite is run against that service.
