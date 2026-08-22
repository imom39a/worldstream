# IMO-55/56/57 browser-safe authorization lane

Date: 2026-08-21
Repository: `/Users/vinothshanmugam/code/agent-streamer`

## Result

Implemented the browser-native WebSocket authorization seam without changing
the existing header-capable SDK/extension path.

The native console path now:

1. sends the runtime bearer in an authenticated, no-store `POST
   /v1/stream/ticket` request;
2. receives a versioned `browser_ws_ticket.v1` response containing an opaque
   `wst1:` ticket;
3. sends only that ticket as the first native WebSocket text frame; and
4. sends the normal `client.hello` only after the ticket frame.

The daemon retains only a BLAKE3 digest of each ticket in process memory. The
ticket is 256-bit random, origin-bound, capped at 256 outstanding entries,
expires after 15 seconds, and is removed on every consume attempt, including
wrong-origin, expired, malformed, and replayed attempts. The store is not
persisted. Loopback `http`/`https` origins are required for browser ticket
issuance and native WebSocket admission; header-capable connections without an
Origin remain compatible for SDKs/extensions.

All three server randomness sites—the browser ticket, SQLite room seed, and
SQLite bearer generation—now use the pinned portable OS CSPRNG
`getrandom 0.4.3`; no server path opens `/dev/urandom`. Randomness failures
retain typed fail-closed mappings. Native admission waits for the first ticket
frame for exactly the 15-second ticket TTL. Timeout, receive failure, malformed,
unknown, expired, replayed, and wrong-origin tickets close with WebSocket policy
code `1008` and the generic reason `browser authorization failed`, without
echoing bearer or ticket material.

Failure responses are closed and generic. The bearer and ticket are absent from
URLs, WebSocket subprotocols, logs/diagnostics, DOM, browser storage, errors,
and close reasons. The console clears its bearer copy after header/ticket
preparation and on close/failure.

## Exact lane files

- `crates/worldstream-protocol/src/messages.rs`
  - versioned browser ticket response shape and redacted `Debug`.
- `crates/worldstream-server/src/lib.rs`
  - in-memory ticket store and lifecycle policy;
  - portable CSPRNG helper and bounded first-frame admission timeout;
  - loopback Origin validation and CORS preflight/response headers;
  - `/v1/stream/ticket` issuance;
  - ticket-first native admission for room and runner WebSockets;
  - lifecycle, capacity/expiry, origin, route, and header compatibility tests.
- `crates/worldstream-server/src/sqlite_backend.rs`
  - portable CSPRNG for room seeds and bearer bytes with storage-unavailable
    fail-closed mapping.
- `Cargo.toml`, `Cargo.lock`, and `crates/worldstream-server/Cargo.toml`
  - pinned `getrandom` dependency and Tokio time support for the bounded
    admission wait.
- `web/console/src/transport.ts`
  - native `fetch` ticket exchange and ticket-first WebSocket handshake;
  - preserved header-capable adapter behavior;
  - memory-only credential/ticket handling and closed error paths.
- `web/console/src/liveSession.ts`
  - testable live-session transport construction and ticket configuration.
- `web/console/src/transport.test.ts`
  - native browser constructor, URL/subprotocol privacy, ticket validation,
    issuance failure tests, and the renamed ticket-issuance test description.
- `web/console/src/liveSession.test.ts`
  - native live-session transport handoff test.
- `web/console/src/privacyDom.test.tsx`
  - runtime bearer/ticket/Authorization DOM exclusion test.
- `web/console/browser-privacy-smoke.sh`
  - browser DOM scan now also rejects ticket-shaped credential material.

## Verification

Passed:

```text
cargo test --locked -p worldstream-protocol -p worldstream-server
12 protocol tests passed; 56 server-library tests passed; 7 worldstreamd tests
passed; worldstreamctl and doc-test targets passed

cargo clippy --locked -p worldstream-protocol -p worldstream-server \
  --all-targets -- -D warnings
passed

cargo fmt --package worldstream-protocol --package worldstream-server -- --check
passed

bash -n web/console/browser-privacy-smoke.sh
passed

PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH \
  pnpm test
43 tests passed across 6 files

PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH \
  pnpm run build
TypeScript check and Vite production build passed

PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH \
  START_PREVIEW=1 CONSOLE_PORT=4178 bash web/console/browser-privacy-smoke.sh
passed: public, participant, operator, Replay, recovery, fault, quarantine,
and terminal fixture routes without credentials or ticket-shaped DOM material

git diff --check -- Cargo.toml Cargo.lock \
  crates/worldstream-server/Cargo.toml crates/worldstream-protocol/src \
  crates/worldstream-server/src/lib.rs crates/worldstream-server/src/sqlite_backend.rs \
  web/console/src web/console/browser-privacy-smoke.sh
passed
```

The full allowed package test run and all-target clippy passed. The shared dirty
worktree was preserved; no reset or cleanup was performed.

## Remaining live boundary

This lane proves the browser-safe transport and daemon admission contracts, but
does not claim a complete live Heist projection/action run. That still requires
a running daemon with a seeded, currently authorized Room/Membership and a
browser served from an allowed loopback Origin. The WebSocket server-side
backend `hello` and subsequent Room attach/sync remain the final live boundary;
the browser fixture/privacy smoke is not evidence of those authorized domain
frames or mutations.
