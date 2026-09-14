# IMO-234 acceptance matrix

This ledger records the completed finite engineering qualification. It is not a
release claim; local generated reports retain `release_evidence=false`.

| Acceptance criterion | Evidence | Result |
| --- | --- | --- |
| Published proof design | [ADR 0037](../adr/0037-successor-anchored-operational-receipts.md) defines V3 witness binding, MMR leaf and node encodings, atomic updates, trust anchor, compatibility, retention, transfer, and fail-closed behavior. | Passed |
| Capture and recovery bounds are independent of retained history | V3 reports use three fixed-size receipts plus logarithmic peaks. SQLite 100k uses a 2,471-byte witness and one boundary read. PostgreSQL 1k/10k/100k uses 2,450/2,346/2,471-byte witnesses, one boundary read, zero prefix/tail delivery, and zero reducer callbacks. The 1m MMR proof needs at most 25 nodes. | Passed |
| Independently scale operational domains past the V2 limit | `each_root_domain_advances_beyond_sixteen_mebibytes_without_retaining_history` advances each rooted domain beyond 16 MiB. `v2_checkpoint_capture_declines_timer_and_member_serving_cache_overflow` separates unbounded retained Timer/Membership source bytes from their capped current serving sets. Local MMR reports cover 1k/10k/100k/1m leaves. | Passed |
| Tamper, omission, ordering, duplication, domain, cause, and stale-proof behavior | Core MMR tests reject altered leaves, paths, domains, indexes, roots, malformed coordinates, and stale receipts. SQLite and PostgreSQL reject coordinated payload-plus-hash substitution. The V2 fallback matrix still covers receipt omission, stale counts/hashes, malformed, duplicate, extra, and reordered encodings for legacy histories. | Passed |
| Exact Head and integrity fencing | `recovery_install_rereads_exact_head_and_integrity_generation_before_yielding_trace`, `runtime_state_retries_after_a_concurrent_timer_commit`, and the live PostgreSQL exact-fence lane cover concurrent commit, Timer update, recovery install, and checkpoint replacement. | Passed |
| Full-replay equivalence | `checkpoint_and_forced_full_replay_preserve_timer_delivery_and_receipt_materializations` compares exact durable Timer, delivery, decision, and receipt tuples. V3 qualification compares Head, Core, activity, Timer, Frame, consequence, Membership, and Activation-decision state. | Passed |
| Equivalent SQLite and PostgreSQL evidence | [SQLite 100k](long-history/sqlite-100000-bounded-executor.json) and [PostgreSQL 17/PgBouncer](bounded-recovery/postgres-17-pgbouncer.json) both complete checkpoint recovery with exact current state and no prefix delivery. | Passed |
| Backup and transfer completeness needed by the proof contract | Backup, SQLite native stream, transfer preflight, and PostgreSQL publication now carry the frozen V2 roots before their count-matched MMR receipts. The final 100k transfer preserves 3 roots, 3 receipts, and 199,994 nodes. | Passed |

## Scale artifacts

| Artifact | SHA-256 | Key bound |
| --- | --- | --- |
| `long-history/sqlite-100000-bounded-executor.json` | `940c3cd19c930ae5a251fd35c321af5e41f7729ac1087814f430cec31d6b8670` | 100k prefix skipped; 2,471-byte witness; zero retained cached history |
| `long-history/postgres-100000-transfer-recovery-local.json` | `5f7373260e96e5cd89837982fa843b1fba2e2eac2e656e5b445d1a54700d0a64` | 100k SQLite-to-PostgreSQL transfer followed by 42 ms V3 recovery |
| `bounded-recovery/postgres-17-pgbouncer.json` | `deb622e4e3b8cc3021cc23119e1a581d238676b2c389a7674d095c9b3f8d8024` | Direct and pooled provider qualification, 1k/10k/100k |
| `operational-mmr/local-1000000.json` | `e9505e12f86a5366d600bd6ac1fede2349260dfaf5329ae051a530f476163749` | 1,999,993 immutable nodes; at most 25 proof nodes |

The completed implementation and qualification harness are at
`eab6d8c18187f7bc7fb8f145ab7fcaca369a71c8`. Commits `40357cc3` and `eab6d8c1`
also keep healthy SQLite Gateway cache installation on guarded V3 recovery plus
one bounded serving-fence transaction. Full replay remains available for
legacy histories, authoritative repair, and forensic verification; it is not
part of an ordinary eligible V3 checkpoint recovery or healthy Gateway cache
installation.
