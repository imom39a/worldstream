# IMO-58 telemetry audit — Luna Tesla

Date: 2026-08-21

## Scope

Audited `crates/worldstream-server/src/telemetry.rs` and every current
producer call site in `crates/worldstream-server/src/lib.rs` against IMO-58's
five acceptance criteria. No timer seam, Heist reducer, protocol, SDK, UI,
packaging, or Linear files were changed.

## Implemented and verified in this lane

- `TelemetryEventV1` has a versioned schema, closed event families, typed
  details, closed reason codes, optional W3C `traceparent`, and bounded numeric
  attributes.
- Redaction is deterministic: only seven allowlisted numeric dimensions
  survive; duplicate keys take the minimum; unknown, textual, oversized, and
  sensitive values are discarded; entity IDs are not metric labels.
- JSON ingress/export paths re-check the bounded shape and 4 KiB event limit.
- Prometheus exposition now includes enqueue/drop/overflow, exporter success
  and failure classes, slow exports, queue depth, and event-family counters.
  Labels remain the closed event-family vocabulary.
- The exporter queue uses nonblocking bounded `try_send`, bounded batches,
  panic/outage isolation, slow-export accounting, rate-limited overflow
  diagnostics, and a shutdown cap of three seconds.
- Endpoint validation rejects credentials, query/fragment secrets, malformed
  authorities, unsupported schemes, and oversized values. The OTLP-like
  transport is injected through a vendor-neutral trait; no SDK or credential is
  required.
- Added focused tests for the complete event/reason wire vocabulary, bounded
  dimensions, duplicate determinism, and all Prometheus counters.

## Acceptance audit and fail-closed disposition

| Criterion | Evidence | Disposition |
| --- | --- | --- |
| Stable lifecycle fields on both adapters | Typed families exist. Current producers cover HTTP admission/commit, timer-fire route admission/commit, WebSocket hello/action, and frame delivery. No producer is wired from SQLite/PostgreSQL storage, recovery, Activation, lease, Runner, migration, or timer scan/retry paths. | **Fail closed** for full criterion. |
| Privacy and bounded dimensions | Module redaction/shape tests and JSON/OTLP batch tests pass; no payload/projection/clue/commitment/capability/bearer/secret field is representable in typed details. | **Pass for the module; integration logging still needs live evidence.** |
| Outage/saturation/slow collector never changes truth/readiness | Runtime unit tests and existing HTTP exporter-failure test cover queue/export isolation. The route wiring submits after backend results. No SQLite/PostgreSQL commit-failure or process-kill integration evidence exists. | **Pass for tested seam; fail closed for full adapter criterion.** |
| Room-local vs process-global readiness | Pure `classify_readiness*` tests distinguish an unhealthy Room from schema/storage/writer/scheduler/recovery failures. The actual `/readyz` path uses its own runtime facts and does not expose telemetry health. | **Pass for the helper; live Room/readiness matrix remains unproven.** |
| Prometheus/JSON/W3C/OTLP without vendor dependency | Serialization, Prometheus text, strict traceparent, endpoint, injected transport, outage, panic, and bounded shutdown tests pass. There is no `/metrics` route, incoming traceparent propagation at the HTTP/WebSocket boundary, real collector/TLS transport, or daemon-owned telemetry runtime acceptance run in this lane. | **Fail closed for end-to-end criterion.** |

## Explicit missing integration seams

`GatewayBackend` is the only supported producer seam currently visible to this
lane. Storage backends do not accept a telemetry handle, and the server's
Runner/Activation/lease, recovery, migration, and scheduler paths do not call
the telemetry module. Adding calls would require edits outside this lane's
authorized files and risks placing telemetry before the authoritative commit
boundary. Those claims are therefore not fabricated.

Current producer coverage also does not establish W3C correlation from request
headers: gateway helper events use `CorrelationV1::none()`. The module can
parse and serialize a valid traceparent, but that is not propagation evidence.

## Verification

Commands run after the changes:

```text
cargo test --locked -p worldstream-server --lib telemetry::tests  # 15 passed
cargo test --locked -p worldstream-server --lib                  # 42 passed
cargo fmt --check -p worldstream-server                            # passed
cargo clippy --locked -p worldstream-server --lib --no-deps -- -D warnings  # passed
git diff --check                                                   # passed for tracked files
```

The requested package-wide clippy command was also attempted:

```text
cargo clippy --locked -p worldstream-server --all-targets -- -D warnings
```

It is fail-closed by four pre-existing lints in the dirty
`crates/worldstream-sqlite/src/lib.rs` timer-seam work (`doc_markdown` and
three `missing_errors_doc` diagnostics). This lane did not modify that file;
the server-only no-dependency clippy check passed.

This report intentionally does not claim IMO-58 completion or change Linear
status. Full acceptance remains blocked on producer wiring/evidence for both
storage adapters, lifecycle/recovery/lease/Runner paths, actual trace
propagation, and an exposed/runtime-verified metrics and collector path.
