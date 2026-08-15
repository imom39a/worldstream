# System Architecture

## Document status

This document implements the [Frozen Requirements](requirements.md). It is normative for v0.1 and v0.2 where it defines an invariant or a frozen technology decision.

The intended system is a single self-hosted Rust process with strong internal module boundaries. Those boundaries make future replacement possible; they are not a reason to deploy microservices now.

## Architecture summary

WorldStream is a deterministic, transition-backed room runtime with Membership-addressed Observation Streams and durable Activation Intents.

~~~mermaid
flowchart LR
    HC["Human client"] --> GW["HTTP and WebSocket gateway"]
    AR["External agent runner"] --> GW
    GW --> RS["Room supervisor"]
    RS --> RA["Single-writer room actor"]
    RA --> AP["Trusted Activity Pack"]
    RA --> ST["Storage service"]
    ST --> DB["SQLite WAL"]
    RA --> DL["Committed frame delivery"]
    DL --> HC
    DL --> AR
    ST --> AC["Activation queue"]
    AC --> AR
    DB --> RP["Recovery and read-only replay"]
    RP --> AP
~~~

The important boundary is not WebSocket versus HTTP. It is:

- WorldStream owns shared-state correctness and participation continuity;
- an Activity Pack owns domain meaning;
- external clients and runners own human interaction and agent execution.

## Architecture goals

1. One understandable total order per room.
2. No acknowledged accepted action is lost on process termination.
3. Retries do not duplicate room mutation.
4. Private state cannot accidentally enter a public or unauthorized projection.
5. A temporary connection or agent invocation is never mistaken for durable identity.
6. A disconnected membership can catch up exactly or receive an explicit projection reset.
7. An external runner can receive an at-least-once activation without WorldStream hosting a model.
8. Recovery and Replay reproduce Core, Activity, aggregate Authoritative State, and lineage hashes.
9. One ordinary developer machine can run the complete system.
10. The second activity can be added without Room Kernel special cases.

## Frozen technology stack

Library versions are pinned in Cargo.lock and the frontend/Python lockfiles when implementation begins. The architecture freezes choices, not unreviewed floating version numbers.

| Layer | Frozen choice | Reason |
|---|---|---|
| Language | Rust stable, edition 2024, pinned by rust-toolchain.toml | One deployable binary, strong types, predictable resource use |
| Async runtime | Tokio | Mature asynchronous I/O, bounded channels, timers, shutdown |
| HTTP and WebSocket | Axum plus Tower and tower-http | Small Rust-native gateway and composable limits/middleware |
| Serialization | Serde and serde_json | Cross-language JSON protocol and simple golden fixtures |
| Schemas | Schemars plus strict typed deserialization | Publish JSON Schemas while preserving Rust types |
| Embedded database | Rusqlite with bundled SQLite | Direct transactions and explicit control over one-writer semantics |
| Identifiers | ULID strings | Readable, sortable external identifiers; room sequence remains authoritative |
| Hashing | BLAKE3 | State, transition, payload, and artifact integrity |
| Time | time crate and RFC 3339 UTC at boundaries | Explicit audit timestamps; pack time remains recorded |
| CLI/config | Clap plus versioned TOML and environment overrides | One server binary and predictable local operation |
| Errors | thiserror in libraries; anyhow only at binary boundary | Typed protocol/storage errors without application boilerplate |
| Telemetry | tracing, JSON logs, Prometheus text metrics | Debuggability without storing application logs in the data directory |
| Python SDK | Python 3.11+, websockets, Pydantic | Fastest path for external agent runners and typed examples |
| Web UI | React, TypeScript, Vite, native browser WebSocket | Small first-party reference UI; no realtime framework dependency |
| Packaging | Native binary and non-root multi-stage Docker image | Easy local use and reproducible demo |

SQLite MUST be a release that contains the 2026 WAL-reset correction, such as SQLite 3.51.3 or an official fixed backport. CI and startup diagnostics MUST print and validate the linked SQLite version. Using Rusqlite's bundled feature prevents the host from silently selecting an older system library.

Not selected for v0.1 or v0.2: SQLx, an ORM, Postgres, Redis, NATS, Kafka, Temporal, Wasmtime, Kubernetes, an embedded model SDK, or a frontend realtime platform.

Primary implementation references:

- [Tokio runtime and tutorial](https://tokio.rs/tokio/tutorial)
- [Axum WebSocket module](https://docs.rs/axum/latest/axum/extract/ws/)
- [Rusqlite crate documentation](https://docs.rs/rusqlite/latest/rusqlite/)
- [SQLite WAL documentation](https://www.sqlite.org/wal.html)
- [SQLite WAL-mode file lifecycle](https://www.sqlite.org/walformat.html)

## Target repository layout

The initial workspace should resist both a monolith and speculative crate explosion:

    agent-streamer/
    ├── Cargo.toml
    ├── Cargo.lock
    ├── rust-toolchain.toml
    ├── README.md
    ├── crates/
    │   ├── worldstream-protocol/
    │   │   └── IDs, envelopes, schemas, cursors, errors
    │   ├── worldstream-core/
    │   │   └── Activity Pack host interface, room actor, projections, activation model
    │   ├── worldstream-sqlite/
    │   │   └── migrations, storage port, backup and integrity operations
    │   └── worldstream-server/
    │       └── gateway, auth, supervisor, scheduler, binaries
    ├── activities/
    │   ├── agent-heist/
    │   └── investigation-room/       added after Heist v0.1
    ├── sdk/
    │   └── python/
    ├── web/
    │   └── console/
    ├── examples/
    │   ├── python-rule-runner/
    │   ├── python-llm-runner/
    │   └── human-client/
    ├── migrations/
    ├── tests/
    │   ├── protocol/
    │   ├── activity-conformance/
    │   ├── privacy/
    │   ├── recovery/
    │   ├── failure-injection/
    │   └── load/
    ├── deploy/
    │   ├── docker/
    │   └── systemd/
    └── docs/

The storage interface belongs in worldstream-core; SQLite is the only implementation. There are no provider, broker, crypto, workflow, plugin, or generic connector crates.

## Runtime components

### Gateway

The gateway is responsible for:

- HTTP and WebSocket framing;
- development bearer-token authentication;
- origin, payload-size, rate, and timeout controls;
- connection heartbeats and graceful close;
- routing an authenticated command to a room;
- bounded per-connection output queues;
- mapping internal results to stable protocol errors.

The gateway MUST NOT apply activity rules, mutate room state, construct private projections, or acknowledge an action before durable commit.

### Room supervisor

The supervisor maps room IDs to active room actors.

- A Room loads lazily from verified Genesis or its newest valid paired Core-and-Activity snapshot plus Transition tail.
- Concurrent load requests coalesce into one load.
- Each active room has one bounded mailbox.
- An idle room may passivate after five minutes if it has no connected sessions, mailbox work, pending activation lease operation, or near-due timer.
- A failed room actor is discarded and reloaded from durable state.
- A Room that fails canonical verification is quarantined and exposes only authenticated host-operator diagnostics, raw export, restore, and verification.

The active-room map is in memory. It is not a distributed registry.

The supervisor uses an explicit per-room lifecycle:

    Loading → Active → Passivating → Inactive
                    ↘ Faulted or Quarantined

Every actor receives a supervisor generation. Passivation occurs through a barrier: mark Passivating, stop routing directly, drain the actor mailbox, confirm no provisional commit/timer work, then remove the actor. Commands arriving during Loading or Passivating wait in a bounded supervisor queue or receive room_busy; they are never sent to a channel whose actor can exit. A stale-generation actor cannot publish after removal, and its expected-head check prevents an obsolete commit.

### Single-writer room actor

Every active room has exactly one logical writer task. Actions in different rooms may run concurrently. Accepted stimuli inside one room are deliberately serialized.

The actor owns:

- current canonical Core and Activity State;
- the complete Room Head: sequence, lineage hash, version identities, and all three state hashes;
- current Membership index used for authorization and Viewer construction;
- provisional timer view;
- calls into the pinned Activity Pack;
- preparation of one atomic storage commit;
- publishing only committed frames.

Core Room Status, operational Room Integrity State, and pack phase are separate:

- status: active or archived;
- integrity: healthy, faulted, or quarantined, paired with a monotonic integrity generation;
- Activity Phase: arbitrary pack-defined stage;
- Outcome: a separate pack-defined result, which may be established before the Terminal Phase.

An archived Room is canonical and irreversible. If healthy, it permits authorized reads/export/Replay and ordered suspend/depart only. A faulted Room rejects every canonical mutation but may serve its last verified authorized Projection, retained Frame Catch-up, and verified Replay with explicit integrity metadata. A quarantined Room serves none of those normal surfaces; only authenticated host-operator diagnostics, raw export, restore, and verification remain. A terminal pack phase normally rejects ordinary domain Actions while the Core Room may remain active until explicitly archived.

The actor MUST use a bounded mailbox. Backpressure reaches the gateway as a typed busy response; it does not create more actor tasks for the same room.

### Versioned Core reducer

`CoreReducerV1` is a pure WorldStream-owned reducer. `CoreRoomStateV1` contains exactly active/archived Room Status and a canonically sorted Membership map. Each Membership contains immutable Member ID, Principal ID, and room-local Principal kind; enabled/suspended/departed standing; participant/spectator/operator Access Mode; and a pack-valid Role exactly when Access Mode is participant. Room Head, hashes, integrity, Sessions, delivery, receipts, Activation, policy, diagnostics, telemetry, and wall time are not Core fields.

A normalized Core Stimulus carries:

- a versioned kind: join, resume, suspend, depart, Access/Role change, atomic Membership changeset, or archive;
- canonical authority attribution without bearer secrets;
- idempotency identity and exact expected Room sequence;
- a stable reason code and semantic recorded time when applicable;
- a canonical before/after pair for each affected Member ID, sorted by Member ID.

Member/Principal binding and Principal kind are immutable. Enabled and suspended are reversible; departed is terminal and rejoin uses a new Member ID without inherited Cursor/private frames. At most one non-departed Membership per Principal exists in a Room. Participant requires a Role; spectator/operator forbids one. The reducer validates the complete final multi-Membership state and pack cardinality once, never an intermediate assignment.

The Activity Pack receives immutable Core-before/proposed-after views. Join, resume, Access Mode, and Role proposals are vetoable; a declared rejection is a stable idempotent administrative result with no Transition. Archive, suspend, and depart are mandatory; a declared veto is a Pack Fault. A pre-existing final state may produce a durable NoChange disposition. Every other accepted Core change creates exactly one Transition. Archive is irreversible and atomically cancels timers and fences pending/leased Activation work in the winning order.

### Activity host

The Activity host invokes one trusted, compiled-in pack revision.

The logical interface is:

~~~rust
pub trait ActivityPack: Send + Sync + 'static {
    fn manifest(&self) -> ActivityManifest;

    fn initialize(
        &self,
        input: RoomInitialization,
        context: &DeterministicContext,
    ) -> Result<CanonicalValue, ActivityFault>;

    fn apply(
        &self,
        state: &CanonicalValue,
        core_before: &CoreRoomStateV1,
        proposed_core_after: &CoreRoomStateV1,
        stimulus: &RecordedStimulus,
        context: &DeterministicContext,
    ) -> Result<AppliedTransition, ApplyError>;

    fn project(
        &self,
        state: &CanonicalValue,
        viewer: &Viewer,
    ) -> Result<ActivityProjection, ActivityFault>;

    fn observe(
        &self,
        before: &CanonicalValue,
        after: &CanonicalValue,
        events: &[DomainEvent],
        viewer: &Viewer,
    ) -> Result<Option<ActivityObservation>, ActivityFault>;
}

pub enum ApplyError {
    Rejected(DeclaredRejection),
    ActivityFault(ActivityFault),
}
~~~

AppliedTransition contains:

- the complete next Canonical Activity State;
- ordered domain events for audit and UI;
- timer schedule/cancel operations;
- deterministic attention signals.

Validation and Activity reduction occur in one apply call so they cannot disagree after state changes. For a non-Core Stimulus, `core_before` and `proposed_core_after` are equal. DeclaredRejection applies to Participant Actions and vetoable Core proposals only; returning it for archive, suspend, depart, or another mandatory host input is an Activity Fault. Project returns the pack-owned Activity Projection, and Observe returns a bounded pack-owned Activity Observation for one viewer. WorldStream wraps Activity Projection with authorized Core Room and Membership facts to form a Projection; protocol envelopes add causal sequence, operational Room Integrity State, schema, and delivery metadata.

The host supplies no database, network, filesystem, environment, model, wallet, or wall-clock handle. Same-process Rust is not a sandbox: compiled-in packs are fully trusted by the host operator. Public or untrusted pack loading is explicitly unsupported.

### Storage service

One dedicated database writer thread owns the SQLite write connection. Room actors submit typed commit batches over a bounded channel. This aligns application ordering with SQLite's one-writer behavior and avoids an async connection pool pretending that writes are parallel.

The storage module MAY use separate dedicated read connections for room loading, catch-up, replay, and host-operator queries. Reads that determine whether a write is valid are repeated inside the write transaction.

The storage service owns:

- forward-only migrations;
- atomic transition commits;
- action-receipt lookup;
- snapshot and transition reads;
- observation-frame and activation queries;
- cursor acknowledgement persistence;
- WAL checkpoint control and metrics;
- backup and integrity-check operations.

### Projection and observation engine

After a pack computes the next state, WorldStream asks it for affected audience projections and deltas.

- Authoritative Room State is never serialized directly to a client.
- Public, operator-membership, and participant viewers are distinct Rust types.
- Every Observation Frame is persisted under an explicit Membership ID.
- A public payload uses an explicitly public type, then is materialized into each authorized enabled membership's single frame stream.
- Projection construction happens before the transition transaction commits.
- Invalid, oversized, or failed projection output aborts the transition rather than committing undisclosable state.

For the frozen room sizes, evaluating at most 32 viewer projections is an acceptable clarity-over-optimization tradeoff.

### Observation delivery

Committed observation frames are durable. Live WebSocket delivery is an acceleration path:

- after storage commit, a room actor enqueues frame references to connected sessions;
- each session fetches or receives only authorized payloads;
- client acknowledgements monotonically advance a membership frame cursor;
- duplicate delivery is allowed;
- queue overflow closes the connection with a resumable slow-consumer error.

The durable inbox, not an in-memory broadcast channel, is the continuity guarantee.

### Activation dispatcher

An attention signal is deterministic Activity Pack output. The host applies an exact, typed per-membership activation policy and persists an activation intent in the same transaction as the causing transition.

The dispatcher:

- offers pending intents on a runner control WebSocket or HTTP long poll;
- atomically grants a bounded claim lease;
- exposes activation context after claim;
- renews, completes, expires, or cancels a lease;
- retries delivery without creating another logical activation;
- records operational attempts without changing room history.

It never invokes a model. If a runner is absent, nothing runs.

Activation-control authority is separate from participant Action authority. A successful claim returns authorized Invocation Context but neither grants room:act nor advances the Membership Cursor. Runner SDKs expose separate activation-control and room-member clients, following [ADR 0003](adr/0003-separate-activation-and-action-authority.md).

### Timer scheduler

Pack-requested timer changes commit with the transition that requested them. A host scheduler scans due timers and submits a recorded TimerFired stimulus to the room actor.

The firing transition and timer fired status commit atomically. A process crash before commit causes a retry; a crash after commit cannot create a second logical firing.

### Recovery and replay

Recovery reconstructs mutable room state. Replay reconstructs the same state read-only for debugging and the UI. Neither contacts runners, sends frames, or executes external effects.

## Canonical data and determinism

### JSON profile

The wire protocol and persisted pack payloads use a strict canonical JSON profile:

- UTF-8 only;
- no duplicate object keys;
- object keys sorted lexicographically for hashing;
- arrays preserve declared order;
- null, booleans, strings, arrays, objects, and integers only;
- no floating-point or non-finite numeric values in Core Room State or Activity State;
- JSON integers remain within the exact cross-language safe range;
- timestamps and ULIDs are normalized strings;
- unknown fields are rejected for commands unless a versioned schema explicitly allows them.

Hashes operate on canonical UTF-8 bytes, never implementation-specific map serialization.

### IDs, sequences, and hashes

- External IDs are ULID strings.
- Every room has an unsigned monotonically increasing sequence.
- Every membership has an independent monotonically increasing observation-frame sequence.
- Room sequence establishes order; ULID lexical order does not.
- Payload hash identifies an idempotent action body.
- Core State hash domain-separates the Core schema version and canonical Core bytes.
- Activity State hash domain-separates the exact pack digest and canonical Activity bytes.
- Authoritative State hash domain-separates and binds both component hashes plus their version identities.
- Genesis hash binds Room/version identities, Core schema version, exact pack digest, canonical configuration, normalized initial timers, Room seed, logical creation time, and all three initial state hashes.
- Transition hash binds Room/sequence/version identities, Core schema version, exact pack digest, the prior Genesis-or-Transition hash, normalized recorded Stimulus, ordered Domain Events, normalized ordered timer changes, deterministic ordered Attention Signals, and all three resulting state hashes.

Every hash input is one canonical typed object containing an explicit domain/version field. Raw concatenation of variable-length fields is forbidden. Golden vectors define integer, byte/digest, and string encoding. Integrity state/generation/incidents, bearer or commit witnesses, receipts, materializations, snapshots, Projections/frames/Cursors/resets/Sessions, policy and Activation records, diagnostics, telemetry, and commit wall time are excluded. Canonical authority attribution, idempotency identity, expected sequence, reason, and semantic time/input fields inside a normalized Stimulus remain included.

The complete Room Head is the atomic tuple `(room_id, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash)`. Genesis establishes it at sequence zero; every accepted Transition replaces the complete tuple.

### Time

Server receipt and commit timestamps are operational audit metadata. A pack only receives recorded logical time in the stimulus.

Deadline resolution uses the host-recorded acceptance time for an action or the recorded TimerFired input. Replay reuses those values.

### Randomness

Room creation records a cryptographically random seed. The deterministic context derives labeled pseudo-random values from:

    BLAKE3(canonical_json({
      "domain": "worldstream/activity-random/v1",
      "room_seed": "...",
      "next_room_sequence": 42,
      "label": "...",
      "index": 0
    }))

The same pack and transition sequence therefore reproduce the same values. A pack cannot call the operating-system random generator.

## Command lifecycle

~~~mermaid
sequenceDiagram
    participant C as "Human or agent client"
    participant G as "Gateway"
    participant R as "Room actor"
    participant P as "Activity Pack"
    participant S as "SQLite writer"

    C->>G: action.submit with action_id and based_on_seq
    G->>R: authenticated typed command
    R->>S: read durable action receipt

    alt Existing action ID
        S-->>R: original stored result
        R-->>C: same accepted or rejected result
    else New action ID
        R->>P: apply current state and recorded stimulus

        alt Rejected
            R->>S: persist stable rejection receipt
            S-->>R: committed
            R-->>C: typed rejection
        else Accepted
            R->>P: derive projections and observations
            R->>S: atomic transition commit batch
            S-->>R: committed
            R->>R: install new in-memory state
            R-->>C: accepted sequence and hashes
            R-->>C: committed observation frames
        end
    end
~~~

Commit-before-acknowledgement is non-negotiable.

### Failure results

| Failure point | Observable result |
|---|---|
| Before SQLite commit | Nothing authoritative happened; the client may retry |
| After commit but before action reply | Retry returns the stored original receipt |
| After commit but before frame send | Cursor catch-up returns the committed frame |
| After activation creation but before offer | The pending intent remains queryable |
| After activation lease but before runner result | Lease expires and the same intent may be claimed again |
| Postcommit snapshot write failure | The accepted Transition remains committed; retry idempotently or recover from an older pair/Genesis |
| Process termination with due timers | Startup scan resubmits idempotent TimerFired candidates |
| Projection construction failure | No transition is committed |
| Pack panic | The room actor fails; the room reloads or is quarantined after repeated deterministic failure |
| Disk full or SQLite I/O error | Mutation fails closed; readiness reports unhealthy |

## SQLite data model

The concrete migrations may add operational columns, but the following entities and constraints are frozen.

### principals

Durable identity outside any room:

    principal_id TEXT PRIMARY KEY
    kind TEXT CHECK kind IN ('human', 'agent')
    display_name TEXT
    metadata_json BLOB
    status TEXT
    created_at TEXT

### capabilities

Development bearer capabilities. Only a strong token hash is stored:

    capability_id TEXT PRIMARY KEY
    token_hash BLOB UNIQUE
    principal_id TEXT
    runner_id TEXT NULL
    room_id TEXT NULL
    scopes_json BLOB
    expires_at TEXT NULL
    revoked_at TEXT NULL

### runners

Server-issued identities for external executor processes:

    runner_id TEXT PRIMARY KEY
    owner_principal_id TEXT
    display_name TEXT
    status TEXT CHECK status IN ('enabled', 'revoked')
    metadata_json BLOB
    created_at TEXT
    updated_at TEXT

Runner availability is temporary and remains in memory/metrics. A runner capability binds this server-issued ID and exact claim scopes; a client cannot choose an unrelated lease identity.

### rooms

    room_id TEXT PRIMARY KEY
    pack_id TEXT
    pack_version TEXT
    pack_digest BLOB
    status TEXT CHECK status IN ('active', 'archived')
    integrity_state TEXT CHECK integrity_state IN ('healthy', 'faulted', 'quarantined')
    integrity_generation INTEGER
    genesis_hash BLOB
    room_seed BLOB
    head_seq INTEGER
    head_lineage_hash BLOB
    core_schema_version TEXT
    head_core_state_hash BLOB
    head_activity_state_hash BLOB
    head_authoritative_state_hash BLOB
    created_at TEXT
    updated_at TEXT

`status` and the three state hashes are verified current materializations. `integrity_state` and `integrity_generation` are durable operational fencing state. Neither this row nor the current Membership rows supersede canonical Genesis/Transitions.

### room_genesis

The immutable source for recovery when every snapshot is absent:

    room_id TEXT PRIMARY KEY
    core_schema_version TEXT
    pack_digest BLOB
    configuration_json BLOB
    initial_core_state_json BLOB
    initial_activity_state_json BLOB
    initial_timers_json BLOB
    room_seed BLOB
    logical_created_at TEXT
    genesis_hash BLOB
    initial_core_state_hash BLOB
    initial_activity_state_hash BLOB
    initial_authoritative_state_hash BLOB

Room sequence zero names genesis. The first later accepted stimulus is sequence one. The previous transition hash for sequence one is the domain-separated genesis hash.

### room_members

    room_id TEXT
    member_id TEXT
    principal_id TEXT
    principal_kind TEXT CHECK principal_kind IN ('human', 'agent')
    role TEXT NULL
    access_mode TEXT CHECK access_mode IN ('participant', 'spectator', 'operator')
    standing TEXT CHECK standing IN ('enabled', 'suspended', 'departed')
    joined_seq INTEGER
    frame_head INTEGER
    last_ack_frame_seq INTEGER
    activation_policy_json BLOB
    created_at TEXT
    updated_at TEXT
    PRIMARY KEY (room_id, member_id)

Member ID, Principal ID/kind, standing, Access Mode, and Role are the current Core materialization. Frame head/Cursor and activation policy are operational delivery/control fields co-located physically but excluded from Core and canonical hashes. Connection and model invocation state MUST NOT be columns in this table.

`role` is required when `access_mode = 'participant'` and absent for spectator/operator Memberships. The Activity Pack defines allowed Roles and cardinality; the Core reducer owns each current assignment. Member/Principal binding and Principal kind never update, departed is terminal, and a uniqueness constraint permits at most one non-departed Membership per Principal in a Room.

### transitions

    room_id TEXT
    seq INTEGER
    transition_id TEXT UNIQUE
    core_schema_version TEXT
    pack_digest BLOB
    initiator_member_id TEXT NULL
    action_id TEXT NULL
    stimulus_kind TEXT
    stimulus_json BLOB
    domain_events_json BLOB
    timer_changes_json BLOB
    attention_signals_json BLOB
    logical_time TEXT
    previous_transition_hash BLOB
    resulting_core_state_hash BLOB
    resulting_activity_state_hash BLOB
    resulting_authoritative_state_hash BLOB
    transition_hash BLOB
    committed_at TEXT
    PRIMARY KEY (room_id, seq)

### action_receipts

    room_id TEXT
    member_id TEXT
    action_id TEXT
    payload_hash BLOB
    result_status TEXT CHECK result_status IN ('accepted', 'domain_rejected')
    transition_seq INTEGER NULL
    response_json BLOB
    created_at TEXT
    PRIMARY KEY (room_id, member_id, action_id)

The stored payload hash detects same-ID/different-command conflicts. Transient admission/runtime errors never enter this table and do not consume an action ID.

### mutation_receipts

Durable idempotency for mutating HTTP administration:

    principal_id TEXT
    operation TEXT
    idempotency_key TEXT
    request_hash BLOB
    response_status INTEGER
    response_json BLOB
    created_at TEXT
    PRIMARY KEY (principal_id, operation, idempotency_key)

The receipt commits with the created/changed resource. Same key and request hash returns the original response; a changed request hash is an idempotency conflict. worldstreamctl generates capability bearer secrets locally and sends only their derived token hash, so retrying capability creation never requires the server to store or replay plaintext.

### snapshots

    room_id TEXT
    seq INTEGER
    lineage_hash BLOB
    core_schema_version TEXT
    pack_digest BLOB
    encoding TEXT
    core_state_blob BLOB
    core_state_hash BLOB
    activity_state_blob BLOB
    activity_state_hash BLOB
    authoritative_state_hash BLOB
    created_at TEXT
    PRIMARY KEY (room_id, seq)

One row is an indivisible paired Core-and-Activity checkpoint at one complete Head. It is written idempotently after the canonical commit; compression, cadence, and creation time are replaceable details outside canonical hashes. A mismatched component invalidates the entire pair.

### integrity incidents and repairs

The current integrity state/generation may be co-located on `rooms`, but incidents and repair attempts are a separate append-only operational audit:

    incident_id TEXT PRIMARY KEY
    room_id TEXT
    observed_generation INTEGER
    state_before TEXT
    state_after TEXT
    reason_code TEXT
    verifier_identity TEXT NULL
    evidence_json BLOB
    recorded_at TEXT

An operator request may enqueue verification, never write `healthy` directly. Only a verifier result conditioned on the current generation may change integrity state, and every change increments the generation. Incident data is excluded from Room sequence and canonical hashes.

### observation_frames

    room_id TEXT
    recipient_member_id TEXT
    frame_seq INTEGER
    cause_room_seq INTEGER
    frame_kind TEXT
    payload_json BLOB
    payload_hash BLOB
    created_at TEXT
    PRIMARY KEY (room_id, recipient_member_id, frame_seq)

Every durable stream belongs to one Membership. Public consequences are materialized into every enabled Membership stream allowed to see them. Spectator and operator Memberships are read-only and have separate Cursors; Principal kind remains independent of Access Mode. This deliberately duplicates small frames to keep privacy and Catch-up semantics unambiguous.

### timers

    room_id TEXT
    timer_id TEXT
    generation INTEGER
    due_at TEXT
    payload_json BLOB
    status TEXT CHECK status IN ('scheduled', 'cancelled', 'fired')
    created_seq INTEGER
    cancelled_seq INTEGER NULL
    fired_seq INTEGER NULL
    PRIMARY KEY (room_id, timer_id, generation)

The scheduler index is on status and due_at.

### activation_intents

    activation_id TEXT PRIMARY KEY
    room_id TEXT
    member_id TEXT
    cause_room_seq INTEGER
    reason_code TEXT
    relevant_from_frame_seq INTEGER
    relevant_to_frame_seq INTEGER
    allowed_actions_json BLOB
    priority INTEGER
    deadline TEXT NULL
    deduplication_key TEXT
    status TEXT CHECK status IN ('pending', 'leased', 'completed', 'expired', 'cancelled')
    current_claim_id TEXT NULL
    lease_runner_id TEXT NULL
    lease_generation INTEGER
    lease_until TEXT NULL
    attempt_count INTEGER
    completed_at TEXT NULL
    created_at TEXT

Require a unique logical activation key on room_id, cause_room_seq, member_id, and deduplication_key. Index pending/leased activations by status, lease_until, priority, and created_at.

The frozen host permits at most one live leased Activation per Membership, so WorldStream-controlled activation starts are serialized. Additional intents may remain pending until the current lease is completed, released, expired, or cancelled. WorldStream cannot prevent a Runner from starting an independent Invocation outside this mechanism; if multiple Invocations submit Actions for one Membership, exact-head admission and Action idempotency resolve the race normally.

### activation_claims

Durable claim receipts and stale-lease protection:

    activation_id TEXT
    claim_id TEXT
    runner_id TEXT
    claim_request_hash BLOB
    lease_generation INTEGER
    claim_result TEXT CHECK claim_result IN ('granted', 'not_available', 'expired')
    lease_until TEXT NULL
    claim_response_json BLOB
    completion_request_hash BLOB NULL
    completion_disposition TEXT NULL
    completion_response_json BLOB NULL
    created_at TEXT
    completed_at TEXT NULL
    PRIMARY KEY (activation_id, claim_id)

A repeated claim ID from the same authenticated runner and identical canonical request returns its stored claim result. Reusing it with a changed request or runner is an idempotency conflict. Renew, release, and complete must match the current claim ID, runner ID, generation, and unexpired lease. Repeating an identical terminal completion returns its stored result; changing the completion body is an idempotency conflict.

### artifacts and room_artifacts

These generic tables appear in v0.2 only:

    artifacts(
        digest BLOB PRIMARY KEY,
        size_bytes INTEGER,
        media_type TEXT,
        relative_path TEXT,
        created_at TEXT
    )

    room_artifacts(
        room_id TEXT,
        digest BLOB,
        label TEXT,
        visibility_json BLOB,
        added_seq INTEGER,
        PRIMARY KEY (room_id, digest, label)
    )

    artifact_uploads(
        upload_id TEXT PRIMARY KEY,
        room_id TEXT,
        uploader_member_id TEXT,
        digest BLOB,
        size_bytes INTEGER,
        media_type TEXT,
        status TEXT CHECK status IN ('staged', 'linked', 'expired'),
        expires_at TEXT,
        created_at TEXT,
        linked_seq INTEGER NULL
    )

Evidence version, supersession, claims, clues, plans, and evidence relationships remain in canonical pack state and events. The core artifact subsystem knows only immutable bytes, authorization metadata, staged ownership, and room references.

### Required SQLite settings

On every write-capable connection:

    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = FULL;
    PRAGMA foreign_keys = ON;
    PRAGMA busy_timeout = 5000;

Every read connection also enables foreign_keys and busy_timeout and sets query_only = ON. Pragmas that are connection-local are never assumed to carry across connections.

Migrations are embedded, forward-only, and run before readiness. The writer controls checkpoints and reports WAL size and checkpoint latency. Large write transactions are avoided.

Actual migrations MUST mark all required fields NOT NULL, use CHECK constraints for every documented enum, and declare foreign keys for correctness relationships, including:

- capabilities and runners to principals;
- genesis, members, transitions, snapshots, frames, timers, activations, uploads, and room artifacts to rooms;
- members and upload owners to principals/memberships as appropriate;
- accepted action receipts to their transition;
- observation/timer/activation cause sequences to a transition in the same room when nonzero;
- activation claims to activation intents and runners;
- room artifacts/uploads to artifact metadata.

Startup and restore diagnostics run integrity_check and foreign_key_check before readiness. Tests prove that every connection enables enforcement.

## Atomic transition transaction

For an accepted stimulus, one BEGIN IMMEDIATE transaction MUST:

1. Re-read the complete Room Head, require `healthy`, conditionally match the integrity generation, and recheck every stimulus-specific durable precondition under the write lock.
2. For a participant action, require based_on_room_seq to equal head exactly and verify the action receipt/payload hash.
3. For a TimerFired candidate, conditionally change the exact room/timer/generation row from scheduled to fired and require one affected row. Zero rows means a duplicate or stale candidate: no pack mutation and no transition.
4. For a Core input, conditionally match its exact canonical before/after Membership changeset and authority witness without exposing an intermediate final-state violation.
5. Insert the next Transition with its normalized Stimulus, ordered outputs, three resulting state hashes, and lineage hash.
6. Install the complete Room Head atomically.
7. Install verified current Core/Membership and Activity materializations.
8. Insert or cancel timers.
9. Insert Membership-addressed Observation Frames and advance each affected Membership's `frame_head`.
10. Insert activation intents under their unique logical keys.
11. Insert the accepted Action or administrative receipt.
12. Commit.

If the integrity state/generation fence changed, the transaction writes nothing and consumes no sequence or receipt. A paired snapshot may be written idempotently only after this commit; snapshot failure cannot change the accepted result.

For a deterministic admitted domain or vetoable-administration rejection, a smaller transaction records the stable rejection receipt without changing Room Head. A pre-existing desired Core state may similarly record NoChange. Authentication/authorization failure, malformed input, rate limit, room busy, storage unavailable, and activity/runtime faults use the transient error path, create no action receipt, and allow retry with the same action ID.

Mutating HTTP administration uses mutation_receipts in the same transaction as its resource change.

Room archive and accepted Membership Standing, Access Mode, or Role changes are Core Stimuli and consume the next Room sequence. Their receipt, Core and Activity results, Domain Events, hashes, timers, addressed Frames, and allowed Activation decisions commit atomically with that Transition. Archive also cancels scheduled timers and fences pending/leased Activation work. Session presence, Runner availability, Activation lease operations, integrity incidents/repair, diagnostics, and telemetry remain operational and never consume Room sequence, following [ADR 0002](adr/0002-sequence-domain-relevant-room-changes.md) and [ADR 0005](adr/0005-canonical-core-state-integrity-and-hash-lineage.md).

The actor installs new in-memory state only after commit returns. If commit succeeded but the actor fails before doing so, the supervisor discards it and reconstructs from storage.

Cursor acknowledgements may be batched because losing a recent acknowledgement only causes duplicate delivery, not data loss. They remain monotonic and can never acknowledge a nonexistent frame.

## Reconnect and delivery semantics

Frozen guarantees:

- exactly one canonical result for one room/member/action ID;
- exactly one logical accepted mutation for that ID;
- at-least-once observation-frame delivery;
- at-least-once activation offer;
- no exactly-once network or model-execution claim.

Within one live WebSocket, the server emits frames in that membership's frame-sequence order. Every public consequence visible to the member is already materialized into this one stream.

Attachment uses an actor barrier so catch-up cannot lose the transition between a database query and live subscription:

1. the client supplies its last durably processed frame cursor;
2. the room actor verifies membership/cursor, registers the session as catching_up, and captures member frame head H;
3. storage reads and sends frames in (cursor, H] using short bounded pages;
4. newly committed frame references above H enter the session's bounded buffer;
5. if the old range was pruned, the server sends resync-required plus an authorized current projection at H;
6. after the through-H frame/reset is installed, the actor atomically switches the session to live and flushes buffered frames in order;
7. buffer overflow closes the connection and requires another attach.

The client deduplicates by room, member, and frame sequence. An action retry uses its independent action ID.

Multiple Sessions attached to one Membership share that Membership's Observation Stream and Cursor. An acknowledgement from any authorized Session advances the shared Cursor, so independent delivery consumers require separate Memberships.

### Default resource limits

Initial defaults, configurable only downward for public deployments:

| Resource | Default |
|---|---:|
| Client action envelope | 64 KiB |
| Observation frame | 256 KiB |
| WebSocket message | 512 KiB |
| Canonical Activity State | 2 MiB |
| Durable members per room | 32 |
| Active sessions per membership | 2 |
| Actor mailbox | 256 commands |
| Connection outbound buffer | 256 frames and 4 MiB |
| Transition domain-event output | 256 KiB |
| Heist rooms active per process target | 100 |

A connection that exceeds its output bound is closed with a typed slow-consumer error and can reconnect from its cursor.

## Activation lifecycle

Activation is participation continuity, not model hosting.

~~~mermaid
stateDiagram-v2
    [*] --> Pending: "Committed attention signal"
    Pending --> Leased: "Runner claims"
    Leased --> Completed: "Runner completes"
    Leased --> Pending: "Lease expires"
    Pending --> Expired: "Deadline or retention"
    Pending --> Cancelled: "Host operator, archive, or terminal policy"
    Leased --> Cancelled: "Host operator, archive, or terminal policy"
~~~

Membership, runner availability, and activation state remain separate:

- an enabled membership can have no runner;
- a runner can stay connected while no model invocation exists;
- a leased activation does not prove a model produced useful work;
- completion records runner handling, while room actions record authoritative consequences.

A runner claim response contains:

- activation ID, claim ID, lease generation, and cause sequence;
- reason code and deadline;
- allowed action kinds;
- current authorized projection;
- frames after the membership cursor, or a projection reset;
- explicit artifact references authorized for that membership;
- lease expiry.

Claim grant/reclaim is one conditional database transaction. Retrying the same claim ID returns the stored original result. Renew, release, and complete conditionally match the authenticated runner, current claim ID, current generation, and unexpired lease. An expired older claim can never complete a later lease.

v0.1 supports a runner control WebSocket and HTTP long poll. It does not call arbitrary user URLs. A webhook is not a committed v0.2 feature.

## Snapshots, recovery, and replay

### Snapshot policy

Defaults:

- create one paired Core-and-Activity snapshot every 250 accepted Transitions or five active minutes, whichever occurs first, in an idempotent postcommit job;
- retain immutable Genesis independently and the latest three automatic pairs; an optional sequence-zero pair is only a cache;
- keep canonical transitions and action receipts for the room lifetime in frozen releases;
- prune acknowledged observation frames only after a seven-day safety window or a configurable per-member high-water mark;
- never use snapshot deletion to change canonical history.

### Room load

1. Read Room Integrity State/generation, immutable Genesis, and the complete Room Head.
2. Verify Genesis, its three initial state hashes, Genesis hash, exact Core schema/pack revision identities, and normalized initial timers.
3. Find the newest compatible paired snapshot at or before Head. Verify both canonical state values, both component hashes, aggregate hash, and applicable lineage hash; discard the whole pair on any mismatch.
4. If no pair verifies, reconstruct initial Core with the exact Core schema and initialize Activity State/timers with the exact pack from Genesis.
5. Replay every later Transition through the same versioned Core reducer and exact pack reducer.
6. At each sequence verify prior lineage, normalized Stimulus/ordered outputs, Transition hash, and Core, Activity, and aggregate hashes.
7. Reconstruct timer consistency and compare the final complete Head plus current Core/Membership/Activity materializations.
8. Install serving state only under the current integrity generation. A healthy result may become Active; detected canonical disagreement moves the Room to quarantined, while an intact Head with an unavailable/unsafe runtime moves it to faulted.

A corrupt newest snapshot can be skipped in favor of an older verified pair or Genesis. Deleting every snapshot and current materialization must still permit full recovery. Missing canonical Transitions or corrupt Genesis quarantine the Room; an unavailable exact Core or pack executor faults it until reinstalled and verified.

Room load, replay, and catch-up MUST NOT hold a long SQLite read transaction that pins the WAL. Each operation captures an immutable upper bound H, then pages append-only rows with short read transactions using sequence greater than the prior page and less than or equal to H. Page size, total duration, and concurrent replay count are bounded. Oldest-reader age and checkpoint blockage are metrics; expensive replay is throttled before it threatens mutation durability.

### Replay mode

Replay:

- is read-only;
- first authenticates and authorizes the present requester;
- reconstructs any retained Room sequence through the same Core and exact pack reducers;
- uses reconstructed Membership Standing, Access Mode, and Role at sequence N to construct participant/private history at N;
- gives no participant/private view to a Membership absent at N and never transfers historical access to a later Role or replacement Member ID;
- applies explicit present spectator/operator/final-reveal projection policy without bypassing pack privacy;
- verifies Genesis/Transition lineage plus Core, Activity, and aggregate hashes;
- does not deliver observation frames;
- does not offer activations;
- does not run clients, runners, models, timers, webhooks, or live evidence sources;
- exposes hash mismatch immediately.

Timeline forks and promotion are deferred until after v0.2.

### Room integrity and verifier repair

`RoomIntegrityState` is operational and never folded into Replay:

- healthy may advance;
- faulted means the last canonical Head verifies but the runtime cannot safely advance it;
- quarantined means canonical integrity cannot be established.

Every state change increments the integrity generation and appends an incident/repair record. Every canonical commit conditionally matches healthy plus the generation captured during preparation. Faulted/quarantined Rooms append no canonical participant or administrative Transition. Operational capability revocation, diagnostics, raw export, restore, and verification remain available.

An authenticated host operator may request repair but cannot clear integrity. The verifier may rebuild materializations/caches, reinstall the exact executor, or restore exact canonical bytes from a verified backup. Only a successful result conditioned on the current generation sets healthy, and it never edits, skips, reorders, synthesizes, or replaces Genesis/Transitions. A restored Room verifies healthy before archive or Membership mutation.

## Runtime filesystem

One configured data directory contains all durable runtime data:

    WORLDSTREAM_DATA_DIR/
    ├── db/
    │   ├── worldstream.sqlite3
    │   ├── worldstream.sqlite3-wal
    │   └── worldstream.sqlite3-shm
    ├── artifacts/                    v0.2
    │   └── blake3/
    │       └── aa/
    │           └── bb/
    │               └── full-digest
    └── tmp/

Rules:

- the directory defaults to an application-specific local path, never the home directory root;
- the service process owns it with directory mode 0700 and files mode 0600 where supported;
- the SQLite main, WAL, and shared-memory files stay together on one local persistent filesystem with working fsync;
- NFS, SMB, object-mounted filesystems, and an ephemeral container layer are unsupported;
- snapshots live in SQLite, not loose files;
- JSON logs go to stdout;
- configuration, TLS keys, bearer-token input, and model-provider credentials do not live in the data directory;
- temporary files are quota-limited and cleaned on startup after verifying they are not referenced.

### Container deployment

The Docker image runs as a non-root user. WORLDSTREAM_DATA_DIR must be a bind mount or persistent local volume. The container should bind loopback by default; public exposure requires an explicit listen address and a TLS reverse proxy.

### Backup

Do not copy only worldstream.sqlite3 while the service is live. WAL and shared-memory state are part of a running WAL database.

Supported backup flow:

1. request an online SQLite backup or VACUUM INTO through worldstreamctl;
2. acquire an artifact-GC/deletion lease for the duration of manifest capture and copy;
3. record the resulting database backup ID and exact digest manifest;
4. copy exactly those immutable content-addressed artifacts;
5. verify every referenced size and digest;
6. fsync the database backup, artifact files/tree, manifest, and destination directories before success;
7. store the configuration version and server build metadata alongside the backup.

Restore occurs into an empty validated data directory, then runs integrity and replay sampling before the server becomes ready.

## v0.2 artifact store

Investigation Room needs immutable source evidence, not a generic knowledge base.

Upload flow:

1. authorize the room and declared maximum size;
2. require tmp and artifacts to be on the same validated local filesystem, then stream bytes to a random file under tmp without using user filenames as paths;
3. enforce a ten-MiB per-blob limit and a default one-hundred-MiB room quota;
4. calculate BLAKE3 while writing;
5. if a verified CAS object already exists, discard the duplicate temp file; otherwise atomically install without replacing an existing object, then fsync the target parent directory;
6. in one small SQLite transaction, insert or verify the generic artifacts row and persist a durable, owner-scoped, expiring artifact_uploads record, then return its upload ID;
7. let a later typed room action reference that upload ID; its transition validates owner/expiry/digest, verifies the generic artifact row, inserts the room reference metadata, and marks the upload linked in one SQLite transaction;
8. delete an unreferenced duplicate temp file safely.

A crash before the staging transaction may leave a harmless CAS blob with no metadata. Expiry may leave an artifacts metadata row with no authoritative room reference. A reconciler may delete the metadata row and blob together only after a grace period when no room_artifact and no live staged upload references the digest. Because file and parent-directory durability precede both staging and authoritative linking, a committed room reference must never point to a directory entry lost on power failure.

The server does not execute, unpack, convert, embed, or summarize artifacts. The reference release supports UTF-8 text, JSON, CSV, and safe image download/preview; unknown formats download as attachments.

Evidence identity is its digest plus activity-level version metadata. A correction creates a new immutable artifact and an explicit supersedes relation. It never overwrites prior evidence.

Optional FTS5 indexing may be evaluated after Investigation works with structured evidence assignment and references. Any index is derived, authorization-filtered before query, and rebuildable. Embeddings are not part of the frozen releases.

## Security boundaries

Trusted:

- the host operator;
- the WorldStream binary;
- compiled-in Activity Packs.

Untrusted:

- network clients;
- human and agent actions;
- runner claims and outputs;
- evidence text and metadata;
- all text rendered by the UI.

Required controls:

- loopback bind by default;
- TLS termination for remote access;
- random 256-bit scoped bearer capabilities with only hashes persisted;
- authentication at session start and authorization for every operation;
- room, membership, and action scope checks in the Room Kernel before pack execution;
- strict schema and unknown-field rejection;
- per-IP, principal, session, membership, and room rate limits;
- bounded payloads, state, pack output, mailboxes, and queues;
- separate projection types and adversarial privacy tests;
- origin allowlist and no credentialed wildcard CORS;
- output escaping and safe artifact content disposition;
- no payload bodies, tokens, private projections, or artifact contents in default logs;
- no chain-of-thought collection;
- no arbitrary outbound network requests;
- no pack-supplied JavaScript.

The self-hosted preview does not include database encryption. Operators needing at-rest protection should use an encrypted local disk.

## Observability and operations

Endpoints:

- GET /healthz: process loop is alive;
- GET /readyz: migrations complete, database writable, scheduler running, and storage version supported;
- GET /metrics: Prometheus text exposition;
- GET /version: server, protocol, schema, Rust build, and SQLite versions.

Key metrics:

- active rooms and passivated rooms;
- sessions by Principal kind and Access Mode;
- room mailbox depth and busy rejections;
- accepted, rejected, duplicate, and conflicting actions;
- transition commit p50/p95/p99;
- observation frame bytes, backlog, redelivery, and slow-consumer closes;
- activation pending count, oldest age, lease expiry, and completion;
- timer lag;
- snapshot duration and recovery tail length;
- replay hash failures;
- database, WAL, temp, and artifact bytes;
- oldest SQLite reader age, replay/catch-up page duration, and checkpoint-blocked time.

worldstreamctl should eventually provide:

- create principal/capability;
- create, inspect, archive, and export room;
- verify replay and hashes;
- list pending activations and timers;
- trigger checkpoint and safe backup;
- run database integrity diagnostics.

Logs include correlation IDs, Room ID, Membership ID where authorized, Action ID, Transition sequence, result code, and latency. They do not include Action payloads or private Observations by default.

## Moderate single-node performance envelope

The initial scale target is deliberately ordinary:

Reference profile:

- Linux release build;
- 4 vCPU and 8 GiB RAM;
- local SSD or NVMe;
- SQLite WAL with synchronous FULL;
- small Heist/Investigation states and ten or fewer live participants per benchmark room.

Targets, not claims:

- 10,000 stored/passivated rooms;
- 100 simultaneously loaded rooms;
- 1,000 mostly idle WebSocket sessions;
- sustained 100 accepted transitions per second aggregate for 30 minutes;
- p95 local-network commit-to-acknowledgement below 100 ms;
- one-hour soak with bounded memory, mailboxes, output queues, WAL, and temp space;
- a 100,000-transition room with a snapshot no more than 250 transitions behind ready within five seconds;
- no acknowledged transition loss across repeated forced termination.

The benchmark report MUST disclose hardware, filesystem, SQLite version and pragmas, payload sizes, pack, participants per room, fan-out, snapshot cadence, p50/p95/p99 latency, process memory, database growth, and recovery time.

### Single-node optimizations allowed

1. Passivate idle room actors.
2. Prune acknowledged observation frames after the retention window.
3. Keep immutable artifacts outside SQLite.
4. Add dedicated read workers if profiling shows room load or replay blocks writes.
5. Tune indexes, snapshot cadence, and WAL checkpointing from metrics.
6. Use a very short storage group-commit window across independent rooms only if it preserves per-room ordering and commit-before-ack semantics.
7. Cache current authorized public projections without making the cache authoritative.

### Moderate scale beyond one node

This is a design seam, not a committed release:

1. First shard independent room-ID ranges across separately operated WorldStream nodes, each with its own local SQLite volume.
2. Route a room to one home node.
3. Move a room through explicit quiesce, export, verify, import, and route-update operations.
4. Keep cross-room transactions nonexistent.

Only measured pressure should justify a later shared-store design with Postgres, object storage, stateless gateways, and one fenced owner lease per room.

Even then:

- one room retains one writer;
- ordering remains per room;
- a broker, if added, carries routing notifications or derived projections, never authoritative mutation;
- there is no active-active room state, global transition order, CRDT merge, custom consensus, or multi-region write path.

### Revisit triggers

Evaluate a post-v0.2 storage RFC only when reproducible profiles show one or more:

- writer commit latency misses the target despite short transactions and correct indexes;
- more than 100 genuinely active rooms are needed on one host;
- the database or backup window becomes operationally unmanageable;
- a hosted deployment needs independent gateway and worker failure domains;
- explicit room sharding cannot meet the required operational experience.

Do not add Postgres, NATS, Redis, Kafka, Kubernetes, or Raft because they look scalable.

## Architecture invariants

1. One room has one pinned pack revision and one total committed order.
2. A committed sequence is never reused or decreased.
3. Immutable Genesis plus the exact Core/pack revisions and Transitions is sufficient after every paired snapshot and current materialization is deleted.
4. No accepted action is acknowledged or streamed before durable commit.
5. One room/member/action ID maps to one durable accepted or domain-rejected result; the same ID with a changed payload is an error.
6. Transient admission, capacity, storage, and runtime faults do not consume a new action ID.
7. A rejected action does not mutate canonical room history.
8. A participant action is admitted only against the exact current room head in the frozen releases.
9. A timer candidate mutates the pack only after the exact scheduled timer generation is conditionally claimed in the same transaction.
10. Core and Activity State plus all three state hashes and the lineage hash at sequence N are reproducible from Genesis and recorded Stimuli through N.
11. A pack cannot observe ambient nondeterminism.
12. Authoritative Room State never crosses the client boundary directly.
13. Authorization precedes observation persistence and artifact access.
14. Every durable viewer is a membership with exactly one addressed observation stream and one cursor.
15. Membership outlives sessions and invocations.
16. Activation is at-least-once intent delivery, not proof of model execution.
17. An expired or superseded activation claim cannot renew, release, or complete a later lease generation.
18. Observation delivery is at least once; clients deduplicate and acknowledge.
19. The catch-up/live actor barrier returns the complete retained authorized range or an explicit projection reset without a handoff gap.
20. A snapshot is a paired postcommit Core-and-Activity cache; snapshots, current materializations, indexes, and projection caches are replaceable derivations.
21. Every canonical commit fences on healthy plus an unchanged integrity generation; canonical disagreement quarantines, while an intact Head that cannot safely advance faults.
22. Replay has no external effects, applies present-plus-historical authorization, and holds no unbounded SQLite read transaction.
23. Mutating HTTP resources and their idempotency receipts commit atomically.
24. A committed artifact reference points only to bytes made durable before the linking transaction.
25. Passivation is generation-fenced; no command is routed to an actor that may disappear.
26. Slow clients and full queues cannot create unbounded memory growth.
27. Investigation-specific semantics stay outside the Room Kernel; only the preplanned generic artifact subsystem is added.
28. v0.1 and v0.2 remain single-node developer-preview deployments.
29. Core Room State is exactly Room Status plus the semantic Membership map; Room Integrity State is operational.
30. The Core reducer alone mutates Core; a pack may veto only join, resume, Access Mode, and Role proposals.
31. Multi-Membership administration validates and commits one final state without an observable invalid intermediate.
32. Departed Membership and archived Room status are irreversible.
33. Only a generation-fenced verifier may restore healthy integrity.
34. Repair never rewrites, skips, or replaces canonical lineage.
