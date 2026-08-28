# Observability and diagnostics

Observability is post-result and bounded. Export failure, logging, or metrics
must never decide canonical truth or change a successful/failed operation.

## Core signals

| Signal | Question answered |
| --- | --- |
| `/healthz` | is the process serving? |
| `/readyz` | are selected storage, authority, writer, scheduler, and startup recovery ready? |
| `/version` | which compatibility/build/storage identity is running? |
| `/metrics` | what bounded runtime/transport/storage counters and gauges are exposed? |
| Studio daemon status | can the local Supervisor establish a typed live snapshot? |
| Room detail/integrity | is this exact Room healthy/faulted/quarantined and fresh? |
| Runner attention | is the exact approved instance live, fresh, at capacity, or holding/backlogging work? |

## Logging

Start local daemon logging with:

```sh
RUST_LOG=info target/debug/worldstreamd --config config/development.toml
```

Logs use structured closed events/reasons. They must not include bearer bytes,
secret paths, request payloads, participant-private Projections, provider
prompts/responses, high-cardinality IDs as metric labels, or raw database errors
that may embed provider details.

## Tracing and telemetry

The server accepts/parses W3C `traceparent` at transport boundaries and can
export bounded OTLP-like HTTP batches to a validated endpoint. Export queues
are nonblocking and bounded; outages are counted and shutdown is time-limited.

Telemetry distinguishes adapter identity and stable reason/event vocabulary.
It does not make storage-neutral behavior depend on one collector.

## Freshness

Treat freshness as its own axis. A PID may exist while a Runner is no longer
making MCP progress. The managed host advances freshness on completed outbound
MCP messages, not merely process existence. Studio preserves authoritative
`observed_at` values instead of replacing them with browser polling time.

## Diagnostic capture

When reporting a problem, capture:

- source/build revision and `/version` response;
- storage profile and redacted effective config;
- health/readiness closed reason;
- relevant stable operation/assignment/Room identifier only when safe;
- bounded structured event/reason codes;
- exact reproduction command with placeholders for secrets;
- whether state survived restart and whether identical retry reconciled.

Never attach `.worldstream` state, bearer/launch references, raw provider
payloads, or private Room evidence to an issue.

Source: [architecture observability](https://github.com/imom39a/worldstream/blob/main/docs/architecture.md#observability-and-operations)
and [security logging rules](https://github.com/imom39a/worldstream/blob/main/docs/security.md#logging-and-telemetry).
