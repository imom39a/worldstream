# IMO-226 activation backlog qualification

This evidence qualifies the bounded refresh-attention policy in
[ADR 0030](../../adr/0030-bounded-refresh-activation-attention.md) at commit
`1bbca49d32ca6071e57d51586423dd8b125909cb`.

## Environment

- Test date: 2026-09-13
- SQLite: the version locked by `Cargo.lock`, using an in-memory database
- PostgreSQL: `postgres:17.11-alpine`
- PostgreSQL image digest:
  `sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73`
- Both tests ran from a clean detached worktree at the commit above.

## Commands

```sh
cargo test -p worldstream-sqlite \
  sustained_refresh_burst_keeps_live_attention_bounded_and_preserves_obligations \
  -- --nocapture

WORLDSTREAM_POSTGRES_TEST_DSN="$DSN" cargo test -p worldstream-postgres \
  live_sustained_refresh_burst_is_bounded_and_preserves_leases_obligations_and_timers \
  -- --nocapture
```

The PostgreSQL test connects through the production runtime connection path,
uses temporary tables with the production activation schema, and calls the
same `supersede_refresh_intents` and `postgres_activation_offers` functions as
the deployed provider. The temporary transaction is rolled back.

## Results

| Backend | Arrivals | Maximum live refreshes | Maximum live bytes | Superseded | Age-retired | Recent executions | Offers after limit | Obligation | Existing lease generation | Scheduled timers | Elapsed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: |
| SQLite | 10,000 | 64 | 8,192 | 9,936 | covered by focused policy test | covered by provider contract tests | covered by provider contract tests | pending | 7 | 1 | 36,438 ms |
| PostgreSQL 17 | 10,000 | 64 | 8,192 | 9,936 | 1 | 60/min | 0 | pending | 7 | 1 | 102,858 ms |

Both sustained-arrival runs passed. They assert the queue limit after every
arrival, so the reported maximum is observed throughout the burst rather than
only at the end. The PostgreSQL run also simulates an outage-age refresh and a
saturated execution window before calling the production offer path.

The tests preserve a deadline-bearing obligation, an already leased refresh,
its lease generation, and a scheduled Timer while 10,000 new refreshes arrive.
Existing receipt, one-live-lease, stale-generation, expiry, revocation, and
context-retirement tests continue to cover exact completion fencing. The
backend status surfaces expose pending count and bytes, oldest pending age,
supersession count, age-retirement count, recent execution count, and scheduled
Timer count with the same terminal disposition vocabulary.

## Defect found

The live PostgreSQL run initially failed while decoding pending attention
bytes. PostgreSQL returns `numeric` for `sum(bigint)`, while the provider read
an `i64`. The production queries now cast the aggregate to `bigint`. The clean
PostgreSQL 17 run above passed with that correction.
