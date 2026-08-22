# IMO-60/61 process kill-point evidence

> Historical implementation note: this page describes the original
> single-SQLite-cell diagnostic and is not current release proof. The
> authoritative release procedure is
> [`imo-60-61-process-runtime-evidence.md`](imo-60-61-process-runtime-evidence.md),
> which requires the packaged 36-cell SQLite/PostgreSQL direct/PostgreSQL
> transaction-pool matrix and the strict detached producer.

`scripts/kill-point-smoke.sh` is the bounded Linux process harness for
the process-level crash/kill-point gap. It is intentionally separate from
`/readyz`, the in-process SQLite test matrix, and the repeated soak
runner.

## What a real pass proves

With an executable `worldstreamd`, the harness creates a fresh 32-byte
owner-only bootstrap secret and supplies its path through
`WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE`. The daemon durably installs
the fixed host-operator capability from that secret; the harness derives the
corresponding `wsb1:` bearer only in memory and places it in a private curl
config file. It then:

1. creates a private owner-only temporary data directory;
2. starts the real daemon and observes only `/healthz` for liveness;
3. submits a valid Counter v2 `POST /v1/rooms` through the daemon's HTTP
   route using the manifest-backed revision digest;
4. records the canonical SHA-256 of the successful machine-readable Room
   response;
5. sends `SIGKILL` at the explicit `after_http_2xx_response`
   boundary;
6. restarts the same daemon against the same data directory; and
7. retries the same idempotency key and requires the exact canonical response
   hash to match.

The retry is the durable outcome comparison: it must resolve the persisted
semantic result rather than perform a new Room creation. A successful run
emits `status: "passed"`, `evidence_class: "process_level"`, and
`kill_points.process_kill_claim: true` as one machine-readable JSON
object.

The harness has bounded startup, request, and cleanup timeouts (each 1–60
seconds), never retains unbounded response or log data, redacts bearer values
and temporary paths from diagnostics, and validates that its generated root
and data directory are owner-only non-symlink directories. It never accepts an
external data directory.

## Current live disposition

Parent verification first found and then fixed a runtime replay defect: a
restart sampled a newer bootstrap preparation time, and the durable receipt
replay incorrectly rejected its older (but valid) commit time. The Core
receipt constructor now validates preparation time on a fresh install while
allowing a later idempotent replay to reconstruct the stored receipt.

After that fix, the real daemon was built and the harness was run in the
pinned `rust:1.97.1-bookworm` Linux container. It returned exit code `0` with
the following verified result:

```json
{"status":"passed","evidence_class":"process_level","operation":{"path":"POST /v1/rooms","http_status":200,"response_code":"committed"},"kill_points":{"status":"covered","process_kill_claim":true,"power_loss_claim":false},"restart":{"status":"passed","same_data_directory":true},"comparison":{"status":"passed","equal":true}}
```

The canonical response SHA-256 was identical before and after SIGKILL. This
is process-level kill/restart evidence; it is not a physical power-loss or
one-hour soak claim.

The fixed source-level cause was authority receipt replay validation, not a
reported SQLite corruption or file-integrity failure. The regression is
covered by the later-time replay test in the SQLite suite.

If a future run rejects the secure bootstrap file, cannot install the host
capability, the live route returns `forbidden`/another bootstrap error, or the
same-data-directory restart cannot replay the secure bootstrap after SIGKILL,
the command emits a blocked report with:

- `status: "blocked"`
- `evidence_class: "missing_prerequisite"` (or `runtime_prerequisite` after
  the SIGKILL boundary)
- `operation.path: "POST /v1/rooms"`
- `kill_points.process_kill_claim: false`
- `kill_points.power_loss_claim: false`

A post-kill replay failure specifically reports `kill_points.status:
"incomplete"` and `restart.status: "failed"`. A successful first Room
response alone never promotes the run.

This remains an honest missing-prerequisite report when a required runtime
prerequisite is absent; it is not process-level evidence.
The harness does not call `/readyz`, does not use a hardcoded bearer, does not
treat a fixture daemon as a successful kill-point run, and does not claim
power loss or release readiness. A successful real run is the only condition
under which durable Room survival is reported.

## Verification

```sh
bash -n scripts/kill-point-smoke.sh
python3 -m unittest tests/kill_point_smoke.py -v
```

For a real Linux run, build the daemon first and capture the JSON output:

```sh
cargo build --locked -p worldstream-server --bin worldstreamd
bash scripts/kill-point-smoke.sh >kill-point-evidence.json
```

The Python tests use a fixture daemon only to verify launcher boundaries,
bootstrap-file wiring, timeout behavior, redaction, path safety, and the
no-readiness substitution rule; they are not evidence of process-level
durability. A live result must be retained separately and must show
`evidence_class: "process_level"` before it can support IMO-60/61.
