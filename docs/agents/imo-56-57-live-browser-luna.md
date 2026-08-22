# IMO-56/57 real-browser evidence lane

Date: 2026-08-21

## Implemented boundary

The production console consumes a versioned live bootstrap from one in-memory
`window.__WORLDSTREAM_LIVE_SESSION__` value and deletes it immediately. It
constructs the native browser transport outside React and retains only a
bearer-free config in the component tree. Browser admission uses a short-lived,
single-use ticket; credentials are never placed in a URL, DOM attribute,
persistent browser storage, or report.

The cmux harness creates a disposable loopback `worldstreamd`, seeds one Agent
Heist Room through public HTTP/SDK surfaces, serves the production Vite bundle,
and injects bootstrap material only through an ephemeral init script. It closes
only its own browser surface. Preserved evidence removes credentials,
manifests, and the disposable database.

## Parent-verified real-browser evidence

```text
KEEP_LIVE_BROWSER_ARTIFACTS=1 WORLDSTREAM_BROWSER_STORY_TIMEOUT=240 \
  bash web/console/live-browser-story.sh

status=passed
evidence=real_cmux_browser_production_console_full_absent_broker_story
six_phase_stale_resync_privacy_replay_hashes=verified
evidence_root=/var/folders/gt/qcjtsn_937df98x1cb6_jmwh0000gn/T/worldstream-live-browser.XXXXXX.KZEFDyiq8w
```

The run used a real SQLite daemon, native browser WebSocket, browser admission
ticket, production console, Python SDK participants, and Runner activation
client. It proved:

- stale-Head rejection followed by a fresh-ticket/fresh-session resync and a
  second synchronization barrier;
- exact current-offer submissions for inspect, publish, plan, commitment, and
  result acknowledgement;
- Briefing, Negotiation, Commitment, Resolution, Result, and Complete;
- absent-Broker Activation recovery and durable timer-driven frame delivery;
- pre-completion final-reveal denial, completion, and exact Replay hash parity;
- deliberate disclosure of the published navigator claim;
- absence of a never-published Broker claim-code canary from public,
  participant, operator, Replay, trace, diagnostics, URL, and browser storage;
- absence of bearer/ticket wire patterns from production assets and retained
  evidence.

Two production defects and one stale SDK contract were found and fixed before
acceptance:

- successful operator timer commits now publish newly committed frames to
  attached streams;
- the server now fulfills its advertised heartbeat contract with bounded
  `server.ping` messages, preventing browser timeout during long Runner work;
- the Python replay validator accepts and validates the protocol's
  `room_health` and `integrity_generation` fields.

## Parent verification

```text
npm --prefix web/console test -- --run
# 7 files, 48 tests passed

npm --prefix web/console run build
npm --prefix web/console run lint
bash -n web/console/live-browser-story.sh
sdk/python/.venv/bin/ruff check examples/heist/wave10_live sdk/python/src sdk/python/tests
# passed

sdk/python/.venv/bin/python -m pytest sdk/python/tests -q
# 41 passed

cargo test --locked -p worldstream-server --all-targets --no-fail-fast
# 70 library and 7 daemon tests passed

cargo clippy --locked --no-deps -p worldstream-server --all-targets -- -D warnings
cargo build --locked -p worldstream-server --bin worldstreamd
# passed
```

## Changed files

- `web/console/src/liveSession.ts`
- `web/console/src/liveSession.test.ts`
- `web/console/src/main.tsx`
- `web/console/src/App.tsx`
- `web/console/live-browser-story.sh`
- `examples/heist/wave10_live/seed_browser_room.py`
- `examples/heist/wave10_live/run_browser_story.py`
- `examples/heist/wave10_live/browser_trace_init.js`
- `crates/worldstream-server/src/lib.rs`
- `sdk/python/src/worldstream_sdk/client.py`
- `sdk/python/tests/test_room_client.py`
- `docs/agents/imo-56-57-live-browser-luna.md`

## Completion boundary

The production-browser, exact-action, stale/resync, six-phase, privacy, and
Replay acceptance evidence required by IMO-56/57 is present. Cross-platform
packaging, PostgreSQL runtime, crash/power-loss, soak, and release-signing
evidence remain owned by their separate issues.
