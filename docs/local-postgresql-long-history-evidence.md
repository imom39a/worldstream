# Local PostgreSQL and long-history evidence

This note records the local operational evidence collected on 2026-09-13 for
WorldStream's PostgreSQL paths and the 1k, 10k, and 100k history exercises. The
earlier provider audit started at revision
`680c38d9c6b3c82696cf95722b601f20a2a5fa2d`; the bounded-recovery follow-up was
regenerated from the implementation documented in the closure commit. Every
generated report sets `release_evidence=false`: these results support
local engineering decisions and Linear closure audits, but they do not claim a
Fly.io or other hosted-provider qualification.

## Bounded-recovery follow-up

The later bounded-recovery implementation adds cut-consistent operational
witnesses to both adapters. SQLite physical migration 17 and PostgreSQL
physical migration 18 bind each retained checkpoint to the exact Timer ledger,
observation Frames and consequences, Membership generations and frame Heads,
and Activation decisions at that cut. The recovery coordinator verifies and
replays at most 250 tail Transitions, then checks those facts under the exact
Head and integrity-generation install fence.

Fresh production-path SQLite runs completed on the receipt-confirmed
`checkpoint` path with zero prefix ranges, zero Transition records delivered to
Core, zero tail Transitions, and zero reducer callbacks at all three scales:

| Transitions | Recovery | RSS | Reducer callbacks | Consequence witnesses | Witness | DB bytes |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 8 ms | 23,314,432 | 0 | 1,000 | 135,280 bytes | 10,625,024 |
| 10,000 | 79 ms | 127,107,072 | 0 | 10,000 | 1,354,783 bytes | 105,857,024 |
| 100,000 | 843 ms | 775,733,248 | 0 | 100,000 | 13,639,786 bytes | 1,075,195,904 |

The complete reports are checked in under
[`docs/evidence/bounded-recovery`](evidence/bounded-recovery). The fresh local
PostgreSQL 17.11 and PgBouncer lane additionally proves a bounded checkpoint
tail, guarded operational comparison, malformed-witness cache miss,
hash-consistent canonical-witness full fallback without quarantine, and an
exact stale-Head failure-record fence. It also proves missing-materialization
rebuild and malformed-Head quarantine. Its receipt-backed scale results were:

| PostgreSQL transitions | Setup | Recovery | RSS | Prefix delivered | Adapter Transition reads | Reducers |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 1,000 | direct commits | 192 ms | 26,787,840 | 0 | 1 | 0 |
| 10,000 | direct commits | 887 ms | 59,097,088 | 0 | 1 | 0 |
| 100,000 | public v2 transfer + verified checkpoint rebuild | 1,185 ms | 1,419,444,224 | 0 | 1 | 0 |

The 100k setup includes one explicit full verified replay to create the
disposable checkpoint after transfer. The measured recovery is the subsequent
ordinary checkpoint path. All six SQLite/PostgreSQL scale measurements were
under the five-second local reference target.

Authoritative full recovery now treats a missing current Core/Activity
materialization as rebuildable while continuing to reject a present mismatch.
The exact recovered bytes are inserted only after the locked healthy Head,
integrity generation, and operational projections all match. Fresh PostgreSQL
17 containers verified raw malformed-Head quarantine at generation 2 through
both ordinary recovery and the explicit checkpoint-rebuild maintenance API.
They also verified that a corrupt newest snapshot falls back to authoritative
full replay without changing the healthy integrity generation.

The scale report was captured immediately before that final cache-isolation
follow-up. The patch removes snapshot validation and its expected-snapshot
vector only from the one-time full-recovery setup. It does not alter the
subsequent measured checkpoint path or its receipt/read counts. The focused
fresh-container tests exercise the final code; the reported 100k RSS is a
conservative pre-optimization measurement.

The remaining resource limitation is narrower than Transition history: this
workload retained one observation consequence per Transition, so its witness
grew from 135 KB to 13.64 MB even though prefix reads and reducer callbacks
remained zero. Capture and verification omit the
checkpoint when its canonical witness would exceed 16 MiB. ADR 0031 records
this limit; IMO-234 tracks compact or partitioned operational accumulators.

## PostgreSQL environment

The disposable test environment used these pinned images:

```text
postgres:17.11-alpine
postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73

edoburu/pgbouncer
edoburu/pgbouncer@sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd
```

The main disposable provider lane was run with:

```text
WORLDSTREAM_PG_LIVE_PYTHON=<pinned-python-3.14> \
WORLDSTREAM_PG_LIVE_EVIDENCE_FILE=/tmp/worldstream-postgres-live-final-680c38d9.json \
scripts/postgres-live-evidence.sh
```

It exited zero, emitted no credentials, removed its containers, and reported:

- admin migration and verification against PostgreSQL 17.11: pass;
- repeat migration after restart: pass and idempotent;
- runtime DDL rejection and read-only migration verification: pass;
- runtime direct commit, conflict, authority-fence, root-guard, rollback, and
  duplicate-resolution behavior: pass;
- the same runtime operations through PgBouncer transaction pooling: pass;
- all seven shared SQLite/PostgreSQL conformance scenarios through both direct
  and transaction-pooled PostgreSQL connections: pass;
- production gateway and redacted evidence harness: pass;
- SQLite-to-PostgreSQL transfer mechanics, checkpoint resume, conflicting chunk
  rejection, exact Room bytes and hashes, complete Room semantics, epoch fence,
  source retirement, and final target authority: pass.

The daemon lane was run separately and exited zero. It verified PostgreSQL
migration, the restricted runtime role, exact engine identity
`postgresql/17.11; server_version_num=170011`, `/healthz`, `/readyz`, log
redaction, and cleanup.

Focused live tests also passed for PostgreSQL scheduler behavior, first-time and
restart recovery of external input, observation-frame retention by age/count/
bytes, cursor progress under a busy Room, and bounded backlog deletion. The
PostgreSQL telemetry suite passed all four default cases plus its opt-in live
admin case. The supporting Rust and Python boundary suites passed: 18 direct
PostgreSQL commit tests, 140 PostgreSQL library tests, 84 backup library tests,
29 live/daemon/harness tests, 28 packaged-acceptance tests, 50 native-package
tests, 33 OCI tests, and 22 transfer-runner tests.

During this audit the runtime role was found to have direct `DELETE` permission
on `worldstream_frames`. That bypassed the bounded `SECURITY DEFINER` retention
operation. Commit `7c788d1f` now revokes and verifies that permission in every
local and packaged PostgreSQL path. The shared conformance fixture uses an
admin-only maintenance store when it must remove raw frame fixtures. Commit
`d61ef1a6` also removed SQLite/PostgreSQL transfer row-shape drift by sharing the
operational-row contract and adding `ExternalInputPreparation` to the transfer
driver.

## Native PostgreSQL restore boundary

`scripts/postgres-native-restore-smoke.sh` returned its documented unavailable
result instead of a pass:

```json
{
  "status": "unavailable",
  "exit_code": 10,
  "reason": "native_docker_retained_mount_authority_unavailable",
  "target_isolated": false,
  "target_published": false
}
```

The installed host tools are PostgreSQL 14.13 and cannot safely qualify a 17.11
source. The script's protected retained-mount path is available in its packaged
Linux lane, not on this macOS Docker host. This remains a deployment or Linux CI
check; it is not counted as passed local evidence.

## 1k, 10k, and 100k history measurements

The production SQLite history fixture produced these measurements:

| Scale | Wall time | User CPU | System CPU | RSS sample | DB bytes | WAL bytes | Snapshot writes | Retained snapshots |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 3.809 s | 2.781 s | 0.475 s | 37,683,200 | 10,432,512 | 4,181,832 | 5 | 3 |
| 10,000 | 29.693 s | 24.488 s | 3.600 s | 225,329,152 | 102,170,624 | 4,227,152 | 41 | 3 |
| 100,001 | recovery: 110.183 s | not separately retained | not separately retained | 1,209,368,576 | 1,020,776,448 | 4,227,152 | 401 | 3 |

The 100,001-transition fixture used the production SQLite core storage path and
recorded exactly 100,001 transitions, models, invocations, attempts, reducer
callbacks, and observation consequences. Crash/reopen completed. The latest
snapshot was at Room sequence 100,000, leaving a one-transition snapshot lag.

The fixed-state SQLite cadence rerun records the serialized materialization
bytes for every snapshot write in a transient observer and exports only a
bounded first/middle/last sample, instead of attributing the whole database
delta to the cache. At 1,000 transitions it observed five writes at sequences
0, 250, 500, 750, and 1,000, with serialized byte totals 886, 888, 888, 888,
and 889 respectively; three rows were retained. SQLite does not expose a
portable writer CPU counter or a per-snapshot WAL delta without forcing a
checkpoint, so both fields are emitted as unavailable with that reason. The
existing 10,000 and 100,001 cadence runs remain valid for count, retention,
and lag; they are not retroactively given per-snapshot CPU/WAL claims.

The bounded 10,000-transition rerun also showed an independent recovery limit:
cadence writes occurred at sequences 9,500, 9,750, and 10,000, but the bounded
operational-witness collection had no eligible rows for the resulting
checkpoint. This is an operational-witness bound issue, rather than evidence
that the cadence writer skipped its scheduled writes, and remains tracked by
the witness-limit work.

The retained source was then streamed into a fresh local PostgreSQL 17.11
container with the production transfer coordinator. The source history was not
regenerated for the measured transfer. Results:

| Measurement | Result |
| --- | ---: |
| Canonical records | 100,010 |
| Native operational records | 200,010 |
| Total stream records | 300,020 |
| Canonical record bytes | 785,670,211 |
| Stream file bytes | 869,605,715 |
| Chunks | 5,299 |
| Maximum records in one chunk | 64 of 64 |
| Maximum encoded chunk bytes | 259,828 of 262,144 |
| Restored SQLite backup pages | 249,213 |
| PostgreSQL hydrated transitions | 100,001 |
| Elapsed transfer exercise | 2,485.29 s |
| Maximum resident set size | 1,539,014,656 bytes |
| Peak memory footprint | 1,645,105,664 bytes |
| Internal RSS before export | 2,834,432 bytes |
| Internal RSS after export | 46,120,960 bytes |
| Internal RSS after import | 942,620,672 bytes |

The transfer report passed keyset-cursor export, bounded chunk sizes, streaming
SQLite restore without constructing `BackupImage`, partial-import resume, exact
backup and transfer-point digests, PostgreSQL hydration, source retirement, and
final PostgreSQL authority. A simulated disk-full export was rejected while the
source remained pending. A one-byte corruption at the stream midpoint was
rejected after its durable checkpoint; it hydrated zero Rooms and zero
Transitions and never published authority.

The live PostgreSQL 17 cadence audit is implemented in the conformance test and
emits exact serialized snapshot bytes plus event-to-event WAL LSN deltas. The
local live harness generated the 10,000-transition workload, but its enclosing
adapter and full-gate run failed before an accepted cadence marker was
recorded. A focused direct test was then run against a fresh pinned PostgreSQL
17.11 container; migration stopped at the reviewed schema-catalog fingerprint
check because concurrent IMO-234 migration work has not yet updated the
contract fingerprint. PostgreSQL per-snapshot CPU remains explicitly unavailable because
the standard catalogs do not provide a portable callback-level counter; WAL
event deltas include intervening canonical writes and are therefore reported
with that limitation.

The SQLite source suite contains the matrix rows for cadence count/time
(`snapshot_cadence_uses_transition_count_and_persisted_active_time`), restart
and snapshot fallback (`drop_reopen_resolves_exact_receipts_and_replays_durable_history_with_snapshot_fallback`), write failure
(`snapshot_failure_after_commit_does_not_change_canonical_result_or_recovery`),
duplicate/concurrent admission (`real_contention_serializes_duplicates_head_candidates_and_inflight_resolve`),
and complete snapshot removal (`recovery_rebuilds_materializations_after_every_paired_snapshot_is_removed`).
Those tests could not be rerun in this worktree while IMO-234's new migration
was between implementation and its expected inventory update; the test target
failed to compile on the 20-versus-19 migration array mismatch.

The high import RSS is material. The transfer iterates source records and chunks
with bounded cursors, but the observed 1.54 GB process peak is comparable to the
complete 870 MB stream. This does not yet prove IMO-225's requirement that peak
memory be bounded by declared working buffers and state rather than bundle size.
It is recorded as a limit and leaves the issue open.

## Ticket disposition

The local evidence closes no additional issue whose full acceptance criteria
were still open. It validates substantial parts of each item and narrows the
remaining work:

| Issue | Local evidence | Disposition and remaining check |
| --- | --- | --- |
| IMO-220 | Production warm-claim path preserves four history rows and reducer counters; 1,000 claims measured p50 9,602 us, p95 11,159 us, p99 15,439 us. | Keep open. Run the 10k/100k warm-claim scales, PostgreSQL parity, and the requested contention matrix. |
| IMO-222 | SQLite and PostgreSQL persist and verify cut-consistent operational witnesses; both adapters completed receipt-confirmed 1k/10k/100k checkpoint recoveries with zero prefix delivery and zero reducer callbacks; PostgreSQL 17.11/PgBouncer passed corrupt-snapshot and canonical-witness fallbacks, missing-materialization rebuild, malformed-Head quarantine, and the stale-Head failure fence. | Close from local implementation evidence. Hosted-provider qualification is tracked separately and the 16 MiB operational-witness limit remains explicit future work. |
| IMO-223 | Production SQLite cadence count/retention was measured at 1k, 10k, and 100,001 with three retained rows; the 1k run now reports exact serialized-byte samples with bounded memory and explicit CPU/WAL attribution limits. A PostgreSQL 17 audit path is implemented, but the live run did not produce an accepted marker because the enclosing adapter/full-gate run failed. | Keep open. Complete the restart/write-failure/duplicate/concurrent-Head matrix and obtain accepted PostgreSQL scale-parity evidence. |
| IMO-225 | 100,001-transition production source, bounded 5,299-chunk stream, resumable PostgreSQL import, corruption/disk-full rejection, exact digests, and final authority all passed. The new 100k recovery lane also exposed row-wise PostgreSQL staging and hydration across high-cardinality relations. | Keep open. Replace per-row round trips with bounded in-memory chunks, durable chunk commits, `COPY`/set-based validation, and a final publication fence; also resolve or bound the measured peak RSS and complete the malformed-stream matrix. |
| IMO-226 | Observation-frame retention passed live PostgreSQL age/count/bytes, busy-Room cursor, and bounded deletion checks. | Keep open. Measure actual Activation backlog behavior during sustained arrivals, outages, and bursts, including pending age, supersession, execution, oldest-useful latency, and Timer behavior. |
| IMO-227 | A deterministic 99-scenario no-model matrix now uses production admission and reports stale rate, useful latency, starvation, attempts, successful actions, and model-equivalent waste. The stated low-rate/short-delay envelope passed. | Keep open. Measure scheduler fairness/backoff and record the post-IMO-217 fixed-baseline comparison; high-rate/long-delay scenarios currently show complete starvation. |
| IMO-230 | External input first execution and restart recovery passed live PostgreSQL, and package-level tests pass. | Keep open. Exercise a real Pack with concurrent Actions and Timers, SQLite/PostgreSQL parity, and overload behavior. |
| IMO-232 | 1k, 10k, and 100k local component evidence is available. | Keep open. The requested 72-hour soak and one-million-transition/model comparison were deliberately not run in this local session. |
| IMO-233 | Packaged preflight, OCI, native-package, runbook, and daemon checks pass locally. | Keep open for the manual Fly.io fresh boot/restart and hosted-provider evidence. |
| IMO-234 | ADR 0031 and the bounded-recovery scale runs isolate an O(retained operational rows) witness scan and a 16 MiB cache cutoff. | Keep open. Design and qualify compact or partitioned operational proofs independently of canonical Transition history. |

IMO-221, IMO-224, IMO-228, IMO-229, and IMO-231 were already closed with their
separate implementation and test evidence. The 72-hour soak and Fly.io checks
are intentionally left for manual in-the-wild testing.

## Reproduction entry points

The repeatable entry points added or hardened by this work are:

```text
scripts/postgres-live-evidence.sh
scripts/postgres-daemon-live-smoke.sh
scripts/postgres-native-restore-smoke.sh
scripts/imo-225-stream-closure.sh
scripts/room-history-qualification.py
scripts/action-starvation-production-matrix.py
```

Commits `a2a02777` and `680c38d9` add the source-bound history and streaming
closure drivers. These runners reject incomplete reports, cap records and bytes
per chunk, retain durable import checkpoints, and keep credentials out of their
published JSON.
