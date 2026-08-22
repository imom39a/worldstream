# IMO-53 Heist reducer parent-review correction

## Scope

This lane adds direct production-pack Rust evidence in the privacy and registry
test modules, plus the independent Python fixture as supplemental evidence. No
production defect was found, so `agent_heist.rs` and deterministic fixture
digests were not changed.

## Executable evidence

| IMO-53 criterion | Direct evidence |
| --- | --- |
| Departed/suspended immutable seat, missing denominator, no knowledge transfer | `suspended_or_departed_seat_stays_immutable_but_is_not_enabled_or_replaced` exercises both `Suspended` and `Departed`, checks fixed seat identity and role binding, disabled/absent projection, private-clue absence, unchanged three-seat denominator, and missing Broker scoring. |
| Briefing/Negotiation actions and stable declared rejections | `briefing_and_negotiation_actions_exercise_declared_success_and_rejections` executes `INSPECT_CLUE`, `PUBLISH_CLUE`, `OFFER_EXCHANGE`, `ACCEPT_EXCHANGE`, `PROPOSE_PLAN`, `ENDORSE_PLAN`, and `CHALLENGE_PLAN` through the production reducer, then exercises each in `Result` and asserts stable `wrong_phase` rejection with unchanged state. Existing `duplicate_commitment_and_exact_deadline_are_rejected_without_mutation` covers commitment admission/reminder, generation fences, deadline, and resolve ordering. |
| Golden score thresholds, including 3/5 and existing 4/5 | Existing `resolve_timer_records_explicit_three_of_five_partial_score` proves the 3/5 boundary; existing `absent_broker_outcome_matrix_is_deterministic_and_fail_closed` proves 4/5, and `golden_majority_and_score_matrix_covers_missing_split_and_partial_votes` covers neighboring matrix rows. No duplicate matrix was added. |
| Result → Complete, cancellation, duplicate/obsolete generations | `result_to_complete_cancels_timer_and_rejects_duplicate_or_obsolete_generations` proves duplicate acknowledgement rejection, Complete transition, exact cancellation of the active generation, and obsolete timer rejection without state mutation. Existing timer tests cover the broader generation/reminder matrix. |
| Genesis-fold Replay exactness | `production_registry_replay_folds_genesis_states_events_timers_heads_and_hashes` uses the registry's production golden corpus and pack, folds Genesis plus a real action and timer transition, and compares exact state/activity/events/timers/Heads, lineage bytes, and hashes against replay. |
| No Heist kernel branch | The Rust additions are tests only; no Heist kernel branch or production reducer behavior was changed. The Python fixture's no-branch check remains supplemental. |

Process-level crash-after-commit is not claimed here: the available SQLite/server
evidence is mapped only as runtime context, and no executable test in this lane
proves the exact process-kill acceptance wording.

## Verification

Commands run from the repository root:

```text
cargo test --locked -p worldstream-core agent_heist_privacy_tests -- --nocapture
cargo test --locked -p worldstream-core agent_heist_registry -- --nocapture
cargo test --locked -p worldstream-core --lib
cargo clippy --locked -p worldstream-core --all-targets --no-deps -- -D warnings
cargo fmt --all -- --check
rustfmt --edition 2024 --check crates/worldstream-core/src/agent_heist_privacy_tests.rs crates/worldstream-core/src/agent_heist_registry.rs
git diff --check -- crates/worldstream-core/src/agent_heist_privacy_tests.rs crates/worldstream-core/src/agent_heist_registry.rs examples/heist docs/agents/imo-53-heist-reducer-luna.md
PYTHONPATH=examples/heist uv run --project sdk/python --locked python -m unittest -v examples/heist/test_imo53_contract.py examples/heist/test_story.py examples/heist/test_fresh_checkout.py examples/heist/test_privacy_matrix.py
uv run --project sdk/python --locked ruff check examples/heist
uv run --project sdk/python --locked ruff format --check examples/heist/story.py examples/heist/test_imo53_contract.py
```

Final results: the focused privacy suite passed 18 tests, the registry suite
passed 3 tests, and the full core library passed 150 tests. Strict core Clippy,
lane-scoped Rustfmt, and the scoped diff check passed. `cargo fmt --all --
--check` remains non-zero only because of pre-existing formatting diffs in
`crates/worldstream-server/src/lib.rs` and `crates/worldstream-server/src/sqlite_backend.rs`,
which are outside this lane's write scope. The Python suite passed 20 tests,
full examples Ruff passed, and the scoped Ruff format check passed. Whole-directory
Ruff format still has the pre-existing unrelated
`examples/heist/wave10_live/seed_browser_room.py` issue, which remains outside
this lane's write scope.
