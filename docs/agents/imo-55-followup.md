# IMO-55 gateway and Python SDK follow-up

This follow-up strengthens the bounded protocol path without claiming live
network or process-restart evidence.

## Orchestrator addendum (2026-08-20)

The parent reviewed and integrated three additional disjoint Luna lanes:

- `SqliteGatewayBackend` now authenticates canonical `wsb1:` bearers against
  durable SQLite authority, binds a bounded session-to-capability map, serves
  verified current projections, maps authorized Observation ACKs, and now
  converts the storage attach/reset-or-retained result into the public
  protocol. Attach issues a gateway token backed by Core's opaque
  `SessionSyncTokenV1`; `room.sync_ack` revalidates the session, capability,
  Room, Membership, baseline, and Core token before releasing the suffix.
  Create now uses the Core-authorized SQLite Genesis path, including durable
  idempotent replay and same-key semantic conflict handling. Action remains
  explicitly fail-closed because the current public Core seams do not expose
  a safe mutable actor-trace path.
- `worldstreamd` wires that backend for the documented `sqlite-bundled`
  profile at `data_dir/worldstream.sqlite3`; PostgreSQL startup remains a safe
  unavailable-backend error and `/readyz` remains truthful rather than being
  promoted to ready prematurely.
- `web/console/src/transport.ts` adds strict bounded envelope/request
  validation and a controllable in-memory transport seam. The UI remains
  fixture-backed until live WebSocket authentication and delivery are proven.

The parent also refreshed `Cargo.lock`, added active/revoked/expired bearer
filtering at the SQLite authentication boundary, and reran the full workspace:
the current parent verification has 25 server tests, 12 protocol tests, and
332 Rust tests across the workspace with formatting, build, and strict clippy
passing. Python/UI checks remain environment-dependent when `uv`/`pnpm` are
not installed.

## Implemented

- `crates/worldstream-protocol/src/messages.rs`
  - Added a deterministic required-capability set (`cursor_ack` and
    `projection_reset`) and validation on `ClientHello`.
  - Require non-empty room, membership, and action identities before an action
    reaches a backend.
  - Kept the existing strict versioned envelope, unknown-field rejection, and
    message/action byte bounds.
- `crates/worldstream-server/src/lib.rs`
  - Rejects a hello that cannot support the cursor/reset synchronization
    contract.
  - Adds a bounded 1,024-frame attach/sync burst and returns retryable
    `slow_consumer` when the retained response is larger.
  - Emits an explicit `room.sync_acked` barrier after retained/reset frames.
  - Includes room and membership identity in observation acknowledgements and
    preserves those fields safely across backend request ownership.
- `crates/worldstream-sqlite/src/lib.rs`
  - Resolves bearer tokens only through the durable capability and principal
    tables; a raw bearer is never treated as universal authority.
  - Exposes a verified Room snapshot seam that rechecks the exact recovered
    Head and observation frame barriers before a gateway adapter can publish
    them.
- `crates/worldstream-server/src/sqlite_backend.rs`
  - Maps the storage-only attach delivery into `room.attached`, optional
    `projection.reset`, and bounded `observation.deliver` protocol values.
  - Keeps the Core synchronization token private and binds its wire handle to
    the authenticated transport Session; mismatched or replayed acknowledgements
    remain rejected.
  - Preserves canonical Action Offers when translating observation payloads.
  - Creates Counter Rooms through the authorized Core Genesis and SQLite
    commit seams, returning the durable response for an idempotent replay and
    rejecting a same-key request with a different semantic hash.
- `sdk/python/src/worldstream_sdk/client.py`
  - Waits for the explicit sync barrier before marking a Room live, so reset
    and retained frames are not silently left in the socket.
  - Fails closed when an inbound room/member frame is addressed to another
    Membership.
  - Preserves exact action identity for retry and exposes accepted duplicate
    and rejected stale/conflict outcomes without reinterpretation.
- `sdk/python/tests/test_room_client.py`
  - Covers sync barrier/reset/frame draining, reconnect from the last ACK'd
    cursor, cross-membership frame rejection, duplicate acceptance, stale
    rejection, lost-reply retry identity, strict JSON, and payload bounds.

## Verification

- `cargo fmt --all` — passed.
- `cargo test --locked -p worldstream-protocol -p worldstream-server` — passed;
  12 protocol tests and 25 server tests in the current parent checkout.
- `cargo clippy --locked -p worldstream-protocol -p worldstream-server
  --all-targets -- -D warnings` — passed.
- The current parent workspace verification also passes `cargo test
  --workspace --locked` (332 Rust tests), `cargo fmt --all -- --check`, and
  workspace clippy with `-D warnings`.
- The earlier `uv` SDK and Ruff results remain historical evidence; the
  current environment does not have `uv`, so those commands were not rerun in
  this continuation.
- `git diff --check` — passed.

## Remaining gaps

These changes now provide a bounded SQLite-backed `GatewayBackend` facade for
authentication, verified current projection, authorized Counter creation,
Observation ACK, attach, and session-bound sync acknowledgement, plus daemon
startup wiring. They do not yet map participant Actions into the public
protocol. They also do not prove a live WebSocket deployment, broker/pooler
behavior, slow-consumer behavior under a real network, or a Linux live
restart/reconnect run. Backend implementations still own authoritative head
comparison and the duplicate/lost/conflict/stale action decision; the gateway
preserves bounded transport and remains fail-closed for unsupported
operations.
