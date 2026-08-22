# IMO-55 Python SDK Counter lifecycle audit

This is the Python SDK lane audit for the IMO-55 lifecycle checkboxes. It
uses only the public HTTP/WebSocket/SDK contract. No Linear status, Rust core,
server, SQLite, manifest, release gate, example, or console file was changed.

## Checkbox audit

- [x] Counter restart/reconnect: `Room.reconnect()` preserves the last
  acknowledged Cursor in `room.attach`; `Room.resync()` performs reconnect,
  retained/reset installation, and the matching sync barrier before returning
  live. The local acceptance suite covers this with a retained reconnect.
- [x] Capability/authority split: participant and Runner sessions use the
  scoped bearer only in the HTTP Authorization header or WebSocket handshake
  header; participant uses `/v1/stream` and `mode=participant`, while Runner
  uses `/v1/runner/stream` and `mode=runner`. No SDK method treats a bearer as
  universal authority.
- [x] Duplicate same identity: a same-body Action retry preserves the exact
  Action identity and accepts the server's `duplicate` result without local
  reinterpretation. A real disposable public-surface story also observed a
  duplicate Action receipt.
- [x] Changed payload conflict: the SDK retains a canonical fingerprint for
  each locally used Action ID and fails closed with `idempotency_conflict`
  before sending a changed body. The server remains authoritative across
  processes and reconnects.
- [x] Hidden Head advance/resync/new identity: accepted results install a
  strictly newer complete Head without rewinding on an older duplicate;
  stale results mark the Room as requiring resync; a new Action identity is
  rejected until `resync()` completes. Actions are never silently rebased.
- [x] Stale stable results: stale `action.rejected` receipts are returned as
  received, including stable duplicate replies, while their incomplete Head
  information cannot overwrite the locally authoritative complete Head.
- [x] Retained frames versus Projection Reset: attach selects one branch;
  retained frames must cover the exact captured range, while Reset must match
  the captured baseline and complete Head. Installation is atomic and only a
  matching `room.sync_acked` makes the Room live; neither sync ACK advances
  Cursor.
- [x] Replay/current projection authority: HTTP Replay and current Projection
  use separate read-only endpoints and strict response schemas. Replay must be
  verified and cannot be installed as current Room state; current Projection
  responses remain addressed to the requested Room.
- [x] Secret-safe errors: bearer-like values are redacted from typed HTTP and
  WebSocket errors/details. Bearers are never placed in URLs, Action payloads,
  logs, or test output.

## Verification

Focused SDK checks from `sdk/python`:

```text
./.venv/bin/python -m pytest -q
39 passed
./.venv/bin/ruff check src tests
All checks passed!
./.venv/bin/ruff format --check src tests
6 files already formatted
```

Existing disposable daemon scenario, run without modifying its source:

```text
./sdk/python/.venv/bin/python examples/heist/wave10_live/run_absent_broker_live.py \
  --spawn-daemon --binary target/debug/worldstreamd
status=completed
evidence_class=real_disposable_daemon_http_websocket_sdk
restart.performed=true
duplicate_action.duplicate=true
replay_verified=true
secrets=not_emitted
```

That run is real public HTTP/WebSocket/SDK evidence for the disposable Agent
Heist path, not a dedicated Counter deployment claim. It is included only to
confirm that the tested transport and lifecycle surfaces can cross a running
daemon boundary.

## Fail-closed blockers

- No dedicated real-process Counter restart/reconnect scenario was available
  in the permitted SDK/test surface. The local Counter lifecycle fixtures are
  therefore the authoritative evidence for the new SDK behavior.
- `scripts/restart-smoke.sh` is Linux-only and intentionally proves a
  fail-closed, unready/unauthorized daemon boundary rather than a successful
  member Counter attach; it was not promoted to Counter acceptance evidence.
- The full checkout remains dirty with unrelated parent changes. Those files
  were preserved and not edited by this lane.
