# IMO-61 Linux reference target and evidence

The authoritative reference-target path is a real packaged-daemon workload,
not a fixture benchmark or a checkout-SDK smoke test. The workload runner is
`scripts/reference-target-workload.py`; its raw report is projected with the
five existing source reports into six normalized inputs, aggregated, and then
validated again by the typed release-evidence producer.

Reference performance targets are non-SLA and non-blocking. An honestly
measured target miss remains publishable. Missing, unattempted, scaled,
simulated, malformed, duplicate, or substituted facts fail closed, as do any
correctness, acknowledged-durability, receipt, or identity failures.

## Frozen workload

The release runner accepts only `worldstream/reference-target-profile/v1` with
the following exact dimensions:

- 10,000 stored/passivated Rooms;
- 100 simultaneously loaded Rooms;
- 1,000 mostly-idle real WebSocket Sessions;
- at least 100 accepted transitions in every one-second monotonic bucket for
  1,800 seconds;
- nearest-rank local commit-to-ack p50/p95/p99, with p95 below 100 ms as the
  target result;
- the exact separately produced 3,600-second bounded-soak report;
- a genuine attempt at a 100,000-transition Room, a newest valid snapshot no
  more than 250 transitions behind, and packaged recovery in at most 5,000 ms;
  and
- the exact separately produced 36-cell forced-termination report with no
  acknowledged loss.

Every dimension records `attempted`, `completed`, its bounded aggregate
observation, the exact target, `target_met`, and `outcome`. Every dimension
must complete. Performance targets remain non-SLA: a completed snapshot-tail
measurement may publish an honestly computed duration miss.

The sustained-rate proof uses exactly 1,800 contiguous, half-open, one-second
buckets from a monotonic start boundary. It retains only aggregate counts and
nearest-rank percentiles, not Room IDs, transition IDs, capabilities, payloads,
or latency samples. Dispatch, queue-full, action-attempt, unconsumed-token,
late-acceptance, and in-window acceptance counts must satisfy the producer's
exact arithmetic, including the fixed 512-token queue bound.

## Exact 100,000-transition recovery boundary

Setup uses a dedicated one-Room SQLite database. A source-bound fixture binary
alternates authorized Membership suspend/resume administration operations
through the production Core authorization and commit APIs until the Room has
exactly 100,000 Transitions. It then removes only the disposable current
materialization and the newest two snapshots, leaving the newest valid paired
snapshot at sequence 99,998. This setup duration is recorded but excluded from
the recovery stopwatch.

The measured boundary starts a fresh process from the exact extracted release
`worldstreamd` and ends only after a current participant Projection has been
returned and verified. The before/after complete Head and Projection hashes
must be equal. The report binds the fixture source and executable bytes, its
strict bounded setup report, the package source revision, and the exact daemon
binary. Missing setup, fewer than 100,000 accepted Transitions, a substituted
revision, an empty tail, or hash drift fails closed; only a completed recovery
duration above 5,000 ms becomes a publishable non-SLA target miss.

## Package and source binding

The target workload starts the daemon from the verified Linux native archive
and exercises its public API. It must also import the Python SDK extracted
from that same verified archive. The report binds the exact digest and size of
the packaged SDK's `pyproject.toml`, `uv.lock`, `worldstream_sdk/__init__.py`,
`worldstream_sdk/client.py`, and compatibility identity. Runtime module paths
must equal the two exact source paths beneath the supplied packaged SDK root;
a checkout copy or an installed copy under `.venv/site-packages` is rejected.

The runner also binds the exact archive, package report, daemon, snapshot-tail
fixture binary and source, manifests, packaged acceptance, one-hour soak, and
kill-point report bytes. The fixture's compiled revision must equal the source
revision in the checksum-bound archived build identity. Inputs are strict,
bounded JSON or bounded regular files; duplicate keys, non-finite numbers,
symlinks, oversized inputs, and digest substitutions fail closed.

Packaged acceptance, one-hour soak, kill-point evidence, and target workload
may be produced by separate jobs and separate hosts. The target report retains
each external report's own environment under `bound_external_sources` with
`source_attribution: bound_external_source`. Equality of certified environment
fields is not evidence that two jobs ran on one host. Only target workload
facts use the target runner's `reference_environment`; acceptance keeps its
own workload/provider `storage_bindings` disclosure.

## Commands

After the verified native archive has been safely extracted, run the target
with the Python environment rooted at the archive's `sdk/python` directory:

```sh
uv run --python 3.14.7 --project "$packaged_sdk_root" --locked python \
  scripts/reference-target-workload.py \
  --output reports/reference-target-workload.json \
  --daemon-bin "$packaged_daemon" \
  --package-archive "$native_archive" \
  --package-report "$package_report" \
  --packaged-acceptance-report "$acceptance_report" \
  --packaged-sdk-root "$packaged_sdk_root" \
  --soak-report "$one_hour_soak_report" \
  --kill-point-report "$kill_point_report" \
  --snapshot-fixture-bin "$snapshot_fixture_bin"
```

Project the raw inputs and aggregate all six mandatory normalized reports:

```sh
uv run --python 3.14.7 --project sdk/python --locked python \
  scripts/reference-evidence-project.py \
  --packaged-acceptance-report "$acceptance_report" \
  --soak-report "$one_hour_soak_report" \
  --kill-point-report "$kill_point_report" \
  --target-report reports/reference-target-workload.json \
  --package-archive "$native_archive" \
  --package-report "$package_report" \
  --daemon-bin "$packaged_daemon" \
  --snapshot-fixture-bin "$snapshot_fixture_bin" \
  --output-dir reference-inputs/normalized \
  --aggregate-report reference-inputs/reference-summary.json
```

`reference-evidence.py` requires exactly one explicit `counter`, `heist`,
`sqlite`, `postgres`, `soak`, and `target` report. A legacy five-report direct
invocation cannot pass. The typed producer additionally requires the exact raw
soak, kill-point, and target reports and revalidates their byte bindings:

```sh
uv run --python 3.14.7 --project sdk/python --locked python \
  scripts/release-evidence-produce-reference.py \
  --output reference-performance-producer.json \
  --artifact-output reference-performance-artifact.json \
  --report reference-inputs/reference-summary.json \
  --counter-report reference-inputs/normalized/counter.json \
  --heist-report reference-inputs/normalized/heist.json \
  --sqlite-report reference-inputs/normalized/sqlite.json \
  --postgres-report reference-inputs/normalized/postgres.json \
  --soak-report reference-inputs/normalized/soak.json \
  --target-report reference-inputs/normalized/target.json \
  --package-archive "$native_archive" \
  --package-report "$package_report" \
  --daemon-bin "$packaged_daemon" \
  --snapshot-fixture-bin "$snapshot_fixture_bin" \
  --packaged-acceptance-report "$acceptance_report" \
  --raw-soak-report "$one_hour_soak_report" \
  --kill-point-report "$kill_point_report" \
  --raw-target-report reports/reference-target-workload.json
```

The hidden reduced profile exists only for bounded process tests and is
explicitly non-publishable. Its output cannot satisfy the frozen release
producer.

## Verification scope

The focused Python tests exercise schema, identity, source attribution,
SDK-path and byte binding, history-limit handling, monotonic bucket and queue
arithmetic, target-miss publication, mandatory sixth input, duplicate/tamper
rejection, process-tree RSS, premature idle-session failure, and cleanup.
Fixture tests are code coverage only; they are not execution evidence for the
30-minute workload, one-hour soak, 36-cell kill matrix, or frozen target
values. Only reports emitted by the package-bound release jobs are execution
evidence.
