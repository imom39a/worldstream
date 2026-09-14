# IMO-232 qualification checklist

This is the current audit of Linear IMO-232, “Qualify 100k-plus Room histories
with replaceable Runners and a 72-hour soak.” The issue and its comments were
reviewed on 2026-09-14. The comments record evidence from commits
`a2a02777`, `680c38d9`, `576e2b86`, and `fc3b23b8`; the current bounded snapshot
instrumentation is in `ea5d10a3` and the follow-up documentation commits after
it. Local reports are engineering evidence and keep `release_evidence=false`.

The status terms below mean:

- **Satisfied**: evidence exists for the complete criterion at the stated
  scope.
- **Runnable**: the implementation or command exists, but the required run or
  a stable dependency is still missing.
- **Partial**: some rows are evidenced, but the criterion still has a material
  uncovered dimension.
- **Open work**: code, harness, or a required production run is still needed.

## Acceptance checklist

| # | Linear acceptance criterion | Status | Evidence and exact remaining work |
|---:|---|---|---|
| 1 | Reproducible artifact, commit, Pack, backend, and Linux reference resources; PostgreSQL measured separately | **Partial** | [`docs/local-postgresql-long-history-evidence.md`](local-postgresql-long-history-evidence.md) records pinned PostgreSQL 17.11/PgBouncer identities and the Fly 4-vCPU/8-GiB SQLite run from the issue comments. The reports identify production SQLite/PostgreSQL paths, but the final current commit, exact Pack digest, Linux image, and all hardware/filesystem facts are not bound into one final manifest. Re-run the final lanes after IMO-234 stabilizes and publish that manifest. |
| 2 | Read/claim/Action p50/p95/p99, useful-contribution latency, stale rejection, recovery rows/reducer calls, RSS/allocations, bytes per Transition, DB/WAL/temp growth, oldest pending work | **Partial** | Deterministic [`scripts/room-history-qualification.py`](../scripts/room-history-qualification.py) reports bounded counters and modeled latency; the issue comments record SQLite recovery/RSS/DB/WAL at 1k/10k/100k, and IMO-220 records 1k warm-claim percentiles. Real p50/p95/p99 for all read/claim/Action paths, allocation data, bytes per Transition, temp growth, and oldest pending work at every required scale still need a production Runner harness. |
| 3 | Warm operations avoid history-prefix scans; checkpoint recovery reports actual prefix/tail work and honestly meets the <=5-second target | **Partial** | [`docs/evidence/bounded-recovery`](evidence/bounded-recovery) proves exact checkpoint recovery at SQLite and PostgreSQL 1k/10k/100k cuts with zero prefix delivery and measured recovery under five seconds. IMO-220’s 1k warm-claim evidence exists. Warm claim/Action scale parity at 10k/100k and a final current-commit measurement remain open. The current SQLite 10k rerun records cadence writes but reports `no_eligible_checkpoint` when the bounded operational-witness limit is exceeded; that dependency belongs to IMO-234. |
| 4 | Complete the 72-hour soak with termination, Runner replacement, offline periods, slow consumers, credential renewal, and timer catch-up | **Moved to IMO-235; intentionally deferred** | No 72-hour wall-clock run was performed. The harness correctly leaves `seventy_two_hour_soak` skipped and cannot promote a short diagnostic run. Linear IMO-235 now owns the exact artifact/image/backend identity, start/end timestamps, termination schedule, offline intervals, slow-consumer profile, credential-renewal proof, timer-catch-up proof, memory/queue/WAL/temp time series, and final report. |
| 5 | Lost replies resolve original identities; acknowledged work is durable; privacy, obligations, and bounds hold | **Partial** | Existing SQLite/PostgreSQL commit and recovery tests cover idempotent identities, duplicate resolution, exact Head fences, corruption/fallback, and durable receipts. The live PostgreSQL evidence also covers direct and pooled duplicate resolution. Production replaceable-Runner loss/reconnect, offline obligations, unauthorized-read probes, and slow-consumer queue bounds are not covered by the long-history fixture and need a dedicated scenario lane. |
| 6 | Restore/transfer above 100k total records and 64 MiB preserves exact canonical and operational state | **Partial** | The transfer evidence records 100,001 canonical transitions, 300,020 total stream records, exact digests, resumability, corruption/disk-full rejection, PostgreSQL hydration, source retirement, and final authority. The transfer report also exposes a 1.54-GiB peak RSS and the IMO-225 malformed-stream/resource work remains open. Repeat the dedicated portability contract at the final current commit and resolve its resource and malformed-stream rows before treating this bullet as complete. |
| 7 | Compare recent-history, summary+retrieval, and current-Projection+explicit-work Runner contexts with identical model/budget conditions | **Satisfied at bounded evaluation scope** | [`docs/evidence/runner-context-evaluation`](evidence/runner-context-evaluation) records three isolated `gpt-5.6-luna` invocations with the same rule brief, output schema, four held-out Pack-compatible cases, and a 24,576-byte prompt ceiling. Recent history produced 2/4 useful Actions and missed two obligations; summary plus authorized retrieval and current Projection plus explicit work each produced 4/4, with zero obsolete claims, unsupported completions, or evidence errors. The artifact remains `release_evidence=false` and keeps policy quality separate from runtime integrity. |
| 8 | Keep failures and unavailable evidence visible; never claim release readiness or 100,000 correct model decisions from deterministic evidence | **Satisfied at harness scope** | The qualification harness is fail-closed: modeled values carry `source: deterministic_model`, unavailable providers are skipped/failed, the 72-hour row remains visible, and `release_evidence=false` is preserved in local reports. This does not close the other bullets; it verifies that incomplete evidence is represented honestly. |

## What can run now

The deterministic matrix is runnable without a provider:

```sh
uv run --python 3.14.7 --no-project python \
  scripts/room-history-qualification.py --compact \
  --output /tmp/imo-232-deterministic.json
```

Production SQLite 1k and 10k fixture commands are runnable and report cadence,
storage, and recovery separately. The current 10k output is useful cadence
evidence even when checkpoint qualification is false. The SQLite unit failure
matrix is currently blocked by IMO-234’s in-progress migration contract update
(the source has 20 migrations while a test expectation still lists 19).

The standalone PostgreSQL cadence test is compiled and ready:

```sh
WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN="$ADMIN_DSN" \
WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN="$RUNTIME_DSN" \
cargo test --locked -p worldstream-postgres --features conformance-tracer \
  --test postgres_commit live_postgres_snapshot_cadence_direct -- --nocapture
```

It must use a fresh pinned `postgres:17.11-alpine` provider. A first attempt
against such a container was stopped by the schema-catalog fingerprint gate
while IMO-234 migration work was between implementation and contract update;
the run did not produce PostgreSQL cadence evidence.

## Separate soak issue

Linear IMO-235, **“Run the 72-hour long-history Runner soak,”** owns criterion
4 and is related to IMO-232. It requires the immutable workload and identity
manifest, periodic process termination and replacement, long offline intervals,
slow consumers, credential renewal, timer catch-up, queue/memory/WAL/temp
measurements, and a final report that proves no acknowledged work or open
obligation disappeared. Its completion cannot be inferred from the short SQLite
diagnostic soak.

IMO-232 can then close once criteria 1–3 and 5–8 have final-current-commit
evidence. If criteria 2 or 6 remain incomplete, keep IMO-232 open; splitting the
duration experiment does not waive those independent acceptance rows.
