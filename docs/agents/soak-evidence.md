# SQLite soak and failure evidence

`scripts/soak-smoke.sh` is a bounded local Cargo-loop preflight. It executes
the repository's existing critical matrix exactly as the gate does:

```text
cargo test -p worldstream-sqlite --locked
```

The default invocation runs three complete matrix iterations and has a five
minute total wall-clock bound. It is intentionally short enough for local
verification. An explicit one-hour diagnostic run is available with:

```text
scripts/soak-smoke.sh --one-hour --output soak-evidence.json
```

That command is not the authoritative IMO-61 release soak because it does not
run a packaged daemon workload or observe daemon-owned queues and files. The
release input is produced on Linux x86-64 by the exact packaged artifact:

```text
scripts/daemon-transition-soak.sh --one-hour \
  --daemon-bin "$EXTRACTED/bin/worldstreamd" \
  --package-archive "$DIST/worldstream-0.1.0-linux-x86_64.tar.gz" \
  --package-report "$DIST/worldstream-0.1.0-linux-x86_64.report.json" \
  --packaged-acceptance-report "$REPORTS/packaged-backend-acceptance.json" \
  --output "$REPORTS/daemon-transition-soak.json"
```

Only the strict failure/soak producer may promote that completed package-bound
report into detached release evidence.

Every cargo command has a hard timeout (120 seconds by default), output is
drained with a bounded in-memory cap, and timed-out process groups receive
TERM followed by KILL after the bounded grace period. `--max-total-seconds`
may lower the bound for a smoke check, but no invocation may set it above one
hour. In one-hour mode, the harness does not start a new Cargo command inside
the final bounded command window; it waits out that tail so the hard deadline
cannot turn an otherwise complete soak into a false failed iteration. The
harness emits one redacted JSON object on stdout and can duplicate it to
`--output`; command output, absolute paths, database paths, and credentials are
not emitted.

The one-hour mode is only complete when its full 3,600-second window is
actually reached. Lowering `--max-total-seconds` is useful for exercising the
boundary in a test, but that run exits incomplete and cannot be used as
one-hour evidence.

## What is measured

For each complete matrix iteration the report records exit status, timeout
status, bounded output accounting, duration, and (when available) a Linux
`/proc` RSS peak for the cargo process. It reports p50, p95, and p99 command
durations using deterministic nearest-rank percentiles (`ceil(p*N)`). These are
reference measurements, not an SLA. RSS is an observation of the cargo parent
process and is not a daemon leak proof; on non-Linux systems the fallback is a
cumulative child `ru_maxrss` observation with that limitation stated in JSON.

An optional `--database PATH` measures the main SQLite file plus `-wal` and
`-shm` sidecars before and after the run, reports byte growth, and fails closed
if growth exceeds `--max-database-growth-bytes` (256 MiB by default). Without
that option, the harness reports `database.status=not_configured`; the unit
matrix creates its own temporary databases and does not expose a stable
daemon database path to this command.

The authoritative packaged-daemon run additionally samples `/metrics` during
the live transition workload. It requires fixed-label, entity-ID-free
observations for the telemetry exporter (256 events globally), telemetry DNS
resolver (two queued requests globally), Room Admission Lane (256 positions
per Room), WebSocket live push queue (256 frames per connection), and
WebSocket outbound payload budget (4 MiB per connection). For every class the
report retains configured scope and capacity, maximum sampled process current,
process and per-unit high-water, successful activity and completion deltas,
and the initial/final/delta backpressure counter. Global high-water and every
per-unit high-water must remain within their applicable hard limit. Activity
and completion must increase during the workload; a zero backpressure delta is
a valid measured normal-load outcome, while missing or negative counters and
partial queue inventories fail closed.

The same authoritative command also runs five exact, source-bound saturation
regressions against the production queue paths: telemetry exporter enqueue,
DNS resolver submission, Room admission reservation, WebSocket frame enqueue,
and WebSocket payload-byte reservation. The aggregate has a 300-second bound;
each captured output is capped at 256 KiB and retained by SHA-256. The strict
producer requires the closed test list, exact Cargo package and test name,
single-test execution result, duration/output bounds, and an exact copy of the
boundary-test object under standard measurements. These hard-gate tests prove
the configured boundary behavior separately from the normal one-hour workload,
where a truthful zero backpressure delta remains valid.

Before running the matrix, the harness runs the bounded test-list command and
checks semantic coverage for create, action, timer, snapshot, recovery,
resource/storage fault, fuzz/property, corruption/quarantine, contention,
authority, delivery, and migration names. Each matrix iteration also compares
Cargo's reported passed-test count with the preflight list count and fails
closed on a gap. The current repository exposes the deterministic
`tests::property_based_replay_fuzz_probe` hook. The harness requires that
semantic fuzz/property group explicitly; if it disappears, a real invocation
stops at preflight with a `fuzz` gap rather than treating ordinary fixtures as
fuzz evidence. This is still a gap detector for the named local fixtures, not
proof that every Linear acceptance criterion is implemented.

## Fixture-only failure hooks

If exposed by the current SQLite test list, the harness runs one exact test
for each of these categories and marks it `evidence_class=fixture_only`:

- resource/storage failure (`actual_read_only_driver_error`, writer locking,
  or failpoint tests);
- fault/recovery (`faulted_replay`, snapshot failure, or unavailable-runtime
  tests); and
- corruption/quarantine (corrupt snapshot, recovery quarantine, replay
  verification, or missing-integrity tests).

Missing resource, fault, or corruption hooks fail closed. A test whose name
contains a kill/power-loss marker is also discovered if one is ever added, but
it remains fixture-only and cannot become process-level evidence merely from
its name. The current repository exposes no process-kill or power-loss command,
so the JSON report explicitly contains:

```json
{
  "kill_points": {
    "status": "not_exposed",
    "process_kill_claim": false,
    "power_loss_claim": false
  }
}
```

The in-process failpoint, corruption, unknown-commit, restart, and snapshot
tests are valuable local evidence, but by themselves they do not prove a
daemon survived SIGKILL, a host crash, disk-full, filesystem corruption, or
physical power loss. No such success is claimed by this fixture harness.

## Release disk-full scenario

The Linux x86-64 release lane runs a separate, bounded process scenario from
`scripts/daemon-transition-soak.py`. A privileged, network-isolated container
uses the exact image
`docker@sha256:12e683a161823b2a839aeea999b9d960e6e1f9a97b1679ad6b441982e2d9cf07`,
a read-only root filesystem, and a read-only mount of the packaged daemon. It
creates a fixed 64 MiB loop-backed ext4 filesystem with 4096-byte blocks,
pre-creates the SQLite database as a regular mode-0600 file, and fills the
filesystem until it reports zero available KiB. A fixed 4096-byte write to
that database must then return `ENOSPC`, write zero bytes, and leave the file
size unchanged.

The exact packaged daemon is started against that full ext4 mount. It must
exit with code 1 before readiness, expose no public mutation endpoint, and
emit the bounded durable-store initialization failure diagnostic. The daemon
deadline is 10 seconds with at most 64 KiB of retained diagnostic output; the
entire container is bounded to 120 seconds and 256 KiB of captured output.
The container script explicitly unmounts the loop filesystem, while the host
always requests container removal and proves the named container is absent.

The process-soak report binds that witness to the daemon binary SHA-256. The
strict `scripts/release-evidence-produce-failure-soak.py` producer rejects a
missing witness or any mismatch in the pinned image and isolation, filesystem
and regular-file facts, attempted write and `ENOSPC` result, daemon identity,
exit/readiness/log bounds, or cleanup proof. Only after that validation is the
exact witness copied into the release failure matrix. This scenario proves
fail-closed startup/bootstrap behavior under a real full ext4 filesystem; it
does not claim runtime recovery after free space returns, arbitrary
physical-volume exhaustion, filesystem corruption, or physical power loss.

## Boundary tests and exact limits

The Python boundary suite uses a temporary fake `cargo` only to test the
launcher contract; it is not product evidence:

```text
python3 tests/soak_smoke.py
bash -n scripts/soak-smoke.sh
```

The suites cover machine-readable output, redaction, optional database
measurement, missing-fixture fail-closed behavior, timeout reporting, the
short default boundary, and exact fail-closed disk-full producer validation.
The focused environment test rejects missing, duplicate, extra, malformed, or
non-ASCII container witness markers; the release lane executes the pinned
ext4 container scenario itself. For historical context only, a local Cargo
loop on 2026-08-20 completed one hour with 507 iterations and 77/77 validation
per iteration; its JSON artifact had SHA-256
`46f1c6e24fa3c035a2c976855e30ed423f8511d712cbb660a6722c830306a950`.
That historical run is not current release proof: it used no packaged daemon
or configured database path and did not measure real database/WAL growth or
the five runtime queue classes. Process-level kill/restart is documented in
`imo-60-61-kill-points.md`.
