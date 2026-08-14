# Wire Protocol and Data Model

## Status and scope

This document specifies the native v0.1 protocol shape. Message names and fields are provisional until the first conformance fixtures are implemented, but their semantics must remain consistent with [Frozen Requirements](requirements.md).

The protocol serves:

- human participant clients;
- agent participant clients;
- external agent runners;
- public/read-only reference UI clients;
- local host-operator tooling.

It does not carry model token streams, hidden reasoning, arbitrary workflow nodes, cross-room project data, payment messages, or pack-supplied executable UI.

## Protocol principles

1. HTTP establishes resources and handles bounded request/response operations.
2. WebSocket carries live room observations, participant actions, and runner activation offers.
3. One room-client WebSocket attaches to one Membership in v0.1; spectator and operator Memberships are read-only, not special cursorless streams.
4. One runner control connection may claim Activations for several Agent Participant Memberships authorized by its capability.
5. The server commits accepted actions before replying or streaming consequences.
6. Network delivery is at least once; IDs and cursors make duplicates safe.
7. Delivery catch-up never implies resuming a model's mind or process.
8. Clients never receive raw authoritative room state.

## Vocabulary represented by the protocol

| Concept | Durable | Owned by |
|---|---|---|
| Principal | Yes | WorldStream identity metadata |
| Room membership | Yes | WorldStream and room policy |
| Room-member session | No | Gateway connection |
| Runner registration/capability | Yes until expiry/revocation | Host operator and WorldStream |
| Runner connection | No | Gateway connection |
| Model invocation | No; opaque optional run ID only | External runner |
| Action result | Yes | Room transition store |
| Observation cursor | Yes | Membership inbox |
| Activation intent | Yes | Activation queue |

Membership lifecycle is enabled, suspended, or departed.

Session lifecycle is connected or disconnected.

Activation lifecycle is pending, leased, completed, expired, or cancelled.

These are independent dimensions.

A Room is described across four independent axes:

- core status: active or archived;
- health: healthy, faulted, or quarantined;
- Activity Phase: a pack-defined stage such as Commitment, Complete, Review, or Closed;
- Outcome: a separate pack-defined result such as success, partial failure, or a deterministic score.

Archived, faulted, and quarantined rooms reject ordinary participant mutation. Authorized projection, export, diagnostics, repair, and replay remain available as appropriate. A terminal pack phase normally rejects domain actions through pack rules while the core room may remain active until the host operator archives it.

## Transport

### Room client WebSocket

Endpoint:

    GET /v1/stream

The client supplies a scoped bearer capability in the HTTP Authorization header. Tokens are never placed in query strings or logged.

The server negotiates the subprotocol:

    worldstream.json.v0.1

Text frames contain strict JSON. Binary frames are reserved. Message compression is disabled by default until memory and decompression limits are tested.

### Runner control WebSocket

Endpoint:

    GET /v1/runner/stream

This connection offers activation IDs and accepts lease operations. It does not automatically attach as a room participant and does not receive private room content before a successful activation claim.

### HTTP

HTTP is used for:

- health, readiness, version, and metrics;
- development identity and capability administration;
- room and membership creation;
- authorized current-projection reads;
- activation long poll and claim fallback;
- replay reads;
- v0.2 artifact upload and download;
- host-operator backup and diagnostics through local tooling.

All mutating HTTP operations use a durable idempotency key scoped by authenticated principal and operation. The resource change and mutation receipt commit together. Same key/request hash returns the original response; same key/different request returns idempotency_conflict.

## Common envelope

Every WebSocket message has this shape:

~~~json
{
  "protocol": "0.1",
  "type": "action.submit",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {}
}
~~~

Fields:

| Field | Required | Meaning |
|---|---|---|
| protocol | Yes | Major/minor wire version |
| type | Yes | Stable message discriminator |
| message_id | Yes | ULID unique for this transmitted envelope |
| request_id | Client requests and correlated replies | ULID chosen by requester |
| body | Yes | Strict type-specific object |

message_id is not an action idempotency key. Retransmitting the same logical action may use a new envelope message ID but MUST reuse the same action_id.

Unknown message types receive unsupported-message. Unknown fields receive invalid-envelope unless the negotiated schema explicitly marks an extension object.

## Handshake

The first client message MUST be client.hello:

~~~json
{
  "protocol": "0.1",
  "type": "client.hello",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "client_name": "heist-rule-runner",
    "client_version": "0.1.0",
    "mode": "participant",
    "supported_protocols": ["0.1"],
    "capabilities": ["cursor_ack", "projection_reset"]
  }
}
~~~

mode is participant, spectator, operator, or runner. Runner mode is valid only on the runner endpoint.

The server replies server.welcome:

~~~json
{
  "protocol": "0.1",
  "type": "server.welcome",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "session_id": "01K...",
    "selected_protocol": "0.1",
    "server_version": "0.1.0-dev",
    "heartbeat_interval_ms": 15000,
    "maximum_message_bytes": 524288,
    "authenticated_principal": {
      "principal_id": "01K...",
      "kind": "agent"
    }
  }
}
~~~

Failure to negotiate closes the socket after a typed error.

## Room attachment

### room.attach

A Room Member attaches a session to an existing Membership:

~~~json
{
  "protocol": "0.1",
  "type": "room.attach",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "room_id": "01K...",
    "member_id": "01K...",
    "after_frame_seq": 184
  }
}
~~~

after_frame_seq is the last fully processed observation frame for this membership. It is not the room sequence and does not describe model execution.

The capability must authorize the principal, room, and membership. A session cannot attach as another membership merely by knowing its ID.

### room.attached

If retained frames cover the cursor:

~~~json
{
  "protocol": "0.1",
  "type": "room.attached",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "room_id": "01K...",
    "member_id": "01K...",
    "principal_kind": "agent",
    "access_mode": "participant",
    "role": "navigator",
    "membership_status": "enabled",
    "room_status": "active",
    "room_health": "healthy",
    "room_head_seq": 91,
    "last_ack_frame_seq": 184,
    "catch_up_through_frame_seq": 191,
    "pack": {
      "id": "worldstream.agent-heist",
      "version": "0.1.0",
      "digest": "blake3:..."
    }
  }
}
~~~

The server then delivers frames 185 through 191. `role` is present only for participant-access Memberships; spectator and operator Memberships have no pack-defined Role.

If the cursor is too old, room.attached contains resync_required: true, followed by projection.reset. It never silently starts at the newest frame.

Attachment is serialized through the room actor. The actor captures membership frame head H and marks the session catching_up before storage reads begin. The server pages retained frames through H with short read transactions while buffering newly committed frames above H in the bounded session queue. It switches to live only after the client path has installed the through-H range or reset. Buffer overflow closes the socket and requires another resumable attachment.

### projection.reset

~~~json
{
  "protocol": "0.1",
  "type": "projection.reset",
  "message_id": "01K...",
  "body": {
    "room_id": "01K...",
    "member_id": "01K...",
    "room_head_seq": 91,
    "room_health": "healthy",
    "frame_seq": 191,
    "projection_schema": "worldstream.projection.v1",
    "projection": {
      "core": {
        "room_status": "active",
        "membership": {
          "status": "enabled",
          "access_mode": "participant",
          "role": "navigator"
        }
      },
      "activity": {
        "schema": "worldstream.agent-heist.navigator.v1",
        "value": {
          "phase": "commitment",
          "deadline": "2026-08-13T18:30:00Z",
          "legal_actions": ["commit_move"]
        }
      }
    },
    "projection_hash": "blake3:..."
  }
}
~~~

The `projection` value contains authorized Core Room State and Membership metadata plus the pack-owned Activity Projection. The surrounding body is the Projection Envelope; `room_health` is operational metadata outside the Projection. A client processes the reset atomically, stores `frame_seq` as its new baseline, and acknowledges it.

`projection_hash` is BLAKE3 over the canonical object `{domain: "worldstream/projection-hash/v1", projection_schema, projection}`. It excludes Projection Envelope fields such as message ID, Room Health, Room sequence, frame sequence, and delivery time, so operational changes do not alter an otherwise identical Projection hash.

## Action submission

Humans and agents use the same message.

### action.submit

~~~json
{
  "protocol": "0.1",
  "type": "action.submit",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "room_id": "01K...",
    "member_id": "01K...",
    "action_id": "01K...",
    "based_on_room_seq": 91,
    "action_type": "commit_move",
    "payload": {
      "plan_id": "01K...",
      "sealed_choice": "..."
    }
  }
}
~~~

Semantics:

- action_id is the durable idempotency key;
- based_on_room_seq expresses the state on which the participant decided;
- the server authenticates identity and membership before pack validation;
- the Room Kernel rejects the action unless based_on_room_seq exactly equals the current room head in v0.1 and v0.2;
- admitted_at logical time is chosen by the host and recorded in the Stimulus candidate;
- an accepted action is not acknowledged until the complete transition transaction commits.

### action.accepted

~~~json
{
  "protocol": "0.1",
  "type": "action.accepted",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "room_id": "01K...",
    "member_id": "01K...",
    "action_id": "01K...",
    "room_seq": 92,
    "transition_id": "01K...",
    "state_hash": "blake3:...",
    "duplicate": false
  }
}
~~~

If a retry finds the stored result, duplicate is true and every other authoritative field matches the original reply.

### action.rejected

~~~json
{
  "protocol": "0.1",
  "type": "action.rejected",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "room_id": "01K...",
    "member_id": "01K...",
    "action_id": "01K...",
    "code": "stale_room_state",
    "message": "The commitment window is now active.",
    "current_room_seq": 93,
    "legal_actions": ["commit_move"],
    "retryable_with_same_action_id": false,
    "may_submit_revised_action": true,
    "details": {}
  }
}
~~~

A deterministic admitted rejection consumes no canonical room sequence but does consume the action ID through a durable domain_rejected receipt. The same ID returns the same rejection. A revised decision uses a new action ID.

Durable action.rejected codes include:

- membership_not_enabled;
- action_not_allowed;
- stale_room_state;
- deadline_passed;
- room_archived;
- activity_terminal;
- activity_domain_rejection.

Malformed/invalid payload, unauthenticated, forbidden, idempotency_conflict, room_busy, rate_limited, storage_unavailable, and activity/runtime faults use the generic error envelope. They do not create an action receipt or consume a previously unseen action ID. A transient retry MUST reuse the same action ID and identical canonical action body.

## Observation frames

### observation.deliver

~~~json
{
  "protocol": "0.1",
  "type": "observation.deliver",
  "message_id": "01K...",
  "body": {
    "room_id": "01K...",
    "member_id": "01K...",
    "frame_seq": 192,
    "cause_room_seq": 93,
    "frame_kind": "delta",
    "observation_schema": "worldstream.agent-heist.navigator.delta.v1",
    "observation": {
      "reason": "commitment_opened",
      "changes": [],
      "legal_actions": ["commit_move"],
      "deadline": "2026-08-13T18:30:00Z"
    },
    "payload_hash": "blake3:..."
  }
}
~~~

One transition may produce:

- no frame for an unaffected membership;
- one or more member-private frames;
- public consequences copied into every authorized enabled membership stream;
- operator-membership-only consequences copied only into operator memberships.

A frame MUST name exactly one recipient Membership and MUST NOT contain hidden Authoritative Room State or another Membership's private payload. frame_seq is monotonic within that Membership's Observation Stream and may skip Room Transitions that were irrelevant or unauthorized.

### observation.ack

~~~json
{
  "protocol": "0.1",
  "type": "observation.ack",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "room_id": "01K...",
    "member_id": "01K...",
    "through_frame_seq": 192
  }
}
~~~

The client sends this only after it has durably processed all frames through that sequence. The server may batch cursor persistence. A crash before persistence can cause duplicate delivery but not a missing frame.

If multiple Sessions attach to the same Membership, they share this Cursor. An acknowledgement by one advances it for all; clients that need independent processing positions require separate Memberships.

The server replies observation.acked with the durably stored cursor when a reply is requested.

## Activation protocol

### Meaning

An activation intent says:

> This Agent Participant may need a fresh Invocation because this typed Room change is relevant.

It does not mean an agent was continuously alive, saw the change, or has already run.

### runner.hello

The runner control connection declares only operational capacity:

~~~json
{
  "protocol": "0.1",
  "type": "runner.hello",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "runner_id": "01K...",
    "maximum_concurrent_activations": 2,
    "supported_pack_ids": ["worldstream.agent-heist"]
  }
}
~~~

runner_id is server-issued and MUST match the authenticated capability binding. The bearer capability, not the declared supported-pack list, determines which Memberships may be claimed.

### activation.offer

An offer contains no private room projection:

~~~json
{
  "protocol": "0.1",
  "type": "activation.offer",
  "message_id": "01K...",
  "body": {
    "activation_id": "01K...",
    "room_id": "01K...",
    "member_id": "01K...",
    "pack_id": "worldstream.agent-heist",
    "reason_code": "commitment_opened",
    "priority": 100,
    "deadline": "2026-08-13T18:30:00Z",
    "lease_duration_ms": 30000
  }
}
~~~

Offers may be duplicated and may race another authorized runner.

### activation.claim

~~~json
{
  "protocol": "0.1",
  "type": "activation.claim",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "activation_id": "01K...",
    "runner_id": "01K...",
    "claim_id": "01K...",
    "requested_lease_ms": 30000
  }
}
~~~

claim_id is idempotent for this activation and authenticated runner. The database atomically grants at most one current lease and persists a canonical claim-request hash plus the result. Same claim ID and same request returns that original result after a lost reply; a changed request or different authenticated runner is an idempotency conflict. A runner uses a new claim ID if it wants to try again after a stored not-available result.

### activation.claimed

Only after claim authorization does the server return private invocation context:

~~~json
{
  "protocol": "0.1",
  "type": "activation.claimed",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "activation_id": "01K...",
    "claim_id": "01K...",
    "lease_generation": 3,
    "lease_until": "2026-08-13T18:29:45Z",
    "cause_room_seq": 93,
    "reason_code": "commitment_opened",
    "relevant_frame_range": {
      "from": 185,
      "through": 192
    },
    "deadline": "2026-08-13T18:30:00Z",
    "allowed_action_types": ["commit_move"],
    "room_head_seq": 93,
    "projection": {
      "core": {
        "room_status": "active",
        "membership": {
          "status": "enabled",
          "access_mode": "participant",
          "role": "navigator"
        }
      },
      "activity": {
        "schema": "worldstream.agent-heist.navigator.v1",
        "value": {
          "phase": "commitment",
          "legal_actions": ["commit_move"]
        }
      }
    },
    "observation_frames": [],
    "artifact_references": []
  }
}
~~~

The runner uses this payload to start a new invocation or route work to a bounded runner-owned execution runtime. WorldStream does not know which model is called.

The claim capability authorizes Activation handling only. To submit a domain Action, the Runner or Invocation uses separate participant authority bound to the target Principal and Membership. Receiving context, claiming, renewing, releasing, or completing the Activation does not advance the Membership Cursor; an authorized room client acknowledges Observation Frames explicitly after durable processing. See [ADR 0003](adr/0003-separate-activation-and-action-authority.md).

### activation.renew

A runner may extend a lease within the server's maximum. The request MUST include activation_id, claim_id, and lease_generation. Renewal conditionally matches the authenticated runner and current unexpired lease. It is operational and does not change canonical room sequence.

### activation.complete

~~~json
{
  "protocol": "0.1",
  "type": "activation.complete",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "activation_id": "01K...",
    "claim_id": "01K...",
    "lease_generation": 3,
    "disposition": "handled",
    "opaque_run_id": "optional-runner-owned-id",
    "submitted_action_ids": ["01K..."]
  }
}
~~~

disposition is handled, declined, or failed. Room consequences exist only in separately accepted Actions. Marking an Activation handled does not certify task quality or create an Activity Outcome.

The server stores a canonical completion-request hash and result. Repeating the identical completion is safe and returns the original result; changing its body under the same claim is an idempotency conflict.

### activation.release

A runner may relinquish a lease so another authorized runner can claim it. Release MUST include the current claim ID and lease generation. Expired leases return to pending until the activation deadline or retry policy expires.

An operation from an expired or superseded claim returns stale_activation_lease and cannot alter the current lease. A new grant increments lease_generation.

### HTTP long poll

Runner SDKs unable to maintain a control WebSocket may use:

    GET  /v1/runner/activations:poll
    POST /v1/runner/activations/{id}:claim
    POST /v1/runner/activations/{id}:renew
    POST /v1/runner/activations/{id}:complete
    POST /v1/runner/activations/{id}:release

Semantics and idempotency match the WebSocket messages.

## Heartbeats and connection close

server.ping and client.pong detect dead sessions. Heartbeats are ephemeral transport messages and do not consume room or observation sequence.

Closing a room-client socket:

- does not change membership;
- does not create a MembershipChanged Stimulus or Transition by itself;
- does not imply a model invocation state;
- leaves unacknowledged frames available.

An explicit Membership departure is an authorized Room administration operation and MUST be recorded as a MembershipChanged Stimulus and ordered Transition. Closing a Session is not a departure.

## HTTP resource APIs

Exact route names may change before the first schema freeze. Required semantics are:

### Development administration

    POST /v1/principals
    POST /v1/capabilities
    POST /v1/rooms
    POST /v1/rooms/{room_id}/members
    POST /v1/rooms/{room_id}:archive

Public network exposure of these endpoints is not a production-ready control plane. Host-operator tools should bind locally or sit behind separate administrator authentication.

worldstreamctl generates bearer capability secrets locally, sends only the protocol-defined token hash to POST /v1/capabilities, and prints the plaintext locally. The server never stores or must replay a plaintext token to satisfy HTTP idempotency.

Room archival and domain-relevant Membership, Access Mode, or Role changes are routed through the Room and commit as ordered Transitions. Their HTTP mutation receipt commits with the same change. Session presence, Runner availability, and other operational changes do not consume Room sequence. See [ADR 0002](adr/0002-sequence-domain-relevant-room-changes.md).

### Room creation

~~~json
{
  "pack": {
    "id": "worldstream.agent-heist",
    "version": "0.1.0",
    "digest": "blake3:..."
  },
  "configuration": {},
  "members": [
    {
      "principal_id": "01K...",
      "role": "navigator",
      "access_mode": "participant"
    }
  ],
  "idempotency_key": "01K..."
}
~~~

The server accepts only a compiled-in allowlisted pack digest. Creation records an immutable canonical sequence-zero genesis object containing pack digest, configuration, ordered Memberships/Roles, Room seed, and logical creation time. Genesis hash, initial Activity State hash, Memberships, initial Projections, timers, resource, and mutation receipt commit atomically. Recovery remains possible after every snapshot is deleted.

### Current projection

    GET /v1/rooms/{room_id}/projection

The authenticated viewer determines which projection the server returns. Supplying another member ID does not grant its view.

### Replay

    GET /v1/rooms/{room_id}/replay?at_room_seq=91

Replay authorization is separate from live-room authorization. During an active room, a participant cannot use replay to reveal state it was not allowed to observe. A completed pack may expose a final-reveal projection explicitly.

Replay responses name:

- exact pack digest;
- requested and reconstructed room sequence;
- state and transition hashes;
- authorized historical projection;
- verification status.

There is no fork or branch-creation API in v0.1 or v0.2.

## v0.2 artifact protocol

### Upload

    POST /v1/rooms/{room_id}/artifacts

The HTTP request supplies:

- idempotency key;
- declared media type and byte length;
- generic label.

Bytes stream to bounded temporary storage and then to durable content-addressed storage using the filesystem ordering in architecture.md. Successful upload persists and returns an owner-scoped, expiring upload_id plus digest, size, label, and detected/safe media type. The staged upload is visible only to its owner and the host operator. It becomes authoritative room content with pack-defined visibility only when an authorized typed action references upload_id and the transition atomically links it.

Evidence version and supersession are Investigation Activity Pack action/state fields. They are not generic artifact API or database fields.

### Download

    GET /v1/rooms/{room_id}/artifacts/{digest}

Authorization occurs before filesystem lookup. The response verifies the digest and uses safe Content-Type and Content-Disposition headers. Range requests and preview support are optional.

Artifact contents are not sent in observation frames. Frames carry authorized immutable references and relevant metadata.

## Error envelope

~~~json
{
  "protocol": "0.1",
  "type": "error",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "code": "cursor_out_of_range",
    "message": "A current authorized projection is required.",
    "retryable": true,
    "details": {
      "minimum_retained_frame_seq": 150
    }
  }
}
~~~

Core error codes:

| Code | Meaning |
|---|---|
| unauthenticated | Capability absent or invalid |
| forbidden | Capability lacks principal, room, member, or operation scope |
| unsupported_protocol | No compatible wire version |
| invalid_envelope | Malformed or unknown fields |
| message_too_large | Configured input bound exceeded |
| room_not_found | Unknown or hidden room |
| membership_not_found | Unknown or hidden membership |
| membership_not_enabled | Suspended or departed |
| room_faulted | Integrity or deterministic activity failure |
| room_quarantined | Hash/genesis integrity failure; read-only diagnostics only |
| room_archived | Ordinary mutation is disabled |
| room_busy | Bounded actor mailbox full |
| cursor_ahead | Client claims an impossible future frame |
| cursor_out_of_range | Retained delta range unavailable; reset required |
| idempotency_conflict | Same key with different canonical payload |
| activation_not_available | Another runner owns a live lease or intent is terminal |
| lease_expired | Operation used an expired claim |
| stale_activation_lease | Claim ID/generation is no longer current |
| invalid_payload | Input failed strict schema before admission |
| activity_fault | Pack/runtime failed before durable action result |
| rate_limited | Caller exceeded a documented limit |
| storage_unavailable | Mutation cannot be durably committed |
| slow_consumer | Connection closed; reconnect from cursor |

Messages should be helpful, but clients must branch on code rather than English text. For an action submission, retryable transient errors do not consume the action ID; retry the identical canonical action with that same ID.

Some codes can appear in different envelopes because the operation has different idempotency semantics. After `action.submit` has passed authentication, strict parsing, and Membership lookup, stable `membership_not_enabled` and `room_archived` results use `action.rejected` and a durable receipt. The same codes use the generic error envelope for attach/read/administrative operations, or when no receiptable participant Action was admitted. Clients must branch on both envelope type and code.

## Canonical hashing

Hash one canonical typed object, never raw concatenated variable-length fields.

For action idempotency:

~~~json
{
  "domain": "worldstream/action-idempotency/v1",
  "protocol": "0.1",
  "room_id": "01K...",
  "member_id": "01K...",
  "based_on_room_seq": 91,
  "action_type": "commit_move",
  "payload": {}
}
~~~

For transition integrity:

~~~json
{
  "domain": "worldstream/transition/v1",
  "room_id": "01K...",
  "room_seq": 92,
  "pack_digest": "blake3:...",
  "previous_transition_or_genesis_hash": "blake3:...",
  "recorded_stimulus": {},
  "ordered_domain_events": [],
  "resulting_state_hash": "blake3:..."
}
~~~

Genesis uses a third domain, worldstream/genesis/v1, and covers exact pack digest, configuration, ordered initial memberships/roles, seed, and logical creation time.

BLAKE3 hashes canonical JSON bytes for these typed objects. Golden vectors MUST be shared by Rust and Python and define integer, digest, string, and byte representation. Canonical JSON forbids floating point and duplicate keys and sorts object keys.

## Protocol evolution

- Major protocol versions may break envelopes.
- Minor versions add optional fields or new message types.
- Pack action and projection schemas are versioned independently and pinned by pack digest.
- Clients advertise supported protocol versions and capabilities.
- The server never silently rewrites an existing Room to a new Activity Pack revision.
- Unknown required capability yields an explicit failure.
- Both v0.1 and v0.2 remain pre-stable developer-preview protocols.

The public Activity Pack ABI is reviewed only after both reference activities pass.

## Required conformance scenarios

### Ordering and idempotency

1. Two participants submit concurrently.
2. One deterministic room order is committed.
3. A retry of an accepted action returns the same transition.
4. Same action ID with different payload returns idempotency_conflict.
5. A rate-limit or room-busy error consumes no receipt; retrying the identical action with the same ID can later be admitted.
6. A deterministic stale-state rejection is durable; retrying that ID returns the same rejection.

### Disconnect and catch-up

1. A participant receives frame N but disconnects before acknowledging it.
2. It attaches with cursor N minus 1.
3. A transition commits between the through-H query and live switch.
4. The server redelivers N through H, then the buffered later frame, without a gap.
5. Acknowledging N is monotonic and safe.

### Projection reset

1. A client attaches with a pruned cursor.
2. The server explicitly reports resync required.
3. It sends an authorized current projection and baseline frame sequence.
4. Later deltas continue from that baseline.

### Fresh invocation activation

1. An agent invocation submits an action and terminates.
2. A later typed transition emits an attention signal.
3. One durable activation ID is offered at least once.
4. A runner claims it with a claim ID and lease generation.
5. It starts a fresh invocation with authorized projection and catch-up.
6. The invocation submits an ordinary idempotent action.
7. Replaying the room reconstructs the activation decision but contacts no runner.
8. A lost claim reply is retried with the same claim ID and returns the original lease result.
9. After lease expiry/reclaim, the old generation cannot complete the new lease.

### Privacy

1. Two memberships have different private state.
2. Stored frame payloads, catch-up, projection reset, replay, logs, and public UI are tested.
3. No unauthorized field or artifact reference crosses audiences.

### Crash recovery

1. The server commits an action and is terminated before its reply.
2. The retry returns the original accepted result.
3. Pending frames and activation intents remain available.
4. Delete every snapshot; Recovery from immutable genesis plus Transitions reaches the recorded Room head, Core Room State, and Activity State hash.
5. Duplicate a due TimerFired candidate; one conditional timer row and one canonical transition win.

### HTTP mutation idempotency

1. Create a room and lose the HTTP response after commit.
2. Retry the same principal/operation/key/request hash.
3. The server returns the original room ID and does not create another room.
4. Changed request content under that key returns idempotency_conflict.

## Minimal Heist interaction

1. Host operator creates principals, room, memberships, and scoped capabilities.
2. Each runner connects its control channel.
3. Each participant client attaches with cursor zero.
4. Initial private projection frames arrive and are acknowledged.
5. Agents submit typed clue, offer, and plan actions.
6. A timer opens the commitment window.
7. One absent Agent Participant's Membership produces an activation offer for an authorized Runner.
8. The runner claims it and starts a fresh invocation.
9. The new invocation receives authorized current projection and relevant frames.
10. All three submit sealed commitments.
11. The pack resolves one deterministic Outcome.
12. Public and private result frames arrive.
13. The host operator replays the room and verifies its final hash.
