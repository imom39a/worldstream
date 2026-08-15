# System Architecture

## Document status

This document implements the [Frozen Requirements](requirements.md). It is normative for v0.1 and v0.2 where it defines an invariant or a frozen technology decision.

The intended system is exactly one self-hosted Rust process with strong internal module boundaries and one startup-selected durable storage profile. The default bundled SQLite files are local; the optional hosted or self-managed PostgreSQL 17 primary may be remote without imposing a same-host process limit. Those boundaries do not authorize multiple WorldStream processes or microservices.

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
    ST --> DB["Bundled SQLite or PostgreSQL 17 primary"]
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
8. Recovery and Replay reproduce recorded Activity State hashes and Core Room State.
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
| Storage profiles | Release-bundled SQLite default; optional `postgres-primary` on PostgreSQL 17 | Zero-dependency local default and provider-neutral remote/self-managed primary under identical semantics |
| Identifiers | ULID strings | Readable, sortable external identifiers; room sequence remains authoritative |
| Hashing | BLAKE3 | State, transition, payload, and artifact integrity |
| Time | time crate and RFC 3339 UTC at boundaries | Explicit audit timestamps; pack semantic time remains recorded |
| CLI/config | Clap plus versioned TOML and environment overrides | One server binary and predictable local operation |
| Errors | thiserror in libraries; anyhow only at binary boundary | Typed protocol/storage errors without application boilerplate |
| Telemetry | Structured JSON logs, Prometheus text metrics, W3C trace correlation, optional OpenTelemetry/OTLP export seam | Vendor-neutral diagnostics outside correctness paths |
| Python SDK | Python 3.11–3.14, websockets, Pydantic | Fastest path for external agent runners and typed examples |
| Web UI | React, TypeScript, Vite, native browser WebSocket | Small first-party reference UI; no realtime framework dependency |
| Packaging | Native Linux x86-64, native Windows x64, Linux/amd64 OCI, macOS source quickstart | Explicitly tested release and development profiles |

The authored v0.1.0 compatibility specification pins Rust 1.97.1 edition 2024, Node 24.18.1 LTS for builds only, Python SDK 3.11–3.14, and Python 3.14.7 for the quickstart. It selects SQLite 3.53.4, recognizes 3.51.3 as the frozen corrective floor, and denies 3.52.0; WorldStream never loads host SQLite. It accepts PostgreSQL major 17 from 17.11 and defines newer-17.x versus other-major policy. It is not a release-valid manifest until exact bundled-build identity, verified PostgreSQL patches, and every required evidence field are populated.

Not selected for v0.1 or v0.2: an ORM, Redis, NATS, Kafka, Temporal, Wasmtime, Kubernetes, a provider database API as a correctness dependency, an embedded model SDK, or a frontend realtime platform.

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
    │   ├── worldstream-postgres/
    │   │   └── PostgreSQL 17 migrations, storage port and verification operations
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
    ├── compatibility.toml
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

The storage interface belongs in worldstream-core; bundled SQLite and `postgres-primary` are its only frozen implementations. There are no provider, broker, crypto, workflow, plugin, or generic connector crates.

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

- A room loads lazily from verified genesis or its newest valid snapshot plus transition tail.
- Concurrent load requests coalesce into one load.
- Each active room has one bounded mailbox.
- An idle room may passivate after five minutes if it has no connected sessions, mailbox work, pending activation lease operation, or near-due timer.
- A failed room actor is discarded and reloaded from durable state.
- A room that fails hash verification is quarantined read-only.

The active-room map is in memory. It is not a distributed registry.

The supervisor uses an explicit per-room lifecycle:

    Loading → Active → Passivating → Inactive
                    ↘ Faulted or Quarantined

Every actor receives a supervisor generation. Passivation occurs through a barrier: mark Passivating, stop routing directly, drain the actor mailbox, confirm no provisional commit/timer work, then remove the actor. Commands arriving during Loading or Passivating wait in a bounded supervisor queue or receive room_busy; they are never sent to a channel whose actor can exit. A stale-generation actor cannot publish after removal, and its expected-head check prevents an obsolete commit.

### Single-writer room actor

Every active room has exactly one logical writer task. Actions in different rooms may run concurrently. Accepted stimuli inside one room are deliberately serialized.

The actor owns:

- current Canonical Activity State and a verified view of Core Room State;
- head room sequence and hashes;
- current Membership index used for authorization and Viewer construction;
- provisional timer view;
- calls into the pinned Activity Pack;
- preparation of one atomic storage commit;
- publishing only committed frames.

Core room status, room health, and pack phase are separate:

- status: active or archived;
- health: healthy, faulted, or quarantined;
- Activity Phase: arbitrary pack-defined stage;
- Outcome: a separate pack-defined result, which may be established before the Terminal Phase.

Archived, faulted, and quarantined rooms reject normal participant mutation. They remain available for authorized inspection, export, repair tooling, and read-only replay. A terminal pack phase normally rejects ordinary domain actions through pack rules while the core room may remain active until explicitly archived.

The actor MUST use a bounded mailbox. Backpressure reaches the gateway as a typed busy response; it does not create more actor tasks for the same room.

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
    ActionRejected(ActionRejection),
    ActivityFault(ActivityFault),
}
~~~

AppliedTransition contains:

- the complete next Canonical Activity State;
- ordered domain events for audit and UI;
- timer schedule/cancel operations;
- deterministic attention signals.

Validation and reduction occur in one apply call so they cannot disagree after state changes. ActionRejected applies only to ParticipantAction; an invalid host-originated Stimulus is an ActivityFault. Project returns the pack-owned Activity Projection, and Observe returns a bounded pack-owned Activity Observation for one viewer. WorldStream wraps Activity Projection with authorized Core Room and Membership facts to form a Projection; protocol envelopes add causal sequence, Room Health, schema, and delivery metadata.

The host supplies no database, network, filesystem, environment, model, wallet, or wall-clock handle. Same-process Rust is not a sandbox: compiled-in packs are fully trusted by the host operator. Public or untrusted pack loading is explicitly unsupported.

### Storage service

Room actors submit typed commit batches over one bounded backend-neutral storage lane. The SQLite profile uses one dedicated writer thread and connection. The PostgreSQL profile may execute independent Room transactions concurrently through direct, session-pooled, or bounded transaction-scoped connections; it has no process-count limitation inside the one WorldStream process. A per-Room root fence and conditional predicates preserve the same one-logical-writer semantics under PostgreSQL Read Committed.

Storage adapters MAY use separate bounded read connections for room loading, catch-up, Replay, and host-operator queries. Reads that determine whether a write is valid are repeated and fenced inside the write transaction. No correctness path depends on connection affinity, session state, named prepared statements, extensions, replica reads, or a provider API.

The storage service owns:

- forward-only migrations;
- atomic transition commits;
- action-receipt lookup;
- snapshot and transition reads;
- observation-frame and activation queries;
- cursor acknowledgement persistence;
- backend-native maintenance metrics and controls;
- backup, restore, and integrity-verification operations.

The backend is selected at startup and remains fixed until shutdown. Loss of PostgreSQL makes readiness unhealthy and mutations fail closed; WorldStream never falls back to SQLite or authoritative in-memory buffering. Both adapters implement the same transaction/result algebra, canonical byte handling, error classification, receipt lookup, migration fingerprint, recovery, and Replay contracts. PostgreSQL remote connections require TLS, the daemon uses a least-privilege DML role, and `synchronous_commit=on` is mandatory.

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
- State hash covers pack digest plus Canonical Activity State.
- Transition hash covers room ID, sequence, stimulus, ordered events, prior transition hash, and resulting state hash.
- Genesis hash covers the exact pack digest, configuration, ordered initial memberships/roles, room seed, and logical creation time.

Every hash input is one canonical typed object containing an explicit domain/version field, such as worldstream/transition/v1. Raw concatenation of variable-length fields is forbidden. Golden vectors define integer, byte/digest, and string encoding.

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
    participant S as "Selected storage profile"

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
| Before database COMMIT | Nothing authoritative happened; the client may retry |
| After commit but before action reply | Retry returns the stored original receipt |
| After commit but before frame send | Cursor catch-up returns the committed frame |
| After activation creation but before offer | The pending intent remains queryable |
| After activation lease but before runner result | Lease expires and the same intent may be claimed again |
| Snapshot write failure | The whole transition transaction fails if snapshot was part of it, or recovery uses the prior snapshot |
| Process termination with due timers | Startup scan resubmits idempotent TimerFired candidates |
| Projection construction failure | No transition is committed |
| Pack panic | The room actor fails; the room reloads or is quarantined after repeated deterministic failure |
| Capacity, database, or storage I/O error | Mutation fails closed; readiness reports unhealthy |

## Logical durable data model

The concrete SQLite and PostgreSQL migrations may use backend-specific DDL and add operational columns, but the following logical entities and constraints are frozen. The sketches use SQLite type spelling only for brevity; persisted canonical objects remain identical bytes under both profiles.

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
    health TEXT CHECK health IN ('healthy', 'faulted', 'quarantined')
    genesis_hash BLOB
    room_seed BLOB
    head_seq INTEGER
    head_transition_hash BLOB
    head_state_hash BLOB
    created_at TEXT
    updated_at TEXT

### room_genesis

The immutable source for recovery when every snapshot is absent:

    room_id TEXT PRIMARY KEY
    pack_digest BLOB
    configuration_json BLOB
    initial_memberships_json BLOB
    room_seed BLOB
    logical_created_at TEXT
    genesis_hash BLOB
    initial_state_hash BLOB

Room sequence zero names genesis. The first later accepted stimulus is sequence one. The previous transition hash for sequence one is the domain-separated genesis hash.

### room_members

    room_id TEXT
    member_id TEXT
    principal_id TEXT
    role TEXT NULL
    access_mode TEXT CHECK access_mode IN ('participant', 'spectator', 'operator')
    status TEXT CHECK status IN ('enabled', 'suspended', 'departed')
    joined_seq INTEGER
    frame_head INTEGER
    last_ack_frame_seq INTEGER
    activation_policy_json BLOB
    created_at TEXT
    updated_at TEXT
    PRIMARY KEY (room_id, member_id)

Connection state and model invocation state MUST NOT be columns in this table.

`role` is required when `access_mode = 'participant'` and absent for spectator/operator Memberships. The Activity Pack defines allowed Roles and cardinality; this table owns each current assignment.

### transitions

    room_id TEXT
    seq INTEGER
    transition_id TEXT UNIQUE
    pack_digest BLOB
    initiator_member_id TEXT NULL
    action_id TEXT NULL
    stimulus_kind TEXT
    stimulus_json BLOB
    domain_events_json BLOB
    logical_time TEXT
    previous_transition_hash BLOB
    resulting_state_hash BLOB
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
    pack_digest BLOB
    encoding TEXT
    state_blob BLOB
    state_hash BLOB
    created_at TEXT
    PRIMARY KEY (room_id, seq)

The state hash covers a domain-separated canonical object containing the pack digest and uncompressed Canonical Activity State bytes. Compression is a replaceable storage detail.

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

The exact release-bundled SQLite source identity/build and all pragmas are recorded in the compatibility manifest. The main, WAL, and shared-memory files remain on one validated local filesystem; system SQLite, network/UNC filesystems, shared writers, and SQLite on a writable container overlay are rejected. The writer controls checkpoints and reports WAL size and checkpoint latency. Large write transactions are avoided.

### Required PostgreSQL 17 behavior

`postgres-primary` connects to one ordinary writable PostgreSQL 17 primary, hosted or self-managed. Runtime transactions use Read Committed, set `synchronous_commit=on`, lock or compare-and-set the Room root and every operation-specific fence, and decide all authoritative preconditions before COMMIT. The runtime role has only required DML/sequence permissions. Remote connections require TLS.

Direct, session-pooled, and bounded transaction-scoped runtime connections are supported. A transaction pooler may select a different connection for every transaction. Named prepared statements, persistent temporary objects, session variables, advisory locks whose meaning outlives one transaction, extensions, replicas, provider APIs, and provider-specific error or failover behavior are not correctness dependencies. Migration, transfer, native dump/restore, and full verification use a direct admin connection outside the daemon.

### Forward-only logical migrations and retained codecs

One ordered logical migration history and schema-contract fingerprint govern both adapters. A logical migration has a stable ID and checksum plus backend-specific DDL/execution; it upgrades an empty store or any earlier v0.1 schema, and is atomic or explicitly restart-safe. Production is forward-only: there are no down migrations, mixed-version serving, rolling multi-version operation, or old-binary start after migration. Rollback restores the pre-upgrade backend backup and previous binary together.

SQLite automatic migration occurs only during exclusive locked startup after creation and verification of a recoverable backup. Production PostgreSQL migration is an explicit offline `worldstreamctl` maintenance operation over a direct admin connection while no WorldStream process serves; daemon startup only checks engine, manifest, schema fingerprint, migration checksums, and runtime capabilities. A development auto-migration option is not production evidence.

Each release retains readers for every canonical and receipt codec that can occur in supported v0.1 data, transfer bundles, and backups. Writers emit only the current manifest-declared versions. Engine-native types never decode and re-encode canonical JSON or receipt bytes during migration, transfer, backup verification, or ordinary persistence.

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

For an accepted stimulus, one storage transaction MUST perform the following ordered logical work. SQLite maps it to `BEGIN IMMEDIATE`; PostgreSQL maps it to one Read Committed transaction with the Room root and operation fences locked or conditionally updated:

1. Re-read the room head and every stimulus-specific durable precondition under the write lock.
2. For a participant action, require based_on_room_seq to equal head exactly and verify the action receipt/payload hash.
3. For a TimerFired candidate, conditionally change the exact room/timer/generation row from scheduled to fired and require one affected row. Zero rows means a duplicate or stale candidate: no pack mutation and no transition.
4. For membership or administration input, conditionally validate/update the relevant durable status and reject a stale generation.
5. Insert the next transition.
6. Update room head sequence and hashes.
7. Apply remaining membership metadata changes, if any.
8. Insert or cancel timers.
9. Insert Membership-addressed Observation Frames and advance each affected Membership's `frame_head`.
10. Insert activation intents under their unique logical keys.
11. Insert a snapshot when the interval is reached.
12. Insert the accepted action receipt.
13. Commit.

For a deterministic admitted domain rejection, a smaller transaction records a domain_rejected action receipt without changing room head sequence. Authentication/authorization failure, malformed input, rate limit, room busy, storage unavailable, and activity/runtime faults use the transient error path, create no action receipt, and allow retry with the same action ID.

Mutating HTTP administration uses mutation_receipts in the same transaction as its resource change.

Room archival and domain-relevant Membership, Access Mode, or Role changes are Administrative or MembershipChanged Stimuli and consume the next Room sequence. Their mutation receipt, core metadata change, pack reflection, Domain Events, and Observation Frames commit with that Transition. Session presence, Runner availability, Activation lease operations, diagnostics, and telemetry remain operational and never consume Room sequence, following [ADR 0002](adr/0002-sequence-domain-relevant-room-changes.md).

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

- snapshot every 250 accepted transitions or five active minutes, whichever occurs first;
- retain the immutable room_genesis record and the latest three automatic snapshots; an optional sequence-zero snapshot is only a cache;
- keep canonical transitions and action receipts for the room lifetime in frozen releases;
- prune acknowledged observation frames only after a seven-day safety window or a configurable per-member high-water mark;
- never use snapshot deletion to change canonical history.

### Room load

1. Read and verify the immutable canonical genesis record, genesis hash, pinned Activity Pack revision, and room head.
2. Find the newest compatible snapshot at or before head.
3. If a valid snapshot exists, verify its Canonical Activity State bytes and state hash; otherwise call initialize from the recorded genesis input.
4. Replay every later transition through the exact pack revision.
5. Verify genesis/previous-hash, transition-hash, and state-hash chains.
6. Reconstruct timer and membership metadata consistency.
7. Compare the resulting Activity State hash with the Room head and verify reconstructed Core Room State against the ordered transitions.
8. Mark the room active, or quarantine it read-only on mismatch.

A corrupt newest snapshot can be skipped in favor of an older verified snapshot or genesis. Deleting every snapshot must still permit full recovery. Missing canonical transitions, corrupt genesis, or an unavailable exact pack revision are fatal.

Room load, Replay, and catch-up MUST NOT hold a long database snapshot or read transaction. Each operation captures an immutable upper bound H, then pages append-only rows with short read transactions using sequence greater than the prior page and less than or equal to H. Page size, total duration, and concurrent Replay count are bounded. SQLite reports oldest-reader age and checkpoint blockage; PostgreSQL reports equivalent pool/snapshot pressure without changing behavior. Expensive Replay is throttled before it threatens mutation durability.

### Replay mode

Replay:

- is read-only;
- reconstructs any retained room sequence;
- produces public or authorized historical projections;
- does not deliver observation frames;
- does not offer activations;
- does not run clients, runners, models, timers, webhooks, or live evidence sources;
- exposes hash mismatch immediately.

Timeline forks and promotion are deferred until after v0.2.

## Runtime filesystem

One configured data directory contains WorldStream-owned runtime data. Under the SQLite profile it also contains the database; under `postgres-primary`, database files remain owned by PostgreSQL while the artifact tree and local operational files remain here:

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
- POSIX installs use owner-only umask 077, directory mode 0700, and files mode 0600; Windows grants only the service identity/owner, SYSTEM, and administrators and rejects broadly writable DACLs;
- the SQLite main, WAL, and shared-memory files stay together on one local persistent ext4, XFS, development APFS, fixed NTFS, or ReFS filesystem with working durable create/fsync/rename;
- NFS, SMB, object/FUSE mounts, network homes, UNC paths, FAT/exFAT, shared volumes, symlink/reparse-point databases, and an ephemeral container layer are unsupported for SQLite;
- snapshots live in the selected database, not loose files;
- JSON logs go to stdout;
- plaintext DSNs, configuration secrets, TLS keys, bearer-token input, exporter credentials, and model-provider credentials do not live in the data directory;
- temporary files are quota-limited and cleaned on startup after verifying they are not referenced.

Startup validates path type, ACL/permissions, exclusive SQLite lock when selected, filesystem class, free space, and durable create/fsync/rename behavior before mutation. The process stops accepting mutation when required durable capacity is unavailable.

### Container deployment

The OCI release is Linux/amd64 only, static/minimal, runs as non-root UID 65532, and supports a read-only root. `WORLDSTREAM_DATA_DIR` is `/var/lib/worldstream` and MUST be a bind mount or persistent local volume; SQLite on the writable container overlay is rejected. The container binds loopback by default; public exposure requires an explicit listen address and TLS termination. There is no Windows container release.

### Backend-native backup and restore

Do not copy only `worldstream.sqlite3` while the service is live. WAL and shared-memory state are part of a running WAL database. WorldStream owns the SQLite online backup/empty-directory restore flow. For PostgreSQL, the host operator or provider owns native snapshot, PITR, dump, and isolated restore using a direct admin connection; WorldStream does not replace, promise, or automate provider HA/durability services.

Every backend-native backup receives an immutable backup ID and a WorldStream manifest. The consistent backup boundary also covers generic artifact metadata and bytes:

1. create the SQLite online backup or a PostgreSQL-native backup at a documented consistent point;
2. acquire an artifact-GC/deletion lease for the duration of manifest capture and copy;
3. record backup ID, Storage Epoch, engine identity, schema/migration/codec/pack-executor versions, and an exact digest manifest;
4. copy exactly those immutable content-addressed artifacts;
5. verify every referenced size and digest;
6. durably finalize backend-native data, artifact files/tree, manifest, and destination metadata before success;
7. restore into an empty isolated target, then run backend integrity checks and the full WorldStream semantic verifier before readiness.

### Full WorldStream semantic verifier

The verifier is read-only and never samples, repairs, fires timers, delivers Frames, or starts Activations. It validates the backup/export ID, Storage Epoch and lineage, engine and compatibility manifest, schema fingerprint and migration checksums, artifact sizes/digests/bytes, operation identities/hashes/dispositions/receipts, exact timers, Frames/Cursors, Activation intents/claims/fences, and availability of every exact retained Activity Pack executor. For every Room recorded healthy it reconstructs Genesis through Head and verifies every transition, state, Core/Activity materialization, and Head hash.

A global lineage, schema, manifest, artifact, or cross-Room authority failure blocks deployment readiness. A Room already recorded as faulted or quarantined may be copied byte-for-byte, remain isolated and unhealthy, and not block otherwise verified Rooms. Any mismatch newly introduced by backup, restore, or transfer aborts verification.

### One-way offline SQLite-to-PostgreSQL transfer

The only supported backend transfer is a versioned, resumable, whole-deployment, offline move from authoritative SQLite to an empty PostgreSQL target. The deterministic transfer bundle records source lineage, export identity, Storage Epoch, schema and codec versions, ordered chunks, row/object counts, per-chunk and whole-export digests, and a semantic fingerprint.

Canonical serialized bytes are copied verbatim, never decoded and re-encoded through PostgreSQL JSON, timestamp, numeric, or text types. The bundle preserves Genesis, Transitions, every Head/hash, Core and Activity materializations, Memberships and authority, exact timer IDs/generations/due values, Frames/Cursors, operation identities/payload hashes/dispositions/receipts, Activation intents/claims/audit/fences, principals/capabilities/revocations, integrity incidents, and artifact metadata and bytes. Snapshots, indexes, caches, telemetry, sessions, runner presence, in-memory mailboxes, delivery attempts, and temporary state are invalidated or rebuilt.

Transfer is two-phase:

1. quiesce the sole WorldStream process, create and verify a recoverable SQLite backup, and durably mark the source `transfer_pending` under its current Storage Epoch;
2. export/import resumable deterministic chunks into the empty target while neither backend serves, fence every nonterminal Activation lease, run PostgreSQL-native checks and the full WorldStream verifier, then require an explicit finalize command to retire SQLite and make PostgreSQL authoritative at the next Storage Epoch.

Recorded scheduled timers keep their IDs, generations, and due values. Startup enters the ordinary fixed-cutoff CatchingUp path and invents neither a fire nor a generation during transfer. Eligible Activation intents remain reclaimable after old leases are fenced.

Before source retirement, abort discards the target and leaves the verified SQLite source authoritative. After PostgreSQL accepts its first write under the new epoch, returning to SQLite is not supported continuity. The retired SQLite deployment is a read-only recovery artifact; reopening it for writes requires an explicit destructive override. There is no live switch, dual write, reverse or room-at-a-time transfer, automatic fallback, or consensus protocol: the offline authority fence is sufficient only because serving is quiesced.

Transfer and recovery race outcomes are frozen independently of adapter implementation:

| Race or failure | Required outcome |
|---|---|
| A server starts against SQLite marked `transfer_pending` | It fails closed and does not serve; only resume or abort may clear the fence. |
| Export/import stops mid-chunk | No target is ready. Resume verifies the export identity and committed chunk digest/count before continuing or safely reapplying that chunk. |
| Source data changes after export identity is captured | The exclusive process/storage fence prevents mutation; any lineage, count, or digest change invalidates the bundle. |
| Finalize is repeated or races an old invocation | Finalize conditionally matches source epoch, export identity, verified target fingerprint, and next epoch. The identical completed finalize returns its stored disposition; different evidence fails. |
| Process termination interrupts finalize | Source remains `transfer_pending` or retired and target remains non-serving until resume proves the target finalization record and source retirement record agree. Neither backend may accept an ambiguous first write. |
| Abort races finalize | Abort is legal only before a target finalization record; otherwise finalize/resume wins and SQLite cannot return as authoritative continuity. |
| A timer becomes due while serving is quiesced | Its exact ID, generation, and due value transfer unchanged; the target's fixed-cutoff CatchingUp scan decides candidates after readiness preparation. |
| An Activation lease is nonterminal at export | Its audit evidence transfers, but the old generation is fenced before target readiness; an eligible intent can be reclaimed under the preserved identity. |
| PostgreSQL disappears or a provider promotes/replaces a primary | Readiness and mutation fail closed until the configured supported primary and full contract are verified; WorldStream performs no automatic fallback/failover. |
| Restore/transfer finds a new mismatch | The whole target stays non-serving. An exactly preserved pre-existing faulted/quarantined Room is the only per-Room isolation exception. |

## v0.2 artifact store

Investigation Room needs immutable source evidence, not a generic knowledge base.

Upload flow:

1. authorize the room and declared maximum size;
2. require tmp and artifacts to be on the same validated local filesystem, then stream bytes to a random file under tmp without using user filenames as paths;
3. enforce a ten-MiB per-blob limit and a default one-hundred-MiB room quota;
4. calculate BLAKE3 while writing;
5. if a verified CAS object already exists, discard the duplicate temp file; otherwise atomically install without replacing an existing object, then fsync the target parent directory;
6. in one small selected-backend transaction, insert or verify the generic artifacts row and persist a durable, owner-scoped, expiring artifact_uploads record, then return its upload ID;
7. let a later typed room action reference that upload ID; its transition validates owner/expiry/digest, verifies the generic artifact row, inserts the room reference metadata, and marks the upload linked in one selected-backend transaction;
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

The self-hosted preview does not include application-layer database encryption. SQLite operators use encrypted local disks; PostgreSQL encryption, keys, and transport-at-rest facilities remain operator/provider concerns.

## Configuration and fail-closed startup

Configuration precedence is deterministic: compiled defaults, one explicitly selected versioned TOML file, `WORLDSTREAM__SECTION__KEY` environment variables, then documented CLI flags. `--config` overrides `WORLDSTREAM_CONFIG`; the server never searches the current directory or home directory. Unknown or duplicate keys, wrong types, out-of-range values, values for an inactive backend, and unsupported compatibility values are fatal. `worldstreamctl config validate`, redacted `config effective`, and `doctor` expose the same parser plus filesystem/ACL, backend, migration, free-space, and manifest diagnostics.

DSNs, bearer capabilities, and exporter credentials enter only through owner-readable secret files or inherited handles. They never appear in plaintext TOML, command arguments, effective-config output, logs, traces, crash reports, or metrics. A PostgreSQL admin/migration DSN is accepted only by an offline maintenance command and is never available to the daemon.

Startup runs in this order and fails closed with stable exit classes for config, platform/filesystem/lock, storage/version/migration, integrity/Replay, and listener failures:

1. validate config and the embedded/published compatibility manifest;
2. initialize local structured logging without a remote dependency;
3. validate data-path ACL, lock, filesystem, capacity, and durable create/fsync/rename behavior;
4. open the selected backend and verify engine identity, connection mode, runtime-role capabilities, schema fingerprint, migration checksums, global integrity, foreign keys where applicable, Room Heads/snapshots, exact pack executors, and Storage Epoch;
5. start the bounded storage writer/lane, scheduler, room supervisor, listener, and only then readiness.

SQLite startup may create/verify a backup and migrate while exclusively locked. PostgreSQL daemon startup verifies only; migration, transfer, dump/restore orchestration, and the full semantic verifier are offline direct-admin commands.

## Observability and operations

Endpoints:

- `GET /healthz`: event-loop/process liveness only; it deliberately does not query storage;
- `GET /readyz`: current compatible schema, globally healthy authoritative storage with writable capacity, and running storage writer/lane and scheduler;
- `GET /metrics`: low-cardinality Prometheus text exposition;
- `GET /version`: product/build, wire/config/storage/Core/hash/manifest versions and exact SQLite or PostgreSQL engine identity.

An already isolated unhealthy Room and telemetry/exporter failure do not fail readiness. A global storage, lineage, manifest, schema, or integrity failure does.

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
- database/pool, WAL where applicable, temp, capacity, and artifact bytes;
- oldest SQLite reader age or PostgreSQL snapshot/pool pressure, Replay/catch-up page duration, and checkpoint-blocked time where applicable;
- telemetry dropped events and rate-limited overflow warnings.

worldstreamctl should eventually provide:

- create principal/capability;
- create, inspect, archive, and export room;
- verify replay and hashes;
- list pending activations and timers;
- trigger backend-native backup/restore workflows and SQLite checkpoint;
- migrate PostgreSQL offline, transfer SQLite to PostgreSQL, and run backend plus full semantic verification.

Logs are structured JSON with W3C trace correlation. They may include Room, Membership, Action, and Transition IDs only in access-controlled logs/traces; those IDs are never metric labels. Logs exclude Action/Observation bodies, capabilities, private observations, prompts/model output, artifact bytes, DSNs, and credentials. Prometheus metrics and the optional OpenTelemetry/OTLP exporter are vendor-neutral seams; no vendor SDK, account, collector, or credential participates in admission, reduction, commit, Replay, Room Health, or readiness.

Telemetry happens after authoritative commit through bounded nonblocking queues while holding no Room or database lock. Overflow drops telemetry, increments a metric, and emits a rate-limited warning. Shutdown grants at most three seconds to flush and never delays or changes an authoritative outcome.

## Release, platform, and evidence matrix

This section implements [ADR 0004](adr/0004-supported-storage-profiles-and-offline-portability.md) and [ADR 0011](adr/0011-release-compatibility-recovery-and-supply-chain-gate.md).

Reviewed [`compatibility.toml`](../compatibility.toml) is the authored source and canonical [`compatibility.json`](../compatibility.json) is its semantically identical, sorted-key mirror. The checked-in pair deliberately describes a specification with `release_ready = false`: migration/schema checksums, exact pack-executor and build/artifact digests, and all execution evidence remain unresolved because no implementation or release exists. Validation fails closed on any unresolved required field.

A real release must generate and review a populated pair, prove semantic parity, set `release_ready = true` only after every hard gate passes, embed the JSON in every binary, and publish it with the release. Startup, `doctor`, `/version`, backups, transfer bundles, release notes, and CI consume that release-valid manifest. The v0.1.0 specification identifies product `0.1.0`, wire `0.1`, config `1`, storage schema `1`, Core semantics `1`, and hash suite `blake3-canonical-json-v1` in addition to the engine/toolchain versions frozen above.

| Profile | Supported/release contract | Mandatory evidence |
|---|---|---|
| Native Linux | `x86_64-unknown-linux-musl`, kernel 5.15+, local ext4/XFS for SQLite; Ubuntu 24.04 x86-64/ext4 reference | Native build/archive, both storage profiles, migration/transfer/restore/Replay, filesystem/ACL/disk-full and performance reference |
| Native Windows | `x86_64-pc-windows-msvc`; Windows 11 25H2+ or Server 2022/2025; fixed NTFS/ReFS | Native build/package plus ACL/filesystem, bundled SQLite, PostgreSQL connection, backup/restore and recovery tests; cross-compilation alone fails the gate |
| OCI | Linux/amd64 only, static/minimal, non-root UID 65532, read-only-root compatible, persistent `/var/lib/worldstream` | Image-by-digest test, both storage profiles, persistent-volume enforcement, no writable-overlay SQLite |
| macOS quickstart | Source build on macOS 15+ APFS, Intel and Apple Silicon, development/default quickstart | Fresh source-build deterministic Heist quickstart; no binary/archive gate |

Each native archive contains `worldstreamd`, `worldstreamctl`, embedded UI, examples/Heist clients, licenses, and compatibility manifest. Published evidence includes source, checksums, Sigstore signatures, SPDX SBOM, and SLSA provenance. ARM64 release artifacts, macOS binary distribution, Windows containers, MSI/MSIX, Windows Service integration, package repositories, Kubernetes/Helm assets, cloud resources, and release-pipeline implementation are not delivered.

| Evidence tier | Bound and required scope |
|---|---|
| Fast hook | Warm p95 at most 20 seconds and hard 60 seconds; changed format/config/schema/golden/secret checks; no network or container dependency |
| Pre-push | Warm target 6 minutes, cold target 15 minutes; lint/unit, SQLite, SDK/UI/protocol/hash plus local PostgreSQL smoke; a visible local skip is incomplete and remote CI may not skip |
| Minimal CI | Target 15 minutes, hard 25 minutes; parallel Linux plus focused native Windows build/package/ACL/filesystem/SQLite/PostgreSQL-connect/recovery |
| Release | Target 3 hours, hard 4 hours; every artifact/platform, signature/SBOM/provenance, full backend conformance, all prior migrations, transfer, isolated restore, verifier, failure/fuzz/benchmark suites, and one-hour SQLite soak |

The deterministic quickstart uses SQLite by default and requires no cloud account, paid model, or remote service; PostgreSQL is opt-in. The certified Ubuntu reference uses 4 vCPU, 8 GiB, local SSD and completes deterministic Heist in under five minutes; a fresh checkout completes in under ten. The PostgreSQL harness uses the official PostgreSQL 17.11 image pinned by digest, a unique project and disposable volume, loopback random port, SCRAM, non-superuser runtime role, separate direct-admin and transaction-pooler runtime DSNs, PgBouncer transaction pooling, and scoped teardown. Provider verification is optional, dated, and limited to a migration plus backup/isolated-restore/full-Replay drill for the exact combination; it asserts no HA, SLA, durability, plan, region, or provider service.

## Reference performance envelope

The initial scale target is deliberately ordinary:

Reference profile:

- Linux release build;
- 4 vCPU and 8 GiB RAM;
- local SSD or NVMe;
- the exact bundled SQLite profile; PostgreSQL is measured and published separately;
- small Heist/Investigation states and ten or fewer live participants per benchmark room.

Targets, not claims:

- 10,000 stored/passivated rooms;
- 100 simultaneously loaded rooms;
- 1,000 mostly idle WebSocket sessions;
- sustained 100 accepted transitions per second aggregate for 30 minutes;
- p95 local-network commit-to-acknowledgement below 100 ms;
- one-hour SQLite soak with bounded memory, mailboxes, output queues, WAL, and temp space;
- a 100,000-transition room with a snapshot no more than 250 transitions behind ready within five seconds;
- no acknowledged transition loss across repeated forced termination.

The benchmark report MUST disclose hardware, filesystem, selected profile, exact engine version/settings/connection mode, payload sizes, pack, participants per room, fan-out, snapshot cadence, p50/p95/p99 latency, process memory, database growth, and recovery time. Performance targets are reference measurements, not universal release blockers, SLAs, or Windows performance claims; correctness, durability, crash recovery, resource bounds, and hash parity remain hard gates on every supported profile.

### One-process optimizations allowed

1. Passivate idle room actors.
2. Prune acknowledged observation frames after the retention window.
3. Keep immutable artifacts outside the selected database.
4. Add dedicated read workers if profiling shows room load or replay blocks writes.
5. Tune indexes, snapshot cadence, pool bounds, and SQLite WAL checkpointing from metrics.
6. Use a very short storage group-commit window across independent rooms only if it preserves per-room ordering and commit-before-ack semantics.
7. Cache current authorized public projections without making the cache authoritative.

### Excluded distributed deployment

PostgreSQL support changes the storage location and concurrency implementation, not the one-process product boundary. The frozen releases have no second live WorldStream process, stateless gateway/worker split, authoritative replica read, room sharding/move protocol, automatic failover, provider HA integration, consensus, active-active state, global transition order, CRDT merge, broker-backed authority, or multi-region mutation. Any later distributed design requires post-v0.2 evidence and a separate ADR; Redis, NATS, Kafka, Kubernetes, and Raft are not implied by the supported PostgreSQL profile.

## Architecture invariants

1. One room has one pinned pack revision and one total committed order.
2. A committed sequence is never reused or decreased.
3. Immutable genesis plus the exact pack digest and transitions is sufficient after every snapshot is deleted.
4. No accepted action is acknowledged or streamed before durable commit.
5. One room/member/action ID maps to one durable accepted or domain-rejected result; the same ID with a changed payload is an error.
6. Transient admission, capacity, storage, and runtime faults do not consume a new action ID.
7. A rejected action does not mutate canonical room history.
8. A participant action is admitted only against the exact current room head in the frozen releases.
9. A timer candidate mutates the pack only after the exact scheduled timer generation is conditionally claimed in the same transaction.
10. State at sequence N is reproducible from genesis and recorded stimuli through N.
11. A pack cannot observe ambient nondeterminism.
12. Authoritative Room State never crosses the client boundary directly.
13. Authorization precedes observation persistence and artifact access.
14. Every durable viewer is a membership with exactly one addressed observation stream and one cursor.
15. Membership outlives sessions and invocations.
16. Activation is at-least-once intent delivery, not proof of model execution.
17. An expired or superseded activation claim cannot renew, release, or complete a later lease generation.
18. Observation delivery is at least once; clients deduplicate and acknowledge.
19. The catch-up/live actor barrier returns the complete retained authorized range or an explicit projection reset without a handoff gap.
20. Snapshots, indexes, and projection caches are replaceable derivations.
21. Hash disagreement faults or quarantines a room.
22. Replay has no external effects and holds no unbounded database read transaction.
23. Mutating HTTP resources and their idempotency receipts commit atomically.
24. A committed artifact reference points only to bytes made durable before the linking transaction.
25. Passivation is generation-fenced; no command is routed to an actor that may disappear.
26. Slow clients and full queues cannot create unbounded memory growth.
27. Investigation-specific semantics stay outside the Room Kernel; only the preplanned generic artifact subsystem is added.
28. v0.1 and v0.2 run exactly one WorldStream process with exactly one startup-selected supported storage profile.
29. Both storage profiles preserve identical canonical bytes, semantic outcomes, receipts, timers, frames/cursors, Activation fences, recovery, and Replay.
30. A Storage Epoch identifies the sole authoritative deployment lineage; offline transfer advances it only at explicit verified finalization.
31. Backend-native restore or SQLite-to-PostgreSQL transfer is never ready before the full WorldStream semantic verifier passes, except that exactly preserved pre-existing unhealthy Rooms remain isolated.
