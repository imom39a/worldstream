# IMO-220 / IMO-222 closure evidence

This note records the locally reproducible evidence for warm SQLite Activation
claims and verified checkpoint recovery. The measurements use the production
`SqliteGatewayBackend::activation_claim` path and the same cache and lifecycle
fences used by the gateway.

## Warm Activation claims

The regression `repeated_warm_activation_claims_do_not_recover_or_read_history`
warms the Room executor, poisons an unread historical row, forbids recovery
fallback, and repeats an idempotent claim. It asserts that the Activity reducer
callback count and canonical Transition-row count are unchanged. An opt-in scale
harness prints p50/p95/p99 latency samples for any requested claim counts:

```text
WORLDSTREAM_WARM_CLAIM_SCALES=1000 cargo test -p worldstream-server repeated_warm_activation_claims_do_not_recover_or_read_history --lib -- --nocapture
```

On the reference checkout, the 1,000-sample run completed with:

```text
warm_activation_claims scale=1000 history_rows=4 p50_us=9602 p95_us=11159 p99_us=15439
```

The 10,000 and 100,000 samples were not completed in this environment after the
1,000-sample result showed approximately 9.6 ms median per database-backed claim.
The harness can be invoked for those scales with
`WORLDSTREAM_WARM_CLAIM_SCALES=10000,100000` plus the command above; no
unmeasured latency claim is made.
The existing server test suite covers stale lifecycle, due Timer, authority,
Membership, delivery, and integrity fences separately. PostgreSQL has no live
provider configured in this checkout; its provider-neutral fixture and compile
checks remain the available parity evidence.

## 100k checkpoint fixture

The source-bound fixture generator is build-checked and emits the exact
`tail_recovery_transition_count`, `tail_recovery_elapsed_ms`, and reducer
callback fields. Run it against a daemon-created database and authority secret
with:

```text
cargo run -p worldstream-sqlite --example reference_snapshot_tail_fixture -- --database "$DB" --authority-secret-file "$SECRET" --output "$REPORT" --room-id "$ROOM_ID" --member-id "$MEMBER_ID" --transition-count 100000
```

That fixture was not executed in this checkout because no daemon-created
reference database and authority secret were available. Therefore this change
reports no measured 100k setup or recovery time; the fixture's exact two
Transition tail is enforced by its `TAIL_TRANSITIONS` constant and report
validation, while the bounded recovery test enforces the 250-row ceiling.

## Verified checkpoint recovery

`verified_checkpoint_recovery_reads_only_the_bounded_tail` verifies a paired
checkpoint and replays only the retained tail. Missing, stale, corrupt,
forged-internally-consistent, and out-of-window checkpoints remain safe cache
misses or integrity failures. PostgreSQL intentionally retains full Genesis
replay because it does not have checkpoint-keyed operational witness rows.
SQLite likewise falls back to Genesis when a stale checkpoint cannot prove
live-head Timer, observation frame/consequence, or membership-generation
projections at the checkpoint cut. ADR 0031 records this trust boundary and
the required future witness schema.

Focused validation from this isolated worktree:

```text
cargo test -p worldstream-sqlite sqlite_migration_inventory_is_contiguous_and_checksum_stable --lib
cargo test -p worldstream-sqlite verified_checkpoint_recovery_reads_only_the_bounded_tail --lib
cargo test -p worldstream-server repeated_warm_activation_claims_do_not_recover_or_read_history --lib
cargo test -p worldstream-postgres migrations --lib
cargo test -p worldstream-postgres --test postgres_commit --no-default-features
cargo check -p worldstream-server -p worldstream-postgres -p worldstream-transfer
```
