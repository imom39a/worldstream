# IMO-59 PostgreSQL packaged-runtime Luna lane

Date: 2026-08-21

## Implemented

- `worldstreamd` now has a startup-fixed `postgres-primary` branch. It reads
  the configured runtime DSN only from an owner-only secret file, applies the
  existing `PostgresConnectionConfig::runtime` TLS-policy check, constructs a
  `PostgresRoomStore` with the least-privileged runtime profile, verifies the
  reviewed PostgreSQL schema before binding the HTTP listener, and attaches
  the existing bounded `PostgresTelemetryBridge` with `adapter=postgres`.
- The SQLite branch remains unchanged in its telemetry, authority bootstrap,
  canonical metadata, engine identity, readiness, and scheduler startup
  sequence.
- `worldstreamctl postgres migrate --dsn-file FILE` and
  `worldstreamctl postgres verify --dsn-file FILE` are explicit offline
  direct-admin operations. The DSN is read from an owner-only file, bounded,
  never accepted as a plaintext argument, and never included in success or
  failure output. The daemon has no admin DSN path.
- A `PostgresGatewayBackend` is wired into the daemon as a fail-closed
  `GatewayBackend` facade. It retains the verified runtime store and maps all
  unsupported Room, authority, Runner, Activation, and scheduler operations to
  the existing bounded `StorageUnavailable` error; it does not manufacture a
  welcome, Room, projection, lease, or readiness claim.

## Deliberate remaining boundaries

The current provider API cannot yet support a production PostgreSQL
`SqliteGatewayBackend` equivalent. Exact missing seams are:

1. `PostgresRoomStore` has no production `AuthorityStoreV1` implementation or
   runtime bearer authentication/bootstrap path. Its authority-fence seed is
   feature-gated conformance code and cannot be used by the daemon.
2. The provider exposes Core commit/conformance primitives, but not the host
   Core admission/sealing adapters needed to build the server's authenticated
   `create_room`, action, timer, replay, projection, observation attach/ack,
   capability, Runner, and Activation gateway operations. Existing
   `*_conformance` methods require test-only Core authority witnesses.
3. The server crate currently exposes only SQLite-named startup constructors
   for verified readiness and engine identity. There is no provider-neutral
   `RuntimeReadiness::postgres_store_verified` or
   `EngineVersion::postgres_verified` seam. Consequently the daemon leaves
   PostgreSQL `/readyz` fail-closed (`503`) and `/version.engine` honestly
   `not_initialized` after the provider schema probe; `/healthz`, `/version`,
   and the operator routes still start and remain available.
4. The existing PostgreSQL adapter uses `NoTls` for its driver connections.
   Runtime configuration rejects remote DSNs without an explicit
   `sslmode=require`, `verify-ca`, or `verify-full`, but a real remote TLS
   connector still needs a reviewed PostgreSQL TLS dependency seam.
5. `SecretSource` exposes only a fixed-width bearer reader. Variable-length
   inherited-handle DSNs are therefore rejected with a bounded diagnostic;
   owner-only DSN files are supported. A shared variable-length secret reader
   is required before inherited DSN handles can be enabled safely.

These are intentionally documented rather than hidden behind SQLite identity,
an in-memory authority store, a no-op scheduler, or synthetic protocol success.

## Verification

From `/Users/vinothshanmugam/code/agent-streamer`:

```text
cargo fmt --all -- --check                                      PASS
cargo check --locked -p worldstream-server --bins              PASS
cargo clippy --locked -p worldstream-server --all-targets -- -D warnings
                                                               PASS
cargo clippy --locked -p worldstream-postgres --all-targets -- -D warnings
                                                               PASS
cargo test --locked -p worldstream-server --all-targets --no-fail-fast
                                                               PASS: 72 library + 10 bin tests
```

The CLI surfaces were also checked with `worldstreamctl postgres --help` and
`worldstreamd --help`. No commit, packaging change, manifest change, or
Linear update was made by this lane.
