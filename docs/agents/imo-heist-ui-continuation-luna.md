# IMO-53–IMO-57 Heist/UI continuation handoff

Date: 2026-08-21
Owner: parent orchestrator, with Luna lanes
Checkout: `/Users/vinothshanmugam/code/agent-streamer`

## Scope and acceptance rule

This is a bounded continuation handoff. It records direct local evidence and fail-closed gaps; it does not change Linear statuses, manifests, release gates, Rust server/sqlite core, or compatibility metadata. No Linear acceptance checkbox is claimed from an offline reducer, a fake transport, a markup-only test, or a narrative report when the checkbox requires a live process/browser/retained artifact.

The local implementation work was limited to `examples/heist/**`, `sdk/python/**`, `web/console/**`, and focused evidence docs/tests. The parent also fixed one UI test/build defect in `web/console/src/liveSession.ts`: the live Room Head fallback now derives `completeHead` from the received Room Head or the operator fixture instead of using an undefined variable.

## Exact local evidence

- `PYTHONPATH=examples/heist /usr/bin/python3 -m unittest examples.heist.test_story examples.heist.test_fresh_checkout`: 13 passed.
- `PYTHONPATH=sdk/python/src:examples/heist/wave10_live uv run --project sdk/python --locked pytest -q examples/heist`: 21 passed.
- `cd sdk/python && ./.venv/bin/python -m pytest -q`: 39 passed.
- `cd sdk/python && ./.venv/bin/ruff check src tests`: passed.
- `cd sdk/python && ./.venv/bin/ruff format --check src tests`: passed; 6 files already formatted.
- `PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH pnpm ui:test -- --run`: 4 files, 31 passed.
- `PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH pnpm ui:lint`: passed (`tsc --noEmit`).
- `PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH pnpm ui:build`: passed; Vite produced `web/console/dist`.
- `sh -n web/console/browser-privacy-smoke.sh`: passed.
- Browser-capable public smoke against Vite preview: passed; public projection loaded and private/credential/raw-state markers were absent.
- `git diff --check` over the allowed surfaces and evidence docs: passed.
- `./.venv/bin/ruff check ../../examples/heist`: passed. Repository-wide Heist `ruff format --check` was not clean: 9 shared example files request formatting; no bulk formatting was performed in this bounded continuation.

The retained offline fixture now compares exact Core, Activity, aggregate, lineage, and all 15 Transition SHA-256 hashes. Its final values are:

```text
Core       53a55ca514d3e1e950be3a4d333854a519007f5f9aebd11d39358c0ae774328a
Activity   a55c41b8ec66d6d7a8ecfc3f48188ec1b20f1346936d950d81ed54fbc7aa517b
Aggregate  a63fa15f679ab486627052ed482c4e8bdf2b9a83a5579078c9df757c7c029831
Lineage    6ae8ad69d00de0c6489a427818f03f4bcaed75919d207470b4420593369b0cae
```

`examples/heist/parity_fixture.json`, `examples/heist/story.py`, and `web/console/src/fixture.ts` fail closed on retained pack identity, replay hash drift, or Transition-list drift. The retained executor locally supports `load`, `project`, `advance`, and `replay`; it does not prove a retained Rust daemon/cross-platform corpus.

## Checkbox-by-checkbox audit

Status `local proof only` means the named seam is directly tested at that narrower scope, not accepted at the Linear checkbox scope. `partial`, `blocked`, and `not proven` are intentionally fail-closed.

### IMO-53 — six-phase Agent Heist reducer

| # | Acceptance checkbox | Status | Direct evidence / blocker |
|---:|---|---|---|
| 1 | Versioned seed selects exactly three immutable Genesis seats; suspension/departure leaves an explicit missing seat without changing denominator or transferring knowledge. | partial | Rust initialization/privacy tests cover fixed seats and suspension, but no direct Departed-state denominator/knowledge test in the allowed surfaces and no live run. |
| 2 | Briefing/Negotiation actions are bounded inspect, publish, exchange, plan, endorsement, and challenge with stable rejections. | partial | Rust handlers/descriptors and offline story cover the seams; every listed action/rejection is not directly exercised by a completed live daemon story. |
| 3 | Commitment enforces admission/deadline bounds, reminder, generation-fenced early closure, later resolve timer, and deadline closure with missing seats. | local proof only | Direct Rust timer/privacy tests cover these rules; this is reducer/test evidence, not a live scheduler acceptance. |
| 4 | Golden matrices cover zero/one/two matching, split, 3-0, 2-1, and three-split outcomes with the stated score bands. | partial | Matrix tests cover the named cases except a score-3 row; no complete 3–4/5 matrix proof. |
| 5 | Result/Complete, timer cancellation, duplicate/stale delivery, crash-after-commit, and Genesis-fold Replay reproduce exact state/events/timers/hashes. | blocked | Offline replay now compares all four component hashes plus 15 Transition hashes, but no retained live daemon output or process-kill artifact proves crash/restart and exact live equality. |
| 6 | No Heist-specific kernel branch or new kernel primitive is introduced. | local proof only | Source inspection shows the normal `ActivityPackV1`/registry path. This does not establish the other acceptance rows. |

### IMO-54 — privacy, Attention, and retained executability

| # | Acceptance checkbox | Status | Direct evidence / blocker |
|---:|---|---|---|
| 1 | Public/participant/operator views expose only their frozen allowlists; operators never receive raw Activity State, private clues/offers, or sealed commitments. | local proof only | Core projection and UI allowlist tests pass; no completed live Heist frame capture was retained. |
| 2 | Commitments remain sealed through Result; only authorized post-Complete final reveal is allowed; historical Replay does not inherit later authority. | local proof only | Core/SQLite authorization and UI final-reveal gate tests pass. Live end-to-end authority evidence is absent. |
| 3 | Paired hidden fixtures are indistinguishable through current/frame/catch-up/reset/error/log/UI payloads and Replay. | partial | Offline paired-surface and individual core/UI tests pass; no real server fixture pair covers every listed transport/log/Replay path. |
| 4 | Five Attention reasons, precedence, one reason per target/Transition, operational intents, and Replay signal-only behavior are enforced. | local proof only | Direct reason/precedence and activation/replay-boundary tests pass. No live Heist activation trace is retained. |
| 5 | Old digests load/project/advance/replay while not selectable for new Rooms; cross-platform golden bytes/hashes are fixed. | not proven | The local retained executor is executable and digest-checked, but it is the current Python fixture; no old non-selectable Rust corpus or fixed cross-platform artifact is available. Compatibility metadata was intentionally not edited. |
| 6 | SQLite crash/recovery with all snapshots removed reproduces every phase, Outcome, explanation, view, and Attention Signal. | blocked | Generic SQLite tests and bounded fixture evidence do not include a Heist all-snapshots-removed process-kill run. |

### IMO-55 — Counter protocol and Python SDK

| # | Acceptance checkbox | Status | Direct evidence / blocker |
|---:|---|---|---|
| 1 | Versioned envelopes have bounds, strict schemas, capability scopes, exact Head handling, stable/transient outcomes, and secret-safe errors. | local proof only | Protocol/server/SDK/UI tests pass, including secret-safe error assertions. This is not a full two-member live Counter acceptance. |
| 2 | Python client atomically installs retained frames or Reset, ACKs the session token, streams Live, reconnects, and handles pruning/slow-consumer closure. | local proof only | 39 SDK tests directly cover reset, retained sync, ACK/cursor, reconnect, and slow-consumer behavior. |
| 3 | Lost reply, duplicate identity, changed-payload conflict, hidden Head advance, stale result, resync, and new Action identity are safe without internal APIs. | local proof only | SDK/server tests cover all named fault seams and fail closed on resync. |
| 4 | Two Memberships expose only authorized Counter views; current and historical Replay respect authority. | partial | Generic membership/replay tests pass; no dedicated two-membership live Counter run with current and historical Replay authorization was retained. |
| 5 | A scripted scenario restarts SQLite and reaches the same projection/hashes. | partial | The disposable Counter run proves restart/reattach and ACK, but reports partial and does not compare post-restart projections or exact hashes. No dedicated real-process Counter hash run was completed here. |

### IMO-56 — first-party Heist UI

| # | Acceptance checkbox | Status | Direct evidence / blocker |
|---:|---|---|---|
| 1 | Public UI shows the public Heist board fields and aggregate result without private data. | local proof only | React tests, privacy DOM tests, fixture allowlists, and public browser smoke pass. The browser smoke is public-route only, not a full live Heist session. |
| 2 | Participant actions are available only from current Action Offers and exact-Head submissions. | partial | Live-session/transport tests enforce offer/head and stale-resync gating; no browser-backed live Heist action submission was demonstrated. |
| 3 | Operator diagnostics never expose raw Core/Activity/sealed/private values. | local proof only | Operator fixture/markup and core privacy assertions pass; no live browser operator capture was retained. |
| 4 | Attach/catch-up/reset/stale-resync/loading/faulted/quarantined states are visible and tested. | local proof only | App/live-session/transport state tests cover these states; browser proof is limited to the public smoke. |
| 5 | Replay is read-only, honors historical authority, and blocks final reveal before Complete. | local proof only | App, SDK, and core tests cover read-only/historical/final gates. No live Replay capture with exact daemon hashes exists. |
| 6 | Browser/DOM privacy checks and production build pass without provider credentials. | partial | Production build, Node-environment rendered-markup tests, and public headless Chrome smoke pass. The privacy unit tests use Vitest `--environment node`/`renderToStaticMarkup`; full browser automation for non-public views is not proven. |

### IMO-57 — full absent-Broker Heist story

| # | Acceptance checkbox | Status | Direct evidence / blocker |
|---:|---|---|---|
| 1 | Deterministic Navigator/Insider/Broker strategies drive all six live phases to the expected Outcome. | blocked | Offline Python story reaches all six phases with score 5/missing Broker. The fresh disposable daemon run stopped at `timer_not_due_or_not_publicly_addressable` with typed retryable `room_busy`, before offer/action/Outcome. |
| 2 | Broker Invocation ends; Attention creates one intent; fresh Invocation has exact context; authorized participant submits the Action. | blocked | Offline activation ledger tests cover separation; the live run reached no offer/claim/private context/action and explicitly reported no fabricated values. |
| 3 | Duplicate offer, lost claim reply, lease expiry/reclaim, stale generation, and idempotent completion preserve Cursor safety. | partial | SDK fake-runner and server fault tests pass; prior narrative disposable reports are not retained raw artifacts and no full live Heist Cursor trace is available. |
| 4 | Kill after Room COMMIT before reply/frame publication loses no result; restart drains overdue timers and resumes delivery/Activation. | blocked | No retained process-kill artifact exists; the bounded SQLite soak explicitly says process-level kill is not exposed, and the current Heist run was timer-blocked. |
| 5 | First-party UI has the same privacy story and final Replay has identical Core/Activity/aggregate/Transition hashes. | not proven | Local UI/fixture comparison now checks exact offline hash lists. The live harness emits `replay_verified`, not the four requested hash families; the fresh run never reached final Replay. |
| 6 | Fresh-checkout script runs the SQLite story without paid models or private services. | partial | Fresh-checkout test runs the offline story. It does not start SQLite/a daemon, and no retained full fresh-checkout live artifact exists. |

## Public disposable-daemon evidence and fail-closed blockers

The current bounded command was:

```text
PYTHONPATH=sdk/python/src uv run --project sdk/python --locked \
  python examples/heist/wave10_live/run_absent_broker_live.py \
  --spawn-daemon --binary target/debug/worldstreamd \
  --timer-timeout 35 --offer-timeout 2
```

It exited blocked with `timer_not_due_or_not_publicly_addressable` / typed retryable `room_busy`, `live_evidence: true`, and no offer, private context, fabricated completion, or final Replay. Earlier narrative Wave10 evidence reports a completed story, restart, duplicate Action, activation claim/reclaim, and Replay verification, but no retained raw receipt/frame/hash artifact is available; it therefore cannot replace the missing exact live evidence.

The decisive blockers remain:

1. No retained live full six-phase Heist run with exact Core/Activity/aggregate/Transition hash equality.
2. No dedicated process-level Counter restart run comparing projections/hashes.
3. No retained process-kill-after-commit or all-snapshots-removed SQLite Heist artifact.
4. No executable old non-selectable retained Rust digest/cross-platform golden corpus; local Python retained execution is narrower evidence.
5. No full browser-backed privacy/action/Replay proof for participant/operator/historical views; the real browser smoke covers the public route only.
6. Current public timer route remains blocked at the first live Heist readiness boundary.

These are fail-closed blockers, not reasons to infer acceptance. No Linear status or checkbox was changed.
