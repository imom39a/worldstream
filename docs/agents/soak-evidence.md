# SQLite soak and failure evidence

`scripts/soak-smoke.sh` is the bounded local evidence harness for the IMO-60
failure/resource boundary and the IMO-61 performance reference. It executes
the repository's existing critical matrix exactly as the gate does:

```text
cargo test -p worldstream-sqlite --locked
```

The default invocation runs three complete matrix iterations and has a five
minute total wall-clock bound. It is intentionally short enough for local
verification. An explicit one-hour run is available with:

```text
scripts/soak-smoke.sh --one-hour --output soak-evidence.json
```

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
tests are valuable local evidence, but they do not prove a daemon survived
SIGKILL, a host crash, disk-full, filesystem corruption, or physical power
loss. No such success is claimed by this harness.

## Boundary tests and exact limits

The Python boundary suite uses a temporary fake `cargo` only to test the
launcher contract; it is not product evidence:

```text
python3 tests/soak_smoke.py
bash -n scripts/soak-smoke.sh
```

The suite covers machine-readable output, redaction, optional database
measurement, missing-fixture fail-closed behavior, timeout reporting, and the
short default boundary. A parent run on 2026-08-20 completed the one-hour
mode with 507 iterations and 77/77 validation per iteration; its full JSON
artifact had SHA-256
`46f1c6e24fa3c035a2c976855e30ed423f8511d712cbb660a6722c830306a950`.
That run used no configured database path, and therefore did not measure real
database/WAL growth. Process-kill/power-loss behavior and cross-process daemon
behavior remain separate evidence lanes; process-level kill/restart is
documented in `imo-60-61-kill-points.md`.
