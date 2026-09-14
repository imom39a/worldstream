# Bounded-recovery evidence

These reports exercise the production Core, SQLite, and PostgreSQL recovery
paths. They are engineering evidence and do not claim release qualification.

## Current V3 evidence

V3 checkpoints store three frozen operational roots plus their MMR receipts.
Recovery authenticates only the logarithmic proof nodes needed for the current
operational rows. It does not reconstruct an inventory of every historical
Frame, consequence, or Activation decision.

The current 100k SQLite evidence is retained at
[`../long-history/sqlite-100000-bounded-executor.json`](../long-history/sqlite-100000-bounded-executor.json).
It reports a 2,471-byte checkpoint witness, at most six MMR peaks per domain,
one boundary Transition read, zero prefix or tail Transitions delivered to
Core, zero retained historical Transitions in the installed executor, and a
13,320,192-byte RSS sample.

The final PostgreSQL/PgBouncer report is `postgres-17-pgbouncer.json`, SHA-256
`deb622e4e3b8cc3021cc23119e1a581d238676b2c389a7674d095c9b3f8d8024`.
It records an overall pass with no errors against pinned PostgreSQL 17.11 and
the pinned PgBouncer image.

| PostgreSQL history | Recovery | V3 witness | RSS after setup | Prefix delivered | Adapter Transition reads | Reducer callbacks |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 127 ms | 2,450 bytes | 22,085,632 bytes | 0 | 1 | 0 |
| 10,000 | 124 ms | 2,346 bytes | 23,216,128 bytes | 0 | 1 | 0 |
| 100,000 | 42 ms | 2,471 bytes | 1,418,903,552 bytes | 0 | 1 | 0 |

The 100k setup first used the public V2 whole-deployment transfer and one
authoritative full replay to capture a V3 checkpoint. Its RSS sample therefore
includes that one-time full replay. The subsequent measured recovery was the
ordinary V3 path and skipped all 100,000 prefix Transitions.

Every tier exactly matched Head, Core state, activity state, Timer ledger,
Frame heads, retained Frames, consequences, Membership generations, and
Activation decisions. The bounded path did not read semantic receipts. The
provider lane also passed tamper fallback, missing-materialization rebuild,
malformed-Head quarantine, exact install fencing, and the current PostgreSQL
snapshot cadence contract.

## Legacy V2 comparison artifacts

The retained `sqlite-1000.json`, `sqlite-10000.json`, and
`sqlite-100000.json` reports document the earlier V2 design. Their witnesses
grew with retained consequences: 135,280 bytes at 1k, 1,354,783 bytes at 10k,
and 13,639,786 bytes at 100k. These reports remain useful regression evidence,
but V3 replaces the linear witness with compact authenticated MMR receipts.

## Reproduction

```sh
cargo run --locked --quiet --release -p worldstream-sqlite \
  --example history_qualification_fixture -- \
  --database "$DB" --transition-count "$COUNT" \
  --output "$REPORT" --stream-metadata

WORLDSTREAM_PG_LIVE_DEBUG_DIR=/tmp/worldstream-pg-evidence \
WORLDSTREAM_POSTGRES_WARM_CLAIM_SAMPLES=1000 \
scripts/postgres-live-evidence.sh \
  --evidence docs/evidence/warm-activation/imo-220-postgres-live-local.json
```

The PostgreSQL script removes its disposable containers and records
`secrets_emitted=false` on success.
