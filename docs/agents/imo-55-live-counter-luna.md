# IMO-55 live Counter acceptance — Luna lane

Status: implemented and exercised against a real spawned `worldstreamd` process.

## Defect and bounded fix

The defect was in the WebSocket path: `action.accepted` was produced after the
durable backend action, but the gateway did not publish the newly committed
observation to already-live sockets.

The gateway now registers a connection only after its attach/sync barrier is
acknowledged. Sync acknowledgement and registration share the same publication
lock as action fanout; the lock is released before socket writes. Each
registration owns a connection-local last-delivered frame sequence seeded from
the maximum frame returned by sync acknowledgement (or its acknowledged
barrier when the returned batch is empty) and a bounded `mpsc` queue. After an
accepted action commit, the gateway serializes publication and asks the backend
for the authorized observation suffix after that sequence. The SQLite adapter
performs an authorized `CatchUp` read only; it does not consume the one-shot
sync binding or advance the shared Membership Cursor. Frames are checked
against the registered room and Membership before enqueueing. Queue overflow,
malformed cross-scope results, and backend read failure close the registration
with a bounded typed error. A reconnect creates a new registration from its
captured Cursor/frame head, so the old connection cannot duplicate delivery.

The Python SDK already defers unrelated WebSocket messages while awaiting an
action reply; the focused SDK regression confirms that a live observation
frame remains available through `Room.events()` after `Room.act()` returns.

## Criterion report

| Criterion | Result | Evidence |
| --- | --- | --- |
| Fresh Counter Room with two Memberships | Passed | Real process scenario created participant and spectator Memberships. |
| Current projection authority | Passed | Participant saw `private_ack_count,value`; spectator saw only `value`. |
| Historical Replay authority | Passed | Both Memberships replayed room sequence 0 and 1 with the same field fence. |
| Cross-authority denial | Passed | Spectator capability attaching to the participant Membership returned `forbidden`. |
| Live private frame | Passed | `private_ack` returned `action.accepted`; participant received an unsolicited frame; spectator received none. |
| Live public frame privacy | Passed | Both live sockets received the increment frame, with the private field only on the participant frame. |
| Duplicate / changed payload / stale / new-ID contracts | Passed | Duplicate reused the transition; changed payload returned `idempotency_conflict`; stale returned `stale_room_state`; only stale recovery used `resync`; the new ID committed room sequence 3 and pushed to both sockets. |
| Restart and reconnect parity | Passed | Same SQLite data directory was restarted. Exact projection, Room Head, and Replay hashes matched before and after for both authorities. |
| No secret emission | Passed | Scenario output marked `secrets:not_emitted`; literal bearer scans found no matches. |

The acceptance script is [run_live_acceptance.py](../../examples/counter/run_live_acceptance.py)
and its local tests are [test_acceptance.py](../../examples/counter/test_acceptance.py).

## Verification commands and results

All commands were run from the repository root after the normal Python SDK
install and Rust build environment were available.

```text
cargo fmt --package worldstream-server -- --check
PASS

cargo test --locked -p worldstream-server --no-run
PASS — server library and both server binaries compiled.

cargo test --locked -p worldstream-server publication -- --nocapture
PASS — 3 deterministic publication/barrier/privacy tests passed.

cargo test --locked -p worldstream-server live_slow_consumer_closes_with_typed_error -- --nocapture
PASS — 1 deterministic typed slow-consumer test passed.

cargo test --locked -p worldstream-server
57 passed; 4 failed in pre-existing shared dirty-worktree tests:
  sqlite_backend::tests::unauthenticated_session_operations_are_forbidden
  tests::operator_member_capability_provisions_agent_principal_idempotently
  tests::operator_member_capability_route_proves_counter_live_path
  tests::operator_runner_capability_route_proves_runner_authority_path
The latest run showed the supervisor test passing; the remaining failures are
the unauthenticated-session expectation and three retained Counter golden
corpus mismatches from unrelated dirty Core changes. No lifecycle/passivation
code was changed in this lane.

cargo build --locked -p worldstream-server --bin worldstreamd
PASS

uv run --project sdk/python --locked pytest -q -rA sdk/python/tests examples/counter
PASS — 42 passed.

uv run --project sdk/python --locked ruff format --check sdk/python/src sdk/python/tests examples/counter
PASS — all files formatted.

uv run --project sdk/python --locked ruff check sdk/python/src sdk/python/tests examples/counter
PASS

cargo clippy --locked -p worldstream-server --all-targets -- -D warnings
PASS

The same strict Clippy command was rerun during the final audit and was
blocked before linting by the unrelated `worldstream-runtime` build-script
check: `compatibility.json drifted from compatibility.toml`. This shared
worktree baseline changed after the successful package Clippy run above; no
compatibility files were edited in this lane.

uv run --project sdk/python --locked python examples/counter/run_live_acceptance.py
PASS — status=completed; real HTTP/WebSocket/Python SDK/SQLite restart
scenario passed all criteria.

rg -n -P 'wsb1:[0-9a-f]{64}|Bearer [0-9a-f]{64}' examples/counter docs/agents/imo-55-live-counter-luna.md
PASS — no matches.
```

The full workspace contains unrelated dirty changes, so this lane did not run
cleanup, reset, commit, or edits outside the coordinated scope. The SQLite
change is confined to the `GatewayBackend` observation/read-suffix seam.
