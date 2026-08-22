# IMO-58 telemetry failure lane

Date: 2026-08-21
Lane: Luna process-level telemetry failure witness
Repository: `/Users/vinothshanmugam/code/agent-streamer`

## Scope

This lane adds `scripts/telemetry-failure-smoke.sh` and its boundary tests in
`tests/telemetry_failure_smoke.py`. It does not change Rust, storage,
packaging, UI, or compatibility-manifest files.

The runner starts a disposable real `worldstreamd` with an owner-only random
bootstrap secret, supplies an unreachable standard OTEL endpoint, creates a
real Counter Room, retries the identical create request, samples readiness and
Prometheus metrics, and drives 384 bounded public HTTP requests carrying a
synthetic invalid bearer. It checks that the committed Room response and
authoritative hash are unchanged by the retry, readiness stays `ready`, and
authorization/marker values do not appear in logs or metrics.

## OTLP boundary

`worldstreamd` currently constructs `StructuredLogTelemetryExporter` with the
default `TelemetryConfig`; it has no public runtime OTLP endpoint field or
collector transport. The witness therefore supplies:

- `OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:1/v1/traces`, an unreachable
  standard OTEL environment value; and
- an attempted `WORLDSTREAM__TELEMETRY__OTLP__ENDPOINT` value using a malformed
  endpoint, which is rejected before the daemon serves.

The first value is not interpreted as collector-delivery evidence. The second
proves the configuration boundary fails closed. The run explicitly reports
`runtime_otlp_hook=not_exposed_by_worldstreamd` and
`collector_delivery=not_claimed`.

## Parent-reproducible commands

```text
cargo build --locked -p worldstream-server --bin worldstreamd
bash scripts/telemetry-failure-smoke.sh
python3 -m unittest -v tests.telemetry_failure_smoke
```

The smoke runner emits one compact JSON evidence object followed by a human
summary. Its temporary root, process, and bootstrap secret are removed during
cleanup. It never prints the secret or bearer.

## Evidence classification

This is real disposable process/HTTP evidence for SQLite daemon startup,
post-commit telemetry submission, Prometheus availability, readiness
independence, idempotent Room creation/hash stability, bounded request
pressure, and log redaction. Queue saturation is observed but is not required
as a success condition because the fixed queue and structured-log exporter are
not public test hooks. No collector receipt, OTLP retry/acknowledgement,
one-hour saturation, native Linux/amd64, Windows, or release evidence is
claimed.

The compatibility manifest remains `release_ready=false`.
