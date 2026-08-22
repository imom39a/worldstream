# IMO-58 PostgreSQL telemetry producer

Status: PostgreSQL storage-owned producer and server integration are parent-verified.

## Implementation

- Added `crates/worldstream-postgres/src/telemetry.rs` with the closed
  `PostgresTelemetryEventV1` vocabulary and `PostgresTelemetrySink` seam.
- Added panic-isolated emission and a migration guard that emits
  `started`, `applied`/`already_current`, or `failed` from the real
  `PostgresAdmin::migrate` call.
- Added optional sinks to `PostgresAdmin` and `PostgresRoomStore` without
  changing their default behavior.
- Added recovery facts around the real snapshot-cache rebuild path and the
  feature-gated Core trace recovery path.
- Added integrity facts for guarded fault/quarantine transitions and closed
  storage diagnostics for schema verification, Room verification, and runtime
  PostgreSQL provider failures, including commit/resolve paths.
- All producer calls are best effort and catch sink panics before they can
  unwind migration, recovery, commit, or verification code. Events contain no
  DSN, SQL, SQLSTATE, Room/entity ID, canonical bytes, payload, capability, or
  secret.

## Evidence

- `cargo clippy --locked --no-deps -p worldstream-postgres --all-targets -- -D warnings` — pass.
- `cargo fmt -p worldstream-postgres -- --check` — pass.
- PostgreSQL library tests — 26/26 pass.
- PostgreSQL adapter tests — 13/13 pass.
- PostgreSQL telemetry tests — 4 pass, 1 opt-in live test ignored by the
  normal suite.
- Disposable PostgreSQL 17.11 live telemetry test — pass. It performed real
  migration and repeat-current calls and observed the expected terminal facts;
  the exact temporary container was removed afterward.
- The telemetry test binaries were rerun directly after the shared workspace
  manifest became temporarily out of sync with `Cargo.lock`; no lockfile was
  edited.

## Parent integration and verification

- Added `PostgresTelemetryBridge` in `worldstream-server`. It maps the closed
  storage vocabulary into `TelemetryDraftV1` with `adapter=postgres`, no entity
  correlation, and only the allowlisted numeric `schema_version` attribute.
- The bridge submits through the existing bounded `try_send` queue. A
  10,000-event saturation test proves queue pressure is observed as drops and
  does not block the storage-facing sink.
- Exact bridge tests cover migration, recovery, integrity, and storage
  diagnostics and verify JSON output contains no Room, canonical, endpoint,
  DSN, or SQL data.
- Parent reran all `worldstream-server` targets (72 library and 7 daemon tests),
  all `worldstream-postgres` targets (27 library, 13 commit, and 4 normal
  telemetry tests), strict all-target Clippy, and scoped formatting.
- Parent ran the ignored live telemetry test against a disposable,
  digest-pinned PostgreSQL 17.11 container; real migration and repeat-current
  terminal facts passed and the container was removed.
- The HTTPS OTLP smoke passed trusted CA/hostname validation, untrusted and
  hostname-mismatch rejection, bounded HTTP failures/slow response, daemon
  exporter selection, and malformed endpoint rejection.

The storage crate remains independent of the exporter implementation; only the
server depends on both adapter vocabularies. No provider outcome, Room truth,
hash, readiness decision, or commit result depends on telemetry delivery.
