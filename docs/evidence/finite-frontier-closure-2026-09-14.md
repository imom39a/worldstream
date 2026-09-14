# Finite long-history frontier closure

Date: 2026-09-14

This note closes the finite implementation and qualification work tracked by
IMO-220, IMO-223, IMO-232, IMO-234, and IMO-236. It deliberately excludes the
72-hour elapsed soak, which remains open as IMO-235. All retained generated
reports are engineering evidence and do not claim release readiness.

The million-transition workload ran only in local Docker. The disposable Fly
app prepared during qualification was deleted before the large workload ran;
the retained hosted-cleanup receipt confirms that neither its app nor volumes
remain. This keeps hosted capacity and cost out of the scale test.

## Result

| Area | Implementation/result | Evidence |
| --- | --- | --- |
| Warm Room executor | SQLite reuses one Head- and integrity-fenced executor for reads, claims, Actions, and Timers. Cached traces discard persisted Transition history. PostgreSQL uses the equivalent fenced trace cache. | [Warm activation](warm-activation/README.md) |
| Snapshot cadence | Snapshot preparation happens at 250 accepted Transitions or five active minutes and retains three coherent pairs. Failure, restart, duplicate, contention, lagging recovery, and PostgreSQL parity pass. A local Linux probe isolates one SQLite cache transaction's CPU, logical bytes, and exact WAL contribution from canonical work. | [Snapshot cadence](snapshot-cadence/README.md) |
| Bounded checkpoint recovery | V3 binds the current checkpoint to frozen V2 roots and authenticated MMR receipts. Current-source SQLite and PostgreSQL recover with one boundary read and no prefix delivery. | [IMO-234 matrix](imo-234-acceptance-matrix.md) |
| Authenticated operational reads | Domain/index/cause-bound MMR proofs fail closed for coordinated payload/hash replacement and malformed, stale, wrong-domain, wrong-index, or altered proof material. | [ADR 0037](../adr/0037-successor-anchored-operational-receipts.md) |
| Retention and full verification | Pruned payloads produce the existing Reset behavior while retained rows remain provable. Administrative verification rebuilds every node/receipt and legacy histories retain explicit full-verification fallback. | [Operational MMR](operational-mmr/README.md) |
| Backup and transfer | Backup, native stream, preflight, and PostgreSQL publication preserve the frozen roots before count-matched MMR receipts and nodes. The final 100k transfer verifies exact parity. | [Long-history evidence](long-history/README.md) |
| Finite Runner reliability | Sixteen release regressions cover lost replies, acknowledgement durability, restart/revocation, Timer obligations, authorization, slow consumers, and bounded queues. | [Criterion-five report](imo-232-criterion-5-local.json) |
| Context strategy | Equal-budget Luna evaluation favors current Projection plus explicit work or summary plus authorized retrieval over a recent-history window. | [Runner context evaluation](runner-context-evaluation/README.md) |
| One-million Linux scale | A source-bound local Docker image runs the production 1m SQLite fixture, the warm Gateway qualifier with 1,000 reads and claims, and MMR proofs at 1k/10k/100k/1m with 4 CPUs and an 8 GiB limit. | [Local Docker evidence](imo-232-local-docker-2026-09-14/README.md) |

## Verification

The final code and evidence build on these implementation points:

- `de8e3e3f757b7771d2ef34a72c123bbb5970fc5e` integrates V3 recovery.
- `b8d6044ac6faa5f647f4eac7dbd214a9ed453584` additionally transfers the
  frozen V2 roots before their MMR receipts and is covered by the final local
  PostgreSQL transfer and Rust suites.
- `a241bad5204162d2d2d5fc08cb61c64426b4bc70` removes redundant qualifier
  inspections, supports a retained fixture, and records the fixture report's
  identity.
- `40357cc351a91066a71a0dbd76be28560cf4de03` makes healthy SQLite Gateway
  cache installation use guarded recovery plus a bounded serving fence instead
  of a full immutable-history inspection.
- `eab6d8c18187f7bc7fb8f145ab7fcaca369a71c8` places the serving materialization,
  Head, integrity, and frame-head reads in one transaction and rejects a
  post-recovery quarantine race. It is the source revision of the local
  one-million Docker image.
- `12bd7969`, `3fd05a5e`, and `a187c53c97d86bde963821d61a8a5a752eb3670d`
  add a bounded opt-in SQLite snapshot-cost probe, make cancellation and WAL
  autocheckpoint handling fail closed, and preserve additive `sqlite-v1`
  report compatibility. The final snapshot probe and Linux test suite use
  `a187c53c`.

The final test record is:

- `worldstream-backup`: 85 passed;
- `worldstream-transfer`: 55 passed;
- `worldstream-postgres`: 151 passed;
- `worldstream-sqlite`: 157 passed, with one intentionally ignored
  subprocess-only probe;
- `worldstream-server`: 209 passed, 5 environment-gated tests ignored;
- focused root ordering, transfer, recovery, MMR, and PostgreSQL provider
  checks: passed;
- full disposable PostgreSQL 17.11/PgBouncer evidence script: exit 0;
- one-million local Docker fixture, warm Gateway, and MMR qualification:
  passed;
- local Linux isolated snapshot fixture and qualification example: passed;
- format, compile, shell syntax, JSON invariants, hashes, and diff checks:
  passed.

## Evidence identities

| Artifact | SHA-256 |
| --- | --- |
| Final-source SQLite warm/cold Gateway qualification | `d80f32c377dff3e191e1973d2d0bf2a8ad56c2a73d892a1d9a30aabb3dcf4725` |
| Local Linux isolated SQLite snapshot transaction | `a0ee0e2f4cbaf5774d61e910ba8685c00c4dd0d1915db1f1623cbf0e8800d9c3` |
| PostgreSQL 17.11/PgBouncer aggregate | `deb622e4e3b8cc3021cc23119e1a581d238676b2c389a7674d095c9b3f8d8024` |
| Shared direct/pooler comparison | `4ab4b2ea7f7e716f167d531579ac62c415bc89bf9c610f26faf3e1fd888e9a84` |
| SQLite 100k bounded executor | `940c3cd19c930ae5a251fd35c321af5e41f7729ac1087814f430cec31d6b8670` |
| PostgreSQL 100k transfer/recovery | `5f7373260e96e5cd89837982fa843b1fba2e2eac2e656e5b445d1a54700d0a64` |
| Local Docker 1m production/Gateway qualification | `02e80effb748de218080979571886cf1a120879a1858f9e54bd66c14cba69e29` |
| Local Docker 1m MMR | `2eb0cb55aed5a979d2048402fe7bb05ca600d20eb639dbfdfa114fe572119aa5` |
| Native local 1m MMR comparison | `e9505e12f86a5366d600bd6ac1fede2349260dfaf5329ae051a530f476163749` |
| 100k+ stream portability | `2dd07b1734cb37a713d943bec64833973e5e4a79baab243d36aeda77d2c14cc3` |
| Finite Runner reliability | `3c5bc4bd5f4bdba6c919b7f0754588bbff1ea0d684861488bde763ff236cb2b5` |
| Luna context evaluation | `afcf043c12e45a86957bb61172b9afff412c71f409474e812d2bf0e3c07c6be5` |

The local Linux/Docker artifact hashes, image digest, resource limits, and
cleanup receipt are recorded in its evidence directory.

## Explicit boundary

The direct recovery layer does not replay the canonical prefix for an eligible
V3 checkpoint. A healthy SQLite Gateway cache miss uses that guarded recovery,
then captures current materialization, Head, integrity, and frame heads in one
bounded read transaction. It does not call the immutable full-history
inspection path and fails closed on concurrent quarantine or Head change. A
legacy history, authoritative repair, or forensic verification may perform a
full replay. Model input never receives the event ledger unless an authorized,
bounded historical-evidence request explicitly asks for a slice.

IMO-235 remains open for the manual 72-hour run. Its outcome cannot be inferred
from these finite tests.
