# Durable external-effect example

The server reference module in
[`crates/worldstream-server/src/external_effect.rs`](../crates/worldstream-server/src/external_effect.rs)
shows the application boundary for an external effect. A Room transition is
committed first, then the application creates a request with a stable
`business_operation_id` such as `booking/room-7`, `work_revision`, target
identity, committed provenance, exact Room and Activation preconditions, and a
bounded payload.

The journal is written before dispatch. A successful target response records
`applied`; a target rejection records `failed`. If the process loses the reply,
the record is `unknown`, including when the target applied the operation before
the reply was lost. On restart, `EffectReconciler::from_snapshot` restores the
record without contacting the target. An explicit probe can then resolve it to
`applied`, `failed`, or leave it `unknown` when the target cannot establish the
answer.

`FakeEffectTarget` demonstrates target-side idempotency: repeated attempts for
one operation/revision return `duplicate` after the first application, while
attempt IDs remain distinct (`.../a1`, `.../a2`). The fake has no real email,
booking, or network side effect and is intended for deterministic tests and
examples only.
