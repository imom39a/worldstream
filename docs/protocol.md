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
5. The server reaches durable database COMMIT before replying to or streaming any newly accepted Action or stable disposition.
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
| Room operation result | Yes when accepted/stably disposed | Semantic Receipt store |
| Observation cursor | Yes | Membership inbox |
| Activation intent | Yes | Activation queue |

Membership Standing is enabled, suspended, or departed. Enabled and suspended are reversible; departed is terminal, and rejoining creates a new Member ID with no inherited Cursor or private frame stream. Suspended/departed Memberships cannot attach, act, receive new frames, or be activated.

Session lifecycle is connected or disconnected.

Activation lifecycle is pending, leased, completed, expired, or cancelled.

These are independent dimensions.

A Room is described across four independent axes:

- Core Room Status: active or irreversibly archived;
- operational Room Integrity State, surfaced as `room_health`: healthy, faulted, or quarantined;
- Activity Phase: a pack-defined stage such as Commitment, Complete, Review, or Closed;
- Outcome: a separate pack-defined result such as success, partial failure, or a deterministic score.

`CoreRoomState v1` contains exactly Room Status plus the semantic Membership map. Room Head, hashes, integrity, Sessions, delivery, receipts, Activation, policy, diagnostics, telemetry, and commit time are envelope or operational records, not Core fields.

A healthy archived Room permits authorized reads/export/Replay and ordered suspend/depart only. A faulted Room rejects every canonical mutation but may serve only its last verified authorized Projection, retained Frame Catch-up, and verified Replay with an explicit integrity envelope. A quarantined Room serves no normal Projection, Catch-up, or claimed-current Replay; only authenticated host-operator diagnostics, raw export, restore, and verification remain. A terminal pack phase normally rejects domain Actions while the Core Room may remain active until the host operator archives it.

Whenever a protocol result names the Room Head, the complete value is represented: Room sequence, Genesis-or-Transition lineage hash, Core schema version, exact pack digest, and Core, Activity, and aggregate Authoritative State hashes. A sequence-only field is a causal convenience, not a Room Head.

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

All canonical Room-administration operations use the Operation Identity `(authenticated_principal, versioned_operation_kind, idempotency_key)`. Their versioned Canonical Request Hash binds target Room, expected basis, reason, and the complete ordered changeset. An Advance, stable Rejection, or administrative NoChange and its Semantic Receipt reach one database COMMIT. Same identity/hash returns the original result; same identity/different hash returns `idempotency_conflict`.

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
    "integrity_generation": 7,
    "room_head": {
      "room_seq": 91,
      "lineage_hash": "blake3:...",
      "core_schema": "worldstream.core-room-state.v1",
      "pack_digest": "blake3:...",
      "core_state_hash": "blake3:...",
      "activity_state_hash": "blake3:...",
      "authoritative_state_hash": "blake3:..."
    },
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

Suspended/departed Memberships and quarantined Rooms cannot attach. A faulted Room may attach only to the last verified authorized Projection/retained range and includes `room_health: "faulted"` plus its generation; it never represents unverified bytes as current.

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
    "room_head": {
      "room_seq": 91,
      "lineage_hash": "blake3:...",
      "core_schema": "worldstream.core-room-state.v1",
      "pack_digest": "blake3:...",
      "core_state_hash": "blake3:...",
      "activity_state_hash": "blake3:...",
      "authoritative_state_hash": "blake3:..."
    },
    "room_health": "healthy",
    "integrity_generation": 7,
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

`projection_hash` is BLAKE3 over the canonical object `{domain: "worldstream/projection-hash/v1", projection_schema, projection}`. It excludes Projection Envelope fields such as message ID, Room Integrity State/generation, Room sequence, frame sequence, and delivery time, so operational changes do not alter an otherwise identical Projection hash.

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
- after strict parsing/hash construction and current authentication/permission to read the result, the server resolves an existing same-identity receipt or Conflict before later Room lifecycle, integrity, Membership, rate, or lane checks;
- only for a new identity, the server applies schema, rate, and size admission and then attempts a bounded Room Admission Lane reservation;
- the host samples `admitted_at` atomically with successful lane insertion; a full/unavailable lane returns `room_busy` with no Semantic Time, receipt, or deadline entitlement;
- the Room Kernel rejects the action unless based_on_room_seq exactly equals the current room head in v0.1 and v0.2;
- the Action window is half-open, `open_at <= admitted_at < deadline`, so equality is late even if a closing timer is delayed;
- `admitted_at` is excluded from the caller-semantic request hash but preserved in the prepared Stimulus and any Semantic Receipt;
- an accepted Action or stable rejection is not acknowledged until its complete Room transaction reaches database COMMIT.

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
    "transition_id": "01K...",
    "admitted_at": "2026-08-13T18:29:59.999Z",
    "room_head": {
      "room_seq": 92,
      "lineage_hash": "blake3:...",
      "core_schema": "worldstream.core-room-state.v1",
      "pack_digest": "blake3:...",
      "core_state_hash": "blake3:...",
      "activity_state_hash": "blake3:...",
      "authoritative_state_hash": "blake3:..."
    },
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
    "admitted_at": "2026-08-13T18:30:00Z",
    "code": "stale_room_state",
    "message": "The commitment window is now active.",
    "current_room_seq": 93,
    "legal_actions": ["commit_move"],
    "retryable_with_same_action_id": false,
    "may_submit_revised_action": true,
    "duplicate": false,
    "details": {}
  }
}
~~~

A deterministic admitted rejection consumes no canonical Room sequence but does consume the Action Operation Identity through a Semantic Receipt. The same identity/hash returns the same rejection, original `admitted_at`, and semantic fields with `duplicate: true`. A revised decision uses a new Action ID.

Durable action.rejected codes include:

- membership_not_enabled;
- action_not_allowed;
- stale_room_state;
- deadline_passed;
- room_archived;
- activity_terminal;
- activity_domain_rejection.

Malformed/invalid payload, unauthenticated, forbidden, `idempotency_conflict`, `room_busy`, `rate_limited`, `storage_unavailable`, and activity/runtime faults use the generic error envelope. They do not create an Action receipt or consume a previously unseen Action ID. A known-absent transient retry MUST reuse the same Action ID and identical canonical Action body. An indeterminate persistence attempt is resolution-only until the server proves stored versus absent.

### Durable operation resolution

The same internal contract covers all Room operation classes:

| Class | Operation Identity | Same-hash replay |
|---|---|---|
| Participant Action | `(room_id, member_id, action_id)` | Original accepted or stable rejected Semantic Receipt |
| Room administration | `(authenticated_principal, versioned_operation_kind, idempotency_key)` | Original Transition, Rejection, or NoChange receipt |
| Timer firing | `(room_id, timer_id, generation)` | Original matching Transition; request hash binds immutable `scheduled_for`/payload and an obsolete generation is `NotApplicable` |
| Host/external input | `(room_id, source_id, input_id)` | Original accepted or stable rejected result where defined |

The server resolves a same-identity retry before later Room lifecycle, integrity, or Membership checks, after current authentication and authorization to read that result. Same identity with changed Canonical Request Hash is `idempotency_conflict` and never executes domain work.

If database COMMIT may or may not have occurred, the server MUST keep the attempt `Indeterminate` and query the authoritative primary with the original identity/hash. It MUST NOT resubmit under a new identity, re-run pack logic, scan that timer again, acknowledge, or publish an assumed result. If resolution cannot finish within the request budget, the server returns `commit_indeterminate`; a client retries only the identical identity/body to continue resolution. Only authoritative `KnownAbsent` permits the server to retry the identical sealed plan or apply the operation-specific reprepare rule.

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
    "room_head": {
      "room_seq": 93,
      "lineage_hash": "blake3:...",
      "core_schema": "worldstream.core-room-state.v1",
      "pack_digest": "blake3:...",
      "core_state_hash": "blake3:...",
      "activity_state_hash": "blake3:...",
      "authoritative_state_hash": "blake3:..."
    },
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

Room archive and Membership Standing, Access Mode, or Role changes are normalized as versioned Core Stimuli with canonical authority attribution, idempotency identity, exact expected Room sequence, stable reason code, and Core before/after values. A single request may carry a Member-ID-sorted atomic final-state changeset with at most one pair per Membership. The server validates the complete state and Role cardinality without returning or persisting an intermediate assignment.

Join, resume, Access Mode, and Role proposals may receive a stable pack-declared administrative rejection with an idempotent receipt and no Transition. Archive, suspend, and depart are mandatory and cannot be vetoed; a pack attempt is a fault. A pre-existing desired state may return durable NoChange. Every other accepted Core change commits its receipt and one ordered Transition. Archive is irreversible and atomically cancels timers and fences Activation work. See [ADR 0002](adr/0002-sequence-domain-relevant-room-changes.md) and [ADR 0005](adr/0005-canonical-core-state-integrity-and-hash-lineage.md).

Session presence, Runner availability, and other operational changes do not consume Room sequence.

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

The server accepts only a compiled-in allowlisted pack digest. Creation constructs initial `CoreRoomState v1`, initializes canonical Activity State and normalized initial timers with the exact pack, and records one immutable sequence-zero Genesis containing the exact version identities, configuration, both initial state values, timer list, Room seed, and logical creation time. Genesis hash plus initial Core, Activity, and aggregate hashes, complete Head zero, verified current materializations, timers, resource, and mutation receipt commit atomically. Genesis creates no Transition, Observation Frame, Attention Signal, or Activation. Recovery remains possible after every paired snapshot and current materialization is deleted.

### Current projection

    GET /v1/rooms/{room_id}/projection

The authenticated viewer determines which projection the server returns. Supplying another member ID does not grant its view.

A faulted Room returns only its last verified authorized Projection with `room_health: "faulted"` and the matching integrity generation. A quarantined Room returns `room_quarantined` and no normal Projection bytes.

### Replay

    GET /v1/rooms/{room_id}/replay?at_room_seq=91

Replay applies two authorization gates. Present authentication/authorization first admits the request. The reconstructed historical Membership at sequence N—its existence, Standing, Access Mode, and Role—then selects the viewer passed to the exact pack. A Membership absent at N receives no participant/private view, and a later Role or replacement Membership never inherits earlier private data. Spectator/operator history and completed final reveal require explicit presently authorized projection policy and do not bypass pack privacy.

Replay responses name:

- exact pack digest;
- requested and reconstructed room sequence;
- complete Room Head with Core, Activity, aggregate, and lineage hashes;
- authorized historical projection;
- verification status.

Replay runs the same versioned Core reducer and exact retained pack reducer, creates no Frames or Activation work, and performs no external effect. Faulted Rooms may return only verified history with an explicit integrity envelope. Quarantined Rooms return no normal or claimed-current Replay; host-operator verification/export use separate diagnostic surfaces.

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
| room_faulted | Last canonical Head verifies, but canonical mutation is fenced; only last-verified authorized serving is available |
| room_quarantined | Canonical integrity cannot be established; no normal Projection, Catch-up, or Replay |
| integrity_generation_changed | Prepared mutation/repair lost the operational integrity fence; no sequence or receipt was consumed |
| room_archived | Ordinary mutation is disabled |
| room_busy | Bounded Room Admission Lane unavailable/full; no Semantic Time or deadline entitlement |
| cursor_ahead | Client claims an impossible future frame |
| cursor_out_of_range | Retained delta range unavailable; reset required |
| idempotency_conflict | Same key with different canonical payload |
| commit_indeterminate | Database COMMIT may or may not have occurred; only identical-operation resolution is permitted |
| activation_not_available | Another runner owns a live lease or intent is terminal |
| lease_expired | Operation used an expired claim |
| stale_activation_lease | Claim ID/generation is no longer current |
| invalid_payload | Input failed strict schema before admission |
| activity_fault | Pack/runtime failed before durable action result |
| rate_limited | Caller exceeded a documented limit |
| storage_unavailable | Persistence is unavailable before handoff or the write is proven absent; no semantic receipt was created |
| slow_consumer | Connection closed; reconnect from cursor |

Messages should be helpful, but clients must branch on code rather than English text. For an Action submission, known-absent transient errors do not consume the Action ID; retry the identical canonical Action with that same ID. `commit_indeterminate` is not permission to submit the domain operation again: the identical request continues identity resolution until the server returns the stored result or proves absence.

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

`admitted_at`, generated Transition ID, commit time, message/request IDs, and retry-attempt metadata are deliberately absent from this caller-semantic object.

The three state hashes are separate typed objects:

~~~json
{"domain":"worldstream/core-state/v1","core_schema":"worldstream.core-room-state.v1","core":{}}
~~~

~~~json
{"domain":"worldstream/activity-state/v1","pack_digest":"blake3:...","activity":{}}
~~~

~~~json
{
  "domain": "worldstream/authoritative-state/v1",
  "core_schema": "worldstream.core-room-state.v1",
  "pack_digest": "blake3:...",
  "core_state_hash": "blake3:...",
  "activity_state_hash": "blake3:..."
}
~~~

The aggregate Authoritative State hash binds but never replaces both component hashes.

For Transition integrity, the following semantic fields are exact even if pre-freeze wire field names change:

~~~json
{
  "domain": "worldstream/transition/v1",
  "room_id": "01K...",
  "room_seq": 92,
  "core_schema": "worldstream.core-room-state.v1",
  "pack_digest": "blake3:...",
  "previous_transition_or_genesis_hash": "blake3:...",
  "recorded_stimulus": {},
  "ordered_domain_events": [],
  "ordered_timer_changes": [],
  "ordered_attention_signals": [],
  "resulting_core_state_hash": "blake3:...",
  "resulting_activity_state_hash": "blake3:...",
  "resulting_authoritative_state_hash": "blake3:..."
}
~~~

The Transition version/hash/codec identities are carried by the typed domain and versioned envelope. Canonical authority attribution, idempotency identity, exact expected sequence, reason, and semantic input/time inside `recorded_stimulus` are included. Integrity state/generation/incidents, bearer/commit witnesses, receipts, materializations, snapshots, Projections/Frames/Cursors/resets/Sessions, policy and Activation records, diagnostics, telemetry, and commit wall time are excluded.

Genesis uses `worldstream/genesis/v1` and binds Room/version identities, Core schema, exact pack digest, canonical configuration, normalized initial timers, Room seed, logical creation time, and initial Core, Activity, and aggregate hashes. The immutable Genesis record also stores both initial canonical state values so it is reconstructible without a snapshot.

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
7. A lost COMMIT reply resolves through the same identity/hash; no pack call or second mutation occurs.
8. An injected ambiguous COMMIT that cannot yet be resolved returns commit_indeterminate, never a generic retry permission.

### Core, hashes, and integrity

1. Replay a Genesis-only Room and verify complete Head zero plus Core, Activity, aggregate, and Genesis hashes.
2. Atomically swap two Membership Roles whose sequential intermediate would violate pack cardinality; one Core Transition commits and no intermediate Projection or hash exists.
3. Exercise enabled ↔ suspended, terminal depart/new-ID rejoin, participant Role requirement, and one-non-departed-Membership-per-Principal constraints.
4. Verify join/resume/Access/Role vetoes are stable no-Transition administrative results while archive/suspend/depart cannot be declared vetoed.
5. Race an accepted mutation with an integrity-generation change; one wins and the loser consumes no sequence or receipt.
6. A faulted Room serves only last-verified authorized data with an integrity envelope; a quarantined Room serves only the host-operator diagnostic surfaces.
7. Rebuild materializations and snapshots through verifier repair, then prove Genesis/Transition bytes and hashes did not change.

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
7. Replaying the Room reproduces the deterministic Attention Signal but does not reevaluate policy, create Activation work, or contact a Runner.
8. A lost claim reply is retried with the same claim ID and returns the original lease result.
9. After lease expiry/reclaim, the old generation cannot complete the new lease.

### Privacy

1. Two memberships have different private state.
2. Stored frame payloads, catch-up, projection reset, replay, logs, and public UI are tested.
3. No unauthorized field or artifact reference crosses audiences.
4. Present authority admits Replay, while the reconstructed Membership/Access/Role at sequence N determines historical private content; later Roles and replacement Member IDs inherit none.

### Crash recovery

1. The server commits an action and is terminated before its reply.
2. The retry returns the original accepted result.
3. Pending frames and activation intents remain available.
4. Delete every paired snapshot and current materialization; Recovery from immutable Genesis plus Transitions reaches the complete Room Head and all three state hashes.
5. Duplicate a due TimerFired candidate; one conditional timer row and one canonical transition win.
6. Resolve unknown TimerFired COMMIT before scanning or preparing that generation again.

### Semantic time and timer races

1. Admit Actions at `D-1`, exactly `D`, and `D+1`; equality and later are durable `deadline_passed` even when the closing timer is delayed.
2. Permit a `D-1` Action to commit after D only when its complete witnesses still pass and no closing timer/archive committed first.
3. Fill participant capacity while reserved host-stimulus capacity still admits the due timer; prove neither earlier reservations nor a reserved timer can be overtaken.
4. Crash before persistence handoff and prove the lane position/sample disappear; retry samples anew. Prove a known-absent retry after handoff preserves the sealed sample.
5. Exercise ScheduleNext, CancelCurrent, and RescheduleCurrent plus every invalid generation, duplicate, non-forward, overflow, and missing-current case.
6. Race Action/timer, archive/timer, cancel/timer, authority/timer, and integrity/timer on SQLite and PostgreSQL and require the same semantic class.
7. Restart with equal-time and overdue generations; drain a fixed cutoff in `(scheduled_for, timer_id, generation)` order, rereading after each, without ordinary same-Room interleaving.
8. Replay the transcript byte-identically with no HostClock or scheduler.

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
