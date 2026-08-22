# IMO-53 / IMO-54 / IMO-57 Heist replay-retention lane

Status: local replay/retention evidence completed; current disposable-daemon
rerun is fail-closed on a public timer prerequisite (2026-08-21). Linear
statuses were not changed.

## Acceptance audit

The acceptance surface represented by `examples/heist/**` is now covered as
follows:

- **IMO-53 — Heist semantics:** the offline `service_window` story traverses
  `Briefing -> Negotiation -> Commitment -> Resolution -> Result -> Complete`,
  records 15 ordered Transitions, and verifies the strict 2-of-3 result:
  score 5, success, and an explicit missing `broker` seat.
- **IMO-54 — privacy/retention:** the existing audience and Activation tests
  keep private clues, sealed commitments, offers, and Activation context
  separate from public/operator/historical views. The retained executor now
  exercises exact `load`, `project`, one-at-a-time `advance`, and read-only
  `replay` behavior for the same Room lineage.
- **IMO-57 — absent Broker:** the local fixture keeps all three immutable seats
  in the denominator, creates no Broker participant Action, records the
  bounded Activation/reclaim evidence, and reaches the deterministic success
  fixture with `missing_roles: ["broker"]`.

Replay compares the retained Transition hash list and the final Core,
Activity, aggregate Authoritative, and lineage hashes. The exact retained
executor identity is pinned to:

```text
worldstream.agent-heist / 0.1.0
blake3:755aa7a88b8236d951da297e700ab41501d091a0ee1585547e90d8a46da95bfe
```

The retained evidence is a separate `retained_executor` object in
`examples/heist/parity_fixture.json`; the console-consumed transcript and its
existing digest remain unchanged.

## Verification

Passed:

```text
PYTHONPATH=examples/heist /usr/bin/python3 -m unittest \
  examples.heist.test_story examples.heist.test_fresh_checkout
# 13 tests, OK

PYTHONPATH=examples/heist /usr/bin/python3 examples/heist/run_story.py --self-test
# status=ok, room_seq=15, six phases, success score=5, broker missing,
# replay verified with Core/Activity/aggregate/lineage and 15 Transition hashes

PYTHONPATH=sdk/python/src:examples/heist/wave10_live \
  uv run --project sdk/python --locked pytest -q \
  examples/heist/test_story.py \
  examples/heist/test_fresh_checkout.py \
  examples/heist/wave10_live/test_absent_broker_live.py
# 18 passed in 0.18s

PYTHONPATH=sdk/python/src:examples/heist/wave10_live \
  uv run --project sdk/python --locked pytest -q examples/heist
# 21 passed in 0.18s

uv run --project sdk/python --locked ruff format --check examples/heist
# 13 files already formatted

uv run --project sdk/python --locked ruff check examples/heist
# All checks passed!

git diff --check -- examples/heist docs/agents/imo-heist-replay-retention-luna.md
# passed
```

An unscoped direct-system-Python discovery was also attempted with
`PYTHONPATH=sdk/python/src:examples/heist/wave10_live`. Its 15 discovered tests
could not import the SDK because that interpreter lacks the `websockets`
dependency; the SDK-scoped `uv ... pytest` command above supplies the declared
environment and passes all 18 relevant tests. This is a tooling/environment
blocker only, not a Heist assertion failure.

## Public disposable-daemon boundary

The checked-in Wave 10 note records an earlier real HTTP/WebSocket/SDK run
that reached all six public phases, restart, duplicate Action, Activation
lease recovery, and verified Replay. Its live Room selected a real failure
result (score 1 with Broker missing), which is correctly distinct from the
offline fixed `service_window` success fixture.

The fresh rerun in this lane used:

```text
PYTHONPATH=sdk/python/src uv run --project sdk/python --locked python \
  examples/heist/wave10_live/run_absent_broker_live.py \
  --spawn-daemon --binary target/debug/worldstreamd \
  --timer-timeout 35 --offer-timeout 2
```

It contacted the disposable daemon and returned `status: blocked`,
`live_evidence: true`, `reason_code: timer_not_due_or_not_publicly_addressable`,
`timer_id: 01ARZ3NDEKTSV4RRFFQ69G5FH0`, `generation: 2`, and a typed retryable
`room_busy` HTTP 429. No offer, private context, secret, or fabricated live
completion was emitted. This lane therefore claims the exact replay/retention
behavior locally and preserves the live timer-route blocker rather than
altering server/core code outside the allowed paths.
