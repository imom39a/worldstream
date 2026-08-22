# IMO activation claim corruption follow-up

Date: 2026-08-21

## Result

This was a production persistence/recovery defect in `crates/worldstream-sqlite`,
not a harness or fixture contract violation.

The remaining public claim blocker was caused by the recovery verifier treating
any durable `activation_decisions` row as corruption. Normal Room commits write
one such row for each replay-produced Attention signal in the same transaction
as the causing Transition. The first Heist `propose_plan` transition produces
an `endorsement_requested` Attention, so a later claim snapshot necessarily
encounters a non-empty `activation_decisions` table.

Before this fix, `verify_inspection_projections` and the guarded recovery path
both executed:

```sql
SELECT count(*) FROM activation_decisions WHERE room_id = ?1
```

and returned `RoomRecoveryErrorV1::Corrupt` whenever the count was nonzero.
The recovery coordinator then durably quarantined the Room under its exact
Head/integrity fence, advancing the integrity row from generation 1 to
generation 2. The public gateway mapped that closed corruption result to
`storage_unavailable`. Retrying it would have been invalid.

## Bounded reproduction

The regression test
`recovery_accepts_activation_decisions_from_a_committed_heist_transition`
uses the real SQLite commit and gateway snapshot seams:

1. Create a valid Heist Room.
2. Commit the first Heist TimerFired transition.
3. Commit a valid Navigator `propose_plan` transition.
4. Confirm the durable `activation_decisions` count is nonzero.
5. Recover through `gateway_room_snapshot`.

Before the fix, step 5 returned `durable Room history or materialization is
corrupt`. After the fix it returns the committed Head. The fixture contains no
bearers, private contexts, or emitted secrets.

The surrounding canonical checks still verify the stored Head, Transition
bytes, Transition hashes, Core/Activity hashes, receipts, timers, frames, and
materializations. The fix does not downgrade corruption to retry and does not
remove quarantine behavior.

## Fix

The zero-row assertion was replaced with exact operational verification:

- decode every retained Transition and derive the expected Activation decision
  from its canonical Attention signal;
- construct the exact canonical decision bytes, including decision ID,
  activation ID, cause sequence, target, and policy evidence;
- require the durable decision key, target member, cause sequence, canonical
  decision bytes, and complete row set to match;
- run this check both during read-only inspection and the guarded recovery
  install transaction.

This keeps Activation decisions outside the immutable lineage hash while still
failing closed if an operational decision is missing, substituted, malformed,
misaddressed, or attached to the wrong transition.

## Verification

Passed:

```text
cargo test --locked -p worldstream-sqlite --lib
# 90 passed; 0 failed

cargo clippy --locked -p worldstream-sqlite --lib --tests -- -D warnings
# passed

rustfmt --edition 2024 --check crates/worldstream-sqlite/src/lib.rs
# passed
```

No public live harness was rerun in this lane. The parent should independently
rerun the actual public daemon path and full gates.
