# IMO-220 warm activation qualification

These redacted engineering reports exercise the production Core, SQLite,
PostgreSQL, and Gateway paths. They do not claim release qualification and
retain `release_evidence=false` in the aggregate provider report.

## Retained artifacts

| Artifact | SHA-256 | Result |
| --- | --- | --- |
| `imo-220-warm-claim-local.json` | `d80f32c377dff3e191e1973d2d0bf2a8ad56c2a73d892a1d9a30aabb3dcf4725` | Final-source SQLite 1k/10k/100k cold install and warm Activation qualification, 1,000 samples per tier |
| `imo-220-safety-local.json` | `c6d4fbe45b615bb2d47cefb97b7f6fa7687bf95665f951f6d1307afe28580bd9` | Eleven exact safety and regression cases passed |
| `imo-220-postgres-live-local.json` | `deb622e4e3b8cc3021cc23119e1a581d238676b2c389a7674d095c9b3f8d8024` | Full PostgreSQL 17.11 and PgBouncer lane passed |
| `imo-220-postgres-live-local.json.imo-50-shared-comparison.json` | `4ab4b2ea7f7e716f167d531579ac62c415bc89bf9c610f26faf3e1fd888e9a84` | Seven of seven shared conformance scenarios passed on direct and pooled connections |

The JSON omits database paths, credentials, connection strings, and raw cargo
logs. The source databases and provider containers were disposable and the
provider report records `secrets_emitted=false` and cleanup `pass`.

## SQLite warm-path results

Every tier installed one verified cached executor before measuring 1,000
current reads and 1,000 fresh claim/release cycles. Historical recovery was
then made unavailable and an immutable in-head Transition was corrupted. A
warm read, claim, or Action therefore fails if it tries to recover or scan the
canonical prefix. Reducer callbacks stayed unchanged during all reads and
claims; two authorized Actions per tier were the only operations that advanced
the canonical history.

| History | Warm read p50/p95/p99 | Fresh claim/release p50/p95/p99 | Maximum sampled claim RSS |
| ---: | ---: | ---: | ---: |
| 1,000 | 4,771 / 5,039 / 5,230 us | 9,580 / 10,100 / 10,724 us | 19,644,416 bytes |
| 10,000 | 4,704 / 4,963 / 5,251 us | 9,421 / 9,826 / 10,326 us | 18,989,056 bytes |
| 100,000 | 4,850 / 5,066 / 5,297 us | 9,569 / 16,157 / 22,087 us | 21,544,960 bytes |

The healthy cold cache-install path now performs guarded V3 recovery and then
reads the current materialization, Head, integrity generation, and frame heads
inside one bounded serving-fence transaction. It does not call the immutable
full-history inspection path. Cache installation fails closed if the Room is
quarantined or its Head changes after recovery. The retained executor keeps
Head, current state, Timers, and bounded work projections while discarding
persisted Transition history. Legacy histories, authoritative repair, and
forensic verification retain their explicit replay behavior.

The measured cold claim, including cache installation, was 16,819 us at 1k,
17,438 us at 10k, and 16,999 us at 100k. Each source fixture recovered through
V3 with one boundary Transition read, zero prefix or tail delivery, and zero
reducer callbacks.

## PostgreSQL and PgBouncer results

The final local provider lane used pinned `postgres:17.11-alpine` and the
pinned PgBouncer image recorded in the report. It ran 1,000 current reads and
1,000 fresh claim/release cycles through each connection profile. Recovery was
forbidden during the measured cycles, reducer callbacks stayed at one, and the
canonical Transition count stayed at one.

| Connection | Current read p50/p95/p99 | Claim/release p50/p95/p99 |
| --- | ---: | ---: |
| Direct | 203,835 / 216,162 / 222,143 us | 583,240 / 601,474 / 618,851 us |
| PgBouncer transaction pool | 28,898 / 34,044 / 38,968 us | 109,393 / 125,415 / 135,426 us |

The same lane passed migrations and restart idempotence, runtime-role DDL
denial, direct and pooled commit resolution, exact Head and integrity fences,
snapshot cadence at 1k and 10k, V3 checkpoint recovery at 1k/10k/100k, a
100k SQLite-to-PostgreSQL transfer, and seven shared provider-conformance
scenarios on both connection paths.

## Acceptance mapping

| IMO-220 row | Evidence |
| --- | --- |
| Warm operations avoid canonical prefix work | SQLite makes recovery unavailable after cache installation; PostgreSQL forbids recovery during measured reads and claims. Both preserve reducer counts. The SQLite healthy cold install also avoids the immutable full-history inspection after guarded recovery. |
| Current state remains bounded | The SQLite cache retains no historical Transitions after installation. PostgreSQL checkpoint recovery reads one boundary Transition, delivers no prefix or tail Transitions, and uses a 2.3-2.5 KiB V3 witness at all measured tiers. |
| Actions and Timers reuse the executor | SQLite accepts two Actions after the corruption fence and `warm_activation_claim_reuses_executor_after_due_timer` advances a due Timer through the same executor. |
| Safety fences remain exact | The eleven-case safety report covers Head, integrity generation, revocation, membership, policy, cursor/reset, expiry, lost replies, corruption, and due-Timer behavior. The PostgreSQL provider lane covers its analogous exact fences. |
| Direct and pooled PostgreSQL paths agree | The provider report and its shared-comparison sidecar pass all seven shared scenarios for both direct and transaction-pooled connections. |

## Reproduction

```sh
scripts/imo-220-warm-safety-matrix.sh \
  --evidence docs/evidence/warm-activation/imo-220-safety-local.json

scripts/imo-220-warm-claim-qualification.sh \
  --evidence docs/evidence/warm-activation/imo-220-warm-claim-local.json \
  --samples 1000

WORLDSTREAM_PG_LIVE_DEBUG_DIR=/tmp/worldstream-pg-evidence \
WORLDSTREAM_POSTGRES_WARM_CLAIM_SAMPLES=1000 \
scripts/postgres-live-evidence.sh \
  --evidence docs/evidence/warm-activation/imo-220-postgres-live-local.json
```
