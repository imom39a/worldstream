# IMO-53/54/55/56/57 public Heist capability continuation

Date: 2026-08-21

## Handoff

No server capability or attach defect was confirmed in the shared checkout.
The public member-capability path and participant attach are sufficient to
drive the real Heist story. The observed `timer_not_due_or_not_publicly_addressable`
result was caused by running the acceptance harness with a timeout shorter
than the frozen public schedule, not by a missing public route or an authority
failure.

No product source, core implementation, manifest, release gate, or Linear
item was changed by this lane. This report is the only new artifact.

## Diagnosis

The public Agent Heist configuration is fixed at:

```text
Briefing       30 seconds
Negotiation   90 seconds
Commitment    30 seconds
Result        20 seconds
```

The first disposable-daemon reproduction used `--timer-timeout 45` and
returned a typed retryable `room_busy` until the first `_fire_when_due` budget
expired. A process-local diagnostic then captured the authorized participant
projection before the first timer:

```text
phase=briefing
room_seq=2
phase_deadline=2026-08-21T12:47:34.117201Z
now=2026-08-21T12:47:04.957224Z
```

The first timer advanced successfully. The next authorized projection was:

```text
phase=negotiation
room_seq=7
phase_deadline=2026-08-21T12:49:04.117201Z
```

That is the expected 90-second Negotiation deadline. Therefore a 35–45
second timer budget, and a budget ending at the exact 90-second boundary,
cannot prove the next timer. The server correctly keeps the timer route
fail-closed and maps an early fire to retryable `room_busy`.

The same diagnostic also proved that the issued member capability could read
the participant projection before any timer was fired. The full run issued all
three member capabilities, opened participant clients, and submitted public
Actions; no `Forbidden`, attach, sync, or capability error occurred.

## Full public evidence

Command:

```sh
PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH \
uv run --project sdk/python --locked python \
  examples/heist/wave10_live/run_absent_broker_live.py \
  --spawn-daemon --binary target/debug/worldstreamd \
  --timer-timeout 240 --offer-timeout 40
```

Result: `status=completed`,
`evidence_class=real_disposable_daemon_http_websocket_sdk`.

Verified by the run:

- `Briefing -> Negotiation -> Commitment -> Resolution -> Result -> Complete`;
- three real member capabilities and public participant Attach/Projection;
- deterministic Navigator and Insider Actions through the public SDK;
- absent Broker with `missing_roles=["broker"]` and a real failure outcome;
- duplicate Action resolution;
- Activation offer, duplicate claim, completion, lease expiry/reclaim, stale
  generation, and idempotent completion;
- daemon restart followed by authorized Projection;
- authorized Replay with `verification=verified`;
- `private_contexts_emitted=false`, `fabricated_offer=false`, and
  `secrets=not_emitted`.

The final outcome is intentionally not the offline success fixture: the live
room has two commitments and scores 1/5, so it reaches a real failure result.
That does not invalidate the six-phase or Replay invariants.

## Acceptance mapping

- IMO-53: the live public reducer traverses all six phases and preserves the
  absent Broker denominator; the complete acceptance row still needs the
  broader golden-matrix and crash/recovery corpus evidence.
- IMO-54: the live participant projection and redaction checks passed in the
  harness; full browser-backed privacy and retained-executability evidence
  remains separate and fail-closed.
- IMO-55: the public HTTP/WebSocket/Python lifecycle used by the Heist passed;
  the Counter-specific restart/hash acceptance remains separate.
- IMO-56: no browser UI run was claimed by this lane; the UI acceptance probe
  remains a distinct evidence requirement.
- IMO-57: the real absent-Broker six-phase story, Activation lifecycle,
  restart, duplicate Action, and Replay passed with an adequate timer budget.

## Verification

The full public run above passed. Independent local checks also passed:

```text
worldstream-server library tests: 48 passed
Python SDK room/runner tests: 33 passed
Heist tests: 21 passed
Console tests: 31 passed
Console type-check/lint: passed
Console production build: passed
```

No code change was necessary after the live diagnosis. External native
durability, browser-backed live UI, and release evidence are not fabricated
here.
