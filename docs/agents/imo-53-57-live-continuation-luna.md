# IMO-53/54/55/56/57 Wave 11 live continuation

Date: 2026-08-21
Lane: Luna live acceptance
Repository: `/Users/vinothshanmugam/code/agent-streamer`
Report: `/tmp/luna-live-heist-report.txt`

## Result

The strongest real absent-Broker story available in the current daemon is now
captured by the new `examples/heist/wave11_live/` orchestrator. It delegates
Room creation, capability issuance, Runner Activation, timer firing, replay,
and fault boundaries to the existing audited wave10 runner. Every story uses a
fresh disposable SQLite daemon and an owner-only temporary bootstrap secret.
Credentials, private Activation context, Room identifiers, and ULID-like
identifiers are redacted from the report.

The aggregate remains `blocked` because one requested fault mode and the live
browser handoff lack current public capabilities. No success is inferred from
those gaps.

## Exact commands and results

Build:

```text
PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH cargo build --locked -p worldstream-server --bin worldstreamd
Finished `dev` profile [unoptimized + debuginfo]
```

Wave10 contract tests used by the live runner:

```text
PYTHONPATH=sdk/python/src:examples/heist/wave10_live uv run --project sdk/python --locked pytest -q examples/heist/wave10_live/test_absent_broker_live.py
7 passed
```

Wave11 orchestrator:

```text
PYTHONPATH=sdk/python/src uv run --project sdk/python --locked python examples/heist/wave11_live/run_live_acceptance.py --binary target/debug/worldstreamd --timer-timeout 180 --offer-timeout 20 --story-timeout 420
exit 2 (aggregate fail-closed)
```

Observed from the three fresh real-daemon runs:

| Story | Result | Evidence |
| --- | --- | --- |
| Standard | `completed` | Six phases, three seats with absent Broker outcome, real HTTP/WebSocket SDK traffic, Runner offers/claims/completions, lease reclaim and stale-generation fencing, restart on the same data directory, duplicate Action receipt, read-only replay, and verified public hash parity. |
| Lost Activation claim reply | `blocked` | Real daemon returned HTTP 400 `internal`; no claim or retry was claimed. `fabricated_offer=false`, `private_contexts_emitted=false`. |
| Kill after Action commit | `completed` | Six phases, restart, exact request retry, matching durable duplicate receipts, and `durable_duplicate_result_matches=true`; replay/hash parity verified. |

The standard story observed `phase_path=[Briefing, Negotiation, Commitment,
Resolution, Result, Complete]`, `six_phase_order=true`, `replay_verified=true`,
and a final absent-Broker score of 5. The kill-boundary run also completed the
same six-phase path; its fixture seed produced a score of 1, which is retained
as the actual daemon result rather than normalized to the successful fixture.

Browser/UI handoff:

```text
PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH START_PREVIEW=1 CONSOLE_PORT=4173 bash web/console/browser-privacy-smoke.sh
browser privacy smoke passed: public, participant, operator, Replay, recovery, fault, quarantine, and terminal fixture routes were inspected without credentials
```

This is browser fixture/privacy evidence only. The live handoff is explicitly
blocked because the existing browser harness has no header-capable WebSocket
adapter; the native browser WebSocket cannot send the disposable daemon
bearer, and the token is not placed in a URL or subprotocol.

Wave11 harness verification:

```text
python3 -m unittest -v examples/heist/wave11_live/test_live_acceptance.py
Ran 3 tests ... OK
uv run --project sdk/python --locked ruff format --check examples/heist/wave11_live
3 files already formatted
uv run --project sdk/python --locked ruff check examples/heist/wave11_live
All checks passed!
```

The report’s credential/private-context scan passed. The wave11 harness itself
is limited to new files under `examples/heist/wave11_live/`; no core, server,
SDK, UI, manifest, or compatibility implementation was changed by this lane.

## Remaining acceptance boundary

IMO-53/54/55/57 now have strong real disposable-daemon evidence for the normal
path and Action-loss restart path. The Activation claim-loss path needs a
publicly supported transport-loss seam that reaches the claim-before-reply
boundary without returning the daemon’s current `internal` rejection. IMO-56
needs a reviewed header-capable extension/browser adapter for live authorized
WebSocket evidence. Until those capabilities exist, this lane remains
fail-closed and does not mark Linear issues complete.
