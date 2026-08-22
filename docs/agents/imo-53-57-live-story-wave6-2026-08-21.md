# IMO-53/54/55/56/57 — Wave 6 deterministic live-story evidence

Date: 2026-08-21
Lane: parent-orchestrated Wave 6 live story
Repository: `/Users/vinothshanmugam/code/agent-streamer`
Evidence policy: fixture, in-process, and real process/network evidence are separated below.

## Result

This lane did not produce a full IMO-53/54/55/56/57 acceptance pass.

The strongest available live run reached the real `worldstreamd` process over HTTP
and WebSocket using the Python SDK. It proved daemon health, verified SQLite
startup, HTTP room creation, restart-safe create idempotency, `client.hello`, and
`server.welcome`. It then stopped at the real member authorization boundary:
the bootstrap capability is a global `HostOperator` capability and the daemon has
no public member-capability issuance path, so `room.attach` is rejected with
`forbidden`. Consequently no live Heist action, observation sync, Activation
delivery, or live console browser session can honestly be claimed from this run.

The deterministic Heist and Activation evidence remains useful, but is explicitly
not promoted to live process/network evidence.

## Commands and results

All commands below were run from the repository root. The live runner at
`examples/heist/wave6-live/run_live_story.py` creates a private temporary
directory and owner-only 32-byte bootstrap file, emits no secret material, and
removes the processes and temporary directory in `finally`. It is the only
additional file created by this lane, and it is within the explicitly allowed
Wave 6 runner directory.

### Real process, HTTP, WebSocket, and Python SDK

Build:

```text
cargo build --locked -p worldstream-server --bin worldstreamd
Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.78s
```

Exact command:

```sh
export PATH="/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH"
uv run --project sdk/python --locked python examples/heist/wave6-live/run_live_story.py
```

The runner used the real `target/debug/worldstreamd` with a temporary
`--data-dir` and loopback listener, then called `worldstream_sdk.Client` and
`worldstream_sdk.Room` against that listener. The Counter v2 request used the
resolved digest already embedded in the runtime:

```text
blake3:1c5f75068220f65f9017a062dbe40203c540108b572284d9446a329914008a92
```

Redacted result (the `room_head_digest` is run-specific because Genesis uses a
fresh random seed and generated IDs; all boolean/status assertions are stable):

```json
{"boundary":"bootstrap HostOperator can create rooms but cannot cross member attach/sync; the runtime has no public member-capability issuance path","evidence_class":"real_process_network_http_and_websocket_sdk","healthz_after_restart":200,"healthz_before":200,"http_create":{"committed":true,"member_count":1,"room_head_digest":"c7558d547a41efffc889bfff1a84e996d14631d89891179812ccf787c3631013","room_head_seq":0},"http_restart_idempotency":{"same_head":true,"same_member_ids":true,"same_room_id":true},"sdk_websocket":{"attach_error":{"code":"forbidden","message":"the capability is not authorized for this operation","retryable":false},"attach_error_after_restart":{"code":"forbidden","retryable":false},"selected_protocol":"0.1","welcome_received":true},"secrets":"not_emitted","status":"partial","version_after":{"engine_status":"verified","release_ready":false},"version_before":{"engine_status":"verified","release_ready":false}}
```

This is genuine loopback process/network evidence. It is not a successful
attach/sync/action story. The restart portion proves that the same temporary
SQLite data directory reopens, the HTTP create idempotency identity is stable,
and the protocol handshake still reaches `server.welcome`; it does not prove a
live Membership cursor reconnect because the first attach never crosses the
authorization boundary.

An earlier attempt using `Client.open_room()` reached the same boundary and
raised the SDK's closed `ProtocolError`:

```text
ProtocolError: the capability is not authorized for this operation
```

No bearer, secret file contents, or credential-derived value was printed.

### Deterministic Heist absent-Broker story (fixture evidence)

```text
python3 examples/heist/run_story.py --self-test
```

Result:

```text
status=ok
outcome=success
score=5
missing_roles=["broker"]
phase_path=[Briefing, Negotiation, Negotiation, Negotiation, Negotiation, Negotiation, Negotiation, Commitment, Commitment, Commitment, Commitment, Resolution, Result, Result, Result, Complete]
activation_created=4
verified_transition_count=15
replay_verified=true
transcript_digest=sha256:c31a01685be5d16cebba477e16c51d0a923b8ca30202e7fd0724d10dc5f505e8
```

The command's complete JSON also reported:

```text
replayed_activity_state_hash=sha256:a55c41b8ec66d6d7a8ecfc3f48188ec1b20f1346936d950d81ed54fbc7aa517b
authoritative_state_hash=sha256:a63fa15f679ab486627052ed482c4e8bdf2b9a83a5579078c9df757c7c029831
core_state_hash=sha256:53a55ca514d3e1e950be3a4d333854a519007f5f9aebd11d39358c0ae774328a
lineage_hash=sha256:6ae8ad69d00de0c6489a427818f03f4bcaed75919d207470b4420593369b0cae
```

Classification: deterministic offline fixture. It proves the absent-Broker
reducer result and retained activation records in the fixture harness only. It
does not prove a `worldstreamd` Heist room, WebSocket delivery, or provider
execution.

### Heist, privacy, and Activation checks

```text
python3 -m unittest discover -s examples/heist -p 'test_*.py' -q
Ran 11 tests in 0.111s — OK

cargo test --locked -p worldstream-core --lib activation -- --nocapture
1 passed, 141 filtered out

cargo test --locked -p worldstream-core --lib agent_heist -- --nocapture
17 passed, 125 filtered out
```

The Activation test covers pure Attention normalization and context preparation.
The Heist tests cover the six-phase timer path, majority/replay behavior,
absent-Broker outcome, privacy projections, Attention precedence, and terminal
reveal boundaries. These are in-process/fixture checks, not live transport
evidence.

### Python SDK seam

```text
uv run --project sdk/python --locked pytest -q
37 passed in 12.78s
```

The SDK tests cover protocol validation, attach/reset/sync bookkeeping, cursor
acknowledgement, reconnect state handling, action request/reply handling, and
closed error projection. The real-process run separately proved the SDK's
WebSocket handshake against `worldstreamd`; it could not reach SDK `sync()` or
`act()` because the server correctly rejected member attach.

### Console live-session seam

```text
pnpm --dir web/console test -- --run
Test Files  3 passed (3)
Tests  19 passed (19)

pnpm --dir web/console build
tsc --noEmit && vite build
21 modules transformed
```

The console tests exercise `WebSocketWorldStreamTransport`, live-session state
projection, reset/sync-barrier transitions, observation delivery, action/error
projection, and the explicit fixture fallback. The production build emitted:

```text
web/console/dist/index.html                         8088f821d4dbd9ab99e44e61381898e443fb8be58fc0815685d0ab45e0a0828c
web/console/dist/assets/index-CxwigASf.js           704b36f755129ec3b8993488df07428c5520936bf84745fd8b1f8175330aef58
web/console/dist/assets/index-BXEVLsje.css          39c5810fb13199c3efb2066ef8936857f935043874d42f50b16bda09c372fd74
```

Classification: in-process and build evidence. No browser automation or
browser-backed live session was available in this checkout. In addition, the
console's browser-native WebSocket path cannot supply the required bearer
header; the configured live path deliberately requires a header-capable
WebSocket adapter. Therefore this lane does not claim browser/network console
evidence.

## Acceptance matrix

| Issue | Evidence established | Evidence still missing | Classification |
| --- | --- | --- | --- |
| IMO-53 | Six-phase reducer, timers, majority/replay, and deterministic story pass; real daemon handshake reaches protocol | A live registered Heist pack, real room attach/sync, action/event sequence, restart/reconnect cursor proof | Partial; fixture + real handshake |
| IMO-54 | Privacy tests, Attention precedence, pure Activation context test, and fixture activation records pass | Live authorized Activation offer/claim/complete boundary and delivery through a running server | Partial; in-process/fixture |
| IMO-55 | 37 SDK tests; real SDK `client.hello`/`server.welcome`; restart-safe HTTP idempotency | Member capability issuance, successful attach/sync/ack/action, and live reconnect | Partial; real handshake only |
| IMO-56 | 19 console tests and production build pass; live-session seam is explicit and fail-closed | Browser-backed session against an authorized live room | Partial; in-process/build |
| IMO-57 | Absent-Broker fixture reaches success, score 5, missing Broker, 4 activations, verified replay | `worldstreamd` must register/select Agent Heist and expose an authorized member/runner path | Partial; fixture only for story |

## Residual gaps and next required evidence

1. Register a verified `worldstream.agent-heist` runtime in the actual daemon
   registry, resolving its manifest/revision identity. The current live daemon
   selects the Counter registry only; the manifest reports the Heist revision
   digest as unresolved.
2. Provide a deliberate, audited member-capability issuance path (or an
   equivalent test-only operator bootstrap seam) so a real client can cross
   `room.attach`, `room.sync_ack`, and `observation.ack`. The current bootstrap
   capability is intentionally host-scoped and cannot be reused as a member
   capability.
3. With that capability, rerun the Python SDK story through a real Heist room:
   absent Broker, six timer phases, Attention/Activation records, action
   acceptance/rejection, retained/reset delivery, cursor acknowledgement, and
   process restart/reconnect.
4. Run the console in a browser with a header-capable WebSocket adapter or a
   reviewed browser authentication mechanism, then capture browser evidence for
   the live-session status, reset barrier, observations, action submission, and
   error/reconnect states.
5. Keep `release_ready=false`; this lane did not modify compatibility manifests
   or claim release evidence.

## Scope compliance

Only this document and the permitted `examples/heist/wave6-live/run_live_story.py`
runner were added by the lane. No production source, compatibility manifest,
`release_ready` value, or existing documentation was modified.
