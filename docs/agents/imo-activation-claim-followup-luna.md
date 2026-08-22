# IMO activation claim follow-up

Date: 2026-08-21

## Interim diagnosis

The prior `room_runtime_state` retry fixed only the later recovery read in the
claim path. The claim first calls `SqliteRoomStore::prepare_activation_claim`,
which calls `gateway_room_snapshot`. That snapshot performed one un-retried
`recover_room` pass. If a TimerFired commit advanced the Room Head while that
recovery was installing its verified materializations, Core returned the typed
`RoomRecoveryErrorV1::ConcurrentChange`. `map_gateway_recovery_error` then
converted it to a gateway concurrent-change error, and the activation adapter
collapsed the result to `SqliteActivationErrorV1::StorageUnavailable`; the
public server consequently emitted `storage_unavailable`.

This was a transient recovery fence race, not SQLite corruption. The fix is a
bounded retry of only `RoomRecoveryErrorV1::ConcurrentChange` in
`gateway_room_snapshot`, with two retries after the initial attempt. Each retry
re-enters complete verified recovery. No integrity, authority, membership,
lease, context, or claim fence is bypassed, and no client retry was added.

## Deterministic evidence

`tests::gateway_snapshot_retries_after_a_concurrent_timer_commit` pauses the
real recovery install guard, commits the exact first Heist TimerFired, releases
the guard, and calls the real gateway snapshot seam. Without the retry it fails
with:

```text
gateway snapshot after TimerFired race: Room changed concurrently
```

With the retry it returns a snapshot whose Head equals the committed TimerFired
Head.

## Changes

- `crates/worldstream-sqlite/src/lib.rs`
  - bounded `ConcurrentChange` retry in `gateway_room_snapshot`;
  - deterministic gateway-snapshot race regression test.
- `docs/agents/imo-activation-claim-followup-luna.md`
  - this report.

No server adapter change was required. Temporary tagged diagnostics used during
the investigation were removed.

## Verification

Passed:

```text
cargo test --locked -p worldstream-sqlite --lib
# 89 passed; 0 failed

cargo clippy --locked -p worldstream-sqlite --lib --tests -- -D warnings
# passed

rustfmt --edition 2024 --check crates/worldstream-sqlite/src/lib.rs
# passed
```

The unchanged public harness was run once after rebuilding `worldstreamd`. It
still returned a blocked live result. Temporary typed diagnostics showed that
this post-fix run reached a separate `RoomRecoveryErrorV1::Corrupt` during the
claim snapshot, and the Room's durable integrity row was `quarantined` at
generation 2. That is fail-closed behavior and is not treated as evidence for
another retry or as a successful claim. The remaining live blocker requires a
separate Heist recovery-corruption investigation; this follow-up does not
weaken the corruption fence or fabricate a claim.
