# IMO-48 SQLite snapshot and recovery evidence

The SQLite adapter now stores an optional paired snapshot cache in migration
`0005-paired-snapshots-v1`. Migration `0004-activation-work-v1` and its
activation schema remain unchanged.

Each cache row is keyed by `(room_id, room_seq)` and stores the paired Core and
Activity bytes together with the complete Head bytes, Head sequence, lineage
hash, Core schema version, exact pack digest, Core hash, Activity hash, and
aggregate Authoritative State hash. Rows are immutable and the automatic cache
retains at most the newest three pairs. The Genesis and Transition rows remain
the sole canonical lineage.

Create and accepted Advance commit their canonical transaction first. A
separate idempotent transaction then attempts to persist the paired cache. A
cache write failure is deliberately ignored by the canonical result path, so a
durable Transition, Head, receipt, timer ledger, membership projection, and
delivery/activation consequences cannot be rolled back or reclassified because
the disposable cache was unavailable.

Recovery continues to verify Genesis and every ordered Transition with the
exact retained runtime. It reconstructs the current Head, Core/Activity state,
timer generations, memberships, and delivery consequences, then installs
replaceable materializations only under the captured healthy integrity
generation and exact Head fence. Missing or corrupt snapshots therefore fall
back to immutable lineage replay; repair does not edit canonical history.

Focused evidence in `crates/worldstream-sqlite/src/lib.rs`:

- `postcommit_snapshot_binds_the_complete_head_and_all_state_hashes`
- `snapshot_failure_after_commit_does_not_change_canonical_result_or_recovery`
- `corrupt_paired_snapshot_is_disposable_and_lineage_rebuild_is_exact`
- `drop_reopen_resolves_exact_receipts_and_replays_durable_history_with_snapshot_fallback`
- `recovery_install_rereads_exact_head_and_integrity_generation_before_yielding_trace`
- `replay_verification_rejects_each_corrupt_or_missing_durable_history_component`
- `unavailable_retained_runtime_faults_but_does_not_quarantine_intact_history`

Verification commands:

```text
cargo fmt --all
cargo test -p worldstream-sqlite --lib
cargo clippy -p worldstream-sqlite --all-targets -- -D warnings
```

The full release verifier, PostgreSQL parity, operator diagnostic/export/restore
surfaces, and activation implementation are outside IMO-48's SQLite recovery
write scope and are not claimed by this evidence.

## Parent lane continuation (2026-08-21)

The recovery fence now admits a previously `faulted` Room for a fresh exact
replay. A successful guarded install transitions `faulted` to `healthy` under
the next integrity generation; a repeated runtime failure leaves the same
faulted generation fenced, and corruption escalates it to `quarantined`.
Quarantined Rooms remain unavailable to this recovery path.

Recovery also reads the current-head paired snapshot as a disposable cache
candidate. It validates the schema marker, complete Head, all stored hashes,
and the exact Genesis/Transition record before using the Core/Activity bytes
to rebuild missing materializations. A missing or corrupt cache row is a
cache miss and falls back to immutable Genesis plus Transition replay.
