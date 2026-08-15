# Wire Protocol and Data Model

## Status and scope

This document specifies the native v0.1 protocol shape. The observation attach/reset/ACK and Activation operation/context contracts are frozen by [Observation Delivery and Activation](observation-and-activation.md); unrelated message names and fields may still evolve before their conformance fixtures, but no change may weaken the Frozen Requirements.

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

All canonical Room-administration operations use the Operation Identity `(authenticated_principal, versioned_operation_kind, idempotency_key)`. For existing-Room administration, the versioned Canonical Request Hash binds target Room, expected basis, reason, and the complete ordered changeset. Room creation instead binds its exact pack digest, configuration, and ordered initial Membership proposal while excluding generated Room/Member IDs, Room seed, and recorded creation time. The server prepares exactly `Create(PreparedRoomCreationV1)` or `Existing(PreparedRoomCommitV1)` and submits it through `commit(PreparedRoomWriteV1)`; the only other storage-port operation is `resolve(identity, hash)`. Genesis creation, an Advance, stable Rejection, or administrative NoChange and its Semantic Receipt reach one database COMMIT. Same identity/hash returns the original result; same identity/different hash returns `idempotency_conflict`.

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

The response captures one complete barrier and selects exactly one synchronization branch:

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
      "room_id": "01K...",
      "room_seq": 91,
      "genesis_or_transition_hash": "blake3:...",
      "core_schema_version": "worldstream.core-room-state.v1",
      "pack_digest": "blake3:...",
      "core_state_hash": "blake3:...",
      "activity_state_hash": "blake3:...",
      "authoritative_state_hash": "blake3:..."
    },
    "cursor": 184,
    "frame_head": 191,
    "retained_floor": 150,
    "sync_token": "opaque-session-bound-token",
    "sync": {
      "kind": "retained_frames",
      "cursor_exclusive": 184,
      "through_frame_head": 191
    },
    "pack": {
      "id": "worldstream.agent-heist",
      "version": "0.1.0",
      "digest": "blake3:..."
    }
  }
}
~~~

The server then delivers the complete retained range 185 through 191. `role` is present only for participant-access Memberships; spectator and operator Memberships have no pack-defined Role.

Suspended/departed Memberships and quarantined Rooms cannot attach. A faulted Room may attach only to the last verified authorized Projection/retained range and includes `room_health: "faulted"` plus its generation; it never represents unverified bytes as current.

On first attach, visibility loss, or an unavailable range, `sync.kind` is `projection_reset` and carries the reset baseline instead. It never silently starts at the newest frame.

Attachment is serialized through the Room lane. The server captures the complete Room Head, Membership frame head/floor/Cursor, and a single-use Session-bound sync token, then marks the Session CatchingUp. It pages retained frames through the captured head while buffering later frames. Only `room.sync_ack` carrying that token after atomic client installation switches this Session to Live. Buffer overflow closes the socket and requires another resumable attachment.

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
      "room_id": "01K...",
      "room_seq": 91,
      "genesis_or_transition_hash": "blake3:...",
      "core_schema_version": "worldstream.core-room-state.v1",
      "pack_digest": "blake3:...",
      "core_state_hash": "blake3:...",
      "activity_state_hash": "blake3:...",
      "authoritative_state_hash": "blake3:..."
    },
    "room_health": "healthy",
    "integrity_generation": 7,
    "baseline_frame_head": 191,
    "reset_reason": "retained_range_unavailable",
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
          "deadline": "2026-08-13T18:30:00Z"
        }
      },
      "action_offers": [
        {
          "domain": "worldstream/action-offer/v1",
          "action_type": "commit_move",
          "payload_schema_digest": "blake3:...",
          "eligibility_window": {
            "opens_at": "2026-08-13T18:29:30Z",
            "deadline": "2026-08-13T18:30:00Z"
          }
        }
      ]
    },
    "projection_hash": "blake3:..."
  }
}
~~~

The `projection` value contains authorized Core Room State and Membership metadata, the pack-owned Activity Projection, and exactly one sibling `action_offers` list. The surrounding body is the Projection Envelope; `room_health` is operational metadata outside the Projection. A Projection Reset is not an Observation Frame. A client processes it atomically, stores `baseline_frame_head`, and then sends the Session's synchronization acknowledgement.
The canonical `projection.action_offers` bytes are supplied by the exact pack view and reused unchanged by Observations, Invocation Context, and host pre-admission.

`projection_hash` is BLAKE3 over the canonical object `{domain: "worldstream/projection-hash/v1", projection_schema, projection}`. It excludes Projection Envelope fields such as message ID, Room Integrity State/generation, Room sequence, frame sequence, and delivery time, so operational changes do not alter an otherwise identical Projection hash.

### room.sync_ack

~~~json
{
  "protocol": "0.1",
  "type": "room.sync_ack",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "room_id": "01K...",
    "member_id": "01K...",
    "through_frame_head": 191,
    "sync_token": "opaque-session-bound-token"
  }
}
~~~

The token is valid only for the Session and barrier that issued it. This acknowledgement changes only that Session's synchronization state and never advances the durable Membership Cursor. A separate `observation.ack` may advance the shared Cursor, but an acknowledgement from another Session can never satisfy this Session's barrier or make this Session Live.

`through_frame_head` MUST equal the captured frame head/reset baseline bound into the token. A lower, higher, expired, or otherwise mismatched acknowledgement returns a typed synchronization error and neither changes this Session's state nor switches it to Live.

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
      "selected_plan_id": "01K...",
      "contribute_required_resource": true
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
      "room_id": "01K...",
      "room_seq": 92,
      "genesis_or_transition_hash": "blake3:...",
      "core_schema_version": "worldstream.core-room-state.v1",
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
    "action_offers": [
      {
        "domain": "worldstream/action-offer/v1",
        "action_type": "commit_move",
        "payload_schema_digest": "blake3:...",
        "eligibility_window": {
          "opens_at": "2026-08-13T18:29:30Z",
          "deadline": "2026-08-13T18:30:00Z"
        }
      }
    ],
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
| Room administration | `(authenticated_principal, versioned_operation_kind, idempotency_key)` | Original Genesis-created receipt with generated Room/Member IDs and Head zero, or original Transition, Rejection, or NoChange receipt |
| Timer firing | `(room_id, timer_id, generation)` | Original Semantic Receipt and matching Transition; Canonical Request Hash binds immutable `scheduled_for`/payload, while an obsolete `NotApplicable` candidate binds no receipt |
| Host/external input | `(room_id, source_id, input_id)` | Original Semantic Receipt and matching Transition; an independently obsolete `NotApplicable` candidate binds no receipt |

The server resolves a same-identity retry before later Room lifecycle, integrity, or Membership checks, after current authentication and authorization to read that result. Same identity with changed Canonical Request Hash is `idempotency_conflict` and never executes domain work.

If database COMMIT may or may not have occurred, the server MUST keep the attempt `Indeterminate` and query the authoritative primary with the original identity/hash. It MUST NOT resubmit under a new identity, re-run pack logic, scan that timer again, regenerate or reseal creation values, acknowledge, or publish an assumed result. If resolution cannot finish within the request budget, the server returns `commit_indeterminate`; a client retries only the identical identity/body to continue resolution. A stored creation resolution returns its original generated Room/Member IDs and exact Head zero. Only authoritative `KnownAbsent` proves no Create, Advance, or disposition committed and permits the server to retry the identical sealed `PreparedRoomWriteV1` or apply its operation-specific reprepare rule. A generated Room-ID collision is a separately proven-absent `Reprepare` that preserves caller identity/hash while resealing generated values.

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
      "action_offers": [
        {
          "domain": "worldstream/action-offer/v1",
          "action_type": "commit_move",
          "payload_schema_digest": "blake3:...",
          "eligibility_window": {
            "opens_at": "2026-08-13T18:29:30Z",
            "deadline": "2026-08-13T18:30:00Z"
          }
        }
      ]
    },
    "frame_payload_hash": "blake3:..."
  }
}
~~~

For one Membership, one accepted Transition may produce:

- no frame for an unaffected membership;
- exactly one coalesced frame containing every authorized public and private consequence for an affected Membership;
- public consequences coalesced into each eligible Membership's one frame;
- operator-only consequences coalesced only into eligible operator Membership frames;
- no frame when the Transition is hidden, even though the global Room Head advances.

A frame MUST name exactly one recipient Membership and MUST NOT contain hidden Authoritative Room State or another Membership's private payload. `frame_seq` is monotonic and never reused within that Membership's Observation Stream and may skip Room Transitions that were irrelevant or unauthorized. Genesis creates no frame. Public consequences are copied into each authorized enabled Membership's own coalesced frame; operator-only consequences go only to operator Memberships.

`frame_payload_hash` protects the canonical Observation Frame payload bytes only. It is not a Canonical Request Hash, Operation Identity field, or idempotency receipt binding.

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

Pruning never advances the Cursor or permits frame-sequence reuse. Acknowledged frames have a seven-day safety window, subject to a hard ceiling of 10,000 frames or 64 MiB per Membership. When the required range is unavailable, attach returns a Projection Reset rather than pretending catch-up was complete.

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

`claim_id` is both the lease-attempt identity and the claim operation ID for this Activation and authenticated Runner. The server derives its canonical request hash from the complete authenticated request; clients do not submit a trusted hash field. The database atomically grants at most one current lease and persists that hash plus the result. Same claim ID and same request returns that original result after a lost reply while any required private context is retained; after context retirement it follows the deterministic `result_retired` rule below. A changed request or different authenticated Runner is an idempotency conflict. A Runner uses a new claim ID if it wants to try again after a stored not-available result.

Claim is one of four independent idempotent Activation operations; renew, release, and complete each carries its own operation ID, from which the server derives and stores a canonical request hash. After authentication, an existing identical receipt is returned before current availability is considered. Database COMMIT linearizes every newly recorded disposition.

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
    "deadline": "2026-08-13T18:30:00Z",
    "room_head": {
      "room_id": "01K...",
      "room_seq": 93,
      "genesis_or_transition_hash": "blake3:...",
      "core_schema_version": "worldstream.core-room-state.v1",
      "pack_digest": "blake3:...",
      "core_state_hash": "blake3:...",
      "activity_state_hash": "blake3:...",
      "authoritative_state_hash": "blake3:..."
    },
    "integrity_generation": 7,
    "policy_revision": 12,
    "authority_generation": 4,
    "membership_generation": 3,
    "frame_head": 192,
    "retained_floor": 150,
    "cursor": 192,
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
          "phase": "commitment"
        }
      },
      "action_offers": [
        {
          "domain": "worldstream/action-offer/v1",
          "action_type": "commit_move",
          "payload_schema_digest": "blake3:...",
          "eligibility_window": {
            "opens_at": "2026-08-13T18:29:30Z",
            "deadline": "2026-08-13T18:30:00Z"
          }
        }
      ]
    },
    "projection_hash": "blake3:...",
    "runner_budget": {
      "schema": "worldstream/runner-budget/v1",
      "max_action_submissions": 1
    },
    "runner_limits": {
      "schema": "worldstream/runner-limits/v1",
      "max_runtime_ms": 30000,
      "max_result_bytes": 65536
    },
    "delivery": {
      "kind": "retained_frames",
      "cursor_exclusive": 192,
      "through_frame_head": 192,
      "frames": []
    },
    "artifact_references": []
  }
}
~~~

The runner uses this exact committed payload to start a new Invocation or route work to a bounded Runner-owned execution runtime. `projection.action_offers` is the context's sole exact ordered ActionOfferV1 list. `runner_budget` and `runner_limits` are versioned bounded witnesses selected by the applicable policy and configuration for this grant; the illustrative values above are neither implementation evidence nor a release-performance claim. `delivery` is exactly one of `retained_frames` or `projection_reset { baseline_frame_head, reason }`. The complete Head and all displayed witnesses belong to the grant. WorldStream does not know which model is called.

The claim capability authorizes Activation handling only. To submit a domain Action, the Runner or Invocation uses separate participant authority bound to the target Principal and Membership. Receiving context, claiming, renewing, releasing, or completing the Activation does not advance the Membership Cursor; an authorized room client acknowledges Observation Frames explicitly after durable processing. See [ADR 0003](adr/0003-separate-activation-and-action-authority.md).

If the exact private context later reaches its retention limit, the stored original grant code and result/context hashes remain unchanged. An identical claim retry resolves that receipt but deterministically returns `result_retired`; it never regenerates context or rewrites the original result.

### activation.renew

A Runner may extend a lease within the server's maximum:

~~~json
{
  "protocol": "0.1",
  "type": "activation.renew",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "activation_id": "01K...",
    "claim_id": "01K...",
    "renew_operation_id": "01K...",
    "lease_generation": 3,
    "requested_lease_ms": 30000
  }
}
~~~

The server derives the canonical request hash. Renewal conditionally matches the authenticated Runner and current unexpired lease. It is operational and does not change canonical Room sequence.

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
    "complete_operation_id": "01K...",
    "lease_generation": 3,
    "disposition": "handled",
    "opaque_run_id": "optional-runner-owned-id",
    "submitted_action_ids": ["01K..."]
  }
}
~~~

disposition is handled, declined, or failed. Room consequences exist only in separately accepted Actions. Marking an Activation handled does not certify task quality or create an Activity Outcome.

The server derives and stores the complete operation's canonical request hash with the operation ID and result. Repeating the identical completion is safe and returns the original result; changing its body under the same operation ID is an idempotency conflict.

### activation.release

A Runner may relinquish a lease so another authorized Runner can claim it:

~~~json
{
  "protocol": "0.1",
  "type": "activation.release",
  "message_id": "01K...",
  "request_id": "01K...",
  "body": {
    "activation_id": "01K...",
    "claim_id": "01K...",
    "release_operation_id": "01K...",
    "lease_generation": 3
  }
}
~~~

The server derives the canonical request hash. Expired leases return to pending until the Activation deadline or policy expiry.

An operation from an expired or superseded claim returns stale_activation_lease and cannot alter the current lease. A new grant increments lease_generation.

Archive cancels every pending/leased intent and advances a Room-wide Activation fence in the same Room transaction. Suspend, departure, identity, Access Mode, and Role changes cancel/fence affected Memberships. Capability revocation is immediate. Runtime recovery state Loading, CatchingUp, Passivating, or Inactive, or Room Integrity State faulted or quarantined, makes pending intents unclaimable without deleting them. A backward clock anomaly fences live leases and returns still-eligible intents to pending under a new generation.

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

Room archive and Membership Standing, Access Mode, or Role changes are normalized as versioned Core Stimuli with canonical authority attribution, idempotency identity, exact expected Room sequence, stable reason code, and Core before/after values. A single request may carry a Member-ID-sorted atomic final-state changeset with at most one typed component per Membership. Component kinds are Join, Resume, AccessModeChange, RoleChange, Suspend, or Depart. Archive remains its own CoreProposed kind and cannot occur inside MembershipChangeSet. The server validates the complete state and Role cardinality without returning or persisting an intermediate assignment.

Join, Resume, AccessModeChange, and RoleChange components are vetoable; Suspend and Depart components are mandatory. A mixed-class MembershipChangeSet is rejected before pack entry with no pack call, Transition, or receipt. An all-vetoable set may receive one stable pack-declared administrative rejection for the whole atomic proposal with an idempotent receipt and no Transition. An all-mandatory set must Apply as a whole and a pack Reject is a fault. Archive is independently mandatory and cannot be vetoed. A pre-existing desired state may return durable NoChange. Every other accepted Core change commits its receipt and one ordered Transition. Archive is irreversible and atomically cancels timers and fences Activation work. See [ADR 0002](adr/0002-sequence-domain-relevant-room-changes.md) and [ADR 0005](adr/0005-canonical-core-state-integrity-and-hash-lineage.md).

Session presence, Runner availability, and other operational changes do not consume Room sequence.

### Room creation

~~~json
{
  "pack": {
    "id": "worldstream.agent-heist",
    "version": "0.1.0",
    "digest": "blake3:..."
  },
  "configuration": {
    "pack_schema": 1,
    "roles": ["navigator", "insider", "broker"],
    "briefing_duration_seconds": 30,
    "negotiation_duration_seconds": 90,
    "commitment_duration_seconds": 30,
    "commitment_reminder_seconds_before_deadline": 10,
    "result_duration_seconds": 20,
    "maximum_plans": 12,
    "maximum_open_offers_per_role": 4
  },
  "members": [
    {
      "principal_id": "01K...NAV",
      "role": "navigator",
      "access_mode": "participant"
    },
    {
      "principal_id": "01K...INS",
      "role": "insider",
      "access_mode": "participant"
    },
    {
      "principal_id": "01K...BRO",
      "role": "broker",
      "access_mode": "participant"
    }
  ],
  "idempotency_key": "01K..."
}
~~~

The server accepts only a selectable compiled-in semantic digest whose `PackRevisionLockV1`, executor, schemas, codecs, and goldens agree in the embedded registry. Before opening storage locks, it generates Room/Member IDs, Room seed, and logical creation time, constructs initial `CoreRoomState v1`, calls `initialize`, computes all three initial hashes and Head zero, and seals them with the administration identity/hash, creation-authority witness, selected exact revision, Genesis, materializations, timers, and `genesis_created` receipt in `PreparedRoomCreationV1`. The Create transaction resolves/fences that identity, rechecks creation authority and generated Room-ID absence, then atomically installs the Room root, immutable sequence-zero Genesis, initial Core/Activity/Membership/timer materializations, hashes/Head zero, and receipt. It has no basis Head, Room Integrity fence, or existing-Room lane. Genesis creates no Transition, Domain Event, Observation Frame, Attention Signal, or Activation. Recovery remains possible after every paired snapshot and current materialization is deleted.

### Current projection

    GET /v1/rooms/{room_id}/projection

The authenticated viewer determines which projection the server returns. Supplying another member ID does not grant its view.

A faulted Room returns only its last verified authorized Projection with `room_health: "faulted"` and the matching integrity generation. A quarantined Room returns `room_quarantined` and no normal Projection bytes.

### Replay

    GET /v1/rooms/{room_id}/replay?at_room_seq=91

Replay applies two authorization gates. Present authentication/authorization first admits the request. The reconstructed historical Membership at sequence N—its existence, Standing, Access Mode, and Role—then selects the viewer passed to the exact pack. A Membership absent at N receives no participant/private view, and a later Role or replacement Membership never inherits earlier private data. Spectator/operator history and completed final reveal require explicit presently authorized projection policy and do not bypass pack privacy.

Replay responses name:

- exact pack digest;
- exact retained executor/codec support status;
- requested and reconstructed room sequence;
- complete Room Head with Core, Activity, aggregate, and lineage hashes;
- authorized historical projection;
- verification status.

Replay runs the same versioned Core reducer and exact retained pack reducer, creates no Frames or Activation work, and performs no external effect. Faulted Rooms may return only verified history with an explicit integrity envelope. Quarantined Rooms return no normal or claimed-current Replay; host-operator verification/export use separate diagnostic surfaces.

There is no fork or branch-creation API in v0.1 or v0.2.

Replay emits no live Observation Frame, Projection Reset, synchronization token, Activation Intent, offer, claim, lease, Invocation Context, or external effect.

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
| room_loading | Verified Room state is not yet available for normal service |
| room_catching_up | Recovery is draining ordered overdue work before normal service |
| room_busy | Bounded Room Admission Lane unavailable/full; no Semantic Time or deadline entitlement |
| cursor_ahead | Client claims an impossible future frame |
| cursor_out_of_range | Retained delta range unavailable; reset required |
| sync_barrier_mismatch | Session token or acknowledged baseline does not match the captured attach barrier |
| idempotency_conflict | Same key with different canonical payload |
| commit_indeterminate | Database COMMIT may or may not have occurred; only identical-operation resolution is permitted |
| activation_not_available | Another runner owns a live lease or intent is terminal |
| activation_fenced | Authority, policy, integrity, Membership, Room, or lease witness changed |
| lease_expired | Operation used an expired claim |
| stale_activation_lease | Claim ID/generation is no longer current |
| result_retired | Deterministic retry response after an original granted receipt's exact private Invocation Context was replaced by its versioned tombstone; the original result code/hash remains unchanged |
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
- The server never changes an existing Room's digest or rewrites its Activity State in place.
- Pack revisions may be selectable-and-runnable or retained-runnable. Every retained Room digest remains runnable; a newer executor never substitutes for it.
- Unknown required capability yields an explicit failure.
- Both v0.1 and v0.2 remain pre-stable developer-preview protocols.

The trusted five-operation `ActivityPackV1` semantic seam is frozen. Any portable, dynamically loaded, or untrusted public plugin ABI is a separate post-v0.2 decision.

See [ADR 0010](adr/0010-activity-pack-v1-and-executable-replay-retention.md).

## Required conformance scenarios

### Activity Pack revision and Action Offers

1. One exact Action Offer byte sequence appears in Projection, reset, observation, Invocation Context, and host pre-admission.
2. An absent action type cannot reach reduce; payload-specific declared rejection remains possible.
3. Join/resume/Access/Role may be pack-rejected; archive/suspend/depart cannot.
4. TimerFired exposes only its exact timer identity, scheduled_for, and payload; packs never assign timer generations.
5. A retained non-selectable digest still initializes, advances, views, observes, and Replays through its exact executor/codecs.
6. A missing executor/codec, digest collision, bound violation, callback panic, or malformed output fails closed with no partial commit.

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
2. Atomically swap two Membership Roles as one all-vetoable MembershipChangeSet whose sequential intermediate would violate pack cardinality; one whole-set Apply commits one Core Transition and no intermediate Projection or hash exists, while one whole-set declared Reject records only the stable rejection.
3. Exercise enabled ↔ suspended, terminal depart/new-ID rejoin, participant Role requirement, and one-non-departed-Membership-per-Principal constraints.
4. Verify individual and homogeneous all-vetoable join/resume/Access/Role proposals may produce stable no-Transition administrative results, while individual and homogeneous all-mandatory suspend/depart proposals and Archive cannot be declared vetoed; a mandatory Reject is PackFault.
5. Submit a mixed vetoable-plus-mandatory MembershipChangeSet and prove pre-pack rejection with no pack call, Transition, or receipt; submit Archive inside MembershipChangeSet and prove the same pre-pack failure.
6. Race an accepted mutation with an integrity-generation change; one wins and the loser consumes no sequence or receipt.
7. A faulted Room serves only last-verified authorized data with an integrity envelope; a quarantined Room serves only the host-operator diagnostic surfaces.
8. Rebuild materializations and snapshots through verifier repair, then prove Genesis/Transition bytes and hashes did not change.

### Disconnect and catch-up

1. Genesis creates no frame; a hidden Transition advances Room Head but emits zero frames; a visible Transition emits one coalesced frame for the Membership.
2. A participant receives frame N but disconnects before acknowledging it and attaches with Cursor N minus 1.
3. The server captures complete Room/frame barrier H and a Session sync token; another Transition commits during catch-up.
4. The server redelivers N through H, but the Session does not become Live until its own `room.sync_ack`; then it receives the buffered later frame without a gap, and that sync ACK does not advance Cursor.
5. Another Session's `observation.ack` may advance the shared Cursor but cannot satisfy this token.

### Projection reset

1. A first attach and a client with a pruned Cursor each receive a full authorized Projection Reset at a captured Room/frame baseline.
2. Pruning changed retained floor only: it did not advance Cursor or reuse a frame sequence.
3. Visibility removal uses a complete Reset or closes the Session, leaving no unauthorized installed data.
4. Only matching `room.sync_ack` enters Live and it does not advance Cursor; later frames continue after the baseline.

### Stable stale Action and Room service surfaces

1. Action ID A is durably rejected as stale against complete basis Head H. Retrying A with the identical hash after the Room advances returns that exact stored rejection.
2. The client completes retained catch-up or Projection Reset, recomputes Action Offers, and—if the intent is still legal—submits new Action ID B. The server never rebases A.
3. A Loading or Room-CatchingUp fixture rejects normal attach/current Projection/reset and participant mutation with the corresponding typed busy surface.
4. A Faulted fixture returns only its last verified authorized Projection/retained frames/Replay plus integrity metadata and cannot advance the Head.
5. A Quarantined fixture rejects normal Projection/catch-up/current Replay and exposes only host-operator diagnostics, raw export, restore, and verification.

### Visibility loss

1. Viewer A has private field X installed at frame N.
2. Transition N+1 removes A's authority for X.
3. A receives a full Projection Reset at the last permissible baseline or its Session closes; no partial delta leaves X installed.
4. No later frame addressed to A contains X, while another viewer's private payload never enters A's stream.

### Fresh invocation activation

1. An agent invocation submits an action and terminates.
2. A later typed transition emits an attention signal.
3. One durable activation ID is offered at least once.
4. A Runner claims it with an operation ID and lease generation while a second Runner proves the one-live-lease-per-Membership rule.
5. It starts a fresh Invocation with the exact committed Projection/Head/witnesses and exactly one retained-frames-or-reset branch.
6. The invocation submits an ordinary idempotent action.
7. Replay verifies Attention and recorded policy-decision evidence but creates/offers/claims nothing and contacts no Runner.
8. While a granted claim's context is retained, a lost reply retried with the same ID/hash returns the exact original context/result bytes; changed hash conflicts.
9. After that granted context is tombstoned, the identical retry returns deterministic `result_retired` while the original result code/hash and context hash remain unchanged and no context is regenerated.
10. After lease expiry/reclaim, the old generation cannot renew, release, or complete the new lease.
11. Archive and affected Membership changes cancel/fence pending and leased intents; authority revocation and a backward-clock anomaly fence stale claims.

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
3. The server resolves `GenesisCreated { Existing }`, returns the original generated Room and Member IDs and exact Head zero, and does not create another room.
4. Prove the creation row had no basis Head, the transaction used no existing-Room lane or integrity/Head fence, and Genesis, initial state/timers/materializations/hashes/Head zero, and the `genesis_created` receipt were all-or-none across every failpoint.
5. Changed request content under that key returns idempotency_conflict.

## Minimal Heist interaction

1. Host operator creates principals, room, memberships, and scoped capabilities.
2. Each runner connects its control channel.
3. Genesis enters Briefing, schedules its deadline, and creates no Frame or Activation.
4. Each participant attaches through an authorized current Projection Reset and acknowledges the matching Session sync token and baseline.
5. Agents inspect/publish clues, exchange bounded offers, and propose/endorse/challenge structured plans.
6. Exact phase timers enter Negotiation and then Commitment.
7. Commitment opening emits Attention for the deliberately absent Broker; host policy creates one eligible Activation.
8. A Runner claims it and starts a fresh Invocation with exact Action Offers but no participant credential or Cursor side effect.
9. Separately authorized participant clients submit sealed {selected_plan_id, contribute_required_resource} commitments.
10. The third commitment closes early, fences both old Commitment timers, and schedules strictly later Resolution.
11. Resolution selects only a two-of-three plan, runs the five named checks, and enters Result.
12. Result reveals aggregates while individual commitments remain sealed; acknowledgements or the deadline enter Complete.
13. A separately authorized post-Complete final-reveal view may expose the frozen private fields.
14. Replay folds Genesis and typed Stimuli with the exact retained executor, verifies the golden checkpoints, and creates no Activation.
