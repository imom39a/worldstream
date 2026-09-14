# IMO-234 acceptance matrix

This is a verification ledger, not a release claim. A row is complete only
when the named command has passed against the stated provider.

| Acceptance row | Evidence | Status |
| --- | --- | --- |
| V2 checkpoint boundary witness, tail replay, and final state | `history_qualification_fixture --transition-count 10 --stream-metadata` reports `checkpoint_v2`, hash/head/operational exactness, and a 10-record tail | SQLite passed |
| SQLite V2 tamper fallback | `cargo test -p worldstream-sqlite checkpoint -- --nocapture` | SQLite passed |
| Recovery replacement and concurrent commit/timer fence | `recovery_install_rereads_exact_head_and_integrity_generation_before_yielding_trace` and `runtime_state_retries_after_a_concurrent_timer_commit` | SQLite passed |
| PostgreSQL V2 direct recovery scale | disposable PG17 adapter log: 1k and 10k checkpoint rows, exact state/core/activity and root-backed operational counts | Passed before cadence audit; final clean matrix pending |
| PostgreSQL cadence direct and PgBouncer | `scripts/postgres-live-evidence.sh` after `964f610e` | Pending clean rerun |
| 100k transfer-backed PostgreSQL recovery | live script transfer-backed lane | Pending IMO-225 rerun and clean matrix |
| Independently scaled Frames, consequences, and activation-decision roots beyond 16 MiB | `cargo test -p worldstream-core each_root_domain_advances_beyond_sixteen_mebibytes_without_retaining_history -- --nocapture` | SQLite/Core passed |
| Timer-ledger and membership source bytes beyond 16 MiB with bounded serving caches | `cargo test -p worldstream-sqlite v2_checkpoint_capture_declines_timer_and_member_serving_cache_overflow -- --nocapture` inserts 17 MiB in each retained source while V2 captures only bounded current timers and member heads; the same test rejects 1,025 current timer or member rows | SQLite passed |
| Per-domain root-hash tamper fallback | `cargo test -p worldstream-sqlite each_v2_operational_root_tamper_falls_back_without_quarantine -- --nocapture` for Frames, consequences, and activation decisions | SQLite passed; omit/reorder/duplicate/stale matrix remains open |
| Full-replay equivalence of timers, delivery, decisions, and receipts | `cargo test -p worldstream-sqlite checkpoint_and_forced_full_replay_preserve_timer_delivery_and_receipt_materializations -- --nocapture` compares exact persisted tuples after a forced V2 fallback; `counter_v4_human_ack_commits_two_targeted_activation_decisions_and_recovers_lineage` proves a nonempty decision set | SQLite passed |

The outstanding rows deliberately remain open. The serving cache contract is
intentionally different from the retained forensic ledger: timers and
members are capped at 1,024 current entries, while historical timer and
membership source bytes can exceed 16 MiB without entering the V2 witness.
