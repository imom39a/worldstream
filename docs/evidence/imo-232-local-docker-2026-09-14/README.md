# IMO-232 local Docker one-million qualification

This directory contains the finite one-million-transition evidence produced on
2026-09-14. The large workload ran only on the local machine. It never ran on
Fly, and `hosted-cleanup.json` records that the disposable Fly app and volume
were absent before this local run completed. Every report is engineering
evidence and does not claim release readiness or replace IMO-235's 72-hour
wall-clock soak.

## Bound execution

The production fixture, warm Gateway test, and MMR qualifier came from source
revision `eab6d8c18187f7bc7fb8f145ab7fcaca369a71c8`. The local image content ID
was `sha256:037631b3be98dd463e18e62a557d776801d47b7a28f032dc43e57796c74ce123`.
The executed binaries were:

| Binary | SHA-256 |
| --- | --- |
| `history_qualification_fixture` | `d316f2e3ca20eb44b7cd6b4e7fbb31fca367f83f88ef867dedede273b64c6a20` |
| `operational_mmr_qualification` | `8cd3171c09b92c77e322d7d23960d4b9a96184fd5921959559bcb2bf31a6702d` |
| `worldstream-server-tests` | `1496b9909c2c1982d74d6c4e4144f032f43359f6ea5fb6ad578179ed3de4e8f1` |

The image used pinned Rust 1.97.1 and Debian Bookworm bases and ran Linux
6.12.76 on arm64. Docker enforced 4 CPUs, 8 GiB of memory, and no additional
swap. `/data` was a disposable Docker-managed volume on the local SSD host.
The runtime sees the host's ten CPUs, so `resource-manifest.json`, rather than
the runtime CPU count, is the source for the enforced four-CPU limit.

## Production SQLite and Gateway result

`imo-232-linux-1m.json` passed with exactly 1,000,000 models, Invocations,
attempts, accepted Transitions, durable Transition rows, and Head sequence.
The fixture prepared and wrote 4,001 snapshots under the 250-Transition
cadence and retained three coherent snapshots.

V3 recovery took 2 ms. It read one checkpoint-boundary Transition, issued no
prefix range read, delivered no prefix or tail Transition to Core, invoked no
reducer, and skipped all 1,000,000 prefix Transitions. The witness was 2,599
bytes with at most seven MMR peaks per domain. The installed executor retained
zero historical Transitions, with a maximum of one while advancing. Current
Pack context was 644 bytes.

The cold Gateway claim, including cache installation, took 17,796 us. After
cache installation the qualifier corrupted an immutable in-head Transition
and prohibited recovery. The warm path still completed 1,000 current reads,
1,000 fresh claim/release cycles, two accepted Actions, and four stale Action
rejections. Reducer counts did not change during reads or claims.

| Operation | p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| Warm current read | 4,405 us | 4,954 us | 6,012 us |
| Warm claim/release | 9,508 us | 11,838 us | 15,489 us |
| Stale Action rejection | 5,594 us | 6,330 us | 6,330 us |

Maximum sampled warm-claim RSS was 18,259,968 bytes. The fixture-process RSS
sample was 10,072,064 bytes.

## Storage result

The disposable database reached 10,539,753,472 bytes (9.816 GiB), plus a
4,330,152-byte WAL and 32,768-byte shared-memory file. That is 10,539.75 main
database bytes per accepted Transition. This is the complete durable audit
fixture: it stores one million rows each for models, Invocations, attempts,
Transitions, and observation consequences, along with indexes, receipts, and
MMR material. It is separate from the 644-byte current agent context and the
zero-history cached executor.

The entrypoint deleted the raw database and sidecars before writing `pass`.
After copying these small reports, the local container, volume, and image were
removed and their absence was verified in `cleanup.json`.

## Operational MMR result

Each tier sampled 1,000 proofs, verified every proof against its final root,
and rejected tampered leaves.

| Leaves | Persisted nodes | Maximum proof nodes | Verify p50/p95/p99 | Build |
| ---: | ---: | ---: | ---: | ---: |
| 1,000 | 1,994 | 14 | 4/5/8 us | 2 ms |
| 10,000 | 19,995 | 17 | 6/6/8 us | 24 ms |
| 100,000 | 199,994 | 21 | 7/7/7 us | 277 ms |
| 1,000,000 | 1,999,993 | 25 | 8/8/9 us | 3,667 ms |

## Artifact identities

| Artifact | SHA-256 |
| --- | --- |
| `imo-232-linux-1m.json` | `02e80effb748de218080979571886cf1a120879a1858f9e54bd66c14cba69e29` |
| `operational-mmr-1000.json` | `1b5e47bfc0a80266fde14d2e4ced8bc228d54992ea91eadfd826a505b28774c7` |
| `operational-mmr-10000.json` | `509f177c909fabfeb2bcc62d7c2245fcb8a874c8b773b8d6f7b0f9585cdeb037` |
| `operational-mmr-100000.json` | `5030e91555afb6cfa1d2e38e53b09bc4582fce8487b02bcbbe79de205d3e2a8e` |
| `operational-mmr-1000000.json` | `2eb0cb55aed5a979d2048402fe7bb05ca600d20eb639dbfdfa114fe572119aa5` |
| `runtime-manifest.json` | `2d8cd6912a2f44d09b220b0c318f46b7513af1df2514500611e9e41e37f12b5b` |
| `resource-manifest.json` | `18eeb6af9a29eef6feea0cc4affe99abce6f46b3e8085f15cba45b39db2b5ef7` |
| `final-resource-sample.json` | `10aab63ac4987fc281c74b022395c1470fcbf5c14ffbcbae404ffa06dc9d708a` |
| `hosted-cleanup.json` | `16c468810a398d6aed836e8b5a7fd913d80086588a5c5c0629f4502b52224c53` |
| `cleanup.json` | `b6552b71958107d21a3d2c4e90c6b332e1965338ea77db81ff0f08cfa0fb41fd` |
| `status` | `9f56e761d79bfdb34304a012586cb04d16b435ef6130091a97702e559260a2f2` |
