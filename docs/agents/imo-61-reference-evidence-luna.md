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
observation, the exact target, `target_met`, and `outcome`. Stored, loaded,
idle, rate, latency, soak, and forced-termination dimensions must complete.
Snapshot-tail recovery is the sole permitted incomplete dimension, and only
for the exact frozen Counter v2 semantic ceiling described below.

The sustained-rate proof uses exactly 1,800 contiguous, half-open, one-second
buckets from a monotonic start boundary. It retains only aggregate counts and
nearest-rank percentiles, not Room IDs, transition IDs, capabilities, payloads,
or latency samples. Dispatch, queue-full, action-attempt, unconsumed-token,
late-acceptance, and in-window acceptance counts must satisfy the producer's
exact arithmetic, including the fixed 512-token queue bound.

## Honest 100,000-transition target miss

The frozen `worldstream.counter` v2 pack accepts at most 16 `increment` and 16
`private_ack` participant Actions. The runner genuinely submits those 32
Actions and then the 33rd Action. The expected public result is
`counter_limit_reached`, so the 100,000-transition snapshot/recovery target is
not reachable with the frozen pack.

The raw report binds the exact Counter pack version, digest, configuration,
accepted Action sequence, terminal rejection code, and
`not_reachable_due_to_frozen_pack_semantics`. It reports an attempted but
incomplete non-SLA target miss; it must not invent a benchmark pack, fabricate
snapshot/recovery observations, or convert the product limitation into a
correctness failure. The producer rejects any other incomplete history result
and does not accept a fabricated completed-history substitute for this frozen
pack.

## Package and source binding

The target workload starts the daemon from the verified Linux native archive
and exercises its public API. It must also import the Python SDK extracted
from that same verified archive. The report binds the exact digest and size of
the packaged SDK's `pyproject.toml`, `uv.lock`, `worldstream_sdk/__init__.py`,
`worldstream_sdk/client.py`, and compatibility identity. Runtime module paths
must equal the two exact source paths beneath the supplied packaged SDK root;
a checkout copy or an installed copy under `.venv/site-packages` is rejected.

The runner also binds the exact archive, package report, daemon, manifests,
packaged acceptance, one-hour soak, and kill-point report bytes. Inputs are
strict, bounded JSON or bounded regular files; duplicate keys, non-finite
numbers, symlinks, oversized inputs, and digest substitutions fail closed.

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
  --kill-point-report "$kill_point_report"
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
