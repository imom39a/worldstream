# Observation Delivery and Activation

## Status

This document freezes the v0.1 observation-delivery and Activation contracts. It is normative for the Room-to-client synchronization boundary and the Runner-control boundary. Canonical Room ordering remains defined by the requirements and architecture documents.

## Ownership boundaries

WorldStream keeps four concerns separate:

| Concern | Durable | Canonical Room state | Advances `room_seq` |
|---|---:|---:|---:|
| Projection and Observation Frame materialization caused by a Transition | Yes | No; derived and addressed | The causing Transition does |
| Membership Cursor and retained-frame floor | Yes | No | No |
| Activation Intent and operation receipts | Yes | No; Attention Signal is canonical evidence | No, except creation is installed beside its causing Transition |
| Session, Runner connection, claim lease, and Invocation | Operational; selected facts are durable | No | No |

Runner control, participant Action authority, and Session synchronization are independent capabilities. Claiming or completing an Activation never grants `room:act`, advances a Cursor, satisfies an attach barrier, or authorizes Replay/export/operator access.

## Observation contract

### Per-Membership stream

Each Membership owns one durable Observation Stream with three independent positions:

- `frame_head`: the greatest frame sequence ever allocated for that Membership; it never decreases or reuses a value;
- `retained_floor`: the smallest frame sequence still physically retained, or `frame_head + 1` when none remain;
- `cursor`: the greatest frame sequence the Membership has durably acknowledged processing.

The Cursor is shared by Sessions attached to the same Membership. Separate consumers that need independent progress require separate Memberships. Pruning never advances the Cursor and never permits sequence reuse.

Genesis creates no Observation Frame. For each later accepted Transition and each authorized enabled Membership, materialization emits **zero or one** coalesced frame. A hidden Transition can advance the Room Head while emitting zero frames to a viewer. A frame names one Membership and one `cause_room_seq`; it can combine every public and private consequence authorized for that viewer but cannot contain another Membership's data.

When a Transition removes a Membership's ability to see data, the server sends a complete authorized Projection Reset at the last permissible boundary or closes the Session. It never expresses removal by an incomplete delta that leaves hidden state installed.

### Retention and pruning

Acknowledged frames remain eligible for catch-up for seven days. A hard per-Membership ceiling of 10,000 frames or 64 MiB may prune older frames sooner. A hard prune can force a Projection Reset on reconnect; it does not change canonical history, the Membership Cursor, or the frame head.

Unacknowledged frames are retained unless the hard ceiling requires pruning. Crossing that ceiling is explicit operational data loss for incremental delivery and therefore forces a reset; it is never presented as complete catch-up.

### Room and Session state machines

Room recovery and Session delivery are different state machines:

~~~mermaid
stateDiagram-v2
    [*] --> Loading
    Loading --> CatchingUp: "verified base loaded"
    CatchingUp --> Active: "ordered overdue work drained"
    Active --> Passivating: "idle barrier begins"
    Passivating --> Inactive: "mailbox drained"
~~~

Loading, CatchingUp, Active, Passivating, and Inactive are the complete runtime recovery lifecycle. Durable Room Integrity State is the separate `healthy | faulted | quarantined` axis. A verification or runtime failure updates and fences integrity and retires the actor; Faulted and Quarantined are serving surfaces derived from integrity, not recovery states.

~~~mermaid
stateDiagram-v2
    [*] --> Attaching
    Attaching --> CatchingUp: "barrier captured"
    CatchingUp --> Live: "this Session's sync token ACKed"
    Attaching --> Closed
    CatchingUp --> Closed
    Live --> Closed
~~~

Loading and Room-CatchingUp deny normal attach, current Projection reads, reset, and participant mutation. Faulted serves only the last verified Projection, retained delivery, and authorized Replay with integrity metadata; it cannot advance the Head. Quarantined serves no normal Projection, catch-up, or current Replay. Only a host operator may use diagnostics, raw export, restore, and verification.

### Gap-free attach barrier

An attach is admitted only for a currently authorized, attachable Membership in an Active Room:

1. The Room lane registers the Session as `Attaching` and atomically captures the complete Room Head, `frame_head`, `retained_floor`, Cursor, and a new Session-specific opaque sync token.
2. The Session becomes `CatchingUp`. Storage returns exactly one of:
   - `RetainedFrames { cursor_exclusive, through_frame_head, frames }`, containing the complete retained range through the captured frame head; or
   - `ProjectionReset { baseline_frame_head, room_head, projection, projection_hash, reason }` on first attach, when the retained range is unavailable or below its floor, or when visibility loss or another condition makes incremental delivery inappropriate.
3. Frames committed above the captured frame head wait in that Session's bounded queue.
4. The client installs the returned range or reset atomically, then sends `room.sync_ack` for the captured baseline together with that Session's sync token.
5. Only the matching token ACK moves that Session to `Live`; the server then flushes buffered frames in sequence order.

A distinct `observation.ack` from any authorized Session may monotonically advance the shared Cursor, but it cannot satisfy any Session's attach barrier. `room.sync_ack` changes only the issuing Session's barrier state and never advances the Cursor. A token is single-use, Session-bound, and invalid after close or a new attach. Buffer overflow closes the slow Session; reconnect repeats the barrier from the durable Cursor and either catches up or resets.

### Stable stale Action results

An Action result is permanently tied to its Action ID, Canonical Request Hash, and complete basis Head. Retrying the same ID and request returns the stored result even if the Room has advanced. A stale result is never rebased or converted to success. The client installs the required catch-up/reset, re-evaluates the current Action Offers, and—if still legal—submits a new Action ID.

## Activation contract

### Attention and policy boundary

An Attention Signal is deterministic, canonical pack output bound into its Transition. It identifies the target Membership, typed reason, deduplication key, priority, and optional semantic deadline. It does not create or run an Invocation.

The host evaluates a versioned operational Activation Policy. The policy revision plus allow/deny/intent decision is installed atomically beside the causing Transition but excluded from canonical hashes. Replay verifies or reproduces the recorded Attention Signal and exposes recorded decision evidence; it never evaluates current policy, creates an intent, offers work, grants a lease, or contacts a Runner.

An intent is unique by `(room_id, cause_room_seq, member_id, attention_deduplication_key)` and has exactly one state:

~~~mermaid
stateDiagram-v2
    [*] --> Pending
    Pending --> Leased: "claim granted"
    Leased --> Completed: "handled, declined, or failed"
    Leased --> Pending: "release or recoverable lease expiry"
    Pending --> Expired: "intent deadline"
    Pending --> Cancelled: "archive or eligibility fence"
    Leased --> Cancelled: "archive or eligibility fence"
~~~

`pending`, `leased`, `completed`, `expired`, and `cancelled` are the only states. Claim/control attempts and their append-only receipts are records, not extra states. At most one live lease may exist for a Membership across all of its intents; other eligible intents remain pending.

### Eligibility and fences

A new claim requires all of the following at the claim linearization point:

- Room Status active, runtime recovery state Active, and Room Integrity State healthy;
- target Membership enabled, Agent Principal, participant Access Mode, and a current pack Role;
- Runner capability bound to the target Principal and Membership;
- current Activation Policy permits the claim;
- exact Room, integrity, policy, authority, Membership, intent, and lease generations match the prepared witnesses;
- the intent is pending, not expired, and no other intent for the Membership has a live lease.

Archive atomically cancels pending/leased intents and advances a room-wide Activation fence. Suspend, departure, identity binding changes, Access Mode changes, or Role changes cancel/fence affected targets. Capability revocation applies immediately. A policy revision fences stale prepared claims. Runtime recovery state Loading, CatchingUp, Passivating, or Inactive, or Room Integrity State faulted or quarantined, preserves otherwise valid pending intents but makes them unclaimable; verified restoration may resume them if their deadline has not passed.

The operational intent expiry is the earlier of its semantic deadline and the policy maximum. A lease is additionally capped by the policy maximum lease. A detected backward wall-clock jump fences every live lease, returns still-eligible intents to pending under a new generation, and requires a new claim.

### Idempotent operations and linearization

Claim, renew, release, and complete each carry its own operation ID; `claim_id` is the claim operation ID, and later operations also name it as their lease identity. The server derives the canonical request hash from the complete authenticated request. The operation first resolves an existing receipt after authentication and before current availability checks:

- same ID and same hash resolves the durable receipt and returns the exact stored result, including after a lost reply, while any required context is retained; a retired granted context returns deterministic wire `result_retired` without changing that stored result;
- same ID and different hash returns `idempotency_conflict`;
- a new request can durably return `granted`, `not_available`, `expired`, `cancelled`, or `fenced` as applicable.

A successful database COMMIT is the linearization point. Renew/release/complete also name the Activation ID, claim ID, Runner identity, and exact lease generation. An expired or superseded claimant cannot alter a later lease. Cancellation after persistence begins is advisory; an unknown COMMIT resolution is resolved by the same operation identity and Canonical Request Hash, never by inventing another ID.

### Exact Invocation Context

Private context is prepared outside the write lock, revalidated under exact witnesses, and persisted byte-for-byte with a granted claim. It contains:

- Activation/claim identity, typed cause and reason, lease generation/expiry, semantic deadline, and the exact complete Room Head;
- current Membership/Role/Access facts and the integrity, policy, authority, delivery, and schema witnesses used for the grant;
- the current authorized Projection, its schema/hash, and the sole exact ordered ActionOfferV1 representation based on that complete Head;
- authorized artifact references plus versioned explicit runner budget and limits; and
- exactly one delivery branch:
  - `RetainedFrames { cursor_exclusive, through_frame_head, frames }`; or
  - `ProjectionReset { baseline_frame_head, reason }`.

A Head or delivery-witness mismatch may reprepare context under the same claim operation ID before any disposition commits. Authority or integrity mismatch is fenced. Claiming or receiving this context does not move the Cursor or satisfy any Session synchronization token.

Compact intent data, operation receipts, Canonical Request Hashes, immutable original result codes/hashes, context hashes, generations, and dispositions are retained for the Room lifetime. Exact private Invocation Context is retained through terminal state plus seven days, or the longer applicable frame-privacy window; afterward an explicit retention discriminator replaces the bytes with a versioned auditable tombstone while preserving the context hash. A later exact retry deterministically returns wire `result_retired`, not regenerated possibly different private bytes and not a mutation of the original stored result.

## Replay authorization

Present authorization admits a Replay request. The historical Membership, Access Mode, and Role at sequence N determine the participant content that may be projected. A Principal absent at N receives no participant-private view, and a later Role never inherits another Membership's history. Spectator and operator historical views follow their explicit projection policies. A pack-defined final reveal is a separate currently authorized view; Replay itself never expands visibility.

## Conformance fixtures

The frozen suite must prove:

1. Genesis emits no frame; a hidden Transition advances Room Head with zero frames; every visible Transition emits at most one coalesced frame per Membership.
2. Frame head never decreases or reuses values; pruning changes only retained floor; acknowledgements are monotonic and bounded by the current frame head.
3. First attach, a below-floor/pruned range, visibility loss, or another incremental-inappropriate case returns Projection Reset. Retained attach returns the complete range through the captured head. Only matching `room.sync_ack` enters Live and never advances Cursor; only separate `observation.ack` can advance the shared Cursor and never satisfies the Session barrier.
4. A slow consumer closes; reconnect yields complete retained catch-up or reset without a handoff gap. Visibility loss leaves no unauthorized installed state or later addressed frames.
5. A stored stale Action result remains stable on the same identity; successful resubmission after synchronization uses a new Action ID.
6. Loading and Room-CatchingUp deny normal service; Faulted exposes only last-verified allowed surfaces; Quarantined exposes host-operator-only verification surfaces.
7. Attention is hash-bound while policy evidence is noncanonical; Replay creates, offers, claims, and invokes nothing.
8. Intent state, unique logical key, single-live-lease rule, claim lost-reply recovery, changed-hash conflict, expiry/reclaim, generation fences, archive cancellation, authority revocation, and backward-clock fencing are exercised.
9. Granted context contains the exact Projection and complete Head plus exactly one retained/reset branch, and the bytes returned by an exact retry match the committed result.
10. Runner control, participant Action submission, Session synchronization, Cursor acknowledgement, Replay, export, and operator access fail independently when only another capability is held.

## Non-goals

This specification does not implement a WebSocket server, frame store, Runner SDK, model host, or exact-once Invocation. It defines the boundaries those implementations must satisfy.
