# HTTP and WebSocket reference

The normative wire contract is `docs/protocol.md`. Build clients from its exact
versioned envelopes and schemas; do not infer a public API from internal Studio
routes or test fixtures.

## Transport boundaries

| Boundary | Purpose |
| --- | --- |
| Room WebSocket | Membership attach, Projection Reset, observations, ACK, Actions |
| Runner WebSocket / HTTP long poll | Activation offers, claims, renewals, completion/release |
| HTTP resource APIs | health/version, catalogs, creation, current projection, Replay, operator operations |
| Studio Supervisor HTTP | local browser-safe bounded workflows; not the public Room protocol |
| assignment MCP stdio | agent-friendly sealed assignment contract |

## Room client lifecycle

```text
connect with exact subprotocol + scoped bearer
  → client hello (version, mode, cursor/reset capabilities)
  → server welcome (session identity)
  → room.attach (Room + Membership + cursor)
  → projection.reset or retained observation catch-up
  → room.sync_ack installs the captured baseline
  → live observation.deliver / observation.ack
  → exact action.submit / accepted|rejected
```

Attachment captures a complete upper Head and buffers later frames while the
client installs the reset/catch-up. The explicit sync token closes the gap; a
client cannot silently treat partial installation as live.

## Action submission

An Action request binds:

- protocol/request correlation identity;
- stable action/operation identity;
- exact Room and Membership authority;
- current Action Offer identity and typed payload;
- exact Head sequence/hash precondition;
- canonical bounded payload bytes.

On lost response, resolve/retry the identical operation. On a stable stale
result, attach/sync again and obtain a new current offer. Never change the body
under the same identity.

## Observation delivery

Observation Frames are per Membership and ordered by frame sequence. ACK only
after durable application. A reset establishes a complete authorized baseline
when history was pruned, visibility changed, or first attach requires it.

Catch-up consumes Observation Frames; Replay reconstructs Canonical History.
They have different authorization, positions, and side effects.

## Activation

Runner handshake binds one exact Runner authority. Activation offer/claim uses
a lease generation and exact context hash. Renew, complete, and release are
fenced to the current lease. Completion is `handled`, `declined`, or `failed`.

## HTTP resource categories

The current daemon implements:

- `/healthz`, `/readyz`, `/version`, and metrics;
- installed Activity Pack catalog/revision reads;
- development Room creation and capability provisioning;
- current Projection and authorized historical Replay;
- operator Room inventory/detail, timers, launch, Runner presence, Activation
  status, and live backup;
- browser ticket plus Room and Runner WebSocket upgrade routes.

Treat development-administration routes as local operator surfaces, not an
internet-facing tenant API.

## Errors and bounds

Errors use a versioned closed envelope with stable code, safe message, retry
classification/next action where declared, and no raw provider/private data.
Unknown fields, oversized frames/payloads, noncanonical IDs/bearers, version
mismatch, and invalid state combinations fail closed.

Primary source: [wire protocol](https://github.com/imom39a/worldstream/blob/main/docs/protocol.md),
[protocol types](https://github.com/imom39a/worldstream/tree/main/crates/worldstream-protocol/src),
and [server router](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-server/src/lib.rs).
