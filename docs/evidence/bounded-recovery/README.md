# Local bounded-recovery evidence

These redacted reports were produced on 2026-09-13 through the production
Core/SQLite and Core/PostgreSQL storage paths. They are local engineering
evidence and do not claim hosted-provider or release qualification.

The SQLite fixture is reproducible with:

```text
cargo run --locked --quiet --release -p worldstream-sqlite \
  --example history_qualification_fixture -- \
  --database "$DB" --transition-count "$COUNT" \
  --output "$REPORT" --stream-metadata
```

| Report | SHA-256 | Recovery | RSS | Consequences | Witness |
| --- | --- | ---: | ---: | ---: | ---: |
| `sqlite-1000.json` | `c9bf6e04c02dd7a74281bdba0bd9f6cd6c20a7c93e4d4575d69f01260d5f6586` | 8 ms | 23,314,432 | 1,000 | 135,280 bytes |
| `sqlite-10000.json` | `bf5a78e3b3a94750eb7b245daa13f8bb6b6cd9a923ac894ae8b27857986abdd2` | 79 ms | 127,107,072 | 10,000 | 1,354,783 bytes |
| `sqlite-100000.json` | `fc52a931cb20489751456fb53c56d3098f0ac877df3b36dba03d77c250e3524d` | 843 ms | 775,733,248 | 100,000 | 13,639,786 bytes |

Every SQLite tier used the completed `checkpoint` execution path. The
receipt-backed accounting records one checkpoint-boundary Transition row read
by the adapter, zero prefix range reads, zero prefix records delivered to Core,
zero tail records, one total adapter Transition read, and zero reducer
callbacks. Each report also contains exact row-for-row comparisons for Timers,
frame Heads, retained Frames, observation consequences, Membership generations,
and Activation decisions. A report cannot set `pass=true` unless all those
checks succeed.

`postgres-17-pgbouncer.json` has SHA-256
`f2c67a90208b785a2013de815ff468f851ca297bd6affe87babcac4ab6cd168f`.
It is the output of `scripts/postgres-live-evidence.sh` and records an overall
pass with no errors against PostgreSQL 17.11. The report covers direct runtime,
transaction-pooled PgBouncer, the production gateway, shared conformance,
checkpoint/tamper recovery, transfer, and cleanup.

| PostgreSQL transitions | Recovery | RSS | Prefix delivered | Adapter Transition reads | Reducer callbacks |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 192 ms | 26,787,840 | 0 | 1 | 0 |
| 10,000 | 887 ms | 59,097,088 | 0 | 1 | 0 |
| 100,000 | 1,185 ms | 1,419,444,224 | 0 | 1 | 0 |

The 1k and 10k PostgreSQL tiers were committed directly through the production
adapter. The 100k tier used a fresh public v2 SQLite-to-PostgreSQL transfer,
then one explicit full verified replay to create the disposable checkpoint.
The measured recovery after that setup was an ordinary receipt-gated
checkpoint recovery. All three PostgreSQL tiers report exact Head, state, and
operational witnesses and prove that semantic receipts were not read by
bounded recovery.

The live lane also proved that authoritative full recovery recreates an absent
current materialization inside the exact healthy Head and integrity-generation
fence. Existing mismatched materializations remain corruption. Malformed Head
bytes quarantine through the exact raw-byte fence in ordinary recovery, and a
separate fresh-container regression proves the same behavior for the explicit
checkpoint-rebuild maintenance API. The same focused lane corrupts the newest
disposable snapshot and proves full authoritative fallback while the Room
remains healthy at its original integrity generation.

The scale report was generated immediately before that final cache-isolation
follow-up. The follow-up removes disposable snapshot validation and its
comparison vector only from the one-time authoritative full-recovery setup; it
does not change the measured checkpoint recovery path or its read/callback
accounting. The final behavior is covered by the focused fresh-container test
above, so the recorded 100k RSS is a conservative pre-optimization value.

The reports contain no credentials or private payloads.
