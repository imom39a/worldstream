# IMO-220 / IMO-222 local closure evidence

This note records reproducible local evidence for warm Activation claims and
verified checkpoint recovery. These results are engineering and issue-closure
evidence; they are not hosted-provider or release qualification.

## Warm Activation claims (IMO-220)

The regression `repeated_warm_activation_claims_do_not_recover_or_read_history`
warms the Room executor, poisons an unread historical row, forbids recovery
fallback, and repeats an idempotent claim. It proves that the Activity reducer
callback count and canonical Transition-row count do not change. The measured
1,000-claim reference result was:

```text
warm_activation_claims scale=1000 history_rows=4 p50_us=9602 p95_us=11159 p99_us=15439
```

Reproduce or extend it with:

```text
WORLDSTREAM_WARM_CLAIM_SCALES=1000,10000,100000 \
cargo test --locked -p worldstream-server \
  repeated_warm_activation_claims_do_not_recover_or_read_history \
  --lib -- --nocapture
```

The 10,000- and 100,000-claim latency samples and requested PostgreSQL
contention matrix remain unmeasured. IMO-220 therefore remains open.

## Verified checkpoint recovery (IMO-222)

SQLite physical migration 17 and PostgreSQL physical migration 18 add a
canonical operational witness beside each disposable paired snapshot. The
witness binds the checkpoint Head, Timer ledger, retained observation Frames
and consequences, Membership generations and frame Heads, and Activation
decisions. Recovery verifies the snapshot and witness, replays at most 250
Transitions, and compares every recovered operational fact in the final exact
Head and integrity-generation transaction.

Missing, stale, malformed, hash-invalid, internally invalid, and out-of-window
checkpoints are cache misses. A hash-consistent canonical witness that disagrees
with live operational state triggers one full Genesis replay. It does not
quarantine the Room unless that authoritative replay also proves corruption.
Failure recording compares the exact expected Head, healthy status, and
integrity generation, so a stale recovery cannot quarantine a newer commit.

The public recovery API returns an execution receipt only after the final
install fence. That receipt distinguishes the completed `checkpoint` and
`full` paths and counts Transition records actually delivered to Core. The
evidence adapters separately count their checkpoint-boundary and tail reads.

### SQLite evidence

| Transitions | Recovery | RSS | Consequences | Witness | DB bytes |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 8 ms | 23,314,432 | 1,000 | 135,280 bytes | 10,625,024 |
| 10,000 | 79 ms | 127,107,072 | 10,000 | 1,354,783 bytes | 105,857,024 |
| 100,000 | 843 ms | 775,733,248 | 100,000 | 13,639,786 bytes | 1,075,195,904 |

All three runs completed on the checkpoint path with one boundary Transition
row read by the adapter, zero prefix range reads, zero prefix records delivered
to Core, zero tail records, one total adapter Transition read, and zero reducer
callbacks. The reports contain exact comparisons for every operational witness:

- [`sqlite-1000.json`](evidence/bounded-recovery/sqlite-1000.json)
- [`sqlite-10000.json`](evidence/bounded-recovery/sqlite-10000.json)
- [`sqlite-100000.json`](evidence/bounded-recovery/sqlite-100000.json)

### PostgreSQL evidence

| Transitions | Setup | Recovery | RSS | Prefix delivered | Adapter Transition reads | Reducer callbacks |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 1,000 | direct production commits | 192 ms | 26,787,840 | 0 | 1 | 0 |
| 10,000 | direct production commits | 887 ms | 59,097,088 | 0 | 1 | 0 |
| 100,000 | public v2 transfer + verified checkpoint rebuild | 1,185 ms | 1,419,444,224 | 0 | 1 | 0 |

The 100k setup used a fresh SQLite source, the public v2 streaming transfer,
and one explicit Genesis-to-Head verified replay to create a checkpoint for the
transferred Room. That one-time qualification/setup replay is not included in
the recovery measurement. The measured operation then called the ordinary
receipt-gated recovery path and completed as `checkpoint`.

The redacted
[`postgres-17-pgbouncer.json`](evidence/bounded-recovery/postgres-17-pgbouncer.json)
reports no errors and covers PostgreSQL 17.11 direct runtime, PgBouncer
transaction pooling, production gateway, shared conformance, a two-Transition
tail, malformed-witness fallback, canonical-witness full fallback without
quarantine, stale-Head failure fencing, missing-materialization rebuild, raw
malformed-Head quarantine, transfer parity, and cleanup. Every scale reports
exact Head, Core state, Activity state, checkpoint hash, and operational
witnesses. It also proves that bounded recovery did not read the 1,001, 10,001,
or 100,001 semantic receipt rows.

PostgreSQL permits an absent current Core/Activity materialization only during
authoritative full recovery. It derives the exact bytes from verified immutable
history, rechecks every operational projection under the locked healthy Head
and integrity generation, and inserts the missing row before committing that
fence. A present mismatch remains corruption. Fresh-container live regressions
also prove that malformed Head bytes quarantine at generation 2 through exact
raw-byte fences in both ordinary recovery and the explicit checkpoint-rebuild
maintenance API. A corrupt newest snapshot is treated as a cache miss: the
authoritative Genesis/Transition replay succeeds and the Room remains healthy
at its original integrity generation.

The scale report predates only that final cache-isolation follow-up. That
follow-up removes snapshot-cache validation and its comparison vector from the
one-time authoritative full-recovery setup; it does not alter the measured
checkpoint path or its receipt/read counters. Fresh-container tests cover the
final code, and the report's 100k RSS remains a conservative pre-optimization
measurement.

All six local 1k/10k/100k measurements completed under the five-second
reference target. This is a measured local result, not a universal SLA.

### Remaining resource limit

The Transition/reducer bound is proven, but total checkpoint bytes are not yet
history-independent. This workload retained one observation consequence per
Transition, so witness size grew from 135 KB to 13.64 MB. Capture and
verification refuse a witness larger than 16 MiB. ADR 0031 records the limit;
IMO-234 tracks compact or partitioned operational proofs independently of
canonical Transition history.

The 100k transfer also exposed row-at-a-time PostgreSQL staging and hydration
across transitions, consequences, receipts, and guards. That affects transfer
throughput and long transaction duration, not bounded recovery correctness.
The follow-up belongs with the open streaming-transfer work: use bounded
in-memory chunks, durable per-chunk digests and resume cursors, PostgreSQL
`COPY`, set-based validation, and a final publication fence.

### Reproduction and validation

```text
cargo run --locked --quiet --release -p worldstream-sqlite \
  --example history_qualification_fixture -- \
  --database "$DB" --transition-count 100000 --output "$REPORT" \
  --stream-metadata

WORLDSTREAM_PG_LIVE_KEEP_TEMP=1 \
WORLDSTREAM_PG_LIVE_EVIDENCE_FILE="$PG_REPORT" \
scripts/postgres-live-evidence.sh

cargo test --locked -p worldstream-core -p worldstream-sqlite \
  -p worldstream-postgres --lib
cargo test --locked -p worldstream-server --lib -- --test-threads=1
cargo test --locked -p worldstream-postgres --features conformance-tracer \
  --test postgres_commit live_checkpoint_rebuild_malformed_head_quarantines
cargo xtask compat verify
uv run --python 3.14.7 --no-project python -m unittest tests.postgres_harness
cargo check --locked --workspace --all-targets
```

Core passed 219 library tests, PostgreSQL passed 141, SQLite passed 141 with one
subprocess helper ignored, and the serial server suite passed 207 with two
environment-gated tests ignored. The Python harness passed all 13 tests.
Workspace compilation, formatting, diff checks, and compatibility verification
passed. Strict clippy exposed pre-existing warnings in untouched Core,
transfer, and studio-supervisor code; recovery-specific findings were corrected
or locally justified without broad unrelated cleanup.
