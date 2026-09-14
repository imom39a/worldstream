# IMO-223 snapshot cadence and isolated cost evidence

This directory closes the finite measurement gap in IMO-223. Both the fixture
and test suite ran in local Docker on 2026-09-14. No Fly app, Machine, volume,
or remote image was created for this work. These are engineering measurements,
not release-readiness evidence.

## Bound artifact and resources

The fixture was built from source revision
`a187c53c97d86bde963821d61a8a5a752eb3670d` with pinned Rust 1.97.1 and Debian
Bookworm base images. The temporary runtime image was
`sha256:437bacedccd7ef3849a5dc5917214e28ef7a0164dac8e8df6a3c6104ef94164c`;
the executed `history_qualification_fixture` binary was
`d323e470398df2a1083682eaebc396580d11b2a6eadca2363ea3af6ec851eea2`.
The fixture container had one CPU, 1 GiB of memory with no additional swap, no
network, and one disposable Docker-managed volume.

## Production SQLite result

The production writer committed exactly 1,000 Transitions and reported
`pass=true`. Snapshot selection happened before expensive snapshot cloning and
serialization.

| Measurement | Result |
| --- | ---: |
| Snapshot preparations | 5 |
| Snapshot writes observed | 5 |
| Snapshot rows retained | 3 |
| Last snapshot Room sequence | 1,000 |
| Transitions since snapshot | 0 |
| Sampled logical snapshot bytes | 886 at Genesis; 888 at 500; 889 at 1,000 |
| Recovery prefix range reads/delivery | 0 / 0 |

Five writes are the Genesis snapshot plus four count-cadence snapshots at the
250-Transition interval. Only three coherent pairs remain, as required by the
retention policy.

The fixture separately measured one due postcommit cache transaction. It first
made the WAL quiescent, disabled automatic checkpointing on the private writer
connection, measured only the cache transaction, read the resulting WAL length,
and restored the prior connection setting.

| Isolated cache transaction | Result |
| --- | ---: |
| Room sequence | 0 |
| Logical payload bytes | 886 |
| Writer-thread CPU | 737,500 ns |
| Exact WAL contribution | 32,992 bytes |

The logical byte count is the exact sum of the three serialized blobs inserted
for the paired snapshot. The WAL value is the physical file length after the
quiescent truncate and single measured transaction. The CPU value comes from
Linux `CLOCK_THREAD_CPUTIME_ID` around that transaction. Aggregate run-level
CPU and WAL fields remain unavailable so consumers cannot mistake mixed
canonical and cache work for snapshot-only cost.

## PostgreSQL boundary

The retained local PostgreSQL 17.11/PgBouncer report separately verifies the
same 250-Transition/five-minute policy, three-row retention, concurrent
same-identity behavior, and 1k/10k cadence observations. It records exact
serialized payload sizes. Stock PostgreSQL catalogs do not expose a portable
per-callback server CPU counter, and the observed LSN interval includes
canonical writes, so those two provider fields remain explicitly unavailable
or non-exact. No estimate is promoted to an exact measurement.

## Verification and cleanup

A local Linux container with one CPU, 3 GiB of memory, no extra swap, and no
network ran the complete `worldstream-sqlite` library suite: 157 passed, zero
failed, and one subprocess-only probe was ignored. The qualification example
test also passed. This includes count/time cadence, restart and rebuild,
postcommit failure safety, cancellation, WAL-autocheckpoint restore, and the
restore-failure writer poison regression.

The runtime image, test builder image, containers, fixture volume, database,
WAL, and shared-memory file were removed after the small reports were copied.
`cleanup.json` verifies their absence.

| Artifact | SHA-256 |
| --- | --- |
| `imo-223-sqlite-linux-local.json` | `a0ee0e2f4cbaf5774d61e910ba8685c00c4dd0d1915db1f1623cbf0e8800d9c3` |
| `resource-manifest.json` | `5efad3e1ef05540faeca7d3c153f1935fa81a22d2d2bb1f0185e47bbf3242344` |
| `test-manifest.json` | `215b3916bde3662a2b3399d3add321a657dd3cfe48764f75321fde158c8da1be` |
| `cleanup.json` | `4c2bb5ff094a87c1e388d8fa0996993e13cfade558a1409c8e2c9ac9701c66a9` |
