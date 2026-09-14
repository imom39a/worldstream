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
| Every operational domain independently beyond 16 MiB | dedicated Frames, consequences, timers, memberships, and activation-decision generators with bound assertions | Missing |
| Per-domain omit/reorder/duplicate/stale tamper | provider parity tests for every root domain | Missing |
| Full-replay equivalence of timers, delivery, decisions, and receipts | independent full-replay comparator | Missing |

The outstanding rows deliberately remain open. Existing 100k fixture output
only independently exceeds 16 MiB for consequences and must not be used to
claim the other domains.
