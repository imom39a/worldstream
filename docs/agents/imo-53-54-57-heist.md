# IMO-53 / IMO-54 / IMO-57 Agent Heist evidence

This note records the pack-only Heist semantics evidence in the current
worktree. It is deliberately separate from host, storage, runner, and process
durability evidence.

## Implemented behavior

The Heist reducer now accepts the pack's `RESOLVE_TIMER` for the `resolve_now`
purpose. Previously the reducer scheduled that timer on Resolution entry but
rejected it as an unknown timer before reaching the existing Resolution to
Result transition.

The executable fixtures cover:

- the six Activity Phases and their monotonic phase generations;
- deadline closure with a deliberately absent Broker;
- strict-majority selection without synthesizing a missing-seat vote;
- exact Result assertions: two votes for `plan-1`, Broker in `missing_roles`,
  score 5, and `success`;
- no-majority/split, partial-score, zero/one-vote, and deterministic replay
  outcome cases;
- duplicate commitment rejection (`prior_commitment`), exact deadline
  rejection (`wrong_phase`), and unchanged state after duplicate rejection;
- timer witness and generation fencing, including replay of an already
  consumed phase timer;
- canonical state restart/reduction parity;
- participant, historical participant, public, historical public, operator,
  and historical operator privacy projections, including sealed commitment
  and fixture-truth exclusion.

The reducer remains an ordinary `ActivityPackV1` implementation. No branch
depends on the operating system, scheduler implementation, database backend,
Runner, or model provider.

## Verification

Run from the repository root:

```text
rustfmt --edition 2024 --check \
  crates/worldstream-core/src/agent_heist.rs \
  crates/worldstream-core/src/agent_heist_privacy_tests.rs
cargo test --locked -p worldstream-core agent_heist_privacy_tests -- --nocapture
cargo clippy --locked -p worldstream-core --lib --tests -- -D warnings
git diff --check -- \
  crates/worldstream-core/src/agent_heist.rs \
  crates/worldstream-core/src/agent_heist_privacy_tests.rs
```

Observed results on 2026-08-20:

- the focused Heist privacy/timer suite: 13 passed, 0 failed;
- scoped rustfmt check: passed;
- scoped Clippy with `-D warnings`: passed;
- scoped diff check: passed.

The broader `worldstream-core` filter also runs the registry golden test. The
parent synchronized the fixed transcript digest after the `RESOLVE_TIMER`
executor correction; the registry golden test now passes. The resulting pack
digest is still not promoted to the release manifest, which remains
fail-closed and unresolved.

## Evidence boundaries

The tests above prove pure pack reduction, canonical timer behavior, privacy
projections, and state-only replay parity. They do not prove durable
Activation lease claim/renew/reclaim, crash after database commit, process
kill/power loss, live transport delivery, UI behavior, or cross-backend
SQLite/PostgreSQL recovery. Those require host/runtime integration fixtures and
remain unclaimed here, as required by the pack boundary.
