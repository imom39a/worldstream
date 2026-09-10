# IMO-211 session-expiry qualification

Midnight Archive schema v5 gives every started expedition one recorded session
deadline exactly 86,400 seconds after the Host's Activity Start timestamp. The
deadline applies only while the Room is nonterminal. It is independent of the
sixteen-turn danger budget, tab presence, network connection, and Projection
reads. Re-entry therefore restores the current retained facts; it does not
pause or extend the session.

The Pack schedules one fixed session Timer at Activity Start. Its current
generation enters the terminal `expired` phase with a null Outcome, clears
uncommitted staging and extraction preparation, and preserves discoveries,
resources, location, completed specialist work, and the original deadline.
Expiry records no extracted or left-behind crew result. Every gameplay or
operational terminal transition cancels outstanding session and companion
response Timers. Duplicate generations, late Timer delivery, and late companion
responses cannot add a contribution or replace an established terminal state.

The v12 Activity Client presents the deadline while active, states that closing
or re-entering does not pause it, distinguishes expiry from every gameplay
Outcome, keeps Actions disabled at terminal state, and permits authorized
Replay. Listing 0.2.0 carries the same disclosure before launch.

## Real-boundary evidence

`node scripts/verify-midnight-archive-acceptance.mjs
--expiry-boundaries-only` derives the exact retained Bundle from the production
proof and runs both Rust targets below serially.

- `midnight_archive_expiry_boundary`: three tests passed. A Runtime is closed,
  reconstructed over the same SQLite Room database after the deadline, and the
  production scheduler commits the overdue recorded Timer. Exact duplicate and
  later Timer generations remain inert, authorized Replay reconstructs the
  terminal facts, and the original participant re-enters the same Room.
- The same target races a real admitted extraction commit against the scheduler
  on both sides of the exact deadline. A pre-deadline admission completes with
  its gameplay Outcome; an admission at the deadline rejects and expiry wins.
  Neither terminal result can be overwritten.
- The same target holds an actual loopback HTTP provider response inside the
  production House model executor while the allowance ledger reports one call
  in flight. The Runtime expires the Room, the valid provider response then
  completes and consumes its original attempt, and an authenticated companion
  WebSocket submission is rejected with `action_not_allowed`. The terminal Head
  and Projection remain byte-for-byte unchanged.
- `midnight_archive_component_room::session_expiry_cancels_pending_reply_retains_facts_and_replays_exactly`:
  one ignored-by-default candidate test passed when selected explicitly. It
  uses the production portable Component Host, SQLite gateway, scheduler,
  participant and companion WebSockets, controlled Host clock, reconnect, and
  authorized Replay against the exact retained Bundle. A completed specialist
  contribution survives expiry while a pending response does not.

The standalone boundary target ran as two restart/race tests in 170.71 seconds
and one held-provider test in 50.51 seconds. The exact Component Replay witness
passed in 83.66 seconds. The clock is advanced through the Host clock boundary;
the test does not wait 24 wall-clock hours.

After raising only the integration harness's bounded socket allowance for a
loaded development machine, the consolidated command also passed in one clean
invocation: all three boundary tests in 537.72 seconds and the exact Component
Replay witness in 62.84 seconds.

## Pack, client, projector, and capacity checks

- Pack tests: 86 passed. Pack checking and production Component proof passed.
  The proof receipt is
  [production-proof-0.1.0-session-expiry.json](production-proof-0.1.0-session-expiry.json).
- Activity Client tests: 112 passed. TypeScript lint and production build
  passed. Browser verification passed for the exact v12 artifact and retained
  client lineages.
- Activity-client correspondence passed for 24 Releases, 3 Distributions, 20
  Deployments, and 29 Bindings.
- Platform tests: 163 passed with one existing live skip; TypeScript lint
  passed. The catalog test compares Listing 0.2.0 directly with the exact
  Bundle descriptor's public Projection schema and digest.
- The Host result-source tests passed and load Listing 0.2.0 as the source of
  both the observed and authorized public-evidence schema identity.
- Result-reconciliation tests passed, including `terminal_without_outcome` for
  expired/null evidence and retry retention without an Indexed Activity Result.
- Local Postgres pgTAP passed 54 assertions. The real terminal-evidence RPC
  retires active Run capacity once, retains Run/Membership identity, remains
  idempotent under duplicates, creates no public result, and authorizes a later
  launch only after retirement.
- Hosted House Runner tests passed with the real durable allowance ledger.
  Concurrent terminal retirement and a restarted coordinator produce one
  retirement receipt, preserve the original binding and consumed allowance,
  free exactly one slot, and do not suspend a resumable Runner.
- Hosted artifact generation/freshness, internal-candidate verification,
  Rust formatting, and `git diff --check` passed.

## Immutable candidate identities

- Pack Revision:
  `blake3:aea45a1c056c4a7da744be33d38df672499062fd2548302fae43adc49b833b07`
- Pack Bundle:
  `blake3:de3cd1d9fa45087b69cb107a663596305864c350f620fe4d7260e0341214d47a`
- portable Component:
  `blake3:7781388b52a2d335d07f5cee7658810bfb036ebe0e3b5c293459c7d38385af0c`
- production transcript:
  `blake3:a2c7ad92e3b64c34ae2a5df143b16e11ed79a381774889c14f49c8793df5a740`
- Activity Client v12 artifact:
  `sha256:c3c200e2ecb55c53bac24ddb31153fb667be1f64999d6ab00a601dd2ad670065`
- Activity Client v12 claims:
  `sha256:83c73a7dbac4aaa01eda6148aea0df61f030209eb67a807e32f1c5ac99e9e4af`
- Activity Client v12 conformance evidence:
  `sha256:8b75a5947b7f788d85b8f2522dd33ac6a691a023ed1278ff9e3f2fb73d32fe08`
- Activity Client v12 Release:
  `sha256:fae51aaa770810d203f00fd8be7d098b069ce2ef8e6fc9b7f31b80accf53345f`
- Public Projection v5 schema:
  `blake3:c8045ca0762df97d4e82488f562f7a69eea2b480e41f06889eef3dff133086d0`
- Result Projector 0.2.0:
  `blake3:93bdc21b4b09ec6e7c1ed7a11df80d984e2f80175e9fae01143f3a00c65d4a17`
- Listing 0.2.0:
  `blake3:efa4b63c9da251431343fbaab03e85ee2de053722345f9bd7b1434c2762283f3`

All earlier Pack, client, Listing, and projector artifacts remain retained for
their original Rooms. The new Listing is unlisted, solo-only, and has result
publication disabled.

## Qualification limits

This is composed real-boundary evidence, not a deployed 24-hour soak. Runtime
restart reconstructs the production backend over the same SQLite database
rather than killing an operating-system process. The held response uses real
HTTP transport, response validation, accounting, and WebSocket submission with
a loopback provider fixture; it is not a paid external-provider call or a
managed Runner subprocess. Live Mira/Jonah provider quality and hosted
multi-roster launch qualification remain IMO-205 and IMO-209 work.
