# Heist acceptance audit: IMO-53, IMO-54, IMO-56, IMO-57

Status: local acceptance substantially evidenced; final IMO-57 loss-boundary
follow-up is live-proven. Linear statuses were not changed.

## Evidence completed

- `PYTHONPATH=sdk/python/src:examples/heist/wave10_live uv run --project sdk/python --locked pytest -q examples/heist/test_story.py examples/heist/test_fresh_checkout.py examples/heist/wave10_live/test_absent_broker_live.py`
  — 16 passed.
- `uv run --project sdk/python --locked ruff check examples/heist` and
  `ruff format --check examples/heist` — passed.
- `PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH pnpm ui:lint` — passed.
- `pnpm ui:test` — 3 files and 23 tests passed.
- `pnpm ui:build` — production Vite build passed.

An earlier completed disposable-daemon run in this checkout produced real
HTTP/WebSocket/SDK evidence with `status: completed`, the exact six-phase path
`Briefing -> Negotiation -> Commitment -> Resolution -> Result -> Complete`,
`duplicate_action.duplicate: true`, `restart.performed: true`, and
`final.replay_verified: true`. It also proved first-claim/idempotent completion,
lease reclaim, stale-generation rejection, and no emitted private contexts or
secrets. The final result was the real absent-Broker failure (score 1), not a
fabricated success.

## Privacy and browser audit

The checked-in UI tests cover public/participant/operator projection separation,
redacted operator diagnostics, synchronization/integrity states, read-only
Replay, and final-reveal gating at `Complete`. The production build contains
no provider credentials. The live harness also rejects participant projections
containing fixture, clue, commitment, or exchange fields and records only
bounded Activation context-presence/hash booleans.

## Parent follow-up evidence (2026-08-21)

The parent added two explicit, default-off transport fault seams and verified
them through the public disposable-daemon path:

- `--exercise-kill-boundary` returned `status: completed`, observed transport
  loss after the accepted Action commit, restarted the same data directory,
  retried the exact Action request twice, and reported matching duplicate
  receipt/transition/head hashes.
- `--exercise-lost-claim-reply` returned `status: completed`, observed the
  lost Runner claim reply, restarted the same data directory with seams
  disabled, retried the exact retained Runner request twice, and reported
  matching `granted` receipts with no emitted context bytes.

Both runs also retained the six-phase path, duplicate Action, restart, and
verified Replay evidence. The server seam unit tests passed in the final
48-test server suite; Rust format/clippy, Python format/lint, and the five
Wave 10 harness tests passed.

## Fail-closed blockers

The earlier audit's transport blockers are superseded by the parent follow-up
above. The only remaining blockers for the broader active Linear set are the
release/provider/platform evidence rows recorded in the release and
PostgreSQL audit reports; no claim is made that those external rows are
resolved by local fixtures.

Therefore IMO-53/54/56 have strong local and browser/privacy evidence, and
IMO-57 now has live proof for both exact lost-reply boundaries. No Rust core,
compatibility manifest, or release status was changed by this audit.
