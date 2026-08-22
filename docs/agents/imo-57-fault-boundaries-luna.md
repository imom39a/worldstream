# IMO-57 fault-boundary follow-up

Status: completed with real disposable-daemon evidence (2026-08-21).

The public gateway now has two explicit, default-off, exact-identity fault
seams in `crates/worldstream-server/src/lib.rs`:

- `WORLDSTREAM_TEST_KILL_AFTER_ACTION_COMMIT_BEFORE_REPLY` terminates after a
  non-duplicate accepted Action has durably committed and before the
  `action.accepted` WebSocket reply is serialized.
- `WORLDSTREAM_TEST_KILL_AFTER_ACTIVATION_CLAIM_BEFORE_REPLY` terminates after
  a first successful Activation claim and before `activation.claimed` is
  serialized. Replays, rejected claims, and non-matching IDs do not trigger it.

The seams are used only by opt-in modes of
`examples/heist/wave10_live/run_absent_broker_live.py` and are absent from the
normal disposable run.

## Live evidence

`--exercise-kill-boundary` returned `status: completed` and proved:

- transport loss after durable Action commit;
- same-directory restart with the seam disabled;
- exact SDK request retry twice;
- matching duplicate receipt, transition identity, and Room-head hashes;
- no secrets emitted.

`--exercise-lost-claim-reply` returned `status: completed` and proved:

- transport loss after durable Activation claim;
- same-directory restart with both seams disabled;
- Runner reconnect and exact retained-request retry twice;
- matching `granted` receipts with bounded context presence/hash metadata;
- no private context bytes or secrets emitted.

Both modes retained the six-phase path
`Briefing -> Negotiation -> Commitment -> Resolution -> Result -> Complete`,
duplicate Action evidence, restart evidence, and verified Replay. The final
pack outcome is reported as observed; a missing Broker is never converted into
a fabricated success.

## Verification

- `cargo test --locked -p worldstream-server --lib` — 48 passed.
- `cargo clippy --locked -p worldstream-server --lib --tests -- -D warnings` — passed.
- `cargo clippy --locked -p worldstream-sqlite --lib --tests -- -D warnings` — passed.
- `cargo fmt --all -- --check` — passed.
- Wave 10 Heist harness — 5 passed; Ruff format/check passed.
- Strict offline pre-push gate — all local implementation/evidence cells passed;
  it remains fail-closed on external release/provider/platform evidence rows.

Linear statuses were not changed. No compatibility manifest or release digest
was fabricated.
