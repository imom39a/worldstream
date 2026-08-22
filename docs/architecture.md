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

Not selected for v0.1 or v0.2: an ORM, Redis, NATS, Kafka, Temporal, Wasmtime, Kubernetes, a provider database API as a correctness dependency, an embedded model SDK, or a frontend realtime platform. Every selected adapter MUST preserve this document's backend-neutral Room Commit semantics.

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
    │   │   └── SQLite adapter, migrations, backup and integrity operations
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

The storage interface belongs in worldstream-core; bundled SQLite and `postgres-primary` are its only frozen implementations. Each implements the same logical Room Commit and resolution port, and providers do not alter semantics. There are no provider, broker, crypto, workflow, plugin, or generic connector crates.

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

The supervisor uses an explicit recovery lifecycle before ordinary service:

    Loading → CatchingUp → Active → Passivating → Inactive

These five runtime states are orthogonal to durable Room Integrity State (`healthy | faulted | quarantined`). An integrity failure updates and fences the integrity axis; it does not create another recovery state. A new or reactivated actor enters at Loading, and an actor that cannot continue is removed to Inactive after the applicable integrity disposition is recorded.

Every actor receives a supervisor generation. Loading verifies state; CatchingUp drains the fixed-cutoff overdue-timer set before ordinary canonical work or normal reads attach. Passivation occurs through a barrier: mark Passivating, stop routing directly, drain the actor mailbox, confirm no provisional commit/timer work, then remove the actor. Commands arriving during Loading, CatchingUp, or Passivating have not reserved Room-lane capacity: they wait in a bounded supervisor queue or receive `room_busy`. A stale-generation actor cannot publish after removal, and complete-Head fencing prevents an obsolete commit.

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

The actor MUST expose one bounded Room Admission Lane for Participant Actions, existing-Room canonical administration, and newly due timer candidates. Room creation has no existing actor or lane. A successful reservation and the applicable host time sample are one admission operation. Reserved host-stimulus capacity prevents participant saturation from starving timers; an earlier reservation cannot be overtaken, and once a timer reserves a position later participant traffic cannot pass it. Backpressure reaches the gateway as `room_busy` without Semantic Time, receipt, or deadline entitlement; it does not create more actor tasks for the same Room.

### Versioned Core reducer

`CoreReducerV1` is a pure WorldStream-owned reducer. `CoreRoomStateV1` contains exactly active/archived Room Status and a canonically sorted Membership map. Each Membership contains immutable Member ID, Principal ID, and room-local Principal kind; enabled/suspended/departed standing; participant/spectator/operator Access Mode; and a pack-valid Role exactly when Access Mode is participant. Room Head, hashes, integrity, Sessions, delivery, receipts, Activation, policy, diagnostics, telemetry, and wall time are not Core fields.

A normalized Core Stimulus carries:

- a versioned kind: Join, Resume, Suspend, Depart, AccessModeChange, RoleChange, MembershipChangeSet, or Archive, where MembershipChangeSet is the atomic multi-Membership form;
- canonical authority attribution without bearer secrets;
- idempotency identity and exact expected Room sequence;
- a stable reason code and semantic recorded time when applicable;
- an optional Room Status before/after pair plus a canonical typed before/after component for each affected Member ID, sorted by Member ID.

Member/Principal binding and Principal kind are immutable. Enabled and suspended are reversible; departed is terminal and rejoin uses a new Member ID without inherited Cursor/private frames. At most one non-departed Membership per Principal exists in a Room. Participant requires a Role; spectator/operator forbids one. The reducer validates the complete final multi-Membership state and pack cardinality once, never an intermediate assignment.

Each Membership component is typed Join, Resume, AccessModeChange, RoleChange, Suspend, or Depart. Archive is its own top-level CoreProposed kind, carries the sole Room Status pair, and cannot occur inside MembershipChangeSet. Join, Resume, AccessModeChange, and RoleChange components are vetoable; Suspend and Depart components are mandatory. Before pack entry, the host rejects a MembershipChangeSet that mixes the two classes, with no pack call, Transition, or receipt. A homogeneous changeset inherits its component class: an all-vetoable set may be rejected only as a whole, while an all-mandatory set must Apply as a whole and a Reject is PackFault. The Activity Pack receives immutable Core-before/proposed-after views. A vetoable declared rejection is a stable idempotent administrative result with no Transition. Archive is mandatory independently. A pre-existing final state may produce a durable NoChange disposition. Every other accepted Core change creates exactly one Transition. Archive is irreversible and atomically cancels timers and fences pending/leased Activation work in the winning order.

### Activity host

The Activity host invokes one exact trusted compiled-in revision through the frozen five-operation seam:

~~~rust
pub trait ActivityPackV1: Send + Sync + 'static {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1;
    fn initialize(
        &self,
        input: &GenesisInputV1,
        cx: &DeterministicContextV1,
    ) -> Result<InitialOutputV1, PackFault>;
    fn reduce(
        &self,
        input: &ReduceInputV1,
        cx: &DeterministicContextV1,
    ) -> Result<ReduceDispositionV1, PackFault>;
    fn view(&self, input: &ViewInputV1) -> Result<PackViewV1, PackFault>;
    fn observe(
        &self,
        input: &ObserveInputV1,
    ) -> Result<Option<PackObservationV1>, PackFault>;
}
~~~

All calls are synchronous, pure, bounded, and finish before persistence. The pack receives no clock, storage, network, filesystem, scheduler, Activation, Session, delivery, telemetry, or artifact-byte capability.

Initialization receives exact Genesis input and returns canonical initial Activity State plus ordered timer requests only. Reduction receives prior Activity State, exact Core before/proposed after, canonically sorted scheduled timers, next sequence, and one normalized typed Stimulus. It returns Apply with complete next state/events/timer requests/Attention, or a declared Reject; PackFault is separate.

Participant Actions and join/resume/Access/Role proposals may be declared Reject. Archive/suspend/depart and required timers/external inputs cannot be vetoed. WorldStream resolves Core NoChange before pack entry.

Validation and Activity reduction occur in the one `reduce` call so they cannot disagree after state changes. For a non-Core Stimulus, Core before and proposed after are equal. `view` returns the pack-owned Activity Projection plus exact Action Offers, and `observe` returns a bounded pack-owned Activity Observation for one viewer. WorldStream wraps the Activity Projection and its one separate ActionOfferV1 list with authorized Core Room and Membership facts to form a Projection; the list appears exactly once as the `projection.action_offers` sibling, while protocol envelopes add causal sequence, operational Room Integrity State/generation, schema, and delivery metadata.

Packs request ScheduleNext, CancelCurrent(expected_generation), or RescheduleCurrent(expected_generation, new due/payload). WorldStream owns, assigns, and verifies monotonic timer generations and strict-forward semantic time.

view returns one authorized Activity Projection and ordered canonical Action Offers. Those same bytes are used by reset, observation, Invocation Context, and host pre-admission. observe receives before/after Core and Activity, normalized Stimulus, ordered events, exact viewer, and exact after-view bytes; it returns zero or one viewer result. A changed authorized view with no observation is PackFault.

WorldStream wraps the pack projection and exact Action Offers with authorized Core facts using that single Projection shape. Operator Membership never receives raw Activity State. Callback panic where catchable, malformed output, bound violation, mandatory-Core veto, privacy/view failure, or deterministic disagreement fails closed before commit.

PackRegistryV1 maps each PackRevisionLockV1 semantic digest to the exact executor, descriptor/schemas, codecs, golden digest, and selectable/runnable status. Selectable implies runnable; every retained digest remains runnable even when non-selectable. A Room never changes digest or rewrites Activity State in place. Missing retained executor/codec is an explicit compatibility failure.

Same-process Rust is trusted, not sandboxed. Dynamic/public pack loading and a portable plugin ABI remain unsupported. See [ADR 0010](adr/0010-activity-pack-v1-and-executable-replay-retention.md).

### Storage service

Callers submit immutable `PreparedRoomWriteV1` values through one bounded backend-neutral port. Its Create branch installs a new Room without an existing Room root or Head fence; its Existing branch uses the Room actor and transaction-scoped Room fence. Both `commit` branches and `resolve` first take the same transaction-scoped Operation Identity serialization guard. SQLite uses its dedicated writer and a transaction-start write reservation; PostgreSQL uses transaction-scoped identity exclusion followed, for Existing only, by a Room-root row lock or an equivalent guarded write under Read Committed. Those physical mechanisms are adapter details and MUST expose the same Room Commit resolutions, canonical bytes, hashes, and crash semantics.

The SQLite profile funnels prepared commits and authoritative resolution through one controlled global writer thread and connection. The PostgreSQL profile may execute different identities and Rooms concurrently through direct, session-pooled, or bounded transaction-scoped connections. It uses an xact-scoped advisory guard derived from the canonical Operation Identity or an equivalent unique-key exclusion that is safe when a transaction pooler changes the connection after each transaction; Existing then takes its per-Room root lock or conditional compare-and-set. PostgreSQL has no global commit serialization requirement.

Storage adapters MAY use separate bounded read connections for Room loading, Catch-up, Replay, nonauthoritative receipt preflight, and host-operator queries. A preflight snapshot miss never proves absence. Every authoritative `resolve` runs on the writable primary inside the Operation Identity guard, waits out any earlier same-identity writer, and rereads before returning `StoredResolution`, `Conflict`, or `KnownAbsent`; inability to acquire or complete that barrier is `ResolutionUnavailable`. Returning an already stored result does not acquire a Room fence. A read made during preparation is only a witness, and every fact that authorizes a new write is repeated under the guarded Room transaction. No correctness path depends on connection affinity, session state, named prepared statements, extensions, replica reads, or a provider API.

The storage service owns:

- forward-only migrations;
- atomic Room creation, Room Advances, and durable dispositions;
- Operation Identity and Semantic Receipt resolution;
- snapshot and transition reads;
- observation-frame and activation queries;
- cursor acknowledgement persistence;
- backend-native maintenance metrics and controls;
- backup, restore, and integrity-verification operations.

The backend is selected at startup and remains fixed until shutdown. Loss of PostgreSQL makes readiness unhealthy and mutations fail closed; WorldStream never falls back to SQLite or authoritative in-memory buffering. Both adapters implement the same Room Commit resolution algebra, canonical byte handling, error classification, receipt lookup, migration fingerprint, recovery, and Replay contracts. PostgreSQL remote connections require TLS, the daemon uses a least-privilege DML role, and `synchronous_commit=on` is mandatory.

### Projection and observation engine

After a pack computes the next state, WorldStream calls `view` and `observe` for affected Membership viewers.

- Authoritative Room State is never serialized directly to a client.
- Public, operator-membership, and participant viewers are distinct Rust types.
- Every Observation Frame is persisted under an explicit Membership ID.
- Genesis emits no frame. Each later Transition produces zero or one coalesced frame for each viewer, so a hidden Transition may advance the Room Head without advancing that Membership's frame head.
- A public payload uses an explicitly public type, then is materialized into each authorized enabled membership's single frame stream.
- Projection construction happens before the transition transaction commits.
- Invalid, oversized, or failed projection output aborts the transition rather than committing undisclosable state.
- The exact ordered Action Offer bytes from `view` are copied unchanged into resets, observations, Invocation Context, and host pre-admission.

For the frozen room sizes, evaluating at most 32 viewer projections is an acceptable clarity-over-optimization tradeoff.

### Observation delivery

Committed observation frames are durable. Live WebSocket delivery is an acceleration path:

- after storage commit, a room actor enqueues frame references to connected sessions;
- each session fetches or receives only authorized payloads;
- client acknowledgements monotonically advance a membership frame cursor;
- each Membership separately persists a never-reused frame head and a retained floor; pruning changes neither the Cursor nor frame allocation;
- duplicate delivery is allowed;
- queue overflow closes the connection with a resumable slow-consumer error.

The durable inbox, not an in-memory broadcast channel, is the continuity guarantee.
The gap-free Session barrier, reset contract, retention ceilings, and failure surfaces are frozen in [Observation Delivery and Activation](observation-and-activation.md) and [ADR 0008](adr/0008-membership-observation-streams-and-reset-barriers.md).

### Activation dispatcher

An Attention Signal is deterministic, canonical Activity Pack output. The host applies an exact, versioned per-Membership Activation Policy and persists its revision plus allow/deny/intent decision in the same transaction as the causing Transition; policy and decision evidence are operational and excluded from canonical hashes.

The dispatcher:

- offers pending intents on a runner control WebSocket or HTTP long poll;
- atomically grants a bounded claim lease;
- persists and exposes the exact authorized Invocation Context only after claim;
- renews, completes, expires, or cancels a lease;
- retries delivery without creating another logical activation;
- records operational attempts without changing room history.

It never invokes a model. If a runner is absent, nothing runs.

Activation-control authority is separate from participant Action authority. A successful claim returns authorized Invocation Context but neither grants room:act nor advances the Membership Cursor. Runner SDKs expose separate activation-control and room-member clients, following [ADR 0003](adr/0003-separate-activation-and-action-authority.md).
The full intent states, operation receipts, eligibility witnesses, context union, cancellation rules, and retention contract are frozen in [Observation Delivery and Activation](observation-and-activation.md) and [ADR 0009](adr/0009-activation-intents-context-and-lease-fencing.md).

### Timer scheduler

WorldStream, not the Activity Pack, owns each monotonic Timer Generation. A pack requests exactly one normalized `ScheduleNext(timer_id, due, payload)`, `CancelCurrent(timer_id, expected_generation)`, or `RescheduleCurrent(timer_id, expected_generation, new_due, new_payload)` mutation per logical Timer ID. The host validates the pack-facing `due`/`new_due`, assigns a never-reused next generation, and records it as immutable host-facing `scheduled_for` with its payload and creation cause. A new schedule MUST be strictly later than the causing Stimulus's typed Semantic Time; an initial Genesis timer MUST be strictly later than the typed recorded creation time.

The scheduler uses HostClock only to discover that a generation is due. It reconstructs TimerFired from the immutable `(room_id, timer_id, generation, scheduled_for, payload)` record, adds no `fired_at`, and takes no durable claim. The exact scheduled-generation witness is consumed by the same Advance as its Transition. Retry/restart therefore cannot change semantic input or create a second firing.

Within one Room, due candidates are considered one at a time in `(scheduled_for, timer_id, generation)` order and reread after every result. All still-scheduled due generations remain obligations; they are never expired, merged, coalesced, skipped, or marked fired because of lag or a processing budget.

Timer mutation validation is exact:

| Pack request | Valid precondition | Atomic result |
|---|---|---|
| `ScheduleNext(timer_id, due, payload)` | No generation is currently scheduled; `due` is strictly later than the causing Stimulus's Semantic Time | Host allocates the next never-used generation and records `due` as its immutable `scheduled_for` |
| `CancelCurrent(timer_id, expected_generation)` | That exact generation is scheduled | Current generation becomes cancelled in the causing Advance |
| `RescheduleCurrent(timer_id, expected_generation, new_due, new_payload)` | That exact generation is scheduled and `new_due` is strictly forward | Current generation is cancelled and the next generation records `new_due` as immutable `scheduled_for` atomically |

Conflicting duplicate mutations, stale/wrong expected generations, cancel of missing/fired/cancelled state, implicit replacement, non-forward time, invalid payload, overflow, and unrepresentable time are Activity Faults with no commit. `NotApplicable` is reserved for a scheduler candidate that was independently valid but legitimately lost a race after preparation.

### Timer, Action, and archive races

| Scenario | Required result |
|---|---|
| Action reserves before deadline and commits before the closing timer | Action may succeed; timer observes the changed Head and reprepares if its generation remains scheduled. |
| Closing timer commits before a previously admitted Action | Action is stale/closed and is never rebased or grandfathered by its earlier `admitted_at`. |
| Action reserves exactly at or after deadline while timer is delayed | Commit a durable `deadline_passed` disposition; no Transition. |
| Room lane is full before deadline | Return `room_busy`; assign no `admitted_at`, receipt, or deadline entitlement. |
| Process crashes after lane reservation but before persistence handoff | Provisional position and sample disappear; retry of the same still-unresolved Action identity receives a fresh sample and may now be late. |
| Known-absent storage retry after persistence handoff | Retry the identical sealed plan and preserve its original `admitted_at`. |
| Old timer candidate races cancel/reschedule | Exact old generation becomes `NotApplicable`; it is never retargeted to the newer generation. |
| Two timer generations have the same Scheduled Time | Reserve candidates in `(scheduled_for, timer_id, generation)` order, never database row order. |
| Timer commits before archive | Timer advances first; archive prepares/commits from the later Head and cancels remaining schedules. |
| Archive commits before timer | Archive atomically cancels schedules and fences Activations; timer candidate becomes `NotApplicable`. |
| Authority or integrity fence changes before COMMIT | No Transition or new receipt; timer remains scheduled and Action/administration follows the fenced result. |
| Timer COMMIT is unknown | Resolve the same timer identity/hash before scanning, preparing, or publishing it again. |
| Replay | Use recorded `admitted_at`, `scheduled_for`, and `recorded_at` in `room_seq` order; do not read HostClock or run the scheduler. |

### Fixed-cutoff CatchingUp

After verified Room load, the supervisor reads HostClock once to capture cutoff `C`. If any scheduled generation has `scheduled_for <= C`, the Room enters CatchingUp rather than Active:

1. Select the first still-scheduled due generation by `(scheduled_for, timer_id, generation)`.
2. Submit and fully resolve that one exact candidate.
3. Reread durable timer state because its Transition may cancel, reschedule, or create other generations.
4. Repeat while any still-applicable generation has `scheduled_for <= C`, including overdue cascades created by earlier catch-up Transitions.
5. Only after the fixed set is drained, transition to Active and permit ordinary same-Room canonical lane reservations. Timers with `scheduled_for > C` become normal lane candidates when HostClock reaches them.

Catch-up executes in bounded slices so other Rooms, runtime duties, and storage can progress. A slice boundary never opens an interleaving window for ordinary canonical work in the catching-up Room. Capability revocation, integrity fault/quarantine, and diagnostics remain available because they are safeguards rather than ordinary canonical commands.

Lag, restart, rate limits, and slice budgets change latency only. A valid overdue cascade remains durable work even when throttled or operationally faulted; no obligation is dropped. Non-progressing invalid output, non-forward scheduling, and time/generation overflow use the Activity Fault path. If HostClock becomes untrustworthy, CatchingUp pauses time-bearing work without advancing `C`, changing a schedule, or making the Room Active.

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
- A versioned Canonical Request Hash binds one Operation Identity to all caller-semantic input.
- Core State hash domain-separates the Core schema version and canonical Core bytes.
- Activity State hash domain-separates the exact pack digest and canonical Activity bytes.
- Authoritative State hash domain-separates and binds both component hashes plus their version identities.
- Genesis hash binds Room/version identities, Core schema version, exact pack digest, canonical configuration, normalized initial timers, Room seed, logical creation time, and all three initial state hashes.
- Transition hash binds Room/sequence/version identities, Core schema version, exact pack digest, the prior Genesis-or-Transition hash, normalized recorded Stimulus, ordered Domain Events, normalized ordered timer changes, deterministic ordered Attention Signals, and all three resulting state hashes.

Every hash input is one canonical typed object containing an explicit domain/version field. Raw concatenation of variable-length fields is forbidden. Golden vectors define integer, byte/digest, and string encoding. Integrity state/generation/incidents, bearer or commit witnesses, receipts, materializations, snapshots, Projections/frames/Cursors/resets/Sessions, policy and Activation records, diagnostics, telemetry, and commit wall time are excluded. Canonical authority attribution, idempotency identity, expected sequence, reason, and semantic time/input fields inside a normalized Stimulus remain included.

The complete Room Head is the atomic tuple `(room_id, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash)`. Genesis establishes it at sequence zero; every accepted Transition replaces the complete tuple.

### Time

There is no universal canonical Transition clock. The only pack-visible time is a typed field whose meaning belongs to its Stimulus:

| Stimulus | Canonical field | Sampling/meaning |
|---|---|---|
| Participant Action | `admitted_at` | HostClock sample taken atomically with successful bounded Room-lane reservation after strict parsing, initial authentication, rate admission, and size checks |
| TimerFired | `scheduled_for` | Immutable value from the exact scheduled Timer Generation; also the firing's effective time |
| Membership, administration, external input | versioned `recorded_at` | Host-recorded value with the stimulus-specific declared meaning |

Timer scan/detection, enqueue, retry, lag, actor dequeue, transaction start, receipt, and commit timestamps are operational audit/telemetry only. They are invisible to Activity Pack reduction and excluded from canonical hashes unless a separately declared recorded Stimulus field names them. `TimerFired.fired_at` does not exist.

One application-owned, injectable HostClock produces normalized UTC samples and evaluates `HostClock >= scheduled_for`. Issued trusted samples never decrease. Ordinary forward movement and suspend/resume count as elapsed time; backward movement never reopens a deadline. A rollback or discontinuity outside the configured trust policy fences new time-bearing canonical work until operator correction and verification. Client time and SQLite/PostgreSQL clock functions never define Room semantics. Exact precision, discontinuity tolerance, and clock implementation belong to the versioned compatibility/configuration contract and deterministic tests use a fake clock.

Action windows are half-open: `open_at <= admitted_at < deadline`. Equality is late. Scheduler lag cannot extend the window: an Action admitted at or after the deadline receives stable `deadline_passed` even while the closing timer remains scheduled. A timely Action may commit after the deadline only if its complete Head and all other witnesses still pass and neither the closing timer nor archive has committed first.

Clock behavior is explicit:

| Observation | HostClock/domain behavior |
|---|---|
| Ordinary forward progress | Issue the normalized sample; newly due timers become eligible. |
| Process suspend/resume or trustworthy large forward passage | Count elapsed time; timers may be overdue and must Catch Up without being skipped. |
| Raw source moves backward within the configured trustworthy normalization policy | Never issue a decreasing sample and never reopen an Action window. |
| Rollback or discontinuity makes the source untrustworthy | Enter clock-untrusted operational state; deny new time-bearing canonical admission and pause timer eligibility/CatchingUp without changing durable schedules. |
| Operator corrects/verifies the source | Resume only from a verified nondecreasing baseline; never rewrite committed Semantic Time. |
| Time is unrepresentable or forward scheduling overflows | Fail before commit; invalid pack output is an Activity Fault and no generation is created. |

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
    participant S as "Selected storage adapter"
    participant D as "Durable database"

    C->>G: action.submit with action_id and based_on_room_seq
    G->>R: authenticated, strictly parsed request
    R->>S: resolve(identity, canonical_request_hash)

    alt Existing same-hash resolution
        S-->>R: original stored result
        R-->>C: same accepted or rejected result
    else Same identity, changed hash
        S-->>R: Conflict
        R-->>C: idempotency_conflict
    else Synchronized KnownAbsent
        R->>R: rate/size admission, reserve lane + sample admitted_at
        alt Lane unavailable
            R-->>C: room_busy, no admitted_at or receipt
        else Admitted
            R->>P: reduce and construct projections/frames outside locks
            R->>R: seal PreparedRoomWriteV1::Existing(PreparedRoomCommitV1)
            R->>S: commit(prepared)
            S->>D: guard identity, fence Room, recheck witnesses, write bundle
            alt Database COMMIT confirmed
                D-->>S: committed
                S-->>R: Resolved(New)
                R->>R: install committed Head and state
                R-->>C: stored semantic result
                R-->>C: postcommit publication
            else COMMIT status unknown
                D-->>S: ambiguous
                S-->>R: Indeterminate
                R->>S: resolve(same identity, same canonical_request_hash)
            end
        end
    end
~~~

Database COMMIT is the sole linearization point. Commit-before-acknowledgement is non-negotiable, and an uncertain attempt stays attached to its original identity until resolved.

### Failure results

| Failure/race point | Required classification and action |
|---|---|
| Existing identity, same Canonical Request Hash | `Resolved(... Existing)`; return the original semantic result read-only |
| Existing identity, changed Canonical Request Hash | `Conflict`; never run domain work |
| Head or policy witness changed before COMMIT | `Reprepare`; discard the sealed plan and follow the operation-specific rule, never blind-retry it |
| Integrity or authority/capability witness changed | `Fenced`; no Transition or new receipt |
| Timer/input witness is cancelled, consumed, missing, or obsolete | `NotApplicable`; no pack application or receipt-only rejection |
| Busy/deadlock/serialization/rollback proves no write committed | `RetryableKnownAbsent`; bounded retry of the identical sealed plan is permitted |
| Malformed sealed plan or verified structural/hash invariant failure | `Fault`; known absent and nonretryable |
| COMMIT may have succeeded | `Indeterminate`; resolve the original identity/hash on the authoritative primary before any reprepare, retry, scan, reply, or publication |
| COMMIT succeeded, actor dies before install/reply | Reload from storage; retry resolves the original receipt; stale/dead actor publishes nothing |
| COMMIT succeeded, frame/Activation publication fails | Observation Catch-up or pending-intent scan redelivers; never recommit |
| Postcommit paired snapshot fails | The Transition remains valid; recovery uses an earlier snapshot or Genesis plus Transitions |
| Projection/frame computation or pack call fails before transaction | No write; Activity Fault handling applies |
| Process terminates with due timers | CatchingUp resubmits the exact immutable Timer Generations in deterministic order |
| Storage/integrity failure prevents a known resolution | Fail closed and expose the precise known-absent, Indeterminate, or Fenced class; do not collapse them into generic retry |
| Activation lease holder fails before completion | The generation-fenced lease expires; the same pending intent may be claimed again |
| Pack panic or deterministic projection failure | No write; discard the actor and reload or quarantine through integrity handling |

## Backend-neutral logical records

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
    authority_generation INTEGER
    expires_at TEXT NULL
    revoked_at TEXT NULL

`authority_generation` is the durable compare-and-set witness for the capability's effective scope/revocation state and increments on every such change. A claim never relies on connection-local authentication state or an inferred counter.

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

`status` and the three state hashes are verified current materializations. `integrity_state` and `integrity_generation` are durable operational fencing state. Create initializes them exactly to `healthy` and `1` in the same transaction as Genesis and its receipt; they are not a pre-existing creation witness and do not enter Genesis or canonical hashes. Neither this row nor the current Membership rows supersede canonical Genesis/Transitions.

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
    recorded_created_at TEXT
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
    retained_frame_floor INTEGER
    last_ack_frame_seq INTEGER
    membership_generation INTEGER
    activation_policy_revision INTEGER
    activation_policy_json BLOB
    created_at TEXT
    updated_at TEXT
    PRIMARY KEY (room_id, member_id)

Member ID, Principal ID/kind, standing, Access Mode, and Role are the current Core materialization. Frame head/Cursor, `membership_generation`, and activation policy/revision are operational delivery/control fields co-located physically but excluded from Core and canonical hashes. Every relevant Membership change increments `membership_generation`; every policy change increments `activation_policy_revision`. Activation claim compare-and-set matches those durable source fields, the capability's `authority_generation`, and the Room's `integrity_generation`; no witness relies on an unpersisted counter. Connection and model invocation state MUST NOT be columns in this table.

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
    previous_transition_hash BLOB
    resulting_core_state_hash BLOB
    resulting_activity_state_hash BLOB
    resulting_authoritative_state_hash BLOB
    transition_hash BLOB
    committed_at TEXT
    PRIMARY KEY (room_id, seq)

`stimulus_json` carries exactly one typed Semantic Time; there is no universal Transition time column.

### semantic_receipts

    room_id TEXT
    operation_kind TEXT CHECK operation_kind IN ('action', 'administration', 'timer_fired', 'external_input')
    operation_identity_json BLOB
    codec_id TEXT
    canonical_request_hash BLOB
    basis_complete_head_json BLOB NULL
    semantic_input_json BLOB
    resolution_kind TEXT CHECK resolution_kind IN ('genesis_created', 'transition_committed', 'rejection_recorded', 'no_change_recorded')
    transition_seq INTEGER NULL
    stored_resolution_json BLOB
    committed_at TEXT
    PRIMARY KEY (operation_kind, operation_identity_json)

Every committed Action, administration, TimerFired, or external-input Operation Identity binds exactly one Canonical Request Hash and StoredResolution. Action and administration indexes may project their typed identity fields, but they do not define a parallel receipt contract. Existing-Room operations retain their exact eight-field basis Complete Head, typed semantic input such as Action `admitted_at` or TimerFired `scheduled_for`/payload, and original result. Room creation is the sole no-basis case: its administration identity row has `basis_complete_head_json = NULL`, `transition_seq = NULL`, `resolution_kind = genesis_created`, the generated Room ID in `room_id`, and a StoredResolution containing the generated Room and initial Member IDs plus exact complete Head zero. The row commits in the same transaction as `rooms`, `room_genesis`, initial `room_members`/timer rows, initial `healthy`/generation-`1` operational integrity, and current Core/Activity materializations. Its Room/Genesis foreign keys are transaction-deferred so the sealed receipt may be staged after the Operation Identity guard but before the generated Room row; the constraints must hold before COMMIT and the staged row is never externally visible. All other stored Room resolutions have a non-null basis, and `transition_seq` is non-null exactly for `transition_committed`. Same identity and hash returns the StoredResolution; a changed hash is Conflict. `NotApplicable` binds no identity, hash, or receipt. Transient admission/runtime errors never enter this record and do not consume an identity.

`codec_id = "worldstream/operation-receipt/v1"` is the serialization umbrella for these Semantic Receipts and for the Activation operation receipts below. It identifies a byte envelope, not one shared domain state or resolution algebra.

For mutating HTTP administration, `worldstreamctl` generates capability bearer secrets locally and sends only their derived token hash, so retrying capability creation never requires the server to store or replay plaintext.

### snapshots

    room_id TEXT
    seq INTEGER
    genesis_or_transition_hash BLOB
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
    frame_payload_hash BLOB
    created_at TEXT
    PRIMARY KEY (room_id, recipient_member_id, frame_seq)

Every durable stream belongs to one Membership. Public consequences are materialized into every enabled Membership stream allowed to see them. Spectator and operator Memberships are read-only and have separate Cursors; Principal kind remains independent of Access Mode. This deliberately duplicates small frames to keep privacy and Catch-up semantics unambiguous.

`frame_payload_hash` is frame-only payload integrity. It never substitutes for the Canonical Request Hash bound to an Operation Identity and Semantic Receipt.

`frame_head` never decreases or reuses a value. `retained_frame_floor` may advance during pruning without moving `last_ack_frame_seq`. Genesis has no frame; a later Transition has at most one row per recipient Membership.

### timers

    room_id TEXT
    timer_id TEXT
    generation INTEGER
    scheduled_for TEXT
    payload_json BLOB
    creation_cause_seq INTEGER
    status TEXT CHECK status IN ('scheduled', 'cancelled', 'fired')
    created_seq INTEGER
    cancelled_seq INTEGER NULL
    fired_seq INTEGER NULL
    PRIMARY KEY (room_id, timer_id, generation)

One `(room_id, timer_id)` has at most one scheduled generation. `scheduled_for`, payload, and creation cause are immutable; generations start at one and are host-owned, monotonic, never reused, and never wrapped. Scheduler indexing is operational and cannot define equal-time order.

### activation_intents

    activation_id TEXT PRIMARY KEY
    room_id TEXT
    member_id TEXT
    cause_room_seq INTEGER
    reason_code TEXT
    policy_revision INTEGER
    authority_generation INTEGER
    membership_generation INTEGER
    integrity_generation INTEGER
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

### activation_operation_receipts

Durable claim receipts and stale-lease protection:

    activation_id TEXT
    operation_kind TEXT CHECK operation_kind IN ('claim', 'renew', 'release', 'complete')
    operation_id TEXT
    runner_id TEXT
    canonical_request_hash BLOB
    claim_id TEXT NULL
    lease_generation INTEGER
    result_code TEXT CHECK result_code IN ('granted', 'not_available', 'expired', 'cancelled', 'fenced', 'completed', 'released', 'renewed')
    lease_until TEXT NULL
    result_json BLOB
    result_hash BLOB
    context_retention_state TEXT CHECK context_retention_state IN ('not_applicable', 'retained', 'retired')
    invocation_context_hash BLOB NULL
    invocation_context_blob BLOB NULL
    invocation_context_tombstone_json BLOB NULL
    created_at TEXT
    PRIMARY KEY (activation_id, operation_kind, operation_id)

For claim, `operation_id` and `claim_id` are the same value; later control operations have a new `operation_id` and name the active `claim_id`. The server derives `canonical_request_hash` from the complete authenticated request. A repeated operation ID from the same authenticated Runner and identical canonical request resolves its durable receipt; it returns the exact original result while any required context is retained and follows the retirement rule below otherwise. Reusing it with a changed request or Runner is an idempotency conflict. Renew, release, and complete match the current claim ID, Runner ID, generation, and unexpired lease.

`result_code`, `result_json`, and `result_hash` permanently identify the original stored disposition. `result_json` contains only non-context result fields; exact private context bytes occur only in `invocation_context_blob`. For a granted claim the result hash binds those immutable non-context result fields plus `invocation_context_hash`. A non-grant or control operation uses `not_applicable` and has no context bytes or tombstone. A granted claim starts as `retained` with exact context bytes and no tombstone. At retention expiry, one audited operation changes only the retention discriminator, removes the private bytes, and writes a versioned tombstone containing the context hash, retirement time, and reason. It never changes the original result code or hash. An identical retry of that originally granted operation then deterministically returns the wire error `result_retired` instead of regenerating context or rewriting the stored result. These receipts serialize under `worldstream/operation-receipt/v1`, while retaining their distinct Activation result and lease-witness model.

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

The exact release-bundled SQLite source inventory and all pragmas are recorded in the compatibility manifest. Each native build identity and its compiler/source relationship are recorded in the detached SBOM and provenance. The main, WAL, and shared-memory files remain on one validated local filesystem; system SQLite, network/UNC filesystems, shared writers, and SQLite on a writable container overlay are rejected. The writer controls checkpoints and reports WAL size and checkpoint latency. Large write transactions are avoided.

### Required PostgreSQL 17 behavior

`postgres-primary` connects to one ordinary writable PostgreSQL 17 primary, hosted or self-managed. Runtime Create, Existing, and `resolve` transactions use Read Committed, set `synchronous_commit=on`, and first take the transaction-scoped Operation Identity exclusion; Existing then locks or compare-and-sets the Room root and every operation-specific fence. All authoritative preconditions are decided before COMMIT. The runtime role has only required DML/sequence permissions. Remote connections require TLS.

Direct, session-pooled, and bounded transaction-scoped runtime connections are supported. A transaction pooler may select a different connection for every transaction. Named prepared statements, persistent temporary objects, session variables, advisory locks whose meaning outlives one transaction, extensions, replicas, provider APIs, and provider-specific error or failover behavior are not correctness dependencies. Migration, transfer, native dump/restore, and full verification use a direct admin connection outside the daemon.

### Forward-only logical migrations and retained codecs

One ordered logical migration history and schema-contract fingerprint govern both adapters. A logical migration has a stable ID and checksum plus backend-specific DDL/execution; it upgrades an empty store or any earlier v0.1 schema, and is atomic or explicitly restart-safe. Production is forward-only: there are no down migrations, mixed-version serving, rolling multi-version operation, or old-binary start after migration. Rollback restores the pre-upgrade backend backup and previous binary together.

SQLite automatic migration occurs only during exclusive locked startup after creation and verification of a recoverable backup. Production PostgreSQL migration is an explicit offline `worldstreamctl` maintenance operation over a direct admin connection while no WorldStream process serves; daemon startup only checks engine, manifest, schema fingerprint, migration checksums, and runtime capabilities. A development auto-migration option is not production evidence.

Each release retains readers for every canonical and receipt codec that can occur in supported v0.1 data, transfer bundles, and backups. Writers emit only the current manifest-declared versions. Engine-native types never decode and re-encode canonical JSON or receipt bytes during migration, transfer, backup verification, or ordinary persistence.

Actual migrations MUST mark all required fields NOT NULL, use CHECK constraints for every documented enum, and declare foreign keys for correctness relationships, including:

- capabilities and runners to principals;
- genesis, members, transitions, snapshots, frames, timers, activations, uploads, and room artifacts to rooms;
- members and upload owners to principals/memberships as appropriate;
- Genesis-created Semantic Receipts to their created Room and Genesis, and Transition-committed Semantic Receipts to their transition;
- observation/timer/activation cause sequences to a transition in the same room when nonzero;
- activation claims to activation intents and runners;
- room artifacts/uploads to artifact metadata.

Startup and restore diagnostics run integrity_check and foreign_key_check before readiness. Tests prove that every connection enables enforcement.

## Backend-neutral atomic Room Commit

### Contract surface

A prepared Room creation or accepted existing-Room Stimulus uses the contract below. SQLite maps every `commit` and `resolve` guard to its controlled writer and transaction-start `BEGIN IMMEDIATE` reservation. PostgreSQL maps the shared guard to transaction-scoped Operation Identity exclusion under Read Committed; `commit(Existing(...))` then locks or conditionally updates the Room root. Every path acquires at most one identity guard first, followed only by its Room or generated-ID locks, so no path reverses the order.

The storage port exposes exactly two semantic operations for a prepared Room write:

    commit(PreparedRoomWriteV1) -> RoomCommitResolution
    resolve(OperationIdentity, CanonicalRequestHash) -> ResolveOutcome

`PreparedRoomWriteV1` is exactly `Create(PreparedRoomCreationV1) | Existing(PreparedRoomCommitV1)`. `commit` accepts one already-admitted, fully computed branch for exactly one new or existing Room. Both operations acquire the same transaction-scoped Operation Identity guard before inspecting or staging a receipt. `resolve` performs no domain work, waits out any earlier/in-flight writer for that identity, rereads the writable authoritative primary while still guarded, and returns exactly one of:

- `StoredResolution`, containing the original Semantic Receipt for the same identity and hash;
- `Conflict`, when that identity is durably bound to another hash;
- `KnownAbsent`, when the guarded authoritative-primary reread proves no resolution exists and therefore maps to `RetryableKnownAbsent`; or
- `ResolutionUnavailable`, when storage cannot acquire/complete the guard or otherwise prove stored versus absent.

A plain Read Committed snapshot miss is never `KnownAbsent`. This serialization rule ensures a resolve racing an uncommitted same-identity receipt waits until that writer commits or rolls back instead of returning a false absence. It is an internal transaction discipline of the two existing port operations, not a third semantic operation or a durable no-result receipt.

Room archive and accepted Membership Standing, Access Mode, or Role changes are Core Stimuli and consume the next Room sequence. Their receipt, Core and Activity results, Domain Events, hashes, timers, addressed Frames, and allowed Activation decisions commit atomically with that Transition. Archive also cancels scheduled timers and fences pending/leased Activation work. Session presence, Runner availability, Activation lease operations, integrity incidents/repair, diagnostics, and telemetry remain operational and never consume Room sequence, following [ADR 0002](adr/0002-sequence-domain-relevant-room-changes.md) and [ADR 0005](adr/0005-canonical-core-state-integrity-and-hash-lineage.md).

A new Existing write has exactly one prepared intent:

- `Advance`: one canonical Transition and every durable consequence caused by it; or
- `DurableDisposition { Rejection | NoChange }`: one stable semantic result and identity fence with no Transition.

Room creation uses only the Create branch of this port and never an existing-Room lane or Head fence. Observation acknowledgements, Activation claims/leases/delivery attempts, capability management, integrity repair, backup/restore, snapshots, telemetry, and derived indexing use separate transactions outside this Room-write port. One Room write never spans Rooms.

### PreparedRoomWriteV1 branches

`PreparedRoomCreationV1` seals every byte its transaction may persist:

| Field | Exact meaning |
|---|---|
| Operation Identity | Administration `(authenticated_principal, versioned_operation_kind, idempotency_key)` for the create-room operation |
| Canonical Request Hash | Versioned hash of the selected exact pack digest, configuration, and ordered initial Membership proposal; excludes generated Room/Member IDs, Room seed, logical creation time, commit time, transport IDs, and retry-attempt data |
| Authority witness | Exact authenticated Principal, creation-capability generation/scope and revocation facts, and authority to create and read this result |
| Pack and generated inputs | Selected exact `PackRevisionLockV1`/digest, generated Room and initial Member IDs, Room seed, and logical creation time |
| Prepared creation bundle | Genesis; initial Core and Activity State; normalized initial timers; Core, Activity, and aggregate Authoritative State hashes; exact Complete Head zero; current Room/Core/Activity/Membership/timer materializations with operational Room Integrity State exactly `healthy` at generation `1`; and the `genesis_created` Semantic Receipt returning the generated IDs and Head zero |

The Create branch has no basis Complete Head, pre-existing integrity witness, existing-Room admission-lane position, or Room fence. Its initial `healthy`/generation-`1` integrity value is newly installed operational state, not a witness or canonical input. Pack selection, Core initialization, the pack's `initialize` call, canonicalization, timer normalization, all three hashes, Genesis/hash construction, Head-zero construction, materializations, and receipt bytes finish before the transaction opens.

`PreparedRoomCommitV1` is the Existing branch. Preparation seals all bytes that its transaction may persist. The value contains:

| Field | Exact meaning |
|---|---|
| Operation Identity | Action `(room_id, member_id, action_id)`; administration `(authenticated_principal, versioned_operation_kind, idempotency_key)`; timer `(room_id, timer_id, generation)`; external input `(room_id, source_id, input_id)` |
| Canonical Request Hash | Versioned hash of all caller-semantic input; excludes Action `admitted_at`, generated Transition ID, commit time, transport IDs, and retry-attempt data |
| Complete Head witness | Exact `(room_id, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash)` observed during preparation |
| Integrity witness | `healthy` plus the exact integrity generation; an operational state/generation change fences the plan |
| Authority witness | Exact authenticated Principal, capability generation/scope, revocation facts, and authority to submit or read this operation's result |
| Policy witness | Exact activation-policy revision whenever the plan includes a noncanonical policy decision or Activation Intent |
| Input witness | The exact normalized Stimulus and every non-Head durable input that affected preparation: Action Membership standing/access/Role/basis/offer, ordered administration changeset and affected versions, timer candidate/current Timer View, or external source/input identity |
| Prepared intent | The complete immutable Advance bundle or the exact safe Rejection/NoChange disposition |

The Complete Head is indivisible. The same `room_seq` with a different prior Transition, Core, Activity, or aggregate Authoritative hash is `Fault` and triggers integrity handling; it is never treated as ordinary contention.

The Canonical Request Hash and prepared canonical Stimulus are different objects. For example, an Action request hash binds the protocol/domain version, Room, Membership, expected basis, Action type, and complete typed payload, while the prepared Stimulus additionally carries host-generated `admitted_at`. A Timer request hash binds the immutable `scheduled_for` and payload to `(room_id, timer_id, generation)`, so changed semantic timer bytes under the same identity are a `Conflict`. The Semantic Receipt preserves both the hash and the committed semantic fields without allowing a retry to alter either.

### Preparation and lock boundary

Before opening the transaction, the Room actor MUST finish:

1. identity preflight lookup and strict admission;
2. Core proposal and Activity Pack execution;
3. deterministic reduction and Attention Signal derivation;
4. canonicalization, schema/size/cardinality/limit checks, and all three resulting state hashes;
5. Transition/hash-chain construction;
6. authorized Projection and zero-or-one Observation Frame computation for every affected viewer;
7. normalized host-owned timer mutations and Activation policy decisions; and
8. immutable `PreparedRoomCommitV1` sealing inside `Existing(...)`.

No Activity Pack initialization or reduction, canonicalization, hash computation, Projection/Frame construction, network publication, telemetry export, derived-index work, or snapshot write may run while storage locks are held. Either transaction may validate and persist only bounded prepared bytes and witnesses.

### Guard and write order

Every Create follows this exact guarded order inside one transaction:

1. Acquire the transaction-scoped Operation Identity serialization guard.
2. Reread the Semantic Receipt key while guarded. Same identity/hash returns the stored `GenesisCreated { Existing }` receipt without writing; same identity/different hash returns `Conflict` without writing. Otherwise conditionally stage the sealed receipt; it remains transaction-private and rolls back with any later guard or bundle failure.
3. Revalidate the complete creation authority/capability witness and revocation state.
4. Prove the generated Room ID is absent. A collision proves this write absent and returns `Reprepare`; only generated values may be resealed under the unchanged identity/hash and caller-semantic input.
5. Persist the Room root, Genesis, initial Core/Activity/Membership/timer materializations, initial operational integrity `healthy`/generation `1`, all three hashes and exact Complete Head zero, and `genesis_created` Semantic Receipt as one bundle.
6. Issue durable database `COMMIT`.

Creation does not acquire or synthesize a basis Head, pre-existing Room Integrity fence, or existing-Room lane. Every new Existing Advance or durable disposition follows this exact guarded order inside one transaction:

1. Acquire the transaction-scoped Operation Identity serialization guard.
2. Acquire the transaction-scoped Room write fence.
3. Recheck Operation Identity. Same identity/hash returns the stored resolution without writing; same identity/different hash returns `Conflict` without writing.
4. Compare every Complete Head field and verify healthy plus unchanged integrity generation.
5. Revalidate the full authority/capability witness and revocation state.
6. Revalidate the policy revision when present and the complete operation-specific input/timer witness.
7. For an `Advance`, persist, in logical dependency order:
   1. the Transition, prior/Transition hash chain, and resulting Core, Activity, and aggregate Authoritative hashes;
   2. the new Complete Head and verified current Core/Activity serving materializations;
   3. the final Membership materialization for the complete atomic changeset, with no visible invalid intermediate;
   4. exact timer candidate consumption plus normalized schedule/cancel/reschedule changes;
   5. addressed Observation Frames and each affected stream's frame head;
   6. activation-policy revision/decision, permitted Activation Intents, and required Membership/archive eligibility and lease-generation fences; and
   7. the Semantic Receipt for the applicable Action, administration, TimerFired, or external input.
8. For a `DurableDisposition`, persist only its Semantic Receipt after all applicable guards pass; do not mutate Head, state, timers, Frames, or Activation.
9. Issue durable database `COMMIT`.

Archive and any final Membership state that is no longer an enabled Agent Participant with participant Access Mode and a current Role cancel and generation-fence that target's pending/leased Activation work inside Advance step 7. This includes suspension, departure, Role removal, and participant-to-spectator/operator changes; the transaction never leaves newly ineligible work claimable.

Adapters may arrange bounded physical statements around backend constraint mechanics only when failure injection proves the same guard precedence, all-or-none bundle, and externally invisible intermediate state. SQLite maps Create, Existing, and resolve to its dedicated writer and transaction-start write reservation. PostgreSQL maps all three to xact-scoped Operation Identity exclusion safe under transaction pooling; Create then checks generated Room-ID absence, Existing then takes a Room-root lock or equivalent guarded write, and resolve waits and rereads without a Room lock. No path acquires a Room/ID lock before its identity guard, and neither adapter may weaken or add a Room Commit resolution class.

Database COMMIT is the sole linearization point. A conditional row change, lock acquisition, driver return, actor-memory installation, acknowledgement, or live publication is not public success and does not order the Room. Authority revocation, archive, Action, and TimerFired races are ordered by their durable commits.

### Room Commit resolution algebra

| Resolution | Exact meaning | Permitted next action |
|---|---|---|
| `Resolved(GenesisCreated { New })` | This attempt committed the prepared creation bundle. | Install/serve the returned generated IDs and Head zero, then acknowledge. |
| `Resolved(GenesisCreated { Existing })` | The same administration identity/hash already committed creation. | Return the original generated Room and Member IDs and exact Head zero; do not regenerate or recommit. |
| `Resolved(TransitionCommitted { New })` | This attempt committed the prepared Advance. | Install returned Head/state, then acknowledge/publish. |
| `Resolved(TransitionCommitted { Existing })` | The same identity/hash already committed that Advance. | Return the original receipt; do not install speculative state or recommit. |
| `Resolved(RejectionRecorded { New })` or `Resolved(RejectionRecorded { Existing })` | The stable Rejection was newly committed or already stored. | Return the original rejection; that identity is consumed. |
| `Resolved(NoChangeRecorded { New })` or `Resolved(NoChangeRecorded { Existing })` | The administrative desired state was already true and the stable NoChange was newly committed or already stored. | Return the original NoChange; no `room_seq` was consumed. |
| `NotApplicable` | An independently valid timer/input candidate is now missing, cancelled, consumed, or obsolete. | Stop; do not call the pack or create a rejection receipt. |
| `Reprepare` | An Existing Head/policy witness changed, or a Create generated Room ID collided, and this plan is proven absent. | Discard the plan and follow the operation-specific reprepare rule below. |
| `Fenced` | Room Integrity State/generation or operational authority no longer permits the write. | Stop with no Transition or receipt; require recovery or fresh authority as applicable. |
| `Conflict` | The identity exists with another Canonical Request Hash. | Return stable conflict; never retry under that identity. |
| `RetryableKnownAbsent` | A commit failure proves absence, or synchronized resolve `KnownAbsent` maps to this proof after waiting out same-identity writers. | A bounded retry may resubmit the identical sealed plan; only the creation-specific lost-plan rule below permits fresh generated values. |
| `Indeterminate` | COMMIT may or may not have happened. | Resolve the same identity/hash on the authoritative primary before anything else. |
| `Fault` | The sealed plan is malformed or a structural/hash invariant is verified false. The write is known absent. | Do not retry; enter the defined fault/integrity path. |

Authentication, strict parsing/schema failure, rate/capacity admission, and pre-admission policy are outside this algebra because no `PreparedRoomWriteV1` exists.

`Reprepare` never means blindly re-execute the same decision:

- a Participant Action is never rebased; prepare a stable stale-basis rejection under the new exact Head when receiptable, otherwise return the applicable nonreceipt error;
- exact-head administration similarly recomputes only a stable stale/NoChange result permitted by its contract, never silently changes the requested basis;
- a timer first rereads its immutable generation: if it remains scheduled, reapply against the new Head with the same timer identity and recorded fields; otherwise return `NotApplicable`;
- a same-Head policy-revision change recomputes only the noncanonical policy decision/Activation portion before resealing; and
- a creation Room-ID collision may regenerate only the Room/Member IDs, seed, logical creation time, and their derived sealed bundle under the same identity/hash and unchanged caller-semantic input.

Only `RetryableKnownAbsent`, including synchronized resolve `KnownAbsent` mapped to it, permits retry of an identical sealed plan. For creation only, if restart discarded that proven-absent sealed plan, fresh preparation may generate only Room/Member IDs, seed, logical creation time, and their derived bundle under the unchanged identity/hash, caller-semantic input, exact pack, and current authority. This exception never permits an Action or other operation to resample or change Semantic Time under the same identity/hash. If a retry encounters changed witnesses, it returns the corresponding `Reprepare`, `Fenced`, or `NotApplicable`; the adapter never edits a retained plan.

### Semantic Receipts and durable dispositions

Every stored Semantic Receipt contains a codec/domain version, Operation Identity, Canonical Request Hash, its exact basis Complete Head when one exists, its original typed Semantic Time when applicable, and exactly one semantic result:

- generated Room and initial Member IDs plus resulting complete Head zero for the sole no-basis room-creation operation;
- accepted Transition identity, sequence, and resulting complete hashes;
- stable safe Rejection code and bounded details; or
- administrative NoChange code and bounded details.

It also records operational commit time for audit, but that timestamp is not canonical Room input. A retry renders a fresh transport envelope marked duplicate while preserving every original semantic field.

Receiptable participant results include stale basis, disabled Membership, illegal Action, expired deadline, archived/terminal Room, and expected pack-domain rejection. Receiptable administration results include stale basis, expected policy/cardinality rejection, and a valid already-satisfied request. `NoChange` is limited to administration whose normalized desired state was true before pack application; an empty Membership changeset consumes no sequence.

Malformed input, authentication/authority failure, rate/capacity rejection, unhealthy integrity, Activity Fault, storage failure, and timer obsolescence never become durable dispositions. Once a participant, timer, external, Membership, or administrative Stimulus is accepted by the reducer, it always creates a Transition even if resulting state bytes are unchanged.

Receipts or equivalent compact semantic tombstones remain resolvable while the Room lineage is retained, including after archive. Compaction may remove presentation-only bytes but must preserve the identity fence and equivalent result. Only an explicit whole-Room purge may remove history and its receipts together.

### Unknown COMMIT resolution and postcommit work

After persistence handoff, cancellation is advisory: the attempt must reach `Resolved`, proven absence, or `Indeterminate`. On `Indeterminate`, the caller repeatedly uses `resolve` against the authoritative primary:

1. `StoredResolution` returns the exact original branch result, including generated Room/Member IDs and Head zero for creation;
2. `Conflict` exposes identity misuse and stops;
3. `ResolutionUnavailable`—including a snapshot miss without the identity guard—preserves `Indeterminate` and retries resolution later without pack execution, scanning, creation-value regeneration, repreparation, acknowledgement, or publication;
4. only synchronized `KnownAbsent`, obtained after the identity guard waits out any earlier writer and the primary is reread, proves the atomic creation, Advance, or disposition transaction did not commit and maps to `RetryableKnownAbsent`.

While a creation remains `Indeterminate`, it is resolution-only: the caller cannot regenerate IDs/seed/time, reseal, initialize again, or enter `Reprepare`. After synchronized `KnownAbsent`, a retained sealed creation may be retried identically; if restart lost it, the caller may freshly generate only the excluded Room/Member IDs, seed, logical creation time, and their derived bundle under the unchanged identity/hash, unchanged caller-semantic input and exact pack, and current authority. No prior result exists in that case. Separately, a commit-time generated Room-ID collision is a proven-absent `Reprepare` and may reseal the same generated fields. For a newly committed creation, the returned generated IDs and Head zero are served only from the committed receipt/bundle; a lost reply is resolved by identity/hash and never regenerates them. For a newly committed Advance, only the current actor generation installs the returned Complete Head and prepared in-memory state. A stale/dead actor acknowledges and publishes nothing; the supervisor reloads and callers resolve their identities. Failed Frame publication is recovered by Observation Catch-up, and failed Activation notification by scanning pending intents. Neither failure recommits.

A paired Core+Activity snapshot at the committed sequence is an idempotent postcommit cache. Snapshot failure never rolls back or faults a valid Transition. Cursor acknowledgements, Activation control operations, delivery attempts, telemetry, and derived indexes likewise remain outside the Room Commit.

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
2. the room actor verifies Membership/Cursor, registers the Session as Attaching, and captures the complete Room Head, frame head H, retained floor, Cursor, and a Session-specific sync token;
3. storage reads and sends frames in (cursor, H] using short bounded pages;
4. newly committed frame references above H enter the session's bounded buffer;
5. on first attach, when the retained range is below its floor/pruned, or when visibility loss or another condition makes incremental delivery inappropriate, the server sends a full authorized Projection Reset at the captured Room/frame baseline;
6. only after the client installs the through-H range/reset and sends `room.sync_ack` for H with that Session's sync token does the actor atomically switch it to Live and flush buffered frames in order;
7. buffer overflow closes the connection and requires another attach.

The client deduplicates by room, member, and frame sequence. An action retry uses its independent action ID.

Multiple Sessions attached to one Membership share that Membership's Observation Stream and Cursor. A distinct `observation.ack` from any authorized Session may advance the shared Cursor but cannot satisfy a synchronization token; `room.sync_ack` changes only the issuing Session's barrier state and never advances Cursor. Independent delivery consumers require separate Memberships.

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
| Room Admission Lane | 256 positions, with a configured host-stimulus reserve |
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
- current authorized Projection, its schema, and its sole ordered ActionOfferV1 list;
- the complete exact Room Head, Projection hash, and Membership/integrity/policy/authority/delivery witnesses;
- exactly one of retained frames after the Membership Cursor or a Projection Reset baseline;
- explicit artifact references authorized for that membership;
- versioned, bounded runner budget and execution limits selected for the grant;
- lease expiry.

Claim grant/reclaim is one conditional database transaction. Claim, renew, release, and complete each use an independent operation ID and Canonical Request Hash and retain an immutable original result code/hash. An identical control or non-grant retry returns that result. An identical granted-claim retry returns the exact original context/result bytes only while its context is retained; after tombstoning it deterministically returns wire `result_retired` while the original result code/hash and context hash remain unchanged. Renew, release, and complete conditionally match the authenticated Runner, current claim ID, current generation, and unexpired lease. An expired older claim can never complete a later lease. Archive and affected Membership/Access/Role changes cancel and generation-fence pending/leased intents; capability revocation applies immediately.

v0.1 supports a runner control WebSocket and HTTP long poll. It does not call arbitrary user URLs. A webhook is not a committed v0.2 feature.

## Snapshots, recovery, and replay

### Snapshot policy

Defaults:

- create one paired Core-and-Activity snapshot every 250 accepted Transitions or five active minutes, whichever occurs first, in an idempotent postcommit job;
- retain immutable Genesis independently and the latest three automatic pairs; an optional sequence-zero pair is only a cache;
- keep canonical Transitions and all Semantic Receipts for the Room lifetime in frozen releases;
- retain acknowledged observation frames for a seven-day safety window, subject to a hard per-Membership ceiling of 10,000 frames or 64 MiB that forces an explicit Projection Reset without moving the Cursor;
- never use snapshot deletion to change canonical history.

### Room load

1. Read Room Integrity State/generation, immutable Genesis, and the complete Room Head.
2. Verify Genesis, its three initial state hashes, Genesis hash, exact Core schema and pinned PackRevisionLock identities, runnable executor/codecs, and normalized initial timers.
3. Find the newest compatible paired snapshot at or before Head. Verify both canonical state values, both component hashes, aggregate hash, and applicable lineage hash; discard the whole pair on any mismatch.
4. If no pair verifies, reconstruct initial Core with the exact Core schema and initialize Activity State/timers with the exact pack from Genesis.
5. Replay every later Transition through the same versioned Core reducer and exact pack reducer.
6. At each sequence verify prior lineage, normalized Stimulus/ordered outputs, Transition hash, and Core, Activity, and aggregate hashes.
7. Reconstruct timer consistency and compare the final complete Head plus current Core/Membership/Activity materializations.
8. Install serving state only under the current integrity generation. A healthy result may become Active; detected canonical disagreement moves the Room to quarantined, while an intact Head with an unavailable/unsafe runtime moves it to faulted.

A corrupt newest snapshot can be skipped in favor of an older verified pair or Genesis. Deleting every snapshot and current materialization must still permit full recovery. Missing canonical Transitions or corrupt Genesis quarantine the Room; an unavailable exact Core or pack executor/codec faults it until that exact dependency is reinstalled and verified. A newer pack revision may not substitute.

Room load, Replay, and catch-up MUST NOT hold a long database snapshot or read transaction. Each operation captures an immutable upper bound H, then pages append-only rows with short read transactions using sequence greater than the prior page and less than or equal to H. Page size, total duration, and concurrent Replay count are bounded. SQLite reports oldest-reader age and checkpoint blockage; PostgreSQL reports equivalent pool/snapshot pressure without changing behavior. Expensive Replay is throttled before it threatens mutation durability.

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

Room Integrity State is operational and never folded into Replay:

- healthy may advance;
- faulted means the last canonical Head verifies but the runtime cannot safely advance it;
- quarantined means canonical integrity cannot be established.

Every state change increments the integrity generation and appends an incident/repair record. Every new Existing Advance or durable disposition conditionally matches healthy plus the generation captured during preparation. Create instead installs `healthy` at generation `1` without a pre-existing integrity witness. Faulted/quarantined Rooms append no canonical participant or administrative Transition. Operational capability revocation, diagnostics, raw export, restore, and verification remain available.

An authenticated host operator may request repair but cannot clear integrity. The verifier may rebuild materializations/caches, reinstall the exact executor, or restore exact canonical bytes from a verified backup. Only a successful result conditioned on the current generation sets healthy, and it never edits, skips, reorders, synthesizes, or replaces Genesis/Transitions. A restored Room verifies healthy before archive or Membership mutation.

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

The verifier is read-only and never samples, repairs, fires timers, delivers Frames, or starts Activations. It validates the backup/export ID, Storage Epoch and lineage, engine and compatibility manifest, schema fingerprint and migration checksums, artifact sizes/digests/bytes, Operation Identities/Canonical Request Hashes/dispositions/Semantic Receipts, exact timers, Frames/Cursors, Activation Intents and every operation receipt, the context-retention discriminator and matching retained Invocation Context bytes or versioned tombstone, request/result/context hashes, lease and witness generations, claims/fences, and availability of every exact retained Activity Pack executor. For every Room recorded healthy it reconstructs Genesis through Head and verifies every Transition, state, Core/Activity materialization, and Head hash.

A global lineage, schema, manifest, artifact, or cross-Room authority failure blocks deployment readiness. A Room already recorded as faulted or quarantined may be copied byte-for-byte, remain isolated and unhealthy, and not block otherwise verified Rooms. Any mismatch newly introduced by backup, restore, or transfer aborts verification.

### One-way offline SQLite-to-PostgreSQL transfer

The only supported backend transfer is a versioned, resumable, whole-deployment, offline move from authoritative SQLite to an empty PostgreSQL target. The deterministic transfer bundle records source lineage, export identity, Storage Epoch, schema and codec versions, ordered chunks, row/object counts, per-chunk and whole-export digests, and a semantic fingerprint.

Canonical serialized bytes are copied verbatim, never decoded and re-encoded through PostgreSQL JSON, timestamp, numeric, or text types. The bundle preserves Genesis, Transitions, every Head/hash, Core and Activity materializations, Memberships and authority, exact timer IDs/generations/`scheduled_for` values, Frames/Cursors, Operation Identities/Canonical Request Hashes/dispositions/Semantic Receipts, Activation Intents and every Activation operation receipt, each context-retention discriminator with its matching retained Invocation Context bytes or versioned tombstone, request/result/context hashes, lease and witness generations, claims/audit/fences, principals/capabilities/revocations, integrity incidents, and artifact metadata and bytes. Snapshots, indexes, caches, telemetry, Sessions, Runner presence, in-memory mailboxes, delivery attempts, and temporary state are invalidated or rebuilt.

Transfer is two-phase:

1. quiesce the sole WorldStream process, create and verify a recoverable SQLite backup, and durably mark the source `transfer_pending` under its current Storage Epoch;
2. export/import resumable deterministic chunks into the empty target while neither backend serves, fence every nonterminal Activation lease, run PostgreSQL-native checks and the full WorldStream verifier, then require an explicit finalize command to retire SQLite and make PostgreSQL authoritative at the next Storage Epoch.

Recorded scheduled timers keep their IDs, generations, and `scheduled_for` values. Startup enters the ordinary fixed-cutoff CatchingUp path and invents neither a fire nor a generation during transfer. Eligible Activation Intents remain reclaimable after old leases are fenced.

Before source retirement, abort discards the target and leaves the verified SQLite source authoritative. After PostgreSQL accepts its first write under the new epoch, returning to SQLite is not supported continuity. The retired SQLite deployment is a read-only recovery artifact; reopening it for writes requires an explicit destructive override. There is no live switch, dual write, reverse or room-at-a-time transfer, automatic fallback, or consensus protocol: the offline authority fence is sufficient only because serving is quiesced.

Transfer and recovery race resolutions are frozen independently of adapter implementation:

| Race or failure | Required resolution |
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
- per-IP, principal, capability, session, membership, room, activation-operation,
  and operator-endpoint rate limits;
- bounded payloads, state, pack output, mailboxes, and queues;
- separate projection types and adversarial privacy tests;
- origin allowlist and no credentialed wildcard CORS;
- output escaping and safe artifact content disposition;
- no payload bodies, tokens, private projections, or artifact contents in default logs;
- no chain-of-thought collection;
- no arbitrary outbound network requests;
- no pack-supplied JavaScript.

Gateway admission uses one shared process-local token-bucket module at the
HTTP and WebSocket transport seams. Every HTTP request consumes the source-IP
bucket before routing. Authenticated HTTP operations then consume the verified
authority principal, the presented capability's stable token hash, a closed
operator-operation class where applicable, and any explicit Room/Membership
target before the semantic backend operation. Runner-Capability issuance is
one operator-control admission: its bounded target vector remains an authority
payload and is not expanded into hundreds of transport buckets. A WebSocket
upgrade first reserves one global pending permit. After authentication it is
atomically converted into global and per-Principal active permits; RAII release
on every exit prevents abandoned upgrades from leaking capacity. Browser
ticket presentation is bounded to 15 seconds, the first `client.hello` to 10
seconds, and a welcomed connection with no inbound traffic to 90 seconds even
while server heartbeats continue.

Every inbound WebSocket message, including malformed and non-text messages,
consumes IP, verified Principal, capability, and server-generated Session
buckets. Valid targeted messages also consume the composite `(Room ID, Member
ID)` Membership bucket and Room bucket before dispatch. Claim, renew, release,
and complete messages additionally consume a key composed from verified
Principal, capability hash, and a closed activation-operation class; no
attacker-selected claim or activation ID creates a global key. Source IP is
the socket peer supplied by the listener; forwarded-address headers are never
trusted. If peer metadata is unavailable, all such traffic shares one
conservative unattributed bucket.

The reference limits are fixed release policy:

| Scope | Burst | Refill | Maximum live keys |
| --- | ---: | ---: | ---: |
| IP | 1,024 | one token / 2 ms | 4,096 |
| Principal | 512 | one token / 4 ms | 4,096 |
| Capability | 256 | one token / 8 ms | 8,192 |
| Session | 256 | one token / 8 ms | 8,192 |
| Membership | 128 | one token / 16 ms | 8,192 |
| Room | 512 | one token / 4 ms | 8,192 |
| Activation operation | 64 | one token / 32 ms | 8,192 |
| Operator endpoint | 128 | one token / 16 ms | 8,192 |

Presented dimensions are checked atomically within each admission decision.
Limiter state retains only BLAKE3 fingerprints keyed by a fresh process CSPRNG
key; raw IPs, bearer material, and Principal, Capability, Session, Membership,
or Room IDs are not retained in limiter keys. Target buckets require a
verified Principal and are paired with an atomic live association budget of at
most 256 distinct Room/Membership targets per Principal and 65,536 total
associations. Existing presented associations remain live when their quota is
exhausted, so repeated rejection cannot age out ownership while retaining the
global target bucket. Idle keys and associations expire after ten minutes.
The connection budgets are 256 pending, 4,096 active globally, and 64 active
per Principal.

A key/association/connection store at capacity, invalid identity material,
poisoned state, or clock regression rejects new work as the same generic
retryable `rate_limited`; it never bypasses admission. Rate-limit metrics use
only the fixed `scope` labels `ip`, `principal`, `capability`, `session`,
`membership`, `room`, `activation`, `operator`, `capacity`, `target_capacity`,
`pending_connection`, `active_connection`, `principal_connection`,
`invalid_identity`, and `clock`. They never expose entity identifiers or
fingerprints.

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
- Room Admission Lane depth, host-stimulus reserve use, and busy rejections;
- accepted, rejected, duplicate, conflicting, known-absent, and indeterminate operations;
- transition commit p50/p95/p99;
- observation frame bytes, backlog, redelivery, and slow-consumer closes;
- activation pending count, oldest age, lease expiry, and completion;
- timer lag, overdue obligations, and CatchingUp duration/slices;
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

Logs are structured JSON with W3C trace correlation. They may include Room, Membership, Action, and Transition IDs only in access-controlled logs/traces; those IDs are never metric labels. Logs exclude Action/Observation bodies, capabilities, private observations, prompts/model output, artifact bytes, DSNs, and credentials. Prometheus metrics and the optional OpenTelemetry/OTLP exporter are vendor-neutral seams; no vendor SDK, account, collector, or credential participates in admission, reduction, commit, Replay, Room Integrity State, or readiness.

Telemetry happens after authoritative commit through bounded nonblocking queues while holding no Room or database lock. Overflow drops telemetry, increments a metric, and emits a rate-limited warning. Shutdown grants at most three seconds to flush and never delays or changes an authoritative result.

## Release, platform, and evidence matrix

This section implements [ADR 0004](adr/0004-supported-storage-profiles-and-offline-portability.md) and [ADR 0011](adr/0011-release-compatibility-recovery-and-supply-chain-gate.md).

Reviewed [`compatibility.toml`](../compatibility.toml) is the authored source and canonical [`compatibility.json`](../compatibility.json) is its semantically identical, sorted-key mirror. It contains the portable runtime contract and closed release subject inventory. Exact final archive, image, evidence, SBOM, and provenance digests are deliberately detached into `release-manifest.json`; compiling those values into the subjects they hash would be self-referential. The final Sigstore bundle is path-only verification material because it authenticates that manifest and cannot contain its own digest. The pre-sign inventory bundle is transitively digest-bound through the signed supply-chain report. Validation fails closed on every unresolved embedded contract field and every missing, extra, or mismatched detached subject.

A real release must generate and review a populated contract pair, prove semantic parity, set `release_ready = true` only when every embedded implementation identity is resolved, embed the JSON in every binary, and publish it with the release. The release is verified only when the detached signed inventory, SBOM, provenance, and every hard-gate evidence subject verify against those exact final bytes. Startup, `doctor`, `/version`, backups, transfer bundles, release notes, and CI consume the embedded contract; release verification consumes the detached evidence bundle. The v0.1.0 contract identifies product `0.1.0`, wire `0.1`, config `1`, storage schema `1`, Core schema version `worldstream.core-room-state.v1`, and hash suite `blake3-canonical-json-v1` in addition to the engine/toolchain versions frozen above.

| Profile | Supported/release contract | Mandatory evidence |
|---|---|---|
| Native Linux | `x86_64-unknown-linux-musl`, kernel 5.15+, local ext4/XFS for SQLite; Ubuntu 24.04 x86-64/ext4 reference | Native build/archive, both storage profiles, migration/transfer/restore/Replay, filesystem/ACL/disk-full and performance reference |
| Native Windows | `x86_64-pc-windows-msvc`; Windows 11 25H2+ or Server 2022/2025; fixed NTFS/ReFS | Native build/package plus ACL/filesystem, bundled SQLite, PostgreSQL connection, backup/restore and recovery tests; cross-compilation alone fails the gate |
| OCI | Linux/amd64 only, static/minimal, non-root UID 65532, read-only-root compatible, persistent `/var/lib/worldstream` | Image-by-digest test, both storage profiles, persistent-volume enforcement, no writable-overlay SQLite |
| macOS quickstart | Source build on macOS 15+ APFS, Intel and Apple Silicon, development/default quickstart | Fresh source-build deterministic Heist quickstart; no binary/archive gate |

Each native archive contains `worldstreamd`, `worldstreamctl`, embedded UI, examples/Heist clients, licenses, and compatibility manifest. Published evidence includes source, checksums, Sigstore signatures, SPDX SBOM, and SLSA provenance. ARM64 release artifacts, macOS binary distribution, Windows containers, MSI/MSIX, Windows Service integration, package repositories, Kubernetes/Helm assets, and cloud resources are not delivered.

| Evidence tier | Bound and required scope |
|---|---|
| Fast hook | Warm p95 at most 20 seconds and hard 60 seconds; changed format/config/schema/golden/secret checks; no network or container dependency |
| Pre-push | Warm target 6 minutes, cold target 15 minutes; lint/unit, SQLite, SDK/UI/protocol/hash plus local PostgreSQL smoke; a visible local skip is incomplete and remote CI may not skip |
| Minimal CI | Target 15 minutes, hard 25 minutes; parallel Linux plus focused native Windows build/package/ACL/filesystem/SQLite/PostgreSQL-connect/recovery |
| Release | Target 3 hours, hard 4 hours; every artifact/platform, signature/SBOM/provenance, full backend conformance, all prior migrations, transfer, isolated restore, verifier, failure/fuzz/benchmark suites, and one-hour SQLite soak |

The deterministic quickstart release target uses SQLite by default and requires no cloud account, paid model, or remote service; PostgreSQL is opt-in. Release evidence MUST demonstrate deterministic Heist in under five minutes on the documented Ubuntu reference of 4 vCPU, 8 GiB, and local SSD, and a fresh checkout in under ten minutes. The required PostgreSQL evidence harness MUST use the official PostgreSQL 17.11 image pinned by an exact digest in the release evidence, a unique project and disposable volume, loopback random port, SCRAM, non-superuser runtime role, separate direct-admin and transaction-pooler runtime DSNs, PgBouncer transaction pooling, and scoped teardown. These are configuration and evidence obligations, not claims satisfied by the current unwired, Counter-only SQLite Room/authority conformance adapter. Provider verification, when supplied, is optional, dated, and limited to a migration plus backup/isolated-restore/full-Replay drill for the exact combination; it asserts no HA, SLA, durability, plan, region, or provider service.

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

The benchmark report MUST attribute every measurement source to its own observed hardware and filesystem facts; it MUST NOT merge acceptance metrics with host facts from a separate soak runner. Each source discloses the selected profile and every engine it actually observed with exact version/settings/connection mode; an unobserved engine is labeled explicitly rather than inferred from another host. The report also discloses payload sizes, pack, participants per room, fan-out, snapshot cadence, p50/p95/p99 latency, process memory, database growth, and recovery time. Performance targets are reference measurements, not universal release blockers, SLAs, or Windows performance claims; correctness, durability, crash recovery, resource bounds, and hash parity remain hard gates on every supported profile.

### One-process optimizations allowed

1. Passivate idle room actors.
2. Prune Observation Frames under the seven-day acknowledged safety window and hard per-Membership 10,000-frame/64-MiB ceiling, always forcing an explicit Reset when the required range is unavailable.
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
3. Immutable Genesis plus the exact Core/pack revisions and Transitions is sufficient after every paired snapshot and current materialization is deleted.
4. Durable database COMMIT is the only Room-write linearization point; no Create, accepted Action, or stable disposition is acknowledged or published before it.
5. Every Operation Identity maps to at most one Canonical Request Hash and Semantic Receipt; same identity with changed semantic input is Conflict.
6. Every commit branch and resolve takes the same Operation Identity guard first; unknown COMMIT is resolved through that synchronized original identity/hash before retry, reprepare, scan, acknowledgement, or publication, and an unguarded snapshot miss never proves absence.
7. Only a stable fenced Rejection or administrative NoChange consumes an identity without changing canonical Room history; transient admission, authority, capacity, integrity, storage, and runtime faults do not.
8. Every new Existing Room write fences the Complete Head plus integrity, authority/capability, policy, and operation-specific input witnesses. A Participant Action is never rebased. Creation instead fences its administration identity, creation authority, and generated Room-ID absence; it has no basis Head or pre-existing Room Integrity fence and atomically installs Genesis, Head zero, materializations, operational integrity `healthy`/generation `1`, and receipt.
9. A timer identity is exactly `(room_id, timer_id, generation)`; `scheduled_for` and payload are immutable request-hash and witness inputs, not identity fields.
10. The bounded Room Admission Lane is provisional and fair to due host stimuli; database COMMIT alone determines canonical order.
11. Loading with overdue timers uses one fixed HostClock cutoff and becomes Active only after all applicable obligations through it drain in deterministic order.
12. Timer generations are host-owned and never reused, have no pack-visible `fired_at` or separate durable claim, and their exact scheduled-generation witness is consumed in the Advance.
13. Core and Activity State, all three state hashes, and lineage at sequence N are reproducible from Genesis and typed recorded Stimuli through N without HostClock or a scheduler.
14. A pack cannot observe ambient nondeterminism.
15. Authoritative Room State never crosses the client boundary directly.
16. Authorization precedes observation persistence and artifact access.
17. Every durable viewer is a Membership with exactly one addressed Observation Stream, never-reused frame head, retained floor, and shared Cursor; Genesis emits no frame and a later Transition emits at most one frame per viewer.
18. Membership outlives Sessions and Invocations.
19. Activation is at-least-once intent delivery, not proof of model execution.
20. An expired or superseded Activation claim cannot renew, release, or complete a later lease generation; every Activation control operation has a durable idempotent receipt.
21. Observation delivery is at least once; clients deduplicate and acknowledge.
22. The Observation Catch-up/live actor barrier returns the complete retained authorized range or an explicit Projection Reset; only matching `room.sync_ack` enters Live without a handoff gap and never advances Cursor, while only separate `observation.ack` may advance Cursor.
23. Paired Core+Activity snapshots are idempotent postcommit caches; snapshots, current materializations, indexes, and projection caches are replaceable derivations.
24. Every new Existing Advance or durable disposition fences on `healthy` plus an unchanged integrity generation; Create initializes `healthy` at generation `1`. Canonical disagreement quarantines, while an intact Head that cannot safely advance faults.
25. Replay has no external effects, applies present-plus-historical authorization, and holds no unbounded database read transaction.
26. Mutating HTTP resources and their Semantic Receipts commit atomically.
27. A committed artifact reference points only to bytes made durable before the linking transaction.
28. Passivation is generation-fenced; no command is routed to an actor that may disappear.
29. Slow clients and full queues cannot create unbounded memory growth.
30. Investigation-specific semantics stay outside the Room Kernel; only the preplanned generic artifact subsystem is added.
31. v0.1 and v0.2 run exactly one WorldStream process with exactly one startup-selected supported storage profile.
32. Core Room State is exactly Room Status plus the semantic Membership map; Room Integrity State is operational.
33. The Core reducer alone mutates Core; a pack may veto only individual or homogeneous all-vetoable Join, Resume, Access Mode, and Role proposals.
34. Multi-Membership administration rejects mixed veto classes before pack entry and otherwise validates and commits or rejects one homogeneous final state without an observable invalid intermediate.
35. Departed Membership and archived Room status are irreversible.
36. Only a generation-fenced verifier may restore healthy integrity.
37. Repair never rewrites, skips, or replaces canonical lineage.
38. `ActivityPackV1` has exactly `descriptor`, `initialize`, `reduce`, `view`, and `observe`; no pack callback runs during persistence.
39. Every retained pack digest remains executable and codec-complete; no Room digest changes in place.
40. One exact Action Offer representation supplies Projections, Resets, Observations, Invocation Context, and admission.
41. Both storage profiles preserve identical canonical bytes, Room Commit resolution classes, receipts, timers, Frames/Cursors, Activation fences, recovery, and Replay.
42. A Storage Epoch identifies the sole authoritative deployment lineage; offline transfer advances it only at explicit verified finalization.
43. Backend-native restore or SQLite-to-PostgreSQL transfer is never ready before the full WorldStream semantic verifier passes, except that exactly preserved pre-existing unhealthy Rooms remain isolated.
