# Local PostgreSQL and long-history evidence

This note records the local operational evidence collected on 2026-09-13 for
WorldStream's PostgreSQL paths and the 1k, 10k, and 100k history exercises. The
tested source revision was `680c38d9c6b3c82696cf95722b601f20a2a5fa2d`.
Every generated report sets `release_evidence=false`: these results support
local engineering decisions and Linear closure audits, but they do not claim a
Fly.io or other hosted-provider qualification.

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
| IMO-222 | SQLite verified checkpoints replay a bounded tail and all invalid-checkpoint cases are covered. | Keep open. PostgreSQL still performs Genesis-to-Head replay because checkpoint-keyed operational witnesses have not been implemented. |
| IMO-223 | Production SQLite snapshot cadence was measured at 1k, 10k, and 100,001 with three retained rows. | Keep open. Attribute CPU/bytes/WAL specifically to snapshots versus canonical transitions, cover the full failure matrix, and run PostgreSQL scale parity. |
| IMO-225 | 100,001-transition production source, bounded 5,299-chunk stream, resumable PostgreSQL import, corruption/disk-full rejection, exact digests, and final authority all passed. | Keep open. Resolve or bound the measured 1.54 GB peak RSS and run the complete malformed/missing/reordered/duplicate large-stream matrix. |
| IMO-226 | Observation-frame retention passed live PostgreSQL age/count/bytes, busy-Room cursor, and bounded deletion checks. | Keep open. Measure actual Activation backlog behavior during sustained arrivals, outages, and bursts, including pending age, supersession, execution, oldest-useful latency, and Timer behavior. |
| IMO-227 | A deterministic 99-scenario no-model matrix now uses production admission and reports stale rate, useful latency, starvation, attempts, successful actions, and model-equivalent waste. The stated low-rate/short-delay envelope passed. | Keep open. Measure scheduler fairness/backoff and record the post-IMO-217 fixed-baseline comparison; high-rate/long-delay scenarios currently show complete starvation. |
| IMO-230 | External input first execution and restart recovery passed live PostgreSQL, and package-level tests pass. | Keep open. Exercise a real Pack with concurrent Actions and Timers, SQLite/PostgreSQL parity, and overload behavior. |
| IMO-232 | 1k, 10k, and 100k local component evidence is available. | Keep open. The requested 72-hour soak and one-million-transition/model comparison were deliberately not run in this local session. |
| IMO-233 | Packaged preflight, OCI, native-package, runbook, and daemon checks pass locally. | Keep open for the manual Fly.io fresh boot/restart and hosted-provider evidence. |

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
