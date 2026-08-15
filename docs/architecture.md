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
    ST --> DB["Durable storage adapter"]
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
| Durable storage seam | Backend-neutral Room Commit port | One semantic contract with backend-specific transaction fencing |
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

Not selected for v0.1 or v0.2: an ORM, Redis, NATS, Kafka, Temporal, Wasmtime, Kubernetes, an embedded model SDK, or a frontend realtime platform. Concrete storage profiles and drivers are frozen by the separate deployment/release profile contract; every selected adapter MUST preserve this document's Room Commit semantics.

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

The storage interface belongs in worldstream-core. Every selected storage adapter implements the same logical Room Commit and resolution port; providers do not alter semantics. There are no broker, crypto, workflow, plugin, or generic connector crates.

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
             ↘ Faulted or Quarantined ↙

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

The actor MUST expose one bounded Room Admission Lane for Participant Actions, canonical administration, and newly due timer candidates. A successful reservation and the applicable host time sample are one admission operation. Reserved host-stimulus capacity prevents participant saturation from starving timers; an earlier reservation cannot be overtaken, and once a timer reserves a position later participant traffic cannot pass it. Backpressure reaches the gateway as `room_busy` without Semantic Time, receipt, or deadline entitlement; it does not create more actor tasks for the same Room.

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

Validation and Activity reduction occur in the one `reduce` call so they cannot disagree after state changes. For a non-Core Stimulus, Core before and proposed after are equal. `view` returns the pack-owned Activity Projection plus exact Action Offers, and `observe` returns a bounded pack-owned Activity Observation for one viewer. WorldStream wraps Activity Projection with authorized Core Room and Membership facts to form a Projection; protocol envelopes add causal sequence, operational Room Integrity State/generation, schema, and delivery metadata.

Packs request ScheduleNext, CancelCurrent(expected_generation), or RescheduleCurrent(expected_generation, new due/payload). WorldStream owns, assigns, and verifies monotonic timer generations and strict-forward semantic time.

view returns one authorized Activity Projection and ordered canonical Action Offers. Those same bytes are used by reset, observation, Invocation Context, and host pre-admission. observe receives before/after Core and Activity, normalized Stimulus, ordered events, exact viewer, and exact after-view bytes; it returns zero or one viewer result. A changed authorized view with no observation is PackFault.

WorldStream wraps the pack projection with authorized Core facts. Operator Membership never receives raw Activity State. Callback panic where catchable, malformed output, bound violation, mandatory-Core veto, privacy/view failure, or deterministic disagreement fails closed before commit.

PackRegistryV1 maps each PackRevisionLockV1 semantic digest to the exact executor, descriptor/schemas, codecs, golden digest, and selectable/runnable status. Selectable implies runnable; every retained digest remains runnable even when non-selectable. A Room never changes digest or rewrites Activity State in place. Missing retained executor/codec is an explicit compatibility failure.

Same-process Rust is trusted, not sandboxed. Dynamic/public pack loading and a portable plugin ABI remain unsupported. See [ADR 0010](adr/0010-activity-pack-v1-and-executable-replay-retention.md).

### Storage service

Room actors submit immutable `PreparedRoomCommit` values through one bounded backend-neutral port. SQLite uses its dedicated writer and a transaction-start write reservation; PostgreSQL uses a transaction-scoped Room-root row lock or an equivalent guarded write under Read Committed. Those physical mechanisms are adapter details and MUST expose the same outcomes, canonical bytes, hashes, and crash semantics.

Storage adapters MAY use separate bounded read connections for Room loading, Catch-up, Replay, receipt resolution, and host-operator queries. A read made during preparation is only a witness; every fact that authorizes a new write is repeated under the Room transaction fence. Returning an already stored result is read-only and does not reacquire the fence.

The storage service owns:

- forward-only migrations;
- atomic Room Advances and durable dispositions;
- Operation Identity and Semantic Receipt resolution;
- snapshot and transition reads;
- observation-frame and activation queries;
- cursor acknowledgement persistence;
- WAL checkpoint control and metrics;
- backup and integrity-check operations.

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

WorldStream, not the Activity Pack, owns each monotonic Timer Generation. A pack requests exactly one normalized `ScheduleNext`, `CancelCurrent(expected_generation)`, or `RescheduleCurrent(expected_generation, new_scheduled_for, new_payload)` mutation per logical Timer ID; the host assigns a never-reused next generation. The generation's Scheduled Time, payload, and creation cause are immutable. A new schedule MUST be strictly later than the causing Stimulus's typed Semantic Time; an initial Genesis timer MUST be strictly later than the typed recorded creation time.

The scheduler uses HostClock only to discover that a generation is due. It reconstructs TimerFired from the immutable `(room_id, timer_id, generation, scheduled_for, payload)` record, adds no `fired_at`, and takes no durable claim. The exact scheduled-generation witness is consumed by the same Advance as its Transition. Retry/restart therefore cannot change semantic input or create a second firing.

Within one Room, due candidates are considered one at a time in `(scheduled_for, timer_id, generation)` order and reread after every outcome. All still-scheduled due generations remain obligations; they are never expired, merged, coalesced, skipped, or marked fired because of lag or a processing budget.

Timer mutation validation is exact:

| Pack request | Valid precondition | Atomic result |
|---|---|---|
| `ScheduleNext(timer_id, scheduled_for, payload)` | No generation is currently scheduled; time is strictly later than the causing Stimulus's Semantic Time | Host allocates the next never-used generation and records its immutable schedule |
| `CancelCurrent(timer_id, expected_generation)` | That exact generation is scheduled | Current generation becomes cancelled in the causing Advance |
| `RescheduleCurrent(timer_id, expected_generation, new_scheduled_for, new_payload)` | That exact generation is scheduled and new time is strictly forward | Current generation is cancelled and the next generation is created atomically |

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
    participant S as "Storage adapter"
    participant D as "Durable database"

    C->>G: action.submit with action_id and based_on_seq
    G->>R: authenticated, strictly parsed request
    R->>S: resolve(identity, request_hash)

    alt Existing same-hash resolution
        S-->>R: original stored result
        R-->>C: same accepted or rejected result
    else Same identity, changed hash
        S-->>R: Conflict
        R-->>C: idempotency_conflict
    else Known absent
        R->>R: rate/size admission, reserve lane + sample admitted_at
        alt Lane unavailable
            R-->>C: room_busy, no admitted_at or receipt
        else Admitted
            R->>P: reduce and construct projections/frames outside locks
            R->>R: seal immutable PreparedRoomCommit
            R->>S: commit(prepared)
            S->>D: fence Room, recheck identity and all witnesses, write bundle
            alt Database COMMIT confirmed
                D-->>S: committed
                S-->>R: Resolved(New)
                R->>R: install committed Head and state
                R-->>C: stored semantic result
                R-->>C: postcommit publication
            else COMMIT status unknown
                D-->>S: ambiguous
                S-->>R: Indeterminate
                R->>S: resolve(same identity, same request_hash)
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
| Storage/integrity failure prevents known outcome | Fail closed and expose the precise known-absent, Indeterminate, or Fenced class; do not collapse them into generic retry |
| Activation lease holder fails before completion | The generation-fenced lease expires; the same pending intent may be claimed again |
| Pack panic or deterministic projection failure | No write; discard the actor and reload or quarantine through integrity handling |

## Backend-neutral logical records

The following records and semantic constraints are frozen. Their field lists illustrate the logical contract, not adapter SQL, physical statement order, index syntax, or provider-specific types. Adapter migrations may differ physically only where the shared Room Commit and conformance contract remains identical.

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

`stimulus_json` carries exactly one typed Semantic Time; there is no universal transition `logical_time`.

### action_receipts

    room_id TEXT
    member_id TEXT
    action_id TEXT
    codec_version TEXT
    request_hash BLOB
    basis_complete_head_json BLOB
    admitted_at TEXT
    result_status TEXT CHECK result_status IN ('accepted', 'domain_rejected')
    transition_seq INTEGER NULL
    semantic_result_json BLOB
    committed_at TEXT
    PRIMARY KEY (room_id, member_id, action_id)

The stored request hash detects same-identity/different-semantic-input Conflict. The receipt retains its exact basis Complete Head, original `admitted_at`, and original result. Transient admission/runtime errors never enter this record and do not consume an Action ID.

### mutation_receipts

Durable idempotency for mutating HTTP administration:

    principal_id TEXT
    operation TEXT
    idempotency_key TEXT
    codec_version TEXT
    request_hash BLOB
    basis_complete_head_json BLOB
    result_status TEXT CHECK result_status IN ('accepted', 'rejected', 'no_change')
    transition_seq INTEGER NULL
    semantic_result_json BLOB
    committed_at TEXT
    PRIMARY KEY (principal_id, operation, idempotency_key)

The Semantic Receipt commits with an Advance, stable Rejection, or administrative NoChange. Same identity and request hash returns the original response; a changed request hash is a Conflict. `worldstreamctl` generates capability bearer secrets locally and sends only their derived token hash, so retrying capability creation never requires the server to store or replay plaintext.

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
    request_hash BLOB
    claim_id TEXT NULL
    lease_generation INTEGER
    result_code TEXT CHECK result_code IN ('granted', 'not_available', 'expired', 'cancelled', 'fenced', 'completed', 'released', 'renewed')
    lease_until TEXT NULL
    result_json BLOB
    invocation_context_hash BLOB NULL
    invocation_context_blob BLOB NULL
    created_at TEXT
    PRIMARY KEY (activation_id, operation_kind, operation_id)

For claim, `operation_id` and `claim_id` are the same value; later control operations have a new `operation_id` and name the active `claim_id`. The server derives `request_hash` from the complete authenticated request. A repeated operation ID from the same authenticated Runner and identical canonical request returns its exact stored result. Reusing it with a changed request or Runner is an idempotency conflict. Renew, release, and complete match the current claim ID, Runner ID, generation, and unexpired lease. Exact granted Invocation Context is retained according to its privacy window, then replaced by a tombstone; later retry returns `result_retired` rather than regenerated bytes.

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

## Backend-neutral atomic Room Commit

### Contract surface

The storage port exposes exactly two semantic operations for a prepared Room write:

    commit(PreparedRoomCommit) -> CommitOutcome
    resolve(OperationIdentity, CanonicalRequestHash) -> ResolveOutcome

`commit` accepts one already-admitted, fully computed write for exactly one Room. `resolve` performs no domain work and returns exactly one of:

- `StoredResolution`, containing the original Semantic Receipt for the same identity and hash;
- `Conflict`, when that identity is durably bound to another hash;
- `KnownAbsent`, when an authoritative-primary read proves no resolution exists; or
- `ResolutionUnavailable`, when storage cannot yet prove stored versus absent.

Room archive and accepted Membership Standing, Access Mode, or Role changes are Core Stimuli and consume the next Room sequence. Their receipt, Core and Activity results, Domain Events, hashes, timers, addressed Frames, and allowed Activation decisions commit atomically with that Transition. Archive also cancels scheduled timers and fences pending/leased Activation work. Session presence, Runner availability, Activation lease operations, integrity incidents/repair, diagnostics, and telemetry remain operational and never consume Room sequence, following [ADR 0002](adr/0002-sequence-domain-relevant-room-changes.md) and [ADR 0005](adr/0005-canonical-core-state-integrity-and-hash-lineage.md).

A new write has exactly one prepared intent:

- `Advance`: one canonical Transition and every durable consequence caused by it; or
- `DurableDisposition { Rejection | NoChange }`: one stable semantic result and identity fence with no Transition.

Room creation, Observation acknowledgements, Activation claims/leases/delivery attempts, capability management, integrity repair, backup/restore, snapshots, telemetry, and derived indexing use separate transactions. One Room Commit never spans Rooms.

### PreparedRoomCommit v1

Preparation seals all bytes that the transaction may persist. The value contains:

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
8. immutable `PreparedRoomCommit` sealing.

No Activity Pack call, reducer, canonicalization, hash computation, Projection/Frame construction, network publication, telemetry export, derived-index work, or snapshot write may run while storage locks are held. The transaction may validate and persist only bounded prepared bytes and witnesses.

### Guard and write order

Every new Advance or durable disposition follows this exact guarded order inside one transaction:

1. Acquire the transaction-scoped Room write fence.
2. Recheck Operation Identity. Same identity/hash returns the stored resolution without writing; same identity/different hash returns `Conflict` without writing.
3. Compare every Complete Head field and verify healthy plus unchanged integrity generation.
4. Revalidate the full authority/capability witness and revocation state.
5. Revalidate the policy revision when present and the complete operation-specific input/timer witness.
6. For an `Advance`, persist, in logical dependency order:
   1. the Transition, prior/Transition hash chain, and resulting Core, Activity, and aggregate Authoritative hashes;
   2. the new Complete Head and verified current Core/Activity serving materializations;
   3. the final Membership materialization for the complete atomic changeset, with no visible invalid intermediate;
   4. exact timer candidate consumption plus normalized schedule/cancel/reschedule changes;
   5. addressed Observation Frames and each affected stream's frame head;
   6. activation-policy revision/decision, permitted Activation Intents, and required Membership/archive eligibility and lease-generation fences; and
   7. the Semantic Receipt for the applicable Action, administration, or external input.
7. For a `DurableDisposition`, persist only its Semantic Receipt after all applicable guards pass; do not mutate Head, state, timers, Frames, or Activation.
8. Issue durable database `COMMIT`.

Archive and any final Membership state that is no longer an enabled Agent Participant with participant Access Mode and a current Role cancel and generation-fence that target's pending/leased Activation work inside step 6. This includes suspension, departure, Role removal, and participant-to-spectator/operator changes; the transaction never leaves newly ineligible work claimable.

Adapters may arrange bounded physical statements around backend constraint mechanics only when failure injection proves the same guard precedence, all-or-none bundle, and externally invisible intermediate state. SQLite maps the fence to its dedicated writer and transaction-start write reservation; PostgreSQL maps it to a Room-root row lock or an equivalent guarded write under Read Committed. Neither adapter may weaken or add a semantic outcome.

Database COMMIT is the sole linearization point. A conditional row change, lock acquisition, driver return, actor-memory installation, acknowledgement, or live publication is not public success and does not order the Room. Authority revocation, archive, Action, and TimerFired races are ordered by their durable commits.

### CommitOutcome algebra

| Outcome | Exact meaning | Permitted next action |
|---|---|---|
| `Resolved(TransitionCommitted { New })` | This attempt committed the prepared Advance. | Install returned Head/state, then acknowledge/publish. |
| `Resolved(TransitionCommitted { Existing })` | The same identity/hash already committed that Advance. | Return the original receipt; do not install speculative state or recommit. |
| `Resolved(RejectionRecorded { New | Existing })` | The stable Rejection was newly committed or already stored. | Return the original rejection; that identity is consumed. |
| `Resolved(NoChangeRecorded { New | Existing })` | The administrative desired state was already true and the stable NoChange was newly committed or already stored. | Return the original NoChange; no `room_seq` was consumed. |
| `NotApplicable` | An independently valid timer/input candidate is now missing, cancelled, consumed, or obsolete. | Stop; do not call the pack or create a rejection receipt. |
| `Reprepare` | Head or applicable policy revision changed and this plan is proven absent. | Discard the plan and follow the operation-specific reprepare rule below. |
| `Fenced` | Integrity/Health or operational authority no longer permits the write. | Stop with no Transition or receipt; require recovery or fresh authority as applicable. |
| `Conflict` | The identity exists with another Canonical Request Hash. | Return stable conflict; never retry under that identity. |
| `RetryableKnownAbsent` | Busy, deadlock, serialization, rollback, or equivalent failure proves no Advance/disposition committed. | A bounded retry may resubmit only the identical sealed plan. |
| `Indeterminate` | COMMIT may or may not have happened. | Resolve the same identity/hash on the authoritative primary before anything else. |
| `Fault` | The sealed plan is malformed or a structural/hash invariant is verified false. The write is known absent. | Do not retry; enter the defined fault/integrity path. |

Authentication, strict parsing/schema failure, rate/capacity admission, and pre-admission policy are outside this algebra because no `PreparedRoomCommit` exists.

`Reprepare` never means blindly re-execute the same decision:

- a Participant Action is never rebased; prepare a stable stale-basis rejection under the new exact Head when receiptable, otherwise return the applicable nonreceipt error;
- exact-head administration similarly recomputes only a stable stale/NoChange result permitted by its contract, never silently changes the requested basis;
- a timer first rereads its immutable generation: if it remains scheduled, reapply against the new Head with the same timer identity and recorded fields; otherwise return `NotApplicable`;
- a same-Head policy-revision change recomputes only the noncanonical policy decision/Activation portion before resealing.

Only `RetryableKnownAbsent` permits retry of an identical sealed plan. If a retry encounters changed witnesses, it returns the corresponding `Reprepare`, `Fenced`, or `NotApplicable`; the adapter never edits the plan.

### Semantic Receipts and durable dispositions

Every stored Semantic Receipt contains a codec/domain version, Operation Identity, Canonical Request Hash, exact basis Complete Head, original typed Semantic Time where applicable, and exactly one semantic result:

- accepted Transition identity, sequence, and resulting complete hashes;
- stable safe Rejection code and bounded details; or
- administrative NoChange code and bounded details.

It also records operational commit time for audit, but that timestamp is not canonical Room input. A retry renders a fresh transport envelope marked duplicate while preserving every original semantic field.

Receiptable participant results include stale basis, disabled Membership, illegal Action, expired deadline, archived/terminal Room, and expected pack-domain rejection. Receiptable administration results include stale basis, expected policy/cardinality rejection, and a valid already-satisfied request. `NoChange` is limited to administration whose normalized desired state was true before pack application; an empty Membership changeset consumes no sequence.

Malformed input, authentication/authority failure, rate/capacity rejection, unhealthy integrity, Activity Fault, storage failure, and timer obsolescence never become durable dispositions. Once a participant, timer, external, Membership, or administrative Stimulus is accepted by the reducer, it always creates a Transition even if resulting state bytes are unchanged.

Receipts or equivalent compact semantic tombstones remain resolvable while the Room lineage is retained, including after archive. Compaction may remove presentation-only bytes but must preserve the identity fence and equivalent result. Only an explicit whole-Room purge may remove history and its receipts together.

### Unknown COMMIT resolution and postcommit work

After persistence handoff, cancellation is advisory: the attempt must reach `Resolved`, proven absence, or `Indeterminate`. On `Indeterminate`, the caller repeatedly uses `resolve` against the authoritative primary:

1. `StoredResolution` returns the exact original result;
2. `Conflict` exposes identity misuse and stops;
3. `ResolutionUnavailable` preserves `Indeterminate` and retries resolution later without pack execution, scanning, acknowledgement, or publication;
4. only `KnownAbsent` proves the atomic transaction did not commit, after which the identical sealed plan may be retried within its bound or discarded/reprepared if its witnesses changed.

For a newly committed Advance, only the current actor generation installs the returned Complete Head and prepared in-memory state. A stale/dead actor acknowledges and publishes nothing; the supervisor reloads and callers resolve their identities. Failed Frame publication is recovered by Observation Catch-up, and failed Activation notification by scanning pending intents. Neither failure recommits.

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
5. if this is the first attach or the old range was pruned, the server sends a full authorized Projection Reset at the captured Room/frame baseline;
6. only after the client installs the through-H range/reset and ACKs H with that Session's sync token does the actor atomically switch it to Live and flush buffered frames in order;
7. buffer overflow closes the connection and requires another attach.

The client deduplicates by room, member, and frame sequence. An action retry uses its independent action ID.

Multiple Sessions attached to one Membership share that Membership's Observation Stream and Cursor. An ordinary acknowledgement from any authorized Session advances the shared Cursor, but cannot satisfy another Session's synchronization token. Independent delivery consumers require separate Memberships.

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
- allowed action kinds;
- current authorized projection;
- the complete exact Room Head, Projection hash, current Action Offers, and Membership/integrity/policy/authority/delivery witnesses;
- exactly one of retained frames after the Membership Cursor or a Projection Reset baseline;
- explicit artifact references authorized for that membership;
- lease expiry.

Claim grant/reclaim is one conditional database transaction. Claim, renew, release, and complete each use an independent operation ID/request hash and return a durable exact result. Retrying an identical operation returns that result. Renew, release, and complete conditionally match the authenticated Runner, current claim ID, current generation, and unexpired lease. An expired older claim can never complete a later lease. Archive and affected Membership/Access/Role changes cancel and generation-fence pending/leased intents; capability revocation applies immediately.

v0.1 supports a runner control WebSocket and HTTP long poll. It does not call arbitrary user URLs. A webhook is not a committed v0.2 feature.

## Snapshots, recovery, and replay

### Snapshot policy

Defaults:

- create one paired Core-and-Activity snapshot every 250 accepted Transitions or five active minutes, whichever occurs first, in an idempotent postcommit job;
- retain immutable Genesis independently and the latest three automatic pairs; an optional sequence-zero pair is only a cache;
- keep canonical transitions and action receipts for the room lifetime in frozen releases;
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
- Room Admission Lane depth, host-stimulus reserve use, and busy rejections;
- accepted, rejected, duplicate, conflicting, known-absent, and indeterminate operations;
- transition commit p50/p95/p99;
- observation frame bytes, backlog, redelivery, and slow-consumer closes;
- activation pending count, oldest age, lease expiry, and completion;
- timer lag, overdue obligations, and CatchingUp duration/slices;
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
2. Prune Observation Frames under the seven-day acknowledged safety window and hard per-Membership 10,000-frame/64-MiB ceiling, always forcing an explicit Reset when the required range is unavailable.
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

A future storage profile does not by itself authorize multiple live application writers. Only measured pressure should justify a later multi-process design with stateless gateways and one fenced owner lease per Room.

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

Do not add NATS, Redis, Kafka, Kubernetes, Raft, or multi-process ownership because they look scalable.

## Architecture invariants

1. One room has one pinned pack revision and one total committed order.
2. A committed sequence is never reused or decreased.
3. Immutable Genesis plus the exact Core/pack revisions and Transitions is sufficient after every paired snapshot and current materialization is deleted.
4. Durable database COMMIT is the only Room-write linearization point; no accepted Action or stable disposition is acknowledged or published before it.
5. Every Operation Identity maps to at most one Canonical Request Hash and Semantic Receipt; same identity with changed semantic input is Conflict.
6. Unknown COMMIT is resolved through the original identity/hash before retry, reprepare, scan, acknowledgement, or publication.
7. Only a stable fenced Rejection or administrative NoChange consumes an identity without changing canonical Room history; transient admission, authority, capacity, integrity, storage, and runtime faults do not.
8. Every new Room write fences the Complete Head plus integrity, authority/capability, policy, and operation-specific input witnesses. A Participant Action is never rebased.
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
22. The Observation Catch-up/live actor barrier returns the complete retained authorized range or an explicit Projection Reset; only the matching Session sync-token acknowledgement enters Live without a handoff gap.
23. Paired Core+Activity snapshots are idempotent postcommit caches; snapshots, current materializations, indexes, and projection caches are replaceable derivations.
24. Every canonical commit fences on `healthy` plus an unchanged integrity generation; canonical disagreement quarantines, while an intact Head that cannot safely advance faults.
25. Replay has no external effects, applies present-plus-historical authorization, and holds no unbounded database read transaction.
26. Mutating HTTP resources and their Semantic Receipts commit atomically.
27. A committed artifact reference points only to bytes made durable before the linking transaction.
28. Passivation is generation-fenced; no command is routed to an actor that may disappear.
29. Slow clients and full queues cannot create unbounded memory growth.
30. Investigation-specific semantics stay outside the Room Kernel; only the preplanned generic artifact subsystem is added.
31. v0.1 and v0.2 remain single-process, single-node developer-preview deployments.
32. Core Room State is exactly Room Status plus the semantic Membership map; Room Integrity State is operational.
33. The Core reducer alone mutates Core; a pack may veto only join, resume, Access Mode, and Role proposals.
34. Multi-Membership administration validates and commits one final state without an observable invalid intermediate.
35. Departed Membership and archived Room status are irreversible.
36. Only a generation-fenced verifier may restore healthy integrity.
37. Repair never rewrites, skips, or replaces canonical lineage.
38. `ActivityPackV1` has exactly `descriptor`, `initialize`, `reduce`, `view`, and `observe`; no pack callback runs during persistence.
39. Every retained pack digest remains executable and codec-complete; no Room digest changes in place.
40. One exact Action Offer representation supplies Projections, Resets, Observations, Invocation Context, and admission.
