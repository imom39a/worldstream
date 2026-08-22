# IMO-54/56/57 UI/live Luna acceptance lane

Status: implemented and verified where the current public protocol reaches;
live Heist execution remains fail-closed (2026-08-21).

## Scope and boundary

This lane audited IMO-54, IMO-56, and IMO-57 against the first-party console,
`examples/heist`, `sdk/python`, the checked-in protocol, and the prior live
evidence. It changed only `web/console`, `examples/heist`, and this report.
No Rust server/core/protocol, packaging, or compatibility file was changed by
this lane. No Linear issue was marked Done.

## Implemented UI changes

- Live `room.attached`, `projection.reset`, `observation.deliver`, Action
  receipts, and protocol errors now update phase, phase generation, deadline,
  room sequence, frame state, Room health, integrity generation, and bounded
  operator diagnostics from existing wire fields.
- Activity projection mapping is allowlisted to public-safe fields: phase,
  deadline, role presence, public claims, commitment count, and aggregate
  outcome. Private clues, own commitments, raw Activity/Core objects, and
  unknown fields are never copied into the UI fixture.
- Delta observations without `action_offers` preserve the installed offers;
  an offer list is replaced only when the protocol carries a new list.
- Participant actions require a Live/Active/Healthy exact synchronized head and
  an Action type present in the installed offers. The UI records the generated
  Action ID and request ID, shows submitting/accepted/rejected states, and
  turns stale-head replies into a non-submittable state until a sync barrier is
  acknowledged. It never rebases or retries the old Action ID.
- The UI explicitly renders stale-head, reset-required, CatchingUp, Faulted,
  Quarantined, Healthy, and exact-head surfaces. Runner diagnostics identify the
  separate `/v1/runner/stream` boundary instead of claiming a Room stream owns
  Runner state.

## Fail-closed live harness

`examples/heist/wave9_live/run_ui_live_acceptance.py` performs a real readiness
check and authorized Runner offer poll. It never creates a fake offer, emits a
bearer, claims an invented Activation, or reports a complete Heist from an
empty poll. Its deterministic classifier and tests report the exact missing
prerequisite:

> a public transition-producing Heist operation or a pre-seeded eligible
> Invocation that creates an Activation

The actual disposable-daemon probe used the issued Runner capability and real
HTTP/WebSocket traffic. It returned:

```json
{"status":"blocked","reason_code":"activation_transition_producer_missing","offer_count":0,"readyz":200,"live_evidence":true,"fabricated_offer":false,"secrets":"not_emitted"}
```

This is consistent with the current protocol: Runner hello/offer/claim/lease
operations exist, but no public operation in this lane can produce the first
Heist Activation transition.

## Changed files

- `web/console/src/liveSession.ts`
- `web/console/src/fixture.ts`
- `web/console/src/components.tsx`
- `web/console/src/styles.css`
- `web/console/src/liveSession.test.ts`
- `web/console/src/App.test.tsx`
- `examples/heist/wave9_live/run_ui_live_acceptance.py`
- `examples/heist/wave9_live/__init__.py`
- `examples/heist/test_ui_live_acceptance.py`
- `docs/agents/imo-ui-live-luna.md`

## Verification

- `pnpm ui:lint` — passed.
- `pnpm ui:test` — 23 tests passed across 3 files.
- `pnpm ui:build` — production Vite build passed.
- `PYTHONPATH=sdk/python/src uv run --project sdk/python --locked pytest -q examples/heist` — 14 passed.
- `uv run --project sdk/python --locked pytest -q sdk/python/tests` — 33 passed.
- Python Ruff check and format check — passed.
- Harness bytecode compile and `git diff --check` — passed.
- Disposable daemon + Runner capability + real Runner WebSocket offer poll —
  passed as live evidence and correctly returned the blocked prerequisite above.

The remaining live acceptance prerequisite is external to this UI lane: the
server must expose a real transition-producing operation or provide a
pre-seeded eligible Invocation. Until then, IMO-57 cannot honestly claim the
six-phase absent-Broker live story, and the UI reports that boundary instead of
simulating it.
