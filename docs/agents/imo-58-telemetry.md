# IMO-58 vendor-neutral telemetry

The server telemetry seam is implemented in
`crates/worldstream-server/src/telemetry.rs`. `OperatorState` can install a
bounded `TelemetryHandle`; the HTTP create route and WebSocket hello/action/
frame paths submit only after their backend result or delivery outcome is
known. Storage adapters, Activation, and `/readyz` remain outside this
optional producer integration. A caller owns the `TelemetryRuntime`, and
export failure never changes an authoritative route result.

## Contract

- `TelemetryEventV1`, `EventDetailsV1`, `ReasonCodeV1`, and `CorrelationV1`
  cover admission, commit outcomes, timers, frame delivery, Activation,
  recovery, migrations, and storage diagnostics.
- `TelemetryEventV1::redact` accepts only typed details and allowlisted bounded
  numeric attributes. It drops payloads, projections, private clues,
  commitments, capabilities, secrets, unknown fields, and oversized values.
  BTreeMap ordering and duplicate-minimization make the transformation
  deterministic. JSON deserialization rejects unknown fields, and the public
  submit/export paths re-check the bounded shape so callers cannot bypass
  redaction. No business/entity identifier is a metric label.
- `TelemetryEventV1::json_line` emits bounded structured JSON. `TelemetryMetrics`
  emits Prometheus text with fixed event-family labels only.
- `TraceParentV1` strictly parses and serializes W3C `traceparent`, rejecting
  zero identifiers, uppercase/whitespace, invalid versions, and malformed
  trace flags. It can create a child context from a caller-provided span ID.
- `TelemetryRuntime` uses a standard-library bounded sync channel and
  `try_send`; queue capacity is bounded at 65,536 events and batches at 128
  events. Queue overflow drops the event, increments counters, and emits only
  a rate-limited overflow warning; redaction rejection and shutdown/disconnect
  drops are counted without being mislabeled as overflow. Export failures,
  including exporter panics, are counted and discarded. Slow exports are
  counted, and shutdown waits no longer than the requested duration, capped at
  three seconds.
- `OtlpLikeExporter` is an optional bounded JSON batch seam over the
  `OtlpTransport` trait. Endpoint validation and transport behavior are
  testable without an HTTP client, network dependency, or vendor SDK.
- `classify_readiness` treats an unhealthy Room as local and schema/storage/
  writer/scheduler failures as process-level. Exporter health is not an input.

## Focused evidence

The module tests cover deterministic redaction/privacy, submit-path validation,
traceparent parsing and propagation, Prometheus text, readiness scope,
malformed endpoints, bounded OTLP batches, exporter outage/panic isolation,
bounded queue overflow, slow collectors, and bounded shutdown.

The server integration test
`tests::http_room_admission_and_commit_are_post_result_telemetry` dispatches a
real `POST /v1/rooms` request through `operator_router` with
`OperatorState::with_telemetry`. A successful backend result produces exactly
two exported events in order: accepted admission, then committed outcome. The
test also verifies two queue enqueues, zero drops, the 4 KiB encoded-event
bound, redaction of the request id and bearer prefix, and a bounded shutdown
flush.

```text
cargo fmt -p worldstream-server
cargo test -p worldstream-server --lib
cargo clippy -p worldstream-server --all-targets -- -D warnings
```

Parent-verifiable result: the focused integration test passed; the complete
server library suite passed 31 tests with 0 failures; formatting and clippy
passed with `-D warnings`.

The optional producer wiring is not yet proven in a live daemon-owned runtime,
a real collector, or against a live PostgreSQL/SQLite failure path. Those
integrations remain follow-up evidence. In particular, the tests do not prove
cross-process queue behavior, a real HTTP/TLS collector,
collector retry/acknowledgement semantics, process-kill durability, or
storage-adapter commit telemetry. Readiness classification remains a pure
helper; exporter health is not wired into `/readyz` and cannot make the process
appear ready.
