# IMO-48 SQLite recovery audit — Luna lane

Date: 2026-08-21

This lane audited the IMO-48 acceptance criteria against the executable SQLite
adapter tests. The production recovery path already had the required
fail-closed behavior; the missing proof was recovery after deleting every
paired snapshot/current materialization and the supervisor-level
passivation/reload fence. This lane adds the SQLite supervisor barrier and
wires server snapshot-backed attach, current, and action paths through it.

## Acceptance matrix

| Criterion | Executable evidence | Result |
| --- | --- | --- |
| Paired snapshot binds both state values, the complete Head, lineage, pack/schema identities, all three hashes, and is written postcommit | `postcommit_snapshot_binds_the_complete_head_and_all_state_hashes`; `snapshot_failure_after_commit_does_not_change_canonical_result_or_recovery` | Pass |
| Snapshot failure cannot change a committed canonical result | `snapshot_failure_after_commit_does_not_change_canonical_result_or_recovery`; `action_advance_is_one_atomic_bundle_and_unknown_commit_resolves_original` | Pass |
| Genesis replay works with all snapshots and current materializations removed, preserving hashes and operational projections | `recovery_rebuilds_materializations_after_every_paired_snapshot_is_removed` | Pass |
| Corrupt newest snapshot is disposable and recovery falls back to the older retained pair/immutable lineage | `corrupt_paired_snapshot_is_disposable_and_lineage_rebuild_is_exact` (the fixture retains the sequence-zero pair while the newest sequence-one pair is corrupted) | Pass |
| Missing transition, altered Genesis/Transition/pack-lock bytes, and chain/projection/hash mismatch fail closed and quarantine | `replay_verification_rejects_each_corrupt_or_missing_durable_history_component`; `recovery_quarantines_corrupt_consequences_transition_indexes_and_receipt_projections`; `recovery_rejects_missing_substituted_wrong_kind_and_wrong_hash_delivery_consequences` | Pass |
| Missing exact executor/runtime faults without masking structural corruption | `unavailable_retained_runtime_faults_but_does_not_quarantine_intact_history`; `retained_runtime_panics_and_malformed_outputs_fault_but_semantic_mismatch_quarantines` | Pass |
| Five-state lifecycle and generation fences reject stale recovery/install/publication work | `supervisor_passivation_barrier_blocks_inflight_work_until_drain`; `supervisor_lifecycle_blocks_snapshot_work_until_current_generation_is_active`; `recovery_install_rereads_exact_head_and_integrity_generation_before_yielding_trace`; `replay_membership_generation_drift_fences_without_quarantine`; `principal_scope_and_membership_generation_drift_each_fence_the_sealed_action` | Pass |
| Faulted and quarantined integrity are distinct and exposed/fenced correctly | `unavailable_retained_runtime_faults_but_does_not_quarantine_intact_history`; `missing_integrity_row_is_conditionally_recreated_as_quarantined`; `faulted_replay_rejects_and_quarantines_an_impossible_same_generation_append` | Pass |
| Crash/unknown result after commit resolves the original receipt exactly once without speculative actor install | `create_unknown_commit_withholds_trace_until_durable_reload`; `action_advance_is_one_atomic_bundle_and_unknown_commit_resolves_original`; `drop_reopen_resolves_exact_receipts_and_replays_durable_history_with_snapshot_fallback` | Pass |
| Timer recovery preserves exact generation/lifecycle and fixed-cutoff gating | `recovery_verifies_or_rebuilds_the_exact_timer_generation_ledger`; `fixed_cutoff_recovery_stays_gated_until_each_due_obligation_is_consumed`; `exact_timer_candidate_retains_witness_and_lifecycle_without_wire_payload` | Pass |
| Ordinary timer firing cannot bypass the supervisor lifecycle | `supervisor_lifecycle_blocks_snapshot_work_until_current_generation_is_active` (the focused backend test denies `fire_timer` in Loading, CatchingUp, and Passivating); `supervisor_passivation_barrier_blocks_inflight_work_until_drain` | Pass |

## Verification

Commands run from the repository root:

```text
cargo test -p worldstream-sqlite --lib
# 91 passed, 3 failed in unrelated Agent Heist tests; each fails before
# exercising this lane with retained-golden mismatch b05a682f0923001914a800072ee68348e68b979453c93e033cba7916b99a4407

cargo clippy -p worldstream-sqlite --all-targets -- -D warnings
# blocked by the same unrelated strict lints in
# crates/worldstream-core/src/agent_heist.rs (lines 2050 and 2057)

cargo fmt -p worldstream-sqlite -p worldstream-server
# passed

cargo fmt --all -- --check
# passed

cargo test -p worldstream-server --lib sqlite_backend::tests::supervisor_lifecycle_blocks_snapshot_work_until_current_generation_is_active
# passed (1 focused test)

cargo clippy -p worldstream-server --lib --all-targets -- -D warnings
# blocked by unrelated strict lints in crates/worldstream-core/src/agent_heist.rs:
# two assigning_clones diagnostics at lines 2050 and 2057

cargo clippy -p worldstream-sqlite --all-targets --no-deps -- -D warnings
# passed (crate-local strict Clippy)

cargo clippy -p worldstream-server --lib --all-targets --no-deps -- -D warnings
# passed (crate-local strict Clippy)

git diff --check -- crates/worldstream-sqlite/src/lib.rs crates/worldstream-server/src/sqlite_backend.rs docs/agents/imo-48-sqlite-recovery-luna.md
# passed
```

## Lifecycle implementation

`SqliteRoomRuntimeStateV1` now exposes the complete supervisor lifecycle:

```text
Loading -> CatchingUp -> Active -> Passivating -> Inactive
```

`SqliteRoomSupervisorV1` owns a monotonic per-Room generation. Reload creates a
strictly newer Loading lease only after an existing Passivating generation has
drained and become Inactive. Every Active operation acquires a per-Room guard;
the guard is held across snapshot reads, attach work, and action authorization
and durable commit. Passivation stops new guards immediately. An operation
already admitted by the retiring generation may publish its completed result
while the state is Passivating; a stale/reloaded generation cannot publish,
and passivation cannot finish while the in-flight count is non-zero. The
deterministic supervisor test holds an operation at a barrier, proves new work,
reload, and passivation completion are denied, then permits same-generation
publication, drains it, and proves the higher-generation Loading -> CatchingUp
-> Active path rejects the old generation.

The backend's lazy path treats an unknown or Inactive Room as Loading, calls
the durable verified `room_runtime_state` read outside the supervisor lock, and
only then advances to CatchingUp/Active. An absent Room is returned as
`NotFound`; a Room with overdue work remains `Busy`. Ordinary current, attach,
and action paths cannot auto-install an unknown Room as Active.

Normal `fire_timer` now follows the same verified Active admission and holds
its per-Room guard across timer candidate lookup, authorization, idempotent
resolution, or durable timer commit, then publishes through the guard. It
returns `Busy` during Loading, CatchingUp, Passivating, or Inactive lazy-load
verification; fixed-cutoff CatchingUp work remains the separate recovery seam.

The focused SQLite/server tests and formatting checks pass. The full SQLite
suite has three unrelated Agent Heist golden-corpus failures in the shared
worktree; no IMO-48 test failed. Full strict Clippy is externally blocked by
two unrelated `assigning_clones` diagnostics in
`crates/worldstream-core/src/agent_heist.rs`; crate-local strict Clippy with
`--no-deps` passes for both requested crates. No production blocker remains in
the requested SQLite supervisor/backend files. Confucius's live-observation
suffix edits were preserved unchanged.

## Parent authentication-order correction

Parent review found that the first lifecycle implementation consulted Room
state before authenticating current/attach/action/timer requests. That made an
unknown or passivating Room distinguishable to an unauthenticated transport
session. The backend now authenticates before any Room lifecycle/storage
lookup, while retaining bounded request-shape validation first. The lifecycle
test now bootstraps a real authenticated capability so it exercises the
Loading/CatchingUp/Passivating fences rather than relying on the old ordering.
Both the authenticated lifecycle test and the unauthenticated fail-closed test
pass together.

## Integrity-surface acceptance follow-up

The remaining integrity-surface gap is now executable in the adapter and server
paths. `SqliteRoomStore::gateway_room_snapshot` checks the durable integrity
fence before recovery; Faulted reads use a pure exact retained-history replay
and never invoke the healing recovery installer. The resulting current
Projection, retained Observation attach/catch-up barrier, and authorized Replay
carry the explicit `RoomIntegrityStateV1` status and generation. Action, timer,
observation-cursor, Runner-control, activation-scheduler, capability-authority,
and recovery-triggering mutation paths are fenced before their durable work.

Quarantined Rooms fail closed for normal current snapshot, attach, observation,
and Replay surfaces. The adapter exposes only bounded, authenticated
HostOperator diagnostics: safe summary, immutable Genesis/Transition/Head raw
export, and exact read-only verification. Member capabilities cannot obtain
these grants. Restore remains deployment-owned; `diagnostic_restore` consumes
and revalidates the typed HostOperator grant, then returns
`RestoreDeploymentRequired` rather than providing a local bypass.

`imo48_integrity_surface_keeps_faulted_reads_verified_and_quarantine_operator_only`
proves the Faulted and Quarantined read/attach boundaries, cross-capability
diagnostic denial, host diagnostics/export/verification, and explicit status /
generation. `recovery_rebuilds_materializations_after_every_paired_snapshot_is_removed`
now also compares a length-delimited BLAKE3 digest of Genesis, pack lock,
Transitions, and current Head before and after repair; only disposable
materializations/snapshots are rebuilt. The existing deterministic
`recovery_install_rereads_exact_head_and_integrity_generation_before_yielding_trace`
test proves stale-generation repair returns `ConcurrentChange`.

The Replay protocol response now includes `room_health` and
`integrity_generation`; server mapping exposes distinct `room_faulted` and
`room_quarantined` error codes. The server's read helper permits the verified
Faulted path while all normal mutation paths retain the Active lifecycle fence.

Current verification from the shared worktree:

```text
cargo check --locked -p worldstream-sqlite                         # passed
cargo check --locked -p worldstream-server                         # passed
cargo test --offline -p worldstream-sqlite                         # 95 passed
cargo test --offline -p worldstream-server                         # 61 passed
cargo test --offline -p worldstream-sqlite imo48_integrity_surface_keeps_faulted_reads_verified_and_quarantine_operator_only
                                                                    # passed
cargo test --offline -p worldstream-sqlite recovery_rebuilds_materializations_after_every_paired_snapshot_is_removed
                                                                    # passed
cargo test --offline -p worldstream-sqlite recovery_install_rereads_exact_head_and_integrity_generation_before_yielding_trace
                                                                    # passed
cargo clippy --offline -p worldstream-sqlite --all-targets --no-deps -- -D warnings
                                                                    # passed
cargo clippy --offline -p worldstream-server --lib --all-targets --no-deps -- -D warnings
                                                                    # passed
cargo clippy --offline -p worldstream-sqlite --all-targets -- -D warnings
                                                                    # passed
cargo clippy --offline -p worldstream-server --all-targets -- -D warnings
                                                                    # passed
```

`cargo fmt --all -- --check` remains blocked by the pre-existing unrelated
format drift in `crates/worldstream-conformance/src/lib.rs`; the allowed
SQLite/server/protocol edits are rustfmt-compatible. The shared `Cargo.lock`
was already dirty outside this lane, so the focused test commands use the
offline cache where Cargo's lockfile freshness check would otherwise reject
the pre-existing lock drift. No Linear status was changed.
