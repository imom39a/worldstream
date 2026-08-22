# Linux durability and kill-point continuation — IMO-48/50/58/60/61

Date: 2026-08-21
Lane: Luna Linux-container durability, restart, kill-point, recovery, telemetry, and bounded soak evidence
Checkout: `/Users/vinothshanmugam/code/agent-streamer`

## Outcome

This continuation ran the strongest safe local Linux evidence available from
the Apple arm64 host. The disposable container was native Linux `aarch64`,
not Linux x86_64. No physical-power-loss, native Linux x86_64, Windows, or
release-readiness claim is made.

The real daemon passed two repeated process-level SIGKILL/restart Room
outcome comparisons. The SQLite matrix, telemetry tests, clippy checks, real
TERM/restart smoke, recovery/corruption/resource fixtures, and a three-
iteration bounded soak also passed.

One confirmed in-scope harness defect was fixed: `scripts/restart-smoke.sh`
provided a valid bootstrap secret but expected `/readyz` to remain
`503 storage_not_initialized`. The real daemon correctly returned `200
{"status":"ready"}`. The harness now asserts readiness after valid
bootstrap while retaining the invalid-storage fail-closed and synthetic
bearer rejection checks. The fixture regression was updated accordingly.

No SQLite product defect or telemetry defect was found. No manifest, release
gate, server route, PostgreSQL/transfer/backup file, or Linear state was
changed by this lane.

## Container identity and exact commands

The container was disposable and selected explicitly as Linux arm64:

```text
docker run --rm --platform linux/arm64 rust:1.97.1-bookworm rustc --version
rustc 1.97.1 (8bab26f4f 2026-07-14)
Linux ... aarch64 GNU/Linux
rust@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97
```

The source was mounted read/write only so Cargo could compile the current
dirty checkout; the build target was kept inside the disposable container:

```text
docker run --rm --platform linux/arm64 \
  -v "$PWD":/workspace -w /workspace rust:1.97.1-bookworm bash -c '
    set -Eeuo pipefail
    export CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0
    CARGO_TARGET_DIR=/tmp/worldstream-target
    cargo fmt --all -- --check
    cargo test --locked -p worldstream-sqlite --lib
    cargo test --locked -p worldstream-server --lib telemetry -- --nocapture
    cargo clippy --locked -p worldstream-sqlite --lib --tests -- -D warnings
    cargo clippy --locked -p worldstream-server --lib --tests -- -D warnings
    cargo build --locked -p worldstream-server --bin worldstreamd
  '
```

Results: SQLite `90 passed`; server telemetry `16 passed`; formatting,
SQLite clippy, server clippy, and daemon build passed.

## Retained process-level kill-point evidence

Exact command used for the real daemon boundary:

```text
WORLDSTREAMD_BIN=/tmp/worldstream-target/debug/worldstreamd \
  bash scripts/kill-point-smoke.sh \
    --startup-timeout-seconds 20 \
    --request-timeout-seconds 20 \
    --shutdown-timeout-seconds 20
```

The command was run twice after a fresh build in the same disposable arm64
Linux container. Both results were redacted machine-readable reports with
`status: "passed"`, `evidence_class: "process_level"`, HTTP `200` and
`response_code: "committed"`, `kill_points.process_kill_claim: true`,
`restart.same_data_directory: true`, and equal canonical JSON SHA-256 values:

```text
run 1: 253fbe46302873446d9a58413e45dbac1d16e89a2911a149f605547bb94e75f5
run 2: 388b7d4f407d8a5207bbb17518492b68e241f58794d71091e925658e47e356fc
```

The independent first run in a separate disposable container also passed:

```text
a8faa320b6463c64c6cf4fb4619a8a02926a85d79150894efc9fbaf3dffca060
```

The differing hashes are expected because each run creates a fresh Room and
temporary authority material. Within each run, the pre-kill and post-restart
hashes were identical. The harness reported no temporary paths or bearer
values and removed its temporary data on exit.

This proves survival of an observable HTTP commit across process SIGKILL and
same-data-directory restart. It does not prove physical power loss, storage
device failure, or a release artifact.

## Retained restart, recovery, telemetry, and soak results

Real daemon restart command:

```text
WORLDSTREAMD_BIN=/tmp/worldstream-target/debug/worldstreamd \
  bash scripts/restart-smoke.sh
```

Result:

```text
WorldStream restart smoke passed: health/version/readiness observed,
SQLite file survived TERM/restart, invalid storage did not serve.
The valid owner-only bootstrap secret makes this daemon ready; the synthetic
bearer remains forbidden and no Room success is claimed by this smoke.
```

Bounded Linux soak command:

```text
bash scripts/soak-smoke.sh \
  --iterations 3 \
  --command-timeout-seconds 120 \
  --max-total-seconds 180
```

The retained redacted report ended with `status: "pass"`,
`evidence_class: "bounded_fixture_only"`, `platform.machine: "aarch64"`,
`one_hour_window_completed: false`, and `release_evidence: false`.
All three matrix runs passed all 90 SQLite tests (`270` test executions).
Elapsed time was `45.65 s`; matrix durations were `9948.773 ms`,
`7909.789 ms`, and `9814.900 ms`. The report measured cargo-process peak
RSS values of `45,584,384`, `47,632,384`, and `49,442,816` bytes, with an
observed run-to-run delta of `3,858,432` bytes. No database path was
configured, so no database-growth claim is made.

The selected resource, replay-fault, and corruption hooks each passed once:

```text
tests::actual_read_only_driver_error_rolls_back_and_the_identical_plan_retries
tests::faulted_replay_rejects_and_quarantines_an_impossible_same_generation_append
tests::corrupt_paired_snapshot_is_disposable_and_lineage_rebuild_is_exact
```

The Linux SQLite test-list preflight covered create, action, timer, snapshot,
recovery, resource/storage fault, fuzz/property, corruption/quarantine,
concurrency, authority, delivery, and migration groups. The soak runner
correctly reported kill points as `not_exposed`; the separate real daemon
harness is the process-level evidence.

Focused local harness verification after the fix:

```text
python3 -m unittest tests.kill_point_smoke tests.restart_smoke tests.soak_smoke -v
17 passed
bash -n scripts/kill-point-smoke.sh scripts/restart-smoke.sh scripts/soak-smoke.sh
git diff --check
```

These Python tests exercise boundary fixtures and redaction/fail-closed
behavior; they are not substituted for the real process-level result above.

## Files changed in this lane

```text
scripts/restart-smoke.sh
tests/restart_smoke.py
docs/agents/imo-durability-killpoint-continuation-luna.md
```

The SQLite and telemetry source paths were verified but not modified by this
continuation.

## Remaining blockers and evidence boundaries

- Native Linux x86_64 and Windows compatibility evidence remain unrun.
- Physical power-loss, filesystem/device fault, and provider-native
  snapshot/PITR/restore evidence remain unproven.
- The bounded run was 45.65 seconds; the required one-hour soak was not run.
- The property-based test is the repository's deterministic local fuzz probe,
  not an extended native mutation campaign.
- The process boundary was tested after the observable HTTP response. No
  claim is made for every possible commit-internal kill point.
- No release manifest, checksum, signing, SBOM, provenance, or OCI release
  evidence was generated.

These boundaries remain fail-closed and should not be converted into completed
IMO-48/50/58/60/61 release acceptance from this arm64-container run.

## IMO-57/60/61 harness audit follow-up

The kill-point and soak evidence paths were audited again after the original
lane report. The kill-point report now records the observable commit boundary,
the post-restart idempotency retry independently (`HTTP 200`, `committed`),
retry/hash equality, observed process exit, and the named manifest evidence
cell `failure-fuzz-resource-and-one-hour-sqlite-soak`. These fields are
diagnostic handoff metadata only; `release_evidence` remains `false` and the
manifest remains unresolved.

The kill-point launcher now emits machine-readable `status: "blocked"`,
`evidence_class: "unsupported_platform"` output on non-Linux hosts. On this
Darwin arm64 host the exact invocation exited `2`, with process-kill and
power-loss claims both false. The soak launcher no longer crashes on platforms
without Python's Unix-only `resource` module; memory is explicitly reported as
`not_measured` when no portable provider exists. Its in-process matrix remains
fixture-only and never promotes process-kill or power-loss evidence.

Focused verification after these changes:

```text
python3 -m unittest -v tests.kill_point_smoke tests.soak_smoke
15 passed
bash -n scripts/kill-point-smoke.sh scripts/soak-smoke.sh
python3 -m py_compile tests/kill_point_smoke.py tests/soak_smoke.py
```

The real Darwin soak completed one SQLite matrix run: `90/90` tests passed in
`14.893 s`, with `fixture_only: true`, `process_kill_claim: false`,
`power_loss_claim: false`, and `one_hour_window_completed: false`.

The available disposable Linux arm64 container rebuilt `worldstreamd` and
ran the real process boundary successfully. Exact machine result:
`status=passed`, `HTTP 200` initial commit, `SIGKILL` process exit observed,
same-data-directory restart healthy, retry `HTTP 200`/`committed`, and equal
canonical hashes:

```text
530ce3041918921fcf4d44761b5ffca133a1f9ad14a7b7ae41b90278824c09e2
```

That result is process-level Linux arm64 evidence for the observable HTTP
commit boundary only. It does not prove physical power loss, Linux x86_64,
Windows, or release evidence. No manifest or gate status was promoted.

The manifest linkage was checked directly: the named row is still
`release_gate=true`, `status=unresolved`, and has an empty artifact digest.
The repository's `tests/gate_smoke.py` currently has one unrelated expectation
failure: it expects the simulated Windows filesystem outcome `FAIL`, while
the current gate implementation returns the more conservative
`SKIP_INCOMPLETE` for unavailable native Windows ACL evidence. That gate/test
pair was outside this lane's permitted edit set and was left unchanged.
