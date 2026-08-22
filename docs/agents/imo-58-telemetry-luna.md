# IMO-58 vendor-neutral telemetry — Luna implementation lane

Date: 2026-08-21
Repository: `/Users/vinothshanmugam/code/agent-streamer`
Release evidence: **false**

## Implemented

- Preserved the versioned `worldstream/telemetry/v1` event contract and closed
  event/reason vocabularies for admission, commit outcomes, timers, frame
  delivery, Activation, recovery, migration, and storage diagnostics.
- Added optional closed `sqlite`/`postgres` adapter attribution to the
  telemetry envelope. It is telemetry-only and cannot affect Room truth,
  projections, commitments, or hashes; the contract explicitly permits no
  attribution at generic gateway boundaries.
- Kept all payloads, projections, clues, commitments, capabilities, bearer
  material, secrets, and entity IDs out of typed details. Only seven bounded
  numeric attributes survive deterministic redaction, with fixed event-family
  Prometheus labels and a 4 KiB event bound.
- Preserved nonblocking bounded `try_send` submission, bounded batches,
  bounded shutdown, panic/outage/slow-export isolation, queue-drop accounting,
  rate-limited overflow diagnostics, and read-only Prometheus metrics.
- Wired the daemon to the optional plain HTTP OTLP transport while retaining
  structured logs as the fallback. HTTPS is still rejected by the daemon
  because no approved TLS transport is present in this dependency-neutral
  lane.
- Fixed malformed telemetry config handling. Invalid endpoint text is no
  longer retained or raised as a process-fatal runtime parse error; config
  stores only `invalid_endpoint`, the daemon emits a typed redacted diagnostic,
  and selects bounded structured logs. Other configuration failures remain
  fail-closed.
- Extended the real-process failure smoke to exercise an unreachable OTLP
  endpoint, malformed-config fallback, bounded HTTP pressure, readiness, and
  authorized current Projection equality before/after telemetry failure.
- Added a storage-owned `SQLite` producer seam with closed, redacted facts for
  migration start/apply/current/failure, recovery start/complete/failure,
  integrity fault/quarantine, and connection/lock/query/integrity diagnostics.
  The server bridge attributes these events to `sqlite` and only submits them
  through the existing bounded nonblocking queue.
- Centralized every SQLite sink call behind panic isolation. A panicking sink
  cannot unwind migration, recovery, metadata initialization, or storage
  correctness paths; the panic-safe test reopens the same metadata and proves
  the idempotent truth is unchanged.

## Criterion evidence

| Criterion | Evidence | Result |
| --- | --- | --- |
| Stable versioned lifecycle fields and both adapter identities | `TelemetryEventV1`, closed typed details, strict `traceparent`, adapter wire test for `sqlite` and `postgres`, existing post-result server producers, and real SQLite migration/recovery producer test | Pass for the vendor-neutral contract and SQLite producer slice; PostgreSQL producer wiring remains outside this scope |
| Privacy and bounded cardinality | Deterministic allowlist tests, endpoint redaction tests, fixed Prometheus labels, OTLP body redaction, no secret/entity fields in the real process witness | Pass |
| Exporter outage/saturation/slow collector cannot alter truth or readiness | Nonblocking queue/runtime tests; real unreachable collector process run; unchanged create response/head/hash and Projection; malformed config fallback; SQLite truth-invariance test with a failing exporter and panic-isolated sink; real collector receipt | Pass for bounded runtime, SQLite producer, and process evidence. TLS/hosted collector availability is not claimed |
| Room-local vs global readiness | `classify_readiness*` tests preserve Room-unhealthy as local and promote schema/storage/recovery failures; telemetry health is not a readiness gate | Pass for the classifier contract; no release claim for deployment-specific Room supervisor evidence |
| Prometheus, structured JSON, W3C trace propagation, optional OTLP without vendor SDK/credentials | `/metrics` route test, structured exporter, trace propagation route test, injected transport, fixed-label SQLite migration metric in the failure smoke, real Docker OpenTelemetry Collector smoke | Pass for this lane; HTTPS transport and hosted credentials are intentionally not claimed |

## Exact verification

```text
cargo test --locked -p worldstream-runtime --lib config --quiet
23 passed, 0 failed, 15 filtered out

cargo test --locked -p worldstream-server --lib --quiet
64 passed, 0 failed

cargo test --locked -p worldstream-server --bin worldstreamd --quiet
7 passed, 0 failed

cargo clippy --locked -p worldstream-runtime --all-targets -- -D warnings
passed

cargo clippy --locked -p worldstream-server --all-targets -- -D warnings
passed

bash -n scripts/telemetry-failure-smoke.sh
passed

python3 -m unittest -v tests.telemetry_failure_smoke
4 passed, 0 failed

rustfmt --edition 2024 --check \
  crates/worldstream-sqlite/src/lib.rs \
  crates/worldstream-server/src/telemetry.rs \
  crates/worldstream-server/src/bin/worldstreamd.rs
passed

cargo clippy --locked --no-deps -p worldstream-sqlite --all-targets -- -D warnings
passed

cargo clippy --locked --no-deps -p worldstream-server --all-targets -- -D warnings
passed

cargo test --locked -p worldstream-sqlite \
  sqlite_real_migration_and_recovery_producers_are_bounded_and_truth_independent
1 passed, 0 failed

cargo test --locked -p worldstream-server \
  sqlite_bridge_preserves_adapter_and_exact_bounded_storage_facts
1 passed, 0 failed

cargo test --locked -p worldstream-server \
  sqlite_storage_stays_truthful_when_exporter_fails
1 passed, 0 failed

cargo test --locked -p worldstream-sqlite \
  sqlite_panicking_sink_cannot_change_migration_recovery_or_metadata_truth
1 passed, 0 failed

cargo test --locked -p worldstream-sqlite --lib
98 passed, 0 failed

bash scripts/telemetry-failure-smoke.sh
passed; real disposable daemon, 191 exporter failures, unchanged Head/hash/Projection, readiness ready

bash scripts/telemetry-collector-smoke.sh
passed; real collector receipt verified with collector image digest
`otel/opentelemetry-collector-contrib@sha256:1f2c54a30e713fac6b3ae77a1ec84010c2007e29ced8ec666214fc2f6739c1cc`;
release_evidence=false
```

After the shared backup crate compiled again, the parent orchestrator reran
the complete SQLite and server library suites, both owned-crate strict Clippy
checks, the Python boundary tests, and the real failure smoke. All passed; no
backup file was changed by this lane.

## Explicit unresolved boundaries

- PostgreSQL producer call sites and provider-native attribution remain outside
  this exclusive SQLite/server scope and are not claimed here.
- The standard-library transport supports plain HTTP only. HTTPS requires an
  approved TLS implementation; no credential-bearing collector configuration
  is accepted.
- Disposable collector and daemon runs are validation evidence only and do
  not make the compatibility manifest release-ready.
