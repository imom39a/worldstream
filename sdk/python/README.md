# WorldStream Python SDK

This package provides the bounded asynchronous client for the public v0.1
Room protocol. It uses one scoped `wsb1:` bearer capability, strict
canonical-JSON validation, and one WebSocket per Room Membership.

```python
from worldstream_sdk import Client, LostActionReply

client = Client("http://127.0.0.1:8080", "wsb1:" + "ab" * 32)
room = await client.open_room(room_id, member_id)
for observation in await room.sync():
    ...
```

`Room.sync()` does not advance the durable Observation Cursor. After durable
processing, call `await room.ack(frame_seq)`. A lost Action reply raises
`LostActionReply`. Retry the retained request with the same Action identity and canonical body. If the Session has closed, reconnect first.

The SDK rejects malformed envelopes, unknown fields, cross-membership frames,
non-canonical numbers, oversized payloads, and mismatched complete Heads before
exposing them to the caller. Bearers are sent only in the HTTP Authorization
header and are redacted from typed error details.

An accepted Action installs a newer complete Room Head without rewinding on an
older duplicate result. A stale rejection never rebases the Action: call
`await room.resync()` before submitting a revised Action identity. Reusing an
Action ID with a changed canonical body fails locally with
`idempotency_conflict`.

A Runner can advertise legacy Pack IDs and exact immutable Pack revisions. Use
the exact form when launch readiness is pinned to one revision:

```python
pack = {
    "id": "worldstream.agent-heist",
    "version": "0.2.0",
    "digest": "blake3:...",
}
runner = await client.open_runner(runner_id, 1, [pack["id"]], [pack])
```

An omitted fourth argument preserves the legacy Runner handshake. An exact
revision list is bounded, rejects duplicates, and is sent only on the Runner
control stream.

This is a protocol/client surface, not live deployment evidence. The checked-in
offline Agent Heist story under `examples/heist/` remains dependency-free and
explicitly does not claim daemon, database, browser, model, or power-loss
execution.

### Explicit compact history selection

`Client.create_room` accepts the optional `canonical_history_format` field. Use
`worldstream/transition/v1` or `worldstream/transition/v2`. Omission selects V1.
Explicit V1 sends the same four fields as an omitted selection. The server must
enable V2 creation before it can accept V2. Null and unknown selections fail
before HTTP submission. Keep the same request identity and format on retries.

`PAYLOAD_BUDGET_V1_ID` and `PAYLOAD_BUDGET_V1_LIMITS` describe the frozen compact
Room policy. Call `check_fresh_payload(value, kind=..., policy_id=...)` only when
you choose to check unresolved fresh work against that exact Room policy. It
counts canonical UTF-8 bytes, including JSON fields and delimiters. Supply the
complete array or accounting object for aggregate kinds. The server remains
responsible for authority, schema validation, and durable receipt resolution.
Complete authorized views include schema identities, Action Offers, and
authorized Core fields where present.

The normal Action transport retains its 256 KiB hard guard. Fresh-work hints
do not run automatically on sends or lost-reply retries. Use
`check_declared_artifact_reference` only for an application-declared reference
schema. Validate that schema separately. The helper grants no download authority.
