# Activation claim blocker: SQLite recovery fence race

Date: 2026-08-21

## Diagnosis

The first Runner claim's `storage_unavailable` was not SQLite corruption or a
failed TimerFired commit. The exact sequence is:

1. `room_runtime_state` starts verified recovery and captures the current
   Room Head/integrity generation.
2. The first Heist TimerFired commits successfully through the normal atomic
   Existing-Room path and advances the Head.
3. Recovery reaches its guarded install with the older Head witness. The
   SQLite guard correctly rejects it as `RoomRecoveryErrorV1::ConcurrentChange`.
4. The runtime-state caller previously returned that closed recovery error
   immediately. The server's existing runtime-state adapter mapping collapses
   every such error to HTTP `storage_unavailable`.

The race is therefore a transient recovery/claim admission conflict. The
authority, lease, recovery Head fence, and TimerFired atomicity were working as
designed. A serial recovery after the accepted TimerFired returns the room's
actual gated state (`CatchingUp` while the next due timer remains), rather than
an unavailable-storage condition.

## Deterministic reproduction

`tests::runtime_state_retries_after_a_concurrent_timer_commit` uses the real
embedded Agent Heist registry and a valid committed Heist Genesis. It pauses
SQLite immediately before the recovery install guard, commits the first exact
Genesis TimerFired while recovery is paused, then releases the guard.

Before the fix, the test failed with:

```text
durable Room Head or integrity generation changed during recovery
```

The TimerFired result was `TransitionCommitted { status: New }`; only the
stale recovery attempt failed.

## Fix

`SqliteRoomStore::room_runtime_state` now retries only
`RoomRecoveryErrorV1::ConcurrentChange`, with two retries after the initial
attempt. The retry re-enters the complete recovery and guarded-install path;
it does not bypass verification, authority, integrity, lease, or claim fences.
All other recovery failures, and persistent contention after the bounded
retry budget, retain their prior error behavior.

## Verification

Passed:

```text
cargo test --locked -p worldstream-sqlite --lib
# 88 passed; 0 failed

cargo clippy --locked -p worldstream-sqlite --lib --tests -- -D warnings
# passed

rustfmt --edition 2024 --check crates/worldstream-sqlite/src/lib.rs
# passed
```

The public Heist harness and server were not modified. No Heist harness,
server provisioning, UI, packaging, or Linear changes belong to this fix.
