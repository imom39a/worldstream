# IMO-58 HTTPS OTLP/HTTP transport slice

## Scope

This slice adds real HTTPS OTLP/HTTP JSON delivery to `worldstreamd` while
keeping the existing vendor-neutral `TelemetryExporter` and bounded queue
contract. Plain HTTP remains available for disposable local collectors.

## Implementation

- `worldstreamd` now selects `TlsHttpOtlpTransport` for a validated `https://`
  endpoint instead of refusing the scheme or silently falling back.
- `StdHttpOtlpTransport` remains the explicit plain-HTTP transport.
- Both transports use bounded DNS resolution, a 500 ms aggregate connect
  budget, 2 second read/write socket deadlines, a bounded request body, and an
  8 KiB response bound. Only HTTP 2xx responses are successful.
- HTTPS uses `rustls` certificate-chain and hostname verification by default;
  there is no insecure verifier or verification bypass. Production trust roots
  are the Mozilla WebPKI bundle from `webpki-roots`.
- Endpoint parsing rejects unsupported schemes, malformed authorities,
  userinfo/credentials, query strings, fragments, whitespace/control bytes,
  invalid ports, and oversized endpoints. The existing typed telemetry
  redaction continues to exclude payloads, entity IDs, bearer material, DSNs,
  and other non-allowlisted values from the JSON body.
- Export failures remain diagnostic-only. The existing nonblocking bounded queue
  and `catch_unwind` worker boundary are unchanged, so delivery cannot affect
  commit truth, Room health, hashes, or readiness.

## Dependency choice and security defaults

- `rustls = 0.23.35`, exact-pinned, with `default-features = false` and
  `ring`, `std`, and `tls12` features. This uses rustls safe protocol defaults
  (TLS 1.2 and TLS 1.3) and the Ring crypto provider without a vendor SDK.
- `webpki-roots = 1.0.3`, exact-pinned, supplies the Mozilla trust-anchor
  bundle. No system trust bypass, custom verifier, plaintext fallback for an
  HTTPS endpoint, client credentials, or endpoint secret headers are exposed.
- The TLS tests use a short-lived test CA only through a private test helper;
  the production constructor always uses the Mozilla bundle.

## Evidence

The focused TLS tests generate a short-lived CA and `localhost` certificate,
run a real rustls TLS collector, and verify:

- successful trusted handshake and hostname verification;
- `POST /v1/logs`, `Content-Type: application/json`, bounded body, and valid
  OTLP JSON shape;
- 2xx success handling;
- rejection of an untrusted certificate and a hostname mismatch;
- non-2xx and oversized response rejection;
- slow-response bounded return;
- endpoint credential/query/fragment rejection and plain HTTP preservation.

The collector parses headers and `Content-Length` and reads exactly the
bounded body before replying. It has read/write deadlines, so handshake and
negative-path tests cannot wait on EOF or hang.

## Commands/results

Run from the repository root:

```text
timeout 30s cargo test --locked -p worldstream-server --lib tls_transport -- --nocapture
2 passed; 0 failed

cargo test --locked -p worldstream-server --lib
70 passed; 0 failed

cargo test --locked -p worldstream-server --all-targets
70 library tests and 7 daemon tests passed; 0 failed (worldstreamctl had 0 tests)

cargo clippy --locked --no-deps -p worldstream-server --all-targets -- -D warnings
passed

rustfmt --edition 2024 --check crates/worldstream-server/src/telemetry.rs crates/worldstream-server/src/bin/worldstreamd.rs
passed

scripts/telemetry_https_smoke.sh
passed
```

The final smoke script reruns the TLS, plain HTTP, daemon selection, and
runtime endpoint-validation tests under a 30-second external timeout.

The focused runtime endpoint suite ran separately with:

```text
cargo test --locked -p worldstream-runtime telemetry_endpoint -- --nocapture
3 passed; 0 failed
```

The broader pre-existing `cargo test --locked -p worldstream-runtime` run is
currently red in unrelated manifest tests:
`resolved_counter_and_unresolved_release_pack_entries_are_exposed`,
`release_ready_manifest_rejects_unresolved_or_no_selectable_release_activity`,
and `unresolved_entries_reject_release_manifests_or_partial_digests`. No
runtime files were changed by this slice; the focused telemetry runtime tests
and the smoke script pass.
