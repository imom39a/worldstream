# IMO-58 telemetry acceptance audit

## Scope

This continuation audited only `crates/worldstream-server/src/telemetry.rs`,
`crates/worldstream-runtime/src/config.rs`, telemetry scripts/tests, and lane
documentation. Existing dirty worktree changes were preserved.

## Current evidence

| Requirement | Evidence | Result |
| --- | --- | --- |
| Versioned event/reason/correlation families | `TelemetryEventV1`, closed enums, typed details, and post-result producers | Pass locally |
| Redaction and bounded cardinality | Numeric allowlist, 16 attributes, 4 KiB event bound, no entity labels, config endpoint redaction | Pass locally |
| W3C correlation | Strict `traceparent` parser, child propagation, producer tests | Pass locally |
| Queue/exporter isolation | Nonblocking `try_send`, fixed queue limits, drop/rate-limited warning, panic/outage/slow-export tests | Pass locally |
| Readiness independence | Readiness classifier excludes telemetry; daemon `/readyz` remains independent of exporter health | Pass locally |
| Optional vendor-neutral OTLP seam | Runtime endpoint validation plus `OtlpTransport`, bounded OTLP/HTTP JSON logs, standard-library HTTP transport | Pass locally |
| Real collector delivery | `scripts/telemetry-collector-smoke.sh` ran a real collector container and verified a received `worldstream/telemetry/v1` record | Verified for optional transport |
| Daemon endpoint wiring | `worldstreamd` still constructs `StructuredLogTelemetryExporter` directly; it does not consume `config.telemetry` | Unresolved, outside this edit scope |

The collector witness used:

```text
image: otel/opentelemetry-collector-contrib:latest
digest: sha256:1f2c54a30e713fac6b3ae77a1ec84010c2007e29ced8ec666214fc2f6739c1cc
collector_delivery: verified
release_evidence: false
```

The script uses a disposable local Docker container, an explicit OTLP/HTTP
logs pipeline, and the standard-library `StdHttpOtlpTransport`. It records the
image digest but does not promote this disposable run to release evidence.

## Exact focused results

```text
cargo test --locked -p worldstream-server --lib telemetry -- --nocapture
17 passed, 0 failed

cargo test --locked -p worldstream-runtime --lib config -- --nocapture
19 passed, 0 failed

cargo clippy --locked -p worldstream-server --lib --tests -- -D warnings
passed

cargo clippy --locked -p worldstream-runtime --lib --tests -- -D warnings
passed

bash -n scripts/telemetry-failure-smoke.sh
passed

scripts/telemetry-collector-smoke.sh
passed; real collector delivery verified
```

The existing process failure witness remains deliberately truthful: its
`release_evidence` is `false`, it proves committed Room/hash and readiness
survive exporter failure pressure, and it reports daemon OTLP delivery as not
claimed. The collector witness is a separate, stronger transport-level proof.

## Unresolved requirements

- `worldstreamd` needs an out-of-scope integration change to select an OTLP
  exporter from `EffectiveConfig.telemetry`; this audit does not claim that
  configuring `WORLDSTREAM__TELEMETRY__OTLP__ENDPOINT` changes daemon delivery.
- HTTPS collector delivery requires an injected TLS-aware transport; the
  standard-library transport intentionally supports plain HTTP only.
- Hosted collector availability, retry/ack behavior under a real network
  outage, cross-platform collector evidence, and release-grade observability
  artifacts remain external requirements.
- No telemetry change may make `release_ready` true or substitute for the
  provider-native, cross-platform, signing, SBOM, and provenance evidence
  required by the broader WorldStream project.
