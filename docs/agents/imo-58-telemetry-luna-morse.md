# IMO-58 server telemetry integration — Luna Morse

Date: 2026-08-21

## Delivered

This lane completes the smallest truthful production integration in the
allowed server write set (`worldstream-server/src/lib.rs` and
`telemetry.rs`):

- Added `GET /metrics`. It renders the existing bounded telemetry runtime
  counters as Prometheus exposition text with a fixed content type. The route
  is read-only, does not probe the exporter, and returns zero-valued metrics
  when telemetry is not configured. Exporter outage or shutdown state cannot
  change the route result or readiness.
- Added strict ingress parsing for the W3C `traceparent` header. Valid values
  are carried only as typed `CorrelationV1`; malformed, non-UTF-8, or absent
  values are ignored. No authorization, payload, Room, capability, or private
  Heist data is copied into telemetry.
- Propagated that correlation through HTTP handlers and both WebSocket
  upgrade/stream paths. Events are submitted only after the backend result or
  frame delivery result is known.
- Added post-result typed producers for timer fire, Runner handshake, pending
  Activation offer reads, Activation claims, and Activation lease
  renew/release/complete operations. The producer uses the existing closed
  phase/reason vocabularies and emits no entity identifiers.
- Kept all dimensions behind the existing allowlist and numeric bounds. The
  metrics endpoint exposes only fixed metric names and the closed event-family
  label set.

Timer phase mapping is deliberately conservative: successful backend timer
fire is `fired`; retryable/busy/storage-indeterminate results are `retried`;
other closed failures are `obsolete`. Activation offer/claim/lease outcomes
use the existing typed phases and backend reason mapping; this does not infer
Room truth.

## Verification

- `cargo test -p worldstream-server --lib --locked --no-fail-fast` — 44 passed.
- `cargo clippy -p worldstream-server --lib --tests --locked -- -D warnings` —
  passed.
- `rustfmt --edition 2024 --check crates/worldstream-server/src/lib.rs
  crates/worldstream-server/src/telemetry.rs` — passed.
- `git diff --check` — required as the final diff gate.

Focused coverage includes the metrics route while the exporter fails, valid
HTTP traceparent propagation, typed timer/activation lifecycle events, bounded
event serialization, and absence of Room/capability/authorization fields.

## Remaining fail-closed boundary

SQLite/PostgreSQL producer parity, migration/recovery/storage diagnostics,
durable lease-reclaim telemetry, `/metrics` deployment exposure, live
collector scrape evidence, and incoming trace propagation from storage or
adapter-owned execution are not implemented here. They require edits to the
SQLite/PostgreSQL/runtime/packaging or deployment layers, which were expressly
outside this lane's write set. No acceptance claim is made for those paths;
they remain fail-closed and must be completed by a separate adapter/runtime
lane with the same bounded event contract.
