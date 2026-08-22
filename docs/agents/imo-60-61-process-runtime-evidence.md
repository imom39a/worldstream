# IMO-60/61 Linux process runtime evidence

The release lane uses a freshly extracted Linux x86-64 archive daemon. It does
not use `target/debug`, a fixture daemon, readiness alone, or a source-built
binary as release evidence.

## Exact process-kill matrix

Run the twelve frozen cells with the extracted binary, its verified archive,
and the package report produced for those exact archive bytes:

```sh
scripts/kill-point-smoke.sh \
  --daemon-bin "$EXTRACTED/bin/worldstreamd" \
  --package-archive "$DIST/worldstream-0.1.0-linux-x86_64.tar.gz" \
  --package-report "$DIST/worldstream-0.1.0-linux-x86_64.report.json" \
  --output reports/kill-point-evidence.json \
  --log-output reports/kill-point.log
```

The report schema is `worldstream/kill-point-evidence/v1`. It contains exactly
these operations and boundaries:

- `room_create`, `action`, `timer`, and `activation_lease`;
- `before_commit`, `after_commit_before_publication`, and
  `after_publication_before_reply`.

The daemon writes and fsyncs an owner-only, create-new marker only when the
closed test protocol, exact cell, and exact operation identity match. The
harness validates that marker and sends `SIGKILL` externally. A later target
traverses earlier non-target seam calls normally. A missing marker, process
abort, graceful exit, wrong identity, timeout, unsafe path, or partial matrix
fails closed.

For Room creation, the harness reopens the production adapter for ordinary WAL
recovery, closes it, and then uses Python's offline read-only SQLite observer.
The observer hashes the main database and WAL before and after its queries,
records its own SQLite version, and requires zero Room/Genesis/Genesis-receipt
rows before retry at `before_commit`, versus exactly one at both post-commit
boundaries. This observer does not claim the product's bundled SQLite engine
identity or conformance.

Room creation has no pre-existing Room observer. Its post-commit publication
stage therefore consists only of admission/commit telemetry. Action and Timer
publication includes telemetry and live Room frames; Activation lease
publication includes Activation telemetry and no Room frame. Every marker
records that narrower publication contract explicitly.

## Process-level transition soak

A short diagnostic run exercises the same process workload without making a
one-hour claim:

```sh
scripts/daemon-transition-soak.sh \
  --daemon-bin "$EXTRACTED/bin/worldstreamd" \
  --duration-seconds 10 \
  --package-archive "$DIST/worldstream-0.1.0-linux-x86_64.tar.gz" \
  --package-report "$DIST/worldstream-0.1.0-linux-x86_64.report.json" \
  --output reports/daemon-transition-soak-short.json \
  --log-output reports/daemon-transition-soak-short.log
```

Only `--one-hour` fixes the workload target, `max_total_seconds`, and
`one_hour_target_seconds` at exactly 3600 seconds:

```sh
scripts/daemon-transition-soak.sh --one-hour \
  --daemon-bin "$EXTRACTED/bin/worldstreamd" \
  --package-archive "$DIST/worldstream-0.1.0-linux-x86_64.tar.gz" \
  --package-report "$DIST/worldstream-0.1.0-linux-x86_64.report.json" \
  --output reports/daemon-transition-soak.json \
  --log-output reports/daemon-transition-soak.log
```

The `worldstream/soak-evidence/v1` report is bound to
`worldstream-daemon-transition-soak/v1`. The public SDK creates Counter Rooms,
submits accepted transitions, and verifies two live spectator frames for every
transition. The report measures accepted load and fan-out, acknowledgement
p50/p95/p99, `worldstreamd` process-tree RSS, exact SQLite main/WAL/SHM growth,
log/temp growth, and same-data-directory SIGKILL recovery time and projection
hash equality. These are measured Linux reference values, not an SLA. Short
mode always remains diagnostic and `release_evidence` remains false; the
strict detached producer owns promotion.

The process report separates temporary-workspace bytes, daemon-log bytes, and
their exact auxiliary-artifact sum. Each category records initial, final,
growth, and its configured hard limit; the strict producer recomputes the
arithmetic and rejects a missing or widened limit. During the workload the
harness samples `worldstream_telemetry_queued` and the packaged daemon's
`worldstream_telemetry_queue_capacity` disclosure, retaining maximum observed
depth, sample count, drop-counter delta, and the exact configured capacity.

Before any disposable process directory is removed, the kill matrix and soak
scan every daemon log for the raw, hex, base64, and unpadded base64url forms of
real random authority and Capability sentinels. The packaged PostgreSQL lane
does the same for actual random authority/HostOperator Capability, admin and
runtime passwords, and complete admin/runtime DSNs across captured daemon
logs, every managed child stdout/stderr, value-free invocation configuration,
all six raw cell reports, and its aggregate report candidate. Owner-only
password/DSN source files are intentional secret inputs and are not retained
as scan channels. Reports retain only sentinel hashes/sizes and per-channel
hashes/sizes; any injected form fails closed.

Both reports bind the exact archive, extracted daemon, package report, legacy
JSON manifest digest, explicit JSON manifest digest, and explicit TOML
manifest digest. The soak report additionally exposes top-level `identity`,
`reference_workload`, and `measurements`. Its nearest-rank acknowledgement
latency includes sample count and p50/p95/p99; memory, main database, WAL,
temporary artifacts, and restart-to-ready recovery are measured directly.
`environment_observation` records the measured Linux host, hardware,
filesystem, and runtime-verified SQLite identity while stating explicitly that
PostgreSQL was not observed by this SQLite workload. The reference joiner may
construct the normalized common two-engine environment only after joining a
passing package-bound six-cell acceptance report; the raw soak never invents
that observation.

When the exact six-cell acceptance report is already available, pass
`--packaged-acceptance-report PATH`. Its raw SHA-256 is added to `identity` only
after the report's archive/manifest binding, six completed cells, parity, and
cleanup validate. An already-generated passing `soak-smoke.sh` report can be
provided with `--preflight-report PATH` on a runtime-only host; this replaces
only the supporting in-process fixture preflight, never the process workload.
Its exact package inventory is `worldstream-sqlite` plus
`worldstream-backup`: canonical mismatch and native restore proofs live in the
backup crate, so a SQLite-only test list cannot honestly cover them.

## Strict producer handoff

After both Linux commands succeed, produce the typed `failure-soak` input:

```sh
python scripts/release-evidence-produce-failure-soak.py \
  --output release-inputs/failure/producers/failure-soak.json \
  --artifact-output release-inputs/failure/artifacts/failure-soak.json \
  --soak-report reports/daemon-transition-soak.json \
  --kill-point-report reports/kill-point-evidence.json \
  --retained-log reports/kill-point.log \
  --retained-log reports/daemon-transition-soak.log \
  --manifest-toml compatibility.toml \
  --manifest-json compatibility.json
```

The manual Linux workflow must upload both raw reports and both retained logs
even when a harness fails. It may run the strict producer only after the exact
twelve-cell matrix and exact one-hour process workload have passed with the
same verified packaged distribution identity.

Focused contract tests:

```sh
uv run --project sdk/python --locked pytest -q tests/kill_point_smoke.py
cargo test --locked -p worldstream-server --lib \
  process_crash_boundary_requires_the_exact_four_part_opt_in
```
