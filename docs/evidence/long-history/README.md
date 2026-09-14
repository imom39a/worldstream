# Long-history qualification evidence

These artifacts are engineering evidence for the finite portions of IMO-232.
They do not substitute for the 72-hour wall-clock soak in IMO-235 and keep
`release_evidence=false` where the report schema provides that field.

## Local 100,000-transition runs

`sqlite-100000-bounded-executor.json` exercises the production SQLite/Core
fixture. Its cached executor retained zero historical Transition records at
the measured Head (maximum one while advancing). Checkpoint recovery read one
boundary Transition, delivered no prefix or tail Transitions to Core, and
skipped the complete 100,000-Transition prefix through a V3 checkpoint. The
process RSS sample was 13,320,192 bytes, the V3 witness was 2,471 bytes with at
most six MMR peaks per domain, and the SQLite database was 1,053,802,496 bytes.

`postgres-100000-transfer-recovery-local.json` was measured after a public v2
whole-deployment SQLite-to-PostgreSQL transfer into pinned PostgreSQL 17.11.
The transfer retained 100,000 Transitions, three frozen V2 roots, three MMR
receipts, and 199,994 immutable MMR nodes. A one-time full verified replay
captured a V3 checkpoint. The measured checkpoint recovery then took 42 ms,
read one boundary Transition, delivered no prefix or tail Transitions to Core,
and skipped the complete 100,000-Transition prefix. The V3 witness was 2,471
bytes and exactly matched live Timer, Frame, consequence, Membership, frame
Head, and Activation-decision counts. The 1,418,903,552-byte RSS sample belongs
to the process after the one-time authoritative full replay and is not a claim
about steady-state serving memory.

| Artifact | SHA-256 |
| --- | --- |
| `sqlite-100000-bounded-executor.json` | `940c3cd19c930ae5a251fd35c321af5e41f7729ac1087814f430cec31d6b8670` |
| `postgres-100000-transfer-recovery-local.json` | `5f7373260e96e5cd89837982fa843b1fba2e2eac2e656e5b445d1a54700d0a64` |

## Local Linux one-million run

The [local Docker report](../imo-232-local-docker-2026-09-14/README.md) was
built from `eab6d8c18187f7bc7fb8f145ab7fcaca369a71c8` and ran with four CPUs,
an 8-GiB memory limit, no additional swap, and a Docker-managed volume on the
local SSD host. It passed exactly 1,000,000 accepted Transitions, 1,000 warm
reads, 1,000 warm claim/release cycles, the corruption and serving-fence
checks, and all four MMR tiers.

V3 recovery took 2 ms, read one boundary Transition, delivered no prefix or
tail to Core, and skipped the million-Transition prefix. The 2,599-byte
witness used at most seven MMR peaks per domain. The executor retained zero
historical Transitions and the fixed current Pack context was 644 bytes.
The main JSON has SHA-256
`02e80effb748de218080979571886cf1a120879a1858f9e54bd66c14cba69e29`.

The 10,539,753,472-byte database was a disposable full audit fixture, not
agent context. The entrypoint deleted it before publishing `pass`, and the
container, volume, and image were removed after the small reports were copied.

The complete local PostgreSQL/PgBouncer report is retained under
`docs/evidence/warm-activation`. Local Linux/Docker artifacts bind the
million-scale run to its source revision, image content ID, binaries, CPU and
memory limits, kernel, and disposable Docker-managed volume and are retained
in their own evidence directory.
