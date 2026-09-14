# IMO-220 warm activation qualification

These are redacted local SQLite results produced on 2026-09-14. They exercise
the production Core/SQLite and GatewayBackend paths; they do not claim hosted
or release qualification. The PostgreSQL acceptance row is deliberately still
open while the shared IMO-234 forensic-row correction is integrated and the
full pinned PostgreSQL 17/PgBouncer lane is rerun.

## Retained artifacts

| Artifact | SHA-256 | Result |
| --- | --- | --- |
| `imo-220-warm-claim-local.json` | `f044bf944ea57555880a90a73e7d3bed3b86a8c682fd954e599c264d5ce37689` | 1k/10k/100k source-backed warm Activation qualification, 1,000 samples/tier |
| `imo-220-safety-local.json` | `c6d4fbe45b615bb2d47cefb97b7f6fa7687bf95665f951f6d1307afe28580bd9` | 11 exact safety and regression cases passed |

The JSON omits database paths, bearer values, connection strings, and raw cargo
logs. The source databases were disposable and deleted after their reports were
written.

## SQLite results

Every history came from the production SQLite history fixture with a V2
checkpoint. The fixture then installed a cold Gateway executor, corrupted an
in-head immutable Transition row, prohibited recovery, and performed the warm
operations below. A pass therefore requires the warm path to serve without
reading that poisoned historical source. It also authenticates the checkpoint
V2 witness hash and snapshot Head, and uses guarded checkpoint recovery to
verify the final durable operational roots.

| History | Fixture rows / Head | Snapshot preparations / writes | Cold claim | Warm current read p50/p95/p99 | Fresh warm claim/release p50/p95/p99 | Claim RSS p50/p95/p99/max |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 1,000 / 1,000 | 5 / 5 | 208,973 µs, 2,306 bytes | 4,430 / 4,861 / 5,096 µs | 8,729 / 9,247 / 12,849 µs | 44,302,336 / 44,302,336 / 44,335,104 / 44,335,104 bytes |
| 10,000 | 10,000 / 10,000 | 41 / 41 | 3,724,105 µs, 2,310 bytes | 4,473 / 24,233 / 42,820 µs | 8,962 / 12,206 / 28,000 µs | 94,339,072 / 94,339,072 / 94,355,456 / 94,355,456 bytes |
| 100,000 | 100,000 / 100,000 | 401 / 401 | 55,412,416 µs, 2,313 bytes | 4,543 / 4,929 / 5,668 µs | 9,157 / 11,023 / 15,439 µs | 16,334,848 / 116,342,784 / 116,604,928 / 116,752,384 bytes |

For each tier, 1,000 warm current reads and 1,000 fresh claim/release cycles
left the cached reducer-callback count unchanged. Each tier then accepted two
authorized `increment` Actions after the corruption fence, adding exactly two
canonical Transition rows. Four stale Actions were rejected, the activation
returned to one durable pending offer, and the serving Head remained at the
post-Action fence.

## Acceptance mapping

| IMO-220 acceptance row | Evidence |
| --- | --- |
| Regression against the old full-history route | `source_backed_warm_activation_claim_qualification` makes recovery unavailable after cache installation and corrupts a historical row. A warm read, claim, or Action that regresses to recovery or prefix history fails. |
| Successive claims, releases, and new operations at 1k/10k/100k avoid canonical prefix work | The retained tier results record actual source Transition rows, reducer counters, 1,000 warm reads, 1,000 fresh claim/release cycles, and two real Actions per tier. Warm reads and claims preserve their reducer counter; only the two Actions add rows/callbacks. |
| Bounded current state and delivery data, with separate cold measurements | Each tier reports cold-claim latency/context separately from warm read and claim percentiles, plus 128 bounded RSS samples. The V2 checkpoint contract proves the selected boundary witness and final durable roots without a prefix delivery. |
| Intervening Action and Timer do not replay Genesis | The source-backed run performs two accepted Actions after its historical corruption fence. `warm_activation_claim_reuses_executor_after_due_timer` installs the warm executor, advances a due Timer, and claims through the same executor with recovery forbidden. |
| Head, revocation, membership/role, integrity, policy, cursor/reset, expiry, and lost-reply fencing | `imo-220-safety-local.json` records 11 passing cases, including commit-time expiry/revoke ordering, principal/membership-generation drift, exact Head/integrity rereads, reset/pruned-prefix fences, activation receipt/context retirement, bounded refresh policy, corruption quarantine, repeated warm claims, and the due-Timer regression. |
| Cold miss remains correct and is not presented as a bounded claim | Every tier records the actual cold claim result (`granted`) separately from warm percentiles and context bytes. The 100k cold claim was 55,412,416 µs; no bounded-cold claim is made. |
| PostgreSQL analogous path | Pending one fresh full PostgreSQL 17.11/PgBouncer run after the shared IMO-234 forensic-row correction. The driver already records direct and transaction-pool warm current-read and fresh claim/release percentiles, reducer counters, and unchanged canonical-row counts. |

## Reproduction

The default invocation preserves the ticket tiers (1k, 10k, and 100k):

```sh
scripts/imo-220-warm-safety-matrix.sh \
  --evidence docs/evidence/warm-activation/imo-220-safety-local.json

scripts/imo-220-warm-claim-qualification.sh \
  --evidence docs/evidence/warm-activation/imo-220-warm-claim-local.json \
  --samples 1000
```

`--tier` is repeatable and accepts only `1000`, `10000`, `100000`, and
`1000000`; omitting it retains the three default ticket tiers. A clean external
runner can set `WORLDSTREAM_HISTORY_FIXTURE_BIN` and
`WORLDSTREAM_WARM_TEST_BIN` to executable release artifacts. The driver invokes
the test artifact with the exact
`source_backed_warm_activation_claim_qualification --ignored --nocapture`
arguments and records the supplied binary SHA-256 values in the resulting
redacted evidence, so a 1m source can be deleted after its report is retained.
