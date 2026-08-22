# IMO-57 Wave 10 live absent-Broker harness

Status: completed with real disposable-daemon evidence (2026-08-21)

## Parent current-source regression correction

The retained-Heist registry work changed the selectable `0.1.0` semantic
revision to
`blake3:b05a682f0923001914a800072ee68348e68b979453c93e033cba7916b99a4407`,
but this live harness still pinned the earlier `755aa7...` digest. A fresh
disposable daemon therefore rejected the first public `create_room` request
with stable `invalid_payload`. Parent minimized that failure to a sub-second
create-only loop, reproduced it twice, and proved that changing only the
request digest to the unique selectable manifest row made creation succeed.

The harness now pins the current exact digest, and its unit test reads
`compatibility.toml` and requires the constant to equal the unique selectable
Agent Heist `0.1.0` row. That test failed before the correction and passes
after it. Parent then reran all three real variants against the current daemon:

- standard six-phase story: completed, restarted, exact Replay hashes;
- Action commit-before-reply termination: completed with two matching durable
  duplicate receipts after restart;
- Activation claim-before-reply termination: completed with two matching
  granted receipts after restart.

All three retained the exact six-phase path, real absent-Broker outcome,
privacy redaction, and `secrets: not_emitted` evidence. The old digest remains
valid only in historical prose where explicitly identified as superseded; it
is not accepted as a current executable identity.

## Final closure evidence

The public live path is now complete. The final disposable-daemon run used the
SDK and public HTTP/WebSocket routes only and returned `status: completed` with
`live_evidence: true`, `six_phase_order: true`, `final.phase: complete`,
`final.replay_verified: true`, `duplicate_action.duplicate: true`, and
`restart.performed: true`. It observed the exact phase path
`Briefing -> Negotiation -> Commitment -> Resolution -> Result -> Complete`.

The run also proved both public Activation stories without emitting private
context bytes: first-claim duplicate lookup, idempotent completion, short-lease
reclaim, stale old-generation completion, and completion of the reclaimed
generation. The final Heist outcome remained a real pack result (`failure`,
score `1`, missing Broker role), not a fabricated success.

The final closure also used two separate, explicit, default-off fault modes.
`--exercise-kill-boundary` observed transport loss after the durable
participant Action commit, restarted the same data directory with the seam
disabled, and retried the exact Action request twice with matching duplicate
receipts, transition identity, and Room-head hashes. Separately,
`--exercise-lost-claim-reply` observed transport loss after a durable Runner
Activation claim, restarted the same data directory with seams disabled,
reconnected the Runner, and retried the exact retained request twice with
matching `granted` receipts. Both modes returned `status: completed`,
`live_evidence: true`, and `secrets: not_emitted`.

During closure, two defects were fixed and independently verified:

- SQLite completion decoded the BLOB `activation_intents.context_bytes` column
  as `Option<String>`, collapsing `InvalidColumnType` into public
  `storage_unavailable`; it now uses `Option<Vec<u8>>`, with a public-store
  completion/idempotency regression.
- The public Replay route was absent, and the harness member capability lacked
  `room:replay`. The route now consumes the existing authorized Replay seam and
  returns the exact SDK response shape; the harness requests the explicit scope.
  The duplicate Action replay also now reuses the original canonical
  `based_on_room_seq` instead of the earlier `publish_clue` result.

`examples/heist/wave10_live/run_absent_broker_live.py` is the public live
acceptance harness for the absent-Broker Agent Heist story. It uses only:

- `POST /v1/rooms` for Heist Genesis;
- `POST /v1/operator/member-capabilities` and
  `POST /v1/operator/runner-capabilities` for explicitly authorized test
  identities;
- the Python SDK `Client`, `Room`, and `Runner` surfaces for projections,
  participant Actions, and Activation offer/claim/complete operations; and
- `POST /v1/operator/rooms/{room_id}/timers/fire` with only `{timer_id,
  generation}` for exact durable timer witnesses.

The harness never writes a database, submits a timer payload or timestamp,
constructs an offer, prints a bearer, emits an Activation context, or treats a
fixture/offline run as live evidence. Participant projections are checked for
absence of final-reveal fixture/clue/commitment fields. Activation receipts
retain only bounded status, generation, and context-presence/hash booleans.

The current-source registry probe resolved the exact Heist revision as
`blake3:755aa7a88b8236d951da297e700ab41501d091a0ee1585547e90d8a46da95bfe`;
the current Core `prepare_genesis_for_new_room` probe accepts that digest,
the frozen configuration, and three Agent participant seats. The disposable
daemon now accepts the public Genesis request, and the harness provisions the
three Agent principals through the public runner-capability route before
issuing participant capabilities. This is necessary because Genesis names
Agent principals but does not itself register them in the authority registry.

The fixed canonical ULID identities are distinct: create uses
`01ARZ3NDEKTSV4RRFFQ69G5FB1`, Agent principals use the `FD0`/`FD1`/`FD2`
identities, member capabilities use the separate `FG0`/`FG1`/`FG2` keys, and
each runner-capability change uses its own three-key group (the
Broker group is `FE0`/`FE1`/`FE2`). The earlier room-derived identity collision
is no longer present.

Historical pre-closure note: the public timer route also accepted the exact Heist timer identity
`01ARZ3NDEKTSV4RRFFQ69G5FH0` at generation 1. At that earlier point, the live
blocker was the
public SDK `propose_plan` Action, which the daemon rejects with bounded error
code `internal`:

```json
{"action_type":"propose_plan","error_code":"internal","fabricated_offer":false,"live_evidence":true,"private_contexts_emitted":false,"reason_code":"action_rejected","retryable":false,"secrets":"not_emitted","status":"blocked"}
```

That earlier run therefore permitted a live path through the first
`phase_deadline` transition into Negotiation, but no live six-phase path is
claimed. No Activation offer, claim, completion, or final Heist result was
observed. The harness did not relax server validation or fabricate a reason
for the daemon's bounded `internal` response.

The deterministic public action plan is Navigator/Insider clue inspection and
publication, a fixed `service_window` plan proposal, Insider endorsement,
two participant commitments, result acknowledgements, and the exact timer
sequence `phase_deadline` generations 1–4 with `resolve_now` generation 1.
The run records the actual public outcome; it does not claim a successful
fixture when the disposable room's random seed selects another fixture.

The harness covers duplicate claim, first invocation completion, short-lease
expiry/reclaim, stale old generation completion, idempotent completion,
duplicate participant Action, restart persistence (when `--spawn-daemon` is
used), verified read-only replay, and exact lost-reply retries for both the
Action and Runner Activation paths. The two fault modes retain the original
request identity from the SDK and never invent one.

## Verification

```sh
PYTHONPATH=sdk/python/src:examples/heist/wave10_live \
  uv run --project sdk/python --locked pytest -q \
  examples/heist/wave10_live/test_absent_broker_live.py
```

With a built daemon, the real disposable run is opt-in:

```sh
PYTHONPATH=sdk/python/src \
  uv run --project sdk/python --locked python \
  examples/heist/wave10_live/run_absent_broker_live.py \
  --spawn-daemon --binary target/debug/worldstreamd
```

If the public timer witness, Heist Action, Activation, or replay boundary is
unavailable, the command exits 2 with a typed `reason_code` and
`live_evidence: true` (after it has contacted the daemon). Missing daemon or
operator inputs are reported as `live_evidence: false`. No Linear status is
changed by this harness.

## Wave 10 verification result

Changed files:

- `examples/heist/wave10_live/__init__.py`
- `examples/heist/wave10_live/run_absent_broker_live.py`
- `examples/heist/wave10_live/test_absent_broker_live.py`
- `docs/agents/imo-57-wave10-live-luna.md`

Commands:

```text
uv run --project sdk/python --locked ruff format examples/heist/wave10_live
# 3 files left unchanged
uv run --project sdk/python --locked ruff check examples/heist/wave10_live
# All checks passed!
PYTHONPATH=sdk/python/src:examples/heist/wave10_live uv run --project sdk/python --locked pytest -q examples/heist/wave10_live/test_absent_broker_live.py
# 5 passed in 0.07s
PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH cargo build --locked -p worldstream-server --bin worldstreamd
# Finished `dev` profile
PYTHONPATH=sdk/python/src uv run --project sdk/python --locked python examples/heist/wave10_live/run_absent_broker_live.py --spawn-daemon --binary target/debug/worldstreamd --timer-timeout 35 --offer-timeout 2
# exit 2; result shown above; Genesis, capability provisioning, and TimerFired generation 1 passed; propose_plan was rejected
```
