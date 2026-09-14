# Local PostgreSQL and long-history evidence

This note records the local operational evidence collected on 2026-09-13 for
WorldStream's PostgreSQL paths and the 1k, 10k, and 100k history exercises. The
earlier provider audit started at revision
`680c38d9c6b3c82696cf95722b601f20a2a5fa2d`; the bounded-recovery follow-up was
regenerated from the implementation documented in the closure commit. Every
generated report sets `release_evidence=false`: these results support
local engineering decisions and Linear closure audits, but they do not claim a
Fly.io or other hosted-provider qualification.

## Current finite closure result

The 2026-09-14 rerun supersedes the open PostgreSQL and compact-witness gaps
described later in this historical note. The final aggregate report is
[`docs/evidence/warm-activation/imo-220-postgres-live-local.json`](evidence/warm-activation/imo-220-postgres-live-local.json),
SHA-256 `deb622e4e3b8cc3021cc23119e1a581d238676b2c389a7674d095c9b3f8d8024`.
It passed pinned PostgreSQL 17.11, direct runtime, PgBouncer transaction
pooling, seven shared conformance scenarios on both paths, snapshot cadence,
V3 recovery at 1k/10k/100k, and a 100k public transfer. The V3 witnesses were
2,450, 2,346, and 2,471 bytes respectively; every recovery read one boundary
Transition and delivered zero prefix or tail Transitions to Core.

The transfer-specific 100k report is
[`docs/evidence/long-history/postgres-100000-transfer-recovery-local.json`](evidence/long-history/postgres-100000-transfer-recovery-local.json),
SHA-256 `5f7373260e96e5cd89837982fa843b1fba2e2eac2e656e5b445d1a54700d0a64`.
It verified 100,000 canonical Transitions, three frozen V2 roots, three MMR
receipts, 199,994 immutable MMR nodes, and exact current operational state
after SQLite-to-PostgreSQL transfer. The source and transfer code are at
`b8d6044ac6faa5f647f4eac7dbd214a9ed453584`.

The SQLite current-source 100k report is
[`docs/evidence/long-history/sqlite-100000-bounded-executor.json`](evidence/long-history/sqlite-100000-bounded-executor.json),
SHA-256 `940c3cd19c930ae5a251fd35c321af5e41f7729ac1087814f430cec31d6b8670`.
Its installed executor retained zero historical Transitions, its checkpoint
witness was 2,471 bytes, and the process RSS sample was 13,320,192 bytes.

The only deliberately excluded qualification is the 72-hour wall-clock soak,
which remains assigned to IMO-235.

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

## IMO-225 bounded stream-transfer closure

On 2026-09-14, the current-source PostgreSQL 17.11 closure harness reran the
100,001-Transition production SQLite fixture after native operational
publication was changed to bounded, typed transaction-local staging and
set-based publication. Canonical replay verification now keysets the Room's
Transitions twice: first for structural preflight and then for executable
replay. It retains only the current Head and executable state, rather than a
vector of every Transition byte string.

The [committed compact report](evidence/imo-225-stream-closure-2026-09-14.json)
was copied from the harness output before it removed its PostgreSQL container
and temporary source, backup, restored backup, and stream tree on exit. It
reported the following exact result:

| Measurement | Result |
| --- | ---: |
| SQLite Transitions | 100,001 |
| Canonical records | 100,010 |
| Native operational records | 200,010 |
| Total stream records | 300,020 |
| Exact retained record bytes | 785,670,211 |
| Stream file bytes | 869,347,584 |
| Chunks | 335 |
| Maximum records / chunk limit | 1,000 / 1,000 |
| Maximum encoded chunk bytes / limit | 4,193,184 / 4,194,304 |
| Restored SQLite backup pages | 249,226 |
| PostgreSQL hydrated Transitions | 100,001 |
| Resume checkpoint next chunk | 1 |
| RSS before export | 2,850,816 bytes |
| RSS after export | 19,333,120 bytes |
| RSS after resume import | 149,323,776 bytes |
| RSS after corruption probe | 64,618,496 bytes |
| RSS after authority finalization | 251,133,952 bytes |
| Maximum measured transfer-process delta | 248,283,136 bytes |

The RSS rows are emitted from the transfer example after each named phase.
The enclosing `/usr/bin/time -l` process reported 2,098,511,872 bytes because
it also compiled the release workspace before executing the example; it is not
used as the transfer working-set measurement. The phase samples show that the
working process remains bounded by page buffers and current executable state,
rather than retaining the 785 MB record set or 869 MB stream.

The closure report also passed exact backup streaming and restore, resume
import, full source-to-target native row equality, canonical replay,
operation-guarded semantic receipts, final target authority, and source
retirement. Its disk-full export probe rejected the write while retaining
source authority. Its one-byte corrupted-stream probe rejected the stream with
zero hydrated Rooms and zero published authority.

The focused release transfer tests completed the remaining fail-closed matrix:

| Case | Evidence |
| --- | --- |
| Over 100k records and over 64 MiB while preserving legacy stream limits | `streaming_container_v2_crosses_legacy_record_and_byte_limits_with_bounded_chunks` passed. |
| Malformed identity or footer | `streaming_container_v2_rejects_tampered_footer_without_partial_success` passed. |
| Reordered or duplicate chunks; interrupted finalization | `streaming_container_v2_rejects_duplicate_chunks_and_interrupted_finalization` passed. |
| Interrupted import with a missing durable destination chunk | `persisted_checkpoint_reconfirms_or_repairs_missing_destination_chunk` passed. |
| Corrupted manifest during resumed import | `manifest_stream_resumes_after_interruption_and_refuses_corruption_before_finalization` passed. |
| Disk-full export | Current-source closure report: rejected, source remained pending. |
| Ambiguous finalization after lost source-retirement result | `stream_authority_coordinator_reconciles_definite_failure_and_lost_retirement_result` passed. |

These tests retain legacy identity-only v2 stream parsing, but do not permit an
old stream without a source-authenticated manifest to cross the
authority-adjacent finalization seam. That boundary preserves old bundle
compatibility without treating weaker historical evidence as publication
authority.

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
and 889 respectively; three rows were retained. A final local Linux run from
`a187c53c` fenced one postcommit cache transaction with a quiescent WAL and
disabled autocheckpointing. It measured 737,500 ns of writer-thread CPU, 886
logical payload bytes, and exactly 32,992 WAL bytes separately from the
canonical commit. The existing 10,000 and 100,001 cadence runs remain valid
for count, retention, and lag; they are not retroactively given per-snapshot
CPU/WAL claims. See `docs/evidence/snapshot-cadence/README.md`.

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

The final live PostgreSQL 17.11 cadence audit passed on both the direct and
PgBouncer paths and records 1k/10k events, three-row retention, and exact
serialized snapshot bytes. PostgreSQL per-snapshot CPU remains explicitly
unavailable because the standard catalogs do not provide a portable
callback-level counter; WAL event deltas include intervening canonical writes
and are therefore reported as non-exact. This is a documented provider
instrumentation boundary, not an inferred zero cost.

The SQLite source suite contains the matrix rows for cadence count/time
(`snapshot_cadence_uses_transition_count_and_persisted_active_time`), restart
and snapshot fallback (`drop_reopen_resolves_exact_receipts_and_replays_durable_history_with_snapshot_fallback`), write failure
(`snapshot_failure_after_commit_does_not_change_canonical_result_or_recovery`),
duplicate/concurrent admission (`real_contention_serializes_duplicates_head_candidates_and_inflight_resolve`),
and complete snapshot removal (`recovery_rebuilds_materializations_after_every_paired_snapshot_is_removed`).
The final local Linux suite ran these regressions with the snapshot probe and
current migrations: 157 passed, zero failed, and one subprocess-only probe was
ignored. The qualification example test also passed.

The earlier 1.54 GB import sample was material: it exposed retained canonical
Transition vectors and row-wise native publication despite bounded source
cursors. The 2026-09-14 post-fix closure below replaces that disposition with
phase-specific transfer-process measurements after set-based native pages and
incremental canonical replay were installed.

## Ticket disposition

The 2026-09-14 finite closure reruns complete the rows below. IMO-235 remains
open because it alone owns the 72-hour elapsed schedule.

| Issue | Local evidence | Disposition and remaining check |
| --- | --- | --- |
| IMO-220 | Production SQLite warm claims passed at 1k/10k/100k with 1,000 read and claim samples per tier after recovery was forbidden and an in-head historical row was corrupted. The final PostgreSQL 17.11 lane passed 1,000 direct and 1,000 pooled samples with unchanged reducer and canonical-row counts. | Close from implementation, scale, safety, and provider evidence. |
| IMO-222 | SQLite and PostgreSQL persist and verify cut-consistent operational witnesses; both adapters completed receipt-confirmed 1k/10k/100k checkpoint recoveries with zero prefix delivery and zero reducer callbacks; PostgreSQL 17.11/PgBouncer passed corrupt-snapshot and canonical-witness fallbacks, missing-materialization rebuild, malformed-Head quarantine, and the stale-Head failure fence. | Close from local implementation evidence. Hosted-provider qualification is tracked separately and the 16 MiB operational-witness limit remains explicit future work. |
| IMO-223 | Production SQLite cadence passed the count/time, restart, failure, duplicate, concurrent-Head, lagging-recovery, and canonical-equivalence matrix. A source-bound local Linux probe separately measured SQLite snapshot CPU, logical bytes, and exact WAL. The pinned PostgreSQL 17.11 direct/PgBouncer lane passed its 1k/10k cadence audit with three retained rows and explicit provider attribution limits. | Close from implementation, failure-matrix, scale, isolated-cost, and provider evidence. |
| IMO-225 | The 2026-09-14 current-source closure passed 100,001 Transitions and 785,670,211 exact record bytes through 335 bounded chunks. It verifies page-bounded native staging and equality, incremental canonical replay, resume, source retirement, final target authority, disk-full and corruption rejection, and the focused malformed/interrupted/missing/duplicate/ambiguous-finalization matrix. Its transfer-process RSS peak was 251,133,952 bytes, a 248,283,136-byte phase delta. | Close from committed local evidence. The report remains explicitly non-release evidence and does not replace hosted-provider qualification tracked elsewhere. |
| IMO-226 | The bounded backlog policy, exact supersession audit trail, timer obligations, SQLite/PostgreSQL parity, and the sustained-arrival/outage/burst qualification are complete in that ticket's retained evidence. | Already closed; its completed blocker satisfies the corresponding IMO-232 finite reliability row. |
| IMO-227 | A deterministic 99-scenario no-model matrix now uses production admission and reports stale rate, useful latency, starvation, attempts, successful actions, and model-equivalent waste. The stated low-rate/short-delay envelope passed. | Keep open. Measure scheduler fairness/backoff and record the post-IMO-217 fixed-baseline comparison; high-rate/long-delay scenarios currently show complete starvation. |
| IMO-230 | External input first execution and restart recovery passed live PostgreSQL, and package-level tests pass. | Keep open. Exercise a real Pack with concurrent Actions and Timers, SQLite/PostgreSQL parity, and overload behavior. |
| IMO-232 | Production SQLite and warm Gateway paths passed through 1m in a locally run Linux container with 4 CPUs and an 8-GiB limit; PostgreSQL, transfer, reliability, MMR, and Luna context-comparison artifacts cover every other finite row. | Close the finite qualification. The 72-hour wall-clock experiment remains exclusively in IMO-235. |
| IMO-233 | Packaged preflight, OCI, native-package, runbook, and daemon checks pass locally. | Keep open for the manual Fly.io fresh boot/restart and hosted-provider evidence. |
| IMO-234 | ADR 0037 and V3 checkpoints replace the linear witness with three frozen roots and logarithmic MMR receipts. SQLite and PostgreSQL 1k/10k/100k recoveries use 2.3-2.5 KiB witnesses, one boundary read, and no prefix delivery; MMR proofs scale through 1m with at most 25 nodes. | Close from design, implementation, tamper/fencing, replay-equivalence, scale, and provider evidence. |
| IMO-236 | SQLite and PostgreSQL serving reads verify domain/index/cause-bound MMR proofs against the admitted V3 receipt. Tamper tests, retention, full verification, backup, and public transfer pass; the final 100k transfer preserves three roots, three receipts, and 199,994 nodes. | Close from ADR 0037, adapter tests, 1k/10k/100k/1m proof measurements, and transfer evidence. |

IMO-221, IMO-224, IMO-228, IMO-229, and IMO-231 were already closed with their
separate implementation and test evidence. The 72-hour soak remains for manual
in-the-wild testing in IMO-235. IMO-233's hosted fresh-boot check is a distinct
release-publication concern rather than part of this finite long-history run.

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
